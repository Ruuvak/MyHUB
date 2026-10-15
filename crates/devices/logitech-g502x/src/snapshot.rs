//! Read-only snapshot of the G502 X: features, DPI, report rate, battery and
//! the onboard profile memory. Only read-tier calls are made.

use std::collections::BTreeMap;
use std::time::Instant;

use hidpp::battery::{self, BatteryStatus};
use hidpp::dpi::{self, DpiList};
use hidpp::onboard::{self, Directory, OnboardInfo, ProfileSector, SectorAssembler};
use hidpp::policy::read_sector_allowed;
use hidpp::report_rate::{self, ReportRateList};
use hidpp::{FeatureId, ParseError, PolicyError};
use myhub_transport::{HidLink, HidppSession, SessionError};
use serde::Serialize;
use thiserror::Error;

/// Sanity bound for the sector size the device reports (the reference probe
/// used the same bound).
const MAX_SECTOR_SIZE: u16 = 4096;

/// Why a snapshot could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReadError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("could not parse a device reply: {0}")]
    Parse(#[from] ParseError),
    /// The sector plan contains a sector the policy never allows to be read.
    /// Detected before any ReadData request is sent.
    #[error("sector plan refused: {0}")]
    SectorPlan(#[from] PolicyError),
    /// A field of the onboard info reply is outside what this driver accepts.
    /// Detected before any ReadData is sent.
    #[error("onboard info field {field} = {value} exceeds the limit {limit}")]
    InfoOutOfBounds {
        field: &'static str,
        value: u16,
        limit: u16,
    },
    #[error("device reports an implausible sector size {0}")]
    ImplausibleSectorSize(u16),
}

/// One row of the feature table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureRow {
    pub index: u8,
    /// Lowercase hex, `0x1d4b`.
    pub id: String,
    pub feature_type: u8,
    pub version: u8,
}

/// Adjustable DPI (0x2201), sensor 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DpiSnapshot {
    pub sensor_count: u8,
    pub list: DpiList,
    pub current: u16,
    pub default: u16,
}

/// Report rate (0x8060).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RateSnapshot {
    /// Supported rates, fastest first.
    pub rates_hz: Vec<u16>,
    pub current_interval_ms: u8,
    pub current_hz: u16,
}

/// Unified battery (0x1004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BatterySnapshot {
    pub percent: u8,
    /// The whole 16 byte reply payload, hex.
    pub raw_hex: String,
    pub status: BatteryStatus,
}

/// A sector parsed as a layout A profile, or why it was not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedProfile {
    /// `0x0001` style key, same as in [`OnboardSnapshot::sectors`].
    pub sector: String,
    /// `true` for factory (ROM) sectors 0x0101 and up.
    pub rom: bool,
    pub profile: Option<ProfileSector>,
    pub error: Option<String>,
}

/// Onboard profiles (0x8100).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OnboardSnapshot {
    pub info: OnboardInfo,
    /// First 4 bytes of the IsOnboardMode reply, hex. Not yet exercised on the
    /// device; the meaning of the bytes is unverified. `None` on a device
    /// error or timeout, see `mode_error`.
    pub mode_raw_hex: Option<String>,
    pub mode_error: Option<String>,
    /// First 4 bytes of the GetActiveProfile reply, hex. Same caveats.
    pub active_profile_raw_hex: Option<String>,
    pub active_profile_error: Option<String>,
    pub directory: Directory,
    /// Set when profile parsing was refused (format other than 3). The raw
    /// sector bytes are still in `sectors`.
    pub profiles_skipped: Option<String>,
    pub profiles: Vec<ParsedProfile>,
    /// Raw sector bytes as lowercase hex, keyed `0x0000`, `0x0001`, ...
    pub sectors: BTreeMap<String, String>,
}

/// Requests and time spent in one step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepStat {
    pub step: &'static str,
    pub requests: u64,
    pub elapsed_ms: u64,
}

/// Everything `read_snapshot` reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    /// `false` when the link could not see short (0x10) reports, so a HID++
    /// 1.0 connection notification may have gone unnoticed during the read.
    pub short_reports_available: bool,
    pub features: Vec<FeatureRow>,
    pub dpi: DpiSnapshot,
    pub report_rate: RateSnapshot,
    pub battery: BatterySnapshot,
    pub onboard: OnboardSnapshot,
    pub steps: Vec<StepStat>,
    pub total_requests: u64,
    pub total_elapsed_ms: u64,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn sector_key(sector: u16) -> String {
    format!("0x{sector:04x}")
}

