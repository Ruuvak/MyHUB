mod common;

use common::{fixture_bytes, hex, info, sector};
use hidpp::error::ParseError;
use hidpp::onboard::{
    Binding, Directory, DirectoryEntry, OnboardInfo, ProfileSector, SectorAssembler,
    SpecialFunction, chunk_plan, read_data_params, slot_name,
};

use Binding::{Consumer, Keyboard, MouseButtons, Special, Unassigned};
use SpecialFunction::{
    DpiDown, DpiShift, DpiUp, GShift, Other, ProfileCycle, WheelLeft, WheelRight,
};

#[test]
fn info_fixture() {
    let i = OnboardInfo::parse(&fixture_bytes("8100_fn0_info")).unwrap();
    assert_eq!(i.memory_model, 1);
    assert_eq!(i.profile_format, 3);
    assert_eq!(i.macro_format, 1);
    assert_eq!(i.profile_count, 5);
    assert_eq!(i.rom_profile_count, 2);
    assert_eq!(i.button_count, 11);
    assert_eq!(i.sector_count, 16);
    assert_eq!(i.sector_size, 255);
    assert_eq!(i.flags, 0x0A);
    assert!(OnboardInfo::parse(&hex("01030105020b1000ff")).is_err());
}

#[test]
fn read_data_params_layout() {
    let p = read_data_params(0x0101, 0x00EF);
    assert_eq!(p[..4], [0x01, 0x01, 0x00, 0xEF]);
    assert!(p[4..].iter().all(|&b| b == 0));
}

#[test]
fn chunk_plan_for_255_ends_at_239() {
    let plan = chunk_plan(255).unwrap();
    assert_eq!(plan.len(), 16);
    assert_eq!(plan[0], 0);
    assert_eq!(plan[14], 224);
    assert_eq!(plan[15], 239);
    assert_eq!(*plan.last().unwrap(), 239);
    // Every chunk stays inside the sector.
    assert!(plan.iter().all(|&o| usize::from(o) + 16 <= 255));
    // Exact multiple: no clamping, no duplicates.
    assert_eq!(chunk_plan(32).unwrap(), vec![0, 16]);
    assert_eq!(chunk_plan(16).unwrap(), vec![0]);
    assert!(chunk_plan(15).is_err());
}

#[test]
fn assembler_rebuilds_fixture_sector_from_simulated_replies() {
    let info = info();
    for name in ["0x0000", "0x0001", "0x0002", "0x0005", "0x0101"] {
        let truth = sector(name);
        let mut asm = SectorAssembler::new(info.sector_size).unwrap();
        assert!(!asm.is_complete());
        for off in chunk_plan(info.sector_size).unwrap() {
            let o = usize::from(off);
            // Simulated device reply: 16 bytes at the requested offset.
            asm.push_chunk(off, &truth[o..o + 16]).unwrap();
        }
        assert!(asm.is_complete());
        assert_eq!(asm.finish().unwrap(), truth, "{name}");
    }
}

#[test]
fn assembler_errors() {
    let mut asm = SectorAssembler::new(255).unwrap();
    assert!(matches!(
        asm.push_chunk(240, &[0; 16]),
        Err(ParseError::ChunkOutOfRange { .. })
    ));
    assert!(matches!(
        asm.push_chunk(0, &[0; 15]),
        Err(ParseError::TooShort { .. })
    ));
    asm.push_chunk(0, &[0; 16]).unwrap();
    assert_eq!(
        asm.finish().unwrap_err(),
        ParseError::SectorIncomplete { missing: 239 }
    );
    assert!(SectorAssembler::new(8).is_err());
}

#[test]
fn directory_fixture() {
    let d = Directory::parse(&sector("0x0000")).unwrap();
    assert_eq!(
        d.entries,
        vec![
            DirectoryEntry {
                sector: 1,
                enabled: true
            },
            DirectoryEntry {
                sector: 2,
                enabled: true
            },
            DirectoryEntry {
                sector: 3,
                enabled: false
            },
            DirectoryEntry {
                sector: 4,
                enabled: false
            },
            DirectoryEntry {
                sector: 5,
                enabled: false
            },
        ]
    );
    assert_eq!(d.enabled_sectors(), vec![1, 2]);
    assert!(Directory::parse(&[0, 1]).is_err());
}

