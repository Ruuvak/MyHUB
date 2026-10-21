//! Headset worker: one background thread ("sony-worker") keeps the Sony WF-C710N
//! detail up to date over Bluetooth RFCOMM and publishes it next to the G502.
//!
//! The worker finds the headset among the paired devices, connects, reads the
//! snapshot, then polls the session: notifications (battery, NC/ASM, EQ,
//! multipoint, sidetone) are folded into the snapshot and republished, and the
//! battery is read again every minute. When the link drops the detail stays
//! (marked disconnected) and the worker reconnects with a back off of 2 s, 5 s,
//! then every 10 s.
//!
//! Writes (NC/ASM, EQ, sidetone, multipoint) run on this thread only, one at a time
//! and at least 250 ms apart, and only while `allowDeviceWrites` is on and the link is
//! up; each goes through the device crate's write path, which verifies the value with
//! a GET. A multipoint change drops the link: the worker publishes the headset as
//! disconnected and reconnects with the short schedule. No Bluetooth address is
//! logged: this module never prints one.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use myhub_core::{Connection, DeviceDetail};
use myhub_rfcomm::{PairedDevice, RfcommError, RfcommLink};
use serde::Deserialize;
use sony_wf_c710n::{
    ALERT_WAIT, AmbientMode, Applied, DROP_WAIT, Finder, Item, MultipointOutcome, NcAsmMode,
    REPLY_TIMEOUT, Session, SessionError, Snapshot, VERIFY_WAIT, WriteError, open, read_snapshot,
    reread, to_detail,
};

use tauri::Manager;

use crate::device_manager::{FlashFlag, ManagerState, Slot, clock_ms, lock, publish_slot};
use crate::settings::SettingsState;

// Radio use: an RFCOMM connect to buds that are not connected (in the case) pages the
// device until it times out, and SDP queries do the same. Intel Bluetooth driver
// activity is a known source of DPC latency spikes [INF], and the user heard system
// audio pop about once a minute while this worker retried [OBS]. So the worker opens a
// link only while the paired device reports connected (ACL up), and otherwise only
// reads the local paired list, which uses no radio.
/// Pause between local presence checks while the headset is not connected.
const PRESENCE_CHECK_INTERVAL: Duration = Duration::from_secs(15);
/// Pauses between connect attempts while the device reports connected but the RFCOMM
/// connect fails: 2 s, 5 s, then at most one attempt per minute.
const RECONNECT_DELAYS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(60),
];
/// One transport read of the serving loop; also the latency of a refresh command
/// and of shutdown.
const POLL_MS: u32 = 200;
/// Interval of the battery request while connected.
const BATTERY_REFRESH: Duration = Duration::from_secs(60);

/// Least time between two writes to the headset.
const WRITE_INTERVAL: Duration = Duration::from_millis(250);
/// How long a headset write command waits for the worker: a multipoint change takes
/// 5 to 9 s on the device (alert, confirmation, link drop); a write starts only when
/// [`required_budget`] is left (multipoint 25 s, EQ bands 28 s, others 14 s). The worker
/// deadline is this wait minus `WRITE_DEADLINE_MARGIN`.
pub const SONY_WRITE_WAIT: Duration = Duration::from_secs(40);

/// What the headset worker accepts.
pub enum SonyCommand {
    /// Read the state again and answer with the new detail (`None` while the
    /// headset is not connected).
    Refresh { reply: Sender<Option<DeviceDetail>> },
    /// Change a setting. The answer is the new detail or an error code of the IPC
    /// contract. After `deadline` the caller has given up and nothing is sent.
    Write {
        op: SonyWrite,
        deadline: Instant,
        reply: Sender<Result<DeviceDetail, String>>,
    },
    /// The settings changed: publish the detail again (`readOnly` may flip).
    SettingsChanged,
    /// Wakes the worker (shutdown).
    Wake,
}

/// A headset setting the user can change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SonyWrite {
    NcAsm {
        mode: NcAsmMode,
        ambient_mode: AmbientMode,
        level: u8,
    },
    EqPreset(u8),
    EqBands(Vec<u8>),
    Sidetone(bool),
    Multipoint {
        on: bool,
        confirm_disconnect: bool,
    },
}

