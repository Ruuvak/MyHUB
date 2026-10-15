//! Sony WF-C710N driver.
//!
//! [`Session`] runs the `mdr` link over any [`myhub_rfcomm::ByteLink`]: it ACKs
//! everything the headset sends, absorbs support function and capability replies and
//! sends reads through `mdr::Policy::new()` (the read tier). The write calls of
//! [`Session`] (`set_nc_asm`, `select_eq_preset`, `set_eq_bands`, `set_sidetone`,
//! `set_multipoint`) each build a policy with only the gate that call needs; the session
//! never keeps an opened gate. [`read_snapshot`] runs the handshake and reads the state.
//! The headset address is never stored in this repository: [`candidates`] finds the
//! device at runtime among the paired devices.
//!
//! The CONNECT capability info reply carries the headset address, so it is never
//! requested.

mod describe;
mod detail;
mod session;
mod snapshot;
mod writes;

use myhub_rfcomm::{PairedDevice, RfcommError, RfcommLink, ServiceUuid, connect};

pub use describe::describe;
pub use detail::to_detail;
pub use mdr::messages::{AmbientMode, NcAsmMode};
pub use session::{READ_POLL_MS, REPLY_TIMEOUT, Session, SessionError, VERIFY_WAIT};
pub use snapshot::{
    Applied, EqSnapshot, Identity, Item, NcAsmSnapshot, Snapshot, handshake, read_snapshot, reread,
};
pub use writes::{ALERT_WAIT, Change, DROP_WAIT, EqBandsChange, MultipointOutcome, WriteError};

/// Stable device ID used by the UI and the app's per-device files.
pub const DEVICE_ID: &str = "sony-wf-c710n";
/// Model name the headset reports, also used to find it among the paired devices.
pub const MODEL_NAME: &str = "WF-C710N";
/// The MDR service UUID 956C7B26-D49A-4BA8-B03F-B17D393CB6E2.
pub const MDR_SERVICE_UUID: ServiceUuid = ServiceUuid {
    data1: 0x956C_7B26,
    data2: 0xD49A,
    data3: 0x4BA8,
    data4: [0xB0, 0x3F, 0xB1, 0x7D, 0x39, 0x3C, 0xB6, 0xE2],
};

/// True when a paired device name looks like this headset.
pub fn name_matches(name: &str) -> bool {
    name.to_ascii_uppercase().contains(MODEL_NAME)
}

/// Paired devices that are this headset: the name matches, or Windows lists the MDR
/// service for them. Each entry says why it matched.
pub fn candidates() -> Result<Vec<(PairedDevice, Match)>, RfcommError> {
    let mut out = Vec::new();
    for device in myhub_rfcomm::list_paired()? {
        let by_name = name_matches(&device.name);
        let by_service = myhub_rfcomm::offers_service(&device, &MDR_SERVICE_UUID).unwrap_or(false);
        if by_name || by_service {
            out.push((
                device,
                Match {
                    by_name,
                    by_service,
                },
            ));
        }
    }
    Ok(out)
}

/// Why a paired device is a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub by_name: bool,
    pub by_service: bool,
}

/// Open an RFCOMM connection to `device` and wrap it in a session.
pub fn open(device: &PairedDevice) -> Result<Session<RfcommLink>, SessionError> {
    let link = connect(device, &MDR_SERVICE_UUID)?;
    Ok(Session::new(link))
}
