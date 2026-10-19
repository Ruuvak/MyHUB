mod common;

use common::pad;
use keychron_proto::{
    LightValue, Policy, PolicyError, Request, Tier, TravelMode, request::EFFECT_MAX,
};

fn all_open() -> Policy {
    common::sized(
        Policy::read_only()
            .with_tier(Tier::Volatile)
            .with_tier(Tier::Persistent),
    )
}

#[test]
fn never_send_list_is_forbidden_even_with_every_gate_open() {
    let frames: [&[u8]; 6] = [
        &[0x0A],
        &[0x0B],
        &[0xA7, 0x15],
        &[0xAB, 0x05],
        &[0xAB, 0x09],
        &[0xBA],
    ];
    for f in frames {
        let err = all_open().check_bytes(f).unwrap_err();
        assert!(
            matches!(err, PolicyError::Forbidden { command, .. } if command == f[0]),
            "{f:02x?} gave {err:?}"
        );
    }
}

#[test]
fn other_writes_and_unknown_commands_are_not_allowlisted() {
    let frames: [&[u8]; 13] = [
        &[0xA9, 0x13, 0x00, 0x02, 0x41, 0x00],
        &[0xA9, 0x15, 0x00, 0x01, 0x00, 0x0E],
        &[0xA9, 0x16, 0x00, 0x03, 0x01],
        &[0xA9, 0x1E, 0x00],
        &[0xA9, 0x21, 0x00, 0x00],
        &[0xA8, 0x02],
        &[0xA8, 0x04, 0xE0],
        &[0xA8, 0x0A, 0x00, 0x01, 0x55, 0xFF, 0xFF],
        &[0x06],
        &[0x0F, 0x00],
        &[0xA7, 0x04, 0x01],
        &[0xA7, 0x0C, 0x58, 0x02],
        &[0x55, 0x01],
    ];
    for f in frames {
        let err = all_open().check_bytes(f).unwrap_err();
        assert!(
            matches!(err, PolicyError::NotAllowlisted { .. }),
            "{f:02x?} gave {err:?}"
        );
    }
    // SET_TRAVAL for selected keys (keyMode 0) is outside v1.
    let per_key = [0xA9, 0x14, 0x00, 0x01, 0x14, 0x04, 0x04, 0x00];
    assert!(matches!(
        all_open().check_bytes(&per_key),
        Err(PolicyError::NotAllowlisted { .. })
    ));
}

#[test]
fn read_policy_refuses_volatile_and_persistent_requests() {
    let policy = Policy::read_only().with_profile_count(3);
    let travel = Request::SetTravelGlobal {
        profile: 0,
        mode: TravelMode::Static,
        actuation: 20,
        press: 4,
        release: 4,
    };
    assert_eq!(
        policy.check(&travel).unwrap_err(),
        PolicyError::TierNotEnabled {
            tier: Tier::Volatile
        }
    );
    // The gates are separate: Volatile does not open Persistent.
    let volatile = Policy::allowing(Tier::Volatile).with_profile_count(3);
    assert_eq!(
        volatile
            .check(&Request::SaveProfile { profile: 0 })
            .unwrap_err(),
        PolicyError::TierNotEnabled {
            tier: Tier::Persistent
        }
    );
    assert!(volatile.check(&Request::GetProfilesInfo).is_ok());
}

#[test]
fn volatile_gate_encodes_set_travel_exactly() {
    let policy = Policy::allowing(Tier::Volatile).with_profile_count(3);
    let req = Request::SetTravelGlobal {
        profile: 1,
        mode: TravelMode::RapidTrigger,
        actuation: 20,
        press: 4,
        release: 5,
    };
    let checked = policy.check(&req).unwrap();
    assert_eq!(
        checked.encode(),
        pad(&[0xA9, 0x14, 0x01, 0x02, 20, 4, 5, 0x01])
    );
    // The raw-byte entry gives the same frame.
    let via_bytes = policy
        .check_bytes(&[0xA9, 0x14, 0x01, 0x02, 20, 4, 5, 0x01])
        .unwrap();
    assert_eq!(via_bytes.encode(), checked.encode());
    assert!(checked.matches(&pad(&[0xA9, 0x14, 0x00])));

    let light = policy
        .check(&Request::SetLight {
            value: LightValue::Color {
                hue: 0x55,
                saturation: 0xFF,
            },
        })
        .unwrap();
    assert_eq!(light.encode(), pad(&[0x07, 0x03, 0x04, 0x55, 0xFF]));

    let save = Policy::allowing(Tier::Persistent).with_profile_count(3);
    assert_eq!(
        save.check(&Request::SaveProfile { profile: 2 })
            .unwrap()
            .encode(),
        pad(&[0xA9, 0x1F, 0x02])
    );
    assert_eq!(
        save.check(&Request::SaveLighting).unwrap().encode(),
        pad(&[0x09, 0x03])
    );
}

