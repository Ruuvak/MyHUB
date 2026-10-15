//! Generic device model shared by the device modules and the app.
//!
//! These types are the Rust side of the IPC contract with the UI
//! (`.claude/crew/ipc-contract.md`): they serialize to camelCase JSON, enums as
//! lowercase (or camelCase where noted) strings. They carry data only; device
//! modules fill them and the app forwards them unchanged.

use serde::{Deserialize, Serialize};

/// What kind of peripheral a device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Headset,
    Gamepad,
}

/// Whether the device is reachable right now. `Disconnected` means paired but
/// switched off or asleep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Connection {
    Connected,
    Disconnected,
}

/// The short form of a device, used in lists and the tray.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    /// Stable identifier without serial numbers.
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub connection: Connection,
    /// Last known battery level, `None` when the device has no battery data.
    pub battery_percent: Option<u8>,
}

/// The full device view: the summary fields plus everything the UI can show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDetail {
    #[serde(flatten)]
    pub summary: DeviceSummary,
    pub vendor: String,
    pub transport: String,
    pub protocol: String,
    /// `true` unless device writes are allowed and the device is connected (see [`DeviceDetail::compute_read_only`]).
    pub read_only: bool,
    /// Unix milliseconds of the last successful read.
    pub updated_at_ms: u64,
    /// UI illustration key, `None` for a generic placeholder.
    pub illustration: Option<String>,
    pub battery: Option<BatteryInfo>,
    pub dpi: Option<DpiInfo>,
    pub report_rate: Option<ReportRateInfo>,
    pub onboard: Option<OnboardView>,
    /// Headset data (Sony WF-C710N); `None` for devices that are not headsets.
    pub headset: Option<HeadsetView>,
}

impl DeviceDetail {
    /// Contract rule: `!(allow_device_writes && connected)`. The mode does not
    /// matter: the app switches the mouse to host mode itself when a write needs it.
    pub fn compute_read_only(&self, allow_device_writes: bool) -> bool {
        !(allow_device_writes && self.summary.connection == Connection::Connected)
    }

    /// `true` when both details are equal in everything but `updated_at_ms`.
    pub fn same_content(&self, other: &Self) -> bool {
        let mut other = other.clone();
        other.updated_at_ms = self.updated_at_ms;
        *self == other
    }
}

/// Last values the user applied to a device, kept on the PC. `None` means the
/// user never set that value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DevicePrefs {
    pub dpi: Option<u16>,
    pub report_rate_hz: Option<u16>,
    /// The mode the user last chose: `host` is app control, `onboard` the mouse
    /// profiles (the default). Stored by enter and leave only; writes off, quit
    /// and shutdown return the mouse to onboard without changing it.
    pub mode: OnboardMode,
}

/// Coarse battery level reported by the mouse (0x1004 byte 1) [INF].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BatteryLevel {
    Full,
    Good,
    Low,
    Critical,
}

/// Charging state of the mouse (0x1004 byte 2, Solaar's names) [INF].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChargingState {
    Discharging,
    Charging,
    AlmostFull,
    Full,
    SlowCharging,
    /// invalid_battery or thermal_error.
    Error,
    Unknown,
}

/// Battery state as the mouse reports it (the percent is the mouse's own figure).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryInfo {
    pub percent: u8,
    /// `None` when byte 1 is not exactly one of the known level flags.
    pub level: Option<BatteryLevel>,
    pub charging: ChargingState,
    /// 0x1004 byte 3 != 0 [INF].
    pub external_power: bool,
}

/// Sensor resolution range and the current value, all in DPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiInfo {
    pub min: u16,
    pub max: u16,
    pub step: u16,
    pub current: u16,
}

/// Supported report rates and the current one, in Hz.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRateInfo {
    pub options_hz: Vec<u16>,
    pub current_hz: u16,
}

/// Who runs the profile logic: the host software or the device itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnboardMode {
    Host,
    #[default]
    Onboard,
}

/// Onboard profile memory as shown to the user (read only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardView {
    pub mode: Option<OnboardMode>,
    /// 1-based number of the active user profile, `None` when unknown.
    pub active_profile: Option<u8>,
    pub button_count: u8,
    /// User profiles first, then factory (ROM) profiles.
    pub profiles: Vec<ProfileInfo>,
    /// Number (3..=5) of the active profile when the mouse is in onboard mode and
    /// that profile's sector is enabled in the directory; `None` otherwise.
    /// Only such a profile can be saved to the mouse.
    pub writable_profile: Option<u8>,
}

/// Charging state of one headset cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CellCharging {
    NotCharging,
    Charging,
    Charged,
    Unknown,
}

/// One headset cell: a bud or the case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadsetCell {
    pub percent: u8,
    pub charging: CellCharging,
}

/// Per cell battery of a true wireless headset; a cell is `None` when not reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadsetBattery {
    pub left: Option<HeadsetCell>,
    pub right: Option<HeadsetCell>,
    pub case: Option<HeadsetCell>,
}

