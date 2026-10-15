//! Device manager: one background thread owns hidapi and the HID++ session,
//! keeps the device model up to date and publishes changes to the UI and tray.
//!
//! Read tier by default: discovery, the snapshot reads and the periodic battery
//! status are read requests, everything else is listening (`poll_events`). The
//! only writes are the volatile settings (current DPI, report rate) the user
//! asks for with `set_dpi` / `set_report_rate`, and the optional restore of the
//! stored values on connect. Both are refused unless `allowDeviceWrites` is on,
//! checked here on the worker thread at execution time. HOST mode is never entered
//! implicitly: only `enter_temporary_mode` switches the mouse (0x8100 fn1,
//! verified) and sets the host mark; `set_dpi` / `set_report_rate` need that mark.
//! MyHUB returns the mouse to onboard mode on `leave_temporary_mode`, when writes
//! are turned off and on shutdown (also reached from the tray Quit), only if it
//! did the switch.
//!
//! The G HUB or the Linux kernel driver may use the same receiver at the same
//! time. Their replies carry other software IDs and are dropped by the
//! session, their notifications (software ID 0) that we do not care about are
//! ignored, and our requests use our own software ID.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hidpp::battery::{self, BatteryStatus};
use hidpp::onboard_write::ProfilePatch;
use hidpp::report_rate;
use hidpp::{FeatureId, Message};
use logitech_g502x::{
    DEVICE_ID, DEVICE_INDEX, FailureDetail, ReadError, ReadbackCheck, RestoreStatus, SaveError,
    SectorWriteError, Snapshot, WriteError, onboard_mode, parse_battery_status, read_snapshot,
    save_active_profile, set_dpi, set_onboard_mode, set_report_rate, to_detail,
};
use myhub_core::{CellCharging, Connection, DeviceDetail, DevicePrefs, DeviceSummary, OnboardMode};
use myhub_transport::{
    ErrorFamily, HidApi, HidLink, HidapiLink, HidapiSession, HidppSession, SessionError,
    connection_notification, find_receivers,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::backup;
use crate::settings::{self, Language, Settings, SettingsState};
use crate::sony::{SONY_WRITE_WAIT, SonyCommand, SonyWorker, SonyWrite};
use crate::tray;

pub const DEVICES_CHANGED_EVENT: &str = "devices-changed";
pub const DEVICE_DETAIL_CHANGED_EVENT: &str = "device-detail-changed";

/// Pause between receiver scans while no session is open.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// How long one pump of the link waits for events.
const POLL_TIMEOUT: Duration = Duration::from_millis(500);
/// Pause before the first retry of a failed snapshot read; it doubles per
/// consecutive failure.
const READ_RETRY_BASE: Duration = Duration::from_secs(2);
/// Longest pause between two read attempts.
const READ_RETRY_MAX: Duration = Duration::from_secs(300);
/// Pause before the worker body is restarted after a panic.
const RESTART_DELAY: Duration = Duration::from_secs(5);
/// How often the worker body is restarted after panics before it gives up.
const MAX_RESTARTS: u32 = 3;
/// Interval of the battery status request while the mouse is connected.
const BATTERY_REFRESH: Duration = Duration::from_secs(60);
/// How long `refresh_device` waits for the worker.
pub const REFRESH_WAIT: Duration = Duration::from_secs(5);
/// How long a write command waits for the worker (rate limit and device I/O
/// included).
pub const WRITE_WAIT: Duration = Duration::from_secs(10);
/// How long `save_profile_to_mouse` waits for the worker (reads, a flash write
/// with read-back and a full re-read).
pub const SAVE_WAIT: Duration = Duration::from_secs(30);
/// A save starts its first frame only if this much of the caller's budget is
/// left, so it is not abandoned by the caller half way.
const SAVE_MIN_REMAINING: Duration = Duration::from_secs(15);
/// How long shutdown waits for a flash write in flight.
const FLASH_SHUTDOWN_WAIT: Duration = Duration::from_secs(60);
/// How long before the caller's timeout the worker stops starting writes.
pub(crate) const WRITE_DEADLINE_MARGIN: Duration = Duration::from_secs(2);
/// Reply timeout of the return to onboard mode while the app shuts down.
const SHUTDOWN_REPLY_TIMEOUT: Duration = Duration::from_millis(700);
/// App control is restored at most once per this time: a mouse that keeps
/// resetting right after the restore does not make the app switch it again and
/// again.
const RESTORE_GAP: Duration = Duration::from_secs(20);
/// Least time between two writes to one device.
const WRITE_INTERVAL: Duration = Duration::from_millis(250);
/// How long shutdown waits for the worker before giving up.
/// Worst case: a request in flight (1.5 s) plus the return to onboard mode
/// (rate-limit wait 250 ms, three requests with the 700 ms shutdown timeout each:
/// spy stop, switch, read-back; about 3.85 s in all) plus margin. The spy stop is
/// not retried while the app shuts down.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
/// How long shutdown waits for the headset worker.
const SONY_SHUTDOWN_WAIT: Duration = Duration::from_secs(2);
/// Pause before the one retry of a failed spy stop, and between later attempts
/// from the pass loop.
const SPY_STOP_RETRY: Duration = Duration::from_millis(200);
const SPY_STALE_RETRY: Duration = Duration::from_secs(5);

/// Which worker owns a published detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// The G502 worker ("device-manager").
    Logitech,
    /// The headset worker ("sony-worker").
    Sony,
}

/// What the commands read: one detail per device, each written only by its own
/// worker. The list order is fixed: the G502 first, then the headset.
#[derive(Debug, Default)]
pub struct ManagerState {
    detail: Option<DeviceDetail>,
    sony: Option<DeviceDetail>,
}

impl ManagerState {
    fn slot_mut(&mut self, slot: Slot) -> &mut Option<DeviceDetail> {
        match slot {
            Slot::Logitech => &mut self.detail,
            Slot::Sony => &mut self.sony,
        }
    }

    /// Summaries of every device seen so far, G502 first.
    fn summaries(&self) -> Vec<DeviceSummary> {
        self.detail
            .iter()
            .chain(self.sony.iter())
            .map(|d| d.summary.clone())
            .collect()
    }

    /// The detail with this id.
    pub(crate) fn find(&self, id: &str) -> Option<DeviceDetail> {
        self.detail
            .iter()
            .chain(self.sony.iter())
            .find(|d| d.summary.id == id)
            .cloned()
    }

    /// The details the tray shows: the G502 and the headset.
    fn tray_details(&self) -> (Option<DeviceDetail>, Option<DeviceDetail>) {
        (self.detail.clone(), self.sony.clone())
    }
}

/// Where a command for a device id goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Logitech,
    Sony,
}

fn route_of(id: &str) -> Route {
    if id == sony_wf_c710n::DEVICE_ID {
        Route::Sony
    } else {
        Route::Logitech
    }
}

/// The two worker inboxes. A refresh goes to the worker that owns the id.
struct Router {
    logitech: Sender<Command>,
    sony: Sender<SonyCommand>,
}

impl Router {
    fn refresh(&self, id: &str) -> Option<Receiver<Option<DeviceDetail>>> {
        let (reply, answer) = mpsc::channel();
        let sent = match route_of(id) {
            Route::Sony => self.sony.send(SonyCommand::Refresh { reply }).is_ok(),
            Route::Logitech => self
                .logitech
                .send(Command::Refresh {
                    id: id.to_owned(),
                    reply,
                })
                .is_ok(),
        };
        sent.then_some(answer)
    }
}

enum Command {
    /// Re-read the device with this id and answer with the new detail.
    Refresh {
        id: String,
        reply: Sender<Option<DeviceDetail>>,
    },
    /// Apply a volatile setting. The answer is the new detail or an error code
    /// of the IPC contract (`writesDisabled`, `disconnected`, ...).
    Write {
        id: String,
        op: WriteOp,
        /// After this instant the caller has given up (command timeout): the
        /// worker refuses the write without sending anything.
        deadline: Instant,
        reply: Sender<Result<DeviceDetail, String>>,
    },
    /// Enter (`enter`) or leave the temporary HOST mode; answers with the detail.
    Mode {
        id: String,
        enter: bool,
        deadline: Instant,
        reply: Sender<Result<DeviceDetail, String>>,
    },
    /// Save settings into the active onboard profile (flash write).
    Save {
        id: String,
        request: SaveRequest,
        deadline: Instant,
        reply: Sender<Result<SaveResult, String>>,
    },
    /// The settings changed: publish the detail again (`readOnly` may flip).
    SettingsChanged,
    /// Wakes the worker (shutdown).
    Wake,
}

/// What `save_profile_to_mouse` carries: the fields to change in the active
/// profile, `None` keeps what the mouse has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveRequest {
    /// Exactly five levels.
    pub dpi_levels: Option<Vec<u16>>,
    pub default_dpi_index: Option<u8>,
    pub shift_dpi_index: Option<u8>,
    /// 1000, 500, 250 or 125.
    pub report_rate_hz: Option<u16>,
}

/// The answer of `save_profile_to_mouse`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub detail: DeviceDetail,
    /// `written` or `unchanged`.
    pub outcome: &'static str,
    /// The saved profile was reloaded by switching to another profile and back.
    /// `false` for unchanged, and when no other profile is enabled (the new
    /// values then apply after a profile switch).
    pub reloaded: bool,
    /// The backup taken before the write (also for `unchanged`: the backup comes
    /// before the comparison).
    pub backup_path: Option<String>,
}

/// What a successful save did, before the detail is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SaveOutcome {
    written: bool,
    /// The profile was reloaded (switch away and back); `false` for unchanged.
    reloaded: bool,
    backup_path: Option<PathBuf>,
}

/// A volatile setting the user can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WriteOp {
    /// Current DPI of sensor 0.
    Dpi(u16),
    /// Report rate in Hz.
    ReportRate(u16),
}

/// Handle to the worker thread; managed by Tauri.
pub struct DeviceManager {
    state: Arc<Mutex<ManagerState>>,
    config_dir: Option<PathBuf>,
    commands: Sender<Command>,
    sony_commands: Sender<SonyCommand>,
    shutdown: Arc<AtomicBool>,
    flash_in_flight: Arc<AtomicBool>,
    thread: Mutex<Option<(JoinHandle<()>, Receiver<()>)>>,
    sony_thread: Mutex<Option<(JoinHandle<()>, Receiver<()>)>>,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A reply channel that already holds the error `code`: the headset is read
/// only, so a write command for it is refused without reaching any worker.
fn refused_reply<T>(code: &str) -> Receiver<Result<T, String>> {
    let (reply, answer) = mpsc::channel();
    let _ = reply.send(Err(code.to_owned()));
    answer
}

impl DeviceManager {
    /// Spawns the worker. A failure to spawn is logged and leaves a manager
    /// that reports no devices.
    pub fn start(app: AppHandle) -> Self {
        let state = Arc::new(Mutex::new(ManagerState::default()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let flash_in_flight = Arc::new(AtomicBool::new(false));
        let (commands, command_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let config_dir = app.path().app_config_dir().ok();
        let data_dir = app.path().app_data_dir().ok();
        let (sony_commands, sony_rx) = mpsc::channel();
        let (sony_done_tx, sony_done_rx) = mpsc::channel();
        let sony_worker = SonyWorker {
            app: app.clone(),
            state: Arc::clone(&state),
            commands: sony_rx,
            shutdown: Arc::clone(&shutdown),
        };
        let sony_spawned = thread::Builder::new()
            .name("sony-worker".into())
            .spawn(move || {
                sony_worker.run();
                let _ = sony_done_tx.send(());
            });
        let sony_thread = match sony_spawned {
            Ok(handle) => Some((handle, sony_done_rx)),
            Err(e) => {
                eprintln!("myhub: cannot start the headset worker: {e}");
                None
            }
        };

        let worker = Worker {
            app,
            config_dir: config_dir.clone(),
            data_dir,
            state: Arc::clone(&state),
            commands: command_rx,
            shutdown: Arc::clone(&shutdown),
            flash_in_flight: Arc::clone(&flash_in_flight),
            api: None,
            host_marks: HashMap::new(),
            spy_marks: HashMap::new(),
            spy_stales: HashMap::new(),
        };
        let spawned = thread::Builder::new()
            .name("device-manager".into())
            .spawn(move || {
                worker.run();
                let _ = done_tx.send(());
            });
        let thread = match spawned {
            Ok(handle) => Some((handle, done_rx)),
            Err(e) => {
                eprintln!("myhub: cannot start the device manager: {e}");
                None
            }
        };
        Self {
            state,
            config_dir,
            commands,
            sony_commands,
            shutdown,
            flash_in_flight,
            thread: Mutex::new(thread),
            sony_thread: Mutex::new(sony_thread),
        }
    }

    /// Asks the worker to apply a volatile setting (serialized with all other
    /// commands). The answer arrives on the returned channel; `None` when the
    /// worker is gone.
    pub fn request_write(
        &self,
        id: &str,
        op: WriteOp,
    ) -> Option<Receiver<Result<DeviceDetail, String>>> {
        if route_of(id) == Route::Sony {
            return Some(refused_reply("writesDisabled"));
        }
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(Command::Write {
                id: id.to_owned(),
                op,
                // The caller gives up after WRITE_WAIT. The worker checks this
                // earlier deadline before the first frame, after the rate-limit
                // wait and right before the Set frame; a write whose host
                // switch already went out may still finish after the caller left.
                deadline: Instant::now() + WRITE_WAIT.saturating_sub(WRITE_DEADLINE_MARGIN),
                reply,
            })
            .ok()
            .map(|()| answer)
    }

    /// Asks the worker to enter or leave the temporary HOST mode (serialized with
    /// all other commands). `None` when the worker is gone.
    pub fn request_mode(
        &self,
        id: &str,
        enter: bool,
    ) -> Option<Receiver<Result<DeviceDetail, String>>> {
        if route_of(id) == Route::Sony {
            return Some(refused_reply("writesDisabled"));
        }
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(Command::Mode {
                id: id.to_owned(),
                enter,
                deadline: Instant::now() + WRITE_WAIT.saturating_sub(WRITE_DEADLINE_MARGIN),
                reply,
            })
            .ok()
            .map(|()| answer)
    }

    /// Asks the worker to save into the active onboard profile. The answer
    /// arrives on the returned channel; `None` when the worker is gone.
    pub fn request_save(
        &self,
        id: &str,
        request: SaveRequest,
    ) -> Option<Receiver<Result<SaveResult, String>>> {
        if route_of(id) == Route::Sony {
            return Some(refused_reply("writesDisabled"));
        }
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(Command::Save {
                id: id.to_owned(),
                request,
                // The caller's give-up instant; `save_profile` wants 15 s left.
                deadline: Instant::now() + SAVE_WAIT,
                reply,
            })
            .ok()
            .map(|()| answer)
    }

    /// Asks the headset worker to change a setting. Another device id is refused with
    /// `notSupported`. The answer arrives on the returned channel; `None` when the
    /// worker is gone.
    pub fn request_sony_write(
        &self,
        id: &str,
        op: SonyWrite,
    ) -> Option<Receiver<Result<DeviceDetail, String>>> {
        if route_of(id) != Route::Sony {
            return Some(refused_reply("notSupported"));
        }
        let (reply, answer) = mpsc::channel();
        self.sony_commands
            .send(SonyCommand::Write {
                op,
                deadline: Instant::now() + SONY_WRITE_WAIT.saturating_sub(WRITE_DEADLINE_MARGIN),
                reply,
            })
            .ok()
            .map(|()| answer)
    }

    /// Tells the worker that the settings changed, so it publishes the detail
    /// again with the new `readOnly`.
    pub fn settings_changed(&self) {
        let _ = self.commands.send(Command::SettingsChanged);
        let _ = self.sony_commands.send(SonyCommand::SettingsChanged);
    }

    /// The values the user last applied to this device; `None` when there are
    /// none (or the file is unreadable, which is logged).
    pub fn prefs(&self, id: &str) -> Option<DevicePrefs> {
        let path = settings::prefs_path(self.config_dir.as_deref()?, id)?;
        settings::load_prefs(&path)
    }

    pub fn devices(&self) -> Vec<DeviceSummary> {
        lock(&self.state).summaries()
    }

    pub fn detail(&self, id: &str) -> Option<DeviceDetail> {
        lock(&self.state).find(id)
    }

    /// Asks the worker to re-read the device. The answer arrives on the
    /// returned channel; `None` when the worker is gone.
    pub fn request_refresh(&self, id: &str) -> Option<Receiver<Option<DeviceDetail>>> {
        Router {
            logitech: self.commands.clone(),
            sony: self.sony_commands.clone(),
        }
        .refresh(id)
    }

    /// Re-applies the tray status text, e.g. after a language change.
    pub fn refresh_tray(&self, app: &AppHandle) {
        let (mouse, headset) = lock(&self.state).tray_details();
        update_tray(app, mouse.as_ref(), headset.as_ref());
    }

    /// Stops the worker and waits a short while for it to finish. The wait is
    /// bounded in case the worker is stuck in a device read.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // A flash write already running is not interrupted: wait for it.
        if !wait_for_flash(&self.flash_in_flight, FLASH_SHUTDOWN_WAIT) {
            eprintln!("myhub: a flash write is still running after {FLASH_SHUTDOWN_WAIT:?}");
        }
        let _ = self.commands.send(Command::Wake);
        let _ = self.sony_commands.send(SonyCommand::Wake);
        // The headset worker sees the flag within one poll (200 ms); a connect
        // in progress is not interrupted, so the wait is bounded.
        if let Some((handle, done)) = lock(&self.sony_thread).take() {
            match done.recv_timeout(SONY_SHUTDOWN_WAIT) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                    let _ = handle.join();
                }
                Err(RecvTimeoutError::Timeout) => {
                    eprintln!("myhub: headset worker did not stop in time");
                }
            }
        }
        if let Some((handle, done)) = lock(&self.thread).take() {
            match done.recv_timeout(SHUTDOWN_WAIT) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                    let _ = handle.join();
                }
                Err(RecvTimeoutError::Timeout) => {
                    eprintln!("myhub: device manager did not stop in time");
                }
            }
        }
    }
}

/// Short name for the tray: the device name without the transport suffix.
fn short_name(name: &str) -> &str {
    name.strip_suffix(" LIGHTSPEED").unwrap_or(name)
}

