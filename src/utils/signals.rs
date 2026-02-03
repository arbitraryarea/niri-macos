//! # Signal Handling - Teaching Niri to Listen 📡
//!
//! We set a signal handler with `calloop::signals::Signals::new`.
//! This does two things:
//! 1. It blocks the thread from receiving these signals normally (pthread_sigmask)
//! 2. It creates a signalfd to read them in the event loop.
//!
//! When spawning children, calloop already deals with the signalfd.
//! `Signals::new` creates it with CLOEXEC, so it will not be inherited by children.
//!
//! But, the sigmask is always inherited, so we want to clear it before spawning children.
//! That way, we don't affect their normal signal handling.
//!
//! In particular, if a child doesn't care about signals, we must not block it from receiving them.
//!
//! This module provides functions to clear the sigmask. Call them before spawning children.
//!
//! Technically, a "more correct" solution would be to remember the original sigmask and restore it
//! after the child exits, but that's painful *and* likely to cause issues, because the user almost
//! never intended to spawn niri with a nonempty sigmask. It indicates a bug in whoever spawned us,
//! so we may as well clean up after them (which is easier than not doing so).
//!
//! ## Platform Support
//!
//! - **Linux**: Full support via signalfd
//! - **macOS**: Using kqueue-based signal handling via calloop
//! - **FreeBSD**: Not yet implemented (contributions welcome!)
//!
//! Running Crayons Ltd. - "Handling signals with grace since 2024"

pub use platform::*;

// 🍎 macOS signal handling
#[cfg(target_os = "macos")]
mod platform {
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Flag to track if we should stop
    static SHOULD_STOP: AtomicBool = AtomicBool::new(false);

    /// Sets up signal handling for macOS
    ///
    /// macOS doesn't have signalfd, but we can use kqueue via calloop's
    /// Unix signal handling, or fall back to traditional signal handlers.
    pub fn listen(handle: &calloop::LoopHandle<crate::niri::State>) {
        // Try to use calloop's signal handling if available
        // Note: calloop's Signals source uses kqueue on macOS
        use calloop::signals::{Signal, Signals};

        let signals = match Signals::new(&[Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP]) {
            Ok(s) => s,
            Err(err) => {
                warn!("🍎 Could not set up signal handling: {}", err);
                warn!("   Using fallback Ctrl+C handler");
                setup_fallback_handler();
                return;
            }
        };

        handle
            .insert_source(signals, |event, _, state| {
                info!("🚦 Received signal {:?}, initiating graceful shutdown", event.signal());
                state.niri.stop_signal.stop();
            })
            .unwrap();

        info!("🍎 Signal handling configured (SIGINT, SIGTERM, SIGHUP)");
    }

    /// Sets up a fallback Ctrl+C handler using std::ctrlc
    fn setup_fallback_handler() {
        // We can't easily integrate this with calloop, but at least
        // the user can Ctrl+C to exit
        unsafe {
            libc::signal(libc::SIGINT, fallback_handler as usize);
            libc::signal(libc::SIGTERM, fallback_handler as usize);
        }
    }

    extern "C" fn fallback_handler(_: libc::c_int) {
        SHOULD_STOP.store(true, Ordering::SeqCst);
        // Note: This doesn't cleanly integrate with the event loop
        // but at least allows termination
    }

    /// Checks if a signal was received via the fallback handler
    pub fn should_stop() -> bool {
        SHOULD_STOP.load(Ordering::SeqCst)
    }

    /// Blocks signals early (before creating threads)
    ///
    /// On macOS, we use pthread_sigmask similar to Linux.
    pub fn block_early() -> io::Result<()> {
        set_sigmask(&preferred_sigset()?)
    }

    /// Unblocks all signals (before spawning children)
    pub fn unblock_all() -> io::Result<()> {
        set_sigmask(&empty_sigset()?)
    }

    fn empty_sigset() -> io::Result<libc::sigset_t> {
        let mut sigset = std::mem::MaybeUninit::uninit();
        if unsafe { libc::sigemptyset(sigset.as_mut_ptr()) } == 0 {
            Ok(unsafe { sigset.assume_init() })
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn preferred_sigset() -> io::Result<libc::sigset_t> {
        let mut set = empty_sigset()?;
        unsafe {
            add_signal(&mut set, libc::SIGINT)?;
            add_signal(&mut set, libc::SIGTERM)?;
            add_signal(&mut set, libc::SIGHUP)?;
        }
        Ok(set)
    }

    // SAFETY: `signum` must be a valid signal number.
    unsafe fn add_signal(set: &mut libc::sigset_t, signum: libc::c_int) -> io::Result<()> {
        if unsafe { libc::sigaddset(set, signum) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn set_sigmask(set: &libc::sigset_t) -> io::Result<()> {
        let oldset = std::ptr::null_mut(); // ignore old mask
        if unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, set, oldset) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

// 🐧 Other Unix platforms (FreeBSD, etc.)
#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
mod platform {
    use std::io;

    // FIXME: implement for FreeBSD. But probably, that should be done in calloop::signals.
    pub fn listen(_handle: &calloop::LoopHandle<crate::niri::State>) {
        warn!("⚠️ Signal handling not implemented for this platform");
    }

    // These two actually build as-is on FreeBSD, but without our own signal handling in listen(),
    // they do more harm than good (they block termination signals without actually installing a
    // termination handler).
    pub fn block_early() -> io::Result<()> {
        Ok(())
    }
    pub fn unblock_all() -> io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::{io, mem};

    pub fn listen(handle: &calloop::LoopHandle<crate::niri::State>) {
        use calloop::signals::{Signal, Signals};

        handle
            .insert_source(
                Signals::new(&[Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP]).unwrap(),
                |event, _, state| {
                    info!("quitting due to receiving signal {:?}", event.signal());
                    state.niri.stop_signal.stop();
                },
            )
            .unwrap();
    }

    // We block the signals early, so that they apply to all threads.
    // They are then blocked *again* by the `Signals` source. That's fine.
    pub fn block_early() -> io::Result<()> {
        set_sigmask(&preferred_sigset()?)
    }

    pub fn unblock_all() -> io::Result<()> {
        set_sigmask(&empty_sigset()?)
    }

    fn empty_sigset() -> io::Result<libc::sigset_t> {
        let mut sigset = mem::MaybeUninit::uninit();
        if unsafe { libc::sigemptyset(sigset.as_mut_ptr()) } == 0 {
            Ok(unsafe { sigset.assume_init() })
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn preferred_sigset() -> io::Result<libc::sigset_t> {
        let mut set = empty_sigset()?;
        unsafe {
            add_signal(&mut set, libc::SIGINT)?;
            add_signal(&mut set, libc::SIGTERM)?;
            add_signal(&mut set, libc::SIGHUP)?;
        }
        Ok(set)
    }

    // SAFETY: `signum` must be a valid signal number.
    unsafe fn add_signal(set: &mut libc::sigset_t, signum: libc::c_int) -> io::Result<()> {
        if unsafe { libc::sigaddset(set, signum) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn set_sigmask(set: &libc::sigset_t) -> io::Result<()> {
        let oldset = std::ptr::null_mut(); // ignore old mask
        if unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, set, oldset) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}