/// `mode` argument of `set_nc_asm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NcModeArg {
    NoiseCancelling,
    Ambient,
}

/// `ambientMode` argument of `set_nc_asm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AmbientArg {
    Normal,
    Voice,
}

/// `setting` argument of `set_headset_toggle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToggleArg {
    Sidetone,
    Multipoint,
}

fn byte(value: i64) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| "outOfRange".to_owned())
}

impl SonyWrite {
    pub fn nc_asm(mode: NcModeArg, ambient: AmbientArg, level: i64) -> Result<Self, String> {
        Ok(Self::NcAsm {
            mode: match mode {
                NcModeArg::NoiseCancelling => NcAsmMode::NoiseCancelling,
                NcModeArg::Ambient => NcAsmMode::Ambient,
            },
            ambient_mode: match ambient {
                AmbientArg::Normal => AmbientMode::Normal,
                AmbientArg::Voice => AmbientMode::Voice,
            },
            level: byte(level)?,
        })
    }

    pub fn eq_preset(preset: i64) -> Result<Self, String> {
        Ok(Self::EqPreset(byte(preset)?))
    }

    pub fn eq_bands(bands: &[i64]) -> Result<Self, String> {
        Ok(Self::EqBands(
            bands.iter().map(|&b| byte(b)).collect::<Result<_, _>>()?,
        ))
    }

    pub fn toggle(setting: ToggleArg, on: bool, confirm_disconnect: bool) -> Self {
        match setting {
            ToggleArg::Sidetone => Self::Sidetone(on),
            ToggleArg::Multipoint => Self::Multipoint {
                on,
                confirm_disconnect,
            },
        }
    }
}

/// The checks every headset write passes before a frame is sent, in this order:
/// writes allowed in the settings, link up, and for multipoint the confirmation that
/// the link will drop.
pub fn precheck(op: &SonyWrite, writes_allowed: bool, connected: bool) -> Result<(), &'static str> {
    if !writes_allowed {
        return Err("writesDisabled");
    }
    if !connected {
        return Err("disconnected");
    }
    if matches!(
        op,
        SonyWrite::Multipoint {
            confirm_disconnect: false,
            ..
        }
    ) {
        return Err("confirmRequired");
    }
    Ok(())
}

/// The real worst case of a write from its start to its answer, every wait at its limit:
/// idle wait, GET, SET ACK and read-back GET (each a reply timeout) plus the wait for the
/// notification; multipoint instead has the GET, two ACKs and the idle wait, the alert
/// wait and the link drop; bands may also select CUSTOM first (a second round).
pub fn required_budget(op: &SonyWrite) -> Duration {
    let round = REPLY_TIMEOUT * 4 + VERIFY_WAIT;
    match op {
        SonyWrite::Multipoint { .. } => REPLY_TIMEOUT * 4 + ALERT_WAIT + DROP_WAIT,
        SonyWrite::EqBands(_) => round * 2,
        _ => round,
    }
}

/// True when `remaining` of the caller's budget covers [`required_budget`].
pub fn enough_budget(op: &SonyWrite, remaining: Duration) -> bool {
    remaining >= required_budget(op)
}

/// The IPC error code of a failed write.
pub fn error_code(e: &WriteError) -> String {
    match e {
        WriteError::OutOfRange(_) => "outOfRange".to_owned(),
        WriteError::NotSupported => "notSupported".to_owned(),
        WriteError::VerifyFailed => "verifyFailed".to_owned(),
        WriteError::AlertMissing => "alertMissing".to_owned(),
        WriteError::AlertRefused => "alertRefused".to_owned(),
        WriteError::Disconnected => "disconnected".to_owned(),
        WriteError::Timeout => "deviceError:timeout".to_owned(),
        WriteError::Session(s) => format!(
            "deviceError:{}",
            match s {
                SessionError::Transport(_) => "transport",
                SessionError::Policy(_) => "policy",
                SessionError::Send(_) => "send",
                SessionError::Unexpected(_) => "unexpected",
                SessionError::Disconnected | SessionError::LinkDead => "disconnected",
                SessionError::Timeout => "timeout",
            }
        ),
    }
}

