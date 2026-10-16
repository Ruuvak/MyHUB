//! Keyboard worker: one background thread ("keychron-worker") shows the Keychron K2 HE
//! next to the other devices. Read only in this slice.
//!
//! Discovery does not talk to the keyboard: the worker enumerates HID interfaces every
//! 2 s while the keyboard is not connected. USB has priority. When the keyboard's raw HID
//! interface shows up it opens it and reads the whole snapshot once (and caches it, see
//! below). Until that read succeeds the device is absent from the list (same as the
//! headset).
//!
//! While connected over USB the worker sends nothing by itself: the Keychron launcher may
//! use the same interface and frames carry no sequence number, so a reply cannot be told
//! from the launcher's reply. It only listens (`Session::poll`) and re-enumerates every
//! 2 s to notice an unplug. A `refresh_device` re-reads the current profile and the
//! lighting values (Read tier); profile changes made on the keyboard are not pushed, so
//! the UI asks for a refresh when the panel opens.
//!
//! Without the cable, when the Keychron Link receiver's raw HID interface is listed, the
//! worker opens a receiver session, reads B1..B3 once and then B2 (the link state) every
//! [`LINK_POLL`]. A linked K2 HE is shown as connected through the receiver with the
//! settings of the last USB read (the cache file `cache/keychron-k2he.json` in the app
//! config dir, written after every USB read); a keyboard frame is never sent over the
//! receiver. Plugging the cable in closes the receiver session and switches to USB.
//!
//! An unrequested report that is not a known event, or a failed request because of one,
//! means another program talks to the keyboard (or the receiver): the detail gets
//! `otherConfigurator`, the worker sends nothing and goes on listening, and reads again
//! only after [`FOREIGN_QUIET`] without a foreign report. No serial number and no device
//! path is logged: this module never prints one.

use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use keychron_k2he::{
    DEVICE_ID, Incoming, KeyboardCache, LinkSnapshot, RawLink, Session, SessionError, Snapshot,
    cache_of, cache_path, list_interfaces, list_receiver_interfaces, read_link, read_link_state,
    read_profiles_info, read_snapshot, refresh_state, select_profile, to_detail,
    to_receiver_detail,
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
/// Gap between two B2 reads over the receiver.
pub const LINK_POLL: Duration = Duration::from_secs(5);

/// What the keyboard worker accepts.
pub enum KeychronCommand {
    /// Re-read the profile and lighting values and answer with the new detail (the
    /// cached one while another program talks to the keyboard).
    Refresh { reply: Sender<Option<DeviceDetail>> },
    /// Make `profile` (0-based, as sent by the UI) the active HE profile. Only over USB;
    /// A9 11 and its A9 10 check, never A9 1F. The answer is the new detail or a
    /// contract error code.
    SelectProfile {
        profile: i64,
        /// The caller's give-up instant minus a margin; a write starts only with
        /// [`MIN_WRITE_BUDGET`] left.
        deadline: Instant,
        reply: Sender<Result<DeviceDetail, String>>,
    },
    /// The settings changed: publish the detail again (`readOnly` may flip).
    SettingsChanged,
    /// Wakes the worker (shutdown).
    Wake,
}

/// How long the caller waits for a profile selection. The worker deadline is this minus the
/// 2 s margin.
pub const KEYBOARD_WRITE_WAIT: Duration = Duration::from_secs(15);

/// A write starts only when at least this much of the deadline is left. Worst case is about
/// 10 s: A9 10 (1 s, resent twice), A9 11 (1 s, never resent), verify A9 10 (3 x 1 s) and
/// the re-read A9 10 (3 x 1 s).
const MIN_WRITE_BUDGET: Duration = Duration::from_secs(11);

/// A queued profile selection.
struct PendingSelect {
    profile: i64,
    deadline: Instant,
    reply: Sender<Result<DeviceDetail, String>>,
}

/// What a profile selection did to the keyboard state.
struct SelectResult {
    /// `Ok` or the contract error code.
    answer: Result<(), String>,
    /// The cached snapshot or the foreign flag changed.
    changed: bool,
    /// The link is gone (reason).
    lost: Option<String>,
}

impl SelectResult {
    fn refused(code: &str) -> Self {
        Self {
            answer: Err(code.to_owned()),
            changed: false,
            lost: None,
        }
    }
}

/// The error code of a write that cannot reach a USB session: writes off first, then a
/// receiver that links the keyboard (`usbRequired`), otherwise `disconnected`.
fn no_usb_refusal(writes_allowed: bool, receiver_linked: bool) -> &'static str {
    if !writes_allowed {
        "writesDisabled"
    } else if receiver_linked {
        "usbRequired"
    } else {
        "disconnected"
    }
}