#[test]
fn out_of_range_values_are_refused_before_encode() {
    let policy = Policy::allowing(Tier::Volatile).with_profile_count(3);
    let travel = |actuation, press, release| Request::SetTravelGlobal {
        profile: 0,
        mode: TravelMode::Static,
        actuation,
        press,
        release,
    };
    assert!(policy.check(&travel(20, 4, 4)).is_ok());
    for (a, p, r, field) in [
        (1, 4, 4, "actuation"),
        (39, 4, 4, "actuation"),
        (20, 0, 4, "press"),
        (20, 39, 4, "press"),
        (20, 4, 0, "release"),
        (20, 4, 39, "release"),
    ] {
        match policy.check(&travel(a, p, r)) {
            Err(PolicyError::OutOfRange { field: f, .. }) => assert_eq!(f, field),
            other => panic!("{a} {p} {r}: {other:?}"),
        }
    }
    assert!(matches!(
        policy.check(&Request::SetLight {
            value: LightValue::Effect(EFFECT_MAX + 1)
        }),
        Err(PolicyError::OutOfRange {
            field: "effect",
            ..
        })
    ));
    assert!(
        policy
            .check(&Request::SetLight {
                value: LightValue::Effect(EFFECT_MAX)
            })
            .is_ok()
    );
    // Chunk lengths.
    let read = common::sized(Policy::read_only());
    let chunk = |len| Request::GetProfileBuffer {
        profile: 0,
        offset: 0,
        len,
    };
    assert!(read.check(&chunk(26)).is_ok());
    assert!(read.check(&chunk(27)).is_err());
    assert!(read.check(&chunk(0)).is_err());
    let keymap = |len| Request::GetKeymapBuffer { offset: 0, len };
    assert!(read.check(&keymap(28)).is_ok());
    assert!(read.check(&keymap(27)).is_err());
    assert!(read.check(&keymap(30)).is_err());
    let mac = |len| Request::GetMacroBuffer { offset: 0, len };
    assert!(read.check(&mac(28)).is_ok());
    assert!(read.check(&mac(29)).is_err());
}

#[test]
fn profile_must_be_below_a_count_the_device_layer_supplied() {
    let sel = Request::SelectProfile { profile: 2 };
    assert_eq!(
        Policy::allowing(Tier::Volatile).check(&sel).unwrap_err(),
        PolicyError::ProfileCountUnknown
    );
    assert!(
        Policy::allowing(Tier::Volatile)
            .with_profile_count(3)
            .check(&sel)
            .is_ok()
    );
    assert_eq!(
        Policy::allowing(Tier::Volatile)
            .with_profile_count(2)
            .check(&sel)
            .unwrap_err(),
        PolicyError::ProfileOutOfRange {
            profile: 2,
            count: 2
        }
    );
}

fn pe(p: &Policy, bytes: &[u8]) -> PolicyError {
    p.check_bytes(bytes).unwrap_err()
}

