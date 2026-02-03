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
//! ## Module Structure
//!
//! - `window` - Cocoa window management (NSWindow, NSView)
//! - `input` - IOKit HID input handling
//! - `display` - Core Graphics display management
//! - `render` - Metal/OpenGL rendering
//! - `event_loop` - calloop integration for macOS events

// Sub-modules
#[cfg(target_os = "macos")]
pub mod window;
#[cfg(target_os = "macos")]
pub mod input;
#[cfg(target_os = "macos")]
pub mod display;
#[cfg(target_os = "macos")]
pub mod render;
#[cfg(target_os = "macos")]
pub mod event_loop;
#[cfg(target_os = "macos")]
pub mod input_adapter;

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

use crate::backend::{IpcOutputMap, OutputId, RenderResult};
use crate::niri::{Niri, RedrawState, State};
use crate::utils::{get_monotonic_time, logical_output};

#[cfg(target_os = "macos")]
use self::window::CocoaWindow;
#[cfg(target_os = "macos")]
use self::input::InputHandler;
#[cfg(target_os = "macos")]
use self::display::DisplayManager;
#[cfg(target_os = "macos")]
use self::event_loop::{MacOSEventSource, FrameTimer};
#[cfg(target_os = "macos")]
use self::render::MetalRenderer;

/// 🖥️ Display information from macOS
#[derive(Debug, Clone)]
pub struct MacOSDisplay {
    pub display_id: u32,
    pub name: String,
    pub physical_size_mm: Option<(u32, u32)>,
    pub resolution: (u32, u32),
    pub refresh_rate: u32,
    pub scale_factor: f64,
    pub is_main: bool,
}

/// ⌨️ Input event types from macOS
#[derive(Debug, Clone)]
pub enum MacOSInputEvent {
    Key {
        keycode: u16,
        pressed: bool,
        modifiers: MacOSModifiers,
    },
    PointerMotion {
        dx: f64,
        dy: f64,
    },
    PointerMotionAbsolute {
        x: f64,
        y: f64,
    },
    PointerButton {
        button: u32,
        pressed: bool,
    },
    PointerAxis {
        horizontal: f64,
        vertical: f64,
        is_trackpad: bool,
    },
    GesturePinch {
        scale: f64,
        phase: GesturePhase,
    },
    GestureSwipe {
        dx: f64,
        dy: f64,
        fingers: u32,
        phase: GesturePhase,
    },
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
    pub alt: bool,
    pub logo: bool,
    pub caps_lock: bool,
}

/// 🎨 The main macOS backend structure
pub struct MacOS {
    config: Rc<RefCell<Config>>,
    output: Output,
    window_size: Size<i32, smithay::utils::Physical>,
    scale_factor: f64,
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    last_frame_time: Instant,
    target_frame_duration: Duration,
    has_focus: bool,
    input_queue: Vec<MacOSInputEvent>,
    modifiers: MacOSModifiers,
    debug_tint_enabled: bool,
    renderer: Option<GlesRenderer>,

    #[cfg(target_os = "macos")]
    window: Option<CocoaWindow>,
    #[cfg(target_os = "macos")]
    input_handler: Option<InputHandler>,
    #[cfg(target_os = "macos")]
    display_manager: Option<DisplayManager>,
    #[cfg(target_os = "macos")]
    event_source: Option<MacOSEventSource>,
    #[cfg(target_os = "macos")]
    frame_timer: FrameTimer,
    #[cfg(target_os = "macos")]
    metal_renderer: Option<MetalRenderer>,
}

impl MacOS {
    pub fn new(
        config: Rc<RefCell<Config>>,
        event_loop: LoopHandle<State>,
    ) -> Self {
        let _span = tracy_client::span!("MacOS::new");

        info!("🍎 Initializing macOS backend...");
        info!("   Welcome to niri on macOS!");
        info!("   Running Crayons Ltd. Certified(TM)");

        let initial_width = 1280;
        let initial_height = 800;
        let scale_factor = 2.0;

        let output = Output::new(
            "macos".to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Apple".into(),
                model: "macOS Window".into(),
                serial_number: "niri-macos-001".into(),
            },
        );

        let mode = Mode {
            size: Size::from((initial_width, initial_height)),
            refresh: 60_000,
        };
        output.change_current_state(Some(mode), None, None, None);
        output.set_preferred(mode);

