//! Maps a [`Snapshot`] to the generic UI model of `myhub-core`.

use keychron_proto::Keycode;
use keychron_proto::he::{AdvanceMode, KeyMode, ProfileBuffer};
use keychron_proto::plan::Keymap;
use myhub_core::{
    Connection, DeviceDetail, DeviceKind, DeviceSummary, HeModeView, HeProfileView, HeView,
    KeyboardView, KeymapKeyView, KeymapLayerView, LightingView, TimersView,
};

use crate::snapshot::Snapshot;
use crate::{DEVICE_ID, DISPLAY_NAME};

/// Highest lighting effect number of the K2 HE (the launcher offers 0..=24) [INF].
const EFFECT_MAX: u8 = 24;

fn tenth_mm(value: u8) -> f32 {
    f32::from(value) / 10.0
}

fn count(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

fn profile_view(index: usize, p: &ProfileBuffer) -> HeProfileView {
    let global = &p.global;
    HeProfileView {
        index: u8::try_from(index).unwrap_or(u8::MAX),
        name: (!p.name.is_empty()).then(|| p.name.clone()),
        // A global word cannot point at itself: an unknown mode shows as global.
        mode: match global.mode {
            KeyMode::Static => HeModeView::Static,
            KeyMode::RapidTrigger => HeModeView::RapidTrigger,
            KeyMode::Global | KeyMode::Unknown(_) => HeModeView::Global,
        },
        actuation_mm: tenth_mm(global.distance),
        press_mm: tenth_mm(global.press),
        release_mm: tenth_mm(global.release),
        per_key_overrides: count(p.keys.iter().filter(|k| k.mode != KeyMode::Global).count()),
        dks_keys: count(
            p.keys
                .iter()
                .filter(|k| k.advance_mode == AdvanceMode::Dks)
                .count(),
        ),
        snap_pairs: count(p.snap.iter().filter(|s| s.used).count()),
        analog_keys: count(
            p.keys
                .iter()
                .filter(|k| k.advance_mode == AdvanceMode::Analog)
                .count(),
        ),
    }
}

fn he_view(snapshot: &Snapshot) -> HeView {
    HeView {
        current_profile: snapshot.profiles_info.current,
        profiles: snapshot
            .profiles
            .iter()
            .enumerate()
            .map(|(i, p)| profile_view(i, p))
            .collect(),
        curve: snapshot
            .curve
            .map(|c| c.points.iter().map(|p| [p.x, p.y]).collect()),
    }
}

fn lighting_view(snapshot: &Snapshot) -> Option<LightingView> {
    let light = &snapshot.light;
    Some(LightingView {
        brightness: light.brightness?,
        effect: light.effect?,
        speed: light.speed?,
        hue: light.hue?,
        saturation: light.saturation?,
        effect_max: EFFECT_MAX,
        per_key: snapshot.lighting.caps.is_some_and(|c| c.per_key),
        mix: snapshot.lighting.caps.is_some_and(|c| c.mix),
        led_count: snapshot.lighting.led_count.unwrap_or(0),
    })
}

/// The layers with the matrix slots that hold a keycode on at least one layer; a slot
/// that reads 0x0000 everywhere has no physical key.
fn keymap_view(keymaps: &[Keymap]) -> Vec<KeymapLayerView> {
    let used = |row: u8, col: u8| {
        keymaps
            .iter()
            .any(|m| m.get(row, col).is_some_and(|code| code != 0))
    };
    keymaps
        .iter()
        .map(|m| KeymapLayerView {
            index: m.layer,
            keys: (0..m.rows)
                .flat_map(|row| (0..m.cols).map(move |col| (row, col)))
                .filter(|&(row, col)| used(row, col))
                .filter_map(|(row, col)| {
                    let code = m.get(row, col)?;
                    Some(KeymapKeyView {
                        row,
                        col,
                        code,
                        label: Keycode::decode(code).to_string(),
                    })
                })
                .collect(),
        })
        .collect()
}

/// The UI model of the keyboard. `connected` is false once the link dropped (the last
/// values stay); `other_configurator` is true while the worker backs off because another
/// program talks to the keyboard. `updated_at_ms` is stamped by the caller.
pub fn to_detail(snapshot: &Snapshot, connected: bool, other_configurator: bool) -> DeviceDetail {
    DeviceDetail {
        summary: DeviceSummary {
            id: DEVICE_ID.to_owned(),
            name: DISPLAY_NAME.to_owned(),
            kind: DeviceKind::Keyboard,
            connection: if connected {
                Connection::Connected
            } else {
                Connection::Disconnected
            },
            battery_percent: None,
        },
        vendor: "Keychron".to_owned(),
        transport: "USB".to_owned(),
        protocol: format!(
            "VIA {} / Keychron HE {}",
            snapshot.via_protocol, snapshot.he.version
        ),
        read_only: true,
        updated_at_ms: 0,
        illustration: Some("k2he".to_owned()),
        battery: None,
        dpi: None,
        report_rate: None,
        onboard: None,
        headset: None,
        keyboard: Some(KeyboardView {
            firmware: snapshot.firmware.clone().unwrap_or_default(),
            via_protocol: snapshot.via_protocol,
            layer_count: snapshot
                .layer_count
                .or_else(|| {
                    snapshot
                        .keymaps
                        .as_ref()
                        .and_then(|k| u8::try_from(k.len()).ok())
                })
                .unwrap_or(0),
            default_layer: snapshot.layer.map(|l| l.default_layer),
            he: Some(he_view(snapshot)),
            lighting: lighting_view(snapshot),
            timers: snapshot.timers.map(|t| TimersView {
                backlight_off_s: t.backlight_off_s,
                sleep_s: t.sleep_s,
                he_scan_idle_s: t.he_scan_idle_s,
            }),
            keymap: snapshot.keymaps.as_deref().map(keymap_view),
            other_configurator,
        }),
    }
}
