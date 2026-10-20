//! Keychron K2 HE driver over USB raw HID (reads, plus profile select in `writes`).
//!
//! [`Session`] runs the `keychron-proto` protocol over any [`RawLink`]: one frame in
//! flight, every frame from `Policy::check` -> `CheckedRequest::encode`, reads resent up
//! to two times after a timeout (once inside a write sequence), writes never resent. The stored policy is read-only; the
//! writes ([`select_profile`], [`set_travel_global`], [`set_lighting`], [`set_keycode`]) open one tier per call on a copy.
//! [`read_snapshot`] reads the whole keyboard state.
//!
//! Another configurator (the Keychron launcher) talking to the keyboard at the same time
//! cannot be told apart from our own replies, because frames carry no sequence number.
//! The session treats any unexpected report as foreign traffic, fails the request and
//! backs off for 5 seconds. See [`SessionError::ForeignTraffic`].
//!
//! [`Session::open_receiver`] opens the Keychron Link receiver instead (read only: B1..B3,
//! see [`read_link`]); a session has a fixed [`keychron_proto::Target`] and refuses
//! requests meant for the other device before any byte is written.
//!
//! The serial number and the device path are never read into output. Evidence tags
//! as in keychron-proto: `[OBS]` seen on the keyboard, `[INF]` inference, `[UNK]` gap.

mod cache;
mod detail;
mod discovery;
mod error;
mod hid;
mod keymap;
mod light;
mod link;
mod linkrx;
mod session;
mod snapshot;
mod travel;
mod writes;

pub use cache::{KeyboardCache, cache_path};
pub use detail::{
    PROTOCOL_LINK, TRANSPORT_RECEIVER, TRANSPORT_USB, cache_of, to_detail, to_receiver_detail,
};
pub use discovery::{InterfaceInfo, Presence, list_interfaces, list_receiver_interfaces, presence};
pub use error::SessionError;
pub use hid::HidapiLink;
pub use keymap::{KeycodeOutcome, set_keycode};
pub use light::{
    LightChange, LightOutcome, LightState, read_light_state, set_lighting, set_lighting_with,
};
pub use link::{FRAME_LEN, LinkError, READ_BUF_LEN, REPORT_LEN, RawLink};
pub use linkrx::{LinkSnapshot, read_link, read_link_state};
pub use session::{
    Event, FOREIGN_BACKOFF, FrameDirection, FrameTrace, Incoming, MIN_REPLY_TIMEOUT, READ_RETRIES,
    REPLY_TIMEOUT, Session, WRITE_GAP,
};
pub use snapshot::{
    LightSettings, Lighting, RegionSlots, SectionNote, Snapshot, VIA_PROTOCOL, read_snapshot,
    refresh_state,
};
pub use travel::{
    DEFAULT_SENSITIVITY, TravelOutcome, TravelSettings, read_global_word, set_travel_global,
    set_travel_global_with,
};
pub use writes::{SaveMode, SelectOutcome, read_profiles_info, select_profile};

/// Re-exported for callers that build a [`TravelSettings`] or patch a cached global word.
pub use keychron_proto::TravelMode;
pub use keychron_proto::he::KeyWord;

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
/// USB product ID of the Keychron Link receiver (2.4 GHz) [OBS]. Over the receiver only
/// B1..B3 get answers; every keyboard frame times out [OBS].
pub const LINK_PID: u16 = 0xD030;
/// Interface number of the receiver's raw HID collection for backends that report usage
/// page 0 (Linux hidraw). [OBS] interface 3 on one Windows PC, [UNK] on Linux, so the
/// fallback refuses.
pub const LINK_RAW_HID_INTERFACE: Option<i32> = None;
/// Key matrix rows, from the keyboard definition [OBS layout from the keymap read: 96
/// slots per layer]. VIA has no matrix query.
pub const MATRIX_ROWS: u8 = 6;
/// Key matrix columns, see [`MATRIX_ROWS`].
pub const MATRIX_COLS: u8 = 16;