/// Why no session could be opened.
enum ConnectError {
    /// This platform has no RFCOMM backend: the worker stops.
    Unsupported,
    /// The headset is not paired or the paired device reports not connected: nothing
    /// was opened. The text is logged once per distinct message.
    NotConnected(String),
    /// A connect was attempted and failed; the text is logged once per distinct message.
    Failed(String),
    /// The module is switched off: nothing was listed or opened.
    Disabled,
}

enum Outcome {
    Stopped,
    Lost(String),
    /// The module was switched off: the session is closed, nothing is published.
    Disabled,
}

pub struct SonyWorker {
    pub app: tauri::AppHandle,
    pub state: Arc<Mutex<ManagerState>>,
    pub commands: Receiver<SonyCommand>,
    pub shutdown: Arc<AtomicBool>,
    /// Up while a write sequence runs; shutdown waits for it.
    pub write_in_flight: Arc<AtomicBool>,
}

/// A failure that means the link is gone (as opposed to a refused request).
fn link_gone(e: &SessionError) -> bool {
    matches!(
        e,
        SessionError::Disconnected | SessionError::LinkDead | SessionError::Transport(_)
    )
}

impl SonyWorker {
    pub fn run(mut self) {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| self.run_loop()));
        if outcome.is_err() {
            log_line!("myhub: the headset worker panicked");
            // Nobody serves the headset any more: show it as disconnected.
            let cached = lock(&self.state).find(sony_wf_c710n::DEVICE_ID);
            if let Some(mut detail) = cached {
                detail.summary.connection = Connection::Disconnected;
                detail.read_only = true;
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
            Slot::Sony,
            Some(detail),
            force,
        );
    }

    fn run_loop(&mut self) {
        // Index into RECONNECT_DELAYS: restarts after a drop and while not connected.
        let mut attempt = 0;
        let mut last_error: Option<String> = None;
        let mut finder = Finder::new();
        while !self.stopping() {
            let mut delay = None;
            match connect_when_enabled(self.module_on(), || finder.find(), open_session) {
                Ok((mut session, mut snapshot)) => {
                    last_error = None;
                    log_line!("myhub: headset connected");
                    let mut stamp = clock_ms();
                    self.publish(self.stamped(&snapshot, true, stamp), true);
                    let outcome = self.serve(&mut session, &mut snapshot, &mut stamp);
                    // Closes the socket.
                    drop(session);
                    match outcome {
                        Outcome::Stopped => return,
                        Outcome::Disabled => {
                            log_line!("myhub: headset module off");
                            attempt = 0;
                            continue;
                        }
                        Outcome::Lost(reason) => {
                            log_line!("myhub: headset link lost: {reason}");
                            self.publish(self.stamped(&snapshot, false, stamp), false);
                            attempt = 0;
                        }
                    }
                }
                Err(ConnectError::Disabled) => {
                    last_error = None;
                    attempt = 0;
                    if lock(&self.state).find(sony_wf_c710n::DEVICE_ID).is_some() {
                        publish_slot(
                            &self.app,
                            &self.state,
                            self.stopping(),
                            Slot::Sony,
                            None,
                            true,
                        );
                    }
                    self.wait_while_off();
                    continue;
                }
                Err(ConnectError::Unsupported) => {
                    log_line!("myhub: headset support is not available on this platform");
                    return;
                }
                Err(ConnectError::NotConnected(message)) => {
                    if last_error.as_deref() != Some(message.as_str()) {
                        log_line!("myhub: headset not connected: {message}");
                        last_error = Some(message);
                    }
                    // Local check only; the first attempt after it comes without delay.
                    attempt = 0;
                    delay = Some(PRESENCE_CHECK_INTERVAL);
                }
                Err(ConnectError::Failed(message)) => {
                    if last_error.as_deref() != Some(message.as_str()) {
                        log_line!("myhub: headset not connected: {message}");
                        last_error = Some(message);
                    }
                }
            }
            let delay = delay.unwrap_or(RECONNECT_DELAYS[attempt]);
            // A refresh command for the headset ends the pause and the back off.
            if self.wait(delay) {
                attempt = 0;
                continue;
            }
            attempt = (attempt + 1).min(RECONNECT_DELAYS.len() - 1);
        }
    }
    /// Polls the session until shutdown or a lost link, publishing every change.
    fn serve(
        &self,
        session: &mut Session<RfcommLink>,
        snapshot: &mut Snapshot,
        stamp: &mut u64,
    ) -> Outcome {
        let mut next_battery = Instant::now() + BATTERY_REFRESH;
        let mut last_write: Option<Instant> = None;
        while !self.stopping() {
            let mut replies = Vec::new();
            let mut writes = Vec::new();
            let mut settings_changed = false;
            while let Ok(command) = self.commands.try_recv() {
                match command {
                    SonyCommand::Refresh { reply } => replies.push(reply),
                    SonyCommand::Write {
                        op,
                        deadline,
                        reply,
                    } => writes.push((op, deadline, reply)),
                    SonyCommand::SettingsChanged => settings_changed = true,
                    SonyCommand::Wake => {}
                }
            }
            // Switched off since the last pass: queued writes are refused, a write that
            // already started has finished (they run one at a time in this thread).
            if !self.module_on() {
                for (_, _, reply) in writes {
                    let _ = reply.send(Err("disconnected".to_owned()));
                }
                answer_all(replies, None);
                return Outcome::Disabled;
            }
            let mut writes = writes.into_iter();
            while let Some((op, deadline, reply)) = writes.next() {
                // Shutdown waits while the flag is up. It is set before the stop check and
                // dropped before the final publish (see keychron.rs).
                let mut in_flight = Some(FlashFlag::set(&self.write_in_flight));
                if self.stopping() {
                    let _ = reply.send(Err("disconnected".to_owned()));
                    continue;
                }
                if let Some(outcome) = self.run_write(
                    session,
                    snapshot,
                    stamp,
                    &mut last_write,
                    op,
                    deadline,
                    reply,
                    &mut in_flight,
                ) {
                    // The link is gone: the writes still queued cannot run.
                    for (_, _, queued) in writes {
                        let _ = queued.send(Err("disconnected".to_owned()));
                    }
                    answer_all(replies, None);
                    return outcome;
                }
            }
            // readOnly depends on allowDeviceWrites.
            let mut dirty = settings_changed;
            let mut items: Vec<Item> = Vec::new();
            if !replies.is_empty() {
                items.extend([Item::Battery, Item::NcAsm, Item::Eq]);
            }
            if let Err(e) = session.poll(POLL_MS) {
                answer_all(replies, None);
                return Outcome::Lost(e.to_string());
            }
            for note in session.take_notifications() {
                match snapshot.apply(&note) {
                    Applied::Updated => dirty = true,
                    Applied::Reread(item) => {
                        if !items.contains(&item) {
                            items.push(item);
                        }
                    }
                    Applied::Ignored => {}
                }
            }
            if Instant::now() >= next_battery {
                next_battery = Instant::now() + BATTERY_REFRESH;
                if !items.contains(&Item::Battery) {
                    items.push(Item::Battery);
                }
            }
            for item in items {
                dirty = true;
                if let Err(e) = reread(session, snapshot, item) {
                    if link_gone(&e) {
                        answer_all(replies, None);
                        return Outcome::Lost(e.to_string());
                    }
                    log_line!("myhub: headset read failed: {e}");
                }
            }
            if dirty {
                *stamp = clock_ms();
                let detail = self.stamped(snapshot, true, *stamp);
                self.publish(detail.clone(), !replies.is_empty());
                answer_all(replies, Some(detail));
            }
        }
        Outcome::Stopped
    }

    fn module_on(&self) -> bool {
        self.app
            .try_state::<SettingsState>()
            .is_none_or(|s| s.modules().sony)
    }

    /// Idle while the module is off: no listing, no socket. Ends when the switch is on
    /// again (a settings change) or on shutdown. Refresh and write get an answer.
    fn wait_while_off(&self) {
        while !self.stopping() {
            match self.commands.recv_timeout(Duration::from_millis(250)) {
                Ok(SonyCommand::Refresh { reply }) => {
                    let _ = reply.send(None);
                }
                Ok(SonyCommand::Write { reply, .. }) => {
                    let _ = reply.send(Err("disconnected".to_owned()));
                }
                Ok(SonyCommand::SettingsChanged) => {
                    if self.module_on() {
                        return;
                    }
                }
                Ok(SonyCommand::Wake) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => thread::sleep(Duration::from_millis(250)),
            }
        }
    }

    fn writes_allowed(&self) -> bool {
        self.app
            .try_state::<SettingsState>()
            .is_some_and(|s| s.get().allow_device_writes)
    }

    /// The detail with the read time stamped on it and `readOnly` from the settings.
    fn stamped(&self, snapshot: &Snapshot, connected: bool, at_ms: u64) -> DeviceDetail {
        let mut detail = to_detail(snapshot, connected);
        detail.updated_at_ms = at_ms;
        detail.read_only = !(self.writes_allowed() && connected);
        detail
    }

    /// Runs one write command: prechecks, keeps [`WRITE_INTERVAL`] since the last
    /// write, applies it through the device crate and answers. `Some` when the link
    /// is gone and the serving loop must end.
    #[allow(clippy::too_many_arguments)]
    fn run_write(
        &self,
        session: &mut Session<RfcommLink>,
        snapshot: &mut Snapshot,
        stamp: &mut u64,
        last_write: &mut Option<Instant>,
        op: SonyWrite,
        deadline: Instant,
        reply: Sender<Result<DeviceDetail, String>>,
        in_flight: &mut Option<FlashFlag>,
    ) -> Option<Outcome> {
        if let Err(code) = precheck(&op, self.writes_allowed(), true) {
            let _ = reply.send(Err(code.to_owned()));
            return None;
        }
        if let Some(last) = *last_write {
            thread::sleep(WRITE_INTERVAL.saturating_sub(last.elapsed()));
        }
        // The caller gave up or will before this write can finish: send nothing.
        if !enough_budget(&op, deadline.saturating_duration_since(Instant::now())) {
            let _ = reply.send(Err("deviceError:timeout".to_owned()));
            return None;
        }
        let result = apply_write(session, snapshot, &op);
        *last_write = Some(Instant::now());
        match result {
            Ok(Done::Applied) => {
                *stamp = clock_ms();
                let detail = self.stamped(snapshot, true, *stamp);
                // The write is over: shutdown must not wait on a worker blocked in the tray.
                in_flight.take();
                self.publish(detail.clone(), true);
                let _ = reply.send(Ok(detail));
                None
            }
            Ok(Done::LinkDropped) => {
                // The final state arrives with the reconnect (device-detail-changed).
                *stamp = clock_ms();
                let detail = self.stamped(snapshot, false, *stamp);
                let _ = reply.send(Ok(detail));
                Some(Outcome::Lost(
                    "the headset dropped the link after a multipoint change".to_owned(),
                ))
            }
            Err(e) => {
                let _ = reply.send(Err(error_code(&e)));
                let gone = match &e {
                    WriteError::Disconnected => true,
                    // The positive answer went out and the link stayed up: reconnect and
                    // read everything again.
                    WriteError::Timeout => matches!(op, SonyWrite::Multipoint { .. }),
                    WriteError::Session(s) => link_gone(s),
                    _ => false,
                };
                if gone {
                    return Some(Outcome::Lost(e.to_string()));
                }
                // The value may differ from what the UI shows: read it again.
                let item = match op {
                    SonyWrite::NcAsm { .. } => Some(Item::NcAsm),
                    SonyWrite::EqPreset(_) | SonyWrite::EqBands(_) => Some(Item::Eq),
                    SonyWrite::Sidetone(_) => Some(Item::Sidetone),
                    SonyWrite::Multipoint { .. } => Some(Item::Multipoint),
                };
                if let Some(item) = item
                    && let Err(read_error) = reread(session, snapshot, item)
                    && link_gone(&read_error)
                {
                    return Some(Outcome::Lost(read_error.to_string()));
                }
                *stamp = clock_ms();
                in_flight.take();
                self.publish(self.stamped(snapshot, true, *stamp), false);
                None
            }
        }
    }

    /// Sleeps up to `duration` in short slices. A refresh command is answered with the
    /// cached detail (there is no session to read from) and ends the sleep: `true`.
    fn wait(&self, duration: Duration) -> bool {
        let deadline = Instant::now() + duration;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || self.stopping() {
                return false;
            }
            let slice = left.min(Duration::from_millis(250));
            match self.commands.recv_timeout(slice) {
                Ok(SonyCommand::Refresh { reply }) => {
                    let cached = lock(&self.state).find(sony_wf_c710n::DEVICE_ID);
                    let _ = reply.send(cached);
                    return true;
                }
                Ok(SonyCommand::Write { op, reply, .. }) => {
                    // No session: writes disabled stays the more useful answer.
                    let code = precheck(&op, self.writes_allowed(), false)
                        .err()
                        .unwrap_or("disconnected");
                    let _ = reply.send(Err(code.to_owned()));
                }
                // Disconnected: readOnly is already true.
                Ok(SonyCommand::SettingsChanged) => {
                    // Switched off: end the pause so the card goes at once.
                    if !self.module_on() {
                        return true;
                    }
                }
                Ok(SonyCommand::Wake) | Err(RecvTimeoutError::Timeout) => {}
                // The manager is gone: nothing to wake us, keep the pace.
                Err(RecvTimeoutError::Disconnected) => thread::sleep(slice),
            }
        }
    }
}