/// Noise cancelling or ambient sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NcAsmModeView {
    NoiseCancelling,
    Ambient,
}

/// Ambient sound flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AmbientModeView {
    Normal,
    Voice,
}

/// Noise cancelling / ambient sound state with the level range of the current ambient mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NcAsmView {
    pub mode: NcAsmModeView,
    pub ambient_mode: AmbientModeView,
    pub level: u8,
    pub level_min: u8,
    pub level_max: u8,
}

/// One equalizer preset the headset offers. `name` is a stable lowercase key
/// (`off`, `bright`, ..., `custom`, `user1`, `user2`) the UI translates; `None` when unlisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqPresetView {
    pub id: u8,
    pub name: Option<String>,
}

/// Equalizer state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqView {
    pub preset: u8,
    pub preset_name: Option<String>,
    pub presets: Vec<EqPresetView>,
    pub bands: Vec<u8>,
    pub step_count: u8,
}

/// Headset specific data (read only for now).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadsetView {
    pub battery: Option<HeadsetBattery>,
    pub nc_asm: Option<NcAsmView>,
    pub eq: Option<EqView>,
    pub multipoint: Option<bool>,
    pub sidetone: Option<bool>,
    /// Codec in use, e.g. `AAC`.
    pub codec: Option<String>,
    pub firmware: Option<String>,
}

/// One onboard profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInfo {
    /// `user-1`.. or `rom-1`..
    pub id: String,
    /// 1-based number within its group (user or ROM).
    pub number: u8,
    pub rom: bool,
    pub enabled: bool,
    pub name: Option<String>,
    /// `None` when the stored interval is not a valid one.
    pub report_rate_hz: Option<u16>,
    pub dpi_levels: Vec<u16>,
    pub default_dpi_index: u8,
    pub shift_dpi_index: u8,
    /// Only the first `button_count` slots.
    pub buttons: Vec<BindingInfo>,
    /// G-shift layer, only the first `button_count` slots.
    pub gshift: Vec<BindingInfo>,
    /// `false` for ROM sectors, which store no CRC.
    pub crc_ok: bool,
}

/// What a button slot is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BindingKind {
    Mouse,
    Keyboard,
    Consumer,
    Special,
    Unassigned,
    Unknown,
}

/// A device-side special function. Serializes camelCase (`wheelLeft`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SpecialAction {
    WheelLeft,
    WheelRight,
    DpiUp,
    DpiDown,
    DpiShift,
    ProfileCycle,
    GShift,
}