#[test]
fn malformed_and_near_miss_frames_are_refused() {
    let p = all_open();
    for f in [&[0xA9, 0x10, 0x01][..], &[0x01, 0x05]] {
        assert!(
            matches!(pe(&p, f), PolicyError::TrailingBytes { .. }),
            "{f:02x?}"
        );
    }
    // A3 only as `a3 00 ff`.
    for f in [&[0xA3][..], &[0xA3, 0x00, 0x00], &[0xA3, 0x01, 0xFF]] {
        assert!(
            matches!(pe(&p, f), PolicyError::NotAllowlisted { .. }),
            "{f:02x?}"
        );
    }
    // Unknown lighting value ids.
    for f in [
        &[0x08, 0x03, 0x00][..],
        &[0x08, 0x03, 0x05],
        &[0x07, 0x03, 0x05, 0x01],
    ] {
        assert!(
            matches!(pe(&p, f), PolicyError::NotAllowlisted { .. }),
            "{f:02x?}"
        );
    }
    // SET_TRAVAL mode 0 (global) and 3.
    for mode in [0u8, 3] {
        let f = [0xA9, 0x14, 0x00, mode, 20, 4, 4, 0x01];
        assert!(
            matches!(pe(&p, &f), PolicyError::OutOfRange { field: "mode", .. }),
            "mode {mode}"
        );
    }
    // Profile at or above the count.
    assert_eq!(
        pe(&p, &[0xA9, 0x12, 0x03, 0x00, 0x00, 0x1A]),
        PolicyError::ProfileOutOfRange {
            profile: 3,
            count: 3
        }
    );
    assert_eq!(
        pe(&p, &[0xA9, 0x1F, 0x03]),
        PolicyError::ProfileOutOfRange {
            profile: 3,
            count: 3
        }
    );
    // An over-long frame is still Forbidden when it starts with a never-send command.
    let mut long = [0u8; 40];
    long[0] = 0xAB;
    assert!(matches!(pe(&p, &long), PolicyError::Forbidden { .. }));
    long[0] = 0xA0;
    assert_eq!(pe(&p, &long), PolicyError::FrameTooLong(40));
}

#[test]
fn reads_past_a_known_size_are_refused_and_unknown_sizes_refuse() {
    let p = common::sized(Policy::read_only());
    let past = |p: &Policy, r: Request| p.check(&r).unwrap_err();
    let profile = |offset, len| Request::GetProfileBuffer {
        profile: 0,
        offset,
        len,
    };
    assert!(p.check(&profile(834, 26)).is_ok());
    assert!(matches!(
        past(&p, profile(835, 26)),
        PolicyError::PastEnd {
            end: 861,
            limit: 860,
            ..
        }
    ));
    let mac = |offset, len| Request::GetMacroBuffer { offset, len };
    assert!(p.check(&mac(1999, 28)).is_ok());
    assert!(matches!(
        past(&p, mac(2000, 28)),
        PolicyError::PastEnd {
            end: 2028,
            limit: 2027,
            ..
        }
    ));
    let km = |offset, len| Request::GetKeymapBuffer { offset, len };
    assert!(p.check(&km(740, 28)).is_ok());
    assert!(matches!(
        past(&p, km(756, 28)),
        PolicyError::PastEnd {
            end: 784,
            limit: 768,
            ..
        }
    ));
    assert!(
        p.check(&Request::GetLedColors {
            start: 81,
            count: 3
        })
        .is_ok()
    );
    assert!(matches!(
        past(
            &p,
            Request::GetLedColors {
                start: 82,
                count: 3
            }
        ),
        PolicyError::PastEnd {
            end: 85,
            limit: 84,
            ..
        }
    ));
    assert!(matches!(
        past(
            &p,
            Request::GetMixRegions {
                start: 60,
                count: 29
            }
        ),
        PolicyError::PastEnd { .. }
    ));
    assert!(p.check(&Request::GetLedRow { row: 5 }).is_ok());
    assert!(matches!(
        past(&p, Request::GetLedRow { row: 6 }),
        PolicyError::OutOfRange {
            field: "led row",
            ..
        }
    ));
    // Nothing supplied: every bounded read refuses.
    let bare = Policy::read_only().with_profile_count(3);
    for r in [
        profile(0, 26),
        mac(0, 28),
        km(0, 28),
        Request::GetLedColors { start: 0, count: 1 },
        Request::GetMixRegions { start: 0, count: 1 },
        Request::GetLedRow { row: 0 },
    ] {
        assert!(
            matches!(past(&bare, r), PolicyError::LimitUnknown { .. }),
            "{r:?}"
        );
    }
}

fn set_key(layer: u8, row: u8, col: u8, code: u16) -> Request {
    Request::SetKeycode {
        layer,
        row,
        col,
        code,
    }
}

