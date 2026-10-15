mod common;

use common::{fixture_bytes, hex, info, sector};
use hidpp::crc;
use hidpp::dpi::DpiList;
use hidpp::error::ParseError;
use hidpp::onboard::{OnboardInfo, ProfileSector};
use hidpp::onboard_write::{
    ProfilePatch, SECTOR_LEN, WRITE_PAD_BYTE, WriteError, apply_patch, begin_write_params,
    end_write_params, write_chunks,
};

fn list() -> DpiList {
    DpiList::parse(&fixture_bytes("2201_fn1_dpi_list_s0")).unwrap()
}

fn patch_of(s: &[u8], p: &ProfilePatch) -> Result<[u8; SECTOR_LEN], WriteError> {
    apply_patch(3, s, &info(), &list(), p).map(|image| *image.new.bytes())
}

#[test]
fn empty_patch_is_byte_identical() {
    for name in ["0x0003", "0x0004", "0x0005", "0x0002"] {
        let s = sector(name);
        let out = patch_of(&s, &ProfilePatch::default()).unwrap();
        assert_eq!(out.as_slice(), s.as_slice(), "{name}");
        assert!(crc::sector_crc_ok(&out));
    }
}

#[test]
fn patch_changes_only_known_offsets_and_recomputes_crc() {
    let s = sector("0x0003");
    let patch = ProfilePatch {
        report_rate_interval_ms: Some(4),
        default_dpi_index: Some(1),
        shift_dpi_index: Some(4),
        dpi_levels: Some([400, 800, 1250, 6400, 25600]),
    };
    let out = patch_of(&s, &patch).unwrap();
    let parsed = ProfileSector::parse(&out, &info()).unwrap();
    assert_eq!(parsed.report_rate, 4);
    assert_eq!(parsed.default_dpi_index, 1);
    assert_eq!(parsed.shift_dpi_index, 4);
    assert_eq!(parsed.dpi_levels, [400, 800, 1250, 6400, 25600]);
    assert!(parsed.crc_ok);
    // Little-endian levels on the wire.
    assert_eq!(&out[3..5], &400u16.to_le_bytes());
    for i in 0x0D..0xFD {
        assert_eq!(out[i], s[i], "byte {i:#04x} must be unchanged");
    }
    // Original parsed values differ, so the patch really did something.
    let before = ProfileSector::parse(&s, &info()).unwrap();
    assert_ne!(before.dpi_levels, parsed.dpi_levels);
    assert_ne!(before.report_rate, parsed.report_rate);
}

#[test]
fn partial_patch_leaves_other_fields() {
    let s = sector("0x0003");
    let before = ProfileSector::parse(&s, &info()).unwrap();
    let out = patch_of(
        &s,
        &ProfilePatch {
            report_rate_interval_ms: Some(4),
            ..Default::default()
        },
    )
    .unwrap();
    let after = ProfileSector::parse(&out, &info()).unwrap();
    assert_eq!(after.report_rate, 4);
    assert_eq!(after.dpi_levels, before.dpi_levels);
    assert_eq!(after.default_dpi_index, before.default_dpi_index);
    assert_eq!(after.shift_dpi_index, before.shift_dpi_index);
    assert!(after.crc_ok);
}

