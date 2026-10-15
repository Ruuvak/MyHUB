mod common;

use common::{info, sector};
use hidpp::crc::{crc16_ccitt_false, sector_crc_computed, sector_crc_ok, sector_crc_stored};

#[test]
fn check_value() {
    assert_eq!(crc16_ccitt_false(b"123456789"), 0x29B1);
}

#[test]
fn short_sectors_have_no_crc() {
    assert_eq!(sector_crc_computed(&[1, 2]), None);
    assert_eq!(sector_crc_stored(&[1, 2]), None);
    assert!(!sector_crc_ok(&[]));
}

#[test]
fn user_and_directory_sectors_match_stored_crc() {
    assert_eq!(info().sector_size, 255);
    for n in 0..=5u16 {
        let s = sector(&format!("0x{n:04x}"));
        assert_eq!(s.len(), 255);
        let computed = sector_crc_computed(&s).unwrap();
        let stored = sector_crc_stored(&s).unwrap();
        assert_eq!(computed, stored, "sector {n:#06x}");
        assert!(sector_crc_ok(&s));
    }
}

#[test]
fn rom_sectors_store_zero_and_compute_known_values() {
    let s1 = sector("0x0101");
    assert_eq!(sector_crc_stored(&s1), Some(0x0000));
    assert_eq!(sector_crc_computed(&s1), Some(0xA498));
    let s2 = sector("0x0102");
    assert_eq!(sector_crc_stored(&s2), Some(0x0000));
    assert_eq!(sector_crc_computed(&s2), Some(0x3B0D));
    assert!(!sector_crc_ok(&s1));
}
