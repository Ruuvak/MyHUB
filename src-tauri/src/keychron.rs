//! Keyboard worker: one background thread ("keychron-worker") shows the Keychron K2 HE
//! next to the other devices. Read only in this slice.
//!
//! Discovery does not talk to the keyboard: the worker enumerates HID interfaces every
//! 2 s while the keyboard is not connected. When the raw HID interface shows up it opens
//! it and reads the whole snapshot once. Until that read succeeds the device is absent
//! from the list (same as the headset).
//!
//! While connected the worker sends nothing by itself: the Keychron launcher may use
//! the same interface and frames carry no sequence number, so a reply cannot be told from
//! the launcher's reply. It only listens (`Session::poll`) and re-enumerates every 2 s to
//! notice an unplug. A `refresh_device` re-reads the current profile and the lighting
//! values (Read tier); profile changes made on the keyboard are not pushed, so the UI
//! asks for a refresh when the panel opens.
//!
//! An unrequested report that is not a known event, or a failed request because of one,
//! means another program talks to the keyboard: the detail gets `otherConfigurator`, the
//! worker sends nothing and goes on listening, and reads the whole snapshot again only
//! after [`FOREIGN_QUIET`] without a foreign report. No serial number and no device path
//! is logged: this module never prints one.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use keychron_k2he::{
    DEVICE_ID, Incoming, RawLink, Session, SessionError, Snapshot, list_interfaces, read_snapshot,
    refresh_state, to_detail,
};
use myhub_core::{Connection, DeviceDetail};
use tauri::Manager;

use crate::device_manager::{ManagerState, Slot, clock_ms, lock, publish_slot};
use crate::settings::SettingsState;

/// Pause between enumerations while the keyboard is absent, and between the checks for
/// an unplug while it is connected.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// Pause after a connect that failed because another program was talking.
const FOREIGN_CONNECT_WAIT: Duration = Duration::from_secs(10);
/// One listen of the serving loop; also the latency of a refresh command and of
/// shutdown.
const POLL: Duration = Duration::from_millis(200);
/// No foreign report for this long before the worker sends anything again.
pub const FOREIGN_QUIET: Duration = Duration::from_secs(10);

/// What the keyboard worker accepts.
pub enum KeychronCommand {
    /// Re-read the profile and lighting values and answer with the new detail (the
    /// cached one while another program talks to the keyboard).
    Refresh { reply: Sender<Option<DeviceDetail>> },
    /// The settings changed: publish the detail again (`readOnly` may flip).
    SettingsChanged,
    /// Wakes the worker (shutdown).
    Wake,
}

/// An open session with its cached snapshot and the foreign traffic state. Generic over
/// the link so tests can use a mock.
struct Keyboard<L: RawLink> {
    session: Session<L>,
    snapshot: Snapshot,
    /// When the last foreign report or foreign failure was seen; `Some` means another
    /// configurator is active and nothing is sent.
    last_foreign: Option<Instant>,
}

impl<L: RawLink> Keyboard<L> {
    fn new(session: Session<L>, snapshot: Snapshot) -> Self {
        Self {
            session,
            snapshot,
            last_foreign: None,
        }
    }

    fn other_configurator(&self) -> bool {
        self.last_foreign.is_some()
    }

    fn detail(&self, connected: bool) -> DeviceDetail {
        to_detail(
            &self.snapshot,
            connected,
            connected && self.other_configurator(),
        )
    }

    /// Foreign traffic seen at `now`. `true` when this starts the foreign state.
    fn mark_foreign(&mut self, now: Instant) -> bool {
        self.last_foreign.replace(now).is_none()
    }

    /// A failed request or listen. Foreign traffic starts or extends the quiet period
    /// (`Ok(true)` when the flag flipped), a link error ends the session (`Err`), any
    /// other failure is logged.
    fn failed(&mut self, error: &SessionError, now: Instant) -> Result<bool, String> {
        match error {
            SessionError::ForeignTraffic
            | SessionError::BackingOff { .. }
            | SessionError::BadReport(_) => Ok(self.mark_foreign(now)),
            SessionError::Link(_) => Err(error.to_string()),
            other => {
                eprintln!("myhub: keyboard read failed: {other}");
                // A failed re-read after the quiet period: wait another one.
                if self.last_foreign.is_some() {
                    self.last_foreign = Some(now);
                }
                Ok(false)
            }
        }
    }