/// [`connect_if_present`] behind the module switch: with the module off `find` is never
/// called, so nothing is listed.
fn connect_when_enabled<T>(
    enabled: bool,
    find: impl FnOnce() -> Result<Option<PairedDevice>, RfcommError>,
    open: impl FnOnce(&PairedDevice) -> Result<T, ConnectError>,
) -> Result<T, ConnectError> {
    if !enabled {
        return Err(ConnectError::Disabled);
    }
    connect_if_present(find, open)
}

/// Looks for the headset and opens a session only when the paired device reports
/// connected; otherwise nothing is sent and [`ConnectError::NotConnected`] comes back.
fn connect_if_present<T>(
    find: impl FnOnce() -> Result<Option<PairedDevice>, RfcommError>,
    open: impl FnOnce(&PairedDevice) -> Result<T, ConnectError>,
) -> Result<T, ConnectError> {
    let device = match find() {
        Ok(found) => found,
        Err(RfcommError::Unsupported) => return Err(ConnectError::Unsupported),
        Err(e) => return Err(ConnectError::Failed(e.to_string())),
    };
    match device {
        None => Err(ConnectError::NotConnected("no paired WF-C710N".to_owned())),
        Some(d) if !d.connected => Err(ConnectError::NotConnected(
            "the paired WF-C710N is not connected".to_owned(),
        )),
        Some(d) => open(&d),
    }
}

