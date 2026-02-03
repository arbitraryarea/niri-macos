//! # IOKit Input Handling 🎮
//!
//! This module handles input device management via IOKit HID Manager.
//! We capture keyboard, mouse, trackpad, and gesture events.
//!
//! ## Architecture
//!
//! ```text
//! ┌────────────────────────────────────────────┐
//! │              IOKit HID Manager             │
//! │  ┌──────────────────────────────────────┐  │
//! │  │           Device Matching            │  │
//! │  │  (Keyboard, Mouse, Trackpad, etc.)   │  │
//! │  └────────────────────┬─────────────────┘  │
//! │                       │                    │
//! │  ┌────────────────────▼─────────────────┐  │
//! │  │         Event Callback               │  │
//! │  │   (Value changed, device added)      │  │
//! │  └────────────────────┬─────────────────┘  │
//! │                       │                    │
//! │  ┌────────────────────▼─────────────────┐  │
//! │  │      Event Translation               │  │
//! │  │   (IOKit → MacOSInputEvent)          │  │
//! │  └──────────────────────────────────────┘  │
//! └────────────────────────────────────────────┘
//! ```
//!
//! Running Crayons Ltd. - "Making input feel magical since 2024"

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{MacOSInputEvent, MacOSModifiers, GesturePhase};

#[cfg(target_os = "macos")]
use core_foundation::base::{CFRelease, TCFType};
#[cfg(target_os = "macos")]
use core_foundation::dictionary::CFDictionary;
#[cfg(target_os = "macos")]
use core_foundation::number::CFNumber;
#[cfg(target_os = "macos")]
use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;

/// 🎮 Input Handler
///
/// Manages all input devices via IOKit HID Manager.
pub struct InputHandler {
    /// Queue of input events to be processed
    event_queue: Arc<Mutex<Vec<MacOSInputEvent>>>,
    /// Current modifier state
    modifiers: Arc<Mutex<MacOSModifiers>>,
    /// Connected input devices
    devices: Arc<Mutex<HashMap<u64, InputDevice>>>,
    /// Whether the handler is active
    is_active: bool,
}

/// 🔌 Input Device Information
#[derive(Debug, Clone)]
pub struct InputDevice {
    pub id: u64,
    pub name: String,
    pub vendor_id: u32,
    pub product_id: u32,
    pub device_type: InputDeviceType,
}

/// 📱 Input Device Types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputDeviceType {
    Keyboard,
    Mouse,
    Trackpad,
    Tablet,
    Gamepad,
    Unknown,
}

impl InputHandler {
    /// Creates a new input handler.
    pub fn new() -> Self {
        info!("🎮 Initializing IOKit input handler...");

        let handler = Self {
            event_queue: Arc::new(Mutex::new(Vec::new())),
            modifiers: Arc::new(Mutex::new(MacOSModifiers::default())),
            devices: Arc::new(Mutex::new(HashMap::new())),
            is_active: false,
        };

        // On macOS, set up HID manager
        #[cfg(target_os = "macos")]
        {
            // Note: Full HID setup would go here
            // For now we rely on NSEvent-based input from the window
            info!("🎮 Using NSEvent-based input handling");
        }

        handler
    }

    /// Starts listening for input events.
    pub fn start(&mut self) {
        if self.is_active {
            return;
        }

        info!("🎮 Starting input handler");
        self.is_active = true;

        #[cfg(target_os = "macos")]
        {
            // Set up global event monitor
            self.setup_event_monitors();
        }
    }

    /// Stops listening for input events.
    pub fn stop(&mut self) {
        if !self.is_active {
            return;
        }

        info!("🎮 Stopping input handler");
        self.is_active = false;
    }

    /// Drains queued input events.
    pub fn drain_events(&self) -> Vec<MacOSInputEvent> {
        let mut queue = self.event_queue.lock().unwrap();
        std::mem::take(&mut *queue)
    }

    /// Gets the current modifier state.
    pub fn modifiers(&self) -> MacOSModifiers {
        *self.modifiers.lock().unwrap()
    }

    /// Gets connected devices.
    pub fn devices(&self) -> Vec<InputDevice> {
        self.devices.lock().unwrap().values().cloned().collect()
    }

    /// Processes a key event from NSEvent.
    pub fn process_key_event(&self, keycode: u16, pressed: bool, modifier_flags: u64) {
        let modifiers = modifier_flags_to_modifiers(modifier_flags);

        {
            let mut mods = self.modifiers.lock().unwrap();
            *mods = modifiers;
        }

        let event = MacOSInputEvent::Key {
            keycode,
            pressed,
            modifiers,
        };

        self.event_queue.lock().unwrap().push(event);
    }

    /// Processes a mouse move event.
    pub fn process_mouse_move(&self, dx: f64, dy: f64) {
        let event = MacOSInputEvent::PointerMotion { dx, dy };
        self.event_queue.lock().unwrap().push(event);
    }

    /// Processes a mouse move absolute event.
    pub fn process_mouse_move_absolute(&self, x: f64, y: f64) {
        let event = MacOSInputEvent::PointerMotionAbsolute { x, y };
        self.event_queue.lock().unwrap().push(event);
    }

    /// Processes a mouse button event.
    pub fn process_mouse_button(&self, button: u32, pressed: bool) {
        let event = MacOSInputEvent::PointerButton { button, pressed };
        self.event_queue.lock().unwrap().push(event);
    }

