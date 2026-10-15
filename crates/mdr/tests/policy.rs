mod common;

use common::*;
use mdr::messages::{self, GeneralSettingKind as Gs};
use mdr::policy::{DenyReason, Tier};
use mdr::{
    CheckedRequest, DataType, DeviceLimits, FunctionTable, Message, Policy, PolicyError, Request,
    Table, parse,
};

fn limits() -> DeviceLimits {
    session_limits()
}

fn read() -> Policy {
    Policy::new()
}

fn setting() -> Policy {
    Policy::new().with_setting_changes()
}

fn disconnecting() -> Policy {
    Policy::new().with_disconnecting_changes()
}

fn both() -> Policy {
    setting().with_disconnecting_changes()
}

fn chk(p: Policy, req: &Request) -> Result<CheckedRequest, PolicyError> {
    p.check_e(&function_table(), &limits(), req)
}

fn t1(bytes: &[u8]) -> Request {
    Request::table1(bytes.to_vec())
}

fn t2(bytes: &[u8]) -> Request {
    Request::table2(bytes.to_vec())
}

fn assert_hard(r: Result<CheckedRequest, PolicyError>, reason: DenyReason) {
    match r {
        Err(PolicyError::HardDenied { reason: got, .. }) => assert_eq!(got, reason),
        other => panic!("expected hard deny {reason:?}, got {other:?}"),
    }
}

fn assert_denied(r: Result<CheckedRequest, PolicyError>) {
    assert!(r.is_err(), "expected a denial, got {r:?}");
}

// --- read tier ---------------------------------------------------------------

#[test]
fn read_tier_requests_pass_by_default() {
    let reads: Vec<Request> = vec![
        messages::protocol_info(Table::One),
        messages::protocol_info(Table::Two),
        messages::capability_info(Table::One),
        messages::capability_info(Table::Two),
        messages::device_info(Table::One, messages::DeviceInfoKind::ModelName),
        messages::device_info(Table::One, messages::DeviceInfoKind::FirmwareVersion),
        messages::device_info(Table::Two, messages::DeviceInfoKind::SeriesAndColor),
        messages::support_function(Table::One),
        messages::support_function(Table::Two),
        messages::battery_lr(),
        messages::battery_cradle(),
        messages::ncasm_capability(0x17),
        messages::ncasm_get_param(0x17),
        messages::eq_capability(),
        messages::eq_get_param(),
        messages::general_setting_capability(Gs::Multipoint),
        messages::general_setting_capability(Gs::Sidetone),
        messages::general_setting_get(Gs::Multipoint),
        messages::general_setting_get(Gs::Sidetone),
        messages::codec_status(),
    ];
    for r in reads {
        let c = chk(read(), &r).unwrap_or_else(|e| panic!("{:02x?}: {e}", r.payload));
        assert_eq!(c.payload(), r.payload);
        assert_eq!(c.data_type(), r.data_type);
    }
}

#[test]
fn read_requests_must_be_exactly_two_bytes() {
    assert_denied(chk(read(), &t1(&[0x22, 0x09, 0x00])));
    assert_denied(chk(read(), &t1(&[0x00, 0x00, 0x00])));
    assert_denied(chk(read(), &t1(&[0x04, 0x04])));
    assert_denied(chk(read(), &t1(&[0x04, 0x00])));
    assert_denied(chk(read(), &t1(&[0x22])));
    assert!(matches!(
        chk(read(), &t1(&[])),
        Err(PolicyError::EmptyPayload)
    ));
}

#[test]
fn reads_need_their_function_in_the_table() {
    let empty = FunctionTable::new();
    let pol = read();
    for r in [
        messages::battery_lr(),
        messages::battery_cradle(),
        messages::ncasm_capability(0x17),
        messages::ncasm_get_param(0x17),
        messages::eq_capability(),
        messages::eq_get_param(),
        messages::general_setting_get(Gs::Multipoint),
        messages::general_setting_capability(Gs::Sidetone),
        messages::codec_status(),
    ] {
        assert!(
            matches!(
                pol.check_e(&empty, &limits(), &r),
                Err(PolicyError::NotSupported { .. })
            ),
            "{:02x?}",
            r.payload
        );
    }
    // The connect reads need no function.
    assert!(
        pol.check_e(&empty, &limits(), &messages::protocol_info(Table::One))
            .is_ok()
    );
    assert!(
        pol.check_e(&empty, &limits(), &messages::support_function(Table::Two))
            .is_ok()
    );
}

