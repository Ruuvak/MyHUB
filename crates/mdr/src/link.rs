//! Sans-IO link layer: ACKs, sequence bit, resend timer, duplicate detection.
//!
//! No sockets, threads or clocks. The caller feeds received bytes and the current time
//! in milliseconds, and writes out whatever the state machine returns. Rules from
//! `PROTOCOL.md` section 3 (read from the app) and `DEVICE-WF-C710N.md` (seen on the
//! wire):
//!
//! * One frame awaiting an ACK at a time. The first send uses seq 0.
//! * An ACK carries the next seq (`1 - acked seq`) and the sender adopts it. An ACK
//!   whose seq equals the seq of the outstanding frame is ignored, and so is an ACK
//!   when nothing is outstanding.
//! * No ACK within [`ACK_TIMEOUT_MS`]: resend the same bytes, up to [`MAX_RESENDS`]
//!   times, then [`Tick::LinkDead`].
//! * Every received frame whose data type needs an ACK is ACKed with
//!   `1 - theirSeq`, including frames we cannot parse and duplicates. A data frame
//!   with the same seq as the last accepted one is a duplicate: ACKed, not delivered.
//! * The headset pushes data frames right after connect, before we send anything.
//!   That needs no setup, `on_bytes` works from the first byte.
//! * [`Link::reset`] is for a reconnect: send seq 0, last received none, and a new
//!   epoch. Alert tokens received before the reset are stale: [`Link::send`] refuses an
//!   alert answer made from them ([`SendError::StaleAlert`]). Epochs come from a global
//!   counter, so a token from another `Link` is stale too.
//! * [`Link::send`] also refuses any request checked against another epoch
//!   ([`SendError::StaleRequest`]): the table and limits it relied on belong to a link that
//!   was reset or to another link.

use thiserror::Error;

use std::sync::atomic::{AtomicU64, Ordering};

use crate::frame::{DataType, Frame, FrameDecoder, ReceivedFrame, encode_ack};
use crate::policy::CheckedRequest;

/// Milliseconds to wait for an ACK before resending.
pub const ACK_TIMEOUT_MS: u64 = 750;
/// Resends before the link is declared dead.
pub const MAX_RESENDS: u32 = 10;

/// Why [`Link::send`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SendError {
    /// A frame is still awaiting its ACK.
    #[error("a frame is still awaiting its ACK")]
    Busy,
    /// The link was declared dead; call [`Link::reset`] after reconnecting.
    #[error("link is dead, reset after reconnecting")]
    Dead,
    /// The request was checked against an older epoch of this link (or another link):
    /// the function table and limits it relied on are stale. Check it again.
    #[error("request was checked before the link was reset")]
    StaleRequest,
    /// The request is an alert answer made from a token of an older epoch (the link was
    /// reset, or the token came from another link).
    #[error("alert token is stale (link was reset)")]
    StaleAlert,
}

/// Something the link produced from received bytes.
#[derive(Debug, PartialEq, Eq)]
pub enum LinkEvent {
    /// Bytes to write to the transport (an ACK).
    Outgoing(Vec<u8>),
    /// A data frame for the application (never a duplicate, never an ACK).
    Delivered(ReceivedFrame),
    /// The outstanding frame was acknowledged.
    Acked,
}

/// Result of [`Link::on_tick`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    /// Write these bytes again (same bytes, same seq).
    Resend(Vec<u8>),
    /// All resends used up without an ACK.
    LinkDead,
}

#[derive(Debug)]
struct Outstanding {
    bytes: Vec<u8>,
    deadline: u64,
    resends: u32,
}

/// The link state machine.
#[derive(Debug)]
pub struct Link {
    decoder: FrameDecoder,
    tx_seq: u8,
    last_rx_seq: Option<u8>,
    outstanding: Option<Outstanding>,
    dead: bool,
    epoch: u64,
}

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

impl Default for Link {
    fn default() -> Self {
        Self {
            decoder: FrameDecoder::new(),
            tx_seq: 0,
            last_rx_seq: None,
            outstanding: None,
            dead: false,
            epoch: NEXT_EPOCH.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl Link {
    pub fn new() -> Self {
        Self::default()
    }

    /// Back to the connect state: send seq 0, last received none, nothing outstanding,
    /// and a new epoch (tokens and frames of the old epoch go stale).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The current epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// True once the resends ran out. [`Link::send`] fails until [`Link::reset`].
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// True while a frame awaits its ACK.
    pub fn is_busy(&self) -> bool {
        self.outstanding.is_some()
    }

    /// The seq the next send will use.
    pub fn next_seq(&self) -> u8 {
        self.tx_seq
    }

    /// Encode and register a checked request. Returns the bytes to write now.
    pub fn send(&mut self, request: CheckedRequest, now_ms: u64) -> Result<Vec<u8>, SendError> {
        if self.dead {
            return Err(SendError::Dead);
        }
        if self.outstanding.is_some() {
            return Err(SendError::Busy);
        }
        if request.alert_epoch().is_some_and(|e| e != self.epoch) {
            return Err(SendError::StaleAlert);
        }
        if request.checked_epoch() != self.epoch {
            return Err(SendError::StaleRequest);
        }
        let bytes = request.encode(self.tx_seq);
        self.outstanding = Some(Outstanding {
            bytes: bytes.clone(),
            deadline: now_ms.saturating_add(ACK_TIMEOUT_MS),
            resends: 0,
        });
        Ok(bytes)
    }

    /// Feed received bytes (any chunking).
    pub fn on_bytes(&mut self, bytes: &[u8], _now_ms: u64) -> Vec<LinkEvent> {
        let mut events = Vec::new();
        for frame in self.decoder.push(bytes) {
            self.on_frame(frame, &mut events);
        }
        events
    }

    fn on_frame(&mut self, frame: Frame, events: &mut Vec<LinkEvent>) {
        if frame.data_type == DataType::Ack {
            let acked = frame.seq & 1;
            if self.outstanding.is_some() && acked != self.tx_seq {
                self.tx_seq = acked;
                self.outstanding = None;
                events.push(LinkEvent::Acked);
            }
            return;
        }
        if !frame.data_type.needs_ack() {
            events.push(LinkEvent::Delivered(ReceivedFrame::new(frame, self.epoch)));
            return;
        }
        let seq = frame.seq & 1;
        events.push(LinkEvent::Outgoing(encode_ack(seq ^ 1)));
        if self.last_rx_seq == Some(seq) {
            return;
        }
        self.last_rx_seq = Some(seq);
        events.push(LinkEvent::Delivered(ReceivedFrame::new(frame, self.epoch)));
    }

    /// The time at which [`Link::on_tick`] has work to do, if any.
    pub fn next_deadline(&self) -> Option<u64> {
        self.outstanding.as_ref().map(|o| o.deadline)
    }

    /// Drive the resend timer. Call when `now_ms` reaches [`Link::next_deadline`].
    pub fn on_tick(&mut self, now_ms: u64) -> Option<Tick> {
        let out = self.outstanding.as_mut()?;
        if now_ms < out.deadline {
            return None;
        }
        if out.resends >= MAX_RESENDS {
            self.outstanding = None;
            self.dead = true;
            return Some(Tick::LinkDead);
        }
        out.resends += 1;
        out.deadline = now_ms.saturating_add(ACK_TIMEOUT_MS);
        Some(Tick::Resend(out.bytes.clone()))
    }
}