#[test]
fn refusals() {
    let s = sector("0x0003");
    let none = ProfilePatch::default();

    // Length.
    for len in [0usize, 254, 256] {
        let mut v = s.clone();
        v.resize(len, 0);
        assert_eq!(
            patch_of(&v, &none),
            Err(WriteError::SectorLength { got: len })
        );
    }

    // Profile format.
    let mut other: OnboardInfo = info();
    other.profile_format = 4;
    assert_eq!(
        apply_patch(3, &s, &other, &list(), &none).map(|i| *i.new.bytes()),
        Err(WriteError::Parse(ParseError::UnsupportedProfileFormat(4)))
    );

    // Bad input CRC (in the CRC bytes and in the body).
    for pos in [0xFEusize, 0xFD, 0x40] {
        let mut bad = s.clone();
        bad[pos] ^= 1;
        assert!(
            matches!(
                patch_of(&bad, &none),
                Err(WriteError::InputCrcInvalid { .. })
            ),
            "byte {pos:#04x}"
        );
    }

    // Rate.
    for ms in [0u8, 3, 5, 6, 7, 9, 16, 255] {
        let p = ProfilePatch {
            report_rate_interval_ms: Some(ms),
            ..Default::default()
        };
        assert_eq!(patch_of(&s, &p), Err(WriteError::RateNotAllowed(ms)));
    }
    for ms in [1u8, 2, 4, 8] {
        let p = ProfilePatch {
            report_rate_interval_ms: Some(ms),
            ..Default::default()
        };
        assert!(patch_of(&s, &p).is_ok());
    }

    // Indices.
    for v in [5u8, 6, 255] {
        let d = ProfilePatch {
            default_dpi_index: Some(v),
            ..Default::default()
        };
        assert!(matches!(
            patch_of(&s, &d),
            Err(WriteError::DpiIndexOutOfRange { value, .. }) if value == v
        ));
        let sh = ProfilePatch {
            shift_dpi_index: Some(v),
            ..Default::default()
        };
        assert!(matches!(
            patch_of(&s, &sh),
            Err(WriteError::DpiIndexOutOfRange { .. })
        ));
    }

    // DPI: zero, below range, above range, off step.
    for bad in [0u16, 50, 99, 125, 25601, 25650, 65535] {
        let p = ProfilePatch {
            dpi_levels: Some([800, 800, bad, 800, 800]),
            ..Default::default()
        };
        assert_eq!(patch_of(&s, &p), Err(WriteError::DpiNotAllowed(bad)));
    }
}

#[test]
fn chunks_cover_the_image_and_pad_the_last() {
    let s = sector("0x0003");
    let image = patch_of(&s, &ProfilePatch::default()).unwrap();
    let chunks = write_chunks(&image);
    assert_eq!(chunks.len(), 16);
    let flat: Vec<u8> = chunks.iter().flatten().copied().collect();
    assert_eq!(&flat[..255], image.as_slice());
    assert_eq!(flat[255], WRITE_PAD_BYTE);
    assert_eq!(WRITE_PAD_BYTE, 0x00);
}

#[test]
fn begin_and_end_write_params() {
    let p = begin_write_params(0x0003, 0, 255).unwrap();
    assert_eq!(p[..6], [0x00, 0x03, 0x00, 0x00, 0x00, 0xFF]);
    assert!(p[6..].iter().all(|&b| b == 0));
    assert_eq!(
        begin_write_params(3, 16, 255),
        Err(WriteError::OffsetNotZero(16))
    );
    assert_eq!(
        begin_write_params(3, 0, 256),
        Err(WriteError::LengthNotSector(256))
    );
    assert_eq!(end_write_params(), [0u8; 16]);
}

fn with_crc(mut s: Vec<u8>) -> Vec<u8> {
    let c = crc::crc16_ccitt_false(&s[..253]);
    s[253..].copy_from_slice(&c.to_be_bytes());
    s
}

