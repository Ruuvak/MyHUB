//! The session: one frame in flight, read retries, foreign traffic back-off.
//!
//! Every frame written comes from `Policy::check` -> `CheckedRequest::encode`. The
//! stored policy is read-only and stays so: a write opens exactly one extra tier on a
//! copy for that one call (`request_with`, crate private; see `writes.rs`). A write is
//! never resent, and two writes are at least [`WRITE_GAP`] apart.
//! The session fills the runtime limits (profile count, buffer sizes, layers, LEDs,
//! matrix) as the snapshot reads them. The link is private: nothing outside this crate
//! can reach `RawLink::write_report` through a session.
//!
//! Foreign traffic rule: frames carry no sequence number, so a reply cannot be told
//! from the launcher's reply to the same command. Any report that is not the reply to
//! the pending request and not a known unsolicited report is counted, fails the current
//! request, and the session then refuses new requests for the back-off (5 s). An input
//! report that is not a 32-byte frame counts the same way.
//!
//! Known blind spots, accepted: `08 03 02 <effect>` is an unsolicited event, but while
//! our own VIA 08 03 02 read is pending it can be taken as our reply (same meaning, the
//! effect value). The launcher's own `08 03 02` replies and `A9 11` echoes are classified
//! as events, not as foreign traffic, so they do not start the back-off.
//!
//! Late replies: a read is resent after a timeout, so the keyboard may answer twice.
//! After a read succeeds on attempt k > 1 the session keeps reading for up to the reply
//! timeout to absorb the k-1 late replies; entries for replies that did not arrive stay
//! in a one-shot stale list for the next exchange. A frame that matches both the pending
//! request and a stale entry cannot be told apart and fails with `AmbiguousReply`.
//! Inherent limit: a reply more than about 2 s late (beyond the absorb window and the
//! next exchange) that arrives while a third request of the same shape is pending is
//! accepted as its answer; that needs a reply far beyond the timeout.

use std::time::{Duration, Instant};

use hidapi::HidApi;
use keychron_proto::{CheckedRequest, FRAME_LEN, Frame, Policy, Request, Target, Tier};

use crate::discovery::{enumerate, ids, select, select_error};
use crate::error::SessionError;
use crate::hid::HidapiLink;
use crate::link::{LinkError, READ_BUF_LEN, REPORT_LEN, RawLink};

/// Reply timeout per attempt [INF: launcher uses 1 s].
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(1);
/// Lowest reply timeout `set_reply_timeout` accepts.
pub const MIN_REPLY_TIMEOUT: Duration = Duration::from_millis(200);
/// Extra attempts after a timeout, reads only [INF: launcher resends 3 times].
pub const READ_RETRIES: u32 = 2;
/// Back-off after foreign traffic.
pub const FOREIGN_BACKOFF: Duration = Duration::from_secs(5);
/// Minimum time between two write-tier frames (Volatile or Persistent).
pub const WRITE_GAP: Duration = Duration::from_millis(250);
/// Most reports taken in one drain; more than this counts as foreign flooding.
const MAX_DRAIN: usize = 64;

/// Direction of a traced frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDirection {
    /// Written to the device: the 32-byte frame (report ID 0 not included).
    Tx,
    /// Read from the device: the report exactly as the backend returned it, so the
    /// length shows whether a report ID came with it.
    Rx,
}

/// Observer for frames. A plain function pointer: it sees the bytes but can neither
/// send nor change anything.
pub type FrameTrace = fn(FrameDirection, &[u8]);

/// Unsolicited reports the keyboard sends by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// `A9 11 ?? pp`: the active HE profile changed on the keyboard [OBS shape].
    ProfileChanged { profile: u8 },
    /// `08 03 02 <effect>`: the lighting effect changed [INF].
    EffectChanged { effect: u8 },
}

/// What [`Session::poll`] saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incoming {
    Event(Event),
    /// A report that is not a known unsolicited one (foreign traffic).
    Foreign(Frame),
}

fn as_event(f: &Frame) -> Option<Event> {
    match (f[0], f[1], f[2]) {
        (0xA9, 0x11, _) => Some(Event::ProfileChanged { profile: f[3] }),
        (0x08, 0x03, 0x02) => Some(Event::EffectChanged { effect: f[3] }),
        _ => None,
    }
}

/// Reply or not: the reply to `pending` first (a reply and an event can share a prefix,
/// the pending request wins), then events, everything else is foreign.
enum Class {
    Reply,
    Event(Event),
    Foreign,
}

