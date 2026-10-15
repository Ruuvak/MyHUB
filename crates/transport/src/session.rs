//! HID++ 2.0 request/reply session over a [`HidLink`].
//!
//! Every outgoing frame is built with `Policy::check_indexed` and
//! `CheckedRequest::encode`; the session has no other way to produce bytes.
//! The session's own policy is the read tier only (`Policy::default()`) and
//! cannot be changed. The volatile tier (SetSensorDPI, SetReportRate, SetOnboardMode) is
//! reachable only through [`HidppSession::request_volatile`], which builds its
//! policy per call. The onboard write tier (0x8100 fn 6, 7, 8, flash) is
//! reachable only through [`HidppSession::write_sector`], which takes a sealed
//! `hidpp::onboard_write::SectorImage` and builds every frame itself, also with a
//! call-local policy. Profile selection (0x8100 fn 3) is reachable only through
//! [`HidppSession::request_profile_select`], again with a call-local policy. The button spy
//! (0x8110 fn 1 and 2) is reachable only through
//! [`HidppSession::request_button_spy`].
//!
//! Reply matching: a reply must match device index, feature index, function
//! and the session's software ID (`hidpp::is_reply_to`). Anything else is not
//! ours. G HUB and the Linux kernel driver may talk to the same receiver with
//! other software IDs, so foreign replies are skipped. HID++ 2.0 notifications
//! (software ID 0) and HID++ 1.0 messages go to a bounded buffer
//! ([`HidppSession::drain_notifications`]).
//!
//! A HID++ 1.0 `0x41` (device connection) message for the session's device index
//! invalidates the feature table
//! (the device may have reset, so stored indexes can point at other features).
//! The session then refuses requests until [`HidppSession::discover`] runs.
//!
//! The same 0x41 message also carries the link state of the device: payload
//! byte 1 bit 0x40 set means the link is down (the mouse is switched off or
//! asleep) \[OBS\]. [`HidppSession::link_state`] reports the last one seen for
//! the session's device index. [`HidppSession::poll_events`] listens without
//! writing anything, so an application can follow notifications while idle.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use hidapi::HidApi;
use hidpp::feature::{
    FEATURE_SET_FN_GET_COUNT, FEATURE_SET_FN_GET_FEATURE_ID, ROOT_FN_GET_FEATURE,
    feature_set_get_feature_id_params, parse_feature_count, parse_feature_id,
    parse_root_get_feature, root_get_feature_params,
};
use hidpp::frame::{HIDPP10_ERROR_SUB_ID, decode};
use hidpp::onboard_write::{
    FN_BEGIN_WRITE, FN_END_WRITE, FN_WRITE_DATA, SECTOR_LEN, SectorImage, SelectableSector,
    begin_write_params, end_write_params, select_profile_params, write_chunks,
};
use hidpp::{
    CheckedRequest, DEFAULT_SW_ID, FeatureEntry, FeatureId, FeatureTable, FeatureTableError,
    Message, ParseError, Policy, PolicyError, is_reply_to,
};
use thiserror::Error;

use crate::discovery::ReceiverPath;
use crate::hidapi_link::HidapiLink;
use crate::link::{HidLink, LinkError, READ_BUF_LEN};

/// How long a request waits for its reply in total (matches the reference
/// probe).
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_millis(1500);
/// Length of one poll of the link (matches the reference probe).
pub const POLL_INTERVAL_MS: i32 = 300;
/// Capacity of the notification buffer. The oldest entry is dropped when full.
pub const NOTIFICATION_CAPACITY: usize = 64;
/// Lower bound of [`HidppSession::set_reply_timeout`].
pub const MIN_REPLY_TIMEOUT: Duration = Duration::from_millis(1);
/// Upper bound of [`HidppSession::set_reply_timeout`].
pub const MAX_REPLY_TIMEOUT: Duration = Duration::from_secs(60);
/// Reply timeout of BeginWrite and EndWrite in [`HidppSession::write_sector`]: a
/// flash erase or commit may be slow.
pub const FLASH_COMMIT_TIMEOUT: Duration = Duration::from_secs(3);
/// Most frames one drain reads, so a chatty device cannot stall a request.
const DRAIN_LIMIT: usize = 1024;