#[test]
fn inquired_type_must_come_from_a_present_function() {
    // Function 0x6B gives 0x17, nothing else.
    assert!(chk(read(), &t1(&[0x60, 0x17])).is_ok());
    assert!(chk(read(), &t1(&[0x66, 0x17])).is_ok());
    for t in [0x13, 0x15, 0x30, 0x01, 0x00, 0xFF] {
        assert_denied(chk(read(), &t1(&[0x60, t])));
        assert_denied(chk(read(), &t1(&[0x66, t])));
    }
    // A table with function 0x64 serves 0x13, and no longer 0x17.
    let ft = ft_with(&[(0x64, 1)], &[]);
    let p = read();
    assert!(
        p.check_e(&ft, &DeviceLimits::new(), &t1(&[0x60, 0x13]))
            .is_ok()
    );
    assert_denied(p.check_e(&ft, &DeviceLimits::new(), &t1(&[0x60, 0x17])));
    // A function present only on table 2 does not count for table 1 reads.
    let ft2 = ft_with(&[], &[(0x6B, 4)]);
    assert_denied(p.check_e(&ft2, &DeviceLimits::new(), &t1(&[0x60, 0x17])));
}

#[test]
fn other_reads_are_denied() {
    for r in [
        &[0x26, 0x05][..], // POWER_GET_PARAM
        &[0x22, 0x01],     // other battery types
        &[0x22, 0x08],
        &[0x70, 0x00], // SENSE
        &[0xE0, 0x00], // AUDIO
        &[0xD6, 0xD3], // general setting we do not know
        &[0xD0, 0xD3],
        &[0x56, 0x01], // EBB
        &[0x50, 0x31], // CUSTOM_EQ
        &[0x12, 0x01], // connection status
        &[0xF0, 0x03], // SYSTEM capability
        &[0xA6, 0x20], // PLAY
    ] {
        assert_denied(chk(both(), &t1(r)));
    }
    // Table 2 knows only the four connect reads.
    for r in [
        &[0x46, 0x01][..],
        &[0x40, 0x01],
        &[0x22, 0x09],
        &[0x60, 0x17],
    ] {
        assert!(matches!(
            chk(both(), &t2(r)),
            Err(PolicyError::NotAllowlisted { .. })
        ));
    }
}

// --- hard denies --------------------------------------------------------------

#[test]
fn only_data_mdr_and_data_mdr_no2_pass() {
    for dt in [
        DataType::ShotMdr,
        DataType::ShotMdrNo2,
        DataType::ShotCommon,
        DataType::LargeDataMdr,
        DataType::LargeDataCommon,
        DataType::DataCommon,
        DataType::Ack,
        DataType::Unknown(0x4C),
    ] {
        // Even a payload that is fine on DATA_MDR.
        let r = Request::new(dt, vec![0x00, 0x00]);
        assert_hard(chk(both(), &r), DenyReason::DataType(dt.to_u8()));
    }
}

#[test]
fn firmware_update_family_is_hard_denied_in_every_gate() {
    let mut cmds: Vec<u8> = (0x30..=0x39).collect();
    cmds.extend([0x3E, 0x3F]);
    for c in cmds {
        for p in [read(), setting(), disconnecting(), both()] {
            assert_hard(chk(p, &t1(&[c, 0x00])), DenyReason::FirmwareUpdate);
            assert_hard(chk(p, &t1(&[c])), DenyReason::FirmwareUpdate);
        }
    }
}

