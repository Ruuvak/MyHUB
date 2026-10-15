//! HID transport over hidapi (F2).
//!
//! * [`discovery`]: finding the receiver's HID++ interface. The matching is a
//!   pure function over plain structs so it is testable without hidapi.
//! * [`session`]: [`HidppSession`], which builds every frame through
//!   `hidpp::Policy::check_indexed` and matches replies by software ID.
//!
//! Safety: a [`HidLink`] is a raw pipe with no policy of its own. The real
//! hidapi link is not constructible outside this crate: the only way to get
//! one is `HidppSession::open`, and the session offers no access to its link.
//! The session itself is read-tier only. The only write-capable call is
//! `HidppSession::request_volatile`, which passes exactly 0x2201 fn3,
//! 0x8060 fn2 and 0x8100 fn1 (RAM settings and the host/onboard switch, never
//! flash) and needs the user's consent.

pub mod discovery;
mod hidapi_link;
mod link;
pub mod session;

/// Enumeration handle that `find_receivers` and `HidppSession::open` take.
pub use hidapi::HidApi;

pub use discovery::{
    HidCandidate, LOGITECH_VID, RECEIVER_PIDS, ReceiverPath, UsageRule, find_receivers,
    list_candidates, select_receivers,
};
/// Name of the real link type, so applications can write `HidppSession<HidapiLink>`.
/// It has no public constructor: only `HidppSession::open` builds one.
pub use hidapi_link::HidapiLink;
pub use link::{HidLink, LinkError};
pub use session::{
    DEFAULT_REPLY_TIMEOUT, ErrorFamily, FLASH_COMMIT_TIMEOUT, FrameDirection, FrameTrace,
    HidppSession, MAX_REPLY_TIMEOUT, MIN_REPLY_TIMEOUT, NOTIFICATION_CAPACITY, POLL_INTERVAL_MS,
    SessionError, WriteLog, connection_notification, hidpp20_error_name, link_lost,
};

/// A session over the real receiver, as returned by `HidppSession::open`.
pub type HidapiSession = HidppSession<hidapi_link::HidapiLink>;
