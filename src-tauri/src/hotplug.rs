//! Scan on system events instead of on a timer. On Windows a HID interface arrival or
//! removal (CM_Register_Notification, CfgMgr32) wakes the Logitech and Keychron workers,
//! so a plugged-in receiver or cable is found at once and the idle scans can be a slow
//! fallback ([`HOTPLUG_FALLBACK`]). The callback does no I/O: it only queues a unit on a
//! channel; a small thread coalesces a burst into one wake per [`DEBOUNCE`]. On Linux a udev monitor on hidraw nodes does the same. On other platforms hotplug is a stub: nothing is registered and the workers keep their short
//! idle intervals.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A burst of arrivals (a device exposes several HID interfaces) gives one wake per this
/// time.
pub const DEBOUNCE: Duration = Duration::from_millis(500);
/// Idle scan interval while hotplug is active: only a safety net.
pub const HOTPLUG_FALLBACK: Duration = Duration::from_secs(60);

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// `true` while a system notification is registered.
pub fn active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// The idle scan interval: `base` without hotplug, [`HOTPLUG_FALLBACK`] with it.
pub fn scan_interval(base: Duration) -> Duration {
    interval_for(active(), base)
}

fn interval_for(hotplug_active: bool, base: Duration) -> Duration {
    if hotplug_active {
        HOTPLUG_FALLBACK
    } else {
        base
    }
}

/// Turns a burst of events into one wake: the first event opens a window of `window`,
/// events inside it are absorbed, and the wake fires when the window ends.
#[derive(Debug)]
pub struct Coalescer {
    window: Duration,
    deadline: Option<Instant>,
}

impl Coalescer {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            deadline: None,
        }
    }

    /// An event arrived.
    pub fn event(&mut self, now: Instant) {
        self.deadline.get_or_insert(now + self.window);
    }

    /// When the pending wake is due; `None` without a pending event.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// `true` once (and re-arms) when the window of the pending event has ended.
    pub fn take_due(&mut self, now: Instant) -> bool {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
            true
        } else {
            false
        }
    }
}