/// HID++ 1.0 sub ID of the device connection notification.
const HIDPP10_DEVICE_CONNECTION: u8 = 0x41;
/// Bit of 0x41 payload byte 1 that is set while the link is down \[OBS\].
const LINK_DOWN_BIT: u8 = 0x40;

/// Reads a HID++ 1.0 0x41 device connection notification: the device index it
/// is about and whether that device's link is up (`false` = switched off or
/// asleep). `None` for any other message.
pub fn connection_notification(message: &Message) -> Option<(u8, bool)> {
    match message {
        Message::Hidpp10 {
            device_index,
            sub_id: HIDPP10_DEVICE_CONNECTION,
            payload,
        } => Some((*device_index, payload[1] & LINK_DOWN_BIT == 0)),
        _ => None,
    }
}

/// Reply payloads of one [`HidppSession::write_sector`] run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteLog {
    /// fn 6 reply payload.
    pub begin: Option<[u8; 16]>,
    /// fn 7 reply payloads, in order.
    pub data: Vec<[u8; 16]>,
    /// fn 8 reply payload.
    pub end: Option<[u8; 16]>,
}

/// `true` when the error means the link is down (a 0x41, or the table was
/// invalidated by one): nothing more may be sent.
pub fn link_lost(e: &SessionError) -> bool {
    matches!(e, SessionError::NeedsRediscovery | SessionError::LinkDown)
}

/// Which protocol generation an error code comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorFamily {
    /// HID++ 2.0 error report (`11 dev FF featIdx fn|sw err`).
    Hidpp20,
    /// HID++ 1.0 error report (sub ID `0x8F`).
    Hidpp10,
}

impl fmt::Display for ErrorFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hidpp20 => "HID++ 2.0",
            Self::Hidpp10 => "HID++ 1.0",
        })
    }
}

/// Standard HID++ 2.0 error names. These come from the public protocol
/// description, not from observation on this device (no error frame has been
/// captured from it). `None` for codes without a standard name here.
pub fn hidpp20_error_name(code: u8) -> Option<&'static str> {
    Some(match code {
        1 => "Unknown",
        2 => "InvalidArgument",
        3 => "OutOfRange",
        4 => "HwError",
        5 => "LogitechInternal",
        6 => "InvalidFeatureIndex",
        7 => "InvalidFunction",
        8 => "Busy",
        9 => "Unsupported",
        _ => return None,
    })
}

/// Direction of a traced frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDirection {
    /// Written to the device.
    Tx,
    /// Read from the device while waiting for a reply.
    Rx,
}

/// Observer for frames, see [`HidppSession::set_frame_trace`]. A plain function
/// pointer: it sees the bytes but can neither send nor change anything.
pub type FrameTrace = fn(FrameDirection, &[u8]);

