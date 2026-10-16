//! Mapping from the G502 X snapshot to the generic `myhub-core` device model.
//!
//! Pure functions over already-read data: nothing here talks to the device.

use hidpp::battery::{self, BatteryStatus};
use hidpp::dpi::DpiEntry;
use hidpp::onboard::{Binding, ProfileSector, SpecialFunction, layout_a};
use myhub_core::{
    BatteryInfo, BatteryLevel, BindingInfo, BindingKind, ChargingState, Connection, DeviceDetail,
    DeviceKind, DeviceSummary, DpiInfo, OnboardMode, OnboardView, ProfileInfo, ReportRateInfo,
    SpecialAction,
};

use crate::DEVICE_ID;
use crate::snapshot::{ParsedProfile, Snapshot};

/// First ROM (factory) profile sector.
const ROM_SECTOR_BASE: u16 = 0x0100;

/// Maps a snapshot to the UI model. `link_up` says whether the mouse is
/// reachable now (`false` keeps the last known values and marks it
/// disconnected); `updated_at_ms` is the Unix time of the read.
pub fn to_detail(snapshot: &Snapshot, link_up: bool, updated_at_ms: u64) -> DeviceDetail {
    let percent = snapshot.battery.percent;
    DeviceDetail {
        summary: DeviceSummary {
            id: DEVICE_ID.to_owned(),
            name: "G502 X LIGHTSPEED".to_owned(),
            kind: DeviceKind::Mouse,
            connection: if link_up {
                Connection::Connected
            } else {
                Connection::Disconnected
            },
            battery_percent: Some(percent),
        },
        vendor: "Logitech".to_owned(),
        transport: "LIGHTSPEED".to_owned(),
        protocol: "HID++ 2.0".to_owned(),
        read_only: true,
        updated_at_ms,
        illustration: Some("g502x".to_owned()),
        battery: Some(battery_info(&snapshot.battery.status)),
        dpi: dpi_info(snapshot),
        report_rate: Some(ReportRateInfo {
            options_hz: snapshot.report_rate.rates_hz.clone(),
            current_hz: snapshot.report_rate.current_hz,
        }),
        onboard: Some(onboard_view(snapshot)),
        headset: None,
        keyboard: None,
    }
}

/// Percent from a 0x1004 (unified battery) broadcast payload: byte 0 \[OBS\]
/// (`37 04 00 00` = 55 %). `None` when the value is above 100.
pub fn parse_battery_notification(payload: &[u8; 16]) -> Option<u8> {
    let percent = payload[0];
    (percent <= 100).then_some(percent)
}

/// A 0x1004 GetStatus payload (reply or event) as a status; `None` when the
/// percent is above 100.
pub fn parse_battery_status(payload: &[u8; 16]) -> Option<BatteryStatus> {
    battery::parse_status(payload)
        .ok()
        .filter(|s| s.percent <= 100)
}

/// Maps the 0x1004 bytes to the UI model: level flags, Solaar's charging state
/// names and external power (byte 3 != 0) [INF].
pub fn battery_info(status: &BatteryStatus) -> BatteryInfo {
    let level = status.level_name().and_then(|name| match name {
        "full" => Some(BatteryLevel::Full),
        "good" => Some(BatteryLevel::Good),
        "low" => Some(BatteryLevel::Low),
        "critical" => Some(BatteryLevel::Critical),
        _ => None,
    });
    let charging = match status.charging_state_name().as_str() {
        "discharging" => ChargingState::Discharging,
        "recharging" => ChargingState::Charging,
        "almost_full" => ChargingState::AlmostFull,
        "full" => ChargingState::Full,
        "slow_recharge" => ChargingState::SlowCharging,
        "invalid_battery" | "thermal_error" => ChargingState::Error,
        _ => ChargingState::Unknown,
    };
    BatteryInfo {
        percent: status.percent,
        level,
        charging,
        external_power: status.external_power != 0,
    }
}

fn dpi_info(snapshot: &Snapshot) -> Option<DpiInfo> {
    snapshot
        .dpi
        .list
        .entries
        .iter()
        .find_map(|entry| match *entry {
            DpiEntry::Range { min, max, step } => Some(DpiInfo {
                min,
                max,
                step,
                current: snapshot.dpi.current,
            }),
            DpiEntry::Discrete(_) => None,
        })
}

/// Byte `index` of a lowercase hex string.
fn hex_byte(hex: &str, index: usize) -> Option<u8> {
    u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()
}

