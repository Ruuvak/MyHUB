//! Pure helpers for the BlueZ backend: address and UUID text forms and the mapping of
//! `org.bluez.Device1` properties to a [`PairedDevice`]. No I/O and no D-Bus types, so
//! they are testable anywhere.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::{BtAddress, PairedDevice, ServiceUuid};

/// Parse `"AA:BB:CC:DD:EE:FF"` into the 48 address bits, first printed byte on top.
pub fn parse_address(text: &str) -> Option<u64> {
    let mut raw = 0u64;
    let mut parts = 0;
    for part in text.split(':') {
        if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        raw = (raw << 8) | u64::from(u8::from_str_radix(part, 16).ok()?);
        parts += 1;
    }
    (parts == 6).then_some(raw)
}

/// The kernel's `bdaddr_t` byte order: least significant byte first.
pub fn bdaddr_bytes(raw: u64) -> [u8; 6] {
    let b = raw.to_le_bytes();
    [b[0], b[1], b[2], b[3], b[4], b[5]]
}

/// The canonical lowercase text form BlueZ uses in the `UUIDs` property.
pub fn uuid_text(u: &ServiceUuid) -> String {
    let d = u.data4;
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        u.data1, u.data2, u.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

/// The 16 big endian bytes of the UUID, the order SDP puts on the wire.
pub fn uuid_bytes(u: &ServiceUuid) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&u.data1.to_be_bytes());
    out[4..6].copy_from_slice(&u.data2.to_be_bytes());
    out[6..8].copy_from_slice(&u.data3.to_be_bytes());
    out[8..].copy_from_slice(&u.data4);
    out
}

/// True when `listed` (a `UUIDs` property) contains `wanted` (compared case blind).
pub fn lists_uuid(listed: &[String], wanted: &str) -> bool {
    listed.iter().any(|u| u.eq_ignore_ascii_case(wanted))
}

/// Build a [`PairedDevice`] from the `Device1` properties; `None` when the device is not
/// paired or its address is unreadable. The name is `Name`, else `Alias`, else empty.
pub fn paired_device(
    address: &str,
    name: Option<&str>,
    alias: Option<&str>,
    paired: bool,
    connected: bool,
) -> Option<PairedDevice> {
    if !paired {
        return None;
    }
    let raw = parse_address(address)?;
    let name = name.or(alias).unwrap_or_default().to_owned();
    Some(PairedDevice::new(name, BtAddress::new(raw), connected))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MDR: ServiceUuid = ServiceUuid {
        data1: 0x956C_7B26,
        data2: 0xD49A,
        data3: 0x4BA8,
        data4: [0xB0, 0x3F, 0xB1, 0x7D, 0x39, 0x3C, 0xB6, 0xE2],
    };

    #[test]
    fn address_text_parses_msb_first_and_maps_to_kernel_order() {
        let raw = parse_address("00:11:22:33:44:55").unwrap();
        assert_eq!(raw, 0x0011_2233_4455);
        assert_eq!(bdaddr_bytes(raw), [0x55, 0x44, 0x33, 0x22, 0x11, 0x00]);
        for bad in [
            "",
            "00:11:22:33:44",
            "00:11:22:33:44:55:66",
            "0:11:22:33:44:55",
            "zz:11:22:33:44:55",
        ] {
            assert_eq!(parse_address(bad), None, "{bad}");
        }
    }

    #[test]
    fn uuid_forms() {
        assert_eq!(uuid_text(&MDR), "956c7b26-d49a-4ba8-b03f-b17d393cb6e2");
        assert_eq!(uuid_bytes(&MDR)[..6], [0x95, 0x6C, 0x7B, 0x26, 0xD4, 0x9A]);
        let listed = vec![
            "0000110b-0000-1000-8000-00805f9b34fb".to_owned(),
            uuid_text(&MDR),
        ];
        assert!(lists_uuid(&listed, "956C7B26-D49A-4BA8-B03F-B17D393CB6E2"));
        assert!(!lists_uuid(&listed[..1], &uuid_text(&MDR)));
    }

    #[test]
    fn device_properties_map_to_a_paired_device() {
        let a = "00:11:22:33:44:55";
        let d = paired_device(a, Some("Buds"), Some("Alias"), true, true).unwrap();
        assert_eq!(d.name, "Buds");
        assert!(d.connected);
        assert_eq!(d.address().raw(), 0x0011_2233_4455);
        assert_eq!(
            paired_device(a, None, Some("Alias"), true, false)
                .unwrap()
                .name,
            "Alias"
        );
        assert_eq!(paired_device(a, None, None, true, false).unwrap().name, "");
        assert!(paired_device(a, Some("Buds"), None, false, false).is_none());
        assert!(paired_device("nonsense", Some("Buds"), None, true, false).is_none());
    }
}
