//! Typed requests and parsed incoming messages (table 1 and table 2 payloads).
//!
//! Layouts are the ones observed on the WF-C710N (`DEVICE-WF-C710N.md`) and, where
//! stated, read from `payloads.md`. Parsing never fails: a payload that is unknown or
//! does not match its layout becomes [`Message::Unknown`] with the raw bytes, so one
//! odd frame cannot stop a stream.
//!
//! Requests built here are unchecked [`Request`] values. They become bytes only
//! through [`crate::policy::Policy::check`].

use serde::Serialize;

use crate::frame::{DataType, ReceivedFrame};
use crate::function::{FunctionEntry, Table, parse_support_function};

/// Command bytes used by this crate (table 1 unless noted).
pub mod command {
    pub const CONNECT_GET_PROTOCOL_INFO: u8 = 0x00;
    pub const CONNECT_RET_PROTOCOL_INFO: u8 = 0x01;
    pub const CONNECT_GET_CAPABILITY_INFO: u8 = 0x02;
    pub const CONNECT_GET_DEVICE_INFO: u8 = 0x04;
    pub const CONNECT_RET_DEVICE_INFO: u8 = 0x05;
    pub const CONNECT_GET_SUPPORT_FUNCTION: u8 = 0x06;
    pub const CONNECT_RET_SUPPORT_FUNCTION: u8 = 0x07;
    pub const COMMON_GET_STATUS: u8 = 0x12;
    pub const COMMON_RET_STATUS: u8 = 0x13;
    pub const POWER_GET_STATUS: u8 = 0x22;
    pub const POWER_RET_STATUS: u8 = 0x23;
    pub const POWER_NTFY_STATUS: u8 = 0x25;
    pub const EQEBB_GET_CAPABILITY: u8 = 0x50;
    pub const EQEBB_RET_CAPABILITY: u8 = 0x51;
    pub const EQEBB_GET_PARAM: u8 = 0x56;
    pub const EQEBB_RET_PARAM: u8 = 0x57;
    pub const EQEBB_SET_PARAM: u8 = 0x58;
    pub const EQEBB_NTFY_PARAM: u8 = 0x59;
    pub const NCASM_GET_CAPABILITY: u8 = 0x60;
    pub const NCASM_RET_CAPABILITY: u8 = 0x61;
    pub const NCASM_GET_PARAM: u8 = 0x66;
    pub const NCASM_RET_PARAM: u8 = 0x67;
    pub const NCASM_SET_PARAM: u8 = 0x68;
    pub const NCASM_NTFY_PARAM: u8 = 0x69;
    pub const ALERT_SET_PARAM: u8 = 0x98;
    pub const ALERT_NTFY_PARAM: u8 = 0x99;
    pub const GENERAL_SETTING_GET_CAPABILITY: u8 = 0xD0;
    pub const GENERAL_SETTING_GET_PARAM: u8 = 0xD6;
    pub const GENERAL_SETTING_RET_PARAM: u8 = 0xD7;
    pub const GENERAL_SETTING_SET_PARAM: u8 = 0xD8;
    pub const GENERAL_SETTING_NTFY_PARAM: u8 = 0xD9;
}

/// EQ preset id of CUSTOM (`EqPresetId`, `enums.md`).
pub const EQ_PRESET_CUSTOM: u8 = 0xA0;
/// Alert message id DISCONNECT_CAUSED_BY_CHANGING_MULTIPOINT (`AlertMessageType`).
pub const ALERT_MULTIPOINT_DISCONNECT: u8 = 0x07;

/// An unchecked request. It has no encoder; give it to [`crate::policy::Policy::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub data_type: DataType,
    pub payload: Vec<u8>,
}

impl Request {
    pub fn new(data_type: DataType, payload: Vec<u8>) -> Self {
        Self { data_type, payload }
    }