/// Tray status line for the device: `G502 X: 55%`, `G502 X: uśpiona` /
/// `G502 X: asleep` while disconnected, `None` without a device.
pub fn status_text(detail: Option<&DeviceDetail>, language: Language) -> Option<String> {
    let detail = detail?;
    let name = short_name(&detail.summary.name);
    Some(
        match (detail.summary.connection, detail.summary.battery_percent) {
            (Connection::Disconnected, _) => match language {
                Language::Pl => format!("{name}: uśpiona"),
                _ => format!("{name}: asleep"),
            },
            (Connection::Connected, Some(percent)) => format!("{name}: {percent}%"),
            (Connection::Connected, None) => name.to_owned(),
        },
    )
}

/// Tray status line for the headset: `WF-C710N: L 100% · P 100% · etui 70%` (English:
/// `L · R · case`), a cell that charges gets ` (ładowanie)` / ` (charging)`, a cell not
/// reported is left out; `WF-C710N: rozłączone` / `WF-C710N: disconnected` while
/// disconnected; `None` without a headset.
pub fn headset_status_text(detail: Option<&DeviceDetail>, language: Language) -> Option<String> {
    let detail = detail?;
    let name = short_name(&detail.summary.name);
    if detail.summary.connection == Connection::Disconnected {
        return Some(match language {
            Language::Pl => format!("{name}: rozłączone"),
            _ => format!("{name}: disconnected"),
        });
    }
    let Some(battery) = detail.headset.as_ref().and_then(|h| h.battery) else {
        return Some(name.to_owned());
    };
    let (right, case, charging) = match language {
        Language::Pl => ("P", "etui", " (ładowanie)"),
        _ => ("R", "case", " (charging)"),
    };
    let parts: Vec<String> = [
        ("L", battery.left),
        (right, battery.right),
        (case, battery.case),
    ]
    .into_iter()
    .filter_map(|(label, cell)| {
        let cell = cell?;
        let suffix = if cell.charging == CellCharging::Charging {
            charging
        } else {
            ""
        };
        Some(format!("{label} {}%{suffix}", cell.percent))
    })
    .collect();
    Some(if parts.is_empty() {
        name.to_owned()
    } else {
        format!("{name}: {}", parts.join(" · "))
    })
}

/// The two tray status lines. With a mouse: the mouse line, then the headset line when there is
/// one. Without a mouse the headset line is the only (first) line.
pub fn tray_lines(
    mouse: Option<&DeviceDetail>,
    headset: Option<&DeviceDetail>,
    language: Language,
) -> (Option<String>, Option<String>) {
    let headset_line = headset_status_text(headset, language);
    match status_text(mouse, language) {
        Some(mouse_line) => (Some(mouse_line), headset_line),
        None => (headset_line, None),
    }
}

fn update_tray(app: &AppHandle, mouse: Option<&DeviceDetail>, headset: Option<&DeviceDetail>) {
    let (first, second) = tray_lines(mouse, headset, tray::current_language(app));
    tray::set_devices_status(app, first, second);
    // The icon follows the mouse while it is known, else the headset.
    let detail = mouse.or(headset);
    tray::set_look(
        app,
        tray::look_for(detail.map(|d| (d.summary.connection, d.summary.battery_percent))),
    );
}

/// What a change of the published detail means for the events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Changes {
    /// The summary list differs: emit `devices-changed`.
    summaries: bool,
    /// The detail differs (ignoring the timestamp): emit
    /// `device-detail-changed`.
    detail: bool,
}

fn changes(old: Option<&DeviceDetail>, new: Option<&DeviceDetail>) -> Changes {
    match (old, new) {
        (None, None) => Changes {
            summaries: false,
            detail: false,
        },
        (Some(old), Some(new)) => Changes {
            summaries: old.summary != new.summary,
            detail: !old.same_content(new),
        },
        _ => Changes {
            summaries: true,
            detail: true,
        },
    }
}

/// Pause before the next read attempt after `failures` consecutive failures
/// (1 = the first): 2 s, 4 s, 8 s, ... capped at 5 min.
fn retry_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    READ_RETRY_BASE
        .saturating_mul(1 << doublings)
        .min(READ_RETRY_MAX)
}

pub(crate) fn clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The link to the receiver failed (unplugged, I/O error). The session must be
/// dropped.
#[derive(Debug)]
struct LinkLost(String);

/// Why a write did not happen.
#[derive(Debug)]
enum WriteFailure {
    /// Refused or failed; the text is an error code of the IPC contract.
    Refused(String),
    /// The receiver link failed: the session must be dropped.
    Lost(LinkLost),
}

/// Maps a driver error to the contract error codes. `deviceError:<code>` carries
/// the HID++ error code in decimal; other device-side failures carry a word.
fn classify_write_error(e: WriteError) -> WriteFailure {
    let code = match e {
        WriteError::NotHostMode => "notHostMode".to_owned(),
        WriteError::DpiOutOfRange(_) | WriteError::RateOutOfRange(_) => "outOfRange".to_owned(),
        WriteError::VerifyFailed { .. } => "verifyFailed".to_owned(),
        WriteError::Encode(_) => "deviceError:encode".to_owned(),
        WriteError::Parse(_) => "deviceError:parse".to_owned(),
        WriteError::Session(SessionError::Link(link)) => {
            return WriteFailure::Lost(LinkLost(link.to_string()));
        }
        // HID++ 1.0 error 0x08 is "unknown device": the mouse is off.
        WriteError::Session(SessionError::Device {
            family: ErrorFamily::Hidpp10,
            code: 0x08,
            ..
        })
        | WriteError::Session(
            SessionError::Timeout | SessionError::LinkDown | SessionError::NeedsRediscovery,
        ) => "disconnected".to_owned(),
        WriteError::Session(SessionError::Device { code, .. }) => format!("deviceError:{code}"),
        WriteError::Session(SessionError::Policy(_)) => "deviceError:policy".to_owned(),
        WriteError::Session(_) => "deviceError:session".to_owned(),
    };
    WriteFailure::Refused(code)
}

/// `true` when the failure detail says the receiver link failed during the
/// read-back or the restore (the reasons are `SessionError` texts, and a link
/// error always starts with "link error").
fn link_failed(detail: &FailureDetail) -> bool {
    let link = |text: &str| text.starts_with("link error");
    match (&detail.restore, &detail.readback) {
        (RestoreStatus::Failed(why, _), _) => link(why),
        (_, ReadbackCheck::Unreadable(why)) => link(why),
        _ => false,
    }
}

fn restore_tag(status: &RestoreStatus) -> &'static str {
    match status {
        // No restore was possible (the link was lost mid-sequence): unknown.
        RestoreStatus::NotAttempted => "unknown",
        RestoreStatus::NotNeeded => "notNeeded",
        RestoreStatus::Restored(_) => "restored",
        RestoreStatus::Failed(..) => "restoreFailed",
    }
}

/// Maps a save error to the contract codes. A lost link stays a lost link.
fn classify_save_error(e: SaveError) -> WriteFailure {
    let refused = |code: &str| WriteFailure::Refused(code.to_owned());
    match e {
        SaveError::NotOnboardMode => refused("notOnboardMode"),
        SaveError::ProfileNotWritable { .. } => refused("profileNotWritable"),
        SaveError::Session(se) => classify_write_error(WriteError::Session(se)),
        // The data is on the flash; the mouse may still run the old copy until
        // the profile is selected again or the mouse restarts.
        // The suffix says which profile the mouse is on now.
        SaveError::Reselect {
            cause, active_now, ..
        } => {
            eprintln!(
                "myhub: the profile was written but reloading it failed: {cause} (active now: {active_now:?})"
            );
            match active_now {
                Some(sector) => refused(&format!("savedReselectFailed:{sector:#06x}")),
                None => refused("savedReselectFailed:unknown"),
            }
        }
        SaveError::Write(w) => match w {
            SectorWriteError::Backup(_) => refused("backupFailed"),
            // Range, index and level-zero refusals.
            SectorWriteError::Patch(_) => refused("outOfRange"),
            // A receiver link error mid-sequence (in the write, the read-back or
            // the restore): nothing reliable could be done, the sector state is
            // unknown (the dead link shows up at the next poll).
            SectorWriteError::WriteFailed { cause, detail } => {
                if matches!(cause, SessionError::Link(_)) || link_failed(&detail) {
                    refused("flashWriteFailed:unknown")
                } else {
                    refused(&format!(
                        "flashWriteFailed:{}",
                        restore_tag(&detail.restore)
                    ))
                }
            }
            SectorWriteError::VerifyFailed { detail, .. } => {
                if link_failed(&detail) {
                    refused("flashWriteFailed:unknown")
                } else {
                    refused(&format!(
                        "flashWriteFailed:{}",
                        restore_tag(&detail.restore)
                    ))
                }
            }
            SectorWriteError::SelectVerifyFailed { .. } => refused("savedReselectFailed:unknown"),
            SectorWriteError::SectorNotAllowed(_)
            | SectorWriteError::UnsupportedFormat(_)
            | SectorWriteError::SectorSize(_)
            | SectorWriteError::ReadCrcInvalid
            | SectorWriteError::BadSource(_)
            | SectorWriteError::ProfileNotEnabled(_)
            | SectorWriteError::TargetProfileInvalid(_) => refused("profileNotWritable"),
            SectorWriteError::ImageInvalid(_) => refused("deviceError:encode"),
            SectorWriteError::Parse(_) => refused("deviceError:parse"),
            SectorWriteError::Session(se) => classify_write_error(WriteError::Session(se)),
        },
    }
}

/// `true` when the failure happened before any frame that changes the flash
/// could have been sent.
fn nothing_written(e: &SaveError) -> bool {
    matches!(
        e,
        SaveError::NotOnboardMode
            | SaveError::ProfileNotWritable { .. }
            | SaveError::Write(
                SectorWriteError::Backup(_)
                    | SectorWriteError::Patch(_)
                    | SectorWriteError::SectorNotAllowed(_)
                    | SectorWriteError::UnsupportedFormat(_)
                    | SectorWriteError::SectorSize(_)
                    | SectorWriteError::ReadCrcInvalid
                    | SectorWriteError::BadSource(_)
                    | SectorWriteError::ProfileNotEnabled(_)
                    | SectorWriteError::TargetProfileInvalid(_)
                    | SectorWriteError::ImageInvalid(_)
            )
    )
}

impl SaveRequest {
    /// The patch for the driver. A request the mouse could never take is
    /// `outOfRange` before anything is sent.
    fn to_patch(&self) -> Result<ProfilePatch, WriteFailure> {
        let out_of_range = || WriteFailure::Refused("outOfRange".into());
        let dpi_levels = match &self.dpi_levels {
            None => None,
            Some(levels) => {
                Some(<[u16; 5]>::try_from(levels.as_slice()).map_err(|_| out_of_range())?)
            }
        };
        let report_rate_interval_ms = match self.report_rate_hz {
            None => None,
            Some(hz) => Some(
                [1u8, 2, 4, 8]
                    .into_iter()
                    .find(|ms| report_rate::interval_to_hz(*ms) == hz)
                    .ok_or_else(out_of_range)?,
            ),
        };
        Ok(ProfilePatch {
            report_rate_interval_ms,
            default_dpi_index: self.default_dpi_index,
            shift_dpi_index: self.shift_dpi_index,
            dpi_levels,
        })
    }
}

/// Runs one user write command: refuses it without sending anything once the
/// caller's deadline passed, clears the restore guard (a user write starts a
/// new restore epoch), then writes through the device.
fn run_user_write<L: HidLink>(
    device: &mut Device<L>,
    id: &str,
    op: WriteOp,
    deadline: Instant,
    allow_writes: bool,
) -> Result<(), WriteFailure> {
    if Instant::now() >= deadline {
        return Err(WriteFailure::Refused("deviceError:timeout".into()));
    }
    device.write_until(id, op, allow_writes, Some(deadline))
}

/// After a full read: when writes are off, MyHUB switched the mouse to host
/// and the read shows host mode, hand the mode back (one attempt). This catches
/// a mouse that was unreachable when the setting was turned off.
fn return_if_writes_off<L: HidLink>(
    device: &mut Device<L>,
    settings: &Settings,
) -> Result<(), LinkLost> {
    if !device.take_fresh_read() || settings.allow_device_writes || !device.host_by_us() {
        return Ok(());
    }
    if device.snapshot.as_ref().and_then(snapshot_mode_byte) == Some(2) {
        device.return_to_onboard()?;
    }
    Ok(())
}

/// Restore of the last mode after a connect read (the first one after app start
/// and every link-up): runs only when a connect read is due, `allowDeviceWrites`
/// and `restoreLastMode` are on and the stored mode is host. `load_prefs` gets
/// the device id. One attempt per connect event; returns a log line per step.
fn restore_last_mode_if_enabled<L: HidLink>(
    device: &mut Device<L>,
    settings: &Settings,
    load_prefs: impl FnOnce(&str) -> Option<DevicePrefs>,
    now: Instant,
) -> Result<Vec<String>, LinkLost> {
    if !device.take_restore_due() {
        return Ok(Vec::new());
    }
    if !(settings.allow_device_writes && settings.restore_last_mode) {
        return Ok(Vec::new());
    }
    let Some(id) = device.detail().map(|d| d.summary.id) else {
        return Ok(Vec::new());
    };
    let Some(prefs) = load_prefs(&id) else {
        return Ok(Vec::new());
    };
    if prefs.mode != OnboardMode::Host {
        return Ok(Vec::new());
    }
    device.restore_app_control(&id, &prefs, settings.allow_device_writes, now)
}

/// Enter app control; the stored mode becomes host only when it worked.
fn enter_and_store<L: HidLink>(
    device: &mut Device<L>,
    prefs_path: Option<&Path>,
    id: &str,
    allow_writes: bool,
    deadline: Instant,
) -> Result<(), WriteFailure> {
    device.enter_temporary(id, allow_writes, Some(deadline))?;
    if let Some(path) = prefs_path {
        let profile = device.remembered_profile;
        update_prefs(path, |prefs| {
            prefs.mode = OnboardMode::Host;
            if profile.is_some() {
                prefs.profile = profile;
            }
        });
    }
    Ok(())
}

/// Leave app control. The user asked for onboard, so the stored mode becomes
/// onboard before any frame goes out, whatever the outcome.
fn leave_and_store<L: HidLink>(
    device: &mut Device<L>,
    prefs_path: Option<&Path>,
    id: &str,
    allow_writes: bool,
    deadline: Instant,
) -> Result<(), WriteFailure> {
    if let Some(path) = prefs_path {
        update_prefs(path, |prefs| prefs.mode = OnboardMode::Onboard);
    }
    device.leave_temporary(id, allow_writes, Some(deadline))
}

/// After a save: if it returned the mouse from app control to onboard (to be
/// able to save), the stored mode becomes onboard, whatever the save's outcome.
fn record_save_mode<L: HidLink>(device: &mut Device<L>, prefs_path: Option<&Path>) {
    if std::mem::take(&mut device.returned_from_host)
        && let Some(path) = prefs_path
    {
        update_prefs(path, |prefs| prefs.mode = OnboardMode::Onboard);
    }
}

/// Loads the prefs file, changes it and saves it atomically. A failure to save
/// is logged: the change is applied on the device either way.
fn update_prefs(path: &Path, change: impl FnOnce(&mut DevicePrefs)) {
    let mut prefs = settings::load_prefs(path).unwrap_or_default();
    change(&mut prefs);
    if let Err(e) = settings::save_prefs(path, &prefs) {
        eprintln!("myhub: cannot save device prefs {}: {e}", path.display());
    }
}

/// Holds the flash-in-flight flag up; clears it when dropped (also on a panic).
struct FlashFlag(Arc<AtomicBool>);

impl FlashFlag {
    fn set(flag: &Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self(Arc::clone(flag))
    }
}

