//! `to_detail` over the fixture snapshot (real device frames, no hardware).

mod common;

use common::*;
use logitech_g502x::{
    Snapshot, battery_info, parse_battery_notification, parse_battery_status, read_snapshot,
    to_detail,
};
use myhub_core::{
    BindingInfo, BindingKind, Connection, DeviceDetail, DeviceKind, OnboardMode, ProfileInfo,
    SpecialAction,
};

/// Snapshot with the replies the real device gave to the two calls that are
/// not in the frame log: mode `01` (onboard) and active profile `0002`.
fn snapshot_with(mode: &str, active: &str) -> Snapshot {
    let (mut session, handle) = fixture_session();
    {
        let mut state = handle.borrow_mut();
        let mut rx = hex(SYNTH_FN2_RX);
        rx[4..4 + mode.len() / 2].copy_from_slice(&hex(mode));
        state.set_reply(hex(SYNTH_FN2_TX), vec![rx]);
        let mut rx = hex(SYNTH_FN4_RX);
        rx[4..4 + active.len() / 2].copy_from_slice(&hex(active));
        state.set_reply(hex(SYNTH_FN4_TX), vec![rx]);
    }
    read_snapshot(&mut session).expect("snapshot")
}

fn detail() -> DeviceDetail {
    to_detail(&snapshot_with("01", "0002"), true, 1_700_000_000_000)
}

fn profile(detail: &DeviceDetail, id: &str) -> ProfileInfo {
    detail
        .onboard
        .as_ref()
        .unwrap()
        .profiles
        .iter()
        .find(|p| p.id == id)
        .unwrap_or_else(|| panic!("profile {id}"))
        .clone()
}

fn slot(bindings: &[BindingInfo], slot: u8) -> &BindingInfo {
    bindings.iter().find(|b| b.slot == slot).expect("slot")
}

#[test]
fn identity_battery_dpi_and_rates() {
    let d = detail();
    assert_eq!(d.summary.id, "logitech-g502x");
    assert_eq!(d.summary.name, "G502 X LIGHTSPEED");
    assert_eq!(d.summary.kind, DeviceKind::Mouse);
    assert_eq!(d.summary.connection, Connection::Connected);
    assert_eq!(d.summary.battery_percent, Some(55));
    assert_eq!(d.vendor, "Logitech");
    assert_eq!(d.transport, "LIGHTSPEED");
    assert_eq!(d.protocol, "HID++ 2.0");
    assert!(d.read_only);
    assert_eq!(d.updated_at_ms, 1_700_000_000_000);
    assert_eq!(d.illustration.as_deref(), Some("g502x"));
    assert_eq!(d.battery.unwrap().percent, 55);

    let dpi = d.dpi.unwrap();
    assert_eq!(
        (dpi.min, dpi.max, dpi.step, dpi.current),
        (100, 25600, 50, 1600)
    );
    let rate = d.report_rate.unwrap();
    assert_eq!(rate.options_hz, vec![1000, 500, 250, 125]);
    assert_eq!(rate.current_hz, 1000);
}

#[test]
fn link_down_marks_the_device_disconnected_and_keeps_the_values() {
    let snapshot = snapshot_with("01", "0002");
    let up = to_detail(&snapshot, true, 1);
    let down = to_detail(&snapshot, false, 1);
    assert_eq!(down.summary.connection, Connection::Disconnected);
    assert_eq!(down.summary.battery_percent, Some(55));
    let mut expected = up;
    expected.summary.connection = Connection::Disconnected;
    assert_eq!(down, expected);
}

#[test]
fn onboard_mode_active_profile_and_layout() {
    let d = detail();
    let ob = d.onboard.as_ref().unwrap();
    assert_eq!(ob.mode, Some(OnboardMode::Onboard));
    assert_eq!(ob.active_profile, Some(2));
    assert_eq!(ob.button_count, 11);

    let ids: Vec<&str> = ob.profiles.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "user-1", "user-2", "user-3", "user-4", "user-5", "rom-1", "rom-2"
        ]
    );
    let enabled: Vec<bool> = ob.profiles.iter().map(|p| p.enabled).collect();
    assert_eq!(enabled, [true, true, false, false, false, true, true]);
    for p in &ob.profiles {
        assert_eq!(p.buttons.len(), 11, "{}", p.id);
        assert_eq!(p.gshift.len(), 11, "{}", p.id);
        assert_eq!(p.dpi_levels.len(), 5, "{}", p.id);
        assert_eq!(p.rom, p.id.starts_with("rom"));
        assert_eq!(p.crc_ok, !p.rom, "crc of {}", p.id);
    }
    assert_eq!(profile(&d, "rom-2").number, 2);
}

