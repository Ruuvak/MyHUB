mod common;

use common::{keymap_reply, keymap_row};
use keychron_proto::plan::{KeymapPlan, MacroPlan, ProfileBufferPlan};
use keychron_proto::{DecodeError, Keycode, Request};

/// Flat big-endian image of the four fixture layers, 6 x 16 keys each.
fn image() -> Vec<u8> {
    let mut img = Vec::new();
    for layer in 0..4 {
        for row in 0..6 {
            for code in keymap_row(layer, row) {
                img.extend_from_slice(&code.to_be_bytes());
            }
        }
    }
    assert_eq!(img.len(), 4 * 192);
    img
}

fn read_layer(layer: u8) -> keychron_proto::plan::Keymap {
    let img = image();
    let plan = KeymapPlan::new(layer, 6, 16).unwrap();
    let replies: Vec<_> = plan
        .requests()
        .iter()
        .map(|r| match *r {
            Request::GetKeymapBuffer { offset, len } => {
                keymap_reply(&img, usize::from(offset), usize::from(len))
            }
            _ => unreachable!(),
        })
        .collect();
    plan.assemble(&replies).unwrap()
}

#[test]
fn keymap_plan_chunks_follow_the_offset_formula() {
    let plan = KeymapPlan::new(2, 6, 16).unwrap();
    let reqs = plan.requests();
    assert_eq!(reqs.len(), 7);
    assert_eq!(
        reqs[0],
        Request::GetKeymapBuffer {
            offset: 2 * 192,
            len: 28
        }
    );
    assert_eq!(
        reqs[1],
        Request::GetKeymapBuffer {
            offset: 2 * 192 + 28,
            len: 28
        }
    );
    // 96 keys = 6 chunks of 14 keys + 1 chunk of 12 keys.
    assert_eq!(
        reqs[6],
        Request::GetKeymapBuffer {
            offset: 2 * 192 + 168,
            len: 24
        }
    );
    assert!(KeymapPlan::new(0, 0, 16).is_err());
    assert!(KeymapPlan::new(255, 255, 255).is_err());
}

#[test]
fn keymap_layer_2_row_5_decodes_with_mo3() {
    let map = read_layer(2);
    assert_eq!(map.keys.len(), 96);
    assert_eq!(map.row(5).unwrap(), keymap_row(2, 5).as_slice());
    let labels: Vec<String> = map
        .row(5)
        .unwrap()
        .iter()
        .map(|&c| Keycode::decode(c).to_string())
        .collect();
    assert_eq!(labels[0], "KC_LCTL");
    assert_eq!(labels[1], "KC_LGUI");
    assert_eq!(labels[2], "KC_LALT");
    assert_eq!(labels[3], "KC_NO");
    assert_eq!(labels[6], "KC_SPC");
    assert_eq!(labels[9], "KC_RALT");
    assert_eq!(labels[10], "MO(3)");
    assert_eq!(map.get(5, 10), Some(0x5223));
    assert_eq!(Keycode::decode(0x5223), Keycode::Momentary(3));
}

#[test]
fn keymap_layer_3_has_transparent_keys_and_custom_names() {
    let map = read_layer(3);
    assert_eq!(
        Keycode::decode(map.get(5, 0).unwrap()),
        Keycode::Transparent
    );
    assert_eq!(
        Keycode::decode(map.get(0, 3).unwrap()).to_string(),
        "Task View"
    );
    assert_eq!(
        Keycode::decode(map.get(1, 1).unwrap()).to_string(),
        "Bluetooth Host 1"
    );
    // Media keys on the Win Fn row 0.
    assert_eq!(
        Keycode::decode(map.get(0, 1).unwrap()).to_string(),
        "KC_BRID"
    );
    assert_eq!(
        Keycode::decode(map.get(0, 7).unwrap()).to_string(),
        "KC_MPRV"
    );
    // Last layer starts at offset 576 and still fits.
    assert_eq!(map.layer, 3);
}

#[test]
fn keymap_assembler_rejects_wrong_replies() {
    let img = image();
    let plan = KeymapPlan::new(0, 6, 16).unwrap();
    assert_eq!(
        plan.assemble(&[]).unwrap_err(),
        DecodeError::ReplyCount {
            expected: 7,
            got: 0
        }
    );
    let mut replies: Vec<_> = plan
        .requests()
        .iter()
        .map(|r| match *r {
            Request::GetKeymapBuffer { offset, len } => {
                keymap_reply(&img, usize::from(offset), usize::from(len))
            }
            _ => unreachable!(),
        })
        .collect();
    replies[2][2] ^= 0x04;
    assert_eq!(
        plan.assemble(&replies).unwrap_err(),
        DecodeError::ReplyMismatch { chunk: 2 }
    );
}

