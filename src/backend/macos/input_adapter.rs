//! # Input Event Adapter 🎮
//!
//! This module translates macOS input events to smithay's input events.
//! We bridge the gap between macOS's event model and Wayland's input protocol.
//!
//! ## Event Flow
//!
//! ```text
//! MacOSInputEvent -> InputAdapter -> Smithay InputEvent -> Wayland client
//! ```
//!
//! Running Crayons Ltd. - "Translating clicks to packets since 2024"

use std::time::Duration;

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Device, DeviceCapability, Event,
    GestureBeginEvent, GestureEndEvent, GesturePinchUpdateEvent, GestureSwipeUpdateEvent,
    InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    PointerMotionEvent, UnusedEvent,
};
use smithay::utils::{Logical, Point, Size};

use super::{MacOSInputEvent, MacOSModifiers, GesturePhase, macos_keycode_to_evdev, macos_button_to_evdev};

/// 🎮 Virtual macOS Input Device
///
/// Represents a macOS input device for smithay's input handling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MacOSDevice {
    name: String,
    capabilities: Vec<DeviceCapability>,
}

impl MacOSDevice {
    /// Creates a new virtual device representing all macOS input.
    pub fn new() -> Self {
        Self {
            name: "macOS Input".to_string(),
            capabilities: vec![
                DeviceCapability::Keyboard,
                DeviceCapability::Pointer,
                DeviceCapability::Touch,
            ],
        }
    }

    /// Creates a keyboard device.
    pub fn keyboard() -> Self {
        Self {
            name: "macOS Keyboard".to_string(),
            capabilities: vec![DeviceCapability::Keyboard],
        }
    }

    /// Creates a pointer device (mouse/trackpad).
    pub fn pointer() -> Self {
        Self {
            name: "macOS Pointer".to_string(),
            capabilities: vec![DeviceCapability::Pointer],
        }
    }
}

impl Default for MacOSDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for MacOSDevice {
    fn id(&self) -> String {
        self.name.clone()
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        // Apple's vendor ID, generic product ID
        Some((0x05AC, 0x0001))
    }

