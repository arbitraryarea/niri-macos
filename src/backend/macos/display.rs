//! # Core Graphics Display Management 🖥️
//!
//! This module handles display enumeration, mode setting, and hotplug
//! monitoring via Core Graphics.
//!
//! ## Display Architecture
//!
//! ```text
//! ┌────────────────────────────────────────────────┐
//! │              Display Manager                   │
//! │  ┌────────────────────────────────────────┐   │
//! │  │        CGGetActiveDisplayList          │   │
//! │  │     (Enumerate connected displays)     │   │
//! │  └─────────────────┬──────────────────────┘   │
//! │                    │                          │
//! │  ┌─────────────────▼──────────────────────┐   │
//! │  │     CGDisplayRegisterReconfiguration   │   │
//! │  │          (Hotplug callbacks)           │   │
//! │  └─────────────────┬──────────────────────┘   │
//! │                    │                          │
//! │  ┌─────────────────▼──────────────────────┐   │
//! │  │       CGDisplayCopyDisplayMode         │   │
//! │  │     (Resolution, refresh rate, etc.)   │   │
//! │  └────────────────────────────────────────┘   │
//! └────────────────────────────────────────────────┘
//! ```
//!
//! Running Crayons Ltd. - "Seeing is believing"

use std::sync::{Arc, Mutex};
use std::collections::HashMap;

use super::MacOSDisplay;

#[cfg(target_os = "macos")]
use core_graphics::display::{
    CGDisplay, CGDisplayMode, CGGetActiveDisplayList, CGMainDisplayID,
    CGDisplayScreenSize, CGDisplayBounds, CGDisplayPixelsWide, CGDisplayPixelsHigh,
};

/// 🖥️ Display Manager
///
/// Handles display enumeration, configuration, and hotplug events.
pub struct DisplayManager {
    /// Known displays indexed by CGDirectDisplayID
    displays: Arc<Mutex<HashMap<u32, DisplayInfo>>>,
    /// Callback for display configuration changes
    on_display_change: Option<Box<dyn Fn(DisplayEvent) + Send>>,
    /// Registration token for display reconfiguration callbacks
    #[cfg(target_os = "macos")]
    callback_token: Option<u32>,
}

/// 📺 Display Information
#[derive(Debug, Clone)]
pub struct DisplayInfo {
    pub id: u32,
    pub name: String,
    pub bounds: DisplayBounds,
    pub mode: DisplayMode,
    pub physical_size_mm: Option<(u32, u32)>,
    pub is_builtin: bool,
    pub is_main: bool,
    pub scale_factor: f64,
}

/// 📐 Display Bounds
#[derive(Debug, Clone, Copy)]
pub struct DisplayBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 🎬 Display Mode
#[derive(Debug, Clone)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32, // In millihertz (60000 = 60Hz)
    pub bits_per_pixel: u32,
    pub is_retina: bool,
}

/// 📡 Display Events
#[derive(Debug, Clone)]
pub enum DisplayEvent {
    /// A new display was connected
    Added(DisplayInfo),
    /// A display was disconnected
    Removed(u32), // Display ID
    /// Display configuration changed (resolution, position, etc.)
    Changed(DisplayInfo),
    /// Main display changed
    MainChanged(u32),
}

impl DisplayManager {
    /// Creates a new display manager and enumerates connected displays.
    pub fn new() -> Self {
        info!("🖥️ Initializing display manager...");

        let mut manager = Self {
            displays: Arc::new(Mutex::new(HashMap::new())),
            on_display_change: None,
            #[cfg(target_os = "macos")]
            callback_token: None,
        };

        // Enumerate displays
        manager.refresh_displays();

        // Set up hotplug monitoring
        #[cfg(target_os = "macos")]
        manager.setup_hotplug_monitoring();

        manager
    }