#[test]
fn binding_decode_table() {
    use Binding::Unknown;
    let cases: &[([u8; 4], Binding)] = &[
        ([0x80, 0x01, 0x00, 0x01], MouseButtons(0x0001)),
        ([0x80, 0x01, 0x00, 0x10], MouseButtons(0x0010)),
        ([0x80, 0x01, 0x01, 0x04], MouseButtons(0x0104)),
        (
            [0x80, 0x02, 0x01, 0x17],
            Keyboard {
                modifiers: 1,
                usage: 0x17,
            },
        ),
        (
            [0x80, 0x02, 0x03, 0x2B],
            Keyboard {
                modifiers: 3,
                usage: 0x2B,
            },
        ),
        ([0x80, 0x03, 0x00, 0xEA], Consumer(0x00EA)),
        ([0x90, 0x01, 0, 0], Special(WheelLeft)),
        ([0x90, 0x02, 0, 0], Special(WheelRight)),
        ([0x90, 0x03, 0, 0], Special(DpiUp)),
        ([0x90, 0x04, 0, 0], Special(DpiDown)),
        ([0x90, 0x07, 0, 0], Special(DpiShift)),
        ([0x90, 0x0A, 0, 0], Special(ProfileCycle)),
        ([0x90, 0x0B, 0, 0], Special(GShift)),
        ([0x90, 0x10, 0, 0], Special(Other(0x10))),
        ([0x90, 0x11, 0, 0], Special(Other(0x11))),
        ([0x90, 0x05, 0, 0], Special(Other(0x05))),
        ([0xFF, 0xFF, 0xFF, 0xFF], Unassigned),
        ([0x90, 0x03, 0x00, 0x01], Unknown([0x90, 0x03, 0x00, 0x01])),
        ([0x81, 0x00, 0x00, 0x00], Unknown([0x81, 0x00, 0x00, 0x00])),
        ([0x80, 0x04, 0x00, 0x00], Unknown([0x80, 0x04, 0x00, 0x00])),
        ([0x00, 0x00, 0x00, 0x00], Unknown([0, 0, 0, 0])),
    ];
    for (bytes, expected) in cases {
        assert_eq!(Binding::decode(*bytes), *expected, "{bytes:02x?}");
    }
}

#[test]
fn slot_names() {
    assert_eq!(slot_name(0, 11).as_deref(), Some("G1"));
    assert_eq!(slot_name(10, 11).as_deref(), Some("G11"));
    assert_eq!(slot_name(11, 11), None);
    assert_eq!(slot_name(15, 11), None);
}

#[test]
fn profile_sector_1_field_by_field() {
    let info = info();
    let raw = sector("0x0001");
    let p = ProfileSector::parse(&raw, &info).unwrap();
    assert_eq!(p.report_rate, 1);
    assert_eq!(p.default_dpi_index, 2);
    assert_eq!(p.shift_dpi_index, 0);
    assert_eq!(p.dpi_levels, [600, 1200, 1600, 2400, 3200]);
    assert_eq!(p.color, [0xFF, 0xFF, 0xFF]);
    assert_eq!(p.power_mode, 0xFF);
    assert_eq!(p.angle_snapping, 0);
    assert_eq!(p.write_counter, [0xFF, 0xFF]);
    assert_eq!(p.reserved, [0xFF; 8]);
    assert_eq!(p.power_save_timeout, 60);
    assert_eq!(p.power_off_timeout, 300);
    assert_eq!(p.name.as_deref(), Some("PROFILE_NAME_DEFAULT"));

    let expected = [
        MouseButtons(0x0001),
        MouseButtons(0x0002),
        MouseButtons(0x0004),
        MouseButtons(0x0008),
        Special(DpiShift),
        MouseButtons(0x0010),
        Special(WheelLeft),
        Special(WheelRight),
        Special(ProfileCycle),
        Special(DpiUp),
        Special(DpiDown),
        Special(Other(0x10)),
        Special(Other(0x11)),
        Unassigned,
        Unassigned,
        Unassigned,
    ];
    assert_eq!(p.buttons, expected);
    // G-shift layer mirrors the default layer in this profile.
    assert_eq!(p.gshift_buttons, expected);

    for (i, name) in [
        "G1", "G2", "G3", "G4", "G5", "G6", "G7", "G8", "G9", "G10", "G11",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(slot_name(i, info.button_count).as_deref(), Some(*name));
    }

    let zone_a = [0x0f, 0, 0, 0, 0, 0, 0, 0, 0x64, 0, 0];
    let zone_b = [0x10, 0, 0, 0, 0, 0, 0, 0, 0x64, 0, 0];
    assert_eq!(p.led_zones, [zone_a, zone_a, zone_b, zone_b]);
    assert_eq!(p.flag, 0x00);
    assert!(p.trailer.is_empty());
    assert_eq!(p.stored_crc, 0x7593);
    assert_eq!(p.computed_crc, 0x7593);
    assert!(p.crc_ok);
    assert_eq!(p.raw, raw);
}

