//! # Backend Module - The Great Hardware Abstraction Layer 🎮
//!
//! This module provides the abstraction over different ways niri can run:
//!
//! - **TTY** (Linux): Direct hardware access via DRM/KMS/libinput
//! - **Winit** (Linux): Nested in another Wayland/X11 compositor
//! - **Headless**: No display, for testing
//! - **macOS** (NEW!): Native Apple platform via Cocoa/Metal
//!
//! Running Crayons Ltd. - "Making pixels pretty since... recently"

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use niri_config::{Config, ModKey};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::Output;
#[cfg(target_os = "linux")]
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::niri::Niri;
use crate::utils::id::IdCounter;

// 🐧 Linux-specific backends
#[cfg(target_os = "linux")]
pub mod tty;
#[cfg(target_os = "linux")]
pub use tty::Tty;

#[cfg(target_os = "linux")]
pub mod winit;
#[cfg(target_os = "linux")]
pub use winit::Winit;

pub mod headless;
pub use headless::Headless;

// 🍎 macOS-specific backend
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos::MacOS;

/// 🎰 The Backend Enum - One Compositor, Many Faces
///
/// This enum represents the different ways niri can interact with hardware.
/// On each platform, different variants are available.
#[allow(clippy::large_enum_variant)]
pub enum Backend {
    // 🐧 Linux backends
    #[cfg(target_os = "linux")]
    Tty(Tty),
    #[cfg(target_os = "linux")]
    Winit(Winit),

    // 🧪 Headless backend (all platforms)
    Headless(Headless),

    // 🍎 macOS backend
    #[cfg(target_os = "macos")]
    MacOS(MacOS),
}

#[derive(PartialEq, Eq)]
pub enum RenderResult {
    /// The frame was submitted to the backend for presentation.
    Submitted,
    /// Rendering succeeded, but there was no damage.
    NoDamage,
    /// The frame was not rendered and submitted, due to an error or otherwise.
    Skipped,
}

pub type IpcOutputMap = HashMap<OutputId, niri_ipc::Output>;

static OUTPUT_ID_COUNTER: IdCounter = IdCounter::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputId(u64);

impl OutputId {
    fn next() -> OutputId {
        OutputId(OUTPUT_ID_COUNTER.next())
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl Backend {
    /// 🚀 Initializes the backend after State is constructed
    pub fn init(&mut self, niri: &mut Niri) {
        let _span = tracy_client::span!("Backend::init");
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.init(niri),
            #[cfg(target_os = "linux")]
            Backend::Winit(winit) => winit.init(niri),
            Backend::Headless(headless) => headless.init(niri),
            #[cfg(target_os = "macos")]
            Backend::MacOS(macos) => macos.init(niri),
        }
    }

    /// 🪑 Returns the seat name for input handling
    pub fn seat_name(&self) -> String {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.seat_name(),
            #[cfg(target_os = "linux")]
            Backend::Winit(winit) => winit.seat_name(),
            Backend::Headless(headless) => headless.seat_name(),
            #[cfg(target_os = "macos")]
            Backend::MacOS(macos) => macos.seat_name(),
        }
    }

