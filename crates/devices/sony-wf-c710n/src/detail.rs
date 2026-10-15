//! Maps a [`Snapshot`] to the generic UI model of `myhub-core`.

use mdr::messages::eq_preset_name;
use mdr::messages::{AmbientMode, AudioCodec, BatteryLr, Cell, ChargingStatus, NcAsmMode};
use myhub_core::{
    AmbientModeView, BatteryInfo, CellCharging, ChargingState, Connection, DeviceDetail,
    DeviceKind, DeviceSummary, EqPresetView, EqView, HeadsetBattery, HeadsetCell, HeadsetView,
    NcAsmModeView, NcAsmView,
};

use crate::snapshot::Snapshot;
use crate::{DEVICE_ID, MODEL_NAME};

fn cell_charging(c: ChargingStatus) -> CellCharging {
    match c {
        ChargingStatus::NotCharging => CellCharging::NotCharging,
        ChargingStatus::Charging => CellCharging::Charging,
        ChargingStatus::Charged => CellCharging::Charged,
        ChargingStatus::Unknown | ChargingStatus::Other(_) => CellCharging::Unknown,
    }
}

fn view_cell(c: Cell) -> HeadsetCell {
    HeadsetCell {
        percent: c.level,
        charging: cell_charging(c.charging),
    }
}

/// Stable lowercase key of an EQ preset id (`EqPresetId`); the UI translates it.
fn preset_key(id: u8) -> Option<String> {
    eq_preset_name(id).map(|name| match name {
        "USER_SETTING1" => "user1".to_owned(),
        "USER_SETTING2" => "user2".to_owned(),
        other => other.to_ascii_lowercase(),
    })
}

fn codec_text(c: AudioCodec) -> Option<String> {
    Some(
        match c {
            AudioCodec::Sbc => "SBC",
            AudioCodec::Aac => "AAC",
            AudioCodec::Ldac => "LDAC",
            AudioCodec::AptX => "aptX",
            AudioCodec::AptXHd => "aptX HD",
            AudioCodec::Lc3 => "LC3",
            AudioCodec::Unsettled | AudioCodec::Other(_) => return None,
        }
        .to_owned(),
    )
}

/// The lower of the two buds, `None` without battery data.
fn lowest_percent(b: &BatteryLr) -> u8 {
    b.left.level.min(b.right.level)
}

/// Overall charging state of the two buds for the generic battery field.
fn pair_charging(b: &BatteryLr) -> ChargingState {
    use ChargingStatus::{Charged, Charging, NotCharging};
    match (b.left.charging, b.right.charging) {
        (Charging, _) | (_, Charging) => ChargingState::Charging,
        (Charged, Charged) => ChargingState::Full,
        (NotCharging, NotCharging) => ChargingState::Discharging,
        _ => ChargingState::Unknown,
    }
}

fn headset_view(s: &Snapshot) -> HeadsetView {
    let nc_asm = s.ncasm.as_ref().map(|nc| {
        let p = &nc.param;
        let ranges = nc.capability.ranges();
        let range = ranges
            .iter()
            .find(|r| r.ambient_mode() == p.ambient_mode)
            .or_else(|| ranges.first());
        NcAsmView {
            mode: match p.mode {
                NcAsmMode::NoiseCancelling => NcAsmModeView::NoiseCancelling,
                NcAsmMode::Ambient => NcAsmModeView::Ambient,
            },
            ambient_mode: match p.ambient_mode {
                AmbientMode::Normal => AmbientModeView::Normal,
                AmbientMode::Voice => AmbientModeView::Voice,
            },
            level: p.level,
            level_min: range.map_or(p.level, |r| r.min()),
            level_max: range.map_or(p.level, |r| r.max()),
        }
    });
    let eq = s.eq.as_ref().map(|eq| EqView {
        preset: eq.param.preset,
        preset_name: preset_key(eq.param.preset),
        presets: eq
            .capability
            .presets()
            .iter()
            .map(|p| EqPresetView {
                id: p.id(),
                name: preset_key(p.id()),
            })
            .collect(),
        bands: eq.param.bands.clone(),
        step_count: eq.capability.step_count(),
    });
    let battery = (s.battery.is_some() || s.cradle.is_some()).then(|| HeadsetBattery {
        left: s.battery.map(|b| view_cell(b.left)),
        right: s.battery.map(|b| view_cell(b.right)),
        case: s.cradle.map(|c| view_cell(c.cell)),
    });
    HeadsetView {
        battery,
        nc_asm,
        eq,
        multipoint: s.multipoint,
        sidetone: s.sidetone,
        codec: s.codec.and_then(codec_text),
        firmware: Some(s.identity.firmware.clone()),
    }
}

/// The UI model for the headset. `connected == false` keeps the last values and marks
/// the device disconnected. `updated_at_ms` is 0: the caller stamps the read time.
pub fn to_detail(snapshot: &Snapshot, connected: bool) -> DeviceDetail {
    let percent = snapshot.battery.as_ref().map(lowest_percent);
    DeviceDetail {
        summary: DeviceSummary {
            id: DEVICE_ID.to_owned(),
            name: MODEL_NAME.to_owned(),
            kind: DeviceKind::Headset,
            connection: if connected {
                Connection::Connected
            } else {
                Connection::Disconnected
            },
            battery_percent: percent,
        },
        vendor: "Sony".to_owned(),
        transport: "Bluetooth".to_owned(),
        protocol: "Sony MDR".to_owned(),
        read_only: true,
        updated_at_ms: 0,
        illustration: Some("wf-c710n".to_owned()),
        battery: snapshot.battery.as_ref().map(|b| BatteryInfo {
            percent: lowest_percent(b),
            level: None,
            charging: pair_charging(b),
            external_power: false,
        }),
        dpi: None,
        report_rate: None,
        onboard: None,
        headset: Some(headset_view(snapshot)),
    }
}