fn classify(frame: &Frame, pending: Option<&CheckedRequest>) -> Class {
    if pending.is_some_and(|p| p.matches(frame)) {
        return Class::Reply;
    }
    match as_event(frame) {
        Some(e) => Class::Event(e),
        None => Class::Foreign,
    }
}

/// A session over a [`RawLink`]. The link is not reachable from outside the crate.
///
/// ```compile_fail
/// fn f(mut s: keychron_k2he::Session<keychron_k2he::HidapiLink>) {
///     let _ = s.link_mut();
/// }
/// ```
///
/// ```compile_fail
/// fn f(s: keychron_k2he::Session<keychron_k2he::HidapiLink>) {
///     let _ = s.link();
/// }
/// ```
///
/// ```compile_fail
/// fn f(s: keychron_k2he::Session<keychron_k2he::HidapiLink>) {
///     let _ = s.link;
/// }
/// ```
///
/// ```compile_fail
/// fn f(s: keychron_k2he::Session<keychron_k2he::HidapiLink>) {
///     let _ = s.policy_with(keychron_proto::Tier::Volatile);
/// }
/// ```
///
/// ```compile_fail
/// fn f(mut s: keychron_k2he::Session<keychron_k2he::HidapiLink>) {
///     let p = keychron_proto::Policy::allowing(keychron_proto::Tier::Volatile);
///     let _ = s.request_with(&p, keychron_proto::Request::GetViaProtocol);
/// }
/// ```
pub struct Session<L: RawLink> {
    link: L,
    policy: Policy,
    events: Vec<Event>,
    foreign: u32,
    backoff_until: Option<Instant>,
    reply_timeout: Duration,
    /// Late replies that may still arrive from the previous exchange. Each entry
    /// absorbs one matching frame, then goes. Cleared when the current exchange ends.
    stale: Vec<Request>,
    /// Entries for the next exchange.
    next_stale: Vec<Request>,
    trace: Option<FrameTrace>,
    frames_sent: u32,
    matched: usize,
    /// When the last write-tier frame went out.
    last_write: Option<Instant>,
    /// The device this session talks to; fixed at open.
    target: Target,
}

impl<L: RawLink> Session<L> {
    /// A keyboard session over any link (mocks in tests). Read-only policy.
    pub fn with_link(link: L) -> Self {
        Self::with_link_for(link, Target::Keyboard)
    }

    /// A session over any link for `target`. Requests meant for the other target are
    /// refused with [`SessionError::TargetMismatch`] before any byte is written.
    pub fn with_link_for(link: L, target: Target) -> Self {
        Session {
            target,
            link,
            policy: Policy::read_only(),
            events: Vec::new(),
            foreign: 0,
            backoff_until: None,
            reply_timeout: REPLY_TIMEOUT,
            stale: Vec::new(),
            next_stale: Vec::new(),
            trace: None,
            frames_sent: 0,
            matched: 1,
            last_write: None,
        }
    }

    /// The device this session talks to.
    pub fn target(&self) -> Target {
        self.target
    }

    pub fn set_frame_trace(&mut self, trace: FrameTrace) {
        self.trace = Some(trace);
    }

    /// Reply timeout per attempt (default [`REPLY_TIMEOUT`], floor
    /// [`MIN_REPLY_TIMEOUT`]).
    pub fn set_reply_timeout(&mut self, timeout: Duration) {
        self.reply_timeout = timeout.max(MIN_REPLY_TIMEOUT);
    }

    /// Frames written so far, resends included.
    pub fn frames_sent(&self) -> u32 {
        self.frames_sent
    }

    /// Reports counted as foreign traffic so far.
    pub fn foreign_count(&self) -> u32 {
        self.foreign
    }

    /// How many interfaces matched at `open` (1 for a session made with `with_link`).
    pub fn matched_interfaces(&self) -> usize {
        self.matched
    }

    /// Time left of the back-off, if any.
    pub fn backoff_remaining(&self) -> Option<Duration> {
        self.backoff_until
            .map(|t| t.saturating_duration_since(Instant::now()))
            .filter(|d| !d.is_zero())
    }

    /// Unsolicited reports seen since the last call.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Fill runtime limits of the policy. The closure gets the current policy and
    /// returns it with limits added. The stored policy never gets a gate: `with_tier` is
    /// only applied to a per-call copy by `policy_with`.
    pub(crate) fn set_limits(&mut self, f: impl FnOnce(Policy) -> Policy) {
        self.policy = f(self.policy);
    }

