//! Logitech G502 X LIGHTSPEED driver (F2+).
//!
//! [`read_snapshot`] sends only read-tier calls. [`set_dpi`] and [`set_report_rate`] are
//! the only writers: volatile (RAM) settings through `HidppSession::request_volatile`,
//! never flash. [`write_profile_sector`] and [`restore_sector`] are the only flash
//! writers (whole user profile sectors, `HidppSession::write_sector`).

mod detail;
mod onboard_write;
mod snapshot;
mod spy;
mod write;

pub use detail::{battery_info, parse_battery_notification, parse_battery_status, to_detail};
pub use onboard_write::{
    BackupFn, FLASH_COMMIT_TIMEOUT, FailureDetail, ProfileSource, ReadbackCheck, RestoreStatus,
    SaveError, SaveReport, SectorWriteError, SequenceLog, WriteOutcome, WriteReport,
    restore_sector, save_active_profile, select_profile, write_directory, write_profile_sector,
};
pub use snapshot::{
    BatterySnapshot, DpiSnapshot, FeatureRow, OnboardSnapshot, ParsedProfile, RateSnapshot,
    ReadError, Snapshot, StepStat, read_snapshot,
};
pub use spy::{SpyError, SpyLine, SpyReport, describe_spy_payload, spy_capture, spy_stop};
pub use write::{WriteError, onboard_mode, set_dpi, set_onboard_mode, set_report_rate};

/// Stable device ID used by the UI and the app's per-device files (no serial
/// numbers).
pub const DEVICE_ID: &str = "logitech-g502x";
/// Receiver USB product ID (Lightspeed receiver, observed).
pub const RECEIVER_PID: u16 = 0xC547;
/// Wireless product ID of the mouse behind the receiver (observed).
pub const WIRELESS_PID: u16 = 0x409F;
/// HID++ device index of the mouse on the receiver (observed).
pub const DEVICE_INDEX: u8 = 1;

/// A read-tier session configured for this device (device index
/// [`DEVICE_INDEX`], default software ID). The caller must still run
/// `discover()`; [`read_snapshot`] does that itself.
pub fn new_session<L: myhub_transport::HidLink>(link: L) -> myhub_transport::HidppSession<L> {
    let mut session = myhub_transport::HidppSession::new(link);
    session.set_device_index(DEVICE_INDEX);
    session
}