impl Drop for FlashFlag {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Waits until `flag` is clear or `limit` has passed; `true` when it is clear.
fn wait_for_flash(flag: &AtomicBool, limit: Duration) -> bool {
    let end = Instant::now() + limit;
    while flag.load(Ordering::SeqCst) {
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
    true
}

fn snapshot_mode_byte_of<L: HidLink>(device: &Device<L>) -> Option<u8> {
    device.snapshot.as_ref().and_then(snapshot_mode_byte)
}

/// First byte of the stored IsOnboardMode reply (1 onboard, 2 host).
fn snapshot_mode_byte(snapshot: &Snapshot) -> Option<u8> {
    let hex = snapshot.onboard.mode_raw_hex.as_deref()?;
    u8::from_str_radix(hex.get(0..2)?, 16).ok()
}

/// One mouse behind an open receiver session: the last snapshot and the
/// bookkeeping to keep it current. Generic over the link so tests can use a
/// mock.
struct Device<L: HidLink> {
    session: HidppSession<L>,
    snapshot: Option<Snapshot>,
    /// What is published: `false` after a link-down notification.
    link_up: bool,
    updated_at_ms: u64,
    next_read: Instant,
    last_battery: Instant,
    poll_timeout: Duration,
    dirty: bool,
    /// A full read is owed: set at start, on a link-up 0x41 and whenever the
    /// feature table was invalidated; cleared only by a successful read, so a
    /// failed snapshot read after a successful discovery is retried.
    read_pending: bool,
    /// Consecutive failed reads, for the backoff.
    read_failures: u32,
    /// When the last write to the device went out (rate limit).
    last_write: Option<Instant>,
    /// A connect read succeeded and the stored prefs may be restored once.
    restore_due: bool,
    /// The next successful read only re-syncs the detail after a write that
    /// timed out; it is not a connect event.
    resync: bool,
    /// MyHUB switched the mouse to host mode and has not switched it back.
    /// Only then does MyHUB return it to onboard mode. Shared with the worker,
    /// which keeps it when this device is dropped (link loss, restart).
    host_mark: Arc<AtomicBool>,
    /// The 0x8110 button spy is running (set after a verified start). Shared
    /// with the worker like the host mark, so it survives this device.
    spy_mark: Arc<AtomicBool>,
    /// A spy stop failed: the spy may still be running. A stop is sent again on
    /// the next reachable pass and on leave; cleared only by a successful stop.
    /// Shared with the worker like the marks.
    spy_stale: Arc<AtomicBool>,
    /// Earliest time of the next stop attempt from the pass loop.
    spy_stale_retry_at: Instant,
    /// The spy must be started again after the read that follows a link-up with
    /// the host mark set.
    spy_restart_due: bool,
    /// Previous spy button mask (edge detection).
    spy_mask: u16,
    /// The profile sector that was active when temporary mode was entered; its
    /// levels are what the DPI buttons walk through.
    remembered_profile: Option<u16>,
    /// The DPI the buttons asked for; the latest press wins.
    dpi_target: Option<u16>,
    /// A press with no known profile levels was logged already.
    spy_unknown_logged: bool,
    /// The app is shutting down (set by the worker): no battery refresh, short
    /// reply timeout for the return to onboard mode.
    shutdown: Arc<AtomicBool>,
    /// A full read succeeded since the last `take_fresh_read`.
    fresh_read: bool,
    /// The last save returned the mouse from app control to onboard mode.
    returned_from_host: bool,
    /// A flash write is running (shared with the manager so shutdown waits).
    flash_in_flight: Arc<AtomicBool>,
    /// A 0x8100 event could not be applied from what is known: confirm with a
    /// cheap read (0x2201 fn2, 0x8100 fn4) instead of a full snapshot.
    confirm_pending: bool,
    /// An unknown 0x8100 event was logged already.
    unknown_event_logged: bool,
    /// When app control was last restored (loop breaker, see [`RESTORE_GAP`]).
    last_restore: Option<Instant>,
}

impl<L: HidLink> Device<L> {
    fn new(mut session: HidppSession<L>, now: Instant) -> Self {
        session.set_device_index(DEVICE_INDEX);
        Self {
            session,
            snapshot: None,
            link_up: false,
            updated_at_ms: 0,
            next_read: now,
            last_battery: now,
            poll_timeout: POLL_TIMEOUT,
            dirty: false,
            read_pending: true,
            read_failures: 0,
            last_write: None,
            restore_due: false,
            resync: false,
            host_mark: Arc::new(AtomicBool::new(false)),
            spy_mark: Arc::new(AtomicBool::new(false)),
            spy_stale: Arc::new(AtomicBool::new(false)),
            spy_stale_retry_at: now,
            spy_restart_due: false,
            spy_mask: 0,
            remembered_profile: None,
            dpi_target: None,
            spy_unknown_logged: false,
            shutdown: Arc::new(AtomicBool::new(false)),
            fresh_read: false,
            flash_in_flight: Arc::new(AtomicBool::new(false)),
            returned_from_host: false,
            confirm_pending: false,
            unknown_event_logged: false,
            last_restore: None,
        }
    }

    /// Uses the worker's host-mode mark, so the knowledge survives this device.
    fn with_host_mark(mut self, mark: Arc<AtomicBool>) -> Self {
        self.host_mark = mark;
        self
    }

    /// Uses the worker's spy mark.
    fn with_spy_mark(mut self, mark: Arc<AtomicBool>) -> Self {
        self.spy_mark = mark;
        self
    }

    /// Uses the worker's spy-may-be-running flag.
    fn with_spy_stale(mut self, flag: Arc<AtomicBool>) -> Self {
        self.spy_stale = flag;
        self
    }

    fn spy_stale(&self) -> bool {
        self.spy_stale.load(Ordering::SeqCst)
    }

    fn spy_active(&self) -> bool {
        self.spy_mark.load(Ordering::SeqCst)
    }

    fn set_spy_active(&self, value: bool) {
        self.spy_mark.store(value, Ordering::SeqCst);
    }

    /// Starts the button spy (0x8110 fn1), so the DPI buttons reach the app in
    /// host mode. A failure is logged and the mouse stays in host mode: the DPI
    /// buttons then do not work, nothing else is affected. Only a lost receiver
    /// link is an error.
    fn start_spy(&mut self) -> Result<(), LinkLost> {
        self.tighten_if_stopping();
        self.spy_mask = 0;
        match self.session.request_button_spy(true) {
            Ok(_) => {
                self.set_spy_active(true);
                eprintln!("myhub: button spy started (DPI buttons active)");
                Ok(())
            }
            Err(SessionError::Link(e)) => Err(LinkLost(e.to_string())),
            Err(e) => {
                self.set_spy_active(false);
                eprintln!(
                    "myhub: could not start the button spy, the DPI buttons will not work: {e}"
                );
                Ok(())
            }
        }
    }

    /// Stops the button spy (0x8110 fn2). Always sent, also when the spy is not
    /// known to run: a second stop is harmless and a spy left running after a
    /// crash would keep sending events. A failure other than a lost link is
    /// retried once after 200 ms (not while shutting down). If it still fails the
    /// spy-may-be-running flag stays up, so the next reachable pass and a leave
    /// send the stop again. The reply bytes are logged on every successful stop
    /// ([UNK] what a stop without a running spy answers).
    fn stop_spy(&mut self) -> Result<(), LinkLost> {
        self.tighten_if_stopping();
        self.set_spy_active(false);
        self.spy_mask = 0;
        self.dpi_target = None;
        let mut result = self.session.request_button_spy(false);
        if matches!(&result, Err(e) if !matches!(e, SessionError::Link(_))) && !self.stopping() {
            thread::sleep(SPY_STOP_RETRY);
            result = self.session.request_button_spy(false);
        }
        match result {
            Ok(reply) => {
                self.spy_stale.store(false, Ordering::SeqCst);
                let hex: String = reply.iter().map(|b| format!("{b:02x}")).collect();
                eprintln!("myhub: button spy stopped (0x8110 fn2 reply {hex})");
                Ok(())
            }
            Err(e) => {
                self.spy_stale.store(true, Ordering::SeqCst);
                self.spy_stale_retry_at = Instant::now() + SPY_STALE_RETRY;
                if let SessionError::Link(link) = e {
                    return Err(LinkLost(link.to_string()));
                }
                eprintln!("myhub: could not stop the button spy (will retry): {e}");
                Ok(())
            }
        }
    }

    /// A 0x8110 notification: bytes 0..1 LE u16, one bit per held button
    /// \[OBS\]. Bit 1 is DPI up, bit 2 DPI down; only press edges count, other
    /// bits are ignored. The target level is picked from the remembered profile
    /// (else the snapshot's active user profile), clamped at the ends, and
    /// queued; the latest target wins.
    fn apply_spy_event(&mut self, function: u8, payload: &[u8; 16]) {
        if function != 0 || !self.spy_active() {
            return;
        }
        let mask = u16::from_le_bytes([payload[0], payload[1]]);
        let pressed = mask & !self.spy_mask;
        self.spy_mask = mask;
        let up = pressed & 0x0002 != 0;
        let down = pressed & 0x0004 != 0;
        if !(up || down) {
            return;
        }
        let sector = self
            .remembered_profile
            .or_else(|| self.active_sector().filter(|s| (1..=5).contains(s)));
        let levels = sector.and_then(|s| self.profile_of(s)).map(|p| {
            let mut levels: Vec<u16> = p.dpi_levels.iter().copied().filter(|d| *d != 0).collect();
            levels.sort_unstable();
            levels.dedup();
            levels
        });
        let Some(levels) = levels.filter(|l| !l.is_empty()) else {
            if !self.spy_unknown_logged {
                self.spy_unknown_logged = true;
                eprintln!(
                    "myhub: DPI button pressed but no profile levels are known (ignored, logged once)"
                );
            }
            return;
        };
        let Some(current) = self.snapshot.as_ref().map(|s| s.dpi.current) else {
            return;
        };
        // Two quick presses build on each other: the base is the pending target.
        let base = self.dpi_target.unwrap_or(current);
        let target = if up {
            levels
                .iter()
                .copied()
                .find(|l| *l > base)
                .or(levels.last().copied())
        } else {
            levels
                .iter()
                .copied()
                .rev()
                .find(|l| *l < base)
                .or(levels.first().copied())
        };
        if let Some(target) = target {
            self.dpi_target = Some(target).filter(|t| *t != current || self.dpi_target.is_some());
        }
    }

    /// Applies the queued button target through the normal `set_dpi` path (rate
    /// limit, verify, snapshot update). One attempt; the latest target was
    /// already chosen when it was queued. Only a lost link is an error.
    fn apply_dpi_target(&mut self, allow_writes: bool) -> Result<(), LinkLost> {
        let Some(target) = self.dpi_target.take() else {
            return Ok(());
        };
        let Some(id) = self.detail().map(|d| d.summary.id) else {
            return Ok(());
        };
        match self.write(&id, WriteOp::Dpi(target), allow_writes) {
            Ok(()) => Ok(()),
            Err(WriteFailure::Lost(lost)) => Err(lost),
            Err(WriteFailure::Refused(code)) => {
                eprintln!("myhub: DPI button target {target} not applied: {code}");
                Ok(())
            }
        }
    }

    /// Uses the manager's flash-in-flight flag.
    fn with_flash_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.flash_in_flight = flag;
        self
    }

    /// Uses the worker's shutdown flag.
    fn with_shutdown(mut self, shutdown: Arc<AtomicBool>) -> Self {
        self.shutdown = shutdown;
        self
    }

    fn host_by_us(&self) -> bool {
        self.host_mark.load(Ordering::SeqCst)
    }

    /// Sets the host mark. Clearing it also forgets the remembered profile.
    fn set_host_by_us(&mut self, value: bool) {
        self.host_mark.store(value, Ordering::SeqCst);
        if !value {
            self.remembered_profile = None;
        }
    }

    fn stopping(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// While the app shuts down every reply wait is short, so the return to
    /// onboard mode fits the shutdown budget. Called before each group of
    /// requests; a request already in flight (or a snapshot read in progress)
    /// keeps the timeout it started with.
    fn tighten_if_stopping(&mut self) {
        if self.stopping() {
            self.session.set_reply_timeout(SHUTDOWN_REPLY_TIMEOUT);
        }
    }

    /// `true` once per successful full read.
    fn take_fresh_read(&mut self) -> bool {
        std::mem::take(&mut self.fresh_read)
    }

    /// The model to publish, `None` until the first successful read.
    fn detail(&self) -> Option<DeviceDetail> {
        self.snapshot
            .as_ref()
            .map(|s| to_detail(s, self.link_up, self.updated_at_ms))
    }

    /// `true` once since the last call when the detail may have changed.
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    fn link_known_down(&self) -> bool {
        self.session.link_state() == Some(false)
    }

    /// One full read (discover + snapshot). `Ok(false)` after a non-link
    /// failure, which is logged and retried later.
    fn read_now(&mut self, now: Instant) -> Result<bool, LinkLost> {
        self.tighten_if_stopping();
        match read_snapshot(&mut self.session) {
            Ok(snapshot) => {
                // A full read that was owed is a connect or wake event: the
                // stored prefs may be restored once.
                let connect = self.read_pending && !self.resync;
                self.restore_due |= connect;
                self.resync = false;
                self.fresh_read = true;
                if self.host_by_us() && snapshot_mode_byte(&snapshot) == Some(1) {
                    // Observation: the mouse is onboard again after a
                    // reconnect, so the host mode set by MyHUB is volatile.
                    eprintln!(
                        "myhub: observation: the mouse is back in onboard mode after a reconnect; the host mode MyHUB set did not survive"
                    );
                    self.set_host_by_us(false);
                    // The spy went with the host mode.
                    self.set_spy_active(false);
                    self.spy_mask = 0;
                    self.dpi_target = None;
                }
                // A link-up while temporary mode is active: the spy is started
                // again once the snapshot is stored.
                self.spy_restart_due |= connect && self.host_by_us();
                self.snapshot = Some(snapshot);
                self.apply_host_overlay();
                self.link_up = true;
                self.updated_at_ms = clock_ms();
                self.last_battery = now;
                self.next_read = now;
                self.dirty = true;
                self.read_pending = false;
                self.read_failures = 0;
                Ok(true)
            }
            Err(ReadError::Session(SessionError::Link(e))) => Err(LinkLost(e.to_string())),
            Err(e) => {
                // 0x08 (unknown device) is expected while the mouse is off.
                eprintln!("myhub: device read failed: {e}");
                self.read_failures = self.read_failures.saturating_add(1);
                self.next_read = now + retry_delay(self.read_failures);
                Ok(false)
            }
        }
    }

    fn wants_read(&self, now: Instant) -> bool {
        !self.link_known_down() && self.read_pending && now >= self.next_read
    }

    /// One pass of the event loop: read when needed, listen, apply
    /// notifications and link changes, refresh the battery when due.
    fn step(&mut self, now: Instant) -> Result<(), LinkLost> {
        self.tighten_if_stopping();
        if self.wants_read(now) {
            self.read_now(now)?;
            if std::mem::take(&mut self.spy_restart_due) && self.host_by_us() {
                self.set_spy_active(false);
                self.start_spy()?;
            }
        }
        self.session
            .poll_events(self.poll_timeout)
            .map_err(|e| LinkLost(e.to_string()))?;
        if self.session.needs_rediscovery() {
            self.read_pending = true;
        }
        self.apply_notifications(now);
        self.confirm_read()?;
        // A stop that failed earlier: try again on a reachable pass.
        if self.spy_stale()
            && !self.stopping()
            && now >= self.spy_stale_retry_at
            && self.link_up
            && !self.link_known_down()
            && !self.session.needs_rediscovery()
        {
            self.stop_spy()?;
        }
        if self.link_known_down() && self.link_up {
            self.link_up = false;
            self.dirty = true;
        }
        self.refresh_battery_if_due(now)
    }

    /// A 0x1004 broadcast carries the battery percent. The feature index comes
    /// from the session table; with the table cleared (link just reset) the
    /// broadcast is skipped and the full read that follows has the value.
    ///
    /// A link-up 0x41 for our device also ends the read backoff: the mouse
    /// just woke up, so the next read is due now.
    fn apply_notifications(&mut self, now: Instant) {
        let battery_index = self.session.table().index_of(FeatureId::UNIFIED_BATTERY);
        // Never hardcoded: the index comes from the session table, which is empty
        // after a reset (events are then skipped; the full read has the state).
        let onboard_index = self.session.table().index_of(FeatureId::ONBOARD_PROFILES);
        let spy_index = self.session.table().index_of(FeatureId::MOUSE_BUTTON_SPY);
        let device_index = self.session.device_index();
        for message in self.session.drain_notifications() {
            if connection_notification(&message) == Some((device_index, true)) {
                self.read_pending = true;
                self.read_failures = 0;
                self.next_read = now;
                continue;
            }
            let Message::Notification {
                device_index: from,
                feature_index,
                function,
                payload,
                ..
            } = message
            else {
                continue;
            };
            if from != device_index {
                continue;
            }
            if Some(feature_index) == onboard_index {
                self.apply_onboard_event(function, &payload);
                continue;
            }
            if Some(feature_index) == spy_index {
                self.apply_spy_event(function, &payload);
                continue;
            }
            if Some(feature_index) != battery_index {
                continue;
            }
            if let Some(status) = parse_battery_status(&payload) {
                self.set_battery_status(status, &payload);
            }
        }
    }

    /// A 0x8100 notification [OBS]: fn0 payload[0..2] = new active profile
    /// sector (BE16) after the profile button, fn1 payload[0] = new DPI level
    /// index after DPI up/down. What the snapshot knows is applied at once;
    /// anything it cannot answer schedules the cheap confirm read.
    fn apply_onboard_event(&mut self, function: u8, payload: &[u8; 16]) {
        if self.snapshot.is_none() {
            return;
        }
        match function {
            0 => {
                let sector = u16::from_be_bytes([payload[0], payload[1]]);
                // Only the user sectors 1..=5 and the ROM sectors 0x0101..=0x0105
                // are profiles; anything else is not trusted, the read decides.
                if !matches!(sector, 1..=5 | 0x0101..=0x0105) {
                    self.confirm_pending = true;
                    return;
                }
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.onboard.active_profile_raw_hex = Some(format!("{sector:04x}0000"));
                    snapshot.onboard.active_profile_error = None;
                }
                self.updated_at_ms = clock_ms();
                self.dirty = true;
                // A profile switch applies that profile's default DPI level.
                let default = self
                    .profile_of(sector)
                    .map(|p| usize::from(p.default_dpi_index));
                self.set_dpi_level(sector, default);
                // The mouse has the last word.
                self.confirm_pending = true;
            }
            1 => {
                let index = usize::from(payload[0]);
                match self.active_sector() {
                    Some(sector) if index < 5 && self.set_dpi_level(sector, Some(index)) => {}
                    _ => self.confirm_pending = true,
                }
            }
            other => {
                if !self.unknown_event_logged {
                    self.unknown_event_logged = true;
                    eprintln!(
                        "myhub: unknown 0x8100 event fn{other}, payload {:02x?} (ignored, logged once)",
                        &payload[..4]
                    );
                }
            }
        }
    }

    /// The parsed profile stored for `sector`, if the snapshot has one.
    fn profile_of(&self, sector: u16) -> Option<&hidpp::onboard::ProfileSector> {
        let key = format!("0x{sector:04x}");
        self.snapshot
            .as_ref()?
            .onboard
            .profiles
            .iter()
            .find(|p| p.sector == key)?
            .profile
            .as_ref()
    }

    /// The active sector as the snapshot stores it (fn4 first two bytes).
    fn active_sector(&self) -> Option<u16> {
        let hex = self
            .snapshot
            .as_ref()?
            .onboard
            .active_profile_raw_hex
            .as_deref()?;
        u16::from_str_radix(hex.get(0..4)?, 16).ok()
    }

    /// In app control the mouse answers fn4 with 0; the profile that was active
    /// when app control began stands in for it. Only while the mouse reads host
    /// mode and this app holds the mark: onboard reads are never touched.
    fn apply_host_overlay(&mut self) {
        if !self.host_by_us() || snapshot_mode_byte_of(self) != Some(2) {
            return;
        }
        let Some(sector) = self.remembered_profile else {
            return;
        };
        if self.active_sector().unwrap_or(0) != 0 {
            return;
        }
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.onboard.active_profile_raw_hex = Some(format!("{sector:04x}0000"));
            snapshot.onboard.active_profile_error = None;
        }
    }

    /// Sets the current DPI to level `index` of `sector`'s profile when that
    /// level exists and is non-zero. `true` when it was applied.
    fn set_dpi_level(&mut self, sector: u16, index: Option<usize>) -> bool {
        let level = index
            .and_then(|i| self.profile_of(sector)?.dpi_levels.get(i).copied())
            .filter(|dpi| *dpi != 0);
        let (Some(dpi), Some(snapshot)) = (level, self.snapshot.as_mut()) else {
            return false;
        };
        snapshot.dpi.current = dpi;
        self.updated_at_ms = clock_ms();
        self.dirty = true;
        true
    }

