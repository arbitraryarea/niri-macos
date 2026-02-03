//! # macOS Native Backend - Where Wayland Meets the Walled Garden 🍎
//!
//! Welcome, brave soul, to the forbidden fruit of compositor development!
//! This module bridges the gap between niri's Wayland-based architecture
//! and Apple's Cocoa/AppKit ecosystem.
//!
//! ## Architecture Overview
//!
//! Unlike the TTY backend which talks directly to DRM/KMS/libinput,
//! we're running as a regular macOS application:
//!
//! ```text
//!                    ┌─────────────────────────┐
//!                    │    niri (Wayland)       │
//!                    │    ┌───────────────┐    │
//!                    │    │  Layout/UI    │    │
//!                    │    └───────┬───────┘    │
//!                    │            │            │
//!                    │    ┌───────▼───────┐    │
//!                    │    │ macOS Backend │    │
//!                    │    └───────┬───────┘    │
//!                    └────────────┼────────────┘
//!                                 │
//!          ┌──────────────────────┼──────────────────────┐
//!          │                      │                      │
//!    ┌─────▼─────┐         ┌──────▼──────┐        ┌──────▼──────┐
//!    │   Cocoa   │         │    Metal    │        │   IOKit     │
//!    │  (Window) │         │  (Render)   │        │   (Input)   │
//!    └───────────┘         └─────────────┘        └─────────────┘
//! ```
//!
//! ## Running Crayons Ltd. Design Philosophy
//!
//! - "If it compiles, it's probably art"
//! - "Comments should spark joy"
//! - "Error messages are love letters to future developers"
//!
//! ## Technical Notes
//!
//! This backend renders to a single macOS window. Wayland clients connect
//! to our embedded server and we composite their content using Metal.
//! Think of it as a Wayland server in a cozy macOS wrapper.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use niri_config::{Config, OutputName};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::utils::Size;
use smithay::wayland::presentation::Refresh;

use super::{IpcOutputMap, OutputId, RenderResult};
use crate::niri::{Niri, RedrawState, State};
use crate::utils::{get_monotonic_time, logical_output};

/// 🖥️ Display information from macOS
///
/// Represents a physical display attached to the system.
/// On macOS, we query these via Core Graphics (CGDisplay).
#[derive(Debug, Clone)]
pub struct MacOSDisplay {
    /// The Core Graphics display ID
    pub display_id: u32,
    /// Display name (e.g., "Built-in Retina Display")
    pub name: String,
    /// Physical size in millimeters (if available)
    pub physical_size_mm: Option<(u32, u32)>,
    /// Current resolution
    pub resolution: (u32, u32),
    /// Refresh rate in millihertz (e.g., 60000 = 60Hz)
    pub refresh_rate: u32,
    /// Scale factor (e.g., 2.0 for Retina)
    pub scale_factor: f64,
    /// Whether this is the main display
    pub is_main: bool,
}

/// ⌨️ Input event types from macOS
///
/// These get translated to smithay input events.
#[derive(Debug, Clone)]
pub enum MacOSInputEvent {
    /// Keyboard key press/release
    Key {
        keycode: u16,
        pressed: bool,
        modifiers: MacOSModifiers,
    },
    /// Mouse/trackpad movement
    PointerMotion {
        dx: f64,
        dy: f64,
    },
    /// Absolute pointer position
    PointerMotionAbsolute {
        x: f64,
        y: f64,
    },
    /// Mouse button press/release
    PointerButton {
        button: u32,
        pressed: bool,
    },
    /// Scroll wheel / trackpad scroll
    PointerAxis {
        horizontal: f64,
        vertical: f64,
        /// Whether this is from a trackpad (enables smooth scrolling)
        is_trackpad: bool,
    },
    /// Trackpad pinch gesture (zoom)
    GesturePinch {
        scale: f64,
        phase: GesturePhase,
    },
    /// Trackpad swipe gesture
    GestureSwipe {
        dx: f64,
        dy: f64,
        fingers: u32,
        phase: GesturePhase,
    },
    /// Trackpad rotation gesture
    GestureRotate {
        angle: f64,
        phase: GesturePhase,
    },
}

/// 🎭 Gesture phases
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GesturePhase {
    Begin,
    Update,
    End,
    Cancelled,
}

/// 🎹 Modifier key state
#[derive(Debug, Clone, Copy, Default)]
pub struct MacOSModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,    // Option key on Mac
    pub logo: bool,   // Command key on Mac
    pub caps_lock: bool,
}