#[test]
fn onboard_mode_variants() {
    let d = to_detail(&snapshot_with("02", "0001"), true, 0);
    let ob = d.onboard.unwrap();
    assert_eq!(ob.mode, Some(OnboardMode::Host));
    assert_eq!(ob.active_profile, Some(1));

    // Unknown mode byte; active sector outside 1..=profile_count.
    for active in ["0000", "0006", "0101"] {
        let d = to_detail(&snapshot_with("07", active), true, 0);
        let ob = d.onboard.unwrap();
        assert_eq!(ob.mode, None);
        assert_eq!(ob.active_profile, None, "{active}");
    }
}

#[test]
fn profile_one_matches_the_fixture() {
    let p = profile(&detail(), "user-1");
    assert_eq!(p.number, 1);
    assert_eq!(p.dpi_levels, vec![600, 1200, 1600, 2400, 3200]);
    assert_eq!(p.default_dpi_index, 2);
    assert_eq!(p.shift_dpi_index, 0);
    assert_eq!(p.name.as_deref(), Some("PROFILE_NAME_DEFAULT"));
    assert_eq!(p.report_rate_hz, Some(1000));
}

#[test]
fn profile_two_buttons_and_gshift() {
    let p = profile(&detail(), "user-2");
    assert_eq!(p.report_rate_hz, Some(500));

    // G5 is the G-shift key.
    let g5 = slot(&p.buttons, 4);
    assert_eq!(g5.kind, BindingKind::Special);
    assert_eq!(g5.special, Some(SpecialAction::GShift));
    assert_eq!(g5.special_code, Some(0x0b));
    assert_eq!(g5.raw_hex, "900b0000");

    // G11 (slot 10) lowers the DPI.
    let g11 = slot(&p.buttons, 10);
    assert_eq!(g11.kind, BindingKind::Special);
    assert_eq!(g11.special, Some(SpecialAction::DpiDown));
    assert_eq!(g11.special_code, Some(0x04));

    // G-shift layer: G3 is keyboard modifier 0x01 usage 0x17.
    let g3 = slot(&p.gshift, 2);
    assert_eq!(g3.kind, BindingKind::Keyboard);
    assert_eq!(g3.modifiers, Some(0x01));
    assert_eq!(g3.usage, Some(0x17));
    assert_eq!(g3.raw_hex, "80020117");
    assert_eq!(g3.mouse_buttons, None);
    assert_eq!(g3.special, None);

    // Slot numbers run 0..button_count in order.
    let slots: Vec<u8> = p.buttons.iter().map(|b| b.slot).collect();
    assert_eq!(slots, (0..11).collect::<Vec<u8>>());
}

#[test]
fn disabled_profiles_are_listed_but_not_enabled() {
    let d = detail();
    for n in 3..=5 {
        let p = profile(&d, &format!("user-{n}"));
        assert!(!p.enabled, "user-{n}");
        assert!(p.crc_ok, "user-{n}");
    }
}

#[test]
fn rom_profiles_are_enabled_without_a_valid_crc() {
    let d = detail();
    for id in ["rom-1", "rom-2"] {
        let p = profile(&d, id);
        assert!(p.rom && p.enabled && !p.crc_ok, "{id}");
    }
}

#[test]
fn detail_serializes_to_the_contract_shape() {
    let value = serde_json::to_value(detail()).unwrap();
    assert_eq!(value["id"], "logitech-g502x");
    assert_eq!(value["connection"], "connected");
    assert_eq!(value["readOnly"], true);
    assert_eq!(value["onboard"]["mode"], "onboard");
    assert_eq!(value["onboard"]["activeProfile"], 2);
    let profiles = value["onboard"]["profiles"].as_array().unwrap();
    assert_eq!(profiles.len(), 7);
    assert_eq!(profiles[1]["buttons"][4]["special"], "gShift");
}