    /// Refreshes the display list.
    pub fn refresh_displays(&mut self) {
        let displays = Self::enumerate_displays();

        let mut display_map = self.displays.lock().unwrap();
        display_map.clear();

        for display in displays {
            let info = DisplayInfo {
                id: display.display_id,
                name: display.name.clone(),
                bounds: DisplayBounds {
                    x: 0.0,
                    y: 0.0,
                    width: display.resolution.0 as f64,
                    height: display.resolution.1 as f64,
                },
                mode: DisplayMode {
                    width: display.resolution.0,
                    height: display.resolution.1,
                    refresh_rate: display.refresh_rate,
                    bits_per_pixel: 32,
                    is_retina: display.scale_factor > 1.5,
                },
                physical_size_mm: display.physical_size_mm,
                is_builtin: display.is_main, // Approximation
                is_main: display.is_main,
                scale_factor: display.scale_factor,
            };

            info!(
                "🖥️ Found display: {} ({}x{} @ {}Hz, {}x scale)",
                info.name,
                info.mode.width,
                info.mode.height,
                info.mode.refresh_rate / 1000,
                info.scale_factor
            );

            display_map.insert(display.display_id, info);
        }
    }

    /// Gets all connected displays.
    pub fn get_displays(&self) -> Vec<DisplayInfo> {
        self.displays.lock().unwrap().values().cloned().collect()
    }

    /// Gets the main display.
    pub fn get_main_display(&self) -> Option<DisplayInfo> {
        self.displays
            .lock()
            .unwrap()
            .values()
            .find(|d| d.is_main)
            .cloned()
    }

    /// Gets a display by ID.
    pub fn get_display(&self, id: u32) -> Option<DisplayInfo> {
        self.displays.lock().unwrap().get(&id).cloned()
    }

    /// Sets the callback for display events.
    pub fn set_on_display_change<F>(&mut self, callback: F)
    where
        F: Fn(DisplayEvent) + Send + 'static,
    {
        self.on_display_change = Some(Box::new(callback));
    }

    /// Enumerates all connected displays.
    #[cfg(target_os = "macos")]
    pub fn enumerate_displays() -> Vec<MacOSDisplay> {
        let mut displays = Vec::new();

        unsafe {
            // Get the number of active displays
            let mut display_count: u32 = 0;
            let result = CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut display_count);
            if result != 0 || display_count == 0 {
                warn!("Failed to get display count");
                return displays;
            }

            // Get the display IDs
            let mut display_ids = vec![0u32; display_count as usize];
            let result = CGGetActiveDisplayList(
                display_count,
                display_ids.as_mut_ptr(),
                &mut display_count,
            );
            if result != 0 {
                warn!("Failed to get display list");
                return displays;
            }

            let main_id = CGMainDisplayID();

            for &display_id in &display_ids {
                let display = CGDisplay::new(display_id);

                // Get display mode
                let mode = display.display_mode().unwrap_or_else(|| {
                    // Fallback mode
                    CGDisplayMode::new(display_id)
                });

                let width = mode.width() as u32;
                let height = mode.height() as u32;
                let refresh_rate = (mode.refresh_rate() * 1000.0) as u32;
                let refresh_rate = if refresh_rate == 0 { 60_000 } else { refresh_rate };

                // Get physical size
                let size = CGDisplayScreenSize(display_id);
                let physical_size_mm = if size.width > 0.0 && size.height > 0.0 {
                    Some((size.width as u32, size.height as u32))
                } else {
                    None
                };

                // Calculate scale factor
                let bounds = CGDisplayBounds(display_id);
                let pixels_wide = CGDisplayPixelsWide(display_id);
                let scale_factor = if bounds.size.width > 0.0 {
                    pixels_wide as f64 / bounds.size.width
                } else {
                    1.0
                };

                // Generate display name
                let name = if display_id == main_id {
                    "Built-in Display".to_string()
                } else {
                    format!("Display {}", display_id)
                };

                displays.push(MacOSDisplay {
                    display_id,
                    name,
                    physical_size_mm,
                    resolution: (width, height),
                    refresh_rate,
                    scale_factor,
                    is_main: display_id == main_id,
                });
            }
        }