    /// 🎨 Access the primary renderer for graphics operations
    pub fn with_primary_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer) -> T,
    ) -> Option<T> {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.with_primary_renderer(f),
            #[cfg(target_os = "linux")]
            Backend::Winit(winit) => winit.with_primary_renderer(f),
            Backend::Headless(headless) => headless.with_primary_renderer(f),
            #[cfg(target_os = "macos")]
            Backend::MacOS(macos) => macos.with_primary_renderer(f),
        }
    }

    /// 🖼️ Renders a frame to the specified output
    pub fn render(
        &mut self,
        niri: &mut Niri,
        output: &Output,
        target_presentation_time: Duration,
    ) -> RenderResult {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.render(niri, output, target_presentation_time),
            #[cfg(target_os = "linux")]
            Backend::Winit(winit) => winit.render(niri, output),
            Backend::Headless(headless) => headless.render(niri, output),
            #[cfg(target_os = "macos")]
            Backend::MacOS(macos) => {
                let _ = target_presentation_time; // Not used yet
                macos.render(niri, output)
            }
        }
    }

    /// ⌨️ Returns the modifier key to use (Super/Alt depending on context)
    pub fn mod_key(&self, config: &Config) -> ModKey {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Winit(_) => config.input.mod_key_nested.unwrap_or({
                if let Some(ModKey::Alt) = config.input.mod_key {
                    ModKey::Super
                } else {
                    ModKey::Alt
                }
            }),
            #[cfg(target_os = "linux")]
            Backend::Tty(_) => config.input.mod_key.unwrap_or(ModKey::Super),
            Backend::Headless(_) => config.input.mod_key.unwrap_or(ModKey::Super),
            #[cfg(target_os = "macos")]
            Backend::MacOS(_) => {
                // On macOS, Cmd (logo) is the natural modifier key
                // But we respect the config if set
                config.input.mod_key.unwrap_or(ModKey::Super)
            }
        }
    }

    /// 🖥️ Changes virtual terminal (Linux TTY only)
    pub fn change_vt(&mut self, vt: i32) {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.change_vt(vt),
            #[cfg(target_os = "linux")]
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
            #[cfg(target_os = "macos")]
            Backend::MacOS(_) => {
                // No VT on macOS, that's a Linux thing!
                let _ = vt;
            }
        }
    }

    /// 😴 Suspends the system (where supported)
    pub fn suspend(&mut self) {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.suspend(),
            #[cfg(target_os = "linux")]
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
            #[cfg(target_os = "macos")]
            Backend::MacOS(_) => {
                // On macOS, sleep is handled by the system
                info!("💤 Suspend requested (macOS handles this at system level)");
            }
        }
    }

    /// 🔲 Toggles debug tint overlay
    pub fn toggle_debug_tint(&mut self) {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.toggle_debug_tint(),
            #[cfg(target_os = "linux")]
            Backend::Winit(winit) => winit.toggle_debug_tint(),
            Backend::Headless(_) => (),
            #[cfg(target_os = "macos")]
            Backend::MacOS(macos) => macos.toggle_debug_tint(),
        }
    }

    /// 📦 Imports a DMA-BUF for rendering
    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.import_dmabuf(dmabuf),
            #[cfg(target_os = "linux")]
            Backend::Winit(winit) => winit.import_dmabuf(dmabuf),
            Backend::Headless(headless) => headless.import_dmabuf(dmabuf),
            #[cfg(target_os = "macos")]
            Backend::MacOS(macos) => macos.import_dmabuf(dmabuf),
        }
    }

    /// 🚀 Early-imports surfaces for scan-out (Linux only)
    #[cfg(target_os = "linux")]
    pub fn early_import(&mut self, surface: &WlSurface) {
        match self {
            Backend::Tty(tty) => tty.early_import(surface),
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
        }
    }

    /// 📊 Returns the IPC output information
    pub fn ipc_outputs(&self) -> Arc<Mutex<IpcOutputMap>> {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.ipc_outputs(),
            #[cfg(target_os = "linux")]
            Backend::Winit(winit) => winit.ipc_outputs(),
            Backend::Headless(headless) => headless.ipc_outputs(),
            #[cfg(target_os = "macos")]
            Backend::MacOS(macos) => macos.ipc_outputs(),
        }
    }

    /// 🎬 Returns the GBM device for screencasting (Linux only)
    #[cfg(all(target_os = "linux", feature = "xdp-gnome-screencast"))]
    pub fn gbm_device(
        &self,
    ) -> Option<smithay::backend::allocator::gbm::GbmDevice<smithay::backend::drm::DrmDeviceFd>>
    {
        match self {
            Backend::Tty(tty) => tty.primary_gbm_device(),
            Backend::Winit(_) => None,
            Backend::Headless(_) => None,
        }
    }

    /// 🔌 Sets monitors active/inactive (power management)
    pub fn set_monitors_active(&mut self, active: bool) {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.set_monitors_active(active),
            #[cfg(target_os = "linux")]
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
            #[cfg(target_os = "macos")]
            Backend::MacOS(_) => {
                // macOS handles display power separately
                let _ = active;
            }
        }
    }

    /// 🔄 Sets VRR (Variable Refresh Rate) for an output
    pub fn set_output_on_demand_vrr(&mut self, niri: &mut Niri, output: &Output, enable_vrr: bool) {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.set_output_on_demand_vrr(niri, output, enable_vrr),
            #[cfg(target_os = "linux")]
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
            #[cfg(target_os = "macos")]
            Backend::MacOS(_) => {
                // ProMotion VRR support could be added here
                let _ = (niri, output, enable_vrr);
            }
        }
    }

    /// 🎛️ Updates ignored DRM nodes configuration (Linux only)
    pub fn update_ignored_nodes_config(&mut self, niri: &mut Niri) {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.update_ignored_nodes_config(niri),
            #[cfg(target_os = "linux")]
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
            #[cfg(target_os = "macos")]
            Backend::MacOS(_) => {
                let _ = niri;
            }
        }
    }

    /// 📺 Handles output configuration changes
    pub fn on_output_config_changed(&mut self, niri: &mut Niri) {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Tty(tty) => tty.on_output_config_changed(niri),
            #[cfg(target_os = "linux")]
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
            #[cfg(target_os = "macos")]
            Backend::MacOS(_) => {
                let _ = niri;
            }
        }
    }

    // =========================================================================
    // 🎯 Backend-specific accessors
    // =========================================================================

    /// 🐧 Returns the TTY backend if active (Linux only)
    #[cfg(target_os = "linux")]
    pub fn tty_checked(&mut self) -> Option<&mut Tty> {
        if let Self::Tty(v) = self {
            Some(v)
        } else {
            None
        }
    }

    /// 🐧 Returns the TTY backend, panics if not active (Linux only)
    #[cfg(target_os = "linux")]
    pub fn tty(&mut self) -> &mut Tty {
        if let Self::Tty(v) = self {
            v
        } else {
            panic!("backend is not Tty");
        }
    }

    /// 🪟 Returns the Winit backend, panics if not active (Linux only)
    #[cfg(target_os = "linux")]
    pub fn winit(&mut self) -> &mut Winit {
        if let Self::Winit(v) = self {
            v
        } else {
            panic!("backend is not Winit")
        }
    }

    /// 🧪 Returns the Headless backend, panics if not active
    pub fn headless(&mut self) -> &mut Headless {
        if let Self::Headless(v) = self {
            v
        } else {
            panic!("backend is not Headless")
        }
    }

    /// 🍎 Returns the macOS backend, panics if not active (macOS only)
    #[cfg(target_os = "macos")]
    pub fn macos(&mut self) -> &mut MacOS {
        if let Self::MacOS(v) = self {
            v
        } else {
            panic!("backend is not MacOS")
        }
    }

    /// 🍎 Returns the macOS backend if active (macOS only)
    #[cfg(target_os = "macos")]
    pub fn macos_checked(&mut self) -> Option<&mut MacOS> {
        if let Self::MacOS(v) = self {
            Some(v)
        } else {
            None
        }
    }
}
