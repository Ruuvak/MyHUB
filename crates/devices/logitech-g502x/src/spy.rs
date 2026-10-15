//! Button spy capture (0x8110 MOUSE_BUTTON_SPY): research tool for finding out
//! what the mouse sends in host mode, where the DPI buttons do nothing.
//!
//! [`spy_capture`] requires host mode (fn2), sends start (0x8110 fn1), prints
//! every notification it hears and ALWAYS sends stop (0x8110 fn2) afterwards:
//! at the end, on a stop request (Ctrl+C), and on an error after the start. It
//! sends nothing else. Both calls go through `HidppSession::request_button_spy`,
//! the only entry point for that feature. The spy is RAM-only; the remap table
//! (fn4) and fn3 are not reachable at all.

use std::time::{Duration, Instant};

use hidpp::{FeatureId, Message};
use myhub_core::OnboardMode;
use myhub_transport::{HidLink, HidppSession, SessionError, connection_notification, link_lost};
use thiserror::Error;

use crate::DEVICE_INDEX;
use crate::write::onboard_mode;

/// One printed line of a capture.
#[derive(Debug, Clone, PartialEq)]
pub struct SpyLine {
    /// Seconds since the start frame was sent.
    pub elapsed_s: f32,
    pub text: String,
}

/// What a capture saw.
#[derive(Debug, Clone, PartialEq)]
pub struct SpyReport {
    /// Notifications from the 0x8110 index.
    pub spy_events: u32,
    /// All other notifications and HID++ 1.0 messages.
    pub other_events: u32,
    /// Result of the stop frame (sent in every case after a started capture).
    pub stop: Result<(), SessionError>,
}

/// Why a capture did not run or ended early.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum SpyError {
    /// fn2 did not report host mode; nothing was sent but that read. Run
    /// `hw_probe host` first.
    #[error("the mouse is not in host mode (run `hw_probe host` first); nothing was started")]
    NotHostMode,
    /// Reading the mode failed before anything was started.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// The start frame or the listening failed. `stop` is the result of the stop
    /// frame that was attempted anyway (it may fail too, for example when the
    /// link is gone).
    #[error("spy capture failed: {cause} (stop: {stop:?})")]
    Failed {
        cause: SessionError,
        stop: Result<(), SessionError>,
    },
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn bits(value: u16) -> String {
    let set: Vec<String> = (0..16)
        .filter(|b| value & (1 << b) != 0)
        .map(|b| b.to_string())
        .collect();
    format!("[{}]", set.join(","))
}

/// Formats the payload of a 0x8110 notification: raw hex, the first two bytes
/// as a u16 big-endian and little-endian, and the set bits (0 = least
/// significant) of both readings. The bit order is unknown.
pub fn describe_spy_payload(function: u8, payload: &[u8; 16]) -> String {
    let be = u16::from_be_bytes([payload[0], payload[1]]);
    let le = u16::from_le_bytes([payload[0], payload[1]]);
    format!(
        "spy fn{function} raw={} BE={be:#06x} bitsBE={} LE={le:#06x} bitsLE={}",
        hex(payload),
        bits(be),
        bits(le)
    )
}

/// Runs a capture for `duration` or until `stop_requested` returns `true`,
/// calling `out` for every line. See the module docs for the guarantees.
pub fn spy_capture<L: HidLink>(
    session: &mut HidppSession<L>,
    duration: Duration,
    stop_requested: &dyn Fn() -> bool,
    out: &mut dyn FnMut(SpyLine),
) -> Result<SpyReport, SpyError> {
    if onboard_mode(session)? != Some(OnboardMode::Host) {
        return Err(SpyError::NotHostMode);
    }
    let spy_index = session.table().index_of(FeatureId::MOUSE_BUTTON_SPY);

    let started = Instant::now();
    // Start. Whatever happens next, the stop frame follows.
    let start = session.request_button_spy(true);
    let mut line = |text: String| {
        out(SpyLine {
            elapsed_s: started.elapsed().as_secs_f32(),
            text,
        });
    };
    let mut failure: Option<SessionError> = start.err();
    if failure.is_none() {
        line("spy started (0x8110 fn1)".into());
    }
    let mut spy_events = 0u32;
    let mut other_events = 0u32;

    while failure.is_none() && started.elapsed() < duration && !stop_requested() {
        let wait = duration
            .saturating_sub(started.elapsed())
            .min(Duration::from_millis(300));
        if let Err(e) = session.poll_events(wait) {
            failure = Some(e);
            break;
        }
        for message in session.drain_notifications() {
            match &message {
                Message::Notification {
                    feature_index,
                    function,
                    payload,
                    ..
                } if Some(*feature_index) == spy_index => {
                    spy_events += 1;
                    line(describe_spy_payload(*function, payload));
                }
                Message::Notification {
                    feature_index,
                    function,
                    payload,
                    ..
                } => {
                    other_events += 1;
                    line(format!(
                        "other index {feature_index} fn{function} raw={}",
                        hex(payload)
                    ));
                }
                other => {
                    other_events += 1;
                    if let Some((device, up)) = connection_notification(other) {
                        line(format!(
                            "link {} (0x41, device {device})",
                            if up { "up" } else { "down" }
                        ));
                        if device == DEVICE_INDEX && !up {
                            failure = Some(SessionError::NeedsRediscovery);
                        }
                    } else {
                        line(format!("other message {other:?}"));
                    }
                }
            }
        }
        if failure.is_none() && session.needs_rediscovery() {
            failure = Some(SessionError::NeedsRediscovery);
        }
    }

    // Always stop. With a lost link the session refuses it before a byte is sent.
    let stop = match session.request_button_spy(false) {
        Err(e) if !link_lost(&e) => {
            // One retry: the stop frame changes nothing when sent twice.
            std::thread::sleep(Duration::from_millis(200));
            session.request_button_spy(false)
        }
        other => other,
    }
    .map(|_| ());
    match &stop {
        Ok(()) => line("spy stopped (0x8110 fn2)".into()),
        Err(e) => line(format!("stop failed: {e}")),
    }
    match failure {
        Some(cause) => Err(SpyError::Failed { cause, stop }),
        None => Ok(SpyReport {
            spy_events,
            other_events,
            stop,
        }),
    }
}

/// Sends only 0x8110 fn2 (stop spy, zero params) and returns the reply payload.
/// No mode requirement: it is the way out after a capture whose process was
/// killed or whose console window was closed (that skips the stop).
pub fn spy_stop<L: HidLink>(session: &mut HidppSession<L>) -> Result<[u8; 16], SessionError> {
    session.request_button_spy(false)
}