/// 🎨 The main macOS backend structure
///
/// This is where the magic happens! We manage:
/// - A Cocoa window for display
/// - Metal rendering pipeline
/// - IOKit-based input handling
/// - Display hotplug detection
pub struct MacOS {
    /// Configuration reference (shared with the compositor)
    config: Rc<RefCell<Config>>,

    /// Our primary output representing the macOS window
    output: Output,

    /// The window dimensions (can be resized)
    window_size: Size<i32, smithay::utils::Physical>,

    /// Scale factor for HiDPI (Retina) displays
    scale_factor: f64,

    /// IPC output information for niri clients
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,

    /// Track when we last rendered (for frame pacing)
    last_frame_time: Instant,

    /// Target frame duration (1/60s default, adjusted for ProMotion)
    target_frame_duration: Duration,

    /// Whether the window currently has focus
    has_focus: bool,

    /// Queued input events to process
    input_queue: Vec<MacOSInputEvent>,

    /// Current modifier state
    modifiers: MacOSModifiers,

    /// Debug tint flag (for visual debugging)
    debug_tint_enabled: bool,

    /// 🦀 Placeholder for the actual renderer
    /// In a full implementation, this would be MetalRenderer or GlesRenderer
    renderer: Option<GlesRenderer>,

    /// 🪟 Window handle (platform-specific)
    /// Wrapped in Option for lazy initialization
    #[cfg(target_os = "macos")]
    window_handle: Option<MacOSWindowHandle>,
}

/// 🪟 Native macOS window handle
///
/// Wraps the Objective-C objects needed to manage the window.
/// We use raw pointers because Cocoa objects are reference-counted
/// separately from Rust's ownership model.
#[cfg(target_os = "macos")]
pub struct MacOSWindowHandle {
    // Note: In the actual implementation, these would be:
    // ns_window: *mut Object,      // NSWindow
    // ns_view: *mut Object,        // NSView (our Metal view)
    // metal_layer: *mut Object,    // CAMetalLayer
    _marker: std::marker::PhantomData<()>,
}

#[cfg(target_os = "macos")]
impl MacOSWindowHandle {
    /// Creates a new window with the specified title and size
    pub fn new(title: &str, width: u32, height: u32) -> Self {
        // TODO: Actual Cocoa implementation
        // This would call NSApplication.shared(), create NSWindow, etc.
        info!("🍎 Creating macOS window: '{}' ({}x{})", title, width, height);
        Self {
            _marker: std::marker::PhantomData,
        }
    }

    /// Requests a redraw on the next display refresh
    pub fn request_redraw(&self) {
        // TODO: Call setNeedsDisplay: on the view
        trace!("🎨 Requesting window redraw");
    }

    /// Updates the window title
    pub fn set_title(&self, _title: &str) {
        // TODO: [window setTitle: title]
    }

    /// Gets the current window size
    pub fn size(&self) -> (u32, u32) {
        // TODO: Query actual window size
        (1280, 800)
    }

    /// Gets the backing scale factor (2.0 for Retina)
    pub fn scale_factor(&self) -> f64 {
        // TODO: [window backingScaleFactor]
        2.0
    }
}