fn onboard_view(snapshot: &Snapshot) -> OnboardView {
    let ob = &snapshot.onboard;
    let info = &ob.info;

    // 0x8100 fn2 wire values: 1 onboard, 2 host (libratbag, Solaar); anything
    // else unknown.
    let mode = ob
        .mode_raw_hex
        .as_deref()
        .and_then(|hex| hex_byte(hex, 0))
        .and_then(|byte| match byte {
            1 => Some(OnboardMode::Onboard),
            2 => Some(OnboardMode::Host),
            _ => None,
        });

    // 0x8100 fn4: BE16 sector number; 1..=profile_count are user profiles.
    let active_profile = ob.active_profile_raw_hex.as_deref().and_then(|hex| {
        let value = u16::from_be_bytes([hex_byte(hex, 0)?, hex_byte(hex, 1)?]);
        (1..=u16::from(info.profile_count))
            .contains(&value)
            .then_some(value as u8)
    });

    let profiles = ob
        .profiles
        .iter()
        .filter_map(|parsed| {
            let sector = sector_number(parsed)?;
            let profile = parsed.profile.as_ref()?;
            let enabled = parsed.rom
                || ob
                    .directory
                    .entries
                    .iter()
                    .any(|e| e.sector == sector && e.enabled);
            let number = if parsed.rom {
                sector.checked_sub(ROM_SECTOR_BASE)?
            } else {
                sector
            };
            let number = u8::try_from(number).ok().filter(|n| *n >= 1)?;
            Some(profile_info(
                profile,
                parsed.rom,
                number,
                enabled,
                info.button_count,
            ))
        })
        .collect();

    // Only the active user profile 3..=5 that is enabled, in onboard mode.
    let writable_profile = active_profile
        .filter(|_| mode == Some(OnboardMode::Onboard))
        .filter(|n| {
            (3..=5).contains(n)
                && ob
                    .directory
                    .entries
                    .iter()
                    .any(|e| e.sector == u16::from(*n) && e.enabled)
        });

    OnboardView {
        mode,
        active_profile,
        button_count: info.button_count,
        profiles,
        writable_profile,
    }
}

/// Sector number from the `0x0001` style key.
fn sector_number(parsed: &ParsedProfile) -> Option<u16> {
    u16::from_str_radix(parsed.sector.strip_prefix("0x")?, 16).ok()
}

fn profile_info(
    profile: &ProfileSector,
    rom: bool,
    number: u8,
    enabled: bool,
    button_count: u8,
) -> ProfileInfo {
    let slots = usize::from(button_count).min(layout_a::BUTTON_SLOTS);
    let bindings = |base: usize| -> Vec<BindingInfo> {
        (0..slots)
            .map(|slot| binding_info(slot, &profile.raw, base))
            .collect()
    };
    ProfileInfo {
        id: format!("{}-{number}", if rom { "rom" } else { "user" }),
        number,
        rom,
        enabled,
        name: profile.name.clone(),
        report_rate_hz: match profile.report_rate {
            ms @ (1 | 2 | 4 | 8) => Some(1000 / u16::from(ms)),
            _ => None,
        },
        dpi_levels: profile.dpi_levels.to_vec(),
        default_dpi_index: profile.default_dpi_index,
        shift_dpi_index: profile.shift_dpi_index,
        buttons: bindings(layout_a::BUTTONS),
        gshift: bindings(layout_a::GSHIFT_BUTTONS),
        // ROM sectors store CRC 0, which never matches.
        crc_ok: profile.crc_ok && !(rom && profile.stored_crc == 0),
    }
}

fn binding_info(slot: usize, raw: &[u8], base: usize) -> BindingInfo {
    let offset = base + slot * 4;
    let bytes: [u8; 4] = [
        raw[offset],
        raw[offset + 1],
        raw[offset + 2],
        raw[offset + 3],
    ];
    let mut out = BindingInfo {
        slot: slot as u8,
        kind: BindingKind::Unknown,
        mouse_buttons: None,
        modifiers: None,
        usage: None,
        special: None,
        special_code: None,
        raw_hex: bytes.iter().map(|b| format!("{b:02x}")).collect(),
    };
    match Binding::decode(bytes) {
        Binding::MouseButtons(mask) => {
            out.kind = BindingKind::Mouse;
            out.mouse_buttons = Some(mask);
        }
        Binding::Keyboard { modifiers, usage } => {
            out.kind = BindingKind::Keyboard;
            out.modifiers = Some(modifiers);
            out.usage = Some(u16::from(usage));
        }
        Binding::Consumer(usage) => {
            out.kind = BindingKind::Consumer;
            out.usage = Some(usage);
        }
        Binding::Special(function) => {
            out.kind = BindingKind::Special;
            out.special_code = Some(bytes[1]);
            out.special = match function {
                SpecialFunction::WheelLeft => Some(SpecialAction::WheelLeft),
                SpecialFunction::WheelRight => Some(SpecialAction::WheelRight),
                SpecialFunction::DpiUp => Some(SpecialAction::DpiUp),
                SpecialFunction::DpiDown => Some(SpecialAction::DpiDown),
                SpecialFunction::DpiShift => Some(SpecialAction::DpiShift),
                SpecialFunction::ProfileCycle => Some(SpecialAction::ProfileCycle),
                SpecialFunction::GShift => Some(SpecialAction::GShift),
                // 0x10 and 0x11 are [UNK]: the raw code is kept, nothing is guessed.
                SpecialFunction::Other(_) => None,
            };
        }
        Binding::Unassigned => out.kind = BindingKind::Unassigned,
        Binding::Unknown(_) => {}
    }
    out
}