    /// The cheap confirm read after a 0x8100 event: current DPI (0x2201 fn2)
    /// and active profile (0x8100 fn4), read tier, one attempt.
    fn confirm_read(&mut self) -> Result<(), LinkLost> {
        if !std::mem::take(&mut self.confirm_pending) {
            return Ok(());
        }
        if !self.link_up
            || self.link_known_down()
            || self.session.needs_rediscovery()
            || self.snapshot.is_none()
        {
            return Ok(());
        }
        self.tighten_if_stopping();
        let dpi = self.session.request_feature(
            FeatureId::ADJUSTABLE_DPI,
            hidpp::dpi::FN_GET_SENSOR_DPI,
            hidpp::dpi::sensor_params(0),
        );
        let active = self.session.request_feature(
            FeatureId::ONBOARD_PROFILES,
            hidpp::onboard::FN_GET_ACTIVE_PROFILE,
            [0; 16],
        );
        for reply in [&dpi, &active] {
            if let Err(SessionError::Link(e)) = reply {
                return Err(LinkLost(e.to_string()));
            }
        }
        if let (Ok(dpi), Some(snapshot)) = (&dpi, self.snapshot.as_mut()) {
            match hidpp::dpi::parse_sensor_dpi(dpi) {
                Ok(current) => snapshot.dpi.current = current.current,
                Err(e) => eprintln!("myhub: confirm read: DPI reply not understood: {e}"),
            }
        }
        if let (Ok(active), Some(snapshot)) = (&active, self.snapshot.as_mut()) {
            snapshot.onboard.active_profile_raw_hex =
                Some(active[..4].iter().map(|b| format!("{b:02x}")).collect());
            snapshot.onboard.active_profile_error = None;
        }
        for reply in [&dpi, &active] {
            if let Err(e) = reply {
                eprintln!("myhub: confirm read failed: {e}");
            }
        }
        self.apply_host_overlay();
        self.updated_at_ms = clock_ms();
        self.dirty = true;
        Ok(())
    }

    /// Stores a 0x1004 status (a reply or an event): percent, level, charging
    /// state and external power all come from the same four bytes.
    fn set_battery_status(&mut self, status: BatteryStatus, payload: &[u8; 16]) {
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.battery.percent = status.percent;
            snapshot.battery.status = status;
            snapshot.battery.raw_hex = payload.iter().map(|b| format!("{b:02x}")).collect();
            self.updated_at_ms = clock_ms();
            self.dirty = true;
        }
    }

    fn refresh_battery_if_due(&mut self, now: Instant) -> Result<(), LinkLost> {
        let due = now.saturating_duration_since(self.last_battery) >= BATTERY_REFRESH;
        if self.stopping() {
            return Ok(());
        }
        if !(due && self.link_up && self.snapshot.is_some() && !self.session.needs_rediscovery()) {
            return Ok(());
        }
        self.last_battery = now;
        let reply = self.session.request_feature(
            FeatureId::UNIFIED_BATTERY,
            battery::FN_GET_STATUS,
            [0; 16],
        );
        match reply {
            Ok(payload) => match parse_battery_status(&payload) {
                Some(status) => self.set_battery_status(status, &payload),
                None => eprintln!("myhub: battery reply not understood"),
            },
            Err(SessionError::Link(e)) => return Err(LinkLost(e.to_string())),
            // The mouse went away between two notifications; the 0x41 follows.
            Err(e) => eprintln!("myhub: battery refresh failed: {e}"),
        }
        Ok(())
    }

    /// How long a write must still wait to keep [`WRITE_INTERVAL`] since the
    /// previous one.
    fn write_delay(&self, now: Instant) -> Duration {
        self.last_write.map_or(Duration::ZERO, |last| {
            WRITE_INTERVAL.saturating_sub(now.saturating_duration_since(last))
        })
    }

    /// `true` once per connect or wake read (see `restore_due`).
    fn take_restore_due(&mut self) -> bool {
        std::mem::take(&mut self.restore_due)
    }

    /// Applies one volatile setting through the validated driver path
    /// (host mode check, range check, send, read back) and updates the
    /// snapshot with the verified value. Refuses with a contract error code
    /// without touching the device when writes are not allowed or the device
    /// is not reachable. Sleeps to keep the write rate limit.
    fn write(&mut self, id: &str, op: WriteOp, allow_writes: bool) -> Result<(), WriteFailure> {
        self.write_until(id, op, allow_writes, None)
    }

    /// [`Device::write`] that gives up with "deviceError:timeout", before
    /// anything is sent, when `deadline` has passed after the rate-limit wait
    /// (and again after the host switch).
    fn write_until(
        &mut self,
        id: &str,
        op: WriteOp,
        allow_writes: bool,
        deadline: Option<Instant>,
    ) -> Result<(), WriteFailure> {
        let expired = || deadline.is_some_and(|d| Instant::now() >= d);
        let timeout = || WriteFailure::Refused("deviceError:timeout".into());
        if !allow_writes {
            return Err(WriteFailure::Refused("writesDisabled".into()));
        }
        let known = self.detail().is_some_and(|d| d.summary.id == id);
        if !known || !self.link_up || self.link_known_down() || self.session.needs_rediscovery() {
            return Err(WriteFailure::Refused("disconnected".into()));
        }
        // No implicit switch: DPI and rate are written only in the temporary
        // mode this app entered (the mark). Nothing is sent otherwise.
        // The snapshot's mode must say host as well (no frame needed).
        if !self.host_by_us() || snapshot_mode_byte_of(self) != Some(2) {
            return Err(WriteFailure::Refused("notHostMode".into()));
        }
        thread::sleep(self.write_delay(Instant::now()));
        if expired() {
            return Err(timeout());
        }
        self.tighten_if_stopping();
        let result = match op {
            WriteOp::Dpi(dpi) => set_dpi(&mut self.session, dpi),
            WriteOp::ReportRate(hz) => set_report_rate(&mut self.session, hz),
        };
        self.last_write = Some(Instant::now());
        match result {
            Ok(value) => {
                self.apply_value(op, value);
                Ok(())
            }
            Err(e) => {
                if let WriteError::VerifyFailed { got, .. } = e {
                    // The read back is what the device really uses now.
                    self.apply_value(op, got);
                }
                if matches!(e, WriteError::Session(SessionError::Timeout)) {
                    // The set frame may have been applied: read the device
                    // again so the published detail matches it. (Also fires
                    // for a timeout on the earlier reads; that is harmless.)
                    self.read_pending = true;
                    self.resync = true;
                    self.next_read = Instant::now();
                }
                Err(classify_write_error(e))
            }
        }
    }

    /// Enters the temporary mode: reads the mode; when the mouse is onboard,
    /// switches it to host (verified) and remembers that MyHUB did it. A mouse
    /// that is already in host mode is marked as ours without a frame. Any failure to get
    /// to host mode is `notHostMode` (a lost link stays a lost link).
    fn ensure_host(&mut self) -> Result<(), WriteFailure> {
        match onboard_mode(&mut self.session) {
            // Already in host mode (inherited, or left by a crash): the user asked
            // for temporary mode, so it is theirs now: mark it, so `set_dpi` works
            // and quit or shutdown return it to onboard mode.
            Ok(Some(OnboardMode::Host)) => {
                self.set_host_by_us(true);
                Ok(())
            }
            Ok(Some(OnboardMode::Onboard)) => {
                // Marked before the frame goes out: the mouse was just read as
                // onboard, so if the switch happens but the reply is lost, the
                // return to onboard mode still knows it is ours.
                self.set_host_by_us(true);
                let sent_before = self.session.request_count();
                let result = set_onboard_mode(&mut self.session, OnboardMode::Host);
                self.last_write = Some(Instant::now());
                match result {
                    Ok(_) => {
                        self.apply_mode(2);
                        Ok(())
                    }
                    Err(e) => {
                        // The mark is cleared only when the fn1 frame itself was
                        // refused: by the policy (nothing sent) or by a device
                        // error answering fn1 (it was the only request sent).
                        // A lost reply or link, NeedsRediscovery, a mouse that
                        // is off (HID++ 1.0 0x08) and any failure after an
                        // ACKed fn1 (the fn2 read-back) may hide a switch that
                        // happened, so the mark stays; a mode 1 read clears it.
                        let fn1_refused = match &e {
                            WriteError::Session(SessionError::Policy(_)) => true,
                            WriteError::Session(SessionError::Device {
                                family: ErrorFamily::Hidpp10,
                                code: 0x08,
                                ..
                            }) => false,
                            WriteError::Session(SessionError::Device { .. }) => {
                                self.session.request_count() - sent_before <= 1
                            }
                            _ => false,
                        };
                        if fn1_refused {
                            self.set_host_by_us(false);
                        }
                        Err(self.mode_call_failed(e, true))
                    }
                }
            }
            Ok(None) => Err(WriteFailure::Refused("notHostMode".into())),
            Err(e) => Err(self.mode_call_failed(WriteError::Session(e), false)),
        }
    }

    /// A failed mode call: a lost link and an unreachable mouse keep their
    /// codes; a timeout also schedules a re-read (the switch may have
    /// happened). A failed switch is otherwise `notHostMode`, a failed mode
    /// read keeps its own code.
    fn mode_call_failed(&mut self, e: WriteError, switching: bool) -> WriteFailure {
        if matches!(e, WriteError::Session(SessionError::Timeout)) {
            self.read_pending = true;
            self.resync = true;
            self.next_read = Instant::now();
        }
        match classify_write_error(e) {
            WriteFailure::Refused(code) if switching && code != "disconnected" => {
                WriteFailure::Refused("notHostMode".into())
            }
            other => other,
        }
    }

    /// `enter_temporary_mode`: refuses like a write (writes off, not connected,
    /// deadline), then switches to host mode (verified) and sets the mark.
    fn enter_temporary(
        &mut self,
        id: &str,
        allow_writes: bool,
        deadline: Option<Instant>,
    ) -> Result<(), WriteFailure> {
        let expired = || deadline.is_some_and(|d| Instant::now() >= d);
        if !allow_writes {
            return Err(WriteFailure::Refused("writesDisabled".into()));
        }
        let known = self.detail().is_some_and(|d| d.summary.id == id);
        if !known || !self.link_up || self.link_known_down() || self.session.needs_rediscovery() {
            return Err(WriteFailure::Refused("disconnected".into()));
        }
        thread::sleep(self.write_delay(Instant::now()));
        if expired() {
            return Err(WriteFailure::Refused("deviceError:timeout".into()));
        }
        self.tighten_if_stopping();
        let was_marked = self.host_by_us();
        // The profile whose levels the DPI buttons walk through: what is active
        // now, before the mouse is in host mode (fn4 reads 0 there).
        let before = self.active_sector().filter(|s| (1..=5).contains(s));
        self.ensure_host()?;
        if !was_marked || self.remembered_profile.is_none() {
            self.remembered_profile = before.or(self.remembered_profile);
        }
        self.apply_host_overlay();
        // The DPI buttons work through the spy; without it host mode still works.
        self.start_spy().map_err(WriteFailure::Lost)
    }

    /// `leave_temporary_mode`: an explicit user click on "back to the profile".
    /// Returns the mouse to onboard mode (verified) whenever it reads host mode,
    /// also when this app did not switch it (a crash, an inherited host state):
    /// the user asked for it, which needs `allowDeviceWrites`. A no-op success
    /// when fn2 reads onboard; a mode that is neither 1 nor 2 is `verifyFailed`.
    /// With the mark set (undoing our own switch) no setting is needed.
    fn leave_temporary(
        &mut self,
        id: &str,
        allow_writes: bool,
        deadline: Option<Instant>,
    ) -> Result<(), WriteFailure> {
        let known = self.detail().is_some_and(|d| d.summary.id == id);
        if !known {
            return Err(WriteFailure::Refused("disconnected".into()));
        }
        if !self.link_up || self.link_known_down() || self.session.needs_rediscovery() {
            return Err(WriteFailure::Refused("disconnected".into()));
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(WriteFailure::Refused("deviceError:timeout".into()));
        }
        let was_marked = self.host_by_us();
        if !was_marked {
            // Not our switch: only with writes allowed, nothing sent otherwise.
            if !allow_writes {
                return Err(WriteFailure::Refused("writesDisabled".into()));
            }
            // Crash recovery: a spy may still be running from an earlier run, in
            // any mode. Stop it before anything else.
            self.stop_spy().map_err(WriteFailure::Lost)?;
            self.tighten_if_stopping();
            match onboard_mode(&mut self.session) {
                Ok(Some(OnboardMode::Host)) => self.set_host_by_us(true),
                Ok(Some(OnboardMode::Onboard)) => return Ok(()),
                Ok(None) => return Err(WriteFailure::Refused("verifyFailed".into())),
                Err(e) => return Err(self.mode_call_failed(WriteError::Session(e), false)),
            }
        }
        // The stop was sent above unless the mark was set; send it again only if
        // that one failed or was skipped.
        let stop_first = self.host_by_us_before_leave_stop(was_marked);
        self.return_to_onboard_with(stop_first)
            .map_err(WriteFailure::Lost)?;
        if snapshot_mode_byte_of(self) == Some(1) {
            Ok(())
        } else if self.host_by_us() {
            // The outcome is unknown (lost reply, rediscovery): the mark stays.
            Err(WriteFailure::Refused("disconnected".into()))
        } else {
            Err(WriteFailure::Refused("verifyFailed".into()))
        }
    }

    /// Whether `return_to_onboard_with` must send the spy stop itself: always
    /// when the mark was set (nothing stopped it yet), and when the earlier stop
    /// of an unmarked leave failed.
    fn host_by_us_before_leave_stop(&self, was_marked: bool) -> bool {
        was_marked || self.spy_stale()
    }

    /// Records a mode the device confirmed (wire byte 1 onboard, 2 host).
    fn apply_mode(&mut self, byte: u8) {
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        let ob = &mut snapshot.onboard;
        match ob.mode_raw_hex.as_mut() {
            Some(hex) if hex.len() >= 2 => hex.replace_range(0..2, &format!("{byte:02x}")),
            _ => ob.mode_raw_hex = Some(format!("{byte:02x}000000")),
        }
        ob.mode_error = None;
        self.updated_at_ms = clock_ms();
        self.dirty = true;
    }

    /// Puts the mouse back to onboard mode, but only when MyHUB switched it to
    /// host. One attempt, best effort, no retry. The flag is cleared by the
    /// attempt but kept while the mouse is unreachable, so a later read can
    /// still trigger the return (`return_if_writes_off`). Needs no setting: it
    /// undoes what a write did.
    fn return_to_onboard(&mut self) -> Result<(), LinkLost> {
        self.return_to_onboard_with(true)
    }

    fn return_to_onboard_with(&mut self, stop_first: bool) -> Result<(), LinkLost> {
        if !self.host_by_us() {
            return Ok(());
        }
        if !self.link_up || self.link_known_down() || self.session.needs_rediscovery() {
            eprintln!("myhub: not returning the mouse to onboard mode: it is not reachable (yet)");
            return Ok(());
        }
        // The spy goes first (always sent), then the switch back.
        if stop_first {
            self.stop_spy()?;
        }
        let remembered = self.remembered_profile;
        self.set_host_by_us(false);
        self.tighten_if_stopping();
        thread::sleep(self.write_delay(Instant::now()));
        let result = set_onboard_mode(&mut self.session, OnboardMode::Onboard);
        self.last_write = Some(Instant::now());
        match result {
            Ok(_) => {
                self.apply_mode(1);
                eprintln!("myhub: returned the mouse to onboard mode");
                Ok(())
            }
            Err(e) => {
                // A lost reply, a rediscovery, a mouse that is off or a lost
                // link says nothing about the mode: keep the mark so a later
                // read can try again (a mode 1 read clears it).
                if matches!(
                    &e,
                    WriteError::Session(
                        SessionError::Timeout
                            | SessionError::NeedsRediscovery
                            | SessionError::Link(_)
                    ) | WriteError::Session(SessionError::Device {
                        family: ErrorFamily::Hidpp10,
                        code: 0x08,
                        ..
                    }) | WriteError::VerifyFailed { .. }
                ) {
                    self.set_host_by_us(true);
                    self.remembered_profile = remembered;
                }
                if let WriteError::Session(SessionError::Link(link)) = e {
                    return Err(LinkLost(link.to_string()));
                }
                eprintln!("myhub: could not return the mouse to onboard mode: {e}");
                Ok(())
            }
        }
    }

    /// Stores a value the device confirmed in the snapshot.
    fn apply_value(&mut self, op: WriteOp, value: u16) {
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        match op {
            WriteOp::Dpi(_) => snapshot.dpi.current = value,
            WriteOp::ReportRate(_) => {
                snapshot.report_rate.current_hz = value;
                if let Ok(ms) = u8::try_from(1000 / value.max(1)) {
                    snapshot.report_rate.current_interval_ms = ms;
                }
            }
        }
        self.updated_at_ms = clock_ms();
        self.dirty = true;
    }

    /// Goes back to app control mode: enters it through the same code as
    /// `enter_temporary_mode` (host switch, mark, spy start), then applies the
    /// stored DPI and report rate that differ from the mouse, DPI first. If the
    /// mouse already runs app control (mark set, host), only the spy is made
    /// sure of and the values are reapplied. One attempt; a failure is logged,
    /// never retried; at most once per [`RESTORE_GAP`]. `Err` only when the
    /// receiver link is lost.
    fn restore_app_control(
        &mut self,
        id: &str,
        prefs: &DevicePrefs,
        allow_writes: bool,
        now: Instant,
    ) -> Result<Vec<String>, LinkLost> {
        let mut log = Vec::new();
        if self
            .last_restore
            .is_some_and(|at| now.saturating_duration_since(at) < RESTORE_GAP)
        {
            log.push("restore of app control mode skipped: done a moment ago".to_owned());
            return Ok(log);
        }
        self.last_restore = Some(now);
        if self.host_by_us() && snapshot_mode_byte_of(self) == Some(2) {
            if !self.spy_active() {
                self.start_spy()?;
                log.push("app control mode already on: spy started".to_owned());
            }
        } else {
            match self.enter_temporary(id, allow_writes, None) {
                Ok(()) => log.push("restored app control mode".to_owned()),
                Err(WriteFailure::Lost(lost)) => return Err(lost),
                Err(WriteFailure::Refused(code)) => {
                    log.push(format!("restore of app control mode failed: {code}"));
                    return Ok(log);
                }
            }
        }
        // A mouse that was already in host mode when the app started gave no
        // profile before the switch: the stored one stands in.
        if self.remembered_profile.is_none()
            && let Some(sector) = prefs
                .profile
                .filter(|s| (1..=5).contains(s) && self.profile_of(*s).is_some())
        {
            self.remembered_profile = Some(sector);
        }
        self.apply_host_overlay();
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Ok(log);
        };
        let mut ops = Vec::new();
        if let Some(dpi) = prefs.dpi.filter(|d| *d != snapshot.dpi.current) {
            ops.push(WriteOp::Dpi(dpi));
        }
        if let Some(hz) = prefs
            .report_rate_hz
            .filter(|h| *h != snapshot.report_rate.current_hz)
        {
            ops.push(WriteOp::ReportRate(hz));
        }
        for op in ops {
            match self.write(id, op, allow_writes) {
                Ok(()) => log.push(format!("restored {op:?}")),
                Err(WriteFailure::Lost(lost)) => return Err(lost),
                Err(WriteFailure::Refused(code)) => {
                    log.push(format!("restore of {op:?} failed: {code}; not retrying"));
                    break;
                }
            }
        }
        Ok(log)
    }

    /// Saves `request` into the profile that is active on the mouse (flash
    /// write). Refuses without a frame when writes are off, the mouse is not
    /// reachable, the request is out of range or the deadline passed. If MyHUB
    /// switched the mouse to host mode it is returned to onboard mode first.
    /// The backup of the target sector goes to `backup_root` before any write
    /// frame; afterwards the device is read again in full (the published detail
    /// then shows what the mouse has).
    #[cfg(test)]
    fn save_profile(
        &mut self,
        id: &str,
        request: &SaveRequest,
        allow_writes: bool,
        deadline: Option<Instant>,
        backup_root: Option<&Path>,
        now_unix: u64,
    ) -> Result<SaveOutcome, WriteFailure> {
        self.save_profile_attempt(id, request, allow_writes, deadline, backup_root, now_unix)
            .0
    }

    /// [`Device::save_profile`] that also reports a receiver link lost during
    /// the re-read after the write. The result is the classified save outcome
    /// either way; the caller answers with it and then drops the session.
    fn save_profile_attempt(
        &mut self,
        id: &str,
        request: &SaveRequest,
        allow_writes: bool,
        deadline: Option<Instant>,
        backup_root: Option<&Path>,
        now_unix: u64,
    ) -> (Result<SaveOutcome, WriteFailure>, Option<LinkLost>) {
        let mut lost = None;
        let result = self.save_inner(
            id,
            request,
            allow_writes,
            deadline,
            backup_root,
            now_unix,
            &mut lost,
        );
        (result, lost)
    }

    #[allow(clippy::too_many_arguments)]
    fn save_inner(
        &mut self,
        id: &str,
        request: &SaveRequest,
        allow_writes: bool,
        deadline: Option<Instant>,
        backup_root: Option<&Path>,
        now_unix: u64,
        lost: &mut Option<LinkLost>,
    ) -> Result<SaveOutcome, WriteFailure> {
        // The caller's give-up instant: too little left means state unknown
        // for the caller, so do not start.
        let expired = || {
            deadline
                .is_some_and(|d| d.saturating_duration_since(Instant::now()) < SAVE_MIN_REMAINING)
        };
        let timeout = || WriteFailure::Refused("deviceError:timeout".into());
        if !allow_writes {
            return Err(WriteFailure::Refused("writesDisabled".into()));
        }
        let known = self.detail().is_some_and(|d| d.summary.id == id);
        if !known || !self.link_up || self.link_known_down() || self.session.needs_rediscovery() {
            return Err(WriteFailure::Refused("disconnected".into()));
        }
        let patch = request.to_patch()?;
        if expired() {
            return Err(timeout());
        }
        if self.host_by_us() {
            // We put the mouse in host mode for a DPI or rate write; a profile
            // is saved in onboard mode.
            self.return_to_onboard().map_err(WriteFailure::Lost)?;
            self.returned_from_host = !self.host_by_us();
            thread::sleep(WRITE_INTERVAL);
        }
        thread::sleep(self.write_delay(Instant::now()));
        if expired() {
            return Err(timeout());
        }
        self.tighten_if_stopping();

        let mut backup_path: Option<PathBuf> = None;
        let mut backup = |sector: u16, bytes: &[u8; 255]| -> Result<(), String> {
            let root = backup_root.ok_or("no app data directory for the backup")?;
            backup_path = Some(backup::write_backup(root, id, now_unix, sector, bytes)?);
            Ok(())
        };
        // From here a flash write may run: shutdown waits for it. The flag is
        // set before the stop check, shutdown sets the stop flag before it reads
        // the flag, so one of the two always sees the other.
        let in_flight = FlashFlag::set(&self.flash_in_flight);
        if self.stopping() {
            return Err(WriteFailure::Refused("disconnected".into()));
        }
        let result = save_active_profile(&mut self.session, &patch, &mut backup);
        drop(in_flight);
        self.last_write = Some(Instant::now());

        if let Err(e) = &result
            && nothing_written(e)
        {
            return Err(classify_save_error(result.unwrap_err()));
        }
        // Frames that may have changed the flash went out: read the mouse again
        // so the published detail is what it has now.
        // A link lost here must not replace the save result: it is reported
        // after the answer, the detail stays as last known.
        if let Err(link) = self.read_now(Instant::now()) {
            *lost = Some(link);
        }
        match result {
            Ok(report) => {
                let log = &report.write.log;
                eprintln!(
                    "myhub: saved profile sector {:#06x}: {:?}; fn6 {:?}; fn7 {:?}; fn8 {:?}; reselect {:?}; reloaded {}; reload fn3 {:?}; backup {:?}",
                    report.sector,
                    report.write.outcome,
                    log.begin,
                    log.data,
                    log.end,
                    report.reselected,
                    report.reloaded,
                    report.reload_log,
                    backup_path
                );
                Ok(SaveOutcome {
                    written: report.write.outcome == logitech_g502x::WriteOutcome::Written,
                    reloaded: report.reloaded
                        && report.write.outcome == logitech_g502x::WriteOutcome::Written,
                    backup_path,
                })
            }
            Err(e) => {
                eprintln!("myhub: saving the profile failed: {e}");
                Err(classify_save_error(e))
            }
        }
    }

    /// Re-reads everything now (the `refresh_device` command). `None` when the
    /// mouse is known to be off or the read failed.
    fn refresh(&mut self, now: Instant) -> Result<Option<DeviceDetail>, LinkLost> {
        if self.link_known_down() {
            return Ok(None);
        }
        self.read_failures = 0;
        Ok(if self.read_now(now)? {
            self.detail()
        } else {
            None
        })
    }
}