impl MacOS {
    /// 🎬 Creates a new macOS backend instance
    ///
    /// This sets up:
    /// - The Cocoa application (if not already running)
    /// - A native window for rendering
    /// - Metal or OpenGL context
    /// - Input event handlers
    ///
    /// # Arguments
    ///
    /// * `config` - The compositor configuration
    /// * `event_loop` - The calloop event loop handle
    ///
    /// # Panics
    ///
    /// Panics if unable to create the window or graphics context.
    /// (We panic rather than return errors because if these fail,
    /// there's nothing useful we can do anyway.)
    pub fn new(
        config: Rc<RefCell<Config>>,
        event_loop: LoopHandle<State>,
    ) -> Self {
        let _span = tracy_client::span!("MacOS::new");

        info!("🍎 Initializing macOS backend...");
        info!("   Welcome to niri on macOS!");
        info!("   Running Crayons Ltd. Certified(TM)");

        // Default window size - a nice 16:10 that looks good on Mac
        let initial_width = 1280;
        let initial_height = 800;
        let scale_factor = 2.0; // Assume Retina by default

        // Create the smithay Output representing our window
        let output = Output::new(
            "macos".to_string(),
            PhysicalProperties {
                size: (0, 0).into(), // We'll set this properly later
                subpixel: Subpixel::Unknown, // macOS handles subpixel rendering
                make: "Apple".into(),
                model: "macOS Window".into(),
                serial_number: "niri-macos-001".into(),
            },
        );

        // Set up the display mode
        let mode = Mode {
            size: Size::from((initial_width, initial_height)),
            refresh: 60_000, // 60Hz default, ProMotion can go higher
        };
        output.change_current_state(Some(mode), None, None, None);
        output.set_preferred(mode);

        // Store output name for IPC
        output.user_data().insert_if_missing(|| OutputName {
            connector: "macos".to_string(),
            make: Some("Apple".to_string()),
            model: Some("macOS Window".to_string()),
            serial: None,
        });

        // Build IPC output info
        let physical_properties = output.physical_properties();
        let ipc_outputs = Arc::new(Mutex::new(HashMap::from([(
            OutputId::next(),
            niri_ipc::Output {
                name: output.name(),
                make: physical_properties.make,
                model: physical_properties.model,
                serial: None,
                physical_size: None,
                modes: vec![niri_ipc::Mode {
                    width: initial_width as u16,
                    height: initial_height as u16,
                    refresh_rate: 60_000,
                    is_preferred: true,
                }],
                current_mode: Some(0),
                is_custom_mode: true,
                vrr_supported: false, // TODO: Support ProMotion VRR
                vrr_enabled: false,
                logical: Some(logical_output(&output)),
            },
        )])));

        // Set up event loop integration
        // In the full implementation, we'd register:
        // 1. A file descriptor for Cocoa event notifications
        // 2. A timer for frame pacing
        // 3. Display link callbacks for vsync

        // Create timer for frame callbacks (placeholder)
        let timer = calloop::timer::Timer::immediate();
        event_loop
            .insert_source(timer, |_, _, state| {
                // Check if animations need another frame
                let macos = state.backend.macos();
                if state.niri.output_state.values().any(|s| s.unfinished_animations_remain) {
                    state.niri.queue_redraw(&macos.output);
                }
                calloop::timer::TimeoutAction::Drop
            })
            .unwrap();

        info!("🍎 macOS backend initialized successfully!");
        info!("   Window: {}x{} @ {}x scale", initial_width, initial_height, scale_factor);

        Self {
            config,
            output,
            window_size: Size::from((initial_width, initial_height)),
            scale_factor,
            ipc_outputs,
            last_frame_time: Instant::now(),
            target_frame_duration: Duration::from_secs_f64(1.0 / 60.0),
            has_focus: true,
            input_queue: Vec::new(),
            modifiers: MacOSModifiers::default(),
            debug_tint_enabled: false,
            renderer: None,
            #[cfg(target_os = "macos")]
            window_handle: None,
        }
    }

    /// 🚀 Initializes the backend after the compositor is ready
    ///
    /// This is called by the compositor after State is fully constructed.
    /// We use this to:
    /// - Bind the renderer to the Wayland display
    /// - Initialize custom shaders
    /// - Add our output to the compositor
    pub fn init(&mut self, niri: &mut Niri) {
        let _span = tracy_client::span!("MacOS::init");

        info!("🍎 Completing macOS backend initialization...");

        // In the full implementation:
        // 1. Create Metal/GL renderer
        // 2. Bind to Wayland display
        // 3. Initialize shaders

        // Add our output to the compositor
        niri.add_output(self.output.clone(), None, false);

        info!("🍎 Backend ready! Let's make some pixels dance! 💃");
    }

    /// 🪑 Returns the seat name for input devices
    ///
    /// On macOS, we have a single "seat" representing the local input.
    pub fn seat_name(&self) -> String {
        "macos".to_owned()
    }