#[test]
fn battery_broadcast_payload() {
    // [OBS] 0x1004 broadcast `37 04 00 00`.
    let mut payload = [0u8; 16];
    payload[..4].copy_from_slice(&[0x37, 0x04, 0x00, 0x00]);
    assert_eq!(parse_battery_notification(&payload), Some(55));
    payload[0] = 100;
    assert_eq!(parse_battery_notification(&payload), Some(100));
    payload[0] = 101;
    assert_eq!(parse_battery_notification(&payload), None);
}

fn status(bytes: [u8; 4]) -> myhub_core::BatteryInfo {
    let mut payload = [0u8; 16];
    payload[..4].copy_from_slice(&bytes);
    battery_info(&parse_battery_status(&payload).expect("status"))
}

#[test]
fn battery_status_maps_to_the_contract_enums() {
    use myhub_core::{BatteryLevel, ChargingState};
    // Fixture frames: 37 04 00 00 and the observed charging pair 35 04 01 01.
    let b = status([0x37, 0x04, 0x00, 0x00]);
    assert_eq!(
        (b.percent, b.level, b.charging, b.external_power),
        (
            55,
            Some(BatteryLevel::Good),
            ChargingState::Discharging,
            false
        )
    );
    let b = status([0x35, 0x04, 0x01, 0x01]);
    assert_eq!(
        (b.percent, b.charging, b.external_power),
        (53, ChargingState::Charging, true)
    );
    for (state, expected) in [
        (0u8, ChargingState::Discharging),
        (1, ChargingState::Charging),
        (2, ChargingState::AlmostFull),
        (3, ChargingState::Full),
        (4, ChargingState::SlowCharging),
        (5, ChargingState::Error),
        (6, ChargingState::Error),
        (7, ChargingState::Unknown),
        (0xff, ChargingState::Unknown),
    ] {
        assert_eq!(
            status([50, 4, state, 0]).charging,
            expected,
            "state {state}"
        );
    }
    for (flags, expected) in [
        (8u8, Some(BatteryLevel::Full)),
        (4, Some(BatteryLevel::Good)),
        (2, Some(BatteryLevel::Low)),
        (1, Some(BatteryLevel::Critical)),
        (0, None),
        (6, None),
    ] {
        assert_eq!(status([50, flags, 0, 0]).level, expected, "flags {flags}");
    }
    // Byte 3 alone decides external power.
    assert!(status([50, 4, 0, 0x80]).external_power);
    assert!(!status([50, 4, 3, 0]).external_power);
    // A percent above 100 is not a battery status.
    let mut payload = [0u8; 16];
    payload[0] = 101;
    assert_eq!(parse_battery_status(&payload), None);
}

#[test]
fn writable_profile_needs_onboard_an_enabled_user_sector_3_to_5() {
    let with = |mode: &str, active: &str, enabled: &[u16]| {
        let mut snap = snapshot_with(mode, active);
        for entry in &mut snap.onboard.directory.entries {
            entry.enabled = enabled.contains(&entry.sector);
        }
        to_detail(&snap, true, 0).onboard.unwrap().writable_profile
    };
    assert_eq!(with("01", "0003", &[1, 2, 3]), Some(3));
    assert_eq!(with("01", "0005", &[1, 2, 5]), Some(5));
    // Not enabled in the directory.
    assert_eq!(with("01", "0003", &[1, 2]), None);
    // Sectors 1 and 2 are never writable.
    assert_eq!(with("01", "0001", &[1, 2, 3]), None);
    assert_eq!(with("01", "0002", &[1, 2, 3]), None);
    // Host mode, unknown mode.
    assert_eq!(with("02", "0003", &[1, 2, 3]), None);
    assert_eq!(with("07", "0003", &[1, 2, 3]), None);
    // The default fixture: profile 2 is active.
    assert_eq!(detail().onboard.unwrap().writable_profile, None);
}
