//! Replays the transcribed fixtures (own headset, addresses scrubbed) and asserts the
//! values quoted in `DEVICE-WF-C710N.md`. No test opens a device.

mod common;

use common::*;
use mdr::messages::{
    self, AmbientMode, AudioCodec, ChargingStatus, DeviceInfo, GeneralSettingKind, NcAsmMode,
    ValueChangeStatus,
};
use mdr::{
    DataType, DeviceLimits, FunctionTable, Incoming, Link, LinkEvent, Message, Origin, Policy,
    Request, Table, decode_frame, parse,
};

fn find<'a, T>(msgs: &'a [Incoming], f: impl Fn(&'a Message) -> Option<T>) -> Option<T> {
    msgs.iter().find_map(|m| f(&m.message))
}

#[test]
fn every_fixture_frame_decodes_cleanly() {
    let mut n = 0;
    for text in ALL_FIXTURES {
        for (_, wire) in load(text) {
            decode_frame(&wire).expect("fixture frame valid");
            n += 1;
        }
        headset_frames(text);
    }
    assert!(n > 150, "{n} frames");
}

#[test]
fn fixtures_carry_no_address_text() {
    for text in ALL_FIXTURES {
        for (_, wire) in load(text) {
            let f = decode_frame(&wire).unwrap();
            // 17 ASCII characters XX:XX:XX:XX:XX:XX would show up as a colon every third byte.
            let has_mac = f.payload.windows(17).any(|w| {
                w.iter().enumerate().all(|(i, b)| {
                    if i % 3 == 2 {
                        *b == b':'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                })
            });
            if has_mac {
                assert!(
                    f.payload.windows(17).any(|w| w == b"00:11:22:33:44:55"),
                    "only the scrubbed address may appear"
                );
            }
        }
    }
}

#[test]
fn handshake_identity_and_protocol_info() {
    let m = headset_messages(HANDSHAKE);
    let info = find(&m, |m| match m {
        Message::ProtocolInfo(i) => Some(i),
        _ => None,
    })
    .expect("protocol info");
    // DEVICE-WF-C710N.md quotes 0x00030030, but the logged payload is
    // `01 00 03 00 30 00 00 00`, which as `01 00 <u32 BE> <t1> <t2>` is 0x03003000.
    assert_eq!(info.version, 0x0300_3000);
    assert!(info.table1_enabled && info.table2_enabled);

    assert!(m.iter().any(
        |x| matches!(&x.message, Message::DeviceInfo(DeviceInfo::Model(s)) if s == "WF-C710N")
    ));
    assert!(m.iter().any(
        |x| matches!(&x.message, Message::DeviceInfo(DeviceInfo::Firmware(s)) if s == "1.0.5")
    ));
    assert!(m.iter().any(|x| matches!(
        &x.message,
        Message::DeviceInfo(DeviceInfo::SeriesAndColor(0x50, 0x01))
    )));
}

#[test]
fn handshake_function_tables() {
    let ft = function_table();
    assert_eq!(ft.count(Table::One), 28);
    assert_eq!(ft.count(Table::Two), 5);
    assert_eq!(ft.version(Table::One, 0x6B), Some(0x04));
    assert_eq!(ft.version(Table::One, 0x50), Some(0x0A));
    assert!(ft.contains(Table::One, 0xD1));
    assert!(ft.contains(Table::One, 0xD2));
    assert_eq!(ft.version(Table::One, 0xF9), Some(0x37));
    assert_eq!(ft.version(Table::Two, 0x42), Some(0x34));
    assert_eq!(ft.version(Table::Two, 0x32), Some(0x02));
    assert!(!ft.contains(Table::Two, 0xD1));
    assert!(ft.has_ncasm_type(0x17));
    assert!(ft.has_eq_type(0x00));
    let entries = ft.entries();
    let nc = entries
        .iter()
        .find(|e| e.table == Table::One && e.id == 0x6B)
        .unwrap();
    assert_eq!(nc.name, Some("NC_ASM"));
    let raw = entries
        .iter()
        .find(|e| e.table == Table::One && e.id == 0x10)
        .unwrap();
    assert_eq!(raw.name, None);
}

#[test]
fn pushed_battery_frames() {
    let m = headset_messages(HANDSHAKE);
    let lr = find(&m, |m| match m {
        Message::BatteryLr(b) => Some(*b),
        _ => None,
    })
    .unwrap();
    assert_eq!((lr.left.level, lr.right.level), (100, 100));
    assert_eq!(lr.left.charging, ChargingStatus::NotCharging);
    assert_eq!((lr.left_threshold, lr.right_threshold), (20, 20));
    let cradle = find(&m, |m| match m {
        Message::Cradle(c) => Some(*c),
        _ => None,
    })
    .unwrap();
    assert_eq!(cradle.cell.level, 50);
    assert_eq!(cradle.cell.charging, ChargingStatus::NotCharging);
    assert_eq!(cradle.threshold, 21);
    // The pushed frames are notifications, the connection status frame is not decoded.
    assert!(m.iter().any(|x| x.origin == Origin::Notification
        && matches!(x.message, Message::BatteryLr(_))));
    assert!(
        m.iter().any(
            |x| matches!(&x.message, Message::Unknown(u) if u.raw == [0x15, 0x01, 0x01, 0x01])
        )
    );
}

#[test]
fn features_ncasm_eq_battery_replies() {
    let m = headset_messages(FEATURES);
    let cap = find(&m, |m| match m {
        Message::NcAsmCapability(c) => Some(c),
        _ => None,
    })
    .unwrap();
    assert_eq!(cap.ranges().len(), 2);
    assert_eq!(cap.ranges()[0].ambient_mode(), AmbientMode::Normal);
    assert_eq!(cap.ranges()[1].ambient_mode(), AmbientMode::Voice);
    for r in cap.ranges() {
        assert_eq!((r.min(), r.max(), r.step()), (1, 20, 1));
    }
    let p = find(&m, |m| match m {
        Message::NcAsmParam(p) => Some(*p),
        _ => None,
    })
    .unwrap();
    assert_eq!(p.status, ValueChangeStatus::Changed);
    assert!(p.total_effect);
    assert_eq!(p.mode, NcAsmMode::NoiseCancelling);
    assert_eq!(p.ambient_mode, AmbientMode::Normal);
    assert_eq!(p.level, 20);

    let eq = find(&m, |m| match m {
        Message::EqCapability(c) => Some(c),
        _ => None,
    })
    .unwrap();
    assert_eq!(eq.band_count(), 6);
    assert_eq!(eq.step_count(), 0x15);
    assert_eq!(eq.presets().len(), 12);
    let ids: Vec<u8> = eq.presets().iter().map(|p| p.id()).collect();
    assert_eq!(
        ids,
        [
            0x00, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0xA0, 0xA1, 0xA2
        ]
    );
    assert!(eq.presets().iter().all(|p| p.extra().is_empty()));

    let state = find(&m, |m| match m {
        Message::EqParam(p) => Some(p),
        _ => None,
    })
    .unwrap();
    assert_eq!(state.preset, 0xA0);
    assert_eq!(state.bands, [0x10, 0x0E, 0x0B, 0x0A, 0x0C, 0x0F]);

    // Pull replies carry the same layout as the notifications.
    assert!(
        m.iter()
            .any(|x| x.origin == Origin::Reply && matches!(x.message, Message::BatteryLr(_)))
    );
    assert!(
        m.iter()
            .any(|x| x.origin == Origin::Reply && matches!(x.message, Message::Cradle(_)))
    );
    // `12 02` is not in this log, the adaptive control capability stays raw.
    assert!(
        m.iter()
            .any(|x| matches!(&x.message, Message::Unknown(u) if u.command == 0x71))
    );
    assert!(
        m.iter()
            .any(|x| matches!(&x.message, Message::Unknown(u) if u.command == 0xD1))
    );
}

#[test]
fn eq_notifications_and_the_bare_changed_signal() {
    let preset = headset_messages(EQ_PRESET);
    let bass = preset
        .iter()
        .find_map(|x| match (&x.message, x.origin) {
            (Message::EqParam(p), Origin::Notification) => Some(p),
            _ => None,
        })
        .unwrap();
    assert_eq!(bass.preset, 0x16);
    assert_eq!(bass.bands, [0x11, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A]);
    assert_eq!(messages::eq_preset_name(bass.preset), Some("BASS"));

    let bands = headset_messages(EQ_BANDS);
    assert!(
        bands
            .iter()
            .any(|x| x.message == Message::EqChanged && x.origin == Origin::Notification)
    );
    // A bare `59 00` is a signal, never a state.
    assert!(
        !bands
            .iter()
            .any(|x| matches!(&x.message, Message::EqParam(p) if p.bands.is_empty()))
    );

    // Select CUSTOM again: the headset pushes the stored bands.
    let restore = headset_messages(EQ_RESTORE);
    assert!(
        restore
            .iter()
            .any(|x| matches!(&x.message, Message::EqParam(p)
        if p.preset == 0xA0 && p.bands == [0x10, 0x0E, 0x0B, 0x0A, 0x0C, 0x0F])
                && x.origin == Origin::Notification)
    );
}

#[test]
fn general_settings_and_inverted_values() {
    let m = headset_messages(TOGGLES);
    let gs: Vec<_> = m
        .iter()
        .filter_map(|x| match &x.message {
            Message::GeneralSetting(g) => Some((g.kind, g.enabled, x.origin)),
            _ => None,
        })
        .collect();
    // d7 d2 00 00 original sidetone: value 00 = ON.
    assert_eq!(gs[0], (GeneralSettingKind::Sidetone, true, Origin::Reply));
    // d9 d2 00 01 notification after the SET: value 01 = OFF.
    assert_eq!(
        gs[1],
        (GeneralSettingKind::Sidetone, false, Origin::Notification)
    );
    // d7 d2 00 01 reply: sidetone off.
    assert_eq!(gs[2], (GeneralSettingKind::Sidetone, false, Origin::Reply));
    assert!(gs.contains(&(GeneralSettingKind::Multipoint, true, Origin::Reply)));
    // Multipoint after a confirmed change reads OFF (`d7 d1 00 01`).
    let mp = headset_messages(MULTIPOINT);
    assert!(
        mp.iter()
            .any(|x| matches!(&x.message, Message::GeneralSetting(g)
        if g.kind == GeneralSettingKind::Multipoint && !g.enabled))
    );
}

#[test]
fn alert_from_the_multipoint_change() {
    let mut m = headset_messages(MULTIPOINT);
    let mut alert = m
        .iter_mut()
        .find_map(|x| match &mut x.message {
            Message::Alert(a) => Some(a),
            _ => None,
        })
        .expect("alert");
    assert_eq!(alert.message_id, 0x07);
    assert_eq!(alert.action_type, 0x01);
    assert!(alert.is_multipoint_disconnect());
    let token = alert.take_token().expect("POSITIVE_NEGATIVE gives a token");
    assert_eq!(token.message_id(), 0x07);
    assert!(alert.take_token().is_none(), "token can be taken once");
    alert = &mut *alert;
    let json = serde_json::to_value(&*alert).unwrap();
    assert_eq!(json["messageId"], 7);
    assert_eq!(json["actionType"], 1);
    assert!(json.get("token").is_none());
}

#[test]
fn alert_without_positive_negative_action_gives_no_token() {
    let mut inc = parse(Table::One, &[0x99, 0x00, 0x07, 0x00]);
    let Message::Alert(a) = &mut inc.message else {
        panic!("alert")
    };
    assert!(a.take_token().is_none());
}

#[test]
fn ncasm_set_round_trip_ambient_level_10() {
    // The NTFY after `68 17 01 01 01 00 0a` carries the same fields as the SET.
    let m = headset_messages(NCASM_SET);
    let ntfy = m
        .iter()
        .find_map(|x| match (&x.message, x.origin) {
            (Message::NcAsmParam(p), Origin::Notification) => Some(*p),
            _ => None,
        })
        .unwrap();
    assert_eq!(ntfy.mode, NcAsmMode::Ambient);
    assert_eq!(ntfy.ambient_mode, AmbientMode::Normal);
    assert_eq!(ntfy.level, 10);
    let req = messages::ncasm_set_param(0x17, &ntfy);
    assert_eq!(req.payload, [0x68, 0x17, 0x01, 0x01, 0x01, 0x00, 0x0A]);

    // Through the policy it encodes to the exact frame that was sent (seq 1).
    let policy = Policy::new().with_setting_changes();
    let sent = load(NCASM_SET)
        .into_iter()
        .find(|(tx, w)| *tx && decode_frame(w).unwrap().payload.first() == Some(&0x68))
        .unwrap()
        .1;
    assert_eq!(wire_of(&policy, &req, 1), sent);
}

#[test]
fn codec_parses_aac() {
    let inc = parse(Table::One, &[0x13, 0x02, 0x02]);
    assert_eq!(inc.message, Message::Codec(AudioCodec::Aac));
    assert_eq!(inc.origin, Origin::Reply);
}

#[test]
fn unknown_and_malformed_payloads_never_fail() {
    let cases: [&[u8]; 9] = [
        &[],
        &[0xFF],
        &[0x67, 0x17, 0x01],
        &[0x67, 0x17, 0x07, 0x01, 0x00, 0x00, 0x14],
        &[0x61, 0x17, 0x05, 0x00],
        &[0x57, 0x00, 0xA0, 0x06, 0x10],
        &[0xD7, 0xD3, 0x00, 0x00],
        &[0xD7, 0xD1, 0x00, 0x02],
        &[0x07, 0x00, 0x03, 0x10],
    ];
    for c in cases {
        let inc = parse(Table::One, c);
        assert!(matches!(inc.message, Message::Unknown(_)), "{c:02x?}");
        assert_eq!(inc.origin, Origin::Other);
    }
    // The same bytes on table 2 are not battery or NC/ASM messages.
    let inc = parse(
        Table::Two,
        &[0x25, 0x09, 0x64, 0x00, 0x64, 0x00, 0x14, 0x14],
    );
    assert!(matches!(inc.message, Message::Unknown(_)));
}

#[test]
fn messages_serialize_in_camel_case() {
    let m = headset_messages(HANDSHAKE);
    let lr = find(&m, |m| match m {
        Message::BatteryLr(b) => Some(*b),
        _ => None,
    })
    .unwrap();
    let json = serde_json::to_value(lr).unwrap();
    assert_eq!(json["leftThreshold"], 20);
    assert_eq!(json["left"]["level"], 100);
    assert_eq!(json["left"]["charging"], "notCharging");
    let info = find(&m, |m| match m {
        Message::ProtocolInfo(i) => Some(i.clone()),
        _ => None,
    })
    .unwrap();
    let json = serde_json::to_value(info).unwrap();
    assert_eq!(json["table1Enabled"], true);
}

fn limits_from_fixtures() -> DeviceLimits {
    session_limits()
}

/// Run every host frame of every fixture through the policy with both gates open.
/// What it accepts must re-encode to the logged bytes, and what it refuses must be
/// exactly the probes this crate must never send again.
#[test]
fn logged_requests_are_accepted_byte_for_byte_or_denied() {
    let ft: FunctionTable = function_table();
    let limits = limits_from_fixtures();
    let policy = Policy::new()
        .with_setting_changes()
        .with_disconnecting_changes();
    let mut accepted = 0;
    let mut denied: Vec<Vec<u8>> = Vec::new();
    for text in ALL_FIXTURES {
        for (tx, wire) in load(text) {
            if !tx {
                continue;
            }
            let f = decode_frame(&wire).unwrap();
            assert!(is_data(f.data_type));
            let req = Request::new(f.data_type, f.payload.clone());
            match policy.check_e(&ft, &limits, &req) {
                Ok(c) => {
                    assert_eq!(c.payload(), f.payload);
                    assert_eq!(wire_of(&policy, &req, f.seq), wire);
                    accepted += 1;
                }
                Err(_) => {
                    if !denied.contains(&f.payload) {
                        denied.push(f.payload.clone());
                    }
                }
            }
        }
    }
    assert!(accepted > 50, "{accepted}");
    denied.sort();
    let expected: Vec<Vec<u8>> = vec![
        vec![0x26, 0x05], // POWER_GET_PARAM, not in the read tier
        vec![0x58, 0x00, 0x16, 0x06, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A], // bands to a non-CUSTOM preset
        vec![0x70, 0x00],                                                 // SENSE
        vec![0x76, 0x00],                                                 // SENSE
        vec![0x98, 0x00, 0x07, 0x01],                                     // raw alert answer
        vec![0xE0, 0x00],                                                 // AUDIO
        vec![0xE0, 0x01],                                                 // AUDIO
    ];
    assert_eq!(denied, expected);
}

#[test]
fn replaying_the_headset_side_through_the_link_acks_every_data_frame() {
    // Feed the whole handshake fixture, headset frames only, to a fresh link.
    let mut link = Link::new();
    let mut data_frames = 0;
    let mut acks = Vec::new();
    for (tx, wire) in load(HANDSHAKE) {
        if tx {
            continue;
        }
        let f = decode_frame(&wire).unwrap();
        let events = link.on_bytes(&wire, 0);
        for e in events {
            if let LinkEvent::Outgoing(a) = e {
                acks.push(a);
            }
        }
        if f.data_type != DataType::Ack {
            data_frames += 1;
            assert_eq!(acks.last().unwrap(), &ack((f.seq & 1) ^ 1));
        }
    }
    assert_eq!(acks.len(), data_frames);
}