/// Opens the RFCOMM link and reads the snapshot.
fn open_session(device: &PairedDevice) -> Result<(Session<RfcommLink>, Snapshot), ConnectError> {
    let mut session = open(device).map_err(|e| match e {
        SessionError::Transport(RfcommError::Unsupported) => ConnectError::Unsupported,
        other => ConnectError::Failed(other.to_string()),
    })?;
    let snapshot = read_snapshot(&mut session).map_err(|e| ConnectError::Failed(e.to_string()))?;
    Ok((session, snapshot))
}

fn answer_all(replies: Vec<Sender<Option<DeviceDetail>>>, detail: Option<DeviceDetail>) {
    for reply in replies {
        let _ = reply.send(detail.clone());
    }
}

/// What a successful write did.
enum Done {
    /// Applied and verified; the snapshot holds the new value.
    Applied,
    /// Multipoint was confirmed and the link dropped.
    LinkDropped,
}

/// Calls the device crate's write path and folds the verified value into the snapshot.
fn apply_write(
    session: &mut Session<RfcommLink>,
    snapshot: &mut Snapshot,
    op: &SonyWrite,
) -> Result<Done, WriteError> {
    match op {
        SonyWrite::NcAsm {
            mode,
            ambient_mode,
            level,
        } => {
            let change = session.set_nc_asm(*mode, *ambient_mode, *level)?;
            if let Some(nc) = snapshot.ncasm.as_mut() {
                nc.param = change.after;
            }
        }
        SonyWrite::EqPreset(id) => {
            let change = session.select_eq_preset(*id)?;
            if let Some(eq) = snapshot.eq.as_mut() {
                eq.param = change.after;
            }
        }
        SonyWrite::EqBands(bands) => {
            let change = session.set_eq_bands(bands)?;
            if let Some(eq) = snapshot.eq.as_mut() {
                eq.param = change.after;
            }
        }
        SonyWrite::Sidetone(on) => {
            snapshot.sidetone = Some(session.set_sidetone(*on)?.after);
        }
        SonyWrite::Multipoint { on, .. } => {
            if let MultipointOutcome::PendingReconnect { .. } = session.set_multipoint(*on)? {
                return Ok(Done::LinkDropped);
            }
        }
    }
    Ok(Done::Applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_manager::WRITE_DEADLINE_MARGIN;
    use myhub_rfcomm::BtAddress;

    fn multipoint(confirm: bool) -> SonyWrite {
        SonyWrite::toggle(ToggleArg::Multipoint, false, confirm)
    }

    #[test]
    fn a_write_is_refused_before_any_frame_unless_enabled_connected_and_confirmed() {
        let nc = SonyWrite::nc_asm(NcModeArg::Ambient, AmbientArg::Voice, 5).unwrap();
        // writesDisabled wins over everything else, disconnected comes next.
        assert_eq!(precheck(&nc, false, false), Err("writesDisabled"));
        assert_eq!(
            precheck(&multipoint(false), false, true),
            Err("writesDisabled")
        );
        assert_eq!(precheck(&nc, true, false), Err("disconnected"));
        assert_eq!(precheck(&nc, true, true), Ok(()));
        // Multipoint drops the link: it needs the explicit confirmation.
        assert_eq!(
            precheck(&multipoint(false), true, true),
            Err("confirmRequired")
        );
        assert_eq!(precheck(&multipoint(true), true, true), Ok(()));
        // Values that do not fit a byte are out of range before the worker sees them.
        assert_eq!(
            SonyWrite::nc_asm(NcModeArg::Ambient, AmbientArg::Normal, 300),
            Err("outOfRange".to_owned())
        );
        assert_eq!(SonyWrite::eq_bands(&[10, -1]), Err("outOfRange".to_owned()));
    }

    #[test]
    fn no_connect_is_attempted_while_the_paired_device_is_not_connected() {
        let mut opened = 0;
        let device = |connected| PairedDevice::new("WF-C710N".into(), BtAddress::new(7), connected);
        for found in [None, Some(device(false))] {
            let r = connect_if_present(
                || Ok(found),
                |_| {
                    opened += 1;
                    Ok(())
                },
            );
            assert!(matches!(r, Err(ConnectError::NotConnected(_))));
        }
        assert_eq!(opened, 0);
    }

    #[test]
    fn a_switched_off_module_lists_and_opens_nothing() {
        let calls = std::cell::Cell::new(0);
        let r = connect_when_enabled(
            false,
            || {
                calls.set(calls.get() + 1);
                Ok(None)
            },
            |_| {
                calls.set(calls.get() + 1);
                Ok(())
            },
        );
        assert!(matches!(r, Err(ConnectError::Disabled)));
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn a_connect_is_attempted_once_the_paired_device_reports_connected() {
        let mut opened = 0;
        let r = connect_if_present(
            || {
                Ok(Some(PairedDevice::new(
                    "WF-C710N".into(),
                    BtAddress::new(7),
                    true,
                )))
            },
            |_| {
                opened += 1;
                Err::<(), _>(ConnectError::Failed("refused".into()))
            },
        );
        assert!(matches!(r, Err(ConnectError::Failed(_))));
        assert_eq!(opened, 1);
    }

    #[test]
    fn a_write_starts_only_with_its_budget_left_and_the_caller_wait_covers_multipoint() {
        let nc = SonyWrite::nc_asm(NcModeArg::Ambient, AmbientArg::Normal, 5).unwrap();
        let bands = SonyWrite::EqBands(vec![10; 6]);
        assert!(required_budget(&multipoint(true)) > required_budget(&nc));
        assert!(required_budget(&bands) > required_budget(&nc));
        assert!(!enough_budget(
            &nc,
            required_budget(&nc) - Duration::from_millis(1)
        ));
        assert!(enough_budget(&nc, required_budget(&nc)));
        // A fresh command has the whole wait, minus one rate-limit pause, for the work.
        let fresh = SONY_WRITE_WAIT - WRITE_DEADLINE_MARGIN - WRITE_INTERVAL;
        assert!(enough_budget(&multipoint(true), fresh));
        assert!(enough_budget(&bands, fresh));
    }
}