#[test]
fn resulting_default_and_shift_level_must_be_non_zero() {
    // Level 3 is zero in this image; default/shift as read point elsewhere.
    let mut s = sector("0x0003");
    s[3 + 3 * 2] = 0;
    s[3 + 3 * 2 + 1] = 0;
    let s = with_crc(s);
    let before = ProfileSector::parse(&s, &info()).unwrap();
    assert!(before.default_dpi_index != 3 && before.shift_dpi_index != 3);
    assert!(patch_of(&s, &ProfilePatch::default()).is_ok());

    let default3 = ProfilePatch {
        default_dpi_index: Some(3),
        ..Default::default()
    };
    assert_eq!(
        patch_of(&s, &default3),
        Err(WriteError::DpiLevelZero {
            field: "default DPI",
            index: 3
        })
    );
    let shift3 = ProfilePatch {
        shift_dpi_index: Some(3),
        ..Default::default()
    };
    assert_eq!(
        patch_of(&s, &shift3),
        Err(WriteError::DpiLevelZero {
            field: "shift DPI",
            index: 3
        })
    );
    // Giving level 3 a real value in the same patch makes it fine.
    let both = ProfilePatch {
        default_dpi_index: Some(3),
        dpi_levels: Some([400, 800, 1200, 1600, 3200]),
        ..Default::default()
    };
    assert!(patch_of(&s, &both).is_ok());

    // As read: the stored default index already points at the zero level.
    let mut bad = s.clone();
    bad[1] = 3;
    let bad = with_crc(bad);
    assert_eq!(
        patch_of(&bad, &ProfilePatch::default()),
        Err(WriteError::DpiLevelZero {
            field: "default DPI",
            index: 3
        })
    );
    // Patching the index away from the zero level repairs it.
    let fix = ProfilePatch {
        default_dpi_index: Some(0),
        ..Default::default()
    };
    assert!(patch_of(&bad, &fix).is_ok());
}

// ---- directory, clone, select ----

fn pd(raw: &[u8; 255], sector: u16, enabled: bool, active: u16) -> Result<[u8; 255], WriteError> {
    patch_directory(raw, sector, enabled, active).map(|i| *i.new.bytes())
}

use hidpp::onboard_write::{
    clone_profile, patch_directory, select_profile_params, validate_directory_image,
};

fn directory() -> [u8; 255] {
    sector("0x0000").try_into().unwrap()
}

fn dir_with_crc(mut d: [u8; 255]) -> [u8; 255] {
    let c = crc::crc16_ccitt_false(&d[..253]);
    d[253..].copy_from_slice(&c.to_be_bytes());
    d
}

#[test]
fn directory_fixture_shape() {
    let d = directory();
    assert_eq!(
        d[..20],
        hex("000101ff000201ff000300ff000400ff000500ff")[..20]
    );
    assert_eq!(d[20..24], [0xFF; 4]);
    assert!(crc::sector_crc_ok(&d));
    assert_eq!(validate_directory_image(&d), Ok(()));
}

#[test]
fn patch_directory_touches_only_byte_2_and_the_crc() {
    let d = directory();
    // Sector 3 is the third entry: offset 8, its flag is byte 10.
    let on = pd(&d, 3, true, 2).unwrap();
    assert_eq!(on[10], 0x01);
    for i in 0..253 {
        if i != 10 {
            assert_eq!(on[i], d[i], "byte {i}");
        }
    }
    assert!(crc::sector_crc_ok(&on));
    assert_ne!(on[253..], d[253..]);
    // Byte 3 of the entry stays 0xFF as read.
    assert_eq!(on[11], 0xFF);
    assert_eq!(validate_directory_image(&on), Ok(()));

    // Disabling gives the original back byte for byte; same value is a no-op.
    assert_eq!(pd(&on, 3, false, 2).unwrap(), d);
    assert_eq!(pd(&d, 3, false, 2).unwrap(), d);
    // Sectors 4 and 5 are entries at offsets 12 and 16.
    for (s, off) in [(4u16, 12usize), (5, 16)] {
        let p = pd(&d, s, true, 2).unwrap();
        assert_eq!(p[off + 2], 1);
        let changed = p.iter().zip(d.iter()).filter(|(a, b)| a != b).count();
        assert!((1..=3).contains(&changed), "{changed} bytes changed");
        assert_eq!(
            p[..253]
                .iter()
                .zip(d.iter())
                .filter(|(a, b)| a != b)
                .count(),
            1
        );
    }
}