pub use platform::Hotplug;

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
        CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER,
        CM_NOTIFY_FILTER_0_0, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CM_Register_Notification,
        CM_Unregister_Notification, CR_SUCCESS, HCMNOTIFICATION,
    };
    use windows_sys::core::GUID;

    use super::{ACTIVE, Coalescer, DEBOUNCE, Ordering};

    /// GUID_DEVINTERFACE_HID {4D1E55B2-F16F-11CF-88CB-001111000030}.
    const GUID_DEVINTERFACE_HID: GUID = GUID {
        data1: 0x4D1E_55B2,
        data2: 0xF16F,
        data3: 0x11CF,
        data4: [0x88, 0xCB, 0x00, 0x11, 0x11, 0x00, 0x00, 0x30],
    };

    /// A live registration. Dropping it unregisters (CM_Unregister_Notification waits for a
    /// callback in flight) and stops the coalescing thread.
    pub struct Hotplug {
        handle: HCMNOTIFICATION,
        /// The callback's context, freed after the unregister.
        context: *mut Sender<()>,
        thread: Option<JoinHandle<()>>,
    }

    // The handle and the context are only touched by `start` and `drop`.
    unsafe impl Send for Hotplug {}

    /// Runs on a CfgMgr32 thread: no I/O, only a send on the channel.
    unsafe extern "system" fn on_event(
        _handle: HCMNOTIFICATION,
        context: *const c_void,
        action: CM_NOTIFY_ACTION,
        _data: *const CM_NOTIFY_EVENT_DATA,
        _size: u32,
    ) -> u32 {
        if (action == CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL
            || action == CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL)
            && !context.is_null()
        {
            // SAFETY: the context is the boxed Sender, alive until after the unregister.
            let sender = unsafe { &*context.cast::<Sender<()>>() };
            let _ = sender.send(());
        }
        0
    }

    impl Hotplug {
        /// Registers for HID interface arrival and removal; `wake` is called from a
        /// helper thread, at most once per debounce window. `None` (logged) when the
        /// registration fails: the workers then keep their short idle intervals.
        pub fn start(wake: impl Fn() + Send + 'static) -> Option<Self> {
            let (tx, rx) = mpsc::channel::<()>();
            let context = Box::into_raw(Box::new(tx));
            let mut filter = CM_NOTIFY_FILTER {
                cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
                ..Default::default()
            };
            filter.FilterType = CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE;
            filter.u.DeviceInterface = CM_NOTIFY_FILTER_0_0 {
                ClassGuid: GUID_DEVINTERFACE_HID,
            };
            let mut handle: HCMNOTIFICATION = std::ptr::null_mut();
            // SAFETY: the filter and the out pointer are valid for the call; the context
            // outlives the registration (freed in `drop` after the unregister).
            let result = unsafe {
                CM_Register_Notification(&filter, context.cast(), Some(on_event), &mut handle)
            };
            if result != CR_SUCCESS {
                log_line!("myhub: hotplug notification unavailable (CONFIGRET {result})");
                // SAFETY: never handed to a callback.
                drop(unsafe { Box::from_raw(context) });
                return None;
            }
            let spawned = thread::Builder::new()
                .name("hotplug".into())
                .spawn(move || {
                    // However the thread ends, workers fall back to the short intervals.
                    let _clear = ClearActive;
                    coalesce(&rx, &wake)
                });
            let thread = match spawned {
                Ok(thread) => thread,
                Err(e) => {
                    log_line!("myhub: cannot start the hotplug thread: {e}");
                    // SAFETY: handle came from the successful registration above.
                    unsafe { CM_Unregister_Notification(handle) };
                    // SAFETY: no callback runs after the unregister returned.
                    drop(unsafe { Box::from_raw(context) });
                    return None;
                }
            };
            ACTIVE.store(true, Ordering::SeqCst);
            Some(Self {
                handle,
                context,
                thread: Some(thread),
            })
        }
    }

    struct ClearActive;

    impl Drop for ClearActive {
        fn drop(&mut self) {
            ACTIVE.store(false, Ordering::SeqCst);
        }
    }

    /// Waits for events; one wake per debounce window, fired when the window ends (the
    /// device is then listed more reliably than at the first interface).
    fn coalesce(rx: &mpsc::Receiver<()>, wake: &impl Fn()) {
        let mut pending = Coalescer::new(DEBOUNCE);
        loop {
            let event = match pending.deadline() {
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some(deadline) => {
                    rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                }
            };
            match event {
                Ok(()) => pending.event(Instant::now()),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if pending.take_due(Instant::now()) {
                wake();
            }
        }
    }

    impl Drop for Hotplug {
        fn drop(&mut self) {
            ACTIVE.store(false, Ordering::SeqCst);
            // SAFETY: the handle is valid and unregistered once; it waits for a callback
            // in flight, so the context can be freed afterwards.
            unsafe { CM_Unregister_Notification(self.handle) };
            // SAFETY: the context is the box made in `start`, no callback can use it now.
            drop(unsafe { Box::from_raw(self.context) });
            // The channel is closed now: the thread ends.
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::os::fd::AsRawFd;
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    use super::{ACTIVE, Coalescer, DEBOUNCE, Ordering};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, mpsc};

    /// How long one poll waits, so the thread notices the stop flag.
    const POLL_MS: i32 = 500;

    /// A udev monitor on hidraw nodes, read on a helper thread. Dropping it stops and joins
    /// the thread.
    pub struct Hotplug {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Hotplug {
        /// Listens for hidraw arrivals and removals; `wake` is called from the helper
        /// thread, at most once per debounce window. `None` (logged) when the monitor or
        /// the thread cannot start: the workers then keep their short idle intervals.
        pub fn start(wake: impl Fn() + Send + 'static) -> Option<Self> {
            let stop = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&stop);
            let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
            // Set before the thread runs, so its exit (ClearActive) always comes after.
            ACTIVE.store(true, Ordering::SeqCst);
            let spawned = thread::Builder::new()
                .name("hotplug".into())
                .spawn(move || {
                    // However the thread ends, workers fall back to the short intervals.
                    let _clear = ClearActive;
                    // Built here, so the socket never crosses threads.
                    let socket = match udev::MonitorBuilder::new()
                        .and_then(|builder| builder.match_subsystem("hidraw"))
                        .and_then(|builder| builder.listen())
                    {
                        Ok(socket) => socket,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    watch(&socket, &flag, &wake);
                });
            let thread = match spawned {
                Ok(thread) => thread,
                Err(e) => {
                    ACTIVE.store(false, Ordering::SeqCst);
                    log_line!("myhub: cannot start the hotplug thread: {e}");
                    return None;
                }
            };
            match ready_rx.recv() {
                Ok(Ok(())) => Some(Self {
                    stop,
                    thread: Some(thread),
                }),
                Ok(Err(e)) => {
                    log_line!("myhub: udev hotplug unavailable: {e}");
                    let _ = thread.join();
                    None
                }
                Err(_) => {
                    let _ = thread.join();
                    None
                }
            }
        }
    }

    struct ClearActive;

    impl Drop for ClearActive {
        fn drop(&mut self) {
            ACTIVE.store(false, Ordering::SeqCst);
        }
    }

    /// Polls the monitor socket; events feed the coalescer and one wake fires when the
    /// debounce window ends.
    fn watch(socket: &udev::MonitorSocket, stop: &AtomicBool, wake: &impl Fn()) {
        let mut pending = Coalescer::new(DEBOUNCE);
        while !stop.load(Ordering::SeqCst) {
            let mut fds = [libc::pollfd {
                fd: socket.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            }];
            // SAFETY: one valid pollfd for the call; the socket outlives it.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, POLL_MS) };
            if ready > 0 {
                let bad = fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL);
                if bad != 0 {
                    log_line!("myhub: udev socket failed, hotplug stops");
                    return;
                }
                // Drain the socket so the next poll blocks again.
                if socket.iter().count() > 0 {
                    pending.event(Instant::now());
                }
            } else if ready < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                log_line!("myhub: udev poll failed, hotplug stops");
                return;
            }
            if pending.take_due(Instant::now()) {
                wake();
            }
        }
    }

    impl Drop for Hotplug {
        fn drop(&mut self) {
            ACTIVE.store(false, Ordering::SeqCst);
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    /// Stub: no system notification on this platform yet.
    pub struct Hotplug;

    impl Hotplug {
        pub fn start(_wake: impl Fn() + Send + 'static) -> Option<Self> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_gives_one_wake_per_window() {
        let t0 = Instant::now();
        let mut burst = Coalescer::new(DEBOUNCE);
        assert!(!burst.take_due(t0), "nothing pending");
        burst.event(t0);
        burst.event(t0 + Duration::from_millis(100));
        burst.event(t0 + Duration::from_millis(400));
        assert!(!burst.take_due(t0 + Duration::from_millis(499)));
        assert!(burst.take_due(t0 + DEBOUNCE));
        assert!(!burst.take_due(t0 + DEBOUNCE * 2), "one wake for the burst");
        // The next event opens a new window.
        let t1 = t0 + Duration::from_secs(5);
        burst.event(t1);
        assert_eq!(burst.deadline(), Some(t1 + DEBOUNCE));
        assert!(burst.take_due(t1 + DEBOUNCE));
    }

    #[test]
    fn idle_interval_is_a_fallback_only_with_hotplug() {
        let base = Duration::from_secs(10);
        assert_eq!(interval_for(false, base), base);
        assert_eq!(interval_for(true, base), HOTPLUG_FALLBACK);
    }
}