#[test]
fn set_keycode_encodes_big_endian_and_needs_the_persistent_gate() {
    let req = set_key(2, 0, 15, 0x004C);
    assert_eq!(req.tier(), Tier::Persistent);
    let checked = all_open().check(&req).unwrap();
    assert_eq!(checked.encode(), pad(&[0x05, 2, 0, 15, 0x00, 0x4C]));
    assert_eq!(
        all_open()
            .check(&set_key(2, 0, 14, 0x7821))
            .unwrap()
            .encode(),
        pad(&[0x05, 2, 0, 14, 0x78, 0x21])
    );
    // Bytes 1..=3 are matched, the keycode bytes are not.
    assert!(checked.matches(&pad(&[0x05, 2, 0, 15, 0x12, 0x34])));
    assert!(!checked.matches(&pad(&[0x05, 2, 0, 14, 0x00, 0x4C])));
    // The raw-byte entry gives the same frame.
    assert_eq!(
        all_open()
            .check_bytes(&[0x05, 2, 0, 15, 0x00, 0x4C])
            .unwrap()
            .encode(),
        checked.encode()
    );
    for tier in [Tier::Read, Tier::Volatile] {
        let policy = common::sized(Policy::allowing(tier));
        assert_eq!(
            policy.check(&req).unwrap_err(),
            PolicyError::TierNotEnabled {
                tier: Tier::Persistent
            }
        );
    }
}

#[test]
fn set_keycode_outside_the_matrix_is_refused_before_encode() {
    let p = all_open();
    assert!(p.check(&set_key(3, 5, 15, 0x0004)).is_ok());
    for (req, field) in [
        (set_key(4, 0, 0, 0x0004), "layer"),
        (set_key(0, 6, 0, 0x0004), "row"),
        (set_key(0, 0, 16, 0x0004), "col"),
    ] {
        match p.check(&req) {
            Err(PolicyError::OutOfRange { field: f, .. }) => assert_eq!(f, field),
            other => panic!("{req:?}: {other:?}"),
        }
    }
    // Without limits from the device layer nothing is sent.
    let bare = Policy::allowing(Tier::Persistent);
    assert!(matches!(
        bare.check(&set_key(0, 0, 0, 0x0004)),
        Err(PolicyError::LimitUnknown { .. })
    ));
}

#[test]
fn set_keycode_refuses_the_quantum_block_and_unknown_codes() {
    let p = all_open();
    // QK_BOOT, QK_REBOOT, QK_DEBUG_TOGGLE, QK_CLEAR_EEPROM and the block edges.
    for code in [0x7C00, 0x7C01, 0x7C02, 0x7C03, 0x7C77, 0x7DFF] {
        assert!(
            matches!(
                p.check(&set_key(0, 0, 0, code)),
                Err(PolicyError::Invalid {
                    field: "keycode",
                    ..
                })
            ),
            "{code:#06x}"
        );
        assert!(
            p.check_bytes(&[0x05, 0, 0, 0, (code >> 8) as u8, code as u8])
                .is_err()
        );
    }
    // Unnameable or unsafe: layer keys, mod-taps, undefined basic codes, unnamed custom keys.
    for code in [
        0x5223, 0x2004, 0x0002, 0x00A4, 0x7E40, 0x7834, 0xFFFF, 0x7E0B, 0x7E0C, 0x7E0D, 0x7E0E,
        0x0066, 0x00A5, 0x00A6, 0x00A7,
    ] {
        assert!(p.check(&set_key(0, 0, 0, code)).is_err(), "{code:#06x}");
    }
    // KC_NO, KC_TRNS, basic, RGB, a named Keychron key.
    for code in [
        0x0000, 0x0001, 0x004C, 0x00E7, 0x7820, 0x7821, 0x7833, 0x7E00,
    ] {
        assert!(p.check(&set_key(0, 0, 0, code)).is_ok(), "{code:#06x}");
    }
}

#[test]
fn parse_code_reads_hex_and_names() {
    use keychron_proto::keycode::parse_code;
    assert_eq!(parse_code("0x004C"), Some(0x004C));
    assert_eq!(parse_code("KC_DEL"), Some(0x004C));
    assert_eq!(parse_code("rgb_mod"), Some(0x7821));
    assert_eq!(parse_code("KC_NO"), Some(0));
    assert_eq!(parse_code("KC_TRNS"), Some(1));
    assert_eq!(parse_code("NOPE"), None);
}