#[test]
fn patch_directory_refusals() {
    let d = directory();
    for s in [0u16, 1, 2, 6, 0x0101, 0xFFFF] {
        assert_eq!(
            pd(&d, s, true, 2),
            Err(WriteError::DirectorySectorNotAllowed(s))
        );
    }
    // No entry for sector 5.
    let mut no5 = d;
    no5[16..20].copy_from_slice(&[0xFF; 4]);
    let no5 = dir_with_crc(no5);
    assert_eq!(pd(&no5, 5, true, 2), Err(WriteError::DirectoryNoEntry(5)));
    // Bad input CRC.
    let mut bad = d;
    bad[254] ^= 1;
    assert!(matches!(
        pd(&bad, 3, true, 2),
        Err(WriteError::InputCrcInvalid { .. })
    ));
    // Malformed: flag 2, entry that is not a user sector, no terminator, empty.
    let mut flag = d;
    flag[10] = 2;
    assert!(matches!(
        pd(&dir_with_crc(flag), 3, true, 2),
        Err(WriteError::DirectoryMalformed(_))
    ));
    let mut rom = d;
    rom[0..2].copy_from_slice(&0x0101u16.to_be_bytes());
    assert!(matches!(
        pd(&dir_with_crc(rom), 3, true, 2),
        Err(WriteError::DirectoryMalformed(_))
    ));
    let mut noterm = [0u8; 255];
    for (i, e) in noterm[..252].chunks_mut(4).enumerate() {
        e.copy_from_slice(&[0, (i % 5 + 1) as u8, 1, 0xFF]);
    }
    assert!(matches!(
        pd(&dir_with_crc(noterm), 3, true, 2),
        Err(WriteError::DirectoryMalformed(_))
    ));
    let empty = dir_with_crc([0xFFu8; 255]);
    assert!(matches!(
        pd(&empty, 3, true, 2),
        Err(WriteError::DirectoryMalformed(_))
    ));
    // The active profile may not end up disabled; zero enabled is refused.
    let on3 = pd(&d, 3, true, 2).unwrap();
    assert_eq!(
        pd(&on3, 3, false, 3),
        Err(WriteError::ActiveProfileDisabled(3))
    );
    let mut only3 = d;
    only3[2] = 0;
    only3[6] = 0;
    only3[10] = 1;
    let only3 = dir_with_crc(only3);
    assert_eq!(
        pd(&only3, 3, false, 0x0101),
        Err(WriteError::NoProfileEnabled)
    );
    // validate_directory_image: wrong length, profile image, nothing enabled.
    assert!(validate_directory_image(&d[..254]).is_err());
    assert!(validate_directory_image(&sector("0x0003")).is_err());
    let mut none = d;
    none[2] = 0;
    none[6] = 0;
    assert_eq!(
        validate_directory_image(&dir_with_crc(none)),
        Err(WriteError::NoProfileEnabled)
    );
}

#[test]
fn clone_round_trip_and_no_own_address_byte() {
    let i = info();
    let src: [u8; 255] = sector("0x0002").try_into().unwrap();
    let copy = clone_profile(&src, &i).unwrap();
    assert_eq!(copy, src);
    let patched = apply_patch(
        3,
        &copy,
        &i,
        &list(),
        &ProfilePatch {
            report_rate_interval_ms: Some(4),
            dpi_levels: Some([400, 800, 1200, 1600, 3200]),
            ..Default::default()
        },
    )
    .map(|image| *image.new.bytes())
    .unwrap();
    let before = ProfileSector::parse(&src, &i).unwrap();
    let after = ProfileSector::parse(&patched, &i).unwrap();
    assert!(after.crc_ok);
    // Everything but rate, levels and CRC is the source's.
    assert_eq!(after.buttons, before.buttons);
    assert_eq!(after.gshift_buttons, before.gshift_buttons);
    assert_eq!(after.name, before.name);
    assert_eq!(after.led_zones, before.led_zones);
    assert_eq!(after.color, before.color);
    assert_eq!(after.default_dpi_index, before.default_dpi_index);

    // Layout A has no byte holding the sector's own address: the user sectors
    // 3, 4 and 5 are byte-identical in the fixture although their addresses differ.
    assert_eq!(sector("0x0003"), sector("0x0004"));
    assert_eq!(sector("0x0004"), sector("0x0005"));

    // Refusals: bad CRC and wrong format.
    let mut bad = src;
    bad[0x40] ^= 1;
    assert!(matches!(
        clone_profile(&bad, &i),
        Err(WriteError::InputCrcInvalid { .. })
    ));
    let mut other = i;
    other.profile_format = 4;
    assert_eq!(
        clone_profile(&src, &other),
        Err(WriteError::Parse(ParseError::UnsupportedProfileFormat(4)))
    );
}

