//! # macOS Event Loop Integration 🔄
//!
//! This module bridges Cocoa's event handling to calloop's event loop.
//! macOS uses a run loop (CFRunLoop) for events, but we need to integrate
//! with calloop for consistency with the rest of niri.
//!
//! ## Architecture
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────┐
//! │                    calloop EventLoop                      │
//! │  ┌─────────────────────────────────────────────────────┐  │
//! │  │              MacOSEventSource                       │  │
//! │  │  ┌───────────────────────────────────────────────┐  │  │
//! │  │  │         pipe (read/write fd)                  │  │  │
//! │  │  │   (wake up calloop from Cocoa events)         │  │  │
//! │  │  └───────────────────────────────────────────────┘  │  │
//! │  └────────────────────────┬────────────────────────────┘  │
//! │                           │                               │
//! │  ┌────────────────────────▼────────────────────────────┐  │
//! │  │              Event Callback                         │  │
//! │  │    (process queued MacOSInputEvents)                │  │
//! │  └─────────────────────────────────────────────────────┘  │
//! └───────────────────────────────────────────────────────────┘
//! ```
//!
//! Running Crayons Ltd. - "Events happen, we handle them"

use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::{Arc, Mutex};

use calloop::generic::Generic;
use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};

use super::MacOSInputEvent;

/// 🔄 macOS Event Source for calloop
///
/// This event source allows calloop to be woken up when macOS events occur.
/// We use a pipe for cross-thread signaling.
pub struct MacOSEventSource {
    /// Read end of the wake-up pipe
    read_pipe: RawFd,
    /// Write end of the wake-up pipe (for signaling)
    write_pipe: RawFd,
    /// Queue of pending events
    event_queue: Arc<Mutex<Vec<MacOSInputEvent>>>,
    /// Registration token
    token: Option<Token>,
}

impl MacOSEventSource {
    /// Creates a new macOS event source.
    pub fn new() -> io::Result<Self> {
        // Create a pipe for signaling
        let mut fds = [0i32; 2];

        #[cfg(unix)]
        unsafe {
            if libc::pipe(fds.as_mut_ptr()) != 0 {
                return Err(io::Error::last_os_error());
            }

            // Make non-blocking
            let flags = libc::fcntl(fds[0], libc::F_GETFL);
            libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK);
            let flags = libc::fcntl(fds[1], libc::F_GETFL);
            libc::fcntl(fds[1], libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        Ok(Self {
            read_pipe: fds[0],
            write_pipe: fds[1],
            event_queue: Arc::new(Mutex::new(Vec::new())),
            token: None,
        })
    }

    /// Signals that events are available.
    pub fn signal(&self) {
        let buf = [1u8];
        unsafe {
            libc::write(self.write_pipe, buf.as_ptr() as *const _, 1);
        }
    }

    /// Queues an event and signals calloop.
    pub fn queue_event(&self, event: MacOSInputEvent) {
        {
            let mut queue = self.event_queue.lock().unwrap();
            queue.push(event);
        }
        self.signal();
    }

    /// Queues multiple events and signals calloop.
    pub fn queue_events(&self, events: Vec<MacOSInputEvent>) {
        {
            let mut queue = self.event_queue.lock().unwrap();
            queue.extend(events);
        }
        self.signal();
    }

    /// Drains pending events.
    pub fn drain_events(&self) -> Vec<MacOSInputEvent> {
        let mut queue = self.event_queue.lock().unwrap();
        std::mem::take(&mut *queue)
    }

    /// Gets the event queue for sharing with other threads.
    pub fn event_queue(&self) -> Arc<Mutex<Vec<MacOSInputEvent>>> {
        self.event_queue.clone()
    }

    /// Clears the signal (reads from the pipe).
    fn clear_signal(&self) {
        let mut buf = [0u8; 64];
        loop {
            let result = unsafe {
                libc::read(self.read_pipe, buf.as_mut_ptr() as *mut _, buf.len())
            };
            if result <= 0 {
                break;
            }
        }
    }
}

impl Drop for MacOSEventSource {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.read_pipe);
            libc::close(self.write_pipe);
        }
    }
}