#[test]
fn profile_sector_2_field_by_field() {
    let info = info();
    let p = ProfileSector::parse(&sector("0x0002"), &info).unwrap();
    assert_eq!(p.report_rate, 2);
    assert_eq!(p.default_dpi_index, 2);
    assert_eq!(p.dpi_levels, [800, 1200, 1600, 2400, 3200]);
    assert_eq!(p.name, None, "first unit 0xFFFF means unset");
    assert_eq!(p.buttons[4], Special(GShift));
    assert_eq!(p.buttons[0], MouseButtons(1));
    assert_eq!(p.buttons[3], MouseButtons(8));
    // G-shift layer.
    assert_eq!(
        p.gshift_buttons[2],
        Keyboard {
            modifiers: 0x01,
            usage: 0x17
        }
    );
    assert_eq!(p.gshift_buttons[3], MouseButtons(0x0008));
    assert_eq!(p.gshift_buttons[4], Unassigned);
    assert_eq!(p.gshift_buttons[5], MouseButtons(0x0010));
    assert_eq!(
        p.gshift_buttons[6],
        Keyboard {
            modifiers: 0x03,
            usage: 0x2B
        }
    );
    assert_eq!(p.gshift_buttons[11], Consumer(0x00EA));
    assert_eq!(p.gshift_buttons[12], Consumer(0x00E9));
    assert_eq!(p.gshift_buttons[13], Unassigned);
    assert!(p.crc_ok);
    assert_eq!(p.stored_crc, 0x3B0D);
}

#[test]
fn profile_sectors_3_to_5_parse() {
    let info = info();
    for n in 3..=5u16 {
        let p = ProfileSector::parse(&sector(&format!("0x{n:04x}")), &info).unwrap();
        assert_eq!(p.dpi_levels, [800, 1200, 1600, 2400, 3200], "sector {n}");
        assert_eq!(p.report_rate, 1);
        assert!(p.crc_ok);
        assert_eq!(p.buttons[11], Unassigned);
        assert_eq!(p.name, None);
    }
}

#[test]
fn rom_sectors_parse_but_report_bad_crc() {
    let info = info();
    let p = ProfileSector::parse(&sector("0x0101"), &info).unwrap();
    assert_eq!(p.stored_crc, 0x0000);
    assert_eq!(p.computed_crc, 0xA498);
    assert!(!p.crc_ok);
}

#[test]
fn parse_refuses_other_formats_and_wrong_length() {
    let info = info();
    let raw = sector("0x0001");

    for fmt in [0u8, 1, 2, 4, 6, 10, 255] {
        let other = OnboardInfo {
            profile_format: fmt,
            ..info
        };
        assert_eq!(
            ProfileSector::parse(&raw, &other),
            Err(ParseError::UnsupportedProfileFormat(fmt))
        );
    }
    assert_eq!(
        ProfileSector::parse(&raw[..254], &info),
        Err(ParseError::SectorLength {
            expected: 255,
            got: 254
        })
    );
    let mut long = raw.clone();
    long.push(0);
    assert_eq!(
        ProfileSector::parse(&long, &info),
        Err(ParseError::SectorLength {
            expected: 255,
            got: 256
        })
    );
    // Info that disagrees with itself: tiny sector size and matching length.
    let tiny = OnboardInfo {
        sector_size: 64,
        ..info
    };
    assert!(matches!(
        ProfileSector::parse(&raw[..64], &tiny),
        Err(ParseError::SectorTooSmall { .. })
    ));
    assert!(ProfileSector::parse(&[], &info).is_err());
}

#[test]
fn corrupted_crc_is_reported_not_hidden() {
    let info = info();
    let mut raw = sector("0x0001");
    raw[0x40] ^= 0x01;
    let p = ProfileSector::parse(&raw, &info).unwrap();
    assert!(!p.crc_ok);
    assert_eq!(p.stored_crc, 0x7593);
    assert_ne!(p.computed_crc, 0x7593);
}

#[test]
fn serde_serializes_parsed_structs() {
    let info = info();
    let p = ProfileSector::parse(&sector("0x0001"), &info).unwrap();
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["name"], "PROFILE_NAME_DEFAULT");
    assert_eq!(v["dpi_levels"][2], 1600);
    assert!(serde_json::to_string(&info).is_ok());
}