#[test]
fn select_profile_params_layout() {
    for s in 1..=5u16 {
        let p = select_profile_params(s).unwrap();
        assert_eq!(p[..2], s.to_be_bytes());
        assert!(p[2..].iter().all(|&b| b == 0));
    }
    for s in [0u16, 6, 0x0101, 0xFFFF] {
        assert_eq!(
            select_profile_params(s),
            Err(WriteError::SelectSectorNotAllowed(s))
        );
    }
}

// ---- sealed image, duplicates, restore constructors ----

use hidpp::onboard_write::apply_patch_over;
use hidpp::onboard_write::{restore_directory_image, restore_profile_image};

fn rate_patch_for_tests() -> ProfilePatch {
    ProfilePatch {
        report_rate_interval_ms: Some(4),
        ..Default::default()
    }
}

#[test]
fn directory_with_a_duplicate_sector_is_malformed() {
    let d = directory();
    // Entry 4 (sector 5) becomes a second entry for sector 3.
    let mut dup = d;
    dup[16..18].copy_from_slice(&3u16.to_be_bytes());
    let dup = dir_with_crc(dup);
    assert_eq!(
        validate_directory_image(&dup),
        Err(WriteError::DirectoryMalformed("duplicate sector entry"))
    );
    assert_eq!(
        pd(&dup, 4, true, 2),
        Err(WriteError::DirectoryMalformed("duplicate sector entry"))
    );
}

#[test]
fn patch_directory_and_apply_patch_images_know_their_sector() {
    let d = directory();
    let images = patch_directory(&d, 3, true, 2).unwrap();
    assert_eq!((images.new.sector(), images.rollback.sector()), (0, 0));
    assert_eq!(images.rollback.bytes(), &d);
    assert_ne!(images.new.bytes(), &d);
    let s = sector("0x0004");
    let images = apply_patch(4, &s, &info(), &list(), &rate_patch_for_tests()).unwrap();
    assert_eq!((images.new.sector(), images.rollback.sector()), (4, 4));
    assert_eq!(images.rollback.bytes().as_slice(), s.as_slice());
    assert_ne!(images.new.bytes().as_slice(), s.as_slice());
    // With a clone as the base, the rollback is still the target's own bytes.
    let src = sector("0x0002");
    let cloned = clone_profile(&src.clone().try_into().unwrap(), &info()).unwrap();
    let images =
        apply_patch_over(4, &s, &cloned, &info(), &list(), &ProfilePatch::default()).unwrap();
    assert_eq!(images.rollback.bytes().as_slice(), s.as_slice());
    assert_eq!(images.new.bytes().as_slice(), src.as_slice());
    // A target that is shaped like a directory has no rollback image.
    assert!(matches!(
        apply_patch(3, &directory(), &info(), &list(), &ProfilePatch::default()),
        Err(WriteError::ImplausibleProfile(_))
    ));
    // Only profile sectors 3..=5 can be named.
    for target in [0u16, 1, 2, 6, 0x0101] {
        assert_eq!(
            apply_patch(target, &s, &info(), &list(), &ProfilePatch::default()),
            Err(WriteError::ProfileSectorNotAllowed(target))
        );
    }
}

