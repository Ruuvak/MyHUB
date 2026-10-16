//! Keychron K2 HE driver over USB raw HID (reads, plus profile select in `writes`).
//!
//! [`Session`] runs the `keychron-proto` protocol over any [`RawLink`]: one frame in
//! flight, every frame from `Policy::check` -> `CheckedRequest::encode`, reads resent up
//! to two times after a timeout, writes never resent. The stored policy is read-only; the
//! only write is [`select_profile`], which opens one tier per call on a copy.
//! [`read_snapshot`] reads the whole keyboard state.
//!
//! Another configurator (the Keychron launcher) talking to the keyboard at the same time
//! cannot be told apart from our own replies, because frames carry no sequence number.
//! The session treats any unexpected report as foreign traffic, fails the request and
//! backs off for 5 seconds. See [`SessionError::ForeignTraffic`].
//!
//! The serial number and the device path are never read into output. Evidence tags
//! as in keychron-proto: `[OBS]` seen on the keyboard, `[INF]` inference, `[UNK]` gap.

mod detail;
mod discovery;
mod error;
mod hid;
mod link;
mod session;
mod snapshot;
mod writes;

pub use detail::to_detail;
pub use discovery::{InterfaceInfo, list_interfaces};
pub use error::SessionError;
pub use hid::HidapiLink;
pub use link::{FRAME_LEN, LinkError, READ_BUF_LEN, REPORT_LEN, RawLink};
pub use session::{
    Event, FOREIGN_BACKOFF, FrameDirection, FrameTrace, Incoming, MIN_REPLY_TIMEOUT, READ_RETRIES,
    REPLY_TIMEOUT, Session, WRITE_GAP,
};
pub use snapshot::{
    LightSettings, Lighting, RegionSlots, SectionNote, Snapshot, VIA_PROTOCOL, read_snapshot,
    refresh_state,
};
pub use writes::{SelectOutcome, read_profiles_info, select_profile};

/// Stable device ID used by the UI and the app's per-device files.
pub const DEVICE_ID: &str = "keychron-k2he";
/// Name shown in the UI.
pub const DISPLAY_NAME: &str = "K2 HE";
/// USB vendor ID (Keychron) [OBS].
pub const VID: u16 = 0x3434;
/// USB product ID of the K2 HE over the cable [OBS].
pub const PID: u16 = 0x0E20;
/// Raw HID usage page [OBS].
pub const USAGE_PAGE: u16 = 0xFF60;
/// Raw HID usage [OBS].
pub const USAGE: u16 = 0x61;
/// USB interface number of the raw HID collection, for backends that report usage page 0
/// (Linux hidraw). [UNK] not known yet, so the fallback refuses.
pub const RAW_HID_INTERFACE: Option<i32> = None;
/// Key matrix rows, from the keyboard definition [OBS layout from the keymap read: 96
/// slots per layer]. VIA has no matrix query.
pub const MATRIX_ROWS: u8 = 6;
/// Key matrix columns, see [`MATRIX_ROWS`].
pub const MATRIX_COLS: u8 = 16;