    /// 🎨 Provides access to the primary renderer
    ///
    /// This allows the compositor to perform render operations
    /// like importing textures, setting up shaders, etc.
    pub fn with_primary_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer) -> T,
    ) -> Option<T> {
        self.renderer.as_mut().map(f)
    }

    /// 🖼️ Renders a frame to the output
    ///
    /// This is the heart of the rendering pipeline:
    /// 1. Gather all render elements from the compositor
    /// 2. Render them to a Metal texture
    /// 3. Present the texture to the window
    /// 4. Handle presentation feedback
    ///
    /// # Returns
    ///
    /// - `RenderResult::Submitted` - Frame was rendered and presented
    /// - `RenderResult::NoDamage` - No changes, frame skipped
    /// - `RenderResult::Skipped` - Error or other reason to skip
    pub fn render(&mut self, niri: &mut Niri, output: &Output) -> RenderResult {
        let _span = tracy_client::span!("MacOS::render");

        // Frame pacing - don't render faster than the display can show
        let elapsed = self.last_frame_time.elapsed();
        if elapsed < self.target_frame_duration {
            // We could sleep here, but better to let the event loop handle it
            return RenderResult::Skipped;
        }

        // TODO: In the full implementation:
        // 1. Get render elements from niri.render()
        // 2. Render to Metal texture
        // 3. Present drawable

        // For now, just handle the timing and callbacks
        self.last_frame_time = Instant::now();

        // Send presentation feedback to Wayland clients
        // This tells them when their content was displayed
        let states = smithay::backend::renderer::element::RenderElementStates::default();
        let mut presentation_feedbacks = niri.take_presentation_feedbacks(output, &states);
        presentation_feedbacks.presented::<_, smithay::utils::Monotonic>(
            get_monotonic_time(),
            Refresh::Unknown,
            0,
            wp_presentation_feedback::Kind::empty(),
        );

        // Update output state
        let output_state = niri.output_state.get_mut(output).unwrap();
        match mem::replace(&mut output_state.redraw_state, RedrawState::Idle) {
            RedrawState::Idle => unreachable!("render called without queued redraw"),
            RedrawState::Queued => (),
            RedrawState::WaitingForVBlank { .. } => unreachable!(),
            RedrawState::WaitingForEstimatedVBlank(_) => unreachable!(),
            RedrawState::WaitingForEstimatedVBlankAndQueued(_) => unreachable!(),
        }

        output_state.frame_callback_sequence =
            output_state.frame_callback_sequence.wrapping_add(1);

        // Request another frame if animations are running
        if output_state.unfinished_animations_remain {
            #[cfg(target_os = "macos")]
            if let Some(ref window) = self.window_handle {
                window.request_redraw();
            }
        }

        RenderResult::Submitted
    }

    /// 🔲 Toggles the debug tint overlay
    ///
    /// When enabled, this applies a colored tint to rendered content
    /// to help visualize damage regions and render order.
    pub fn toggle_debug_tint(&mut self) {
        self.debug_tint_enabled = !self.debug_tint_enabled;
        info!(
            "🎨 Debug tint {}",
            if self.debug_tint_enabled { "enabled" } else { "disabled" }
        );
        // TODO: Apply to renderer
    }

    /// 📦 Imports a DMA-BUF for use in rendering
    ///
    /// DMA-BUFs are Linux's way of sharing GPU buffers.
    /// On macOS, we'd need to convert these to IOSurfaces.
    /// For now, we don't support this (most macOS clients don't use dmabuf anyway).
    pub fn import_dmabuf(&mut self, _dmabuf: &Dmabuf) -> bool {
        warn!("🚫 DMA-BUF import not supported on macOS");
        warn!("   (This is expected - macOS clients use different buffer sharing)");
        false
    }

    /// 📊 Returns the IPC output map
    ///
    /// This is used by the niri IPC server to report output information.
    pub fn ipc_outputs(&self) -> Arc<Mutex<IpcOutputMap>> {
        self.ipc_outputs.clone()
    }

    /// 📐 Handles window resize events from Cocoa
    ///
    /// Called when the user resizes the window or the display changes.
    pub fn handle_resize(&mut self, niri: &mut Niri, width: i32, height: i32) {
        info!("📐 Window resized to {}x{}", width, height);

        self.window_size = Size::from((width, height));

        // Update the output mode
        let mode = Mode {
            size: self.window_size,
            refresh: 60_000,
        };
        self.output.change_current_state(Some(mode), None, None, None);

        // Update IPC outputs
        {
            let mut ipc_outputs = self.ipc_outputs.lock().unwrap();
            if let Some(output) = ipc_outputs.values_mut().next() {
                output.modes[0].width = width.max(0) as u16;
                output.modes[0].height = height.max(0) as u16;
                if let Some(logical) = output.logical.as_mut() {
                    logical.width = width as u32;
                    logical.height = height as u32;
                }
            }
            niri.ipc_outputs_changed = true;
        }

        niri.output_resized(&self.output);
    }

    /// ⌨️ Processes a keyboard event
    ///
    /// Translates macOS key codes to Wayland key codes and
    /// dispatches to the compositor's input handling.
    pub fn handle_key_event(
        &mut self,
        keycode: u16,
        pressed: bool,
        modifiers: MacOSModifiers,
    ) {
        self.modifiers = modifiers;
        self.input_queue.push(MacOSInputEvent::Key {
            keycode,
            pressed,
            modifiers,
        });
    }

    /// 🖱️ Processes pointer motion
    pub fn handle_pointer_motion(&mut self, dx: f64, dy: f64) {
        self.input_queue.push(MacOSInputEvent::PointerMotion { dx, dy });
    }

    /// 🎯 Processes pointer button press/release
    pub fn handle_pointer_button(&mut self, button: u32, pressed: bool) {
        self.input_queue.push(MacOSInputEvent::PointerButton { button, pressed });
    }

    /// 📜 Processes scroll events
    pub fn handle_scroll(&mut self, horizontal: f64, vertical: f64, is_trackpad: bool) {
        self.input_queue.push(MacOSInputEvent::PointerAxis {
            horizontal,
            vertical,
            is_trackpad,
        });
    }

    /// 🔎 Processes pinch gestures (zoom)
    pub fn handle_pinch_gesture(&mut self, scale: f64, phase: GesturePhase) {
        self.input_queue.push(MacOSInputEvent::GesturePinch { scale, phase });
    }

    /// 👆 Processes swipe gestures
    pub fn handle_swipe_gesture(&mut self, dx: f64, dy: f64, fingers: u32, phase: GesturePhase) {
        self.input_queue.push(MacOSInputEvent::GestureSwipe {
            dx,
            dy,
            fingers,
            phase,
        });
    }

    /// 🔄 Processes rotation gestures
    pub fn handle_rotate_gesture(&mut self, angle: f64, phase: GesturePhase) {
        self.input_queue.push(MacOSInputEvent::GestureRotate { angle, phase });
    }

    /// 🎭 Handles focus changes
    pub fn handle_focus_change(&mut self, focused: bool) {
        self.has_focus = focused;
        info!(
            "🎭 Window focus {}",
            if focused { "gained" } else { "lost" }
        );
    }

    /// 🚪 Handles window close request
    ///
    /// Called when the user clicks the red close button.
    /// We pass this to the compositor to initiate shutdown.
    pub fn handle_close_requested(&self, niri: &mut Niri) {
        info!("🚪 Window close requested");
        niri.stop_signal.stop();
    }

    /// 🔄 Drains the input event queue
    ///
    /// Returns all queued input events for processing by the compositor.
    pub fn drain_input_events(&mut self) -> Vec<MacOSInputEvent> {
        mem::take(&mut self.input_queue)
    }

    /// 📍 Returns the output (for use by the compositor)
    pub fn output(&self) -> &Output {
        &self.output
    }

    /// 📏 Returns current scale factor
    pub fn scale_factor(&self) -> f64 {
        self.scale_factor
    }

    /// 🎥 Updates the scale factor (for display changes)
    pub fn set_scale_factor(&mut self, scale: f64) {
        if (self.scale_factor - scale).abs() > 0.01 {
            info!("📏 Scale factor changed: {} -> {}", self.scale_factor, scale);
            self.scale_factor = scale;
            // TODO: Notify compositor of scale change
        }
    }

    /// 🖥️ Enumerates connected displays
    ///
    /// Returns information about all displays attached to the system.
    /// This is useful for multi-monitor support in the future.
    #[cfg(target_os = "macos")]
    pub fn enumerate_displays() -> Vec<MacOSDisplay> {
        // TODO: Implement using CGGetActiveDisplayList and CGDisplayCopyDisplayMode
        vec![MacOSDisplay {
            display_id: 0,
            name: "Built-in Display".to_string(),
            physical_size_mm: Some((286, 179)), // 13" MacBook
            resolution: (2560, 1600),
            refresh_rate: 60_000,
            scale_factor: 2.0,
            is_main: true,
        }]
    }
}