#[test]
fn keycode_ranges_decode() {
    assert_eq!(Keycode::decode(0x0000), Keycode::No);
    assert_eq!(Keycode::decode(0x0004).to_string(), "KC_A");
    assert_eq!(Keycode::decode(0x7821).to_string(), "RGB_MOD");
    assert_eq!(Keycode::decode(0x7820).to_string(), "RGB_TOG");
    assert_eq!(Keycode::decode(0x7833).to_string(), "RGB_M_T");
    assert_eq!(Keycode::decode(0x7013), Keycode::NkroToggle);
    assert_eq!(Keycode::decode(0x7e00).to_string(), "Left Option");
    assert_eq!(Keycode::decode(0x7e12).to_string(), "Profile 3");
    assert_eq!(Keycode::decode(0x7e40).to_string(), "CUSTOM(64)");
    assert_eq!(Keycode::decode(0x5221), Keycode::Momentary(1));
    assert_eq!(Keycode::decode(0x5200), Keycode::To(0));
    assert_eq!(Keycode::decode(0x5240), Keycode::DefaultLayer(0));
    assert_eq!(Keycode::decode(0x5262), Keycode::Toggle(2));
    assert_eq!(Keycode::decode(0x5283), Keycode::OneShotLayer(3));
    assert_eq!(Keycode::decode(0x52A2), Keycode::OneShotMods(2));
    assert_eq!(Keycode::decode(0x52C1), Keycode::TapToggle(1));
    assert_eq!(Keycode::decode(0x7702), Keycode::Macro(2));
    assert_eq!(Keycode::decode(0x0104).to_string(), "LCTL(KC_A)");
    assert_eq!(Keycode::decode(0x0304).to_string(), "LCTL(LSFT(KC_A))");
    assert_eq!(Keycode::decode(0x1204).to_string(), "RSFT(KC_A)");
    assert_eq!(Keycode::decode(0x2104).to_string(), "MT(LCTL, KC_A)");
    assert_eq!(Keycode::decode(0x412C).to_string(), "LT(1, KC_SPC)");
    assert_eq!(Keycode::decode(0x5022).to_string(), "LM(1, LSFT)");
    assert_eq!(Keycode::decode(0x7C00), Keycode::Unknown(0x7C00));
    assert_eq!(Keycode::decode(0x7C00).to_string(), "0x7c00");
}

#[test]
fn he_and_macro_plans_chunk_and_join() {
    let plan = ProfileBufferPlan::new(1, 860).unwrap();
    assert_eq!(plan.requests().len(), 34);
    assert_eq!(
        plan.requests()[33],
        Request::GetProfileBuffer {
            profile: 1,
            offset: 858,
            len: 2
        }
    );
    // Hand-built replies: echo of bytes 2..5, data at 6.
    let data: Vec<u8> = (0..860u32).map(|i| (i * 7 % 251) as u8).collect();
    let replies: Vec<_> = (0..34usize)
        .map(|i| {
            let off = i * 26;
            let len = 26.min(860 - off);
            let mut f = [0u8; 32];
            f[..6].copy_from_slice(&[
                0xA9,
                0x12,
                1,
                (off & 0xFF) as u8,
                (off >> 8) as u8,
                len as u8,
            ]);
            f[6..6 + len].copy_from_slice(&data[off..off + len]);
            f
        })
        .collect();
    assert_eq!(plan.assemble(&replies).unwrap(), data);

    let mplan = MacroPlan::new(2027).unwrap();
    assert_eq!(mplan.requests().len(), 73);
    assert_eq!(
        mplan.requests()[72],
        Request::GetMacroBuffer {
            offset: 72 * 28,
            len: 11
        }
    );
    let mdata: Vec<u8> = (0..2027u32).map(|i| (i % 253) as u8).collect();
    let mreplies: Vec<_> = (0..73usize)
        .map(|i| {
            let off = i * 28;
            let len = 28.min(2027 - off);
            let mut f = [0u8; 32];
            f[..4].copy_from_slice(&[0x0E, (off >> 8) as u8, off as u8, len as u8]);
            f[4..4 + len].copy_from_slice(&mdata[off..off + len]);
            f
        })
        .collect();
    assert_eq!(mplan.assemble(&mreplies).unwrap(), mdata);
}
