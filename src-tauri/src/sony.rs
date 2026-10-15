//! Headset worker: one background thread ("sony-worker") keeps the Sony WF-C710N
//! detail up to date over Bluetooth RFCOMM and publishes it next to the G502.
//!
//! Read tier only. The worker finds the headset among the paired devices, connects,
//! reads the snapshot, then polls the session: notifications (battery, NC/ASM, EQ,
//! multipoint, sidetone) are folded into the snapshot and republished, and the
//! battery is read again every minute. When the link drops the detail stays
//! (marked disconnected) and the worker reconnects with a back off of 2 s, 5 s,
//! then every 10 s. Nothing is ever written to the headset, and no Bluetooth
//! address is logged: this module never prints one.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use myhub_core::{Connection, DeviceDetail};
use myhub_rfcomm::{RfcommError, RfcommLink};
use sony_wf_c710n::{
    Applied, Item, Session, SessionError, Snapshot, candidates, open, read_snapshot, reread,
    to_detail,
};

use crate::device_manager::{ManagerState, Slot, clock_ms, lock, publish_slot};

/// Pause between scans while no headset is paired or the first connect fails.
const SCAN_INTERVAL: Duration = Duration::from_secs(10);
/// Pauses after a lost link: first 2 s, then 5 s, then [`SCAN_INTERVAL`].
const RECONNECT_DELAYS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    SCAN_INTERVAL,
];
/// One transport read of the serving loop; also the latency of a refresh command
/// and of shutdown.
const POLL_MS: u32 = 200;
/// Interval of the battery request while connected.
const BATTERY_REFRESH: Duration = Duration::from_secs(60);
/// Consecutive failed connects after which the retry interval grows to [`LONG_SCAN_INTERVAL`].
const FAILS_BEFORE_LONG: u32 = 3;
/// Retry interval while the headset stays unreachable (buds in the case).
const LONG_SCAN_INTERVAL: Duration = Duration::from_secs(60);

/// What the headset worker accepts.
pub enum SonyCommand {
    /// Read the state again and answer with the new detail (`None` while the
    /// headset is not connected).
    Refresh { reply: Sender<Option<DeviceDetail>> },
    /// Wakes the worker (shutdown).
    Wake,
}

/// Why no session could be opened.
enum ConnectError {
    /// This platform has no RFCOMM backend: the worker stops.
    Unsupported,
    /// Anything else; the text is logged once per distinct message.
    Failed(String),
}

enum Outcome {
    Stopped,
    Lost(String),
}

pub struct SonyWorker {
    pub app: tauri::AppHandle,
    pub state: Arc<Mutex<ManagerState>>,
    pub commands: Receiver<SonyCommand>,
    pub shutdown: Arc<AtomicBool>,
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
            eprintln!("myhub: the headset worker panicked");
            // Nobody serves the headset any more: show it as disconnected.
            let cached = lock(&self.state).find(sony_wf_c710n::DEVICE_ID);
            if let Some(mut detail) = cached {
                detail.summary.connection = Connection::Disconnected;
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
        // Index into RECONNECT_DELAYS: starts at the slow scan, restarts after a drop.
        let mut attempt = RECONNECT_DELAYS.len() - 1;
        let mut last_error: Option<String> = None;
        let mut fails = 0u32;
        while !self.stopping() {
            match self.connect() {
                Ok((mut session, mut snapshot)) => {
                    last_error = None;
                    eprintln!("myhub: headset connected");
                    let mut stamp = clock_ms();
                    self.publish(stamped(&snapshot, true, stamp), true);
                    let outcome = self.serve(&mut session, &mut snapshot, &mut stamp);
                    // Closes the socket.
                    drop(session);
                    match outcome {
                        Outcome::Stopped => return,
                        Outcome::Lost(reason) => {
                            eprintln!("myhub: headset link lost: {reason}");
                            self.publish(stamped(&snapshot, false, stamp), false);
                            attempt = 0;
                            fails = 0;
                        }
                    }
                }
                Err(ConnectError::Unsupported) => {
                    eprintln!("myhub: headset support is not available on this platform");
                    return;
                }
                Err(ConnectError::Failed(message)) => {
                    fails += 1;
                    if last_error.as_deref() != Some(message.as_str()) {
                        eprintln!("myhub: headset not connected: {message}");
                        last_error = Some(message);
                    }
                }
            }
            let delay = if fails >= FAILS_BEFORE_LONG {
                LONG_SCAN_INTERVAL
            } else {
                RECONNECT_DELAYS[attempt]
            };
            // A refresh command for the headset ends the pause and the long back off.
            if self.wait(delay) {
                fails = 0;
                attempt = 0;
                continue;
            }
            attempt = (attempt + 1).min(RECONNECT_DELAYS.len() - 1);
        }
    }

    /// Finds the headset among the paired devices, connects and reads the snapshot.
    fn connect(&self) -> Result<(Session<RfcommLink>, Snapshot), ConnectError> {
        let found = match candidates() {
            Ok(found) => found,
            Err(RfcommError::Unsupported) => return Err(ConnectError::Unsupported),
            Err(e) => return Err(ConnectError::Failed(e.to_string())),
        };
        let Some((device, _)) = found.into_iter().next() else {
            return Err(ConnectError::Failed("no paired WF-C710N".to_owned()));
        };
        let mut session = open(&device).map_err(|e| match e {
            SessionError::Transport(RfcommError::Unsupported) => ConnectError::Unsupported,
            other => ConnectError::Failed(other.to_string()),
        })?;
        let snapshot =
            read_snapshot(&mut session).map_err(|e| ConnectError::Failed(e.to_string()))?;
        Ok((session, snapshot))
    }

    /// Polls the session until shutdown or a lost link, publishing every change.
    fn serve(
        &self,
        session: &mut Session<RfcommLink>,
        snapshot: &mut Snapshot,
        stamp: &mut u64,
    ) -> Outcome {
        let mut next_battery = Instant::now() + BATTERY_REFRESH;
        while !self.stopping() {
            let mut replies = Vec::new();
            while let Ok(command) = self.commands.try_recv() {
                if let SonyCommand::Refresh { reply } = command {
                    replies.push(reply);
                }
            }
            let mut dirty = false;
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
                    eprintln!("myhub: headset read failed: {e}");
                }
            }
            if dirty {
                *stamp = clock_ms();
                let detail = stamped(snapshot, true, *stamp);
                self.publish(detail.clone(), !replies.is_empty());
                answer_all(replies, Some(detail));
            }
        }
        Outcome::Stopped
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
                Ok(SonyCommand::Wake) | Err(RecvTimeoutError::Timeout) => {}
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

/// The detail with the read time stamped on it.
fn stamped(snapshot: &Snapshot, connected: bool, at_ms: u64) -> DeviceDetail {
    let mut detail = to_detail(snapshot, connected);
    detail.updated_at_ms = at_ms;
    detail
}