    /// A request on table 1 (`DATA_MDR`).
    pub fn table1(payload: Vec<u8>) -> Self {
        Self::new(DataType::DataMdr, payload)
    }

    /// A request on table 2 (`DATA_MDR_NO2`).
    pub fn table2(payload: Vec<u8>) -> Self {
        Self::new(DataType::DataMdrNo2, payload)
    }
}

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/// CONNECT_GET_PROTOCOL_INFO `00 00`.
pub fn protocol_info(table: Table) -> Request {
    Request::new(table.data_type(), vec![0x00, 0x00])
}

/// CONNECT_GET_CAPABILITY_INFO `02 00`.
pub fn capability_info(table: Table) -> Request {
    Request::new(table.data_type(), vec![0x02, 0x00])
}

/// Device info kind of CONNECT_GET_DEVICE_INFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceInfoKind {
    /// `04 01`
    ModelName,
    /// `04 02`
    FirmwareVersion,
    /// `04 03`
    SeriesAndColor,
}

/// CONNECT_GET_DEVICE_INFO `04 <kind>`.
pub fn device_info(table: Table, kind: DeviceInfoKind) -> Request {
    let k = match kind {
        DeviceInfoKind::ModelName => 0x01,
        DeviceInfoKind::FirmwareVersion => 0x02,
        DeviceInfoKind::SeriesAndColor => 0x03,
    };
    Request::new(table.data_type(), vec![0x04, k])
}

/// CONNECT_GET_SUPPORT_FUNCTION `06 00`.
pub fn support_function(table: Table) -> Request {
    Request::new(table.data_type(), vec![0x06, 0x00])
}

/// POWER_GET_STATUS `22 09`, left and right battery with threshold.
pub fn battery_lr() -> Request {
    Request::table1(vec![0x22, 0x09])
}

/// POWER_GET_STATUS `22 0a`, cradle (case) battery with threshold.
pub fn battery_cradle() -> Request {
    Request::table1(vec![0x22, 0x0A])
}

/// NCASM_GET_CAPABILITY `60 <t>`.
pub fn ncasm_capability(inquired: u8) -> Request {
    Request::table1(vec![0x60, inquired])
}

/// NCASM_GET_PARAM `66 <t>`.
pub fn ncasm_get_param(inquired: u8) -> Request {
    Request::table1(vec![0x66, inquired])
}

/// NCASM_SET_PARAM `68 <t> <status> <total> <mode> <ambient mode> <level>`.
pub fn ncasm_set_param(inquired: u8, p: &NcAsmParam) -> Request {
    Request::table1(vec![
        0x68,
        inquired,
        p.status.to_u8(),
        u8::from(p.total_effect),
        p.mode.to_u8(),
        p.ambient_mode.to_u8(),
        p.level,
    ])
}

/// EQEBB_GET_CAPABILITY `50 00`.
pub fn eq_capability() -> Request {
    Request::table1(vec![0x50, 0x00])
}

/// EQEBB_GET_PARAM `56 00`.
pub fn eq_get_param() -> Request {
    Request::table1(vec![0x56, 0x00])
}

/// Select a preset: `58 00 <preset id> 00` (empty band list, as the app does).
pub fn eq_select_preset(preset: u8) -> Request {
    Request::table1(vec![0x58, 0x00, preset, 0x00])
}

/// Write the bands of CUSTOM: `58 00 a0 <n> <steps>`. Select CUSTOM first, the
/// policy cannot know which preset is active (see `policy` module doc).
pub fn eq_set_bands(steps: &[u8]) -> Request {
    let mut p = vec![0x58, 0x00, EQ_PRESET_CUSTOM, steps.len() as u8];
    p.extend_from_slice(steps);
    Request::table1(p)
}

/// COMMON_GET_STATUS `12 02`, audio codec.
pub fn codec_status() -> Request {
    Request::table1(vec![0x12, 0x02])
}

