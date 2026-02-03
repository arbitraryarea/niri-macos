//! # Cocoa Window Management 🪟
//!
//! This module handles the actual Cocoa window creation and management.
//! We use the `cocoa` and `objc` crates to interface with Objective-C.
//!
//! ## Window Architecture
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │           NSWindow                  │
//! │  ┌───────────────────────────────┐  │
//! │  │         NSView                │  │
//! │  │  ┌─────────────────────────┐  │  │
//! │  │  │     CAMetalLayer        │  │  │
//! │  │  │   (Metal rendering)     │  │  │
//! │  │  └─────────────────────────┘  │  │
//! │  └───────────────────────────────┘  │
//! └─────────────────────────────────────┘
//! ```
//!
//! Running Crayons Ltd. - "Windows without the Windows"

#![allow(non_upper_case_globals)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(target_os = "macos")]
use cocoa::appkit::{
    NSApp, NSApplication, NSApplicationActivationPolicy, NSBackingStoreBuffered,
    NSWindow, NSWindowStyleMask, NSView, NSApplicationActivateIgnoringOtherApps,
};
#[cfg(target_os = "macos")]
use cocoa::base::{id, nil, NO, YES};
#[cfg(target_os = "macos")]
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
#[cfg(target_os = "macos")]
use core_foundation::base::TCFType;
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;
#[cfg(target_os = "macos")]
use objc::declare::ClassDecl;
#[cfg(target_os = "macos")]
use objc::runtime::{Class, Object, Sel, BOOL};
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};

/// 🪟 Cocoa Window Handle
///
/// Manages an NSWindow with a Metal-backed NSView for rendering.
pub struct CocoaWindow {
    #[cfg(target_os = "macos")]
    ns_window: id,
    #[cfg(target_os = "macos")]
    ns_view: id,
    width: u32,
    height: u32,
    scale_factor: f64,
    is_visible: bool,
    needs_redraw: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    delegate: id,
}

#[cfg(target_os = "macos")]
unsafe impl Send for CocoaWindow {}
#[cfg(target_os = "macos")]
unsafe impl Sync for CocoaWindow {}

