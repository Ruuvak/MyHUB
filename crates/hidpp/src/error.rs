//! Shared error types for payload parsers and parameter encoders.

use thiserror::Error;

/// A payload could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    /// The payload is shorter than the parser needs.
    #[error("payload too short: need {need} bytes, got {got}")]
    TooShort { need: usize, got: usize },
    /// A field holds a value outside its documented domain.
    #[error("invalid value {value:#x} for {field}")]
    InvalidValue { field: &'static str, value: u32 },
    /// The DPI list is structurally broken.
    #[error("malformed DPI list: {0}")]
    MalformedDpiList(&'static str),
    /// Only profile format 3 (layout A) is verified on the device.
    #[error("unsupported profile format {0}: only format 3 (layout A) is supported")]
    UnsupportedProfileFormat(u8),
    /// Sector bytes do not match the sector size reported by the device.
    #[error("sector length {got} does not match sector size {expected}")]
    SectorLength { expected: usize, got: usize },
    /// Sector size cannot hold the layout.
    #[error("sector size {size} is too small (need at least {need})")]
    SectorTooSmall { size: usize, need: usize },
    /// A chunk offset does not fit inside the sector.
    #[error("chunk offset {offset} out of range for sector size {size}")]
    ChunkOutOfRange { offset: usize, size: usize },
    /// Not every byte of the sector was read.
    #[error("sector incomplete: {missing} bytes not yet read")]
    SectorIncomplete { missing: usize },
}

/// Request parameters could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EncodeError {
    /// The DPI is not part of the sensor's DPI list.
    #[error("DPI {dpi} is not in the sensor's DPI list")]
    DpiNotSupported { dpi: u16 },
    /// A DPI level byte was given, but the feature version has no such field.
    #[error("dpiLevel needs feature version >= 2, device reports {version}")]
    DpiLevelNeedsV2 { version: u8 },
    /// Feature version >= 2 has a dpiLevel byte and it must be given explicitly.
    #[error("dpiLevel must be given explicitly for feature version {version}")]
    DpiLevelRequired { version: u8 },
    /// The sensor index does not match the DPI list used for validation.
    #[error("sensor {sensor} does not match the DPI list for sensor {list_sensor}")]
    SensorMismatch { sensor: u8, list_sensor: u8 },
    /// The report interval is not in the device's report rate list.
    #[error("report interval {interval_ms} ms is not supported by the device")]
    RateNotSupported { interval_ms: u8 },
}
