//! Maps a [`Snapshot`] to the generic UI model of `myhub-core`.

use keychron_proto::Keycode;
use keychron_proto::he::{AdvanceMode, KeyMode, ProfileBuffer};
use keychron_proto::plan::Keymap;
use myhub_core::{
    Connection, ConnectionPath, DeviceDetail, DeviceKind, DeviceSummary, HeModeView, HeProfileView,
    HeView, KeyboardView, KeymapKeyView, KeymapLayerView, LightingView, TimersView,
};

use crate::cache::KeyboardCache;
use crate::snapshot::Snapshot;
use crate::{DEVICE_ID, DISPLAY_NAME};

/// Transport shown over the cable.
pub const TRANSPORT_USB: &str = "USB";
/// Transport shown through the Keychron Link receiver.
pub const TRANSPORT_RECEIVER: &str = "2.4 GHz (Keychron Link)";
/// Protocol shown over the receiver when no settings are known.
pub const PROTOCOL_LINK: &str = "Keychron Link";

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
    shell(
        TRANSPORT_USB,
        usb_protocol(snapshot),
        connected,
        usb_view(
            snapshot,
            other_configurator,
            connected && !other_configurator,
        ),
    )
}

/// The cache of the last USB read: the view as shown over the cable plus the protocol
/// string, stamped with `at_ms` as the read time.
pub fn cache_of(snapshot: &Snapshot, at_ms: u64) -> KeyboardCache {
    let mut view = usb_view(snapshot, false, false);
    view.cached_at_ms = Some(at_ms);
    KeyboardCache {
        protocol: usb_protocol(snapshot),
        view,
    }
}

/// The keyboard seen through the 2.4 GHz receiver. The settings are the cached ones of
/// the last USB read (an empty view without a cache); nothing here is read from the
/// keyboard. `connected` false keeps the cached values and marks the keyboard as gone.
pub fn to_receiver_detail(
    cache: Option<&KeyboardCache>,
    receiver_firmware: Option<String>,
    connected: bool,
    other_configurator: bool,
) -> DeviceDetail {
    let (protocol, mut view) = match cache {
        Some(c) => (c.protocol.clone(), c.view.clone()),
        None => (
            PROTOCOL_LINK.to_owned(),
            KeyboardView {
                firmware: String::new(),
                via_protocol: 0,
                layer_count: 0,
                default_layer: None,
                he: None,
                lighting: None,
                timers: None,
                keymap: None,
                other_configurator: false,
                connection_path: ConnectionPath::Receiver,
                configurable: false,
                cached_at_ms: None,
                receiver_firmware: None,
            },
        ),
    };
    view.connection_path = ConnectionPath::Receiver;
    view.configurable = false;
    view.other_configurator = connected && other_configurator;
    view.receiver_firmware = receiver_firmware;
    shell(TRANSPORT_RECEIVER, protocol, connected, view)
}

fn usb_protocol(snapshot: &Snapshot) -> String {
    format!(
        "VIA {} / Keychron HE {}",
        snapshot.via_protocol, snapshot.he.version
    )
}

fn shell(transport: &str, protocol: String, connected: bool, view: KeyboardView) -> DeviceDetail {
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
        transport: transport.to_owned(),
        protocol,
        read_only: true,
        updated_at_ms: 0,
        illustration: Some("k2he".to_owned()),
        battery: None,
        dpi: None,
        report_rate: None,
        onboard: None,
        headset: None,
        keyboard: Some(view),
    }
}

fn usb_view(snapshot: &Snapshot, other_configurator: bool, configurable: bool) -> KeyboardView {
    KeyboardView {
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
        connection_path: ConnectionPath::Usb,
        configurable,
        cached_at_ms: None,
        receiver_firmware: None,
    }
}