#[test]
fn restore_profile_image_rules() {
    let i = info();
    let s = sector("0x0003");
    let image = restore_profile_image(5, &s, &i).unwrap();
    assert_eq!(
        (image.sector(), image.bytes().as_slice()),
        (5, s.as_slice())
    );
    for target in [0u16, 1, 2, 6] {
        assert_eq!(
            restore_profile_image(target, &s, &i),
            Err(WriteError::ProfileSectorNotAllowed(target))
        );
    }
    // A directory image is no profile.
    assert!(matches!(
        restore_profile_image(3, &directory(), &i),
        Err(WriteError::ImplausibleProfile(_))
    ));
    // Bad CRC, short image, implausible rate and index.
    let mut bad = s.clone();
    bad[0x40] ^= 1;
    assert!(matches!(
        restore_profile_image(3, &bad, &i),
        Err(WriteError::InputCrcInvalid { .. })
    ));
    assert!(matches!(
        restore_profile_image(3, &s[..254], &i),
        Err(WriteError::SectorLength { got: 254 })
    ));
    let mut rate = s.clone();
    rate[0] = 3;
    assert!(matches!(
        restore_profile_image(3, &with_crc(rate.clone()), &i),
        Err(WriteError::ImplausibleProfile(_))
    ));
    let mut idx = s.clone();
    idx[1] = 5;
    assert!(matches!(
        restore_profile_image(3, &with_crc(idx), &i),
        Err(WriteError::ImplausibleProfile(_))
    ));
}

#[test]
fn restore_directory_image_must_fit_the_current_directory() {
    let now = directory();
    // Same entries, only flags differ: fine.
    let flags = patch_directory(&now, 3, true, 2).unwrap();
    let ok = restore_directory_image(flags.new.bytes(), &now, 2).unwrap();
    assert_eq!(ok.sector(), 0);
    assert_eq!(ok.bytes(), flags.new.bytes());
    assert!(restore_directory_image(&now, &now, 2).is_ok());

    // Different entry list: an entry removed, added, or reordered.
    let mut fewer = now;
    fewer[16..20].copy_from_slice(&[0xFF; 4]);
    assert!(matches!(
        restore_directory_image(&dir_with_crc(fewer), &now, 2),
        Err(WriteError::DirectoryMismatch(_))
    ));
    let mut more = now;
    more[20..24].copy_from_slice(&[0, 0x06, 0, 0xFF]);
    more[24..28].copy_from_slice(&[0xFF; 4]);
    assert!(matches!(
        restore_directory_image(&dir_with_crc(more), &now, 2),
        Err(WriteError::DirectoryMismatch(_))
    ));
    let mut swapped = now;
    swapped[8..12].copy_from_slice(&[0, 4, 0, 0xFF]);
    swapped[12..16].copy_from_slice(&[0, 3, 0, 0xFF]);
    assert!(matches!(
        restore_directory_image(&dir_with_crc(swapped), &now, 2),
        Err(WriteError::DirectoryMismatch(_))
    ));
    // A byte other than a flag differs (byte 3 of an entry).
    let mut byte3 = now;
    byte3[11] = 0;
    assert!(matches!(
        restore_directory_image(&dir_with_crc(byte3), &now, 2),
        Err(WriteError::DirectoryMismatch(_))
    ));
    // Active profile disabled, nothing enabled, bad CRC, wrong length.
    let mut no2 = now;
    no2[6] = 0;
    assert_eq!(
        restore_directory_image(&dir_with_crc(no2), &now, 2),
        Err(WriteError::ActiveProfileDisabled(2))
    );
    let mut none = now;
    none[2] = 0;
    none[6] = 0;
    assert_eq!(
        restore_directory_image(&dir_with_crc(none), &now, 0x0101),
        Err(WriteError::NoProfileEnabled)
    );
    let mut bad = now;
    bad[254] ^= 1;
    assert!(restore_directory_image(&bad, &now, 2).is_err());
    assert!(restore_directory_image(&now[..254], &now, 2).is_err());
    // The current directory itself must have a valid CRC.
    assert!(restore_directory_image(&now, &bad, 2).is_err());
}