/// General setting kinds seen on the WF-C710N.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GeneralSettingKind {
    /// Type `0xD1`, function GENERAL_SETTING_1.
    Multipoint,
    /// Type `0xD2`, function GENERAL_SETTING_2.
    Sidetone,
}

impl GeneralSettingKind {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Multipoint => 0xD1,
            Self::Sidetone => 0xD2,
        }
    }

    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            0xD1 => Some(Self::Multipoint),
            0xD2 => Some(Self::Sidetone),
            _ => None,
        }
    }
}

/// GENERAL_SETTING_GET_CAPABILITY `d0 <kind>`.
pub fn general_setting_capability(kind: GeneralSettingKind) -> Request {
    Request::table1(vec![0xD0, kind.to_u8()])
}

/// GENERAL_SETTING_GET_PARAM `d6 <kind>`.
pub fn general_setting_get(kind: GeneralSettingKind) -> Request {
    Request::table1(vec![0xD6, kind.to_u8()])
}

/// GENERAL_SETTING_SET_PARAM `d8 <kind> 00 <value>`; value `00` is ON, `01` is OFF.
pub fn general_setting_set(kind: GeneralSettingKind, enabled: bool) -> Request {
    Request::table1(vec![
        0xD8,
        kind.to_u8(),
        0x00,
        if enabled { 0x00 } else { 0x01 },
    ])
}

// ---------------------------------------------------------------------------
// Parsed messages
// ---------------------------------------------------------------------------

/// How a message reached us.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// `RET_*`, the answer to a GET.
    Reply,
    /// `NTFY_*`, pushed by the headset.
    Notification,
    /// Not classified (unknown command).
    Other,
}

/// A parsed incoming payload.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Incoming {
    pub table: Table,
    pub origin: Origin,
    pub message: Message,
}

/// `01 00 <version u32 BE> <table 1> <table 2>`. Enabled is byte `0x00`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolInfo {
    pub version: u32,
    pub table1_enabled: bool,
    pub table2_enabled: bool,
}

/// Device info reply, `05 <kind> ...`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceInfo {
    /// `05 01 <len> <text>`
    Model(String),
    /// `05 02 <len> <text>`
    Firmware(String),
    /// `05 03 <b0> <b1>`, meaning of the bytes \[UNK\] (`50 01` on the WF-C710N).
    SeriesAndColor(u8, u8),
}

/// Charging byte of the POWER status messages (`BatteryChargingStatus`, `enums.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChargingStatus {
    NotCharging,
    Charging,
    Unknown,
    Charged,
    Other(u8),
}

impl ChargingStatus {
    fn from_u8(b: u8) -> Self {
        match b {
            0 => Self::NotCharging,
            1 => Self::Charging,
            2 => Self::Unknown,
            3 => Self::Charged,
            o => Self::Other(o),
        }
    }
}

/// One earbud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    pub level: u8,
    pub charging: ChargingStatus,
}

/// POWER type `09`: `[L level][L charging][R level][R charging][L thr][R thr]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryLr {
    pub left: Cell,
    pub right: Cell,
    pub left_threshold: u8,
    pub right_threshold: u8,
}

/// POWER type `0a`: `[level][charging][threshold]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cradle {
    pub cell: Cell,
    pub threshold: u8,
}

/// ValueChangeStatus: `00` under changing, `01` changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ValueChangeStatus {
    UnderChanging,
    Changed,
}

impl ValueChangeStatus {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::UnderChanging => 0,
            Self::Changed => 1,
        }
    }
}

/// NC/ASM mode byte: `00` noise cancelling, `01` ambient sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NcAsmMode {
    NoiseCancelling,
    Ambient,
}

impl NcAsmMode {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::NoiseCancelling => 0,
            Self::Ambient => 1,
        }
    }
}

/// Ambient sound mode: `00` normal, `01` voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AmbientMode {
    Normal,
    Voice,
}