#[test]
fn reset_settings_is_hard_denied() {
    for r in [
        &[0xF4, 0x09, 0x00][..], // SYSTEM_SET_STATUS reset settings
        &[0xF4, 0x09, 0x01],     // factory reset
        &[0xF8, 0x09, 0x00],     // SYSTEM_SET_PARAM
        &[0xFC, 0x09, 0x00],     // SYSTEM_SET_EXT_PARAM
        &[0xF4, 0x03, 0x00],     // any SYSTEM set
        &[0xF0, 0x09],           // GET_CAPABILITY with the reset type
        &[0xF6, 0x09],
        &[0xF2, 0x09],
    ] {
        assert_hard(chk(both(), &t1(r)), DenyReason::ResetSettings);
        assert_hard(chk(both(), &t2(r)), DenyReason::ResetSettings);
    }
    // EQEBB customizable sound effect reset (inquired type 0x34), table 1.
    for r in [&[0x58, 0x34, 0x00][..], &[0x56, 0x34], &[0x50, 0x34]] {
        assert_hard(chk(both(), &t1(r)), DenyReason::ResetSettings);
    }
}

#[test]
fn every_peripheral_command_on_table_2_is_hard_denied() {
    let mut cmds: Vec<u8> = (0x30..=0x39).collect();
    cmds.extend([0x3C, 0x3D]);
    for c in cmds {
        for p in [read(), both()] {
            assert_hard(chk(p, &t2(&[c, 0x02])), DenyReason::Peripheral);
            assert_hard(chk(p, &t2(&[c, 0x02, 0x00, 0x01])), DenyReason::Peripheral);
        }
    }
    // `36 02` (the paired list read) is denied as well, and so is `38 02`, a SET.
    assert_hard(chk(read(), &t2(&[0x36, 0x02])), DenyReason::Peripheral);
    assert_hard(
        chk(both(), &t2(&[0x38, 0x02, 0x01])),
        DenyReason::Peripheral,
    );
}

#[test]
fn raw_alert_answers_are_denied_even_with_every_gate() {
    for r in [
        &[0x98, 0x00, 0x07, 0x01][..],
        &[0x98, 0x00, 0x07, 0x00],
        &[0x98, 0x00, 0x01, 0x01],
    ] {
        assert_hard(chk(both(), &t1(r)), DenyReason::AlertNeedsToken);
    }
}

#[test]
fn unknown_commands_are_denied() {
    for c in [
        0x08, 0x10, 0x14, 0x40, 0x70, 0x72, 0x80, 0x94, 0xA0, 0xB0, 0xE8, 0xFE, 0xFF,
    ] {
        assert_denied(chk(both(), &t1(&[c, 0x00, 0x00])));
    }
}

// --- setting tier --------------------------------------------------------------

fn ncasm(status: u8, total: u8, mode: u8, amb: u8, level: u8) -> Request {
    t1(&[0x68, 0x17, status, total, mode, amb, level])
}

#[test]
fn setting_tier_requests_pass_with_the_gate_and_fail_without() {
    let cases: Vec<Request> = vec![
        ncasm(1, 1, 1, 0, 10),
        ncasm(1, 1, 1, 1, 20),
        ncasm(0, 1, 1, 0, 1),
        ncasm(1, 1, 0, 0, 0),
        ncasm(1, 1, 0, 0, 20),
        messages::eq_select_preset(0x16),
        messages::eq_select_preset(0xA0),
        messages::eq_select_preset(0x00),
        messages::eq_set_bands(&[0x10, 0x0E, 0x0B, 0x0A, 0x0C, 0x0F]),
        messages::eq_set_bands(&[0, 0, 0, 20, 20, 20]),
        messages::general_setting_set(Gs::Sidetone, true),
        messages::general_setting_set(Gs::Sidetone, false),
    ];
    for r in cases {
        let c = chk(setting(), &r).unwrap_or_else(|e| panic!("{:02x?}: {e}", r.payload));
        assert_eq!(c.payload(), r.payload);
        // The disconnecting gate alone does not unlock it, nor does the read tier.
        for closed in [read(), disconnecting()] {
            assert!(
                matches!(
                    chk(closed, &r),
                    Err(PolicyError::TierNotEnabled {
                        tier: Tier::Setting
                    })
                ),
                "{:02x?}",
                r.payload
            );
        }
        assert!(chk(both(), &r).is_ok());
    }
}