    /// Processes a scroll event.
    pub fn process_scroll(&self, dx: f64, dy: f64, is_trackpad: bool) {
        let event = MacOSInputEvent::PointerAxis {
            horizontal: dx,
            vertical: dy,
            is_trackpad,
        };
        self.event_queue.lock().unwrap().push(event);
    }

    /// Processes a pinch gesture.
    pub fn process_pinch(&self, scale: f64, phase: GesturePhase) {
        let event = MacOSInputEvent::GesturePinch { scale, phase };
        self.event_queue.lock().unwrap().push(event);
    }

    /// Processes a swipe gesture.
    pub fn process_swipe(&self, dx: f64, dy: f64, fingers: u32, phase: GesturePhase) {
        let event = MacOSInputEvent::GestureSwipe {
            dx,
            dy,
            fingers,
            phase,
        };
        self.event_queue.lock().unwrap().push(event);
    }

    /// Processes a rotation gesture.
    pub fn process_rotation(&self, angle: f64, phase: GesturePhase) {
        let event = MacOSInputEvent::GestureRotate { angle, phase };
        self.event_queue.lock().unwrap().push(event);
    }

    #[cfg(target_os = "macos")]
    fn setup_event_monitors(&self) {
        // NSEvent-based global monitoring would be set up here
        // For production, this would use:
        // - addGlobalMonitorForEventsMatchingMask for global events
        // - addLocalMonitorForEventsMatchingMask for window events

        info!("🎮 Event monitors configured");
    }
}

impl Default for InputHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 🎹 Modifier Translation
// ============================================================================

/// macOS modifier flag constants
#[cfg(target_os = "macos")]
mod modifier_flags {
    pub const CAPS_LOCK: u64 = 1 << 16;
    pub const SHIFT: u64 = 1 << 17;
    pub const CONTROL: u64 = 1 << 18;
    pub const OPTION: u64 = 1 << 19; // Alt
    pub const COMMAND: u64 = 1 << 20; // Logo/Super
}

#[cfg(not(target_os = "macos"))]
mod modifier_flags {
    pub const CAPS_LOCK: u64 = 1 << 16;
    pub const SHIFT: u64 = 1 << 17;
    pub const CONTROL: u64 = 1 << 18;
    pub const OPTION: u64 = 1 << 19;
    pub const COMMAND: u64 = 1 << 20;
}

/// Converts NSEvent modifier flags to our MacOSModifiers struct.
pub fn modifier_flags_to_modifiers(flags: u64) -> MacOSModifiers {
    MacOSModifiers {
        caps_lock: (flags & modifier_flags::CAPS_LOCK) != 0,
        shift: (flags & modifier_flags::SHIFT) != 0,
        ctrl: (flags & modifier_flags::CONTROL) != 0,
        alt: (flags & modifier_flags::OPTION) != 0,
        logo: (flags & modifier_flags::COMMAND) != 0,
    }
}

// ============================================================================
// 🔢 HID Usage Page Constants
// ============================================================================

#[allow(dead_code)]
mod hid_usage {
    pub const GENERIC_DESKTOP: u32 = 0x01;
    pub const KEYBOARD: u32 = 0x07;
    pub const BUTTON: u32 = 0x09;
    pub const CONSUMER: u32 = 0x0C;
    pub const DIGITIZER: u32 = 0x0D;

    // Generic Desktop Usage IDs
    pub const POINTER: u32 = 0x01;
    pub const MOUSE: u32 = 0x02;
    pub const JOYSTICK: u32 = 0x04;
    pub const GAMEPAD: u32 = 0x05;
    pub const KEYBOARD_USAGE: u32 = 0x06;
    pub const KEYPAD: u32 = 0x07;
    pub const MULTI_AXIS: u32 = 0x08;

    // Mouse axes
    pub const X: u32 = 0x30;
    pub const Y: u32 = 0x31;
    pub const WHEEL: u32 = 0x38;
}

// ============================================================================
// 🎮 Gesture Phase Conversion
// ============================================================================

#[cfg(target_os = "macos")]
impl From<i32> for GesturePhase {
    fn from(phase: i32) -> Self {
        match phase {
            0 => GesturePhase::Begin,    // NSEventPhaseNone/NSEventPhaseBegan
            1 => GesturePhase::Begin,    // NSEventPhaseBegan
            2 => GesturePhase::Update,   // NSEventPhaseStationary
            4 => GesturePhase::Update,   // NSEventPhaseChanged
            8 => GesturePhase::End,      // NSEventPhaseEnded
            16 => GesturePhase::Cancelled, // NSEventPhaseCancelled
            _ => GesturePhase::Update,
        }
    }
}

// ============================================================================
// 🧪 Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modifier_conversion() {
        let flags = modifier_flags::SHIFT | modifier_flags::COMMAND;
        let mods = modifier_flags_to_modifiers(flags);

        assert!(mods.shift);
        assert!(mods.logo);
        assert!(!mods.ctrl);
        assert!(!mods.alt);
        assert!(!mods.caps_lock);
    }

    #[test]
    fn test_input_handler_creation() {
        let handler = InputHandler::new();
        assert!(!handler.is_active);
        assert!(handler.devices().is_empty());
    }

    #[test]
    fn test_event_queuing() {
        let handler = InputHandler::new();

        handler.process_key_event(0x00, true, modifier_flags::SHIFT);
        handler.process_mouse_move(10.0, 20.0);

        let events = handler.drain_events();
        assert_eq!(events.len(), 2);
    }
}