impl AmbientMode {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Voice => 1,
        }
    }
}

/// NC/ASM param for inquired type 0x17: `[status][total][mode][ambient mode][level]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NcAsmParam {
    pub status: ValueChangeStatus,
    pub total_effect: bool,
    pub mode: NcAsmMode,
    pub ambient_mode: AmbientMode,
    pub level: u8,
}

/// One entry of the NC/ASM capability, `[ambient mode][min][max][step]` (field order
/// min, max, step is \[INF\] in the device notes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AmbientRange {
    ambient_mode: AmbientMode,
    min: u8,
    max: u8,
    step: u8,
}

impl AmbientRange {
    pub fn ambient_mode(&self) -> AmbientMode {
        self.ambient_mode
    }

    pub fn min(&self) -> u8 {
        self.min
    }

    pub fn max(&self) -> u8 {
        self.max
    }

    pub fn step(&self) -> u8 {
        self.step
    }
}

/// `61 17 <count> [entry]*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NcAsmCapability {
    ranges: Vec<AmbientRange>,
}

impl NcAsmCapability {
    pub fn ranges(&self) -> &[AmbientRange] {
        &self.ranges
    }
}

/// One preset of the EQ capability: `[id][extra len][extra]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqPresetInfo {
    id: u8,
    extra: Vec<u8>,
}

impl EqPresetInfo {
    pub fn id(&self) -> u8 {
        self.id
    }

    pub fn extra(&self) -> &[u8] {
        &self.extra
    }
}

/// `51 00 <band count> <step count> <preset count> [preset]*`. The step count is the
/// number of steps, steps run `0..step_count`, flat is `(step_count - 1) / 2`
/// (\[INF\], `DEVICE-WF-C710N.md` Round 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqCapability {
    band_count: u8,
    step_count: u8,
    presets: Vec<EqPresetInfo>,
}

impl EqCapability {
    pub fn band_count(&self) -> u8 {
        self.band_count
    }

    pub fn step_count(&self) -> u8 {
        self.step_count
    }

    pub fn presets(&self) -> &[EqPresetInfo] {
        &self.presets
    }
}

/// `57|59 00 <preset> <band count> <steps>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqParam {
    pub preset: u8,
    pub bands: Vec<u8>,
}

/// General setting param `d7|d9 <kind> 00 <value>`; value `00` = ON, `01` = OFF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSetting {
    pub kind: GeneralSettingKind,
    pub enabled: bool,
}

/// Audio codec byte (`AudioCodec`, `enums.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioCodec {
    Unsettled,
    Sbc,
    Aac,
    Ldac,
    AptX,
    AptXHd,
    Lc3,
    Other(u8),
}

impl AudioCodec {
    fn from_u8(b: u8) -> Self {
        match b {
            0x00 => Self::Unsettled,
            0x01 => Self::Sbc,
            0x02 => Self::Aac,
            0x10 => Self::Ldac,
            0x20 => Self::AptX,
            0x21 => Self::AptXHd,
            0x30 => Self::Lc3,
            o => Self::Other(o),
        }
    }
}

/// Permission to answer an alert. Created only by [`parse_received`] from a frame the
/// link delivered as `99 00 <id> 01` (FIXED_MESSAGE, POSITIVE_NEGATIVE), not `Clone`,
/// consumed by [`crate::policy::Policy::check_alert_answer`]. It records the link epoch
/// it was received in; [`crate::link::Link::send`] refuses an answer from an older epoch.
///
/// ```compile_fail
/// let _ = mdr::AlertToken { message_id: 7, epoch: 0 };
/// ```
///
/// ```compile_fail
/// let _ = mdr::AlertToken::new(7, 0);
/// ```
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<mdr::AlertToken>();
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct AlertToken {
    message_id: u8,
    epoch: u64,
}