/// Runs `body` until it returns. After a panic `on_panic` runs, then (unless
/// [`MAX_RESTARTS`] restarts were used up) `pause` decides whether to go
/// on (`false` = stop). Returns `true` when it gave up after the restarts.
fn supervise<T>(
    ctx: &mut T,
    mut body: impl FnMut(&mut T),
    mut on_panic: impl FnMut(&mut T),
    mut pause: impl FnMut(&mut T) -> bool,
) -> bool {
    let mut restarts = 0;
    loop {
        if panic::catch_unwind(AssertUnwindSafe(|| body(ctx))).is_ok() {
            return false;
        }
        on_panic(ctx);
        if restarts >= MAX_RESTARTS {
            eprintln!("myhub: the device manager keeps failing, stopping it");
            return true;
        }
        restarts += 1;
        if !pause(ctx) {
            return false;
        }
    }
}

/// Last resort before the worker gives up for good: if MyHUB switched the mouse
/// to host mode, discover it again on a fresh session and try the return to
/// onboard mode once. Nothing is sent when the mark is not set.
fn return_blind<L: HidLink>(device: &mut Device<L>) -> Result<(), LinkLost> {
    if !device.host_by_us() {
        return Ok(());
    }
    if let Err(e) = device.session.discover() {
        eprintln!("myhub: cannot return the mouse to onboard mode: discovery failed: {e}");
        return Ok(());
    }
    // The mark says the mouse was in host mode and discovery just worked.
    device.link_up = true;
    device.return_to_onboard()
}

struct Worker {
    app: AppHandle,
    config_dir: Option<PathBuf>,
    /// App data directory (backups live under `backups/`).
    data_dir: Option<PathBuf>,
    state: Arc<Mutex<ManagerState>>,
    commands: Receiver<Command>,
    shutdown: Arc<AtomicBool>,
    flash_in_flight: Arc<AtomicBool>,
    api: Option<HidApi>,
    /// Per device id: MyHUB switched it to host mode. Outlives the `Device`.
    host_marks: HashMap<String, Arc<AtomicBool>>,
    /// Per device id: the button spy runs. Outlives the `Device`.
    spy_marks: HashMap<String, Arc<AtomicBool>>,
    /// Per device id: a spy stop failed, the spy may still run.
    spy_stales: HashMap<String, Arc<AtomicBool>>,
}

impl Worker {
    fn stopping(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// The current settings, read at the moment of use (defaults without the
    /// managed state, which forbids writes).
    fn settings(&self) -> Settings {
        self.app
            .try_state::<SettingsState>()
            .map(|s| s.get())
            .unwrap_or_default()
    }

    fn prefs_path(&self, id: &str) -> Option<PathBuf> {
        settings::prefs_path(self.config_dir.as_deref()?, id)
    }

    /// Runs one `set_dpi` / `set_report_rate` command: checks the setting at
    /// execution time, writes through the device, stores the prefs and returns
    /// the new detail (not yet published).
    fn execute_write(
        &self,
        device: &mut Device<HidapiLink>,
        id: &str,
        op: WriteOp,
        deadline: Instant,
    ) -> Result<DeviceDetail, WriteFailure> {
        run_user_write(
            device,
            id,
            op,
            deadline,
            self.settings().allow_device_writes,
        )?;
        self.store_prefs(id, op);
        device
            .detail()
            .ok_or_else(|| WriteFailure::Refused("disconnected".into()))
    }

    /// Runs `enter_temporary_mode` / `leave_temporary_mode` (not yet published).
    fn execute_mode(
        &self,
        device: &mut Device<HidapiLink>,
        id: &str,
        enter: bool,
        deadline: Instant,
    ) -> Result<DeviceDetail, WriteFailure> {
        let path = self.prefs_path(id);
        let allow = self.settings().allow_device_writes;
        if enter {
            enter_and_store(device, path.as_deref(), id, allow, deadline)?;
        } else {
            leave_and_store(device, path.as_deref(), id, allow, deadline)?;
        }
        device
            .detail()
            .ok_or_else(|| WriteFailure::Refused("disconnected".into()))
    }

    /// Runs one `save_profile_to_mouse` command (not yet published).
    fn execute_save(
        &self,
        device: &mut Device<HidapiLink>,
        id: &str,
        request: &SaveRequest,
        deadline: Instant,
    ) -> (
        Result<(SaveOutcome, DeviceDetail), WriteFailure>,
        Option<LinkLost>,
    ) {
        let backups = self.data_dir.as_ref().map(|dir| dir.join("backups"));
        let (result, lost) = device.save_profile_attempt(
            id,
            request,
            self.settings().allow_device_writes,
            Some(deadline),
            backups.as_deref(),
            clock_ms() / 1000,
        );
        record_save_mode(device, self.prefs_path(id).as_deref());
        let result = result.and_then(|outcome| {
            let detail = device
                .detail()
                .ok_or_else(|| WriteFailure::Refused("disconnected".into()))?;
            Ok((outcome, detail))
        });
        (result, lost)
    }

    /// Remembers the value the user applied. A failure to save is logged: the
    /// value is applied on the device either way.
    fn store_prefs(&self, id: &str, op: WriteOp) {
        let Some(path) = self.prefs_path(id) else {
            return;
        };
        update_prefs(&path, |prefs| match op {
            WriteOp::Dpi(dpi) => prefs.dpi = Some(dpi),
            WriteOp::ReportRate(hz) => prefs.report_rate_hz = Some(hz),
        });
    }

    /// After the first successful read and after each link-up: goes back to the
    /// last mode and values when `restoreLastMode` and `allowDeviceWrites` are on
    /// and the stored mode is host. One attempt per connect event, no retries.
    fn restore_on_connect(&self, device: &mut Device<HidapiLink>) -> Result<(), LinkLost> {
        let settings = self.settings();
        let lines = restore_last_mode_if_enabled(
            device,
            &settings,
            |id| {
                self.prefs_path(id)
                    .and_then(|path| settings::load_prefs(&path))
            },
            Instant::now(),
        )?;
        for line in lines {
            eprintln!("myhub: {line}");
        }
        Ok(())
    }

    /// Stores `readOnly` per the contract: the user switch, host mode and the
    /// connection decide.
    fn finalize(&self, mut detail: DeviceDetail) -> DeviceDetail {
        detail.read_only = detail.compute_read_only(self.settings().allow_device_writes);
        detail
    }

    /// Runs the worker body; a panic is logged, the published device list is
    /// emptied and the body restarts after [`RESTART_DELAY`], at most
    /// [`MAX_RESTARTS`] times.
    fn run(mut self) {
        let gave_up = supervise(
            &mut self,
            |worker| worker.run_loop(),
            |worker| {
                eprintln!("myhub: the device manager panicked");
                worker.publish(None, true);
            },
            |worker| {
                // The hidapi context may be in an unknown state.
                worker.api = None;
                worker.wait(RESTART_DELAY);
                !worker.stopping()
            },
        );
        if gave_up {
            self.last_resort_return();
        }
    }

    /// After the restarts are used up: one attempt to hand the mouse back to its
    /// onboard profiles, if MyHUB switched it to host mode. Never panics.
    fn last_resort_return(&mut self) {
        if !self.host_marks.values().any(|m| m.load(Ordering::SeqCst)) {
            return;
        }
        self.api = None;
        let attempt = panic::catch_unwind(AssertUnwindSafe(|| match self.try_open() {
            Some(mut device) => {
                if let Err(LinkLost(reason)) = return_blind(&mut device) {
                    eprintln!("myhub: link lost while returning to onboard mode: {reason}");
                }
            }
            None => eprintln!(
                "myhub: could not reopen the receiver to return the mouse to onboard mode"
            ),
        }));
        if attempt.is_err() {
            eprintln!("myhub: the return to onboard mode panicked");
        }
    }

    fn run_loop(&mut self) {
        let mut device: Option<Device<_>> = None;
        while !self.stopping() {
            let Some(connected) = device.as_mut() else {
                device = self.try_open();
                if device.is_none() {
                    self.wait(SCAN_INTERVAL);
                }
                continue;
            };
            if let Err(LinkLost(reason)) = self.serve(connected) {
                eprintln!("myhub: receiver link lost: {reason}");
                device = None;
                self.publish(None, true);
            }
        }
        // Shutdown: give the mouse back to its onboard profiles, once, before
        // the session is dropped (only when MyHUB switched it to host).
        if let Some(connected) = device.as_mut()
            && let Err(LinkLost(reason)) = connected.return_to_onboard()
        {
            eprintln!("myhub: receiver link lost while shutting down: {reason}");
        }
    }

    /// Answers queued commands, then runs one pass of the device loop and
    /// publishes the result.
    fn serve(&mut self, device: &mut Device<HidapiLink>) -> Result<(), LinkLost> {
        // All queued refreshes share one read and one answer.
        let mut pending: Vec<(String, Sender<Option<DeviceDetail>>)> = Vec::new();
        let mut republish = false;
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Refresh { id, reply } => pending.push((id, reply)),
                Command::SettingsChanged => {
                    republish = true;
                    if !self.settings().allow_device_writes {
                        // Writes were turned off: hand the mode back.
                        device.return_to_onboard()?;
                        if device.take_dirty() {
                            self.publish(device.detail(), true);
                        }
                    }
                }
                Command::Write {
                    id,
                    op,
                    deadline,
                    reply,
                } => {
                    if self.stopping() {
                        let _ = reply.send(Err("disconnected".to_owned()));
                        continue;
                    }
                    // Writes run one at a time, in arrival order.
                    let result = self.execute_write(device, &id, op, deadline);
                    if device.take_dirty() {
                        self.publish(device.detail(), true);
                    }
                    match result {
                        Ok(detail) => {
                            let _ = reply.send(Ok(self.finalize(detail)));
                        }
                        Err(WriteFailure::Refused(code)) => {
                            let _ = reply.send(Err(code));
                        }
                        Err(WriteFailure::Lost(lost)) => {
                            let _ = reply.send(Err("disconnected".to_owned()));
                            return Err(lost);
                        }
                    }
                }
                Command::Mode {
                    id,
                    enter,
                    deadline,
                    reply,
                } => {
                    if self.stopping() {
                        let _ = reply.send(Err("disconnected".to_owned()));
                        continue;
                    }
                    let result = self.execute_mode(device, &id, enter, deadline);
                    if device.take_dirty() {
                        self.publish(device.detail(), true);
                    }
                    match result {
                        Ok(detail) => {
                            let _ = reply.send(Ok(self.finalize(detail)));
                        }
                        Err(WriteFailure::Refused(code)) => {
                            let _ = reply.send(Err(code));
                        }
                        Err(WriteFailure::Lost(lost)) => {
                            let _ = reply.send(Err("disconnected".to_owned()));
                            return Err(lost);
                        }
                    }
                }
                Command::Save {
                    id,
                    request,
                    deadline,
                    reply,
                } => {
                    if self.stopping() {
                        let _ = reply.send(Err("disconnected".to_owned()));
                        continue;
                    }
                    let (result, lost) = self.execute_save(device, &id, &request, deadline);
                    if device.take_dirty() {
                        self.publish(device.detail(), true);
                    }
                    match result {
                        Ok((outcome, detail)) => {
                            let _ = reply.send(Ok(SaveResult {
                                detail: self.finalize(detail),
                                outcome: if outcome.written {
                                    "written"
                                } else {
                                    "unchanged"
                                },
                                reloaded: outcome.reloaded,
                                backup_path: outcome.backup_path.map(|p| p.display().to_string()),
                            }));
                        }
                        Err(WriteFailure::Refused(code)) => {
                            let _ = reply.send(Err(code));
                        }
                        Err(WriteFailure::Lost(link)) => {
                            let _ = reply.send(Err("disconnected".to_owned()));
                            return Err(link);
                        }
                    }
                    // The save was answered with its own result; now the worker
                    // learns that the receiver is gone.
                    if let Some(link) = lost {
                        return Err(link);
                    }
                }
                Command::Wake => {}
            }
        }
        if republish && let Some(detail) = device.detail() {
            self.publish(Some(detail), true);
        }
        if !pending.is_empty() {
            let current = device.detail().map(|d| d.summary.id);
            let known = |id: &str| current.as_deref() == Some(id);
            let answer = if pending.iter().any(|(id, _)| known(id)) {
                device.refresh(Instant::now())?
            } else {
                None
            };
            let answer = answer.map(|d| self.finalize(d));
            if device.take_dirty() {
                self.publish(device.detail(), true);
            }
            for (id, reply) in pending {
                let _ = reply.send(if known(&id) { answer.clone() } else { None });
            }
        }
        if self.stopping() {
            return Ok(());
        }
        device.step(Instant::now())?;
        return_if_writes_off(device, &self.settings())?;
        if self.stopping() {
            return Ok(());
        }
        // A DPI button press queued by the spy: applied through the set_dpi path.
        device.apply_dpi_target(self.settings().allow_device_writes)?;
        self.restore_on_connect(device)?;
        if device.take_dirty() {
            self.publish(device.detail(), false);
        }
        Ok(())
    }

    /// Sleeps up to `duration`, answering refresh commands with "nothing"
    /// because there is no session to read from.
    fn wait(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || self.stopping() {
                return;
            }
            match self.commands.recv_timeout(left) {
                Ok(Command::Refresh { reply, .. }) => {
                    let _ = reply.send(None);
                }
                Ok(Command::Write { reply, .. }) => {
                    let code = if self.settings().allow_device_writes {
                        "disconnected"
                    } else {
                        "writesDisabled"
                    };
                    let _ = reply.send(Err(code.to_owned()));
                }
                Ok(Command::Mode { reply, enter, .. }) => {
                    let code = if enter && !self.settings().allow_device_writes {
                        "writesDisabled"
                    } else {
                        "disconnected"
                    };
                    let _ = reply.send(Err(code.to_owned()));
                }
                Ok(Command::Save { reply, .. }) => {
                    let code = if self.settings().allow_device_writes {
                        "disconnected"
                    } else {
                        "writesDisabled"
                    };
                    let _ = reply.send(Err(code.to_owned()));
                }
                Ok(Command::SettingsChanged) => {}
                Ok(Command::Wake) => return,
                Err(_) => return,
            }
        }
    }

    /// Looks for the receiver and opens a session on it.
    fn try_open(&mut self) -> Option<Device<HidapiLink>> {
        if self.api.is_none() {
            match HidApi::new() {
                Ok(api) => self.api = Some(api),
                Err(e) => {
                    eprintln!("myhub: hidapi unavailable: {e}");
                    return None;
                }
            }
        }
        let api = self.api.as_mut()?;
        if let Err(e) = api.refresh_devices() {
            eprintln!("myhub: cannot enumerate HID devices: {e}");
            return None;
        }
        let receivers = find_receivers(api);
        let receiver = receivers.first()?;
        match HidapiSession::open(api, receiver) {
            Ok(session) => {
                let mark = Arc::clone(self.host_marks.entry(DEVICE_ID.to_owned()).or_default());
                let spy = Arc::clone(self.spy_marks.entry(DEVICE_ID.to_owned()).or_default());
                let stale = Arc::clone(self.spy_stales.entry(DEVICE_ID.to_owned()).or_default());
                Some(
                    Device::new(session, Instant::now())
                        .with_host_mark(mark)
                        .with_spy_mark(spy)
                        .with_spy_stale(stale)
                        .with_shutdown(Arc::clone(&self.shutdown))
                        .with_flash_flag(Arc::clone(&self.flash_in_flight)),
                )
            }
            Err(e) => {
                eprintln!("myhub: cannot open the receiver: {e}");
                None
            }
        }
    }

    /// Stores the new detail and emits events for what changed. `force`
    /// emits the detail event even when only the timestamp changed (an
    /// explicit refresh).
    fn publish(&self, detail: Option<DeviceDetail>, force: bool) {
        let detail = detail.map(|d| self.finalize(d));
        publish_slot(
            &self.app,
            &self.state,
            self.stopping(),
            Slot::Logitech,
            detail,
            force,
        );
    }
}