    /// One pass of the serving loop: the light refresh if asked (and nobody else is
    /// talking), one listen of up to `listen`, the events folded into the snapshot, and
    /// the full re-read once the keyboard was quiet for [`FOREIGN_QUIET`]. `Ok(true)`
    /// when the detail changed, `Err` with the reason when the link is gone.
    fn step(&mut self, now: Instant, refresh: bool, listen: Duration) -> Result<bool, String> {
        let mut changed = false;
        if refresh && !self.other_configurator() {
            match refresh_state(&mut self.session, &mut self.snapshot) {
                Ok(()) => changed = true,
                Err(e) => changed |= self.failed(&e, now)?,
            }
        }
        match self.session.poll(listen) {
            Ok(Some(Incoming::Foreign(_))) => changed |= self.mark_foreign(now),
            Ok(_) => {}
            Err(e) => changed |= self.failed(&e, now)?,
        }
        for event in self.session.drain_events() {
            changed |= self.snapshot.apply_event(event);
        }
        if self
            .last_foreign
            .is_some_and(|last| now.saturating_duration_since(last) >= FOREIGN_QUIET)
        {
            match read_snapshot(&mut self.session) {
                Ok(fresh) => {
                    self.snapshot = fresh;
                    self.last_foreign = None;
                    changed = true;
                }
                Err(e) => changed |= self.failed(&e, now)?,
            }
        }
        Ok(changed)
    }
}

/// True while the keyboard's raw HID interface is listed. Only enumerates.
fn present() -> bool {
    list_interfaces().is_ok_and(|found| found.iter().any(|i| i.matched))
}

/// Why no session could be opened.
enum ConnectError {
    /// The keyboard is not there: nothing to log.
    Absent,
    /// Another program was talking to the keyboard.
    Foreign,
    /// Anything else; the text is logged once per distinct message.
    Failed(String),
}

enum Outcome {
    Stopped,
    Lost(String),
}

pub struct KeychronWorker {
    pub app: tauri::AppHandle,
    pub state: Arc<Mutex<ManagerState>>,
    pub commands: Receiver<KeychronCommand>,
    pub shutdown: Arc<AtomicBool>,
}