impl EventSource for MacOSEventSource {
    type Event = Vec<MacOSInputEvent>;
    type Metadata = ();
    type Ret = ();
    type Error = io::Error;

    fn process_events<F>(
        &mut self,
        _readiness: Readiness,
        _token: Token,
        mut callback: F,
    ) -> io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        // Clear the signal
        self.clear_signal();

        // Drain and process events
        let events = self.drain_events();
        if !events.is_empty() {
            callback(events, &mut ());
        }

        Ok(PostAction::Continue)
    }

    fn register(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        let token = token_factory.token();
        self.token = Some(token);
        poll.register(
            &mut Generic::new(self.read_pipe, Interest::READ, Mode::Level),
            token,
        )
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.unregister(poll)?;
        self.register(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        if let Some(token) = self.token.take() {
            poll.unregister(&mut Generic::new(self.read_pipe, Interest::READ, Mode::Level))?;
        }
        Ok(())
    }
}

// ============================================================================
// 🕐 Frame Timer for VSync
// ============================================================================

/// 🕐 Frame Timer
///
/// A timer that fires at the display refresh rate for frame pacing.
pub struct FrameTimer {
    /// Target frame duration in nanoseconds
    target_duration_ns: u64,
    /// Last frame time
    last_frame_ns: u64,
}

impl FrameTimer {
    /// Creates a new frame timer with the specified target FPS.
    pub fn new(target_fps: u32) -> Self {
        Self {
            target_duration_ns: 1_000_000_000 / target_fps as u64,
            last_frame_ns: 0,
        }
    }

    /// Sets the target FPS.
    pub fn set_target_fps(&mut self, fps: u32) {
        self.target_duration_ns = 1_000_000_000 / fps as u64;
    }

    /// Checks if it's time for the next frame.
    pub fn should_render(&self) -> bool {
        let now = Self::now_ns();
        now - self.last_frame_ns >= self.target_duration_ns
    }

    /// Marks that a frame was rendered.
    pub fn frame_rendered(&mut self) {
        self.last_frame_ns = Self::now_ns();
    }

    /// Returns time until next frame in nanoseconds.
    pub fn time_until_next_frame(&self) -> u64 {
        let now = Self::now_ns();
        let elapsed = now.saturating_sub(self.last_frame_ns);
        self.target_duration_ns.saturating_sub(elapsed)
    }

    fn now_ns() -> u64 {
        #[cfg(unix)]
        {
            let mut ts = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            unsafe {
                libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
            }
            (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
        }

        #[cfg(not(unix))]
        {
            use std::time::Instant;
            static START: once_cell::sync::Lazy<Instant> = once_cell::sync::Lazy::new(Instant::now);
            START.elapsed().as_nanos() as u64
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
    fn test_event_source_creation() {
        let source = MacOSEventSource::new().unwrap();
        assert!(source.event_queue.lock().unwrap().is_empty());
    }

    #[test]
    fn test_event_queuing() {
        let source = MacOSEventSource::new().unwrap();

        source.queue_event(MacOSInputEvent::PointerMotion { dx: 1.0, dy: 2.0 });

        let events = source.drain_events();
        assert_eq!(events.len(), 1);

        // Queue should be empty after draining
        assert!(source.drain_events().is_empty());
    }

    #[test]
    fn test_frame_timer() {
        let timer = FrameTimer::new(60);
        assert!(timer.target_duration_ns > 0);
    }

    #[test]
    fn test_frame_timer_fps_change() {
        let mut timer = FrameTimer::new(60);
        let original_duration = timer.target_duration_ns;

        timer.set_target_fps(120);
        assert!(timer.target_duration_ns < original_duration);
    }
}