    /// The stored policy plus exactly `tier`, for one call. The stored policy itself is
    /// not changed.
    pub(crate) fn policy_with(&self, tier: Tier) -> Policy {
        self.policy.with_tier(tier)
    }

    /// Check `request` against the policy and run it: drain, write, wait for the reply.
    /// A read is resent up to [`READ_RETRIES`] times after a timeout; nothing else is.
    pub fn request(&mut self, request: Request) -> Result<Frame, SessionError> {
        let policy = self.policy;
        self.request_with(&policy, request)
    }

    /// [`Session::request`] against an explicit one-call policy. Same rules: one frame in
    /// flight, drain and foreign checks, back-off, no resend for a non-Read tier. A
    /// write-tier frame also waits until [`WRITE_GAP`] has passed since the last one; that
    /// wait is not part of any reply timeout.
    pub(crate) fn request_with(
        &mut self,
        policy: &Policy,
        request: Request,
    ) -> Result<Frame, SessionError> {
        if request.target() != self.target {
            return Err(SessionError::TargetMismatch {
                request: request.target(),
                session: self.target,
            });
        }
        let checked = policy.check(&request)?;
        // Nothing is read while backing off, so the stale list stays as it is.
        if let Some(left) = self.backoff_remaining() {
            return Err(SessionError::BackingOff {
                remaining_ms: u64::try_from(left.as_millis()).unwrap_or(u64::MAX),
            });
        }
        if checked.tier() != Tier::Read
            && let Some(last) = self.last_write
        {
            let wait = (last + WRITE_GAP).saturating_duration_since(Instant::now());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
        }
        let result = self.exchange(&checked);
        self.stale = std::mem::take(&mut self.next_stale);
        result
    }

    fn exchange(&mut self, checked: &CheckedRequest) -> Result<Frame, SessionError> {
        self.drain()?;
        // Only reads are resent: a write that timed out may have been applied.
        let attempts = if checked.tier() == Tier::Read {
            1 + READ_RETRIES
        } else {
            1
        };
        for attempt in 1..=attempts {
            self.write(checked)?;
            match self.await_reply(checked) {
                Ok(frame) => {
                    if attempt > 1 {
                        self.absorb_late(checked, attempt - 1)?;
                    }
                    return Ok(frame);
                }
                Err(SessionError::Timeout) if attempt == attempts => {
                    // Every attempt may still get a late reply.
                    for _ in 0..attempts {
                        self.next_stale.push(*checked.request());
                    }
                    return Err(SessionError::Timeout);
                }
                Err(SessionError::Timeout) => {}
                Err(e) => return Err(e),
            }
        }
        Err(SessionError::Timeout)
    }

    fn write(&mut self, checked: &CheckedRequest) -> Result<(), SessionError> {
        let frame = checked.encode();
        let mut report = [0u8; REPORT_LEN];
        report[1..].copy_from_slice(&frame);
        if let Some(trace) = self.trace {
            trace(FrameDirection::Tx, &frame);
        }
        self.link.write_report(&report)?;
        if checked.tier() != Tier::Read {
            self.last_write = Some(Instant::now());
        }
        self.frames_sent += 1;
        Ok(())
    }

    /// One report from the link as a frame; `None` when nothing arrived. A report of
    /// any other shape counts as foreign traffic.
    fn read_frame(&mut self, timeout_ms: i32) -> Result<Option<Frame>, SessionError> {
        let mut buf = [0u8; READ_BUF_LEN];
        let n = self.link.read_report(&mut buf, timeout_ms)?;
        if n == 0 {
            return Ok(None);
        }
        let n = n.min(READ_BUF_LEN);
        if let Some(trace) = self.trace {
            trace(FrameDirection::Rx, &buf[..n]);
        }
        // [INF] The backend may return the report ID with the data; strip a leading 0.
        let start = match n {
            FRAME_LEN => 0,
            REPORT_LEN if buf[0] == 0 => 1,
            other => {
                let _ = self.mark_foreign();
                return Err(SessionError::BadReport(other));
            }
        };
        let mut frame = [0u8; FRAME_LEN];
        frame.copy_from_slice(&buf[start..start + FRAME_LEN]);
        Ok(Some(frame))
    }

    /// Absorb one late reply: removes one stale entry that matches `frame`.
    fn take_stale(&mut self, frame: &Frame) -> bool {
        match self.stale.iter().position(|r| r.matches(frame)) {
            Some(i) => {
                self.stale.swap_remove(i);
                true
            }
            None => false,
        }
    }