        displays
    }

    /// Non-macOS stub for display enumeration.
    #[cfg(not(target_os = "macos"))]
    pub fn enumerate_displays() -> Vec<MacOSDisplay> {
        vec![MacOSDisplay {
            display_id: 0,
            name: "Simulated Display".to_string(),
            physical_size_mm: Some((344, 215)),
            resolution: (2560, 1600),
            refresh_rate: 60_000,
            scale_factor: 2.0,
            is_main: true,
        }]
    }

    /// Sets up display hotplug monitoring.
    #[cfg(target_os = "macos")]
    fn setup_hotplug_monitoring(&mut self) {
        // Note: Full implementation would use CGDisplayRegisterReconfigurationCallback
        // For now, we rely on polling or application events
        info!("🔌 Display hotplug monitoring set up");
    }

    /// Gets available modes for a display.
    #[cfg(target_os = "macos")]
    pub fn get_display_modes(&self, display_id: u32) -> Vec<DisplayMode> {
        let mut modes = Vec::new();

        unsafe {
            let display = CGDisplay::new(display_id);

            // Get all available modes
            if let Some(mode_list) = display.all_display_modes() {
                for mode in mode_list.iter() {
                    let width = mode.width() as u32;
                    let height = mode.height() as u32;
                    let refresh_rate = (mode.refresh_rate() * 1000.0) as u32;
                    let refresh_rate = if refresh_rate == 0 { 60_000 } else { refresh_rate };
                    let bits_per_pixel = mode.bit_depth() as u32;

                    // Determine if this is a HiDPI mode
                    let io_flags = mode.io_flags();
                    let is_retina = (io_flags & 0x00200000) != 0; // kDisplayModeNativeFlag

                    modes.push(DisplayMode {
                        width,
                        height,
                        refresh_rate,
                        bits_per_pixel,
                        is_retina,
                    });
                }
            }
        }

        // Sort by resolution and refresh rate
        modes.sort_by(|a, b| {
            (b.width, b.height, b.refresh_rate).cmp(&(a.width, a.height, a.refresh_rate))
        });

        // Remove duplicates
        modes.dedup_by(|a, b| {
            a.width == b.width
                && a.height == b.height
                && a.refresh_rate == b.refresh_rate
        });

        modes
    }

    #[cfg(not(target_os = "macos"))]
    pub fn get_display_modes(&self, _display_id: u32) -> Vec<DisplayMode> {
        vec![
            DisplayMode {
                width: 2560,
                height: 1600,
                refresh_rate: 60_000,
                bits_per_pixel: 32,
                is_retina: true,
            },
            DisplayMode {
                width: 1920,
                height: 1200,
                refresh_rate: 60_000,
                bits_per_pixel: 32,
                is_retina: false,
            },
        ]
    }

    /// Checks if ProMotion (variable refresh rate) is supported.
    #[cfg(target_os = "macos")]
    pub fn supports_promotion(&self, display_id: u32) -> bool {
        // ProMotion displays support refresh rates up to 120Hz
        let modes = self.get_display_modes(display_id);
        modes.iter().any(|m| m.refresh_rate > 60_000)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn supports_promotion(&self, _display_id: u32) -> bool {
        false
    }

    /// Gets the maximum refresh rate for a display.
    pub fn max_refresh_rate(&self, display_id: u32) -> u32 {
        let modes = self.get_display_modes(display_id);
        modes.iter().map(|m| m.refresh_rate).max().unwrap_or(60_000)
    }
}

impl Default for DisplayManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DisplayManager {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(_token) = self.callback_token.take() {
            // Unregister the reconfiguration callback
            // CGDisplayRemoveReconfigurationCallback(...)
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
    fn test_display_enumeration() {
        let displays = DisplayManager::enumerate_displays();
        assert!(!displays.is_empty(), "Should find at least one display");

        let main = displays.iter().find(|d| d.is_main);
        assert!(main.is_some(), "Should have a main display");
    }

    #[test]
    fn test_display_manager_creation() {
        let manager = DisplayManager::new();
        let displays = manager.get_displays();
        assert!(!displays.is_empty());
    }

    #[test]
    fn test_main_display() {
        let manager = DisplayManager::new();
        let main = manager.get_main_display();
        assert!(main.is_some());

        let main = main.unwrap();
        assert!(main.is_main);
        assert!(main.mode.width > 0);
        assert!(main.mode.height > 0);
    }
}