impl KeychronWorker {
    pub fn run(mut self) {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| self.run_loop()));
        if outcome.is_err() {
            eprintln!("myhub: the keyboard worker panicked");
            // Nobody serves the keyboard any more: show it as disconnected.
            let cached = lock(&self.state).find(DEVICE_ID);
            if let Some(mut detail) = cached {
                detail.summary.connection = Connection::Disconnected;
                detail.read_only = true;
                if let Some(keyboard) = detail.keyboard.as_mut() {
                    keyboard.other_configurator = false;
                }
                self.publish(detail, false);
            }
        }
    }

    fn stopping(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    fn publish(&self, detail: DeviceDetail, force: bool) {
        publish_slot(
            &self.app,
            &self.state,
            self.stopping(),
            Slot::Keychron,
            Some(detail),
            force,
        );
    }

    fn run_loop(&mut self) {
        let mut last_error: Option<String> = None;
        while !self.stopping() {
            let mut pause = SCAN_INTERVAL;
            match self.connect() {
                Ok(mut keyboard) => {
                    last_error = None;
                    eprintln!("myhub: keyboard connected");
                    let mut stamp = clock_ms();
                    self.publish(self.stamped(&keyboard, true, stamp), true);
                    let outcome = self.serve(&mut keyboard, &mut stamp);
                    // Closes the interface.
                    match outcome {
                        Outcome::Stopped => return,
                        Outcome::Lost(reason) => {
                            eprintln!("myhub: keyboard link lost: {reason}");
                            self.publish(self.stamped(&keyboard, false, stamp), false);
                        }
                    }
                }
                Err(ConnectError::Absent) => last_error = None,
                Err(ConnectError::Foreign) => {
                    if last_error.as_deref() != Some("foreign") {
                        eprintln!("myhub: keyboard busy: another program is talking to it");
                        last_error = Some("foreign".to_owned());
                    }
                    pause = FOREIGN_CONNECT_WAIT;
                }
                Err(ConnectError::Failed(message)) => {
                    if last_error.as_deref() != Some(message.as_str()) {
                        eprintln!("myhub: keyboard not connected: {message}");
                        last_error = Some(message);
                    }
                }
            }
            self.wait(pause);
        }
    }

    /// Looks for the interface, opens it and reads the snapshot.
    fn connect(&self) -> Result<Keyboard<keychron_k2he::HidapiLink>, ConnectError> {
        if !present() {
            return Err(ConnectError::Absent);
        }
        let mut session = Session::open().map_err(|e| match e {
            SessionError::NotFound => ConnectError::Absent,
            other => ConnectError::Failed(other.to_string()),
        })?;
        let snapshot = read_snapshot(&mut session).map_err(|e| match e {
            SessionError::ForeignTraffic
            | SessionError::BackingOff { .. }
            | SessionError::BadReport(_) => ConnectError::Foreign,
            other => ConnectError::Failed(other.to_string()),
        })?;
        Ok(Keyboard::new(session, snapshot))
    }

    /// Listens until shutdown or a lost link, publishing every change.
    fn serve(
        &self,
        keyboard: &mut Keyboard<keychron_k2he::HidapiLink>,
        stamp: &mut u64,
    ) -> Outcome {
        let mut next_scan = Instant::now() + SCAN_INTERVAL;
        while !self.stopping() {
            let mut replies = Vec::new();
            let mut dirty = false;
            while let Ok(command) = self.commands.try_recv() {
                match command {
                    KeychronCommand::Refresh { reply } => replies.push(reply),
                    KeychronCommand::SettingsChanged => dirty = true,
                    KeychronCommand::Wake => {}
                }
            }
            let now = Instant::now();
            match keyboard.step(now, !replies.is_empty(), POLL) {
                Ok(changed) => {
                    if changed {
                        *stamp = clock_ms();
                    }
                    dirty |= changed;
                }
                Err(reason) => {
                    *stamp = clock_ms();
                    let detail = self.stamped(keyboard, false, *stamp);
                    answer_all(replies, Some(detail));
                    return Outcome::Lost(reason);
                }
            }
            if now >= next_scan {
                next_scan = now + SCAN_INTERVAL;
                if !present() {
                    let detail = self.stamped(keyboard, false, *stamp);
                    answer_all(replies, Some(detail));
                    return Outcome::Lost("the keyboard was unplugged".to_owned());
                }
            }
            if dirty || !replies.is_empty() {
                let detail = self.stamped(keyboard, true, *stamp);
                self.publish(detail.clone(), !replies.is_empty());
                answer_all(replies, Some(detail));
            }
        }
        Outcome::Stopped
    }

    fn writes_allowed(&self) -> bool {
        self.app
            .try_state::<SettingsState>()
            .is_some_and(|s| s.get().allow_device_writes)
    }

    /// The detail with the read time stamped on it and `readOnly` from the settings.
    fn stamped<L: RawLink>(
        &self,
        keyboard: &Keyboard<L>,
        connected: bool,
        at_ms: u64,
    ) -> DeviceDetail {
        let mut detail = keyboard.detail(connected);
        detail.updated_at_ms = at_ms;
        detail.read_only = !(self.writes_allowed() && connected);
        detail
    }

    /// Sleeps up to `duration` in short slices. A refresh command is answered with the
    /// cached detail (there is no session to read from) and does not end the sleep.
    fn wait(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || self.stopping() {
                return;
            }
            let slice = left.min(Duration::from_millis(250));
            match self.commands.recv_timeout(slice) {
                Ok(KeychronCommand::Refresh { reply }) => {
                    let cached = lock(&self.state).find(DEVICE_ID);
                    let _ = reply.send(cached);
                }
                // Disconnected: readOnly is already true.
                Ok(KeychronCommand::SettingsChanged | KeychronCommand::Wake)
                | Err(RecvTimeoutError::Timeout) => {}
                // The manager is gone: nothing to wake us, keep the pace.
                Err(RecvTimeoutError::Disconnected) => thread::sleep(slice),
            }
        }
    }
}