#[test]
fn ncasm_level_must_be_inside_the_capability_range() {
    assert_denied(chk(setting(), &ncasm(1, 1, 1, 0, 0))); // ambient level 0
    assert_denied(chk(setting(), &ncasm(1, 1, 1, 0, 21)));
    assert_denied(chk(setting(), &ncasm(1, 1, 1, 1, 0xFF)));
    assert_denied(chk(setting(), &ncasm(1, 1, 0, 0, 21))); // NC with a level outside
    assert_denied(chk(setting(), &ncasm(1, 1, 0, 1, 0xFF)));
    // NC mode accepts 0, the level the app sends.
    assert!(chk(setting(), &ncasm(1, 1, 0, 0, 0)).is_ok());
    assert!(chk(setting(), &ncasm(1, 1, 0, 1, 0)).is_ok());
}

#[test]
fn ncasm_fields_and_inquired_type_are_validated() {
    assert_denied(chk(setting(), &ncasm(2, 1, 1, 0, 10)));
    assert_denied(chk(setting(), &ncasm(1, 2, 1, 0, 10)));
    assert_denied(chk(setting(), &ncasm(1, 1, 2, 0, 10)));
    assert_denied(chk(setting(), &ncasm(1, 1, 1, 2, 10)));
    assert_denied(chk(setting(), &t1(&[0x68, 0x13, 1, 1, 1, 0, 10]))); // wrong type
    assert_denied(chk(setting(), &t1(&[0x68, 0x17, 1, 1, 1, 0]))); // short
    assert_denied(chk(setting(), &t1(&[0x68, 0x17, 1, 1, 1, 0, 10, 0]))); // long
}

#[test]
fn ncasm_set_is_denied_while_the_limits_are_unknown() {
    let r = ncasm(1, 1, 1, 0, 10);
    let p = setting();
    let e = p.check_e(&function_table(), &DeviceLimits::new(), &r);
    assert!(matches!(e, Err(PolicyError::ParamsNotAllowed { .. })));
    // Even NC level 0.
    let e = p.check_e(
        &function_table(),
        &DeviceLimits::new(),
        &ncasm(1, 1, 0, 0, 0),
    );
    assert!(matches!(e, Err(PolicyError::ParamsNotAllowed { .. })));
    // And without the function it is not supported.
    let e = p.check_e(&FunctionTable::new(), &limits(), &r);
    assert!(matches!(e, Err(PolicyError::NotSupported { .. })));
}

#[test]
fn eq_preset_must_be_in_the_capability() {
    assert_denied(chk(setting(), &t1(&[0x58, 0x00, 0x01, 0x00]))); // ROCK, not offered
    assert_denied(chk(setting(), &t1(&[0x58, 0x00, 0xA3, 0x00])));
    assert_denied(chk(setting(), &t1(&[0x58, 0x00, 0xFF, 0x00])));
    let e = setting().check_e(
        &function_table(),
        &DeviceLimits::new(),
        &messages::eq_select_preset(0x16),
    );
    assert!(matches!(e, Err(PolicyError::ParamsNotAllowed { .. })));
    // Another inquired type is not part of the allowlist.
    assert_denied(chk(setting(), &t1(&[0x58, 0x01, 0x16, 0x00])));
    assert_denied(chk(setting(), &t1(&[0x58, 0x31, 0x16, 0x00])));
}