// ============================================================================
// 🔧 Utility Functions
// ============================================================================

/// 🍎 Translates macOS virtual key codes to Linux evdev codes
///
/// macOS uses its own key code system (defined in Events.h).
/// We need to translate these to the evdev codes that Wayland expects.
///
/// # Arguments
///
/// * `macos_keycode` - The macOS virtual key code
///
/// # Returns
///
/// The corresponding Linux evdev key code, or None if unknown.
#[allow(dead_code)]
pub fn macos_keycode_to_evdev(macos_keycode: u16) -> Option<u32> {
    // This is a partial mapping of common keys
    // Full mapping would include all 128 macOS key codes
    Some(match macos_keycode {
        0x00 => 30,  // A
        0x01 => 31,  // S
        0x02 => 32,  // D
        0x03 => 33,  // F
        0x04 => 35,  // H
        0x05 => 34,  // G
        0x06 => 44,  // Z
        0x07 => 45,  // X
        0x08 => 46,  // C
        0x09 => 47,  // V
        0x0B => 48,  // B
        0x0C => 16,  // Q
        0x0D => 17,  // W
        0x0E => 18,  // E
        0x0F => 19,  // R
        0x10 => 21,  // Y
        0x11 => 20,  // T
        0x12 => 2,   // 1
        0x13 => 3,   // 2
        0x14 => 4,   // 3
        0x15 => 5,   // 4
        0x16 => 7,   // 6
        0x17 => 6,   // 5
        0x18 => 13,  // =
        0x19 => 10,  // 9
        0x1A => 8,   // 7
        0x1B => 12,  // -
        0x1C => 9,   // 8
        0x1D => 11,  // 0
        0x1E => 27,  // ]
        0x1F => 24,  // O
        0x20 => 22,  // U
        0x21 => 26,  // [
        0x22 => 23,  // I
        0x23 => 25,  // P
        0x24 => 28,  // Return
        0x25 => 38,  // L
        0x26 => 36,  // J
        0x27 => 40,  // '
        0x28 => 37,  // K
        0x29 => 39,  // ;
        0x2A => 43,  // \
        0x2B => 51,  // ,
        0x2C => 52,  // /
        0x2D => 49,  // N
        0x2E => 50,  // M
        0x2F => 52,  // .
        0x30 => 15,  // Tab
        0x31 => 57,  // Space
        0x32 => 41,  // `
        0x33 => 14,  // Backspace
        0x35 => 1,   // Escape
        0x37 => 125, // Left Command -> Left Meta
        0x38 => 42,  // Left Shift
        0x39 => 58,  // Caps Lock
        0x3A => 56,  // Left Alt (Option)
        0x3B => 29,  // Left Control
        0x3C => 54,  // Right Shift
        0x3D => 100, // Right Alt (Option)
        0x3E => 97,  // Right Control
        0x7A => 59,  // F1
        0x78 => 60,  // F2
        0x63 => 61,  // F3
        0x76 => 62,  // F4
        0x60 => 63,  // F5
        0x61 => 64,  // F6
        0x62 => 65,  // F7
        0x64 => 66,  // F8
        0x65 => 67,  // F9
        0x6D => 68,  // F10
        0x67 => 87,  // F11
        0x6F => 88,  // F12
        0x7B => 105, // Left Arrow
        0x7C => 106, // Right Arrow
        0x7D => 108, // Down Arrow
        0x7E => 103, // Up Arrow
        _ => return None,
    })
}