/// The `<kind>` of `deviceError:<kind>` for a session error.
fn error_kind(error: &SessionError) -> &'static str {
    match error {
        SessionError::Timeout => "timeout",
        SessionError::AmbiguousReply => "ambiguous",
        SessionError::BadReport(_) => "badReport",
        SessionError::Link(_) => "link",
        SessionError::Policy(_) => "policy",
        SessionError::Decode(_) => "decode",
        SessionError::DeviceStatus { .. } => "status",
        _ => "session",
    }
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

    /// Select the HE profile `profile` (0-based). Preconditions, before any frame:
    /// writes allowed, no other configurator, `profile` below the profile count, enough
    /// budget. Runs `select_profile(.., save = false)`: A9 11 only, never A9 1F, no retry.
    /// A failure after a frame may have gone out re-reads A9 10 here, so the snapshot is
    /// known again before anything else is served.
    fn select_profile(
        &mut self,
        writes_allowed: bool,
        profile: i64,
        deadline: Instant,
        now: Instant,
    ) -> SelectResult {
        if !writes_allowed {
            return SelectResult::refused("writesDisabled");
        }
        if self.other_configurator() {
            return SelectResult::refused("busy");
        }
        let count = i64::from(self.snapshot.profiles_info.count);
        let Some(index) = u8::try_from(profile).ok().filter(|p| i64::from(*p) < count) else {
            return SelectResult::refused("outOfRange");
        };
        if deadline.saturating_duration_since(now) < MIN_WRITE_BUDGET {
            return SelectResult::refused("deviceError:budget");
        }
        let mut result = SelectResult {
            answer: Ok(()),
            changed: false,
            lost: None,
        };
        match select_profile(&mut self.session, index, false) {
            Ok(outcome) => {
                result.changed = self.patch_profile(outcome.after);
            }
            Err(SessionError::VerifyFailed { got, .. }) => {
                // The keyboard told us what it is on now.
                result.changed = self.patch_profile(got);
                result.answer = Err("verifyFailed".to_owned());
            }
            Err(
                SessionError::ForeignTraffic
                | SessionError::BackingOff { .. }
                | SessionError::BadReport(_),
            ) => {
                result.changed = self.mark_foreign(now);
                result.answer = Err("busy".to_owned());
            }
            Err(error @ SessionError::Link(_)) => {
                result.answer = Err(format!("deviceError:{}", error_kind(&error)));
                result.lost = Some(error.to_string());
            }
            Err(error) => {
                result.answer = Err(format!("deviceError:{}", error_kind(&error)));
                // No frame went out for a policy refusal; anything else may have
                // changed the profile, so read it again.
                if !matches!(error, SessionError::Policy(_)) {
                    match read_profiles_info(&mut self.session) {
                        Ok(info) => result.changed = self.patch_profile(info.current),
                        Err(e) => match self.failed(&e, now) {
                            Ok(flipped) => result.changed = flipped,
                            Err(reason) => result.lost = Some(reason),
                        },
                    }
                }
            }
        }
        result
    }

    /// Sets the cached current profile; `true` when it changed.
    fn patch_profile(&mut self, current: u8) -> bool {
        let changed = self.snapshot.profiles_info.current != current;
        self.snapshot.profiles_info.current = current;
        changed
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

/// An open receiver session: the link state of the keyboard behind the Keychron Link
/// receiver. Only B2 is ever sent after the first read; generic over the link for tests.
struct Link<L: RawLink> {
    session: Session<L>,
    /// B3 of the first read.
    firmware: Option<String>,
    /// B2 says the linked device is a connected K2 HE.
    linked: bool,
    /// When foreign traffic was last seen; `Some` means nothing is sent.
    last_foreign: Option<Instant>,
    /// When B2 is read next.
    next_read: Instant,
    /// The last B2 read failed for a reason that is not foreign traffic (logged once).
    read_failing: bool,
}

impl<L: RawLink> Link<L> {
    fn new(session: Session<L>, first: &LinkSnapshot, now: Instant) -> Self {
        Self {
            session,
            firmware: first.firmware.clone(),
            linked: first.state.is_k2he(),
            last_foreign: None,
            next_read: now + LINK_POLL,
            read_failing: false,
        }
    }

    fn other_configurator(&self) -> bool {
        self.last_foreign.is_some()
    }

    fn detail(&self, cache: Option<&KeyboardCache>, connected: bool) -> DeviceDetail {
        to_receiver_detail(
            cache,
            self.firmware.clone(),
            connected,
            self.other_configurator(),
        )
    }

    fn mark_foreign(&mut self, now: Instant) -> bool {
        self.last_foreign.replace(now).is_none()
    }

    /// A failed read or listen, as for the USB session.
    fn failed(&mut self, error: &SessionError, now: Instant) -> Result<bool, String> {
        match error {
            SessionError::ForeignTraffic
            | SessionError::BackingOff { .. }
            | SessionError::BadReport(_) => Ok(self.mark_foreign(now)),
            SessionError::Link(_) => Err(error.to_string()),
            other => {
                if !self.read_failing {
                    eprintln!("myhub: keyboard link read failed: {other}");
                    self.read_failing = true;
                }
                if self.last_foreign.is_some() {
                    self.last_foreign = Some(now);
                }
                Ok(false)
            }
        }
    }

    /// One pass of the serving loop: B2 when due or asked for (and nobody else is
    /// talking), then one listen of up to `listen`. `Ok(true)` when the detail changed,
    /// `Err` with the reason when the link is gone. Nothing but B2 is ever written.
    fn step(&mut self, now: Instant, refresh: bool, listen: Duration) -> Result<bool, String> {
        let mut changed = false;
        // While another program talks nothing is sent; once it was quiet for
        // FOREIGN_QUIET one B2 read is tried and the flag clears when it succeeds.
        let quiet = self
            .last_foreign
            .is_some_and(|last| now.saturating_duration_since(last) >= FOREIGN_QUIET);
        if (refresh || now >= self.next_read) && (quiet || !self.other_configurator()) {
            self.next_read = now + LINK_POLL;
            match read_link_state(&mut self.session) {
                Ok(state) => {
                    self.read_failing = false;
                    let linked = state.is_k2he();
                    changed |= linked != self.linked || self.last_foreign.take().is_some();
                    self.linked = linked;
                }
                Err(e) => changed |= self.failed(&e, now)?,
            }
        }
        match self.session.poll(listen) {
            Ok(Some(Incoming::Foreign(_))) => changed |= self.mark_foreign(now),
            Ok(_) => {}
            Err(e) => changed |= self.failed(&e, now)?,
        }
        // The receiver sends no keyboard events.
        drop(self.session.drain_events());
        Ok(changed)
    }
}

/// True while the keyboard's raw HID interface is listed. Only enumerates.
fn present() -> bool {
    list_interfaces().is_ok_and(|found| found.iter().any(|i| i.matched))
}

/// True while the receiver's raw HID interface is listed. Only enumerates.
fn receiver_present() -> bool {
    list_receiver_interfaces().is_ok_and(|found| found.iter().any(|i| i.matched))
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

/// How a receiver session ended.
enum ReceiverEnd {
    Stopped,
    /// Scan again after this pause.
    Retry(Duration),
}

pub struct KeychronWorker {
    pub app: tauri::AppHandle,
    pub state: Arc<Mutex<ManagerState>>,
    pub commands: Receiver<KeychronCommand>,
    pub shutdown: Arc<AtomicBool>,
    /// App config dir; the cache of the last USB read lives under it.
    pub config_dir: Option<PathBuf>,
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
                    keyboard.configurable = false;
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
        let mut cache = self
            .config_dir
            .as_deref()
            .and_then(|dir| KeyboardCache::load(&cache_path(dir)));
        while !self.stopping() {
            let mut pause = SCAN_INTERVAL;
            // USB has priority: the receiver is only looked at while the cable is out.
            if !present() && receiver_present() {
                match self.serve_receiver(cache.as_ref(), &mut last_error) {
                    ReceiverEnd::Stopped => return,
                    ReceiverEnd::Retry(wait) => pause = wait,
                }
                self.wait(pause);
                continue;
            }
            match self.connect() {
                Ok(mut keyboard) => {
                    last_error = None;
                    eprintln!("myhub: keyboard connected");
                    let mut stamp = clock_ms();
                    self.store_cache(&mut cache, &keyboard.snapshot);
                    self.publish(self.stamped(&keyboard, true, stamp), true);
                    let outcome = self.serve(&mut keyboard, &mut stamp, &mut cache);
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

    /// Keeps the snapshot as the cache of the last USB read, in memory and on disk
    /// (atomic write; a failure logs one line without the path and is otherwise ignored).
    fn store_cache(&self, cache: &mut Option<KeyboardCache>, snapshot: &Snapshot) {
        let fresh = cache_of(snapshot, clock_ms());
        if let Some(dir) = self.config_dir.as_deref()
            && let Err(e) = fresh.save(&cache_path(dir))
        {
            eprintln!("myhub: keyboard cache not written: {:?}", e.kind());
        }
        *cache = Some(fresh);
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
        cache: &mut Option<KeyboardCache>,
    ) -> Outcome {
        let mut next_scan = Instant::now() + SCAN_INTERVAL;
        while !self.stopping() {
            let mut replies = Vec::new();
            let mut dirty = false;
            let mut selects = Vec::new();
            while let Ok(command) = self.commands.try_recv() {
                match command {
                    KeychronCommand::Refresh { reply } => replies.push(reply),
                    KeychronCommand::SelectProfile {
                        profile,
                        deadline,
                        reply,
                    } => selects.push(PendingSelect {
                        profile,
                        deadline,
                        reply,
                    }),
                    KeychronCommand::SettingsChanged => dirty = true,
                    KeychronCommand::Wake => {}
                }
            }
            if let Err(reason) = self.run_selects(keyboard, stamp, cache, selects) {
                answer_all(replies, Some(self.stamped(keyboard, false, *stamp)));
                return Outcome::Lost(reason);
            }
            let now = Instant::now();
            match keyboard.step(now, !replies.is_empty(), POLL) {
                Ok(changed) => {
                    if changed {
                        *stamp = clock_ms();
                        if !keyboard.other_configurator() {
                            self.store_cache(cache, &keyboard.snapshot);
                        }
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

    /// Runs the queued profile selections one after the other. `allowDeviceWrites` is
    /// read once per command. A change is cached, stored and published before the
    /// answer goes out. `Err` with the reason when the link is gone: the command that
    /// met it gets its error code, the ones behind it `disconnected`.
    fn run_selects(
        &self,
        keyboard: &mut Keyboard<keychron_k2he::HidapiLink>,
        stamp: &mut u64,
        cache: &mut Option<KeyboardCache>,
        selects: Vec<PendingSelect>,
    ) -> Result<(), String> {
        drain_selects(
            keyboard,
            selects,
            || self.writes_allowed(),
            |keyboard, changed, publish| {
                if changed {
                    *stamp = clock_ms();
                    if !keyboard.other_configurator() {
                        self.store_cache(cache, &keyboard.snapshot);
                    }
                }
                let detail = self.stamped(keyboard, true, *stamp);
                if publish {
                    self.publish(detail.clone(), true);
                }
                detail
            },
        )
    }

    /// The keyboard through the receiver: opens the receiver session, reads B1..B3 once
    /// and then B2 every [`LINK_POLL`], until shutdown, a lost link, or the cable showing
    /// up (the session is closed on return either way).
    fn serve_receiver(
        &self,
        cache: Option<&KeyboardCache>,
        last_error: &mut Option<String>,
    ) -> ReceiverEnd {
        let mut session = match Session::open_receiver() {
            Ok(session) => session,
            Err(SessionError::ReceiverNotFound) => return ReceiverEnd::Retry(SCAN_INTERVAL),
            Err(other) => return self.receiver_failed(other.to_string(), last_error),
        };
        let first = match read_link(&mut session) {
            Ok(first) => first,
            Err(
                SessionError::ForeignTraffic
                | SessionError::BackingOff { .. }
                | SessionError::BadReport(_),
            ) => {
                let message = "receiver busy: another program is talking to it".to_owned();
                if last_error.as_deref() != Some(message.as_str()) {
                    eprintln!("myhub: {message}");
                    *last_error = Some(message);
                }
                return ReceiverEnd::Retry(FOREIGN_CONNECT_WAIT);
            }
            Err(other) => return self.receiver_failed(other.to_string(), last_error),
        };
        *last_error = None;
        let mut link = Link::new(session, &first, Instant::now());
        eprintln!("myhub: keyboard receiver opened, linked: {}", link.linked);
        let mut stamp = clock_ms();
        if let Some(detail) = self.receiver_detail(&link, cache, stamp) {
            self.publish(detail, true);
        }
        let mut next_scan = Instant::now() + SCAN_INTERVAL;
        while !self.stopping() {
            let mut replies = Vec::new();
            let mut dirty = false;
            while let Ok(command) = self.commands.try_recv() {
                match command {
                    KeychronCommand::Refresh { reply } => replies.push(reply),
                    KeychronCommand::SelectProfile { reply, .. } => {
                        let code = no_usb_refusal(self.writes_allowed(), link.linked);
                        let _ = reply.send(Err(code.to_owned()));
                    }
                    KeychronCommand::SettingsChanged => dirty = true,
                    KeychronCommand::Wake => {}
                }
            }
            let now = Instant::now();
            match link.step(now, !replies.is_empty(), POLL) {
                Ok(changed) => {
                    if changed {
                        stamp = clock_ms();
                    }
                    dirty |= changed;
                }
                Err(reason) => {
                    eprintln!("myhub: keyboard receiver lost: {reason}");
                    stamp = clock_ms();
                    answer_all(replies, self.mark_gone(&link, cache, stamp));
                    return ReceiverEnd::Retry(SCAN_INTERVAL);
                }
            }
            if now >= next_scan {
                next_scan = now + SCAN_INTERVAL;
                if present() {
                    // The cable is in: USB takes over, the caller opens it right away.
                    answer_all(replies, lock(&self.state).find(DEVICE_ID));
                    return ReceiverEnd::Retry(Duration::ZERO);
                }
                if !receiver_present() {
                    eprintln!("myhub: keyboard receiver unplugged");
                    stamp = clock_ms();
                    answer_all(replies, self.mark_gone(&link, cache, stamp));
                    return ReceiverEnd::Retry(SCAN_INTERVAL);
                }
            }
            if dirty || !replies.is_empty() {
                let detail = self.receiver_detail(&link, cache, stamp);
                if let Some(detail) = &detail {
                    self.publish(detail.clone(), !replies.is_empty());
                }
                answer_all(replies, detail);
            }
        }
        ReceiverEnd::Stopped
    }

    fn receiver_failed(&self, message: String, last_error: &mut Option<String>) -> ReceiverEnd {
        if last_error.as_deref() != Some(message.as_str()) {
            eprintln!("myhub: keyboard receiver not usable: {message}");
            *last_error = Some(message);
        }
        self.mark_disconnected();
        ReceiverEnd::Retry(SCAN_INTERVAL)
    }

    /// The detail for the receiver state: connected when B2 says a K2 HE is linked,
    /// disconnected when it is not but the keyboard was published before, otherwise
    /// `None` (it stays absent).
    fn receiver_detail<L: RawLink>(
        &self,
        link: &Link<L>,
        cache: Option<&KeyboardCache>,
        at_ms: u64,
    ) -> Option<DeviceDetail> {
        if !link.linked && lock(&self.state).find(DEVICE_ID).is_none() {
            return None;
        }
        Some(self.finish(link.detail(cache, link.linked), link.linked, at_ms))
    }

    /// The receiver is gone: publish the keyboard as disconnected if it was shown.
    fn mark_gone<L: RawLink>(
        &self,
        link: &Link<L>,
        cache: Option<&KeyboardCache>,
        at_ms: u64,
    ) -> Option<DeviceDetail> {
        lock(&self.state).find(DEVICE_ID)?;
        let detail = self.finish(link.detail(cache, false), false, at_ms);
        self.publish(detail.clone(), false);
        Some(detail)
    }

    /// A receiver that cannot be used: a published, connected keyboard goes to
    /// disconnected (its last values stay).
    fn mark_disconnected(&self) {
        let cached = lock(&self.state).find(DEVICE_ID);
        if let Some(mut detail) = cached
            && detail.summary.connection == Connection::Connected
        {
            detail.summary.connection = Connection::Disconnected;
            detail.read_only = true;
            detail.updated_at_ms = clock_ms();
            if let Some(keyboard) = detail.keyboard.as_mut() {
                keyboard.other_configurator = false;
                keyboard.configurable = false;
            }
            self.publish(detail, false);
        }
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
        self.finish(keyboard.detail(connected), connected, at_ms)
    }

    fn finish(&self, mut detail: DeviceDetail, connected: bool, at_ms: u64) -> DeviceDetail {
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
                Ok(KeychronCommand::SelectProfile { reply, .. }) => {
                    let code = no_usb_refusal(self.writes_allowed(), false);
                    let _ = reply.send(Err(code.to_owned()));
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

/// Runs the queued selections one after the other. `writes_allowed` is asked once per
/// command. `settle(keyboard, changed, publish)` is called after each command (stamp,
/// cache, and publish when asked) and returns the detail to answer with; it is called
/// before the answer is sent, and for a command that ended the link only when something
/// changed (without publishing). `Err` with the reason when the link is gone: that
/// command gets its error code, the ones behind it `disconnected`.
fn drain_selects<L: RawLink>(
    keyboard: &mut Keyboard<L>,
    selects: Vec<PendingSelect>,
    mut writes_allowed: impl FnMut() -> bool,
    mut settle: impl FnMut(&Keyboard<L>, bool, bool) -> DeviceDetail,
) -> Result<(), String> {
    let mut selects = selects.into_iter();
    while let Some(select) = selects.next() {
        let result = keyboard.select_profile(
            writes_allowed(),
            select.profile,
            select.deadline,
            Instant::now(),
        );
        if let Some(reason) = result.lost {
            if result.changed {
                settle(keyboard, true, false);
            }
            let code = result.answer.err().unwrap_or_default();
            let _ = select.reply.send(Err(code));
            for rest in selects {
                let _ = rest.reply.send(Err("disconnected".to_owned()));
            }
            return Err(reason);
        }
        let detail = settle(keyboard, result.changed, result.changed);
        let _ = select.reply.send(result.answer.map(|()| detail));
    }
    Ok(())
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
    use std::sync::mpsc;

    use keychron_k2he::{LinkError, READ_BUF_LEN, REPORT_LEN};
    use keychron_proto::he::{HeInfo, ProfilesInfo};
    use keychron_proto::system::KcVersion;

    use super::*;

    #[derive(Default)]
    struct Shared {
        inbox: VecDeque<Vec<u8>>,
        written: usize,
        /// Frame bytes of every report written.
        sent: Vec<u8>,
        /// The reply to a B2 request; other requests are never answered.
        b2: Option<Vec<u8>>,
        /// Command, sub and first argument of every report written.
        frames: Vec<[u8; 3]>,
        /// A keyboard that answers A9 10 and A9 11 when set.
        he: Option<He>,
        /// The write that fails with a link error (1-based count of reports written).
        fail_write_at: Option<usize>,
    }

    /// A keyboard's HE profile state for the select tests.
    struct He {
        current: u8,
        /// A9 11 gets no reply and changes nothing.
        mute_select: bool,
        /// A9 11 is answered but the profile does not change.
        stuck: bool,
        /// A9 11 is answered by a report that is not its reply (another program).
        foreign_select: bool,
    }

    /// A link that holds what the test queued and counts the reports written; it
    /// never answers a request.
    struct Mock(Rc<RefCell<Shared>>);

    impl RawLink for Mock {
        fn write_report(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), LinkError> {
            let mut shared = self.0.borrow_mut();
            shared.written += 1;
            if shared.fail_write_at == Some(shared.written) {
                return Err(LinkError::Hid("unplugged".to_owned()));
            }
            shared.sent.push(report[1]);
            shared.frames.push([report[1], report[2], report[3]]);
            if report[1] == 0xA9
                && let Some(he) = shared.he.as_mut()
            {
                let reply = match report[2] {
                    0x10 => Some(vec![0xA9, 0x10, he.current, 3, 0x5C, 0x03, 20, 20]),
                    0x11 if he.foreign_select => Some(vec![0x55; 32]),
                    0x11 if !he.mute_select => {
                        if !he.stuck {
                            he.current = report[3];
                        }
                        Some(vec![0xA9, 0x11, 0, report[3]])
                    }
                    _ => None,
                };
                if let Some(mut reply) = reply {
                    reply.resize(32, 0);
                    shared.inbox.push_back(reply);
                }
            }
            if report[1] == 0xB2
                && let Some(reply) = shared.b2.clone()
            {
                shared.inbox.push_back(reply);
            }
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

    const B2_UP: [u8; 7] = [0xB2, 0x01, 0x34, 0x34, 0x20, 0x0E, 0x01];
    const B2_NO_KEYBOARD: [u8; 7] = [0xB2, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00];
    const B2_NOT_CONNECTED: [u8; 7] = [0xB2, 0x01, 0x34, 0x34, 0x20, 0x0E, 0x00];

    fn frame(bytes: &[u8]) -> Vec<u8> {
        let mut v = bytes.to_vec();
        v.resize(32, 0);
        v
    }

    fn link(linked: bool, b2: Option<&[u8]>, now: Instant) -> (Link<Mock>, Rc<RefCell<Shared>>) {
        let shared = Rc::new(RefCell::new(Shared::default()));
        shared.borrow_mut().b2 = b2.map(frame);
        let mut session =
            Session::with_link_for(Mock(Rc::clone(&shared)), keychron_proto::Target::Receiver);
        session.set_reply_timeout(Duration::from_millis(200));
        let link = Link {
            session,
            firmware: Some("d.3.1".to_owned()),
            linked,
            last_foreign: None,
            next_read: now + LINK_POLL,
            read_failing: false,
        };
        (link, shared)
    }

    #[test]
    fn receiver_reads_b2_only_when_due_and_follows_the_link_state() {
        let t0 = Instant::now();
        let (mut rx, shared) = link(false, Some(&B2_UP), t0);
        // Not due yet: nothing is sent.
        assert_eq!(rx.step(t0, false, Duration::ZERO), Ok(false));
        assert_eq!(shared.borrow().written, 0);
        // A refresh reads B2 now: a connected K2 HE.
        assert_eq!(rx.step(t0, true, Duration::ZERO), Ok(true));
        assert!(rx.linked);
        assert_eq!(shared.borrow().sent, vec![0xB2]);
        // The same answer again changes nothing; the poll interval holds the next read.
        assert_eq!(rx.step(t0, true, Duration::ZERO), Ok(false));
        assert_eq!(
            rx.step(t0 + LINK_POLL / 2, false, Duration::ZERO),
            Ok(false)
        );
        assert_eq!(shared.borrow().sent, vec![0xB2, 0xB2]);
        // Due again: the keyboard went away (B2 connected byte 0).
        shared.borrow_mut().b2 = Some(frame(&B2_NOT_CONNECTED));
        assert_eq!(rx.step(t0 + LINK_POLL, false, Duration::ZERO), Ok(true));
        assert!(!rx.linked);
        // Another device behind the receiver counts as not linked.
        let (mut rx, _) = link(true, Some(&B2_NO_KEYBOARD), t0);
        assert_eq!(rx.step(t0, true, Duration::ZERO), Ok(true));
        assert!(!rx.linked);
    }

    #[test]
    fn receiver_foreign_traffic_stops_sending_until_the_quiet_time_passed() {
        let t0 = Instant::now();
        let (mut rx, shared) = link(true, Some(&B2_UP), t0);
        shared.borrow_mut().inbox.push_back(frame(&[0x55; 32]));
        assert_eq!(rx.step(t0, false, Duration::ZERO), Ok(true));
        assert!(rx.other_configurator());
        assert!(rx.detail(None, true).keyboard.unwrap().other_configurator);
        // Neither a refresh nor the poll interval sends anything meanwhile.
        assert_eq!(rx.step(t0, true, Duration::ZERO), Ok(false));
        let later = t0 + LINK_POLL + Duration::from_secs(1);
        assert_eq!(rx.step(later, false, Duration::ZERO), Ok(false));
        assert_eq!(shared.borrow().written, 0);
        // After the quiet time the flag clears and B2 is due. The session own back-off runs
        // on the real clock (5 s from the foreign report above), so the read is refused
        // before a byte is written and a new quiet time starts: the attempt was made.
        let quiet = t0 + FOREIGN_QUIET;
        assert_eq!(rx.step(quiet, false, Duration::ZERO), Ok(false));
        assert_eq!(rx.last_foreign, Some(quiet));
        assert_eq!(shared.borrow().written, 0);
    }

    #[test]
    fn receiver_detail_is_connected_or_disconnected_and_never_configurable() {
        let t0 = Instant::now();
        let (rx, _) = link(true, None, t0);
        let up = rx.detail(None, true);
        assert_eq!(up.summary.connection, Connection::Connected);
        assert_eq!(up.transport, "2.4 GHz (Keychron Link)");
        assert!(!up.keyboard.unwrap().configurable);
        let down = rx.detail(None, false);
        assert_eq!(down.summary.connection, Connection::Disconnected);
    }

    const SELECT_BUDGET: Duration = Duration::from_secs(13);

    /// A keyboard on profile 1 (0-based) that answers A9 10 and A9 11.
    fn he_keyboard(mute_select: bool, stuck: bool) -> (Keyboard<Mock>, Rc<RefCell<Shared>>) {
        let (kb, shared) = keyboard();
        shared.borrow_mut().he = Some(He {
            current: 1,
            mute_select,
            stuck,
            foreign_select: false,
        });
        (kb, shared)
    }

    fn select(kb: &mut Keyboard<Mock>, writes: bool, profile: i64) -> SelectResult {
        let now = Instant::now();
        kb.select_profile(writes, profile, now + SELECT_BUDGET, now)
    }

    #[test]
    fn select_preconditions_refuse_before_any_frame() {
        let (mut kb, shared) = he_keyboard(false, false);
        let err = |r: SelectResult| r.answer.unwrap_err();
        assert_eq!(err(select(&mut kb, false, 2)), "writesDisabled");
        assert_eq!(err(select(&mut kb, true, 3)), "outOfRange");
        assert_eq!(err(select(&mut kb, true, -1)), "outOfRange");
        let now = Instant::now();
        let late = kb.select_profile(true, 2, now + Duration::from_secs(10), now);
        assert_eq!(err(late), "deviceError:budget");
        kb.mark_foreign(now);
        assert_eq!(err(select(&mut kb, true, 2)), "busy");
        // Writes off outranks busy.
        assert_eq!(err(select(&mut kb, false, 2)), "writesDisabled");
        assert_eq!(shared.borrow().written, 0);
    }

    #[test]
    fn no_usb_refusal_orders_writes_off_then_receiver_then_disconnected() {
        // Pure function: the receiver loop and `wait()` only call it and send no frame.
        assert_eq!(no_usb_refusal(true, true), "usbRequired");
        assert_eq!(no_usb_refusal(false, true), "writesDisabled");
        assert_eq!(no_usb_refusal(false, false), "writesDisabled");
        assert_eq!(no_usb_refusal(true, false), "disconnected");
    }

    #[test]
    fn select_sends_a9_10_a9_11_a9_10_and_shows_the_new_profile() {
        let (mut kb, shared) = he_keyboard(false, false);
        let result = select(&mut kb, true, 2);
        assert!(result.answer.is_ok() && result.changed && result.lost.is_none());
        assert_eq!(
            shared.borrow().frames,
            vec![[0xA9, 0x10, 0], [0xA9, 0x11, 2], [0xA9, 0x10, 0]]
        );
        let he = kb.detail(true).keyboard.unwrap().he.unwrap();
        assert_eq!(he.current_profile, 2);
        // The profile is already current: nothing is written.
        let again = select(&mut kb, true, 2);
        assert!(again.answer.is_ok() && !again.changed);
        assert_eq!(shared.borrow().frames.len(), 4);
    }

    #[test]
    fn select_reports_verify_failed_with_the_profile_the_keyboard_has() {
        let (mut kb, shared) = he_keyboard(false, true);
        let result = select(&mut kb, true, 2);
        assert_eq!(result.answer.unwrap_err(), "verifyFailed");
        assert_eq!(
            kb.detail(true)
                .keyboard
                .unwrap()
                .he
                .unwrap()
                .current_profile,
            1
        );
        assert_eq!(shared.borrow().frames.len(), 3);
    }

    #[test]
    fn select_timeout_rereads_once_and_never_resends_a9_11() {
        let (mut kb, shared) = he_keyboard(true, false);
        let result = select(&mut kb, true, 2);
        assert_eq!(result.answer.unwrap_err(), "deviceError:timeout");
        assert!(result.lost.is_none());
        assert_eq!(
            shared.borrow().frames,
            vec![[0xA9, 0x10, 0], [0xA9, 0x11, 2], [0xA9, 0x10, 0]]
        );
        assert_eq!(
            kb.detail(true)
                .keyboard
                .unwrap()
                .he
                .unwrap()
                .current_profile,
            1
        );
    }

    fn pending(profile: i64) -> (PendingSelect, mpsc::Receiver<Result<DeviceDetail, String>>) {
        let (reply, answer) = mpsc::channel();
        let deadline = Instant::now() + SELECT_BUDGET;
        (
            PendingSelect {
                profile,
                deadline,
                reply,
            },
            answer,
        )
    }

    fn settle_detail(kb: &Keyboard<Mock>) -> DeviceDetail {
        kb.detail(true)
    }

    #[test]
    fn a_foreign_report_during_select_answers_busy_without_a_re_read() {
        let (mut kb, shared) = he_keyboard(false, false);
        shared.borrow_mut().he.as_mut().unwrap().foreign_select = true;
        let result = select(&mut kb, true, 2);
        assert_eq!(result.answer.unwrap_err(), "busy");
        assert!(result.lost.is_none());
        assert!(kb.other_configurator());
        // A9 10, A9 11 and nothing after.
        assert_eq!(
            shared.borrow().frames,
            vec![[0xA9, 0x10, 0], [0xA9, 0x11, 2]]
        );
    }

    #[test]
    fn link_loss_mid_select_answers_its_code_and_the_queued_ones_disconnected() {
        let (mut kb, shared) = he_keyboard(false, false);
        // The second report written (A9 11) fails.
        shared.borrow_mut().fail_write_at = Some(2);
        let (first, first_rx) = pending(2);
        let (second, second_rx) = pending(0);
        let result = drain_selects(
            &mut kb,
            vec![first, second],
            || true,
            |kb, _, _| settle_detail(kb),
        );
        assert!(result.is_err());
        assert_eq!(
            first_rx.try_recv().unwrap().unwrap_err(),
            "deviceError:link"
        );
        assert_eq!(second_rx.try_recv().unwrap().unwrap_err(), "disconnected");
        // A9 10, the failed A9 11, no re-read.
        assert_eq!(shared.borrow().written, 2);
    }

    #[test]
    fn publish_comes_before_the_answer_and_writes_are_checked_per_command() {
        let (mut kb, shared) = he_keyboard(false, false);
        let (first, first_rx) = pending(2);
        let (second, second_rx) = pending(0);
        let published = RefCell::new(Vec::new());
        let asked = RefCell::new(0);
        let result = drain_selects(
            &mut kb,
            vec![first, second],
            || {
                // The setting flips between the two commands.
                *asked.borrow_mut() += 1;
                *asked.borrow() == 1
            },
            |kb, changed, publish| {
                if publish {
                    // Nobody has been answered yet.
                    assert!(first_rx.try_recv().is_err());
                    published.borrow_mut().push(changed);
                }
                settle_detail(kb)
            },
        );
        assert!(result.is_ok());
        assert_eq!(*published.borrow(), vec![true]);
        let detail = first_rx.try_recv().unwrap().unwrap();
        assert_eq!(detail.keyboard.unwrap().he.unwrap().current_profile, 2);
        assert_eq!(second_rx.try_recv().unwrap().unwrap_err(), "writesDisabled");
        // Only the first command sent frames: A9 10, A9 11, A9 10.
        assert_eq!(shared.borrow().written, 3);
        assert_eq!(*asked.borrow(), 2);
    }
}