#[test]
fn eq_bands_need_custom_the_right_count_and_steps_below_the_step_count() {
    // Wrong band count.
    assert_denied(chk(
        setting(),
        &t1(&[0x58, 0x00, 0xA0, 0x05, 1, 2, 3, 4, 5]),
    ));
    assert_denied(chk(
        setting(),
        &t1(&[0x58, 0x00, 0xA0, 0x07, 1, 2, 3, 4, 5, 6, 7]),
    ));
    // Count byte right, bytes missing or extra.
    assert_denied(chk(setting(), &t1(&[0x58, 0x00, 0xA0, 0x06, 1, 2, 3])));
    assert_denied(chk(
        setting(),
        &t1(&[0x58, 0x00, 0xA0, 0x06, 1, 2, 3, 4, 5, 6, 7]),
    ));
    // Step equal to the step count (21) and above.
    assert_denied(chk(
        setting(),
        &t1(&[0x58, 0x00, 0xA0, 0x06, 10, 10, 10, 10, 10, 21]),
    ));
    assert_denied(chk(
        setting(),
        &t1(&[0x58, 0x00, 0xA0, 0x06, 0xFF, 10, 10, 10, 10, 10]),
    ));
    // Bands for another preset would silently overwrite CUSTOM on the headset.
    assert_denied(chk(
        setting(),
        &t1(&[0x58, 0x00, 0x16, 0x06, 10, 10, 10, 10, 10, 10]),
    ));
    assert_denied(chk(
        setting(),
        &t1(&[0x58, 0x00, 0xA1, 0x06, 10, 10, 10, 10, 10, 10]),
    ));
    // Unknown limits.
    let e = setting().check_e(
        &function_table(),
        &DeviceLimits::new(),
        &messages::eq_set_bands(&[10; 6]),
    );
    assert!(matches!(e, Err(PolicyError::ParamsNotAllowed { .. })));
}

#[test]
fn sidetone_and_multipoint_values_are_bounded() {
    assert_denied(chk(both(), &t1(&[0xD8, 0xD2, 0x00, 0x02])));
    assert_denied(chk(both(), &t1(&[0xD8, 0xD2, 0x01, 0x00])));
    assert_denied(chk(both(), &t1(&[0xD8, 0xD1, 0x00, 0x02])));
    assert_denied(chk(both(), &t1(&[0xD8, 0xD3, 0x00, 0x00])));
    assert_denied(chk(both(), &t1(&[0xD8, 0xD2, 0x00])));
    // Without the function in the table.
    let e = both().check_e(
        &FunctionTable::new(),
        &limits(),
        &messages::general_setting_set(Gs::Sidetone, false),
    );
    assert!(matches!(e, Err(PolicyError::NotSupported { .. })));
}

// --- disconnecting tier ---------------------------------------------------------

#[test]
fn multipoint_needs_the_disconnecting_gate_and_the_setting_tier_is_not_enough() {
    let on = messages::general_setting_set(Gs::Multipoint, true);
    let off = messages::general_setting_set(Gs::Multipoint, false);
    assert_eq!(on.payload, [0xD8, 0xD1, 0x00, 0x00]);
    assert_eq!(off.payload, [0xD8, 0xD1, 0x00, 0x01]);
    for r in [&on, &off] {
        for closed in [read(), setting()] {
            assert!(matches!(
                chk(closed, r),
                Err(PolicyError::TierNotEnabled {
                    tier: Tier::Disconnecting
                })
            ));
        }
        assert!(chk(disconnecting(), r).is_ok());
        assert!(chk(both(), r).is_ok());
    }
    // The disconnecting gate does not unlock sidetone either.
    assert!(matches!(
        chk(
            disconnecting(),
            &messages::general_setting_set(Gs::Sidetone, true)
        ),
        Err(PolicyError::TierNotEnabled {
            tier: Tier::Setting
        })
    ));
}

// --- alert answers --------------------------------------------------------------

fn token(id: u8, action: u8) -> Option<mdr::AlertToken> {
    link_with_token(id, action).1
}

#[test]
fn alert_positive_for_multipoint_needs_the_disconnecting_gate() {
    let (mut link, tok) = link_with_token(0x07, 0x01);
    let c = disconnecting()
        .check_alert_answer(tok.unwrap(), true)
        .unwrap();
    assert_eq!(c.payload(), [0x98, 0x00, 0x07, 0x01]);
    assert_eq!(c.data_type(), DataType::DataMdr);
    // The exact frame the probe sent (seq 0).
    assert_eq!(
        link.send(c, 0).unwrap(),
        data1(0, &[0x98, 0x00, 0x07, 0x01])
    );
    for closed in [read(), setting()] {
        assert!(matches!(
            closed.check_alert_answer(token(0x07, 0x01).unwrap(), true),
            Err(PolicyError::TierNotEnabled {
                tier: Tier::Disconnecting
            })
        ));
    }
}

