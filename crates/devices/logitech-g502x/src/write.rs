//! Volatile settings of the G502 X: current DPI (0x2201 fn3), report rate
//! (0x8060 fn2) and the host/onboard mode switch (0x8100 fn1). Nothing is written to flash; callers must have the user's
//! consent. Both calls go through `HidppSession::request_volatile`, the only
//! entry point for Set* calls.
//!
//! Flow of each call: read 0x8100 fn2 IsOnboardMode and require host mode (no
//! write otherwise), read the device's current list, validate the value with
//! the hidpp encoders, send, read the value back and compare.

use hidpp::dpi::{self, DpiList};
use hidpp::report_rate::{self, ReportRateList};
use hidpp::{EncodeError, FeatureId, ParseError, onboard};
use myhub_core::OnboardMode;
use myhub_transport::{HidLink, HidppSession, SessionError};
use thiserror::Error;

/// Wire values of 0x8100 fn1 / fn2: 0x01 is onboard, 0x02 is host (libratbag
/// HIDPP20_ONBOARD_MODE / HIDPP20_HOST_MODE, Solaar the same).
const MODE_ONBOARD: u8 = 1;
const MODE_HOST: u8 = 2;

fn mode_byte(mode: OnboardMode) -> u8 {
    match mode {
        OnboardMode::Onboard => MODE_ONBOARD,
        OnboardMode::Host => MODE_HOST,
    }
}

/// Reads 0x8100 fn2 IsOnboardMode. `None` for a byte that is neither 1 nor 2.
pub fn onboard_mode<L: HidLink>(
    session: &mut HidppSession<L>,
) -> Result<Option<OnboardMode>, SessionError> {
    let payload = session.request_feature(
        FeatureId::ONBOARD_PROFILES,
        onboard::FN_IS_ONBOARD_MODE,
        [0; 16],
    )?;
    Ok(match payload[0] {
        MODE_ONBOARD => Some(OnboardMode::Onboard),
        MODE_HOST => Some(OnboardMode::Host),
        _ => None,
    })
}

/// Switches the mouse between host and onboard mode (0x8100 fn1, volatile
/// tier, no flash), reads the mode back (fn2) and returns the verified mode.
/// A different mode read back is `VerifyFailed` with the wire bytes.
pub fn set_onboard_mode<L: HidLink>(
    session: &mut HidppSession<L>,
    mode: OnboardMode,
) -> Result<OnboardMode, WriteError> {
    let wanted = mode_byte(mode);
    let mut params = [0u8; 16];
    params[0] = wanted;
    session.request_volatile(
        FeatureId::ONBOARD_PROFILES,
        onboard::FN_SET_ONBOARD_MODE,
        params,
    )?;
    let back = session.request_feature(
        FeatureId::ONBOARD_PROFILES,
        onboard::FN_IS_ONBOARD_MODE,
        [0; 16],
    )?;
    if back[0] == wanted {
        Ok(mode)
    } else {
        Err(WriteError::VerifyFailed {
            expected: u16::from(wanted),
            got: u16::from(back[0]),
        })
    }
}

/// Why a volatile setting was not applied.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WriteError {
    /// 0x8100 fn2 did not report host mode. Nothing was written.
    #[error("the device is not in host mode")]
    NotHostMode,
    /// The DPI is not in the device's DPI list. Nothing was written.
    #[error("DPI {0} is outside the device's DPI list")]
    DpiOutOfRange(u16),
    /// The rate is not one the device offers. Nothing was written.
    #[error("report rate {0} Hz is not supported by the device")]
    RateOutOfRange(u16),
    /// The value read back differs from the requested one.
    #[error("verification failed: expected {expected}, device reports {got}")]
    VerifyFailed { expected: u16, got: u16 },
    /// A parameter could not be built for another reason.
    #[error("could not build the request: {0}")]
    Encode(EncodeError),
    #[error("could not parse a device reply: {0}")]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Session(#[from] SessionError),
}

/// Requires host mode: 0x8100 fn2 byte 0 == 2. A device error here is an
/// error of the call; nothing is written before this check passes.
fn require_host_mode<L: HidLink>(session: &mut HidppSession<L>) -> Result<(), WriteError> {
    let payload = session.request_feature(
        FeatureId::ONBOARD_PROFILES,
        onboard::FN_IS_ONBOARD_MODE,
        [0; 16],
    )?;
    if payload[0] == MODE_HOST {
        Ok(())
    } else {
        Err(WriteError::NotHostMode)
    }
}

fn feature_version<L: HidLink>(
    session: &HidppSession<L>,
    feature: FeatureId,
) -> Result<u8, WriteError> {
    session
        .table()
        .by_id(feature)
        .map(|e| e.version)
        .ok_or(WriteError::Session(SessionError::FeatureAbsent(feature)))
}

/// Sets the current DPI of sensor 0 and returns the verified value.
pub fn set_dpi<L: HidLink>(session: &mut HidppSession<L>, dpi: u16) -> Result<u16, WriteError> {
    require_host_mode(session)?;
    let feature = FeatureId::ADJUSTABLE_DPI;
    let version = feature_version(session, feature)?;
    let list =
        session.request_feature(feature, dpi::FN_GET_SENSOR_DPI_LIST, dpi::sensor_params(0))?;
    let list = DpiList::parse(&list)?;
    // dpiLevel = 0: BRIEF [DER] line. The G HUB caller of 0x2201 v2 SetSensorDPI
    // is not in the decompile; 0 equals the zero padding v1-style tools send.
    let params =
        dpi::set_sensor_dpi_params(0, dpi, Some(0), version, &list).map_err(|e| match e {
            EncodeError::DpiNotSupported { dpi } => WriteError::DpiOutOfRange(dpi),
            other => WriteError::Encode(other),
        })?;
    session.request_volatile(feature, dpi::FN_SET_SENSOR_DPI, params)?;
    let back = session.request_feature(feature, dpi::FN_GET_SENSOR_DPI, dpi::sensor_params(0))?;
    let got = dpi::parse_sensor_dpi(&back)?.current;
    if got == dpi {
        Ok(got)
    } else {
        Err(WriteError::VerifyFailed { expected: dpi, got })
    }
}

/// Sets the report rate (one of the device's rates, in Hz) and returns the
/// verified rate in Hz.
pub fn set_report_rate<L: HidLink>(
    session: &mut HidppSession<L>,
    hz: u16,
) -> Result<u16, WriteError> {
    require_host_mode(session)?;
    let feature = FeatureId::REPORT_RATE;
    let list = session.request_feature(feature, report_rate::FN_GET_REPORT_RATE_LIST, [0; 16])?;
    let list = ReportRateList::parse(&list)?;
    let interval = list
        .intervals_ms()
        .into_iter()
        .find(|&ms| report_rate::interval_to_hz(ms) == hz)
        .ok_or(WriteError::RateOutOfRange(hz))?;
    let params = report_rate::set_report_rate_params(interval, &list).map_err(|e| match e {
        EncodeError::RateNotSupported { .. } => WriteError::RateOutOfRange(hz),
        other => WriteError::Encode(other),
    })?;
    session.request_volatile(feature, report_rate::FN_SET_REPORT_RATE, params)?;
    let back = session.request_feature(feature, report_rate::FN_GET_REPORT_RATE, [0; 16])?;
    let got = report_rate::interval_to_hz(report_rate::parse_current_interval_ms(&back)?);
    if got == hz {
        Ok(got)
    } else {
        Err(WriteError::VerifyFailed { expected: hz, got })
    }
}