/// 🖱️ Translates macOS mouse button numbers to Linux button codes
///
/// macOS button numbers:
/// - 0: Left
/// - 1: Right
/// - 2: Middle
/// - 3+: Extra buttons
#[allow(dead_code)]
pub fn macos_button_to_evdev(macos_button: u32) -> u32 {
    match macos_button {
        0 => 0x110, // BTN_LEFT
        1 => 0x111, // BTN_RIGHT
        2 => 0x112, // BTN_MIDDLE
        n => 0x113 + (n - 3), // BTN_SIDE and up
    }
}

// ============================================================================
// 🧪 Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keycode_translation() {
        // Test some common keys
        assert_eq!(macos_keycode_to_evdev(0x00), Some(30)); // A
        assert_eq!(macos_keycode_to_evdev(0x24), Some(28)); // Return
        assert_eq!(macos_keycode_to_evdev(0x35), Some(1));  // Escape
        assert_eq!(macos_keycode_to_evdev(0x31), Some(57)); // Space

        // Unknown keycode
        assert_eq!(macos_keycode_to_evdev(0xFF), None);
    }

    #[test]
    fn test_button_translation() {
        assert_eq!(macos_button_to_evdev(0), 0x110); // Left
        assert_eq!(macos_button_to_evdev(1), 0x111); // Right
        assert_eq!(macos_button_to_evdev(2), 0x112); // Middle
    }

    #[test]
    fn test_gesture_phase_equality() {
        assert_eq!(GesturePhase::Begin, GesturePhase::Begin);
        assert_ne!(GesturePhase::Begin, GesturePhase::End);
    }

    #[test]
    fn test_modifiers_default() {
        let mods = MacOSModifiers::default();
        assert!(!mods.shift);
        assert!(!mods.ctrl);
        assert!(!mods.alt);
        assert!(!mods.logo);
        assert!(!mods.caps_lock);
    }
}
