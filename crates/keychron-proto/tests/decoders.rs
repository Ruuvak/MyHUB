mod common;

use common::{pad, read_pairs, reply_for};
use keychron_proto::he::{decode_curve, decode_he_info, decode_profiles_info};
use keychron_proto::lighting::{
    decode_indicators, decode_led_caps, decode_led_colors, decode_led_row, decode_mix_info,
    decode_mix_slots,
};
use keychron_proto::system::{
    decode_features, decode_firmware, decode_kc_version, decode_layer, decode_layout_lang,
    decode_misc_info, decode_timers,
};
use keychron_proto::via::{
    decode_layer_count, decode_light, decode_macro_buffer_size, decode_macro_count, decode_protocol,
};
use keychron_proto::{DecodeError, LightId, LightValue, Policy, Request};

#[test]
fn every_golden_request_is_allowlisted_and_matches_its_reply() {
    let pairs = read_pairs();
    assert_eq!(pairs.len(), 37);
    for (req, reply) in pairs {
        let checked = common::sized(Policy::read_only())
            .check_bytes(&req)
            .unwrap_or_else(|e| panic!("{req:02x?}: {e}"));
        assert_eq!(checked.encode(), pad(&req), "round trip of {req:02x?}");
        assert!(checked.matches(&reply), "{req:02x?}");
        // A reply for another command must not match.
        let mut other = reply;
        other[0] ^= 0x01;
        assert!(!checked.matches(&other), "{req:02x?}");
    }
}

#[test]
fn system_replies_decode() {
    let v = decode_kc_version(&reply_for("a0")).unwrap();
    assert_eq!((v.kc_version, v.instruction_set), (2, 2));

    assert_eq!(
        decode_firmware(&reply_for("a1")).unwrap(),
        "v1.2.1 2025-06-17-10:09:52"
    );

    let f = decode_features(&reply_for("a2"), 2).unwrap();
    assert_eq!((f.f0, f.f1), (0x8F, 0x00));
    assert!(f.get_default_layer && f.support_le && f.support_rf && f.support_mock_key);
    assert!(f.support_keychron_rgb && !f.upload_default_layer && !f.quick_start);
    assert_eq!(
        decode_features(&reply_for("a2"), 9).unwrap_err(),
        DecodeError::UnsupportedInstructionSet(9)
    );

    let l = decode_layer(&reply_for("a3 00 ff")).unwrap();
    assert_eq!((l.default_layer, l.current_layer), (2, None));
    let mut known = reply_for("a3 00 ff");
    known[2] = 3;
    assert_eq!(decode_layer(&known).unwrap().current_layer, Some(3));

    let m = decode_misc_info(&reply_for("a7 01")).unwrap();
    assert_eq!((m.status, m.version, m.flags), (0, 2, 0x13));
    assert!(!m.single_key_reset);
    assert_eq!(decode_layout_lang(&reply_for("a7 03")).unwrap().index, 0);

    let t = decode_timers(&reply_for("a7 0b")).unwrap();
    assert_eq!(
        (t.backlight_off_s, t.sleep_s, t.he_scan_idle_s),
        (600, 7200, 1800)
    );
    // Wrong reply type is an error, not a misread.
    assert!(matches!(
        decode_timers(&reply_for("a7 03")),
        Err(DecodeError::WrongReply { index: 1, .. })
    ));
}

#[test]
fn via_replies_decode() {
    assert_eq!(decode_protocol(&reply_for("01")).unwrap(), 12);
    assert_eq!(decode_layer_count(&reply_for("11")).unwrap(), 4);
    assert_eq!(decode_macro_count(&reply_for("0c")).unwrap(), 16);
    assert_eq!(decode_macro_buffer_size(&reply_for("0d")).unwrap(), 2027);
    assert_eq!(
        decode_light(&reply_for("08 03 01"), LightId::Brightness).unwrap(),
        LightValue::Brightness(255)
    );
    assert_eq!(
        decode_light(&reply_for("08 03 02"), LightId::Effect).unwrap(),
        LightValue::Effect(5)
    );
    assert_eq!(
        decode_light(&reply_for("08 03 03"), LightId::Speed).unwrap(),
        LightValue::Speed(127)
    );
    assert_eq!(
        decode_light(&reply_for("08 03 04"), LightId::Color).unwrap(),
        LightValue::Color {
            hue: 0,
            saturation: 255
        }
    );
    // A reply for another id is refused.
    assert!(decode_light(&reply_for("08 03 04"), LightId::Speed).is_err());
}