#[test]
fn alert_positive_for_any_other_message_id_is_denied() {
    for id in [0x00, 0x01, 0x02, 0x06, 0x08, 0x09, 0x0A, 0xFF] {
        let e = both().check_alert_answer(token(id, 0x01).unwrap(), true);
        assert!(
            matches!(e, Err(PolicyError::ParamsNotAllowed { .. })),
            "id {id:#x}"
        );
    }
}

#[test]
fn alert_negative_needs_the_setting_gate() {
    for id in [0x07, 0x01, 0x0A] {
        let c = setting()
            .check_alert_answer(token(id, 0x01).unwrap(), false)
            .unwrap();
        assert_eq!(c.payload(), [0x98, 0x00, id, 0x00]);
        for closed in [read(), disconnecting()] {
            assert!(matches!(
                closed.check_alert_answer(token(id, 0x01).unwrap(), false),
                Err(PolicyError::TierNotEnabled {
                    tier: Tier::Setting
                })
            ));
        }
    }
}

#[test]
fn tokens_exist_only_for_positive_negative_alerts() {
    assert!(token(0x07, 0x01).is_some());
    assert!(token(0x07, 0x00).is_none());
    assert!(token(0x07, 0x02).is_none());
    // A notification that is not a fixed message alert gives nothing.
    let mut inc = parse(Table::One, &[0x99, 0x01, 0x07, 0x01]);
    assert!(matches!(inc.message, Message::Unknown(_)));
    if let Message::Alert(a) = &mut inc.message {
        assert!(a.take_token().is_none());
    }
}

#[test]
fn raw_parse_never_attaches_a_token() {
    let mut inc = parse(Table::One, &[0x99, 0x00, 0x07, 0x01]);
    let Message::Alert(a) = &mut inc.message else {
        panic!("alert")
    };
    assert_eq!((a.message_id, a.action_type), (0x07, 0x01));
    assert!(!a.has_token());
    assert!(a.take_token().is_none());
}

// --- encoding ---------------------------------------------------------------------

#[test]
fn checked_request_bytes_equal_the_checked_request() {
    let r = messages::battery_lr();
    assert_eq!(wire_of(&read(), &r, 0), data1(0, &[0x22, 0x09]));
    assert_eq!(wire_of(&read(), &r, 1), data1(1, &[0x22, 0x09]));
    let r2 = messages::support_function(Table::Two);
    assert_eq!(wire_of(&read(), &r2, 0), data2(0, &[0x06, 0x00]));
    let f = mdr::decode_frame(&wire_of(&read(), &r, 1)).unwrap();
    assert_eq!(f.payload, r.payload);
    assert_eq!(f.data_type, DataType::DataMdr);
}

// --- factory power off, NC/ASM step ------------------------------------------------

#[test]
fn factory_power_off_is_hard_denied() {
    for r in [
        &[0x24, 0x03, 0xFF][..], // POWER_SET_STATUS, POWER_OFF, FACTORY_POWER_OFF
        &[0x24, 0x03, 0x00],
        &[0x28, 0x03, 0xFF], // POWER_SET_PARAM
        &[0x28, 0x03, 0x01],
        &[0x24, 0x03],
    ] {
        for p in [read(), setting(), disconnecting(), both()] {
            assert_hard(chk(p, &t1(r)), DenyReason::PowerOff);
        }
    }
    // Other power set commands are simply not allowlisted.
    assert!(matches!(
        chk(both(), &t1(&[0x28, 0x05, 0x00])),
        Err(PolicyError::NotAllowlisted { .. })
    ));
}

