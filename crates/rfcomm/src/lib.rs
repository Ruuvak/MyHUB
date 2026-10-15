//! Bluetooth Classic RFCOMM byte transport.
//!
//! [`ByteLink`] is the only thing protocol code sees: write bytes, read bytes with a
//! timeout. [`list_paired`], [`offers_service`] and [`connect`] find a paired device
//! and open an RFCOMM socket to a service UUID. Windows only for now (Winsock
//! `AF_BTH`, the channel is resolved by Windows through SDP); on other targets every
//! function returns [`RfcommError::Unsupported`] (Linux/BlueZ comes later).
//!
//! Device addresses are opaque ([`BtAddress`]): `Debug` and `Display` show only the last
//! byte, so an address cannot end up in a log by accident. [`MockLink`] replays scripted
//! bytes for tests; nothing in the tests opens a device.

mod mock;
#[cfg(not(windows))]
mod unsupported;
#[cfg(windows)]
mod windows;

use std::fmt;

use thiserror::Error;

pub use mock::MockLink;
#[cfg(not(windows))]
pub use unsupported::{RfcommLink, connect, list_paired, offers_service};
#[cfg(windows)]
pub use windows::{RfcommLink, connect, list_paired, offers_service};

/// Why a transport call failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RfcommError {
    /// This target has no RFCOMM backend yet.
    #[error("Bluetooth RFCOMM is not supported on this platform")]
    Unsupported,
    /// The link was closed, aborted or reset by the peer.
    #[error("Bluetooth link disconnected")]
    Disconnected,
    /// An OS call failed. `code` is the Windows error code.
    #[error("{operation} failed (OS error {code})")]
    Os { operation: &'static str, code: i32 },
}

/// A bidirectional byte pipe to the headset.
pub trait ByteLink {
    /// Write all bytes or fail.
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), RfcommError>;

    /// Read up to `buf.len()` bytes, waiting at most `timeout_ms`. `Ok(0)` means the
    /// timeout passed with no data; a dropped link is [`RfcommError::Disconnected`].
    fn read(&mut self, buf: &mut [u8], timeout_ms: u32) -> Result<usize, RfcommError>;
}

/// A Bluetooth device address that never prints in full.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BtAddress(u64);

impl BtAddress {
    /// `raw` holds the 48 address bits, most significant byte first when printed.
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    pub(crate) fn new(raw: u64) -> Self {
        Self(raw & 0xFFFF_FFFF_FFFF)
    }

    #[cfg(windows)]
    pub(crate) fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for BtAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "**:**:**:**:**:{:02X}", self.0 & 0xFF)
    }
}

impl fmt::Debug for BtAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BtAddress({self})")
    }
}

/// A 128 bit service UUID in the Windows GUID field layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceUuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// A paired (remembered) Bluetooth device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    pub name: String,
    pub connected: bool,
    address: BtAddress,
}

impl PairedDevice {
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    pub(crate) fn new(name: String, address: BtAddress, connected: bool) -> Self {
        Self {
            name,
            connected,
            address,
        }
    }

    /// The masked address, for display.
    pub fn address(&self) -> BtAddress {
        self.address
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_display_and_debug_show_only_the_last_byte() {
        let a = BtAddress::new(0x0011_2233_44FC);
        assert_eq!(a.to_string(), "**:**:**:**:**:FC");
        let d = PairedDevice::new("x".into(), a, false);
        assert_eq!(format!("{a:?}"), "BtAddress(**:**:**:**:**:FC)");
        assert_eq!(
            format!("{d:?}"),
            "PairedDevice { name: \"x\", connected: false, address: BtAddress(**:**:**:**:**:FC) }"
        );
    }
}
