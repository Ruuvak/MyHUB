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

use myhub_rfcomm::{BtAddress, PairedDevice, RfcommError, RfcommLink, ServiceUuid, connect};

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

/// Finds the headset for the background worker with as little radio use as possible.
///
/// Unlike [`candidates`] it matches by name first and asks for the MDR service
/// (SDP, a radio operation on Windows) only for a device that is connected, did not
/// match by name and was not asked before in this run.
#[derive(Debug, Default)]
pub struct Finder {
    /// Address and the answer of `offers_service`, one entry per device asked.
    asked: Vec<(BtAddress, bool)>,
}

impl Finder {
    pub fn new() -> Self {
        Self::default()
    }

    /// The headset among the paired devices, `None` when there is none. The device
    /// may report `connected == false`: then nothing may be opened.
    pub fn find(&mut self) -> Result<Option<PairedDevice>, RfcommError> {
        self.find_with(myhub_rfcomm::list_paired, |d| {
            myhub_rfcomm::offers_service(d, &MDR_SERVICE_UUID).ok()
        })
    }

    /// [`Finder::find`] with the listing and the service query injected.
    pub fn find_with(
        &mut self,
        list: impl FnOnce() -> Result<Vec<PairedDevice>, RfcommError>,
        mut offers: impl FnMut(&PairedDevice) -> Option<bool>,
    ) -> Result<Option<PairedDevice>, RfcommError> {
        let devices = list()?;
        // A connected name match wins over a disconnected one.
        let named = devices
            .iter()
            .position(|d| name_matches(&d.name) && d.connected)
            .or_else(|| devices.iter().position(|d| name_matches(&d.name)));
        if let Some(i) = named {
            return Ok(devices.into_iter().nth(i));
        }
        for device in devices.into_iter().filter(|d| d.connected) {
            let address = device.address();
            let known = self.asked.iter().find(|(a, _)| *a == address).map(|e| e.1);
            let offered = match known {
                Some(answer) => answer,
                // An error is not an answer: ask again on the next run.
                None => match offers(&device) {
                    Some(answer) => {
                        self.asked.push((address, answer));
                        answer
                    }
                    None => false,
                },
            };
            if offered {
                return Ok(Some(device));
            }
        }
        Ok(None)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, tag: u64, connected: bool) -> PairedDevice {
        PairedDevice::new(name.to_owned(), BtAddress::new(tag), connected)
    }

    #[test]
    fn the_service_is_asked_only_for_connected_devices_that_did_not_match_by_name() {
        let mut finder = Finder::new();
        let mut asked = Vec::new();
        let list = || {
            Ok(vec![
                device("Phone", 1, false),
                device("Pad", 2, true),
                device("Buds", 3, true),
            ])
        };
        let found = finder
            .find_with(list, |d| {
                asked.push(d.address());
                Some(d.address() == BtAddress::new(3))
            })
            .unwrap();
        assert_eq!(found.map(|d| d.address()), Some(BtAddress::new(3)));
        // The disconnected phone is never asked.
        assert_eq!(asked, vec![BtAddress::new(2), BtAddress::new(3)]);
    }

    #[test]
    fn a_connected_name_match_beats_a_disconnected_one_and_errors_are_not_cached() {
        let mut finder = Finder::new();
        let list = || {
            Ok(vec![
                device("Sony WF-C710N", 1, false),
                device("Sony WF-C710N", 2, true),
            ])
        };
        let found = finder.find_with(list, |_| None).unwrap().unwrap();
        assert_eq!(found.address(), BtAddress::new(2));
        let mut calls = 0;
        for _ in 0..2 {
            let r = finder
                .find_with(
                    || Ok(vec![device("Pad", 5, true)]),
                    |_| {
                        calls += 1;
                        None
                    },
                )
                .unwrap();
            assert!(r.is_none());
        }
        assert_eq!(calls, 2, "a failed query is asked again");
    }

    #[test]
    fn the_service_answer_is_cached_and_a_name_match_needs_no_query() {
        let mut finder = Finder::new();
        let mut calls = 0;
        let list = || Ok(vec![device("Pad", 2, true), device("Buds", 3, true)]);
        for _ in 0..3 {
            assert!(
                finder
                    .find_with(list, |_| {
                        calls += 1;
                        Some(false)
                    })
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(calls, 2, "each device once per run");
        let named = vec![device("Pad", 2, true), device("Sony WF-C710N", 4, false)];
        let found = finder
            .find_with(|| Ok(named), |_| panic!("no query for a name match"))
            .unwrap();
        assert_eq!(found.map(|d| d.connected), Some(false));
    }
}