#[test]
fn ncasm_level_must_sit_on_the_capability_step() {
    // Capability with step 3 for both ambient modes: levels 1, 4, 7, ... 19.
    let (ft, limits) = session_with(
        &[(0x6B, 4)],
        &[&[
            0x61, 0x17, 0x02, 0x00, 0x01, 0x14, 0x03, 0x01, 0x01, 0x14, 0x03,
        ]],
    );
    let p = setting();
    for (level, ok) in [
        (1, true),
        (4, true),
        (19, true),
        (2, false),
        (5, false),
        (20, false),
    ] {
        let r = t1(&[0x68, 0x17, 1, 1, 1, 0, level]);
        assert_eq!(p.check_e(&ft, &limits, &r).is_ok(), ok, "level {level}");
    }
    // NC mode level 0 stays allowed, other NC levels follow the step.
    assert!(
        p.check_e(&ft, &limits, &t1(&[0x68, 0x17, 1, 1, 0, 0, 0]))
            .is_ok()
    );
    assert!(
        p.check_e(&ft, &limits, &t1(&[0x68, 0x17, 1, 1, 0, 0, 4]))
            .is_ok()
    );
    assert!(
        p.check_e(&ft, &limits, &t1(&[0x68, 0x17, 1, 1, 0, 0, 5]))
            .is_err()
    );
}

#[test]
fn limits_only_come_from_received_capability_frames() {
    // Non-capability frames are not absorbed, and empty limits deny the setters.
    let mut l = DeviceLimits::new();
    let mut link = mdr::Link::new();
    for e in link.on_bytes(&data1(0, &[0x67, 0x17, 1, 1, 0, 0, 20]), 0) {
        if let mdr::LinkEvent::Delivered(f) = e {
            assert!(!l.absorb(&f));
        }
    }
    assert!(chk(setting(), &ncasm(1, 1, 1, 0, 10)).is_ok());
    let e = setting().check_e(&function_table(), &l, &ncasm(1, 1, 1, 0, 10));
    assert!(matches!(e, Err(PolicyError::ParamsNotAllowed { .. })));
    // A capability on a table 2 frame is ignored.
    let mut link = mdr::Link::new();
    for e in link.on_bytes(&data2(0, &[0x61, 0x17, 1, 0, 1, 20, 1]), 0) {
        if let mdr::LinkEvent::Delivered(f) = e {
            assert!(!l.absorb(&f));
        }
    }
}

// --- epochs ---------------------------------------------------------------------

/// Deliver crafted frames on a link and absorb them.
fn absorb_on(link: &mut mdr::Link, seq: u8, payload: &[u8]) -> (FunctionTable, DeviceLimits) {
    let mut ft = FunctionTable::new();
    let mut lim = DeviceLimits::new();
    for e in link.on_bytes(&data1(seq, payload), 0) {
        if let mdr::LinkEvent::Delivered(f) = e {
            ft.absorb(&f);
            lim.absorb(&f);
        }
    }
    (ft, lim)
}

#[test]
fn old_table_and_limits_are_denied_after_a_reset_until_reabsorbed() {
    let (ft, limits) = session_with(
        &[(0x6B, 4), (0x50, 10), (0xD2, 1)],
        &[&[
            0x61, 0x17, 0x02, 0x00, 0x01, 0x14, 0x01, 0x01, 0x01, 0x14, 0x01,
        ]],
    );
    let old = ft.epoch().unwrap();
    assert_eq!(limits.epoch(), Some(old));
    let p = setting();
    let set = ncasm(1, 1, 1, 0, 10);
    assert!(p.check(old, &ft, &limits, &set).is_ok());
    // The link was reset: the epoch moved on.
    let mut link = mdr::Link::new();
    let new = link.epoch();
    assert_ne!(new, old);
    // A request that reads the table is denied.
    assert!(matches!(
        p.check(new, &ft, &limits, &messages::battery_lr()),
        Err(PolicyError::HardDenied {
            reason: DenyReason::StaleFunctionTable,
            ..
        })
    ));
    // The connect reads consult nothing, so the handshake can run again.
    assert!(
        p.check(new, &ft, &limits, &messages::support_function(Table::One))
            .is_ok()
    );
    // A fresh table with the old limits: the limits are stale.
    let (ft2, _) = {
        let mut sup = vec![0x07, 0x00, 0x01, 0x6B, 0x04];
        let r = absorb_on(&mut link, 0, &sup);
        sup.clear();
        r
    };
    assert_eq!(ft2.epoch(), Some(new));
    assert!(matches!(
        p.check(new, &ft2, &limits, &set),
        Err(PolicyError::HardDenied {
            reason: DenyReason::StaleLimits,
            ..
        })
    ));
    // Re-absorbing the capability in the new epoch makes the request legal again.
    let (_, fresh) = absorb_on(
        &mut link,
        1,
        &[
            0x61, 0x17, 0x02, 0x00, 0x01, 0x14, 0x01, 0x01, 0x01, 0x14, 0x01,
        ],
    );
    assert_eq!(fresh.epoch(), Some(new));
    assert!(p.check(new, &ft2, &fresh, &set).is_ok());
}