/// Stores the new detail of one device and emits events for what changed. The
/// summary list always holds every known device. `force` emits the detail event
/// even when only the timestamp changed (an explicit refresh). A `None` clears the
/// device's slot.
pub(crate) fn publish_slot(
    app: &AppHandle,
    state: &Mutex<ManagerState>,
    stopping: bool,
    slot: Slot,
    detail: Option<DeviceDetail>,
    force: bool,
) {
    let (change, summaries, tray) = {
        let mut state = lock(state);
        let change = changes(state.slot_mut(slot).as_ref(), detail.as_ref());
        *state.slot_mut(slot) = detail.clone();
        (change, state.summaries(), state.tray_details())
    };
    if stopping {
        return;
    }
    if change.summaries
        && let Err(e) = app.emit(DEVICES_CHANGED_EVENT, &summaries)
    {
        eprintln!("myhub: cannot emit {DEVICES_CHANGED_EVENT}: {e}");
    }
    if let Some(detail) = detail.as_ref()
        && (change.detail || force)
        && let Err(e) = app.emit(DEVICE_DETAIL_CHANGED_EVENT, detail)
    {
        eprintln!("myhub: cannot emit {DEVICE_DETAIL_CHANGED_EVENT}: {e}");
    }
    if change.summaries || change.detail {
        update_tray(app, tray.0.as_ref(), tray.1.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;

    use myhub_transport::LinkError;

    use super::*;

    const FRAMES_LOG: &str = include_str!("../../crates/hidpp/tests/fixtures/frames.log");
    const IDX_BATTERY: u8 = 6;
    const IDX_ONBOARD: u8 = 9;
    /// Synthetic 0x8100 fn2 / fn4 exchanges (not in the log), as in the driver
    /// tests: mode host (0x02 on the wire), active profile 2.
    const FN2: (&str, &str) = (
        "1101092b00000000000000000000000000000000",
        "1101092b02000000000000000000000000000000",
    );
    const FN4: (&str, &str) = (
        "1101094b00000000000000000000000000000000",
        "1101094b00020000000000000000000000000000",
    );

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    fn long(prefix: &[u8]) -> Vec<u8> {
        let mut v = prefix.to_vec();
        v.resize(20, 0);
        v
    }

    type ReplyUpdates = Vec<(Vec<u8>, Vec<u8>)>;

    mod s6;

    /// Answers 0x8100 frames itself: `Some(frames)` (empty = silence) or `None` to
    /// fall back to the static replies.
    type OnboardHook = Box<dyn FnMut(&[u8]) -> Option<Vec<Vec<u8>>>>;

    #[derive(Default)]
    struct Mock {
        replies: HashMap<Vec<u8>, Vec<u8>>,
        inbox: VecDeque<Vec<u8>>,
        writes: usize,
        /// Answer every request with a HID++ 1.0 0x8F error 0x08 (device off).
        device_off: bool,
        /// Fail every read like an unplugged receiver.
        broken: bool,
        /// Swallow this many requests to the onboard feature without a reply.
        mute_onboard: usize,
        /// Requests that get no reply at all (a timeout).
        silent: Vec<Vec<u8>>,
        /// Extra frame delivered right after the reply to this request.
        push_after: HashMap<Vec<u8>, Vec<u8>>,
        /// After the reply to this request, these requests get new replies.
        then_reply: HashMap<Vec<u8>, ReplyUpdates>,
        /// Every request written, in order.
        sent: Vec<Vec<u8>>,
        /// A simulated onboard memory (see `s6`).
        onboard_hook: Option<OnboardHook>,
        /// `(function, n)`: after the n-th answered 0x8100 frame of this function
        /// the link breaks (queued frames are still delivered).
        break_after: Option<(u8, usize)>,
    }

    type Handle = Rc<RefCell<Mock>>;

    struct MockLink(Handle);

    impl HidLink for MockLink {
        fn write_report(&mut self, report: &[u8; 20]) -> Result<(), LinkError> {
            let mut mock = self.0.borrow_mut();
            mock.writes += 1;
            mock.sent.push(report.to_vec());
            if mock.device_off {
                // 10 dev 8F featureIndex fn|sw error
                let frame = vec![0x10, 0x01, 0x8f, report[2], report[3], 0x08, 0x00];
                mock.inbox.push_back(frame);
                return Ok(());
            }
            if report[2] == IDX_ONBOARD
                && let Some(mut hook) = mock.onboard_hook.take()
            {
                let answer = hook(report.as_slice());
                mock.onboard_hook = Some(hook);
                if let Some(frames) = answer {
                    mock.inbox.extend(frames);
                    if let Some((function, n)) = mock.break_after
                        && report[3] >> 4 == function
                    {
                        if n <= 1 {
                            mock.broken = true;
                            mock.break_after = None;
                        } else {
                            mock.break_after = Some((function, n - 1));
                        }
                    }
                    return Ok(());
                }
            }
            if mock.mute_onboard > 0 && report[2] == IDX_ONBOARD {
                mock.mute_onboard -= 1;
                return Ok(());
            }
            if mock
                .silent
                .iter()
                .any(|tx| tx.as_slice() == report.as_slice())
            {
                return Ok(());
            }
            let extra = mock.push_after.remove(report.as_slice());
            let reply = mock
                .replies
                .get(report.as_slice())
                .cloned()
                .unwrap_or_else(|| panic!("unexpected TX {report:02x?}"));
            mock.inbox.push_back(reply);
            mock.inbox.extend(extra);
            if let Some(updates) = mock.then_reply.get(report.as_slice()).cloned() {
                mock.replies.extend(updates);
            }
            Ok(())
        }

        fn read_report(
            &mut self,
            buf: &mut [u8; 64],
            _timeout_ms: i32,
        ) -> Result<usize, LinkError> {
            let mut mock = self.0.borrow_mut();
            if mock.broken && mock.inbox.is_empty() {
                return Err(LinkError::Hid("unplugged".into()));
            }
            match mock.inbox.pop_front() {
                Some(frame) => {
                    buf[..frame.len()].copy_from_slice(&frame);
                    Ok(frame.len())
                }
                None => {
                    drop(mock);
                    thread::sleep(Duration::from_millis(1));
                    Ok(0)
                }
            }
        }
    }

    fn mock_device() -> (Device<MockLink>, Handle) {
        let handle: Handle = Rc::new(RefCell::new(Mock::default()));
        {
            let mut mock = handle.borrow_mut();
            let mut pending: Option<Vec<u8>> = None;
            for line in FRAMES_LOG.lines().filter(|l| !l.trim().is_empty()) {
                let (tag, rest) = line.split_once(' ').expect("log line");
                match tag {
                    "TX" => pending = Some(hex(rest.trim())),
                    _ => {
                        let tx = pending.take().expect("RX without TX");
                        mock.replies.insert(tx, hex(rest.trim()));
                    }
                }
            }
            for (tx, rx) in [FN2, FN4] {
                mock.replies.insert(hex(tx), hex(rx));
            }
            // 0x8110 button spy fn1 (start) and fn2 (stop): zero replies [OBS].
            for tx in [
                "11010a1b00000000000000000000000000000000",
                "11010a2b00000000000000000000000000000000",
            ] {
                mock.replies.insert(hex(tx), hex(tx));
            }
        }
        let mut session = HidppSession::new(MockLink(Rc::clone(&handle)));
        session.set_reply_timeout(Duration::from_millis(50));
        let mut device = Device::new(session, Instant::now());
        device.poll_timeout = Duration::from_millis(5);
        (device, handle)
    }

    fn step(device: &mut Device<MockLink>) {
        device.step(Instant::now()).expect("step");
    }

    fn battery_broadcast(percent: u8) -> Vec<u8> {
        long(&[0x11, 0x01, IDX_BATTERY, 0x00, percent, 0x04])
    }

    #[test]
    fn first_step_reads_the_snapshot() {
        let (mut device, _handle) = mock_device();
        assert!(device.detail().is_none());
        step(&mut device);
        assert!(device.take_dirty());
        let detail = device.detail().expect("detail");
        assert_eq!(detail.summary.connection, Connection::Connected);
        assert_eq!(detail.summary.battery_percent, Some(55));
        let onboard = detail.onboard.expect("onboard");
        assert_eq!(onboard.active_profile, Some(2));
    }

    #[test]
    fn battery_broadcast_updates_the_battery_without_a_request() {
        let (mut device, handle) = mock_device();
        step(&mut device);
        device.take_dirty();
        let writes = handle.borrow().writes;

        handle.borrow_mut().inbox.push_back(battery_broadcast(41));
        step(&mut device);
        assert!(device.take_dirty());
        let detail = device.detail().unwrap();
        assert_eq!(detail.summary.battery_percent, Some(41));
        assert_eq!(detail.battery.unwrap().percent, 41);
        assert_eq!(handle.borrow().writes, writes, "a request was sent");

        // A frame for another feature index is ignored.
        handle
            .borrow_mut()
            .inbox
            .push_back(long(&[0x11, 0x01, 8, 0x00, 7]));
        step(&mut device);
        assert!(!device.take_dirty());
        assert_eq!(device.detail().unwrap().summary.battery_percent, Some(41));
    }

    #[test]
    fn link_down_keeps_the_last_detail_and_link_up_rereads() {
        let (mut device, handle) = mock_device();
        step(&mut device);
        device.take_dirty();
        handle.borrow_mut().inbox.push_back(battery_broadcast(41));
        step(&mut device);
        device.take_dirty();

        // Switched off: `11 62 9f 40`.
        handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
        let writes = handle.borrow().writes;
        step(&mut device);
        assert!(device.take_dirty());
        let detail = device.detail().expect("last detail kept");
        assert_eq!(detail.summary.connection, Connection::Disconnected);
        assert_eq!(detail.summary.battery_percent, Some(41));
        // While down nothing is read or written, however often it steps.
        for _ in 0..3 {
            step(&mut device);
        }
        assert_eq!(handle.borrow().writes, writes, "wrote while link down");
        assert_eq!(
            status_text(device.detail().as_ref(), Language::Pl).as_deref(),
            Some("G502 X: uśpiona")
        );

        // Switched on: `11 a2 9f 40`; the next steps rediscover and re-read.
        handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
        step(&mut device);
        step(&mut device);
        assert!(device.take_dirty());
        let detail = device.detail().unwrap();
        assert_eq!(detail.summary.connection, Connection::Connected);
        assert_eq!(detail.summary.battery_percent, Some(55), "fresh read");
        assert!(handle.borrow().writes > writes);
    }

    #[test]
    fn a_failed_snapshot_read_after_link_up_is_retried_until_connected() {
        let (mut device, handle) = mock_device();
        step(&mut device);
        device.take_dirty();
        handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
        step(&mut device);
        assert_eq!(
            device.detail().unwrap().summary.connection,
            Connection::Disconnected
        );

        // Link up; discovery succeeds but the first ReadData times out.
        handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
        handle.borrow_mut().mute_onboard = 1;
        step(&mut device); // hears the 0x41
        assert!(device.read_pending);
        step(&mut device); // discover ok, snapshot read times out
        assert!(!device.session.needs_rediscovery(), "discovery succeeded");
        assert!(device.read_pending, "failed read must stay pending");
        assert_eq!(device.read_failures, 1);
        assert_eq!(
            device.detail().unwrap().summary.connection,
            Connection::Disconnected
        );

        // Not before the backoff, then the next pass re-reads.
        let writes = handle.borrow().writes;
        step(&mut device);
        assert_eq!(handle.borrow().writes, writes);
        device.next_read = Instant::now();
        step(&mut device);
        assert!(!device.read_pending);
        assert_eq!(device.read_failures, 0);
        assert_eq!(
            device.detail().unwrap().summary.connection,
            Connection::Connected
        );
    }

    #[test]
    fn retry_delay_doubles_from_two_seconds_and_caps_at_five_minutes() {
        let secs = |n| retry_delay(n).as_secs();
        let seq: Vec<u64> = (1..=10).map(secs).collect();
        assert_eq!(seq, [2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
        assert_eq!(secs(0), 2);
        assert_eq!(secs(u32::MAX), 300);
    }

    #[test]
    fn link_up_and_refresh_reset_the_backoff() {
        let (mut device, handle) = mock_device();
        handle.borrow_mut().device_off = true;
        step(&mut device);
        device.next_read = Instant::now();
        step(&mut device);
        assert_eq!(device.read_failures, 2);
        assert!(device.next_read > Instant::now() + Duration::from_secs(3));

        handle.borrow_mut().device_off = false;
        handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
        step(&mut device);
        assert_eq!(device.read_failures, 0);
        step(&mut device);
        assert!(
            device.detail().is_some(),
            "read right after the link-up, no backoff wait"
        );

        let (mut device, handle) = mock_device();
        handle.borrow_mut().device_off = true;
        step(&mut device);
        assert_eq!(device.read_failures, 1);
        handle.borrow_mut().device_off = false;
        assert!(device.refresh(Instant::now()).unwrap().is_some());
        assert_eq!(device.read_failures, 0);
    }

    #[test]
    fn supervise_restarts_after_panics_at_most_three_times() {
        #[derive(Default)]
        struct Ctx {
            runs: u32,
            panics: u32,
            pauses: u32,
        }
        let mut ctx = Ctx::default();
        supervise(
            &mut ctx,
            |c| {
                c.runs += 1;
                panic!("test panic");
            },
            |c| c.panics += 1,
            |c| {
                c.pauses += 1;
                true
            },
        );
        assert_eq!((ctx.runs, ctx.panics, ctx.pauses), (4, 4, 3));

        let mut ctx = Ctx::default();
        supervise(
            &mut ctx,
            |c| {
                c.runs += 1;
                assert!(c.runs > 2, "test panic");
            },
            |c| c.panics += 1,
            |_| true,
        );
        assert_eq!((ctx.runs, ctx.panics), (3, 2));
    }

    #[test]
    fn failed_reads_while_the_mouse_is_off_are_retried_not_fatal() {
        let (mut device, handle) = mock_device();
        handle.borrow_mut().device_off = true;
        step(&mut device);
        assert!(device.detail().is_none());
        assert!(!device.take_dirty());

        // Not before the retry delay.
        let writes = handle.borrow().writes;
        step(&mut device);
        assert_eq!(handle.borrow().writes, writes);

        handle.borrow_mut().device_off = false;
        device.next_read = Instant::now();
        step(&mut device);
        assert!(device.detail().is_some());
    }

    #[test]
    fn a_broken_link_is_fatal() {
        let (mut device, handle) = mock_device();
        step(&mut device);
        handle.borrow_mut().broken = true;
        assert!(device.step(Instant::now()).is_err());
    }

    #[test]
    fn refresh_rereads_unless_the_mouse_is_off() {
        let (mut device, handle) = mock_device();
        step(&mut device);
        let detail = device.refresh(Instant::now()).unwrap().expect("detail");
        assert_eq!(detail.summary.battery_percent, Some(55));

        handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
        step(&mut device);
        assert_eq!(device.refresh(Instant::now()).unwrap(), None);
    }

    #[test]
    fn battery_is_requested_every_minute() {
        let (mut device, handle) = mock_device();
        step(&mut device);
        let writes = handle.borrow().writes;
        // Not due yet.
        step(&mut device);
        assert_eq!(handle.borrow().writes, writes);
        // Due: one 0x1004 fn1 request, reply `37 04` (55 %) from the log.
        let later = Instant::now() + BATTERY_REFRESH;
        device.step(later).expect("step");
        assert_eq!(handle.borrow().writes, writes + 1);
    }

    #[test]
    fn registry_lists_both_devices_and_finds_each_by_id() {
        let g502 = detail_for(Connection::Connected, Some(55));
        let mut headset = g502.clone();
        headset.summary.id = sony_wf_c710n::DEVICE_ID.to_owned();
        headset.summary.name = "WF-C710N".to_owned();
        headset.summary.kind = myhub_core::DeviceKind::Headset;
        headset.summary.battery_percent = Some(80);
        let mut state = ManagerState::default();
        // The headset is published first: the G502 still comes first in the list.
        *state.slot_mut(Slot::Sony) = Some(headset);
        assert_eq!(state.summaries().len(), 1);
        *state.slot_mut(Slot::Logitech) = Some(g502);
        let ids: Vec<_> = state.summaries().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, [DEVICE_ID, sony_wf_c710n::DEVICE_ID]);
        assert_eq!(
            state.find(sony_wf_c710n::DEVICE_ID).unwrap().summary.name,
            "WF-C710N"
        );
        assert_eq!(
            state.find(DEVICE_ID).unwrap().summary.battery_percent,
            Some(55)
        );
        assert!(state.find("nope").is_none());
        // The tray gets both details.
        let (mouse, headset) = state.tray_details();
        assert_eq!(mouse.unwrap().summary.id, DEVICE_ID);
        assert_eq!(headset.unwrap().summary.id, sony_wf_c710n::DEVICE_ID);
        *state.slot_mut(Slot::Logitech) = None;
        assert!(state.tray_details().0.is_none());
    }

    #[test]
    fn tray_lines_for_mouse_and_headset() {
        use myhub_core::{HeadsetBattery, HeadsetCell, HeadsetView};
        let cell = |percent, charging| Some(HeadsetCell { percent, charging });
        let mut headset = detail_for(Connection::Connected, Some(100));
        headset.summary.name = "WF-C710N".to_owned();
        headset.headset = Some(HeadsetView {
            battery: Some(HeadsetBattery {
                left: cell(100, CellCharging::NotCharging),
                right: cell(100, CellCharging::Charging),
                case: cell(70, CellCharging::Charged),
            }),
            nc_asm: None,
            eq: None,
            multipoint: None,
            sidetone: None,
            codec: None,
            firmware: None,
        });
        let line = |d: &DeviceDetail, l| headset_status_text(Some(d), l).unwrap();
        assert_eq!(
            line(&headset, Language::Pl),
            "WF-C710N: L 100% · P 100% (ładowanie) · etui 70%"
        );
        assert_eq!(
            line(&headset, Language::En),
            "WF-C710N: L 100% · R 100% (charging) · case 70%"
        );
        if let Some(b) = headset.headset.as_mut().and_then(|h| h.battery.as_mut()) {
            b.right = None;
        }
        assert_eq!(line(&headset, Language::En), "WF-C710N: L 100% · case 70%");
        let mut off = headset.clone();
        off.summary.connection = Connection::Disconnected;
        assert_eq!(line(&off, Language::Pl), "WF-C710N: rozłączone");
        assert_eq!(line(&off, Language::En), "WF-C710N: disconnected");

        let mouse = detail_for(Connection::Connected, Some(55));
        let en = Language::En;
        let (first, second) = tray_lines(Some(&mouse), Some(&headset), en);
        assert_eq!(first.as_deref(), Some("G502 X: 55%"));
        assert_eq!(second.as_deref(), Some("WF-C710N: L 100% · case 70%"));
        let (first, second) = tray_lines(None, Some(&headset), en);
        assert_eq!(first.as_deref(), Some("WF-C710N: L 100% · case 70%"));
        assert_eq!(second, None);
        assert_eq!(tray_lines(Some(&mouse), None, en).1, None);
        assert_eq!(tray_lines(None, None, en), (None, None));
    }

    #[test]
    fn refresh_goes_to_the_worker_that_owns_the_id() {
        let (logitech, logitech_rx) = mpsc::channel();
        let (sony, sony_rx) = mpsc::channel();
        let router = Router { logitech, sony };
        assert!(router.refresh(sony_wf_c710n::DEVICE_ID).is_some());
        assert!(matches!(
            sony_rx.try_recv(),
            Ok(SonyCommand::Refresh { .. })
        ));
        assert!(logitech_rx.try_recv().is_err());
        assert!(router.refresh(DEVICE_ID).is_some());
        assert!(matches!(
            logitech_rx.try_recv(),
            Ok(Command::Refresh { id, .. }) if id == DEVICE_ID
        ));
        assert!(sony_rx.try_recv().is_err());
        // Write commands for the read only headset never reach a worker.
        let refusal = refused_reply::<DeviceDetail>("writesDisabled");
        assert_eq!(refusal.try_recv().unwrap().unwrap_err(), "writesDisabled");
    }

    fn detail_for(connection: Connection, percent: Option<u8>) -> DeviceDetail {
        let (mut device, _handle) = mock_device();
        step(&mut device);
        let mut detail = device.detail().unwrap();
        detail.summary.connection = connection;
        detail.summary.battery_percent = percent;
        detail
    }

    #[test]
    fn status_text_per_state_and_language() {
        let on = detail_for(Connection::Connected, Some(55));
        assert_eq!(
            status_text(Some(&on), Language::Pl).as_deref(),
            Some("G502 X: 55%")
        );
        assert_eq!(
            status_text(Some(&on), Language::En).as_deref(),
            Some("G502 X: 55%")
        );
        let off = detail_for(Connection::Disconnected, Some(55));
        assert_eq!(
            status_text(Some(&off), Language::Pl).as_deref(),
            Some("G502 X: uśpiona")
        );
        assert_eq!(
            status_text(Some(&off), Language::En).as_deref(),
            Some("G502 X: asleep")
        );
        let bare = detail_for(Connection::Connected, None);
        assert_eq!(
            status_text(Some(&bare), Language::En).as_deref(),
            Some("G502 X")
        );
        assert_eq!(status_text(None, Language::En), None);
    }

    #[test]
    fn changes_ignore_the_timestamp_and_separate_summary_from_detail() {
        let base = detail_for(Connection::Connected, Some(55));
        assert_eq!(
            changes(None, None),
            Changes {
                summaries: false,
                detail: false
            }
        );
        assert_eq!(
            changes(None, Some(&base)),
            Changes {
                summaries: true,
                detail: true
            }
        );
        assert_eq!(
            changes(Some(&base), None),
            Changes {
                summaries: true,
                detail: true
            }
        );

        let mut later = base.clone();
        later.updated_at_ms += 1000;
        assert_eq!(
            changes(Some(&base), Some(&later)),
            Changes {
                summaries: false,
                detail: false
            }
        );

        let mut dpi = base.clone();
        dpi.dpi.as_mut().unwrap().current = 800;
        assert_eq!(
            changes(Some(&base), Some(&dpi)),
            Changes {
                summaries: false,
                detail: true
            }
        );

        let mut battery = base.clone();
        battery.summary.battery_percent = Some(54);
        assert_eq!(
            changes(Some(&base), Some(&battery)),
            Changes {
                summaries: true,
                detail: true
            }
        );
    }

    // ---- volatile writes

    const ID: &str = "logitech-g502x";
    const IDX_DPI: u8 = 7;
    const IDX_RATE: u8 = 11;

    fn dpi_set_tx(dpi: u16) -> Vec<u8> {
        let [hi, lo] = dpi.to_be_bytes();
        long(&[0x11, 0x01, IDX_DPI, 0x3b, 0x00, hi, lo, 0x00])
    }

    fn dpi_get_tx() -> Vec<u8> {
        long(&[0x11, 0x01, IDX_DPI, 0x2b, 0x00])
    }

    fn dpi_get_reply(dpi: u16) -> Vec<u8> {
        let [hi, lo] = dpi.to_be_bytes();
        long(&[0x11, 0x01, IDX_DPI, 0x2b, 0x00, hi, lo, 0x06, 0x40])
    }

    fn rate_set_tx(ms: u8) -> Vec<u8> {
        long(&[0x11, 0x01, IDX_RATE, 0x2b, ms])
    }

    fn rate_get_tx() -> Vec<u8> {
        long(&[0x11, 0x01, IDX_RATE, 0x1b])
    }

    /// The mock device accepts a SetSensorDPI and then reports `read_back`.
    fn accept_dpi(handle: &Handle, dpi: u16, read_back: u16) {
        let mut mock = handle.borrow_mut();
        mock.replies.insert(dpi_set_tx(dpi), dpi_set_tx(dpi));
        mock.replies.insert(dpi_get_tx(), dpi_get_reply(read_back));
    }

    fn accept_rate(handle: &Handle, ms: u8, read_back_ms: u8) {
        let mut mock = handle.borrow_mut();
        mock.replies.insert(rate_set_tx(ms), rate_set_tx(ms));
        mock.replies.insert(
            rate_get_tx(),
            long(&[0x11, 0x01, IDX_RATE, 0x1b, read_back_ms]),
        );
    }

    fn read_device() -> (Device<MockLink>, Handle) {
        let (mut device, handle) = mock_device();
        step(&mut device);
        device.take_dirty();
        (device, handle)
    }

    fn refused(result: Result<(), WriteFailure>) -> String {
        match result {
            Err(WriteFailure::Refused(code)) => code,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn writes_are_refused_when_disabled_without_touching_the_device() {
        let (mut device, handle) = read_device();
        let writes = handle.borrow().writes;
        let code = refused(device.write(ID, WriteOp::Dpi(1200), false));
        assert_eq!(code, "writesDisabled");
        let code = refused(device.write(ID, WriteOp::ReportRate(500), false));
        assert_eq!(code, "writesDisabled");
        assert_eq!(handle.borrow().writes, writes);
    }

    #[test]
    fn set_dpi_updates_the_detail_and_marks_it_dirty() {
        let (mut device, handle) = read_device();
        device.set_host_by_us(true); // temporary mode is active
        assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1600);
        accept_dpi(&handle, 1200, 1200);
        let writes = handle.borrow().writes;
        device.write(ID, WriteOp::Dpi(1200), true).expect("write");
        // mode read, list read, set, read back
        assert_eq!(handle.borrow().writes, writes + 4);
        assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1200);
        assert!(device.take_dirty());
    }

    #[test]
    fn set_report_rate_updates_the_detail() {
        let (mut device, handle) = read_device();
        device.set_host_by_us(true); // temporary mode is active
        assert_eq!(
            device.detail().unwrap().report_rate.unwrap().current_hz,
            1000
        );
        accept_rate(&handle, 2, 2);
        device
            .write(ID, WriteOp::ReportRate(500), true)
            .expect("write");
        let detail = device.detail().unwrap();
        assert_eq!(detail.report_rate.unwrap().current_hz, 500);
        assert!(device.take_dirty());
    }

    #[test]
    fn write_errors_map_to_contract_codes() {
        let (mut device, handle) = read_device();
        device.set_host_by_us(true); // temporary mode is active

        // outOfRange: only the two reads per call go out.
        let writes = handle.borrow().writes;
        assert_eq!(
            refused(device.write(ID, WriteOp::Dpi(1201), true)),
            "outOfRange"
        );
        assert_eq!(
            refused(device.write(ID, WriteOp::ReportRate(333), true)),
            "outOfRange"
        );
        assert_eq!(handle.borrow().writes, writes + 4);
        assert!(!device.take_dirty());

        // verifyFailed: the snapshot takes what the device reports.
        accept_dpi(&handle, 1200, 800);
        assert_eq!(
            refused(device.write(ID, WriteOp::Dpi(1200), true)),
            "verifyFailed"
        );
        assert_eq!(device.detail().unwrap().dpi.unwrap().current, 800);
        assert!(device.take_dirty());

        // deviceError:<code> from the set frame (HID++ 2.0 error 2).
        handle.borrow_mut().replies.insert(
            dpi_set_tx(1300),
            long(&[0x11, 0x01, 0xff, IDX_DPI, 0x3b, 0x02]),
        );
        assert_eq!(
            refused(device.write(ID, WriteOp::Dpi(1300), true)),
            "deviceError:2"
        );

        // notHostMode: 0x8100 fn2 reports a mode that is neither 1 nor 2.
        handle
            .borrow_mut()
            .replies
            .insert(hex(FN2.0), hex("1101092b03000000000000000000000000000000"));
        let writes = handle.borrow().writes;
        assert_eq!(
            refused(device.write(ID, WriteOp::Dpi(1400), true)),
            "notHostMode"
        );
        assert_eq!(handle.borrow().writes, writes + 1, "only the mode read");
    }

    #[test]
    fn writes_to_an_unknown_or_sleeping_device_say_disconnected() {
        let (mut device, handle) = read_device();
        let writes = handle.borrow().writes;
        assert_eq!(
            refused(device.write("other-device", WriteOp::Dpi(1200), true)),
            "disconnected"
        );
        // Switched off: `11 62 9f 40`.
        handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
        step(&mut device);
        assert_eq!(
            refused(device.write(ID, WriteOp::Dpi(1200), true)),
            "disconnected"
        );
        assert_eq!(handle.borrow().writes, writes, "wrote to a sleeping device");
    }

    #[test]
    fn a_link_error_during_a_write_is_a_lost_receiver() {
        let (mut device, handle) = read_device();
        device.set_host_by_us(true); // temporary mode is active
        handle.borrow_mut().broken = true;
        assert!(matches!(
            device.write(ID, WriteOp::Dpi(1200), true),
            Err(WriteFailure::Lost(_))
        ));
    }

    #[test]
    fn device_off_during_a_write_says_disconnected() {
        let (mut device, handle) = read_device();
        device.set_host_by_us(true); // temporary mode is active
        handle.borrow_mut().device_off = true;
        assert_eq!(
            refused(device.write(ID, WriteOp::Dpi(1200), true)),
            "disconnected"
        );
    }

    #[test]
    fn writes_keep_the_rate_limit() {
        let (mut device, handle) = read_device();
        device.set_host_by_us(true); // temporary mode is active
        assert_eq!(device.write_delay(Instant::now()), Duration::ZERO);
        accept_dpi(&handle, 1200, 1200);
        device.write(ID, WriteOp::Dpi(1200), true).unwrap();
        let last = device.last_write.expect("recorded");
        assert_eq!(device.write_delay(last), WRITE_INTERVAL);
        assert_eq!(
            device.write_delay(last + Duration::from_millis(100)),
            Duration::from_millis(150)
        );
        assert_eq!(device.write_delay(last + WRITE_INTERVAL), Duration::ZERO);
        // A second write really waits.
        accept_dpi(&handle, 1300, 1300);
        let started = Instant::now();
        device.write(ID, WriteOp::Dpi(1300), true).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn restore_is_due_once_per_connect_event() {
        let (mut device, handle) = mock_device();
        assert!(!device.take_restore_due());
        step(&mut device);
        assert!(device.take_restore_due(), "first read is a connect");
        assert!(!device.take_restore_due(), "one attempt only");

        // Steps without a connect event do not re-arm it.
        step(&mut device);
        step(&mut device);
        assert!(!device.take_restore_due());

        // A user refresh of a healthy device is not a connect event.
        device.refresh(Instant::now()).unwrap();
        assert!(!device.take_restore_due());

        // Off and on again: the re-read is.
        handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
        step(&mut device);
        handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
        step(&mut device);
        step(&mut device);
        assert!(device.take_restore_due());
        assert!(!device.take_restore_due());
    }

    #[test]
    fn write_error_codes_cover_the_session_errors() {
        use myhub_transport::LinkError;
        let code = |e: WriteError| match classify_write_error(e) {
            WriteFailure::Refused(code) => code,
            WriteFailure::Lost(_) => "lost".to_owned(),
        };
        assert_eq!(
            code(WriteError::Session(SessionError::Timeout)),
            "disconnected"
        );
        assert_eq!(
            code(WriteError::Session(SessionError::LinkDown)),
            "disconnected"
        );
        assert_eq!(
            code(WriteError::Session(SessionError::Device {
                code: 8,
                family: ErrorFamily::Hidpp10,
                name: None,
            })),
            "disconnected"
        );
        assert_eq!(
            code(WriteError::Session(SessionError::Device {
                code: 8,
                family: ErrorFamily::Hidpp20,
                name: Some("Busy"),
            })),
            "deviceError:8"
        );
        assert_eq!(
            code(WriteError::Session(SessionError::Link(LinkError::Hid(
                "x".into()
            )))),
            "lost"
        );
    }

    // ---- fix round 1

    fn settings_with(allow: bool, restore: bool) -> Settings {
        Settings {
            allow_device_writes: allow,
            restore_last_mode: restore,
            ..Settings::default()
        }
    }

    #[test]
    fn an_expired_write_is_refused_without_sending_anything() {
        let (mut device, handle) = read_device();
        device.set_host_by_us(true); // temporary mode is active
        accept_dpi(&handle, 1200, 1200);
        let writes = handle.borrow().writes;
        let expired = Instant::now();
        let code = refused(run_user_write(
            &mut device,
            ID,
            WriteOp::Dpi(1200),
            expired,
            true,
        ));
        assert_eq!(code, "deviceError:timeout");
        assert_eq!(handle.borrow().writes, writes);
        // The same write with time left goes through.
        run_user_write(
            &mut device,
            ID,
            WriteOp::Dpi(1200),
            Instant::now() + Duration::from_secs(10),
            true,
        )
        .expect("write");
        assert_eq!(handle.borrow().writes, writes + 4);
    }

    #[test]
    fn a_timeout_after_the_set_frame_schedules_a_resync_that_is_not_a_connect() {
        let (mut device, handle) = read_device();
        device.set_host_by_us(true); // temporary mode is active
        {
            let mut mock = handle.borrow_mut();
            mock.silent.push(dpi_set_tx(1200));
            mock.replies.insert(dpi_set_tx(1200), dpi_set_tx(1200));
        }
        assert_eq!(
            refused(device.write(ID, WriteOp::Dpi(1200), true)),
            "disconnected"
        );
        assert!(device.read_pending && device.resync);
        device.take_restore_due(); // the initial connect, consumed
        step(&mut device);
        assert!(!device.read_pending, "the resync read ran");
        assert!(device.take_dirty());
        assert!(!device.take_restore_due(), "a resync is no connect event");
    }

    // ---- lazy host switch and return to onboard

    const FN2_TX: &str = "1101092b00000000000000000000000000000000";

    fn mode_reply(byte: u8) -> Vec<u8> {
        long(&[0x11, 0x01, IDX_ONBOARD, 0x2b, byte])
    }

    fn mode_set_tx(byte: u8) -> Vec<u8> {
        long(&[0x11, 0x01, IDX_ONBOARD, 0x1b, byte])
    }

    /// The mouse is onboard (01); a SetOnboardMode(host) is accepted and
    /// reads back as host, a SetOnboardMode(onboard) reads back as onboard.
    fn onboard_mouse(handle: &Handle) {
        let mut mock = handle.borrow_mut();
        mock.replies.insert(hex(FN2_TX), mode_reply(1));
        mock.replies.insert(mode_set_tx(2), mode_set_tx(2));
        mock.replies.insert(mode_set_tx(1), mode_set_tx(1));
        mock.then_reply
            .insert(mode_set_tx(2), vec![(hex(FN2_TX), mode_reply(2))]);
        mock.then_reply
            .insert(mode_set_tx(1), vec![(hex(FN2_TX), mode_reply(1))]);
    }

    fn hex_frames(handle: &Handle, from: usize) -> Vec<String> {
        handle.borrow().sent[from..]
            .iter()
            .map(|f| f.iter().map(|b| format!("{b:02x}")).collect())
            .collect()
    }

    fn mode_of(device: &Device<MockLink>) -> Option<OnboardMode> {
        device.detail().unwrap().onboard.unwrap().mode
    }

    #[test]
    fn enter_then_set_dpi_then_leave_sends_these_frames() {
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        accept_dpi(&handle, 1200, 1200);
        // The snapshot was read while the mock said host; the mouse is onboard.
        let from = handle.borrow().sent.len();
        device.enter_temporary(ID, true, None).expect("enter");
        assert_eq!(
            hex_frames(&handle, from),
            [
                // 0x8100 fn2: the mouse is onboard (01)
                "1101092b00000000000000000000000000000000",
                // 0x8100 fn1 SetOnboardMode: 02 = host
                "1101091b02000000000000000000000000000000",
                // 0x8100 fn2 read back: host
                "1101092b00000000000000000000000000000000",
                // 0x8110 fn1: button spy start (the DPI buttons)
                "11010a1b00000000000000000000000000000000",
            ]
        );
        assert!(device.host_by_us());
        assert!(device.spy_active());
        assert_eq!(mode_of(&device), Some(OnboardMode::Host));

        let from = handle.borrow().sent.len();
        device.write(ID, WriteOp::Dpi(1200), true).expect("write");
        assert_eq!(
            hex_frames(&handle, from),
            [
                "1101092b00000000000000000000000000000000",
                "1101071b00000000000000000000000000000000",
                "1101073b0004b000000000000000000000000000",
                "1101072b00000000000000000000000000000000",
            ]
        );
        assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1200);

        let from = handle.borrow().sent.len();
        device.leave_temporary(ID, true, None).expect("leave");
        assert_eq!(
            hex_frames(&handle, from),
            [
                // 0x8110 fn2: button spy stop, before the switch back
                "11010a2b00000000000000000000000000000000",
                // 0x8100 fn1 SetOnboardMode: 01 = onboard
                "1101091b01000000000000000000000000000000",
                "1101092b00000000000000000000000000000000",
            ]
        );
        assert!(!device.host_by_us());
        assert!(!device.spy_active());
        assert_eq!(mode_of(&device), Some(OnboardMode::Onboard));
    }

    #[test]
    fn set_dpi_and_rate_without_the_mark_say_not_host_mode_and_send_nothing() {
        // The mouse is onboard.
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        let from = handle.borrow().sent.len();
        assert_eq!(
            refused(device.write(ID, WriteOp::Dpi(1200), true)),
            "notHostMode"
        );
        assert_eq!(
            refused(device.write(ID, WriteOp::ReportRate(500), true)),
            "notHostMode"
        );
        assert!(hex_frames(&handle, from).is_empty());
        assert!(!device.host_by_us());

        // In host mode that the app did not enter (no mark): the same.
        let (mut device, handle) = read_device();
        let from = handle.borrow().sent.len();
        assert_eq!(
            refused(device.write(ID, WriteOp::Dpi(1200), true)),
            "notHostMode"
        );
        assert!(hex_frames(&handle, from).is_empty());
    }

    #[test]
    fn leave_is_a_no_op_success_when_the_mouse_already_reads_onboard() {
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        let from = handle.borrow().sent.len();
        device.leave_temporary(ID, true, None).expect("no-op");
        // A spy stop (crash recovery), then the mode read; nothing is switched.
        assert_eq!(
            hex_frames(&handle, from),
            [
                "11010a2b00000000000000000000000000000000",
                "1101092b00000000000000000000000000000000"
            ]
        );
        assert!(!device.host_by_us());
        // An unknown device is not a no-op.
        assert_eq!(
            refused(device.leave_temporary("other", true, None)),
            "disconnected"
        );
    }

    #[test]
    fn leave_returns_a_host_mouse_even_when_the_app_did_not_switch_it() {
        // After a crash or an inherited host state: no mark, fn2 reads host.
        let (mut device, handle) = read_device();
        {
            let mut mock = handle.borrow_mut();
            mock.replies.insert(mode_set_tx(1), mode_set_tx(1));
            mock.then_reply
                .insert(mode_set_tx(1), vec![(hex(FN2_TX), mode_reply(1))]);
        }
        assert!(!device.host_by_us());
        let from = handle.borrow().sent.len();
        device.leave_temporary(ID, true, None).expect("leave");
        assert_eq!(
            hex_frames(&handle, from),
            [
                // Without the mark the spy stop comes first, before the mode read
                // (crash recovery); it is not sent a second time.
                "11010a2b00000000000000000000000000000000",
                "1101092b00000000000000000000000000000000",
                "1101091b01000000000000000000000000000000",
                "1101092b00000000000000000000000000000000",
            ]
        );
        assert!(!device.host_by_us());
        assert_eq!(mode_of(&device), Some(OnboardMode::Onboard));

        // A return that does not verify is reported, not hidden.
        let (mut device, handle) = read_device();
        {
            let mut mock = handle.borrow_mut();
            mock.replies.insert(mode_set_tx(1), mode_set_tx(1));
            // fn2 keeps reading host after the ACKed fn1 (VerifyFailed: the
            // outcome is unknown, the mark stays).
        }
        let result = device.leave_temporary(ID, true, None);
        assert_eq!(refused(result), "disconnected");
        assert!(device.host_by_us());

        // Unreachable: nothing is sent.
        let (mut device, handle) = read_device();
        handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
        step(&mut device);
        let from = handle.borrow().sent.len();
        assert_eq!(
            refused(device.leave_temporary(ID, true, None)),
            "disconnected"
        );
        assert!(hex_frames(&handle, from).is_empty());
    }

    #[test]
    fn enter_refuses_like_a_write_without_sending() {
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        let from = handle.borrow().sent.len();
        assert_eq!(
            refused(device.enter_temporary(ID, false, None)),
            "writesDisabled"
        );
        assert_eq!(
            refused(device.enter_temporary("other", true, None)),
            "disconnected"
        );
        assert_eq!(
            refused(device.enter_temporary(ID, true, Some(Instant::now()))),
            "deviceError:timeout"
        );
        assert!(hex_frames(&handle, from).is_empty());
        assert!(!device.host_by_us());
    }

    #[test]
    fn a_failed_switch_refuses_with_not_host_mode_and_sends_no_set_frame() {
        // The switch frame gets a device error.
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        handle.borrow_mut().replies.insert(
            mode_set_tx(2),
            long(&[0x11, 0x01, 0xff, IDX_ONBOARD, 0x1b, 0x02]),
        );
        let from = handle.borrow().sent.len();
        assert_eq!(
            refused(device.enter_temporary(ID, true, None)),
            "notHostMode"
        );
        assert!(!device.host_by_us());
        let sent = hex_frames(&handle, from);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert!(
            !sent
                .iter()
                .any(|f| f.starts_with("110107") || f.starts_with("11010b"))
        );

        // The device accepts the frame but stays onboard: verification fails.
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        handle.borrow_mut().then_reply.clear();
        let from = handle.borrow().sent.len();
        assert_eq!(
            refused(device.enter_temporary(ID, true, None)),
            "notHostMode"
        );
        // An ACKed fn1 followed by a failing read-back keeps the mark: the
        // switch may have happened (a mode 1 read clears it).
        assert!(device.host_by_us());
        let sent = hex_frames(&handle, from);
        assert_eq!(sent.len(), 3, "{sent:?}");
        assert!(!sent.iter().any(|f| f.starts_with("11010b")));
    }

    #[test]
    fn entering_on_a_mouse_already_in_host_marks_it_without_a_switch_frame() {
        let (mut device, handle) = read_device();
        // Default mock reply: host (02).
        accept_dpi(&handle, 1200, 1200);
        {
            let mut mock = handle.borrow_mut();
            mock.replies.insert(mode_set_tx(1), mode_set_tx(1));
            mock.then_reply
                .insert(mode_set_tx(1), vec![(hex(FN2_TX), mode_reply(1))]);
        }
        let from = handle.borrow().sent.len();
        device.enter_temporary(ID, true, None).expect("enter");
        let sent = hex_frames(&handle, from);
        assert_eq!(
            sent,
            [
                "1101092b00000000000000000000000000000000",
                "11010a1b00000000000000000000000000000000"
            ]
        );
        assert!(device.host_by_us(), "the user asked for temporary mode");
        // Now set_dpi works ...
        device.write(ID, WriteOp::Dpi(1200), true).expect("write");
        // ... and leaving (or quitting) returns the mouse.
        device.return_to_onboard().unwrap();
        assert!(!device.host_by_us());
        assert_eq!(mode_of(&device), Some(OnboardMode::Onboard));
    }

    #[test]
    fn return_to_onboard_runs_once_and_only_after_our_switch() {
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        accept_dpi(&handle, 1200, 1200);
        device.enter_temporary(ID, true, None).expect("enter");
        assert!(device.host_by_us());

        let from = handle.borrow().sent.len();
        device.return_to_onboard().unwrap();
        assert_eq!(
            hex_frames(&handle, from),
            [
                // 0x8110 fn2: the button spy stops first
                "11010a2b00000000000000000000000000000000",
                // 0x8100 fn1 SetOnboardMode: 01 = onboard
                "1101091b01000000000000000000000000000000",
                // 0x8100 fn2 read back
                "1101092b00000000000000000000000000000000",
            ]
        );
        assert!(!device.host_by_us());
        assert_eq!(mode_of(&device), Some(OnboardMode::Onboard));
        assert!(device.take_dirty());

        // One attempt: nothing more is sent.
        let writes = handle.borrow().writes;
        device.return_to_onboard().unwrap();
        assert_eq!(handle.borrow().writes, writes);
    }

    #[test]
    fn a_failed_return_is_not_retried() {
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        accept_dpi(&handle, 1200, 1200);
        device.enter_temporary(ID, true, None).expect("enter");
        handle.borrow_mut().replies.insert(
            mode_set_tx(1),
            long(&[0x11, 0x01, 0xff, IDX_ONBOARD, 0x1b, 0x08]),
        );
        device.return_to_onboard().unwrap();
        assert!(!device.host_by_us());
        let writes = handle.borrow().writes;
        device.return_to_onboard().unwrap();
        assert_eq!(handle.borrow().writes, writes);
    }

    #[test]
    fn a_reconnect_that_shows_onboard_again_forgets_our_switch() {
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        accept_dpi(&handle, 1200, 1200);
        device.enter_temporary(ID, true, None).expect("enter");
        assert!(device.host_by_us());

        // The mouse resets itself: back to onboard after switching off and on.
        handle
            .borrow_mut()
            .replies
            .insert(hex(FN2_TX), mode_reply(1));
        handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
        step(&mut device);
        handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
        step(&mut device);
        step(&mut device);
        assert!(!device.host_by_us(), "the flag survived a reset");
        assert_eq!(mode_of(&device), Some(OnboardMode::Onboard));

        // Nothing is sent to "return" a mouse that is already onboard.
        let writes = handle.borrow().writes;
        device.return_to_onboard().unwrap();
        assert_eq!(handle.borrow().writes, writes);
    }

    // ---- fix round after the supervised test

    fn quick(secs: u64) -> Instant {
        Instant::now() + Duration::from_secs(secs)
    }

    #[test]
    fn a_lost_switch_reply_keeps_the_flag_and_schedules_a_resync() {
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        handle.borrow_mut().silent.push(mode_set_tx(2));
        assert_eq!(
            refused(device.enter_temporary(ID, true, None)),
            "disconnected"
        );
        assert!(device.host_by_us(), "the switch may have happened");
        assert!(device.read_pending && device.resync);
    }

    #[test]
    fn the_host_mark_outlives_the_device_and_a_onboard_read_clears_it() {
        let mark = Arc::new(AtomicBool::new(false));
        let (device, handle) = read_device();
        let mut device = device.with_host_mark(Arc::clone(&mark));
        onboard_mouse(&handle);
        accept_dpi(&handle, 1200, 1200);
        device.enter_temporary(ID, true, None).expect("enter");
        assert!(mark.load(Ordering::SeqCst));
        drop(device); // link loss or restart

        // A new device over the same mouse gets the mark and, while the mouse
        // is still in host mode, keeps it.
        handle
            .borrow_mut()
            .replies
            .insert(hex(FN2_TX), mode_reply(2));
        let mut session = HidppSession::new(MockLink(Rc::clone(&handle)));
        session.set_reply_timeout(Duration::from_millis(50));
        let mut next = Device::new(session, Instant::now()).with_host_mark(Arc::clone(&mark));
        next.poll_timeout = Duration::from_millis(5);
        step(&mut next);
        assert!(next.host_by_us());

        // The mouse reset itself: a mode 1 read clears the mark.
        handle
            .borrow_mut()
            .replies
            .insert(hex(FN2_TX), mode_reply(1));
        next.refresh(Instant::now()).unwrap();
        assert!(!mark.load(Ordering::SeqCst));
    }

    #[test]
    fn an_unreachable_mouse_keeps_the_flag_until_a_later_read_returns_it() {
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        accept_dpi(&handle, 1200, 1200);
        device.enter_temporary(ID, true, None).expect("enter");
        assert!(device.host_by_us());

        // Switched off: the return cannot happen, the flag stays.
        handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
        step(&mut device);
        let writes = handle.borrow().writes;
        device.return_to_onboard().unwrap();
        assert_eq!(handle.borrow().writes, writes);
        assert!(device.host_by_us());

        // Back on, still host mode, writes are off: the read triggers the return.
        handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
        step(&mut device);
        step(&mut device);
        let off = settings_with(false, false);
        let from = handle.borrow().sent.len();
        return_if_writes_off(&mut device, &off).unwrap();
        let sent = hex_frames(&handle, from);
        assert_eq!(sent.len(), 3, "{sent:?}");
        assert!(sent[0].starts_with("11010a2b"), "spy stop first: {sent:?}");
        assert!(sent[1].starts_with("1101091b01"), "{sent:?}");
        assert!(!device.host_by_us());
        assert_eq!(mode_of(&device), Some(OnboardMode::Onboard));
    }

    #[test]
    fn a_fresh_read_returns_the_mode_only_when_writes_are_off_and_it_is_ours() {
        // Writes on: nothing happens.
        let (mut device, handle) = read_device();
        onboard_mouse(&handle);
        accept_dpi(&handle, 1200, 1200);
        device.enter_temporary(ID, true, None).expect("enter");
        device.refresh(Instant::now()).unwrap();
        let writes = handle.borrow().writes;
        return_if_writes_off(&mut device, &settings_with(true, false)).unwrap();
        assert_eq!(handle.borrow().writes, writes);
        assert!(device.host_by_us());

        // No fresh read since: nothing happens even with writes off.
        return_if_writes_off(&mut device, &settings_with(false, false)).unwrap();
        assert_eq!(handle.borrow().writes, writes);

        // Not ours: a host-mode mouse is left alone.
        let (mut device, handle) = read_device();
        device.refresh(Instant::now()).unwrap();
        let writes = handle.borrow().writes;
        return_if_writes_off(&mut device, &settings_with(false, false)).unwrap();
        assert_eq!(handle.borrow().writes, writes);
    }

    #[test]
    fn the_deadline_is_checked_again_after_the_rate_limit_wait() {
        let (mut device, handle) = read_device();
        device.set_host_by_us(true); // temporary mode is active
        accept_dpi(&handle, 1200, 1200);
        device.last_write = Some(Instant::now());
        let writes = handle.borrow().writes;
        // Alive when the write starts, expired when the 250 ms wait is over.
        let deadline = Instant::now() + Duration::from_millis(100);
        let result = device.write_until(ID, WriteOp::Dpi(1200), true, Some(deadline));
        assert_eq!(refused(result), "deviceError:timeout");
        assert_eq!(handle.borrow().writes, writes, "something was sent");
    }

    #[test]
    fn no_battery_request_while_shutting_down() {
        let (mut device, handle) = read_device();
        let flag = Arc::new(AtomicBool::new(true));
        device = device.with_shutdown(Arc::clone(&flag));
        let writes = handle.borrow().writes;
        device.step(quick(0) + BATTERY_REFRESH).expect("step");
        assert_eq!(handle.borrow().writes, writes);
        flag.store(false, Ordering::SeqCst);
        device.step(quick(0) + BATTERY_REFRESH).expect("step");
        assert_eq!(handle.borrow().writes, writes + 1);
    }
}