impl AlertToken {
    fn new(message_id: u8, epoch: u64) -> Self {
        Self { message_id, epoch }
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The alert message id this token answers.
    pub fn message_id(&self) -> u8 {
        self.message_id
    }
}

/// `99 00 <message id> <action type>`. `token` is present when the action type is
/// `01` (POSITIVE_NEGATIVE) and can be taken once with [`Alert::take_token`].
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    pub message_id: u8,
    pub action_type: u8,
    #[serde(skip)]
    token: Option<AlertToken>,
}

impl Alert {
    pub fn take_token(&mut self) -> Option<AlertToken> {
        self.token.take()
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// True for the multipoint disconnect alert (id 0x07).
    pub fn is_multipoint_disconnect(&self) -> bool {
        self.message_id == ALERT_MULTIPOINT_DISCONNECT
    }
}

/// Raw payload of anything not parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unknown {
    pub command: u8,
    pub raw: Vec<u8>,
}

/// The parsed payload.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Message {
    ProtocolInfo(ProtocolInfo),
    DeviceInfo(DeviceInfo),
    SupportFunction(Vec<FunctionEntry>),
    BatteryLr(BatteryLr),
    Cradle(Cradle),
    NcAsmCapability(NcAsmCapability),
    NcAsmParam(NcAsmParam),
    EqCapability(EqCapability),
    EqParam(EqParam),
    /// A bare `59 00`: something changed or a request was ignored. Re-read with `56 00`.
    EqChanged,
    GeneralSetting(GeneralSetting),
    Alert(Alert),
    Codec(AudioCodec),
    Unknown(Unknown),
}

fn text(rest: &[u8]) -> Option<String> {
    let (&len, body) = rest.split_first()?;
    if body.len() != usize::from(len) {
        return None;
    }
    Some(String::from_utf8_lossy(body).into_owned())
}

fn cell(level: u8, charging: u8) -> Cell {
    Cell {
        level,
        charging: ChargingStatus::from_u8(charging),
    }
}

fn parse_ncasm_param(p: &[u8]) -> Option<NcAsmParam> {
    let [_, 0x17, status, total, mode, amb, level] = *p else {
        return None;
    };
    Some(NcAsmParam {
        status: match status {
            0 => ValueChangeStatus::UnderChanging,
            1 => ValueChangeStatus::Changed,
            _ => return None,
        },
        total_effect: match total {
            0 => false,
            1 => true,
            _ => return None,
        },
        mode: match mode {
            0 => NcAsmMode::NoiseCancelling,
            1 => NcAsmMode::Ambient,
            _ => return None,
        },
        ambient_mode: match amb {
            0 => AmbientMode::Normal,
            1 => AmbientMode::Voice,
            _ => return None,
        },
        level,
    })
}

fn parse_ncasm_capability(p: &[u8]) -> Option<NcAsmCapability> {
    let [_, 0x17, n, rest @ ..] = p else {
        return None;
    };
    if rest.len() != usize::from(*n) * 4 {
        return None;
    }
    let mut ranges = Vec::new();
    for e in rest.as_chunks::<4>().0 {
        ranges.push(AmbientRange {
            ambient_mode: match e[0] {
                0 => AmbientMode::Normal,
                1 => AmbientMode::Voice,
                _ => return None,
            },
            min: e[1],
            max: e[2],
            step: e[3],
        });
    }
    Some(NcAsmCapability { ranges })
}

fn parse_eq_capability(p: &[u8]) -> Option<EqCapability> {
    let [_, 0x00, bands, steps, count, rest @ ..] = p else {
        return None;
    };
    let mut rest = rest;
    let mut presets = Vec::new();
    for _ in 0..*count {
        let [id, elen, tail @ ..] = rest else {
            return None;
        };
        let elen = usize::from(*elen);
        if tail.len() < elen {
            return None;
        }
        presets.push(EqPresetInfo {
            id: *id,
            extra: tail[..elen].to_vec(),
        });
        rest = &tail[elen..];
    }
    if !rest.is_empty() {
        return None;
    }
    Some(EqCapability {
        band_count: *bands,
        step_count: *steps,
        presets,
    })
}