#[test]
fn absorbing_a_frame_from_a_newer_epoch_clears_the_old_content() {
    let mut a = mdr::Link::new();
    let mut ft = FunctionTable::new();
    let mut lim = DeviceLimits::new();
    for e in a.on_bytes(&data1(0, &[0x07, 0x00, 0x02, 0x6B, 0x04, 0x50, 0x0A]), 0) {
        if let mdr::LinkEvent::Delivered(f) = e {
            ft.absorb(&f);
        }
    }
    for e in a.on_bytes(&data1(1, &[0x61, 0x17, 0x01, 0x00, 0x01, 0x14, 0x01]), 0) {
        if let mdr::LinkEvent::Delivered(f) = e {
            assert!(lim.absorb(&f));
        }
    }
    assert_eq!(ft.count(Table::One), 2);
    a.reset();
    for e in a.on_bytes(&data1(0, &[0x07, 0x00, 0x01, 0x12, 0xFF]), 0) {
        if let mdr::LinkEvent::Delivered(f) = e {
            ft.absorb(&f);
        }
    }
    assert_eq!(ft.count(Table::One), 1);
    assert!(ft.contains(Table::One, 0x12));
    assert!(!ft.contains(Table::One, 0x6B));
    assert_eq!(ft.epoch(), Some(a.epoch()));
    // Limits clear the same way when an EQ capability arrives in the new epoch.
    for e in a.on_bytes(&data1(1, &[0x51, 0x00, 0x06, 0x15, 0x00]), 0) {
        if let mdr::LinkEvent::Delivered(f) = e {
            assert!(lim.absorb(&f));
        }
    }
    assert_eq!(lim.epoch(), Some(a.epoch()));
    let e = setting().check(a.epoch(), &ft, &lim, &ncasm(1, 1, 0, 0, 0));
    assert!(e.is_err(), "NC/ASM limits were cleared");
}

#[test]
fn one_delivered_alert_gives_exactly_one_token() {
    let mut link = mdr::Link::new();
    let mut frames = Vec::new();
    for e in link.on_bytes(&data1(0, &[0x99, 0x00, 0x07, 0x01]), 0) {
        if let mdr::LinkEvent::Delivered(f) = e {
            frames.push(f);
        }
    }
    assert_eq!(frames.len(), 1);
    // Absorbing borrows the frame and never makes a token.
    FunctionTable::new().absorb(&frames[0]);
    DeviceLimits::new().absorb(&frames[0]);
    let frame = frames.pop().unwrap();
    let mut inc = mdr::parse_received(frame); // consumes the frame
    let Message::Alert(a) = &mut inc.message else {
        panic!("alert")
    };
    assert!(a.take_token().is_some());
    assert!(a.take_token().is_none());
}

#[test]
fn eq_band_info_read_needs_the_eq_function_and_type() {
    let r = messages::eq_extended_info();
    assert_eq!(r.payload, [0x5A, 0x00]);
    assert_eq!(chk(read(), &r).unwrap().payload(), [0x5A, 0x00]);
    assert_denied(chk(read(), &t1(&[0x5A, 0x01])));
    assert_denied(chk(read(), &t1(&[0x5A, 0x00, 0x00])));
    assert_denied(chk(read(), &t1(&[0x5A])));
    // Inquired type 0x34 (customizable sound effect reset) is hard denied on every EQEBB command.
    assert_denied(chk(read(), &t1(&[0x5A, 0x34])));
    assert_denied(read().check_e(&FunctionTable::new(), &limits(), &r));
    let other = ft_with(&[(0x51, 0x00)], &[]);
    assert_denied(read().check_e(&other, &limits(), &r));
}