/// Runs one step and records its request count and elapsed time.
fn step<L: HidLink, T>(
    session: &mut HidppSession<L>,
    steps: &mut Vec<StepStat>,
    name: &'static str,
    body: impl FnOnce(&mut HidppSession<L>) -> Result<T, ReadError>,
) -> Result<T, ReadError> {
    let requests_before = session.request_count();
    let started = Instant::now();
    let out = body(session)?;
    steps.push(StepStat {
        step: name,
        requests: session.request_count() - requests_before,
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
    Ok(out)
}

fn read<L: HidLink>(
    session: &mut HidppSession<L>,
    feature: FeatureId,
    function: u8,
    params: [u8; 16],
) -> Result<[u8; 16], ReadError> {
    Ok(session.request_feature(feature, function, params)?)
}

/// An error that is allowed for a call that has never been exercised on the
/// device: a device error report or a missing reply. Link, policy and table
/// errors stay fatal.
fn soft_read<L: HidLink>(
    session: &mut HidppSession<L>,
    feature: FeatureId,
    function: u8,
) -> Result<(Option<String>, Option<String>), ReadError> {
    match session.request_feature(feature, function, [0; 16]) {
        Ok(payload) => Ok((Some(hex(&payload[..4])), None)),
        Err(e @ (SessionError::Device { .. } | SessionError::Timeout)) => {
            Ok((None, Some(e.to_string())))
        }
        Err(e) => Err(e.into()),
    }
}

fn read_sector<L: HidLink>(
    session: &mut HidppSession<L>,
    sector: u16,
    sector_size: u16,
) -> Result<Vec<u8>, ReadError> {
    let mut assembler = SectorAssembler::new(sector_size)?;
    for offset in onboard::chunk_plan(sector_size)? {
        let payload = read(
            session,
            FeatureId::ONBOARD_PROFILES,
            onboard::FN_READ_DATA,
            onboard::read_data_params(sector, offset),
        )?;
        assembler.push_chunk(offset, &payload)?;
    }
    Ok(assembler.finish()?)
}

/// Reads the full snapshot. Order: discover; 0x2201 fn0, fn1, fn2; 0x8060 fn0,
/// fn1; 0x1004 fn1; 0x8100 fn0, fn2, fn4; ReadData of sector 0x0000, user
/// sectors `1..=profile_count`, ROM sectors `0x0101..` (`rom_profile_count`).
///
/// Failures are fatal except fn2 and fn4 of 0x8100 (never exercised on the
/// device; a device error or timeout is stored in the snapshot) and profile
/// parsing (stored per sector). Any sector in the plan outside the policy
/// bounds is refused before the first ReadData is sent.
pub fn read_snapshot<L: HidLink>(session: &mut HidppSession<L>) -> Result<Snapshot, ReadError> {
    let started = Instant::now();
    let mut steps = Vec::new();

    step(session, &mut steps, "discover", |s| Ok(s.discover()?))?;
    let features = session
        .table()
        .entries()
        .iter()
        .map(|e| FeatureRow {
            index: e.index,
            id: format!("0x{:04x}", e.id.0),
            feature_type: e.feature_type,
            version: e.version,
        })
        .collect();

    let dpi = step(session, &mut steps, "dpi", |s| {
        let f = FeatureId::ADJUSTABLE_DPI;
        let count = read(s, f, dpi::FN_GET_SENSOR_COUNT, [0; 16])?;
        let sensor_count = dpi::parse_sensor_count(&count)?;
        let list = read(s, f, dpi::FN_GET_SENSOR_DPI_LIST, dpi::sensor_params(0))?;
        let list = DpiList::parse(&list)?;
        let current = read(s, f, dpi::FN_GET_SENSOR_DPI, dpi::sensor_params(0))?;
        let current = dpi::parse_sensor_dpi(&current)?;
        Ok(DpiSnapshot {
            sensor_count,
            list,
            current: current.current,
            default: current.default,
        })
    })?;

    let report_rate = step(session, &mut steps, "report_rate", |s| {
        let f = FeatureId::REPORT_RATE;
        let list = read(s, f, report_rate::FN_GET_REPORT_RATE_LIST, [0; 16])?;
        let list = ReportRateList::parse(&list)?;
        let current = read(s, f, report_rate::FN_GET_REPORT_RATE, [0; 16])?;
        let interval = report_rate::parse_current_interval_ms(&current)?;
        Ok(RateSnapshot {
            rates_hz: list.rates_hz(),
            current_interval_ms: interval,
            current_hz: report_rate::interval_to_hz(interval),
        })
    })?;

    let battery = step(session, &mut steps, "battery", |s| {
        let payload = read(
            s,
            FeatureId::UNIFIED_BATTERY,
            battery::FN_GET_STATUS,
            [0; 16],
        )?;
        let status = battery::parse_status(&payload)?;
        Ok(BatterySnapshot {
            percent: status.percent,
            raw_hex: hex(&payload),
            status,
        })
    })?;

    let onboard_feature = FeatureId::ONBOARD_PROFILES;
    let info = step(session, &mut steps, "onboard_info", |s| {
        let payload = read(s, onboard_feature, onboard::FN_GET_INFO, [0; 16])?;
        Ok(OnboardInfo::parse(&payload)?)
    })?;
    // Bounds on what the device claims, checked before any ReadData: layout A
    // has 16 profile slots and the policy allows ROM sectors 0x0101..=0x0105.
    for (field, value, limit) in [
        ("profile_count", info.profile_count, 16),
        (
            "profile_count vs sector_count",
            info.profile_count,
            info.sector_count,
        ),
        ("rom_profile_count", info.rom_profile_count, 5),
    ] {
        if value > limit {
            return Err(ReadError::InfoOutOfBounds {
                field,
                value: u16::from(value),
                limit: u16::from(limit),
            });
        }
    }
    let (mode_raw_hex, mode_error) = step(session, &mut steps, "onboard_mode", |s| {
        soft_read(s, onboard_feature, onboard::FN_IS_ONBOARD_MODE)
    })?;
    let (active_profile_raw_hex, active_profile_error) =
        step(session, &mut steps, "onboard_active_profile", |s| {
            soft_read(s, onboard_feature, onboard::FN_GET_ACTIVE_PROFILE)
        })?;

    if info.sector_size > MAX_SECTOR_SIZE {
        return Err(ReadError::ImplausibleSectorSize(info.sector_size));
    }
    // Directory, user sectors, then ROM sectors; every one checked against the
    // policy bounds before the first ReadData goes out.
    let mut plan = vec![onboard::DIRECTORY_SECTOR];
    plan.extend((1..=u16::from(info.profile_count)).collect::<Vec<_>>());
    plan.extend((0..u16::from(info.rom_profile_count)).map(|n| 0x0101 + n));
    if let Some(&sector) = plan.iter().find(|&&s| !read_sector_allowed(s)) {
        return Err(PolicyError::SectorNotAllowed { sector }.into());
    }

    let sector_data = step(session, &mut steps, "onboard_sectors", |s| {
        let mut data = Vec::with_capacity(plan.len());
        for &sector in &plan {
            data.push((sector, read_sector(s, sector, info.sector_size)?));
        }
        Ok(data)
    })?;

    let directory = Directory::parse(
        sector_data
            .iter()
            .find(|(sector, _)| *sector == onboard::DIRECTORY_SECTOR)
            .map(|(_, bytes)| bytes.as_slice())
            .unwrap_or_default(),
    )?;

    let profiles_skipped = (info.profile_format != onboard::SUPPORTED_PROFILE_FORMAT).then(|| {
        format!(
            "profile format {} is not {}: profiles not parsed, raw sectors kept",
            info.profile_format,
            onboard::SUPPORTED_PROFILE_FORMAT
        )
    });
    let profiles = if profiles_skipped.is_some() {
        Vec::new()
    } else {
        sector_data
            .iter()
            .filter(|(sector, _)| *sector != onboard::DIRECTORY_SECTOR)
            .map(|(sector, bytes)| {
                let parsed = ProfileSector::parse(bytes, &info);
                ParsedProfile {
                    sector: sector_key(*sector),
                    rom: *sector >= 0x0101,
                    error: parsed.as_ref().err().map(ToString::to_string),
                    profile: parsed.ok(),
                }
            })
            .collect()
    };
    let sectors = sector_data
        .iter()
        .map(|(sector, bytes)| (sector_key(*sector), hex(bytes)))
        .collect();

    Ok(Snapshot {
        short_reports_available: session.short_reports_available(),
        features,
        dpi,
        report_rate,
        battery,
        onboard: OnboardSnapshot {
            info,
            mode_raw_hex,
            mode_error,
            active_profile_raw_hex,
            active_profile_error,
            directory,
            profiles_skipped,
            profiles,
            sectors,
        },
        total_requests: steps.iter().map(|s| s.requests).sum(),
        steps,
        total_elapsed_ms: started.elapsed().as_millis() as u64,
    })
}