fn parse_eq_param(p: &[u8]) -> Option<EqParam> {
    let [_, 0x00, preset, n, steps @ ..] = p else {
        return None;
    };
    if steps.len() != usize::from(*n) {
        return None;
    }
    Some(EqParam {
        preset: *preset,
        bands: steps.to_vec(),
    })
}

fn parse_general_setting(p: &[u8]) -> Option<GeneralSetting> {
    let [_, kind, 0x00, value] = *p else {
        return None;
    };
    Some(GeneralSetting {
        kind: GeneralSettingKind::from_u8(kind)?,
        enabled: match value {
            0x00 => true,
            0x01 => false,
            _ => return None,
        },
    })
}

fn parse_known(table: Table, p: &[u8]) -> Option<(Origin, Message)> {
    use Origin::{Notification, Reply};
    let cmd = *p.first()?;
    match (table, cmd) {
        (_, command::CONNECT_RET_PROTOCOL_INFO) => {
            let [_, 0x00, a, b, c, d, t1, t2] = *p else {
                return None;
            };
            Some((
                Reply,
                Message::ProtocolInfo(ProtocolInfo {
                    version: u32::from_be_bytes([a, b, c, d]),
                    table1_enabled: t1 == 0,
                    table2_enabled: t2 == 0,
                }),
            ))
        }
        (_, command::CONNECT_RET_DEVICE_INFO) => {
            let info = match p {
                [_, 0x01, rest @ ..] => DeviceInfo::Model(text(rest)?),
                [_, 0x02, rest @ ..] => DeviceInfo::Firmware(text(rest)?),
                [_, 0x03, a, b] => DeviceInfo::SeriesAndColor(*a, *b),
                _ => return None,
            };
            Some((Reply, Message::DeviceInfo(info)))
        }
        (_, command::CONNECT_RET_SUPPORT_FUNCTION) => {
            Some((Reply, Message::SupportFunction(parse_support_function(p)?)))
        }
        (Table::One, command::POWER_RET_STATUS | command::POWER_NTFY_STATUS) => {
            let origin = if cmd == command::POWER_RET_STATUS {
                Reply
            } else {
                Notification
            };
            match p {
                [_, 0x09, ll, lc, rl, rc, lt, rt] => Some((
                    origin,
                    Message::BatteryLr(BatteryLr {
                        left: cell(*ll, *lc),
                        right: cell(*rl, *rc),
                        left_threshold: *lt,
                        right_threshold: *rt,
                    }),
                )),
                [_, 0x0A, level, charging, thr] => Some((
                    origin,
                    Message::Cradle(Cradle {
                        cell: cell(*level, *charging),
                        threshold: *thr,
                    }),
                )),
                _ => None,
            }
        }
        (Table::One, command::NCASM_RET_CAPABILITY) => {
            Some((Reply, Message::NcAsmCapability(parse_ncasm_capability(p)?)))
        }
        (Table::One, command::NCASM_RET_PARAM) => {
            Some((Reply, Message::NcAsmParam(parse_ncasm_param(p)?)))
        }
        (Table::One, command::NCASM_NTFY_PARAM) => {
            Some((Notification, Message::NcAsmParam(parse_ncasm_param(p)?)))
        }
        (Table::One, command::EQEBB_RET_CAPABILITY) => {
            Some((Reply, Message::EqCapability(parse_eq_capability(p)?)))
        }
        (Table::One, command::EQEBB_RET_PARAM) => {
            Some((Reply, Message::EqParam(parse_eq_param(p)?)))
        }
        (Table::One, command::EQEBB_NTFY_PARAM) => {
            if p == [command::EQEBB_NTFY_PARAM, 0x00] {
                Some((Notification, Message::EqChanged))
            } else {
                Some((Notification, Message::EqParam(parse_eq_param(p)?)))
            }
        }
        (Table::One, command::GENERAL_SETTING_RET_PARAM) => {
            Some((Reply, Message::GeneralSetting(parse_general_setting(p)?)))
        }
        (Table::One, command::GENERAL_SETTING_NTFY_PARAM) => Some((
            Notification,
            Message::GeneralSetting(parse_general_setting(p)?),
        )),
        (Table::One, command::ALERT_NTFY_PARAM) => {
            let [_, 0x00, id, action] = *p else {
                return None;
            };
            Some((
                Notification,
                Message::Alert(Alert {
                    message_id: id,
                    action_type: action,
                    token: None,
                }),
            ))
        }
        (Table::One, command::COMMON_RET_STATUS) => {
            let [_, 0x02, codec] = *p else {
                return None;
            };
            Some((Reply, Message::Codec(AudioCodec::from_u8(codec))))
        }
        _ => None,
    }
}