    fn mark_foreign(&mut self) -> SessionError {
        self.foreign += 1;
        self.backoff_until = Some(Instant::now() + FOREIGN_BACKOFF);
        SessionError::ForeignTraffic
    }

    /// Take everything already waiting, before a request is written.
    fn drain(&mut self) -> Result<(), SessionError> {
        let mut foreign = false;
        let mut emptied = false;
        for _ in 0..MAX_DRAIN {
            let Some(frame) = self.read_frame(0)? else {
                emptied = true;
                break;
            };
            match classify(&frame, None) {
                Class::Event(e) => self.events.push(e),
                Class::Reply => {}
                Class::Foreign => foreign |= !self.take_stale(&frame),
            }
        }
        if foreign || !emptied {
            return Err(self.mark_foreign());
        }
        Ok(())
    }

    fn await_reply(&mut self, checked: &CheckedRequest) -> Result<Frame, SessionError> {
        let deadline = Instant::now() + self.reply_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(SessionError::Timeout);
            }
            let ms = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
            let Some(frame) = self.read_frame(ms)? else {
                return Err(SessionError::Timeout);
            };
            match classify(&frame, Some(checked)) {
                Class::Reply if self.stale.iter().any(|r| r.matches(&frame)) => {
                    // Our reply or a late one of the previous exchange: cannot tell.
                    // The other frame of the pair may still come: absorb it next time.
                    self.next_stale.push(*checked.request());
                    return Err(SessionError::AmbiguousReply);
                }
                Class::Reply => return Ok(frame),
                Class::Event(e) => self.events.push(e),
                Class::Foreign if self.take_stale(&frame) => {}
                Class::Foreign => return Err(self.mark_foreign()),
            }
        }
    }

    /// After a read that needed `expected` resends: wait up to the reply timeout for the
    /// `expected` late replies to the earlier attempts. Entries for the ones that did
    /// not arrive go to the next exchange.
    fn absorb_late(
        &mut self,
        checked: &CheckedRequest,
        mut expected: u32,
    ) -> Result<(), SessionError> {
        let deadline = Instant::now() + self.reply_timeout;
        while expected > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let ms = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
            let Some(frame) = self.read_frame(ms)? else {
                break;
            };
            match classify(&frame, Some(checked)) {
                Class::Reply => expected -= 1,
                Class::Event(e) => self.events.push(e),
                Class::Foreign if self.take_stale(&frame) => {}
                Class::Foreign => return Err(self.mark_foreign()),
            }
        }
        for _ in 0..expected {
            self.next_stale.push(*checked.request());
        }
        Ok(())
    }

    /// Wait up to `timeout` for one unsolicited report. Sends nothing. Events are also
    /// queued for [`Session::drain_events`]; foreign reports are counted and start the
    /// back-off like anywhere else.
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<Incoming>, SessionError> {
        let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let Some(frame) = self.read_frame(ms)? else {
            return Ok(None);
        };
        Ok(Some(match classify(&frame, None) {
            Class::Event(e) => {
                self.events.push(e);
                Incoming::Event(e)
            }
            _ => {
                let _ = self.mark_foreign();
                Incoming::Foreign(frame)
            }
        }))
    }
}

impl Session<HidapiLink> {
    /// Finds the K2 HE's raw HID interface and opens it. With several matches the first
    /// is used; [`Session::matched_interfaces`] says how many there were. Nothing is
    /// written.
    pub fn open() -> Result<Self, SessionError> {
        Self::open_target(Target::Keyboard)
    }

    /// Finds the Keychron Link receiver's raw HID interface (PID [`crate::LINK_PID`]) and
    /// opens it. Same rules as [`Session::open`]. Only B1..B3 can be sent over it.
    pub fn open_receiver() -> Result<Self, SessionError> {
        Self::open_target(Target::Receiver)
    }

    fn open_target(target: Target) -> Result<Self, SessionError> {
        let api =
            HidApi::new().map_err(|_| LinkError::Hid("could not enumerate HID devices".into()))?;
        let (pid, fallback) = ids(target);
        let entries = enumerate(&api, pid);
        let chosen = select(&entries, fallback).map_err(|e| select_error(e, target))?;
        let first = &entries[chosen[0]];
        let link = HidapiLink::open(&api, &first.path)?;
        let mut session = Session::with_link_for(link, target);
        session.matched = chosen.len();
        Ok(session)
    }
}