        output.user_data().insert_if_missing(|| OutputName {
            connector: "macos".to_string(),
            make: Some("Apple".to_string()),
            model: Some("macOS Window".to_string()),
            serial: None,
        });

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
                vrr_supported: false,
                vrr_enabled: false,
                logical: Some(logical_output(&output)),
            },
        )])));

        // Set up frame callback timer
        let timer = calloop::timer::Timer::immediate();
        event_loop
            .insert_source(timer, |_, _, state| {
                let macos = state.backend.macos();
                if state.niri.output_state.values().any(|s| s.unfinished_animations_remain) {
                    state.niri.queue_redraw(&macos.output);
                }
                calloop::timer::TimeoutAction::Drop
            })
            .unwrap();

        info!("🍎 macOS backend initialized successfully!");
        info!("   Window: {}x{} @ {}x scale", initial_width, initial_height, scale_factor);

        // Set up event source for calloop integration
        #[cfg(target_os = "macos")]
        let event_source = match MacOSEventSource::new() {
            Ok(source) => {
                info!("🔄 macOS event source created");
                Some(source)
            }
            Err(e) => {
                error!("Failed to create event source: {}", e);
                None
            }
        };

        #[cfg(target_os = "macos")]
        if let Some(ref source) = event_source {
            // Register the event source with the calloop
            let queue = source.event_queue();
            if let Err(e) = event_loop.insert_source(
                calloop::channel::channel::<()>().1, // Placeholder
                move |_, _, _state| {
                    // Events will be processed via the main input queue
                },
            ) {
                warn!("Failed to register event source: {:?}", e);
            }
        }

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
            window: None,
            #[cfg(target_os = "macos")]
            input_handler: None,
            #[cfg(target_os = "macos")]
            display_manager: None,
            #[cfg(target_os = "macos")]
            event_source,
            #[cfg(target_os = "macos")]
            frame_timer: FrameTimer::new(60),
            #[cfg(target_os = "macos")]
            metal_renderer: None,
        }
    }

    pub fn init(&mut self, niri: &mut Niri) {
        let _span = tracy_client::span!("MacOS::init");

        info!("🍎 Completing macOS backend initialization...");

        #[cfg(target_os = "macos")]
        {
            // Initialize display manager
            self.display_manager = Some(DisplayManager::new());

            // Detect ProMotion displays and update frame timer
            if let Some(ref dm) = self.display_manager {
                if let Some(main_display) = dm.get_main_display() {
                    if dm.supports_promotion(main_display.id) {
                        let max_fps = dm.max_refresh_rate(main_display.id) / 1000;
                        self.frame_timer.set_target_fps(max_fps);
                        self.target_frame_duration = Duration::from_secs_f64(1.0 / max_fps as f64);
                        info!("🖥️ ProMotion detected, targeting {}Hz", max_fps);
                    }
                }
            }

            // Create the Cocoa window
            match CocoaWindow::new("niri", self.window_size.w as u32, self.window_size.h as u32) {
                Ok(window) => {
                    self.window = Some(window);
                    info!("🪟 Cocoa window created successfully");
                }
                Err(e) => {
                    error!("Failed to create Cocoa window: {}", e);
                }
            }

            // Initialize Metal renderer
            match MetalRenderer::new() {
                Ok(renderer) => {
                    self.metal_renderer = Some(renderer);
                    info!("🎨 Metal renderer initialized");
                }
                Err(e) => {
                    error!("Failed to initialize Metal renderer: {}", e);
                    info!("📝 Will fall back to software rendering");
                }
            }

            // Initialize input handler
            let mut input_handler = InputHandler::new();
            input_handler.start();
            self.input_handler = Some(input_handler);
        }

        niri.add_output(self.output.clone(), None, false);

        info!("🍎 Backend ready! Let's make some pixels dance! 💃");
    }

    pub fn seat_name(&self) -> String {
        "macos".to_owned()
    }

    pub fn with_primary_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer) -> T,
    ) -> Option<T> {
        self.renderer.as_mut().map(f)
    }

    pub fn render(&mut self, niri: &mut Niri, output: &Output) -> RenderResult {
        let _span = tracy_client::span!("MacOS::render");

        // Frame pacing using the frame timer
        #[cfg(target_os = "macos")]
        if !self.frame_timer.should_render() {
            return RenderResult::Skipped;
        }

        #[cfg(not(target_os = "macos"))]
        {
            let elapsed = self.last_frame_time.elapsed();
            if elapsed < self.target_frame_duration {
                return RenderResult::Skipped;
            }
        }

        self.last_frame_time = Instant::now();

        #[cfg(target_os = "macos")]
        self.frame_timer.frame_rendered();

        // Send presentation feedback
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

        // Request window redraw for actual rendering
        #[cfg(target_os = "macos")]
        if let Some(ref window) = self.window {
            window.request_redraw();

            // If we have animations running, schedule next frame
            if output_state.unfinished_animations_remain {
                let next_frame_ns = self.frame_timer.time_until_next_frame();
                trace!("Scheduling next frame in {}ns", next_frame_ns);
            }
        }

        RenderResult::Submitted
    }

    pub fn toggle_debug_tint(&mut self) {
        self.debug_tint_enabled = !self.debug_tint_enabled;
        info!(
            "🎨 Debug tint {}",
            if self.debug_tint_enabled { "enabled" } else { "disabled" }
        );
    }

    pub fn import_dmabuf(&mut self, _dmabuf: &Dmabuf) -> bool {
        warn!("🚫 DMA-BUF import not supported on macOS");
        false
    }

    pub fn ipc_outputs(&self) -> Arc<Mutex<IpcOutputMap>> {
        self.ipc_outputs.clone()
    }

    pub fn handle_resize(&mut self, niri: &mut Niri, width: i32, height: i32) {
        info!("📐 Window resized to {}x{}", width, height);

        self.window_size = Size::from((width, height));

        let mode = Mode {
            size: self.window_size,
            refresh: 60_000,
        };
        self.output.change_current_state(Some(mode), None, None, None);

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

    pub fn handle_key_event(&mut self, keycode: u16, pressed: bool, modifiers: MacOSModifiers) {
        self.modifiers = modifiers;
        self.input_queue.push(MacOSInputEvent::Key {
            keycode,
            pressed,
            modifiers,
        });
    }

    pub fn handle_pointer_motion(&mut self, dx: f64, dy: f64) {
        self.input_queue.push(MacOSInputEvent::PointerMotion { dx, dy });
    }

    pub fn handle_pointer_button(&mut self, button: u32, pressed: bool) {
        self.input_queue.push(MacOSInputEvent::PointerButton { button, pressed });
    }

    pub fn handle_scroll(&mut self, horizontal: f64, vertical: f64, is_trackpad: bool) {
        self.input_queue.push(MacOSInputEvent::PointerAxis {
            horizontal,
            vertical,
            is_trackpad,
        });
    }

    pub fn handle_pinch_gesture(&mut self, scale: f64, phase: GesturePhase) {
        self.input_queue.push(MacOSInputEvent::GesturePinch { scale, phase });
    }

    pub fn handle_swipe_gesture(&mut self, dx: f64, dy: f64, fingers: u32, phase: GesturePhase) {
        self.input_queue.push(MacOSInputEvent::GestureSwipe { dx, dy, fingers, phase });
    }

    pub fn handle_rotate_gesture(&mut self, angle: f64, phase: GesturePhase) {
        self.input_queue.push(MacOSInputEvent::GestureRotate { angle, phase });
    }

    pub fn handle_focus_change(&mut self, focused: bool) {
        self.has_focus = focused;
        info!("🎭 Window focus {}", if focused { "gained" } else { "lost" });
    }

    pub fn handle_close_requested(&self, niri: &mut Niri) {
        info!("🚪 Window close requested");
        niri.stop_signal.stop();
    }

    pub fn drain_input_events(&mut self) -> Vec<MacOSInputEvent> {
        mem::take(&mut self.input_queue)
    }

    /// Processes all queued input events and returns converted events.
    #[cfg(target_os = "macos")]
    pub fn process_input_events(&mut self) -> Vec<input_adapter::ConvertedEvent> {
        use crate::utils::get_monotonic_time;

        let raw_events = self.drain_input_events();
        let time = get_monotonic_time();

        raw_events
            .into_iter()
            .filter_map(|event| input_adapter::convert_input_event(event, time))
            .collect()
    }

    /// Gets input events from the input handler (if any).
    #[cfg(target_os = "macos")]
    pub fn poll_input(&mut self) {
        if let Some(ref handler) = self.input_handler {
            // Drain events from the input handler and queue them
            let events = handler.drain_events();
            for event in events {
                self.input_queue.push(event);
            }
        }
    }

    pub fn output(&self) -> &Output {
        &self.output
    }

    pub fn scale_factor(&self) -> f64 {
        self.scale_factor
    }

    pub fn set_scale_factor(&mut self, scale: f64) {
        if (self.scale_factor - scale).abs() > 0.01 {
            info!("📏 Scale factor changed: {} -> {}", self.scale_factor, scale);
            self.scale_factor = scale;
        }
    }

    #[cfg(target_os = "macos")]
    pub fn enumerate_displays() -> Vec<MacOSDisplay> {
        DisplayManager::enumerate_displays()
    }
}