/// Parse a payload from raw bytes. Never fails. An alert parsed here never carries an
/// [`AlertToken`]; use [`parse_received`] for frames from the link.
pub fn parse(table: Table, payload: &[u8]) -> Incoming {
    match parse_known(table, payload) {
        Some((origin, message)) => Incoming {
            table,
            origin,
            message,
        },
        None => Incoming {
            table,
            origin: Origin::Other,
            message: Message::Unknown(Unknown {
                command: payload.first().copied().unwrap_or(0),
                raw: payload.to_vec(),
            }),
        },
    }
}

/// Parse a frame the link delivered. The only path that attaches an [`AlertToken`]: a
/// `99 00 <id> 01` on `DATA_MDR` gets one that records the frame's link epoch.
///
/// It takes the frame by value, so one delivered alert yields at most one token.
///
/// ```compile_fail
/// fn f(r: mdr::ReceivedFrame) {
///     let _ = mdr::parse_received(r);
///     let _ = mdr::parse_received(r);
/// }
/// ```
pub fn parse_received(frame: ReceivedFrame) -> Incoming {
    parse_frame(&frame, true)
}

/// Parse a delivered frame without ever creating a token (for the table and limits).
pub(crate) fn parse_untokened(frame: &ReceivedFrame) -> Incoming {
    parse_frame(frame, false)
}

fn parse_frame(frame: &ReceivedFrame, with_token: bool) -> Incoming {
    let table = match frame.data_type() {
        DataType::DataMdr | DataType::ShotMdr | DataType::LargeDataMdr => Table::One,
        DataType::DataMdrNo2 | DataType::ShotMdrNo2 => Table::Two,
        _ => {
            return Incoming {
                table: Table::One,
                origin: Origin::Other,
                message: Message::Unknown(Unknown {
                    command: frame.payload().first().copied().unwrap_or(0),
                    raw: frame.payload().to_vec(),
                }),
            };
        }
    };
    let mut inc = parse(table, frame.payload());
    if with_token
        && frame.data_type() == DataType::DataMdr
        && let Message::Alert(a) = &mut inc.message
        && a.action_type == 0x01
    {
        a.token = Some(AlertToken::new(a.message_id, frame.epoch()));
    }
    inc
}

/// Name of an EQ preset id (`EqPresetId`, `enums.md`), `None` when not listed here.
pub fn eq_preset_name(id: u8) -> Option<&'static str> {
    Some(match id {
        0x00 => "OFF",
        0x10 => "BRIGHT",
        0x11 => "EXCITED",
        0x12 => "MELLOW",
        0x13 => "RELAXED",
        0x14 => "VOCAL",
        0x15 => "TREBLE",
        0x16 => "BASS",
        0x17 => "SPEECH",
        0xA0 => "CUSTOM",
        0xA1 => "USER_SETTING1",
        0xA2 => "USER_SETTING2",
        _ => return None,
    })
}