impl CocoaWindow {
    /// Creates a new Cocoa window with the specified title and dimensions.
    #[cfg(target_os = "macos")]
    pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
        unsafe {
            // Initialize the application if not already done
            let app = NSApp();
            if app == nil {
                return Err("Failed to get NSApplication".into());
            }
            app.setActivationPolicy_(NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular);

            // Create autorelease pool for memory management
            let _pool = NSAutoreleasePool::new(nil);

            // Define window style
            let style_mask = NSWindowStyleMask::NSTitledWindowMask
                | NSWindowStyleMask::NSClosableWindowMask
                | NSWindowStyleMask::NSMiniaturizableWindowMask
                | NSWindowStyleMask::NSResizableWindowMask;

            // Create the window frame
            let frame = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width as f64, height as f64),
            );

            // Create the NSWindow
            let ns_window: id = msg_send![class!(NSWindow), alloc];
            let ns_window: id = msg_send![
                ns_window,
                initWithContentRect:frame
                styleMask:style_mask
                backing:NSBackingStoreBuffered
                defer:NO
            ];

            if ns_window == nil {
                return Err("Failed to create NSWindow".into());
            }

            // Set window title
            let title_str = NSString::alloc(nil).init_str(title);
            let _: () = msg_send![ns_window, setTitle: title_str];

            // Center the window on screen
            let _: () = msg_send![ns_window, center];

            // Create the content view with layer backing for Metal
            let ns_view: id = msg_send![class!(NSView), alloc];
            let ns_view: id = msg_send![ns_view, initWithFrame: frame];
            let _: () = msg_send![ns_view, setWantsLayer: YES];

            // Set the view as the window's content view
            let _: () = msg_send![ns_window, setContentView: ns_view];

            // Create and set up the window delegate
            let delegate = create_window_delegate();
            let _: () = msg_send![ns_window, setDelegate: delegate];

            // Make the window key and visible
            let _: () = msg_send![ns_window, makeKeyAndOrderFront: nil];

            // Activate the application
            app.activateIgnoringOtherApps_(YES);

            // Get the backing scale factor (for Retina displays)
            let scale_factor: f64 = msg_send![ns_window, backingScaleFactor];

            info!("🪟 Created Cocoa window: {}x{} @ {}x scale", width, height, scale_factor);

            Ok(Self {
                ns_window,
                ns_view,
                width,
                height,
                scale_factor,
                is_visible: true,
                needs_redraw: Arc::new(AtomicBool::new(false)),
                delegate,
            })
        }
    }

    /// Non-macOS stub
    #[cfg(not(target_os = "macos"))]
    pub fn new(_title: &str, width: u32, height: u32) -> Result<Self, String> {
        Ok(Self {
            width,
            height,
            scale_factor: 2.0,
            is_visible: true,
            needs_redraw: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Requests a redraw of the window's content.
    pub fn request_redraw(&self) {
        self.needs_redraw.store(true, Ordering::SeqCst);

        #[cfg(target_os = "macos")]
        unsafe {
            let _: () = msg_send![self.ns_view, setNeedsDisplay: YES];
        }
    }

    /// Checks if the window needs to be redrawn.
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw.load(Ordering::SeqCst)
    }

    /// Clears the redraw flag.
    pub fn clear_redraw_flag(&self) {
        self.needs_redraw.store(false, Ordering::SeqCst);
    }

    /// Sets the window title.
    #[cfg(target_os = "macos")]
    pub fn set_title(&self, title: &str) {
        unsafe {
            let title_str = NSString::alloc(nil).init_str(title);
            let _: () = msg_send![self.ns_window, setTitle: title_str];
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn set_title(&self, _title: &str) {}

    /// Gets the current window size.
    pub fn size(&self) -> (u32, u32) {
        #[cfg(target_os = "macos")]
        unsafe {
            let frame: NSRect = msg_send![self.ns_view, frame];
            return (frame.size.width as u32, frame.size.height as u32);
        }

        #[cfg(not(target_os = "macos"))]
        (self.width, self.height)
    }

    /// Gets the backing scale factor (2.0 for Retina).
    pub fn scale_factor(&self) -> f64 {
        #[cfg(target_os = "macos")]
        unsafe {
            let scale: f64 = msg_send![self.ns_window, backingScaleFactor];
            return scale;
        }

        #[cfg(not(target_os = "macos"))]
        self.scale_factor
    }

    /// Shows the window.
    #[cfg(target_os = "macos")]
    pub fn show(&mut self) {
        unsafe {
            let _: () = msg_send![self.ns_window, makeKeyAndOrderFront: nil];
            self.is_visible = true;
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn show(&mut self) {
        self.is_visible = true;
    }

    /// Hides the window.
    #[cfg(target_os = "macos")]
    pub fn hide(&mut self) {
        unsafe {
            let _: () = msg_send![self.ns_window, orderOut: nil];
            self.is_visible = false;
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn hide(&mut self) {
        self.is_visible = false;
    }

    /// Returns whether the window is currently visible.
    pub fn is_visible(&self) -> bool {
        self.is_visible
    }

    /// Gets the raw NSWindow pointer (for Metal rendering).
    #[cfg(target_os = "macos")]
    pub fn ns_window(&self) -> id {
        self.ns_window
    }

    /// Gets the raw NSView pointer (for Metal layer).
    #[cfg(target_os = "macos")]
    pub fn ns_view(&self) -> id {
        self.ns_view
    }

    /// Sets up the Metal layer for rendering.
    #[cfg(target_os = "macos")]
    pub fn setup_metal_layer(&self) -> Result<id, String> {
        unsafe {
            // Get or create the CAMetalLayer
            let layer: id = msg_send![self.ns_view, layer];
            if layer == nil {
                return Err("View has no layer".into());
            }

            // Check if it's already a CAMetalLayer
            let class_name: id = msg_send![layer, className];
            let name: *const i8 = msg_send![class_name, UTF8String];
            let name_str = std::ffi::CStr::from_ptr(name).to_string_lossy();

            if name_str == "CAMetalLayer" {
                return Ok(layer);
            }

            // Create a new CAMetalLayer
            let metal_layer: id = msg_send![class!(CAMetalLayer), layer];
            if metal_layer == nil {
                return Err("Failed to create CAMetalLayer".into());
            }

            // Configure the layer
            let _: () = msg_send![metal_layer, setContentsScale: self.scale_factor];
            let _: () = msg_send![self.ns_view, setLayer: metal_layer];

            Ok(metal_layer)
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for CocoaWindow {
    fn drop(&mut self) {
        unsafe {
            // Release the window
            let _: () = msg_send![self.ns_window, close];
            let _: () = msg_send![self.ns_window, release];
            if self.delegate != nil {
                let _: () = msg_send![self.delegate, release];
            }
        }
    }
}

// ============================================================================
// 🎭 Window Delegate
// ============================================================================

#[cfg(target_os = "macos")]
static mut WINDOW_DELEGATE_CLASS: Option<&'static Class> = None;

/// Creates a custom NSWindowDelegate to handle window events.
#[cfg(target_os = "macos")]
fn create_window_delegate() -> id {
    unsafe {
        let class = get_or_create_delegate_class();
        let delegate: id = msg_send![class, alloc];
        let delegate: id = msg_send![delegate, init];
        delegate
    }
}

#[cfg(target_os = "macos")]
fn get_or_create_delegate_class() -> &'static Class {
    unsafe {
        if let Some(class) = WINDOW_DELEGATE_CLASS {
            return class;
        }

        let superclass = class!(NSObject);
        let mut decl = ClassDecl::new("NiriWindowDelegate", superclass)
            .expect("Failed to create NiriWindowDelegate class");

        // Add protocol conformance
        decl.add_method(
            sel!(windowWillClose:),
            window_will_close as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(windowDidResize:),
            window_did_resize as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(windowDidBecomeKey:),
            window_did_become_key as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(windowDidResignKey:),
            window_did_resign_key as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(windowDidChangeBackingProperties:),
            window_did_change_backing as extern "C" fn(&Object, Sel, id),
        );

        let class = decl.register();
        WINDOW_DELEGATE_CLASS = Some(class);
        class
    }
}

// ============================================================================
// 🎯 Delegate Methods
// ============================================================================

#[cfg(target_os = "macos")]
extern "C" fn window_will_close(_this: &Object, _sel: Sel, _notification: id) {
    info!("🪟 Window will close");
    // Signal the compositor to shut down
}

#[cfg(target_os = "macos")]
extern "C" fn window_did_resize(_this: &Object, _sel: Sel, notification: id) {
    unsafe {
        let window: id = msg_send![notification, object];
        let frame: NSRect = msg_send![window, frame];
        info!("📐 Window resized to {}x{}", frame.size.width, frame.size.height);
    }
}

#[cfg(target_os = "macos")]
extern "C" fn window_did_become_key(_this: &Object, _sel: Sel, _notification: id) {
    info!("🎯 Window became key (focused)");
}

#[cfg(target_os = "macos")]
extern "C" fn window_did_resign_key(_this: &Object, _sel: Sel, _notification: id) {
    info!("😴 Window resigned key (unfocused)");
}

#[cfg(target_os = "macos")]
extern "C" fn window_did_change_backing(_this: &Object, _sel: Sel, notification: id) {
    unsafe {
        let window: id = msg_send![notification, object];
        let scale: f64 = msg_send![window, backingScaleFactor];
        info!("📏 Backing scale factor changed to {}", scale);
    }
}

// ============================================================================
// 🧪 Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_stub() {
        // This test runs on any platform
        let window = CocoaWindow::new("Test", 800, 600).unwrap();
        assert_eq!(window.size(), (800, 600));
        assert!(window.scale_factor() > 0.0);
    }
}