/// One button binding; only the fields that fit `kind` are set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingInfo {
    /// 0-based slot index; the UI labels it `G{slot+1}`.
    pub slot: u8,
    pub kind: BindingKind,
    /// Mouse button bitmask (0x01 left, 0x02 right, 0x04 middle, 0x08 back, 0x10 forward).
    pub mouse_buttons: Option<u16>,
    /// HID modifier bitmask for keyboard bindings.
    pub modifiers: Option<u8>,
    /// HID keyboard usage or consumer usage.
    pub usage: Option<u16>,
    pub special: Option<SpecialAction>,
    /// Raw special code, also set for codes without a known action.
    pub special_code: Option<u8>,
    /// The 4 raw binding bytes as lowercase hex.
    pub raw_hex: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> DeviceSummary {
        DeviceSummary {
            id: "g502x".into(),
            name: "G502 X".into(),
            kind: DeviceKind::Mouse,
            connection: Connection::Connected,
            battery_percent: Some(55),
        }
    }

    #[test]
    fn summary_serializes_as_ui_contract() {
        assert_eq!(
            serde_json::to_string(&summary()).unwrap(),
            r#"{"id":"g502x","name":"G502 X","kind":"mouse","connection":"connected","batteryPercent":55}"#
        );
    }

    fn detail() -> DeviceDetail {
        DeviceDetail {
            summary: summary(),
            vendor: "Logitech".into(),
            transport: "LIGHTSPEED".into(),
            protocol: "HID++ 2.0".into(),
            read_only: true,
            updated_at_ms: 1_700_000_000_000,
            illustration: Some("g502x".into()),
            battery: Some(BatteryInfo {
                percent: 55,
                level: Some(BatteryLevel::Good),
                charging: ChargingState::Discharging,
                external_power: false,
            }),
            dpi: Some(DpiInfo {
                min: 100,
                max: 25600,
                step: 50,
                current: 1600,
            }),
            report_rate: Some(ReportRateInfo {
                options_hz: vec![1000, 500],
                current_hz: 1000,
            }),
            headset: None,
            onboard: Some(OnboardView {
                mode: Some(OnboardMode::Host),
                active_profile: None,
                button_count: 11,
                writable_profile: Some(3),
                profiles: vec![ProfileInfo {
                    id: "user-1".into(),
                    number: 1,
                    rom: false,
                    enabled: true,
                    name: None,
                    report_rate_hz: Some(1000),
                    dpi_levels: vec![800, 1600],
                    default_dpi_index: 1,
                    shift_dpi_index: 0,
                    buttons: vec![BindingInfo {
                        slot: 4,
                        kind: BindingKind::Special,
                        mouse_buttons: None,
                        modifiers: None,
                        usage: None,
                        special: Some(SpecialAction::GShift),
                        special_code: Some(0x0b),
                        raw_hex: "900b0000".into(),
                    }],
                    gshift: vec![],
                    crc_ok: true,
                }],
            }),
        }
    }

    #[test]
    fn detail_flattens_summary_and_uses_camel_case() {
        let value = serde_json::to_value(detail()).unwrap();
        // Flattened summary fields sit at the top level.
        assert_eq!(value["id"], "g502x");
        assert_eq!(value["batteryPercent"], 55);
        assert!(value.get("summary").is_none());
        assert_eq!(value["readOnly"], true);
        assert_eq!(value["updatedAtMs"], 1_700_000_000_000u64);
        assert_eq!(value["battery"]["percent"], 55);
        assert_eq!(value["battery"]["level"], "good");
        assert_eq!(value["battery"]["charging"], "discharging");
        assert_eq!(value["battery"]["externalPower"], false);
        assert_eq!(value["onboard"]["writableProfile"], 3);
        assert_eq!(value["dpi"]["current"], 1600);
        assert_eq!(value["reportRate"]["optionsHz"][1], 500);
        assert_eq!(value["onboard"]["mode"], "host");
        assert!(value["onboard"]["activeProfile"].is_null());
        assert_eq!(value["onboard"]["buttonCount"], 11);
        let profile = &value["onboard"]["profiles"][0];
        assert_eq!(profile["reportRateHz"], 1000);
        assert_eq!(profile["defaultDpiIndex"], 1);
        assert_eq!(profile["crcOk"], true);
        let binding = &profile["buttons"][0];
        assert_eq!(binding["kind"], "special");
        assert_eq!(binding["special"], "gShift");
        assert_eq!(binding["specialCode"], 11);
        assert_eq!(binding["rawHex"], "900b0000");
        assert!(binding["mouseButtons"].is_null());
    }

    #[test]
    fn charging_states_serialize_camel_case() {
        let name = |c: ChargingState| {
            serde_json::to_value(c)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(name(ChargingState::AlmostFull), "almostFull");
        assert_eq!(name(ChargingState::SlowCharging), "slowCharging");
        assert_eq!(name(ChargingState::Error), "error");
        assert_eq!(name(ChargingState::Unknown), "unknown");
        let level = serde_json::to_value(BatteryLevel::Critical).unwrap();
        assert_eq!(level, "critical");
    }

    #[test]
    fn special_actions_serialize_camel_case() {
        let names: Vec<String> = [
            SpecialAction::WheelLeft,
            SpecialAction::WheelRight,
            SpecialAction::DpiUp,
            SpecialAction::DpiDown,
            SpecialAction::DpiShift,
            SpecialAction::ProfileCycle,
            SpecialAction::GShift,
        ]
        .iter()
        .map(|a| {
            serde_json::to_value(a)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
        assert_eq!(
            names,
            [
                "wheelLeft",
                "wheelRight",
                "dpiUp",
                "dpiDown",
                "dpiShift",
                "profileCycle",
                "gShift"
            ]
        );
    }

    #[test]
    fn read_only_follows_the_contract() {
        let mut d = detail();
        assert!(!d.compute_read_only(true));
        assert!(d.compute_read_only(false));
        d.summary.connection = Connection::Disconnected;
        assert!(d.compute_read_only(true));
        d.summary.connection = Connection::Connected;
        // The onboard mode is irrelevant.
        d.onboard.as_mut().unwrap().mode = Some(OnboardMode::Onboard);
        assert!(!d.compute_read_only(true));
        d.onboard.as_mut().unwrap().mode = None;
        assert!(!d.compute_read_only(true));
        d.onboard = None;
        assert!(!d.compute_read_only(true));
    }

    #[test]
    fn device_prefs_round_trip_and_tolerate_missing_fields() {
        let prefs = DevicePrefs {
            dpi: Some(1200),
            report_rate_hz: None,
            mode: OnboardMode::Host,
        };
        let json = serde_json::to_string(&prefs).unwrap();
        assert_eq!(json, r#"{"dpi":1200,"reportRateHz":null,"mode":"host"}"#);
        // An old file without a mode means onboard.
        let old: DevicePrefs = serde_json::from_str(r#"{"dpi":800}"#).unwrap();
        assert_eq!(old.mode, OnboardMode::Onboard);
        assert_eq!(serde_json::from_str::<DevicePrefs>(&json).unwrap(), prefs);
        assert_eq!(
            serde_json::from_str::<DevicePrefs>("{}").unwrap(),
            DevicePrefs::default()
        );
    }

    #[test]
    fn same_content_ignores_the_timestamp_only() {
        let a = detail();
        let mut b = detail();
        b.updated_at_ms += 60_000;
        assert!(a.same_content(&b));
        b.summary.battery_percent = Some(54);
        assert!(!a.same_content(&b));
    }
}