#[test]
fn lighting_replies_decode() {
    let caps = decode_led_caps(&reply_for("a8 01")).unwrap();
    assert!(caps.per_key && caps.mix);

    let ind = decode_indicators(&reply_for("a8 03")).unwrap();
    assert_eq!(
        (ind.support_mask, ind.disable_mask, ind.hue, ind.saturation),
        (0x02, 0x00, 255, 0)
    );

    // LED row 4: LED 0x3c at col 0, none at col 1, only the first 16 columns.
    let row = decode_led_row(&reply_for("a8 06 04 ff ff ff"), 16).unwrap();
    assert_eq!(row.status, 0);
    assert_eq!(row.leds.len(), 16);
    assert_eq!(row.leds[0], Some(0x3C));
    assert_eq!(row.leds[1], None);
    assert_eq!(row.leds[2], Some(0x3D));
    assert_eq!(row.leds[15], None);
    assert!(decode_led_row(&reply_for("a8 06 04 ff ff ff"), 30).is_err());

    let colors = decode_led_colors(&reply_for("a8 09 00 09"), 9).unwrap();
    assert_eq!(colors.colors.len(), 9);
    assert_eq!((colors.colors[0].hue, colors.colors[1].hue), (0x00, 0x2B));
    let last = decode_led_colors(&reply_for("a8 09 51 03"), 3).unwrap();
    assert_eq!(last.colors.len(), 3);

    let mix = decode_mix_info(&reply_for("a8 0b")).unwrap();
    assert_eq!((mix.regions, mix.slots_per_region), (2, 5));
    let slots = decode_mix_slots(&reply_for("a8 0e 00 00 03"), 3).unwrap();
    assert_eq!(slots.slots[0].effect, 5);
    assert_eq!(slots.slots[0].duration_ms, 5000);
    assert_eq!(slots.slots[0].speed, 127);
    assert!(slots.slots[1].is_empty() && slots.slots[2].is_empty());
}

#[test]
fn he_replies_decode() {
    let info = decode_he_info(&reply_for("a9 01")).unwrap();
    assert_eq!((info.version, info.feature_byte), (4, 0));
    assert!(!info.dkr && !info.turbo && !info.reset_single_key);
    let mut v5 = reply_for("a9 01");
    v5[2] = 5;
    assert_eq!(
        decode_he_info(&v5).unwrap_err(),
        DecodeError::UnsupportedHeVersion(5)
    );

    let p = decode_profiles_info(&reply_for("a9 10")).unwrap();
    assert_eq!(
        (p.current, p.count, p.size, p.dks_count, p.snap_count),
        (0, 3, 860, 20, 20)
    );

    let c = decode_curve(&reply_for("a9 20")).unwrap();
    let pts: Vec<(u8, u8)> = c.points.iter().map(|p| (p.x, p.y)).collect();
    assert_eq!(pts, [(0, 0), (10, 31), (30, 95), (40, 127)]);
}

#[test]
fn replies_serialize_for_the_ui() {
    // Compile-time and runtime check that decoded structs are Serialize.
    fn is_serialize<T: serde::Serialize>(_: &T) {}
    is_serialize(&decode_timers(&reply_for("a7 0b")).unwrap());
    is_serialize(&decode_profiles_info(&reply_for("a9 10")).unwrap());
    // Requests for reads exist for every decoder above.
    assert!(Request::GetProfilesInfo.matches(&reply_for("a9 10")));
}