    fn syspath(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// 🕐 Event with timestamp
pub struct TimestampedEvent<E> {
    pub event: E,
    pub time: Duration,
}

impl<E> TimestampedEvent<E> {
    pub fn new(event: E, time: Duration) -> Self {
        Self { event, time }
    }
}

/// ⌨️ Keyboard Event Wrapper
pub struct MacOSKeyboardEvent {
    pub keycode: u32,
    pub state: KeyState,
    pub time: Duration,
    pub device: MacOSDevice,
}

impl Event for MacOSKeyboardEvent {
    fn time(&self) -> Duration {
        self.time
    }

    fn device(&self) -> MacOSDevice {
        self.device.clone()
    }
}

impl KeyboardKeyEvent for MacOSKeyboardEvent {
    fn key_code(&self) -> u32 {
        self.keycode
    }

    fn state(&self) -> KeyState {
        self.state
    }
}

/// 🖱️ Pointer Motion Event Wrapper
pub struct MacOSPointerMotionEvent {
    pub dx: f64,
    pub dy: f64,
    pub time: Duration,
    pub device: MacOSDevice,
}

impl Event for MacOSPointerMotionEvent {
    fn time(&self) -> Duration {
        self.time
    }

    fn device(&self) -> MacOSDevice {
        self.device.clone()
    }
}

impl PointerMotionEvent for MacOSPointerMotionEvent {
    fn delta_x(&self) -> f64 {
        self.dx
    }

    fn delta_y(&self) -> f64 {
        self.dy
    }

    fn delta_x_unaccel(&self) -> f64 {
        self.dx
    }

    fn delta_y_unaccel(&self) -> f64 {
        self.dy
    }
}

/// 🎯 Pointer Button Event Wrapper
pub struct MacOSPointerButtonEvent {
    pub button: u32,
    pub state: ButtonState,
    pub time: Duration,
    pub device: MacOSDevice,
}

impl Event for MacOSPointerButtonEvent {
    fn time(&self) -> Duration {
        self.time
    }

    fn device(&self) -> MacOSDevice {
        self.device.clone()
    }
}

impl PointerButtonEvent for MacOSPointerButtonEvent {
    fn button_code(&self) -> u32 {
        self.button
    }

    fn state(&self) -> ButtonState {
        self.state
    }
}

/// 📜 Pointer Axis Event Wrapper
pub struct MacOSPointerAxisEvent {
    pub horizontal: f64,
    pub vertical: f64,
    pub is_trackpad: bool,
    pub time: Duration,
    pub device: MacOSDevice,
}

impl Event for MacOSPointerAxisEvent {
    fn time(&self) -> Duration {
        self.time
    }

    fn device(&self) -> MacOSDevice {
        self.device.clone()
    }
}

impl PointerAxisEvent for MacOSPointerAxisEvent {
    fn amount(&self, axis: Axis) -> Option<f64> {
        match axis {
            Axis::Horizontal => {
                if self.horizontal.abs() > 0.001 {
                    Some(self.horizontal)
                } else {
                    None
                }
            }
            Axis::Vertical => {
                if self.vertical.abs() > 0.001 {
                    Some(self.vertical)
                } else {
                    None
                }
            }
        }
    }

    fn amount_v120(&self, axis: Axis) -> Option<f64> {
        // Convert to v120 discrete scrolling
        // v120 = 120 units per wheel click
        self.amount(axis).map(|v| v * 15.0) // Approximate conversion
    }

    fn source(&self) -> AxisSource {
        if self.is_trackpad {
            AxisSource::Finger
        } else {
            AxisSource::Wheel
        }
    }

    fn relative_direction(&self, _axis: Axis) -> smithay::backend::input::AxisRelativeDirection {
        smithay::backend::input::AxisRelativeDirection::Identical
    }
}

/// 🔎 Pinch Gesture Event Wrapper
pub struct MacOSGesturePinchEvent {
    pub scale: f64,
    pub phase: GesturePhase,
    pub time: Duration,
    pub device: MacOSDevice,
}

impl Event for MacOSGesturePinchEvent {
    fn time(&self) -> Duration {
        self.time
    }

    fn device(&self) -> MacOSDevice {
        self.device.clone()
    }
}

/// 👆 Swipe Gesture Event Wrapper
pub struct MacOSGestureSwipeEvent {
    pub dx: f64,
    pub dy: f64,
    pub fingers: u32,
    pub phase: GesturePhase,
    pub time: Duration,
    pub device: MacOSDevice,
}

impl Event for MacOSGestureSwipeEvent {
    fn time(&self) -> Duration {
        self.time
    }

    fn device(&self) -> MacOSDevice {
        self.device.clone()
    }
}

// ============================================================================
// 🔄 Event Conversion
// ============================================================================

/// Converts a MacOSInputEvent to an InputEvent for smithay.
pub fn convert_input_event(
    event: MacOSInputEvent,
    time: Duration,
) -> Option<ConvertedEvent> {
    let device = MacOSDevice::new();

    match event {
        MacOSInputEvent::Key { keycode, pressed, modifiers: _ } => {
            let evdev_code = macos_keycode_to_evdev(keycode)?;
            Some(ConvertedEvent::Keyboard(MacOSKeyboardEvent {
                keycode: evdev_code,
                state: if pressed { KeyState::Pressed } else { KeyState::Released },
                time,
                device: MacOSDevice::keyboard(),
            }))
        }
        MacOSInputEvent::PointerMotion { dx, dy } => {
            Some(ConvertedEvent::PointerMotion(MacOSPointerMotionEvent {
                dx,
                dy,
                time,
                device: MacOSDevice::pointer(),
            }))
        }
        MacOSInputEvent::PointerMotionAbsolute { x: _, y: _ } => {
            // Absolute motion needs different handling
            // For now, skip it
            None
        }
        MacOSInputEvent::PointerButton { button, pressed } => {
            let evdev_button = macos_button_to_evdev(button);
            Some(ConvertedEvent::PointerButton(MacOSPointerButtonEvent {
                button: evdev_button,
                state: if pressed { ButtonState::Pressed } else { ButtonState::Released },
                time,
                device: MacOSDevice::pointer(),
            }))
        }
        MacOSInputEvent::PointerAxis { horizontal, vertical, is_trackpad } => {
            Some(ConvertedEvent::PointerAxis(MacOSPointerAxisEvent {
                horizontal,
                vertical,
                is_trackpad,
                time,
                device: MacOSDevice::pointer(),
            }))
        }
        MacOSInputEvent::GesturePinch { scale, phase } => {
            Some(ConvertedEvent::GesturePinch(MacOSGesturePinchEvent {
                scale,
                phase,
                time,
                device: MacOSDevice::pointer(),
            }))
        }
        MacOSInputEvent::GestureSwipe { dx, dy, fingers, phase } => {
            Some(ConvertedEvent::GestureSwipe(MacOSGestureSwipeEvent {
                dx,
                dy,
                fingers,
                phase,
                time,
                device: MacOSDevice::pointer(),
            }))
        }
        MacOSInputEvent::GestureRotate { angle: _, phase: _ } => {
            // Rotation gesture not directly supported
            None
        }
    }
}

/// Enum of converted events
pub enum ConvertedEvent {
    Keyboard(MacOSKeyboardEvent),
    PointerMotion(MacOSPointerMotionEvent),
    PointerButton(MacOSPointerButtonEvent),
    PointerAxis(MacOSPointerAxisEvent),
    GesturePinch(MacOSGesturePinchEvent),
    GestureSwipe(MacOSGestureSwipeEvent),
}

// ============================================================================
// 🧪 Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_creation() {
        let device = MacOSDevice::new();
        assert!(device.has_capability(DeviceCapability::Keyboard));
        assert!(device.has_capability(DeviceCapability::Pointer));
    }

    #[test]
    fn test_keyboard_device() {
        let device = MacOSDevice::keyboard();
        assert!(device.has_capability(DeviceCapability::Keyboard));
        assert!(!device.has_capability(DeviceCapability::Pointer));
    }

    #[test]
    fn test_event_conversion_key() {
        let event = MacOSInputEvent::Key {
            keycode: 0x00, // A
            pressed: true,
            modifiers: MacOSModifiers::default(),
        };

        let converted = convert_input_event(event, Duration::ZERO);
        assert!(matches!(converted, Some(ConvertedEvent::Keyboard(_))));
    }

    #[test]
    fn test_event_conversion_motion() {
        let event = MacOSInputEvent::PointerMotion { dx: 10.0, dy: 20.0 };

        let converted = convert_input_event(event, Duration::ZERO);
        assert!(matches!(converted, Some(ConvertedEvent::PointerMotion(_))));
    }

    #[test]
    fn test_event_conversion_button() {
        let event = MacOSInputEvent::PointerButton {
            button: 0,
            pressed: true,
        };

        let converted = convert_input_event(event, Duration::ZERO);
        if let Some(ConvertedEvent::PointerButton(e)) = converted {
            assert_eq!(e.button, 0x110); // BTN_LEFT
            assert_eq!(e.state, ButtonState::Pressed);
        } else {
            panic!("Expected PointerButton event");
        }
    }
}