/// A session-level failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionError {
    /// The safety policy refused the request. Nothing was written.
    #[error("refused by the safety policy: {0}")]
    Policy(#[from] PolicyError),
    /// The link failed.
    #[error("link error: {0}")]
    Link(#[from] LinkError),
    /// No matching reply within the reply timeout.
    #[error("no reply within the timeout")]
    Timeout,
    /// The feature table is invalid (a device connection notification was
    /// seen, or discovery has not run or failed). Run `discover()` first.
    #[error("feature table invalid: run discover() again")]
    NeedsRediscovery,
    /// The device answered with an error report. `name` is set only for
    /// standard HID++ 2.0 codes ([`hidpp20_error_name`]).
    #[error("{family} device error {code:#04x} ({})", .name.unwrap_or("no standard name"))]
    Device {
        code: u8,
        family: ErrorFamily,
        name: Option<&'static str>,
    },
    /// A HID++ 1.0 0x41 says the device link is down (switched off or asleep),
    /// so discovery was not attempted. Nothing was written.
    #[error("the device link is down")]
    LinkDown,
    /// The device does not implement the requested feature.
    #[error("feature {0} is not implemented by the device")]
    FeatureAbsent(FeatureId),
    /// Root does not know FeatureSet (0x0001); enumeration is impossible.
    #[error("device does not report the FeatureSet feature")]
    FeatureSetMissing,
    /// A discovery reply could not be parsed.
    #[error("malformed reply: {0}")]
    Parse(#[from] ParseError),
    /// A discovery row could not be stored.
    #[error("feature table rejected a row: {0}")]
    Table(#[from] FeatureTableError),
}

/// A HID++ session with one device behind a receiver.
#[derive(Debug)]
pub struct HidppSession<L: HidLink> {
    link: L,
    device_index: u8,
    sw_id: u8,
    policy: Policy,
    table: FeatureTable,
    notifications: VecDeque<Message>,
    dropped_notifications: u64,
    needs_rediscovery: bool,
    link_up: Option<bool>,
    reply_timeout: Duration,
    request_count: u64,
    trace: Option<FrameTrace>,
}

impl<L: HidLink> HidppSession<L> {
    /// Read-tier session: device index 1, software ID `hidpp::DEFAULT_SW_ID`,
    /// empty feature table. Run [`Self::discover`] before any feature request.
    pub fn new(link: L) -> Self {
        Self {
            link,
            device_index: 1,
            sw_id: DEFAULT_SW_ID,
            policy: Policy::default(),
            table: FeatureTable::new(),
            notifications: VecDeque::new(),
            dropped_notifications: 0,
            needs_rediscovery: true,
            link_up: None,
            reply_timeout: DEFAULT_REPLY_TIMEOUT,
            request_count: 0,
            trace: None,
        }
    }

    /// Installs an observer that sees every frame written and every frame read
    /// while waiting for a reply (diagnostics, e.g. a verbose probe). It is a
    /// read-only view and adds no way to send.
    pub fn set_frame_trace(&mut self, trace: FrameTrace) {
        self.trace = Some(trace);
    }

    /// Sets the device index (1..=6 for devices behind a receiver). Invalidates
    /// the feature table and forgets the link state, because both belong to
    /// the previous device.
    pub fn set_device_index(&mut self, device_index: u8) {
        self.device_index = device_index;
        self.link_up = None;
        self.invalidate_table();
    }

    /// Sets the software ID. The value is validated by the safety policy (the
    /// policy accepts 2..=15), not by this crate. Nothing is sent.
    pub fn set_sw_id(&mut self, sw_id: u8) -> Result<(), SessionError> {
        self.policy.check_indexed(
            &FeatureTable::new(),
            self.device_index,
            0,
            ROOT_FN_GET_FEATURE,
            sw_id,
            [0; 16],
        )?;
        self.sw_id = sw_id;
        Ok(())
    }

    /// Sets the total reply timeout (default [`DEFAULT_REPLY_TIMEOUT`]),
    /// clamped to [`MIN_REPLY_TIMEOUT`]..=[`MAX_REPLY_TIMEOUT`].
    pub fn set_reply_timeout(&mut self, timeout: Duration) {
        self.reply_timeout = timeout.clamp(MIN_REPLY_TIMEOUT, MAX_REPLY_TIMEOUT);
    }

    /// `false` when the link cannot see short (0x10) reports (the Windows
    /// short collection could not be opened), so a HID++ 1.0 0x41 connection
    /// notification may go unnoticed.
    pub fn short_reports_available(&self) -> bool {
        self.link.short_reports_available()
    }

    pub fn device_index(&self) -> u8 {
        self.device_index
    }

    pub fn sw_id(&self) -> u8 {
        self.sw_id
    }

    /// The feature table. Empty until [`Self::discover`] succeeded.
    pub fn table(&self) -> &FeatureTable {
        &self.table
    }

    /// `true` while requests are refused until [`Self::discover`] runs.
    pub fn needs_rediscovery(&self) -> bool {
        self.needs_rediscovery
    }

    /// Link state of the session's device from the last HID++ 1.0 0x41
    /// notification for its device index: `Some(true)` up, `Some(false)` down
    /// (switched off or asleep), `None` when no such notification was seen.
    pub fn link_state(&self) -> Option<bool> {
        self.link_up
    }

    /// Number of requests written to the link so far.
    pub fn request_count(&self) -> u64 {
        self.request_count
    }

    /// Takes the buffered notifications, oldest first.
    pub fn drain_notifications(&mut self) -> Vec<Message> {
        self.notifications.drain(..).collect()
    }

    /// Notifications dropped because the buffer was full.
    pub fn dropped_notifications(&self) -> u64 {
        self.dropped_notifications
    }

    fn invalidate_table(&mut self) {
        self.table = FeatureTable::new();
        self.needs_rediscovery = true;
    }

    fn push_notification(&mut self, message: Message) {
        if self.notifications.len() >= NOTIFICATION_CAPACITY {
            self.notifications.pop_front();
            self.dropped_notifications += 1;
        }
        self.notifications.push_back(message);
    }

    /// Enumerates the device's features into a fresh table: Root
    /// GetFeature(0x0001), FeatureSet GetCount, FeatureSet GetFeatureID for
    /// `1..=count`. Any failure leaves the table empty and the session in the
    /// needs-rediscovery state. Returns [`SessionError::LinkDown`], without
    /// writing, when a 0x41 seen while draining says the device is off.
    pub fn discover(&mut self) -> Result<(), SessionError> {
        // Anything queued from before (a connection notification that caused
        // this rediscovery, late replies) is dropped first. A 0x41 among it is
        // moot because the table is rebuilt now anyway.
        let _ = self.drain_link();
        if self.link_up == Some(false) {
            // The drain showed that the device is off: nothing is sent.
            self.invalidate_table();
            return Err(SessionError::LinkDown);
        }
        self.table = FeatureTable::new();
        self.needs_rediscovery = false;
        let result = self.discover_inner();
        if result.is_err() {
            self.invalidate_table();
        }
        result
    }

    fn discover_inner(&mut self) -> Result<(), SessionError> {
        let policy = self.policy;
        let root = self.exchange(
            &policy,
            0,
            ROOT_FN_GET_FEATURE,
            root_get_feature_params(FeatureId::FEATURE_SET),
        )?;
        let set = parse_root_get_feature(&root)?;
        if !set.is_present() {
            return Err(SessionError::FeatureSetMissing);
        }
        // The policy resolves wire indexes through the table, so the
        // FeatureSet row must be stored before FeatureSet can be called.
        self.table.insert_entry(FeatureEntry {
            index: set.index,
            id: FeatureId::FEATURE_SET,
            feature_type: set.feature_type,
            version: set.version,
        })?;
        let count_payload = self.exchange(&policy, set.index, FEATURE_SET_FN_GET_COUNT, [0; 16])?;
        let count = parse_feature_count(&count_payload)?;
        for index in 1..=count {
            let payload = self.exchange(
                &policy,
                set.index,
                FEATURE_SET_FN_GET_FEATURE_ID,
                feature_set_get_feature_id_params(index),
            )?;
            let row = parse_feature_id(&payload)?;
            self.table.insert(index, row)?;
        }
        Ok(())
    }

    /// Sends one request to a wire feature index and returns the 16 reply
    /// payload bytes. Refused while the table is invalid.
    pub fn request(
        &mut self,
        feature_index: u8,
        function: u8,
        params: [u8; 16],
    ) -> Result<[u8; 16], SessionError> {
        if self.needs_rediscovery {
            return Err(SessionError::NeedsRediscovery);
        }
        let policy = self.policy;
        self.exchange(&policy, feature_index, function, params)
    }

    /// Resolves `feature` through the table and sends the request. Errors with
    /// [`SessionError::FeatureAbsent`] when the device lacks the feature.
    pub fn request_feature(
        &mut self,
        feature: FeatureId,
        function: u8,
        params: [u8; 16],
    ) -> Result<[u8; 16], SessionError> {
        if self.needs_rediscovery {
            return Err(SessionError::NeedsRediscovery);
        }
        let index = self
            .table
            .index_of(feature)
            .ok_or(SessionError::FeatureAbsent(feature))?;
        let policy = self.policy;
        self.exchange(&policy, index, function, params)
    }

    /// The only entry point for Set* calls: resolves `feature` through the
    /// table and sends the request through a call-local policy with the
    /// volatile tier enabled (`Policy::with_volatile_writes`), so exactly
    /// 0x2201 function 3, 0x8060 function 2 and 0x8100 function 1 (SetOnboardMode)
    /// with bounds-checked parameters pass, and everything else (other 0x8100
    /// writes, hidden features, DFU, bad parameters) is refused before a byte is sent. These calls change RAM
    /// state of the device, never flash.
    ///
    /// Callers must have the user's consent for the write. The session's own
    /// policy stays read-only; nothing is stored between calls.
    pub fn request_volatile(
        &mut self,
        feature: FeatureId,
        function: u8,
        params: [u8; 16],
    ) -> Result<[u8; 16], SessionError> {
        if self.needs_rediscovery {
            return Err(SessionError::NeedsRediscovery);
        }
        let index = self
            .table
            .index_of(feature)
            .ok_or(SessionError::FeatureAbsent(feature))?;
        let policy = Policy::default().with_volatile_writes();
        self.exchange(&policy, index, function, params)
    }

    /// The only entry point for flash writes: sends one [`SectorImage`] as
    /// 0x8100 BeginWrite (fn 6), 16 WriteData (fn 7) and EndWrite (fn 8). The
    /// frames are built here from the image, each through `Policy::check_indexed`
    /// with a call-local `Policy::default().with_onboard_writes()`, so the
    /// policy still bounds the BeginWrite sector (allowlist, ROM denied), offset
    /// and length. The raw per-frame path is private: nothing outside this
    /// module can send fn 6, 7 or 8 with chosen bytes.
    ///
    /// fn 6 and fn 8 wait up to [`FLASH_COMMIT_TIMEOUT`] for their reply. The
    /// first error stops the WriteData frames, but EndWrite is still sent (G HUB
    /// behaviour) unless the error means the link is down (a 0x41 or
    /// `NeedsRediscovery`, [`link_lost`]): then nothing more is sent. Returns the
    /// reply payloads seen so far and the first error, if any. The caller must
    /// have the user's explicit approval and should read the sector back.
    ///
    /// The raw per-frame method `onboard_write_frame` exists but is private,
    /// which these doctests pin (the first compiles, the second must fail with
    /// "method is private", E0624, which rustdoc checks on nightly):
    ///
    /// ```
    /// use hidpp::onboard_write::SectorImage;
    /// use myhub_transport::{HidLink, HidppSession};
    /// fn f<L: HidLink>(s: &mut HidppSession<L>, image: &SectorImage) {
    ///     let _ = s.write_sector(image);
    /// }
    /// ```
    ///
    /// ```compile_fail,E0624
    /// use hidpp::FeatureId;
    /// use myhub_transport::{HidLink, HidppSession};
    /// fn f<L: HidLink>(s: &mut HidppSession<L>) {
    ///     let _ = s.onboard_write_frame(FeatureId::ONBOARD_PROFILES, 6, [0; 16], None);
    /// }
    /// ```
    pub fn write_sector(&mut self, image: &SectorImage) -> (WriteLog, Result<(), SessionError>) {
        let mut log = WriteLog::default();
        let mut failure: Option<SessionError> = None;
        let feature = FeatureId::ONBOARD_PROFILES;

        // Offset 0 and length 255 are constants of the image type, so the
        // parameters cannot be refused here; the policy checks them again.
        let begin = begin_write_params(image.sector(), 0, SECTOR_LEN as u16)
            .expect("constant offset and length are valid");
        match self.onboard_write_frame(feature, FN_BEGIN_WRITE, begin, Some(FLASH_COMMIT_TIMEOUT)) {
            Ok(reply) => log.begin = Some(reply),
            Err(e) => failure = Some(e),
        }
        if failure.is_none() {
            for chunk in write_chunks(image.bytes()) {
                match self.onboard_write_frame(feature, FN_WRITE_DATA, chunk, None) {
                    Ok(reply) => log.data.push(reply),
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
        }
        // EndWrite is always sent, also after an error, except when the link
        // is down: nothing can be sent then.
        if failure.as_ref().is_some_and(link_lost) {
            return (log, Err(failure.take().expect("checked above")));
        }
        match self.onboard_write_frame(
            feature,
            FN_END_WRITE,
            end_write_params(),
            Some(FLASH_COMMIT_TIMEOUT),
        ) {
            Ok(reply) => log.end = Some(reply),
            Err(e) => {
                failure.get_or_insert(e);
            }
        }
        (log, failure.map_or(Ok(()), Err))
    }

    /// One fn 6, 7 or 8 frame (private). Any other feature or function is
    /// refused before a byte is sent. `reply_timeout` replaces the session's
    /// reply timeout for this call only; `None` keeps the session's.
    fn onboard_write_frame(
        &mut self,
        feature: FeatureId,
        function: u8,
        params: [u8; 16],
        reply_timeout: Option<Duration>,
    ) -> Result<[u8; 16], SessionError> {
        if feature != FeatureId::ONBOARD_PROFILES || !(6..=8).contains(&function) {
            return Err(SessionError::Policy(PolicyError::NotAllowlisted {
                feature_id: feature,
                function,
            }));
        }
        if self.needs_rediscovery {
            return Err(SessionError::NeedsRediscovery);
        }
        let index = self
            .table
            .index_of(feature)
            .ok_or(SessionError::FeatureAbsent(feature))?;
        let policy = Policy::default().with_onboard_writes();
        let saved = self.reply_timeout;
        if let Some(timeout) = reply_timeout {
            self.set_reply_timeout(timeout);
        }
        let result = self.exchange(&policy, index, function, params);
        self.reply_timeout = saved;
        result
    }

    /// The only entry point for profile selection: 0x8100 SetCurrentProfile
    /// (fn 3) through a call-local `Policy::default().with_profile_select()`.
    /// The typed `sector` (1..=5) becomes BE16 in bytes 0..1, zeros after; the
    /// policy checks it again. Changes which profile the mouse runs; callers need
    /// the user's approval.
    pub fn request_profile_select(
        &mut self,
        sector: SelectableSector,
    ) -> Result<[u8; 16], SessionError> {
        let params = select_profile_params(sector.get()).expect("a SelectableSector is valid");
        if self.needs_rediscovery {
            return Err(SessionError::NeedsRediscovery);
        }
        let feature = FeatureId::ONBOARD_PROFILES;
        let index = self
            .table
            .index_of(feature)
            .ok_or(SessionError::FeatureAbsent(feature))?;
        let policy = Policy::default().with_profile_select();
        self.exchange(&policy, index, 3, params)
    }

    /// The only entry point for the button spy: 0x8110 fn 1 (start) or fn 2
    /// (stop), no parameters, through a call-local
    /// `Policy::default().with_button_spy()`. The spy is RAM-only; while it runs
    /// the mouse sends button notifications. Callers should always send the stop.
    pub fn request_button_spy(&mut self, start: bool) -> Result<[u8; 16], SessionError> {
        if self.needs_rediscovery {
            return Err(SessionError::NeedsRediscovery);
        }
        let feature = FeatureId::MOUSE_BUTTON_SPY;
        let index = self
            .table
            .index_of(feature)
            .ok_or(SessionError::FeatureAbsent(feature))?;
        let policy = Policy::default().with_button_spy();
        self.exchange(&policy, index, if start { 1 } else { 2 }, [0; 16])
    }

    /// Policy check, drain, write, then wait for the matching reply.
    ///
    /// Frames already queued on the link are drained before the write: they
    /// cannot be replies to this request, and some replies (ReadData) do not
    /// echo their parameters, so a late reply to an earlier request would
    /// otherwise be taken for this one. The link is drained again after a
    /// timeout or a table invalidation, for the same reason.
    fn exchange(
        &mut self,
        policy: &Policy,
        feature_index: u8,
        function: u8,
        params: [u8; 16],
    ) -> Result<[u8; 16], SessionError> {
        let checked = policy.check_indexed(
            &self.table,
            self.device_index,
            feature_index,
            function,
            self.sw_id,
            params,
        )?;
        // A 0x41 found here invalidates the table, and `checked` was resolved
        // against the old one, so nothing is sent.
        self.drain_link()?;
        let frame = checked.encode();
        if let Some(trace) = self.trace {
            trace(FrameDirection::Tx, &frame);
        }
        self.link.write_report(&frame)?;
        self.request_count += 1;

        let result = self.await_reply(&checked);
        if matches!(
            result,
            Err(SessionError::Timeout | SessionError::NeedsRediscovery)
        ) {
            // Late frames must not match the next request. A failure here is
            // not worth masking the original result; the next write reports it.
            let _ = self.drain_link();
        }
        result
    }

    /// Reads and routes everything currently queued on the link (timeout 0).
    /// Returns `NeedsRediscovery` when a 0x41 was among the frames, after the
    /// queue is empty (or [`DRAIN_LIMIT`] frames were read).
    fn drain_link(&mut self) -> Result<(), SessionError> {
        let mut buf = [0u8; READ_BUF_LEN];
        let mut connection_seen = false;
        for _ in 0..DRAIN_LIMIT {
            let n = self.link.read_report(&mut buf, 0)?.min(READ_BUF_LEN);
            if n == 0 {
                break;
            }
            let Ok(message) = decode(&buf[..n]) else {
                continue;
            };
            connection_seen |= self.route_unmatched(message).is_err();
        }
        if connection_seen {
            Err(SessionError::NeedsRediscovery)
        } else {
            Ok(())
        }
    }

    /// Listens to the link for up to `timeout` and routes every frame like the
    /// drain and wait loops do: notifications go to the buffer
    /// ([`Self::drain_notifications`]), a HID++ 1.0 0x41 clears the feature
    /// table, sets the needs-rediscovery state and updates
    /// [`Self::link_state`], and replies to anyone's requests are dropped.
    ///
    /// Nothing is ever written, so this works with the table invalid. Returns
    /// as soon as the frames that were waiting have been routed, or when the
    /// timeout elapsed; only a link error is an error (a 0x41 is not, check
    /// [`Self::needs_rediscovery`]).
    pub fn poll_events(&mut self, timeout: Duration) -> Result<(), SessionError> {
        let start = Instant::now();
        let deadline = start.checked_add(timeout).unwrap_or(start);
        let mut buf = [0u8; READ_BUF_LEN];
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(());
            }
            let remaining_ms = (deadline - now).as_millis().max(1);
            let wait = i32::try_from(remaining_ms.min(POLL_INTERVAL_MS as u128))
                .unwrap_or(POLL_INTERVAL_MS);
            let n = self.link.read_report(&mut buf, wait)?.min(READ_BUF_LEN);
            if n == 0 {
                continue;
            }
            if let Ok(message) = decode(&buf[..n]) {
                // A 0x41 is recorded by the router; poll_events has no
                // request to abort, so the signal itself is not an error.
                let _ = self.route_unmatched(message);
            }
            // Something arrived: route what else is queued without waiting
            // and hand control back so the caller can react.
            for _ in 0..DRAIN_LIMIT {
                let n = self.link.read_report(&mut buf, 0)?.min(READ_BUF_LEN);
                if n == 0 {
                    break;
                }
                if let Ok(message) = decode(&buf[..n]) {
                    let _ = self.route_unmatched(message);
                }
            }
            return Ok(());
        }
    }

    /// Handles a frame that is not the reply being waited for. A HID++ 1.0
    /// 0x41 invalidates the table and returns `NeedsRediscovery`.
    fn route_unmatched(&mut self, message: Message) -> Result<(), SessionError> {
        match message {
            Message::Notification { .. } => self.push_notification(message),
            Message::Hidpp10 {
                device_index,
                sub_id: HIDPP10_DEVICE_CONNECTION,
                payload,
            } => {
                self.push_notification(message);
                // Another device behind the receiver says nothing about our
                // table or link: the notification is kept, nothing else.
                if device_index != self.device_index {
                    return Ok(());
                }
                self.link_up = Some(payload[1] & LINK_DOWN_BIT == 0);
                self.invalidate_table();
                return Err(SessionError::NeedsRediscovery);
            }
            // A 0x8F error answering somebody else's request.
            Message::Hidpp10 {
                sub_id: HIDPP10_ERROR_SUB_ID,
                ..
            } => {}
            Message::Hidpp10 { .. } => self.push_notification(message),
            // Replies to other software IDs, devices or features, or stale
            // replies found while draining.
            Message::Response { .. } | Message::Error { .. } => {}
        }
        Ok(())
    }

    /// Reads until the matching reply, a device error, a table invalidation or
    /// the deadline.
    fn await_reply(&mut self, checked: &CheckedRequest) -> Result<[u8; 16], SessionError> {
        let start = Instant::now();
        // Cannot overflow after the clamp in `set_reply_timeout`; if it ever
        // did, failing as an immediate timeout is the safe direction.
        let deadline = start.checked_add(self.reply_timeout).unwrap_or(start);
        let mut buf = [0u8; READ_BUF_LEN];
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(SessionError::Timeout);
            }
            let remaining_ms = (deadline - now).as_millis().max(1);
            let wait = i32::try_from(remaining_ms.min(POLL_INTERVAL_MS as u128))
                .unwrap_or(POLL_INTERVAL_MS);
            let n = self.link.read_report(&mut buf, wait)?.min(READ_BUF_LEN);
            if n == 0 {
                continue;
            }
            if let Some(trace) = self.trace {
                trace(FrameDirection::Rx, &buf[..n]);
            }
            // Reports that are neither 0x10 nor 0x11, or have a bad length,
            // are not HID++ and are skipped.
            let Ok(message) = decode(&buf[..n]) else {
                continue;
            };

            if is_reply_to(checked, &message) {
                return match message {
                    Message::Response { payload, .. } => Ok(payload),
                    Message::Error { code, .. } => Err(SessionError::Device {
                        code,
                        family: ErrorFamily::Hidpp20,
                        name: hidpp20_error_name(code),
                    }),
                    Message::Hidpp10 { payload, .. } => Err(SessionError::Device {
                        code: payload[2],
                        family: ErrorFamily::Hidpp10,
                        name: None,
                    }),
                    // is_reply_to is false for notifications.
                    Message::Notification { .. } => continue,
                };
            }
            self.route_unmatched(message)?;
        }
    }
}

impl HidppSession<HidapiLink> {
    /// Opens the receiver and starts a read-tier session on it. The hidapi
    /// link is built here and never exposed, so nothing but this session can
    /// write to the device. Run [`Self::discover`] next.
    pub fn open(api: &HidApi, receiver: &ReceiverPath) -> Result<Self, LinkError> {
        Ok(Self::new(HidapiLink::open(api, receiver)?))
    }
}