// ============================================================================
// 🔧 Keycode Translation
// ============================================================================

#[allow(dead_code)]
pub fn macos_keycode_to_evdev(macos_keycode: u16) -> Option<u32> {
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
        0x37 => 125, // Left Command
        0x38 => 42,  // Left Shift
        0x39 => 58,  // Caps Lock
        0x3A => 56,  // Left Alt
        0x3B => 29,  // Left Control
        0x3C => 54,  // Right Shift
        0x3D => 100, // Right Alt
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

#[allow(dead_code)]
pub fn macos_button_to_evdev(macos_button: u32) -> u32 {
    match macos_button {
        0 => 0x110, // BTN_LEFT
        1 => 0x111, // BTN_RIGHT
        2 => 0x112, // BTN_MIDDLE
        n => 0x113 + (n - 3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keycode_translation() {
        assert_eq!(macos_keycode_to_evdev(0x00), Some(30));
        assert_eq!(macos_keycode_to_evdev(0x24), Some(28));
        assert_eq!(macos_keycode_to_evdev(0x35), Some(1));
        assert_eq!(macos_keycode_to_evdev(0x31), Some(57));
        assert_eq!(macos_keycode_to_evdev(0xFF), None);
    }

    #[test]
    fn test_button_translation() {
        assert_eq!(macos_button_to_evdev(0), 0x110);
        assert_eq!(macos_button_to_evdev(1), 0x111);
        assert_eq!(macos_button_to_evdev(2), 0x112);
    }
}