fn answer_all(replies: Vec<Sender<Option<DeviceDetail>>>, detail: Option<DeviceDetail>) {
    for reply in replies {
        let _ = reply.send(detail.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use keychron_k2he::{LinkError, READ_BUF_LEN, REPORT_LEN};
    use keychron_proto::he::{HeInfo, ProfilesInfo};
    use keychron_proto::system::KcVersion;

    use super::*;

    #[derive(Default)]
    struct Shared {
        inbox: VecDeque<Vec<u8>>,
        written: usize,
    }

    /// A link that holds what the test queued and counts the reports written; it
    /// never answers a request.
    struct Mock(Rc<RefCell<Shared>>);

    impl RawLink for Mock {
        fn write_report(&mut self, _report: &[u8; REPORT_LEN]) -> Result<(), LinkError> {
            self.0.borrow_mut().written += 1;
            Ok(())
        }

        fn read_report(
            &mut self,
            buf: &mut [u8; READ_BUF_LEN],
            _timeout_ms: i32,
        ) -> Result<usize, LinkError> {
            match self.0.borrow_mut().inbox.pop_front() {
                Some(r) => {
                    buf[..r.len()].copy_from_slice(&r);
                    Ok(r.len())
                }
                None => Ok(0),
            }
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            via_protocol: 12,
            kc: KcVersion {
                kc_version: 2,
                instruction_set: 2,
            },
            firmware: Some("v1.2.1".to_owned()),
            features: None,
            layer: None,
            misc: None,
            layout_lang: None,
            timers: None,
            layer_count: Some(4),
            macro_count: None,
            macro_buffer_size: None,
            light: keychron_k2he::LightSettings::default(),
            lighting: keychron_k2he::Lighting::default(),
            he: HeInfo {
                version: 4,
                feature_byte: 0,
                switch_axis: false,
                reset_single_key: false,
                dkr: false,
                turbo: false,
                gamepad_disable: false,
                support_mechanical: false,
            },
            profiles_info: ProfilesInfo {
                current: 1,
                count: 3,
                size: 860,
                dks_count: 20,
                snap_count: 20,
            },
            profiles: Vec::new(),
            curve: None,
            keymaps: None,
            elapsed_ms: 0,
            requests: 0,
            notes: Vec::new(),
        }
    }

    fn keyboard() -> (Keyboard<Mock>, Rc<RefCell<Shared>>) {
        let shared = Rc::new(RefCell::new(Shared::default()));
        let mut session = Session::with_link(Mock(Rc::clone(&shared)));
        session.set_reply_timeout(Duration::from_millis(200));
        (Keyboard::new(session, snapshot()), shared)
    }

    #[test]
    fn foreign_traffic_sets_the_flag_and_nothing_is_sent_until_the_quiet_time_passed() {
        let (mut kb, shared) = keyboard();
        let t0 = Instant::now();
        // A report that is neither a reply nor a known event.
        shared.borrow_mut().inbox.push_back(vec![0x55; 32]);
        assert_eq!(kb.step(t0, false, Duration::ZERO), Ok(true));
        assert!(kb.other_configurator());
        assert!(kb.detail(true).keyboard.unwrap().other_configurator);
        // A refresh while another program talks answers the cache and sends nothing.
        assert_eq!(kb.step(t0, true, Duration::ZERO), Ok(false));
        let just_before = t0 + FOREIGN_QUIET - Duration::from_millis(1);
        assert_eq!(kb.step(just_before, true, Duration::ZERO), Ok(false));
        assert_eq!(shared.borrow().written, 0);
        assert!(kb.other_configurator());
        // More foreign traffic restarts the quiet time.
        shared.borrow_mut().inbox.push_back(vec![0x66; 32]);
        assert_eq!(kb.step(just_before, false, Duration::ZERO), Ok(false));
        let later = t0 + FOREIGN_QUIET + Duration::from_secs(1);
        assert_eq!(kb.step(later, false, Duration::ZERO), Ok(false));
        assert_eq!(shared.borrow().written, 0);
        // After the quiet time the worker tries the full read. The session own back-off
        // runs on the real clock (5 s, started by the foreign reports above), so here the
        // read is refused before a byte is written: the flag stays and another quiet time
        // starts, which shows the attempt was made.
        let quiet = just_before + FOREIGN_QUIET;
        assert_eq!(kb.step(quiet, false, Duration::ZERO), Ok(false));
        assert_eq!(kb.last_foreign, Some(quiet));
        assert_eq!(shared.borrow().written, 0);
        assert!(kb.other_configurator());
    }

    #[test]
    fn events_update_the_snapshot_without_sending_and_a_clean_listen_changes_nothing() {
        let (mut kb, shared) = keyboard();
        let now = Instant::now();
        assert_eq!(kb.step(now, false, Duration::ZERO), Ok(false));
        let mut event = vec![0xA9, 0x11, 0x00, 0x02];
        event.resize(32, 0);
        shared.borrow_mut().inbox.push_back(event);
        assert_eq!(kb.step(now, false, Duration::ZERO), Ok(true));
        let he = kb.detail(true).keyboard.unwrap().he.unwrap();
        assert_eq!(he.current_profile, 2);
        assert!(!kb.other_configurator());
        assert_eq!(shared.borrow().written, 0);
    }
}
