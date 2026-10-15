//! Onboard profile sector patching and write frame parameters (0x8100 fn 6, 7, 8).
//!
//! Pure functions, no I/O. [`apply_patch`] lays the editable fields of a
//! layout A profile (report rate, default and shift DPI index, five DPI levels)
//! over the raw 255 bytes read from the device and recomputes the CRC. Every
//! other byte stays exactly as read. The frame parameter builders only produce
//! the 16 parameter bytes; the bytes reach the wire only through
//! [`crate::policy::Policy`], and only behind its onboard write gate.

use thiserror::Error;

use crate::crc;
use crate::dpi::DpiList;
use crate::error::ParseError;
use crate::onboard::{DIRECTORY_SECTOR, OnboardInfo, ProfileSector, layout_a};

/// 0x8100 BeginWrite function number.
pub const FN_BEGIN_WRITE: u8 = 6;
/// 0x8100 WriteData function number.
pub const FN_WRITE_DATA: u8 = 7;
/// 0x8100 SetCurrentProfile function number (profile select, own policy gate).
/// Parameters: sector BE16 in bytes 0..1, the rest zero (REA `hidpp_spec.md`
/// line 38: "[0..1] profile/sector"; libratbag sends index+1, which equals the
/// user sector number).
pub const FN_SET_CURRENT_PROFILE: u8 = 3;
/// Profile sectors that may be written and selected by name: user sectors 3..=5
/// are writable (profile content), see `policy::WRITE_SECTOR_ALLOWLIST`.
pub const PROFILE_SECTORS: [u16; 3] = [0x0003, 0x0004, 0x0005];
/// Sectors SetCurrentProfile may select: user sectors 1..=5.
pub const SELECTABLE_SECTORS: std::ops::RangeInclusive<u16> = 1..=5;
/// 0x8100 EndWrite function number.
pub const FN_END_WRITE: u8 = 8;

/// The only sector size that can be written (layout A, whole sector).
pub const SECTOR_LEN: usize = 255;
/// Bytes per WriteData frame.
pub const WRITE_CHUNK_LEN: usize = 16;
/// WriteData frames per sector: `ceil(255 / 16)`.
pub const WRITE_CHUNK_COUNT: usize = 16;
/// Filler for the unused last byte of the final chunk. \[UNK\] The vendor
/// software pads from a constant template whose value is not in the notes; the
/// sector is 255 bytes long, so the device is expected to ignore byte 256.
pub const WRITE_PAD_BYTE: u8 = 0x00;
/// Report intervals (ms) that may be stored: 1000, 500, 250, 125 Hz.
pub const ALLOWED_RATE_INTERVALS_MS: [u8; 4] = [1, 2, 4, 8];

/// A finished 255 byte sector image bound to the sector it is for. It can only
/// be made inside this module: by [`apply_patch`] (also after [`clone_profile`]),
/// [`patch_directory`] and the validated restore constructors
/// ([`restore_profile_image`], [`restore_directory_image`],
/// [`restore_profile_image`], [`restore_directory_image`]) and the rollback
/// image that [`apply_patch`] and [`patch_directory`] return with the new one.
/// The transport writes nothing but a `SectorImage`, so
/// no caller can send arbitrary bytes to arbitrary sectors.
///
/// The fields and the constructor are private, which these doctests pin:
///
/// ```
/// fn sector_of(i: &hidpp::onboard_write::SectorImage) -> u16 { i.sector() }
/// ```
///
/// ```compile_fail
/// let _ = hidpp::onboard_write::SectorImage { sector: 3, bytes: [0; 255] };
/// ```
///
/// ```compile_fail
/// let _ = hidpp::onboard_write::SectorImage::new(3, [0; 255]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectorImage {
    sector: u16,
    bytes: [u8; SECTOR_LEN],
}

/// The new image and the image that undoes it, both made from the target's
/// own bytes and bound to the same sector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchedImages {
    /// What to write.
    pub new: SectorImage,
    /// What was there, to write back if the write fails.
    pub rollback: SectorImage,
}

/// A user sector that SetCurrentProfile may name (1..=5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectableSector(u16);

impl SelectableSector {
    pub fn new(sector: u16) -> Result<Self, WriteError> {
        if SELECTABLE_SECTORS.contains(&sector) {
            Ok(Self(sector))
        } else {
            Err(WriteError::SelectSectorNotAllowed(sector))
        }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl SectorImage {
    fn new(sector: u16, bytes: [u8; SECTOR_LEN]) -> Self {
        Self { sector, bytes }
    }

    /// The sector this image is for.
    pub fn sector(&self) -> u16 {
        self.sector
    }

    /// The 255 image bytes (CRC included).
    pub fn bytes(&self) -> &[u8; SECTOR_LEN] {
        &self.bytes
    }
}

/// Fields of a layout A profile that MyHUB may change. `None` keeps the byte(s)
/// as read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProfilePatch {
    /// Report interval in ms: 1, 2, 4 or 8.
    pub report_rate_interval_ms: Option<u8>,
    /// Default DPI level index, 0..=4.
    pub default_dpi_index: Option<u8>,
    /// Shift DPI level index, 0..=4.
    pub shift_dpi_index: Option<u8>,
    /// Five DPI levels, each non-zero and in the sensor's 0x2201 list.
    pub dpi_levels: Option<[u16; 5]>,
}

/// A patch or a write frame was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WriteError {
    #[error("sector length {got} is not {SECTOR_LEN}")]
    SectorLength { got: usize },
    /// Profile format != 3, or the info does not match the sector.
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("input sector CRC is invalid (stored {stored:#06x}, computed {computed:#06x})")]
    InputCrcInvalid { stored: u16, computed: u16 },
    #[error("report interval {0} ms is not one of 1, 2, 4, 8")]
    RateNotAllowed(u8),
    #[error("{field} index {value} is above 4")]
    DpiIndexOutOfRange { field: &'static str, value: u8 },
    #[error("{field} index {index} points at a DPI level that is 0 (or not a level)")]
    DpiLevelZero { field: &'static str, index: u8 },
    #[error("DPI {0} is not allowed (zero or not in the sensor's DPI list)")]
    DpiNotAllowed(u16),
    #[error("directory sector {0:#06x} is not one of the profile sectors 3, 4, 5")]
    DirectorySectorNotAllowed(u16),
    #[error("the directory has no entry for sector {0:#06x}")]
    DirectoryNoEntry(u16),
    #[error("sector {0:#06x} is not a profile sector 3, 4, 5")]
    ProfileSectorNotAllowed(u16),
    #[error("image is not a plausible profile: {0}")]
    ImplausibleProfile(&'static str),
    #[error("directory image does not fit the current directory: {0}")]
    DirectoryMismatch(&'static str),
    #[error("directory image is malformed: {0}")]
    DirectoryMalformed(&'static str),
    #[error("the active profile {0:#06x} would end up disabled")]
    ActiveProfileDisabled(u16),
    #[error("no profile would be enabled")]
    NoProfileEnabled,
    #[error("profile {0:#06x} cannot be selected (only sectors 1..=5)")]
    SelectSectorNotAllowed(u16),
    #[error("BeginWrite offset must be 0, got {0}")]
    OffsetNotZero(u16),
    #[error("BeginWrite length must be {SECTOR_LEN}, got {0}")]
    LengthNotSector(u16),
}

/// Applies `patch` over the raw sector bytes `sector` (read from the device)
/// and returns the new image for profile sector `target` (3, 4 or 5) with a
/// fresh CRC.
///
/// Refuses (before any byte is produced) when the sector is not 255 bytes, the
/// profile format is not 3, the input CRC is wrong, the rate interval is not in
/// {1, 2, 4, 8}, an index is above 4, a DPI is 0 or not in `dpi_list`, or the
/// level at the resulting default or shift index (patched or as read) is 0.
/// Only offsets 0x00, 0x01, 0x02 and 0x03..0x0D change, plus the CRC at
/// 0xFD..0xFE.
pub fn apply_patch(
    target: u16,
    current: &[u8],
    info: &OnboardInfo,
    dpi_list: &DpiList,
    patch: &ProfilePatch,
) -> Result<PatchedImages, WriteError> {
    apply_patch_over(target, current, current, info, dpi_list, patch)
}

/// Like [`apply_patch`], but patches `base` (for example a copy of another
/// profile from [`clone_profile`]) instead of the target's own bytes `current`.
/// The rollback image is always built from `current`.
pub fn apply_patch_over(
    target: u16,
    current: &[u8],
    sector: &[u8],
    info: &OnboardInfo,
    dpi_list: &DpiList,
    patch: &ProfilePatch,
) -> Result<PatchedImages, WriteError> {
    use layout_a as a;

    if !PROFILE_SECTORS.contains(&target) {
        return Err(WriteError::ProfileSectorNotAllowed(target));
    }
    let rollback = original_image(target, current, info)?;
    if sector.len() != SECTOR_LEN {
        return Err(WriteError::SectorLength { got: sector.len() });
    }
    let parsed = ProfileSector::parse(sector, info)?;
    if !parsed.crc_ok {
        return Err(WriteError::InputCrcInvalid {
            stored: parsed.stored_crc,
            computed: parsed.computed_crc,
        });
    }

    if let Some(ms) = patch.report_rate_interval_ms
        && !ALLOWED_RATE_INTERVALS_MS.contains(&ms)
    {
        return Err(WriteError::RateNotAllowed(ms));
    }
    for (field, value) in [
        ("default DPI", patch.default_dpi_index),
        ("shift DPI", patch.shift_dpi_index),
    ] {
        if let Some(value) = value
            && usize::from(value) >= a::DPI_LEVEL_COUNT
        {
            return Err(WriteError::DpiIndexOutOfRange { field, value });
        }
    }
    if let Some(levels) = patch.dpi_levels {
        for dpi in levels {
            if dpi == 0 || !dpi_list.contains(dpi) {
                return Err(WriteError::DpiNotAllowed(dpi));
            }
        }
    }

    // The levels the image will hold, and the levels that the default and shift
    // indexes (patched or as read) select, must be non-zero.
    let levels = patch.dpi_levels.unwrap_or(parsed.dpi_levels);
    for (field, index) in [
        (
            "default DPI",
            patch.default_dpi_index.unwrap_or(parsed.default_dpi_index),
        ),
        (
            "shift DPI",
            patch.shift_dpi_index.unwrap_or(parsed.shift_dpi_index),
        ),
    ] {
        if levels.get(usize::from(index)).is_none_or(|&l| l == 0) {
            return Err(WriteError::DpiLevelZero { field, index });
        }
    }

    let mut image = [0u8; SECTOR_LEN];
    image.copy_from_slice(sector);
    if let Some(ms) = patch.report_rate_interval_ms {
        image[a::REPORT_RATE] = ms;
    }
    if let Some(i) = patch.default_dpi_index {
        image[a::DEFAULT_DPI_INDEX] = i;
    }
    if let Some(i) = patch.shift_dpi_index {
        image[a::SHIFT_DPI_INDEX] = i;
    }
    if let Some(levels) = patch.dpi_levels {
        for (n, dpi) in levels.into_iter().enumerate() {
            let o = a::DPI_LEVELS + n * 2;
            image[o..o + 2].copy_from_slice(&dpi.to_le_bytes());
        }
    }
    let crc = crc::crc16_ccitt_false(&image[..SECTOR_LEN - 2]);
    image[SECTOR_LEN - 2..].copy_from_slice(&crc.to_be_bytes());
    Ok(PatchedImages {
        new: SectorImage::new(target, image),
        rollback,
    })
}

/// BeginWrite (fn 6) parameters: sector BE16, offset BE16 (must be 0), length
/// BE16 (must be 255).
pub fn begin_write_params(sector: u16, offset: u16, length: u16) -> Result<[u8; 16], WriteError> {
    if offset != 0 {
        return Err(WriteError::OffsetNotZero(offset));
    }
    if usize::from(length) != SECTOR_LEN {
        return Err(WriteError::LengthNotSector(length));
    }
    let mut params = [0u8; 16];
    params[..2].copy_from_slice(&sector.to_be_bytes());
    params[2..4].copy_from_slice(&offset.to_be_bytes());
    params[4..6].copy_from_slice(&length.to_be_bytes());
    Ok(params)
}

/// The 16 WriteData (fn 7) frames of a sector image. The last chunk holds 15
/// image bytes and one [`WRITE_PAD_BYTE`].
pub fn write_chunks(image: &[u8; SECTOR_LEN]) -> [[u8; WRITE_CHUNK_LEN]; WRITE_CHUNK_COUNT] {
    let mut out = [[WRITE_PAD_BYTE; WRITE_CHUNK_LEN]; WRITE_CHUNK_COUNT];
    for (chunk, src) in out.iter_mut().zip(image.chunks(WRITE_CHUNK_LEN)) {
        chunk[..src.len()].copy_from_slice(src);
    }
    out
}

/// EndWrite (fn 8) parameters: none.
pub fn end_write_params() -> [u8; 16] {
    [0u8; 16]
}

/// One directory entry located in the raw sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirEntry {
    /// Offset of the 4-byte entry.
    offset: usize,
    sector: u16,
    enabled: bool,
}

/// Entries of a directory image: 4-byte `{sector BE16, enabled, 0xFF}` up to the
/// 0xFFFF terminator. Byte 3 is not interpreted (live value 0xFF, libratbag
/// writes 0x00), so every byte outside the flag is kept as read.
fn directory_entries(raw: &[u8; SECTOR_LEN]) -> Result<Vec<DirEntry>, WriteError> {
    let mut entries = Vec::new();
    // The last two bytes are the CRC, so only whole entries before them count.
    for i in 0..(SECTOR_LEN - 2) / 4 {
        let o = i * 4;
        let sector = u16::from_be_bytes([raw[o], raw[o + 1]]);
        if sector == 0xFFFF {
            if entries.is_empty() {
                return Err(WriteError::DirectoryMalformed("no entries"));
            }
            return Ok(entries);
        }
        if sector == 0 || sector >= 0x0100 {
            return Err(WriteError::DirectoryMalformed("entry is not a user sector"));
        }
        if raw[o + 2] > 1 {
            return Err(WriteError::DirectoryMalformed("enabled flag is not 0 or 1"));
        }
        if entries.iter().any(|e: &DirEntry| e.sector == sector) {
            return Err(WriteError::DirectoryMalformed("duplicate sector entry"));
        }
        entries.push(DirEntry {
            offset: o,
            sector,
            enabled: raw[o + 2] == 1,
        });
    }
    Err(WriteError::DirectoryMalformed("no 0xFFFF terminator"))
}

fn check_input_crc(raw: &[u8; SECTOR_LEN]) -> Result<(), WriteError> {
    let stored = crc::sector_crc_stored(raw).unwrap_or(0);
    let computed = crc::sector_crc_computed(raw).unwrap_or(0);
    if stored == computed {
        Ok(())
    } else {
        Err(WriteError::InputCrcInvalid { stored, computed })
    }
}

/// Checks a directory image (sector 0x0000): CRC valid, well-formed entries,
/// terminator, at least one enabled profile. Used before restoring one.
pub fn validate_directory_image(raw: &[u8]) -> Result<(), WriteError> {
    let raw: &[u8; SECTOR_LEN] = raw
        .try_into()
        .map_err(|_| WriteError::SectorLength { got: raw.len() })?;
    check_input_crc(raw)?;
    if !directory_entries(raw)?.iter().any(|e| e.enabled) {
        return Err(WriteError::NoProfileEnabled);
    }
    Ok(())
}

/// Enables or disables the directory entry of `sector` (3, 4 or 5) by changing
/// byte 2 of that entry only (0x01 or 0x00) and recomputing the CRC. Everything
/// else stays as read.
///
/// Refuses when the sector is not 3, 4 or 5, the directory has no entry for it,
/// the input CRC is bad or the directory is malformed, when the result would
/// leave the active profile `active` (fn4 sector) disabled, or when no profile
/// would be enabled at all.
pub fn patch_directory(
    raw: &[u8; SECTOR_LEN],
    sector: u16,
    enabled: bool,
    active: u16,
) -> Result<PatchedImages, WriteError> {
    if !PROFILE_SECTORS.contains(&sector) {
        return Err(WriteError::DirectorySectorNotAllowed(sector));
    }
    check_input_crc(raw)?;
    let entries = directory_entries(raw)?;
    let entry = entries
        .iter()
        .find(|e| e.sector == sector)
        .ok_or(WriteError::DirectoryNoEntry(sector))?;
    let mut image = *raw;
    image[entry.offset + 2] = u8::from(enabled);
    let after = directory_entries(&image)?;
    if after.iter().any(|e| e.sector == active && !e.enabled) {
        return Err(WriteError::ActiveProfileDisabled(active));
    }
    if !after.iter().any(|e| e.enabled) {
        return Err(WriteError::NoProfileEnabled);
    }
    let crc = crc::crc16_ccitt_false(&image[..SECTOR_LEN - 2]);
    image[SECTOR_LEN - 2..].copy_from_slice(&crc.to_be_bytes());
    Ok(PatchedImages {
        new: SectorImage::new(DIRECTORY_SECTOR, image),
        rollback: SectorImage::new(DIRECTORY_SECTOR, *raw),
    })
}

/// A copy of the profile sector `src_raw` with its CRC, ready for
/// [`apply_patch`]. Both the source and the result must parse as format 3 with
/// a valid CRC. Layout A holds no byte with the sector's own address (sectors
/// 0x0003, 0x0004 and 0x0005 are byte-identical on the device and in the
/// fixture, which they could not be if one did), so the copy is the source
/// bytes unchanged.
pub fn clone_profile(
    src_raw: &[u8; SECTOR_LEN],
    info: &OnboardInfo,
) -> Result<[u8; SECTOR_LEN], WriteError> {
    let src = ProfileSector::parse(src_raw, info)?;
    if !src.crc_ok {
        return Err(WriteError::InputCrcInvalid {
            stored: src.stored_crc,
            computed: src.computed_crc,
        });
    }
    let mut image = *src_raw;
    let crc = crc::crc16_ccitt_false(&image[..SECTOR_LEN - 2]);
    image[SECTOR_LEN - 2..].copy_from_slice(&crc.to_be_bytes());
    let out = ProfileSector::parse(&image, info)?;
    if !out.crc_ok {
        return Err(WriteError::InputCrcInvalid {
            stored: out.stored_crc,
            computed: out.computed_crc,
        });
    }
    Ok(image)
}

/// SetCurrentProfile (fn 3) parameters: sector BE16, the rest zero. Only user
/// sectors 1..=5.
pub fn select_profile_params(sector: u16) -> Result<[u8; 16], WriteError> {
    if !SELECTABLE_SECTORS.contains(&sector) {
        return Err(WriteError::SelectSectorNotAllowed(sector));
    }
    let mut params = [0u8; 16];
    params[..2].copy_from_slice(&sector.to_be_bytes());
    Ok(params)
}

/// Image of the bytes read from `target` itself (the rollback of a patch):
/// sector 3, 4 or 5 or the directory 0x0000. The bytes must have a valid CRC
/// and parse (format 3 and not shaped like a directory for a profile; entries,
/// terminator and one enabled profile for the directory). No check of the
/// values: a rollback must reproduce the bytes the device had.
fn original_image(
    target: u16,
    bytes: &[u8],
    info: &OnboardInfo,
) -> Result<SectorImage, WriteError> {
    let raw: [u8; SECTOR_LEN] = bytes
        .try_into()
        .map_err(|_| WriteError::SectorLength { got: bytes.len() })?;
    if target == DIRECTORY_SECTOR {
        validate_directory_image(&raw)?;
    } else if PROFILE_SECTORS.contains(&target) {
        let parsed = ProfileSector::parse(&raw, info)?;
        if !parsed.crc_ok {
            return Err(WriteError::InputCrcInvalid {
                stored: parsed.stored_crc,
                computed: parsed.computed_crc,
            });
        }
        // A directory image never goes into a profile sector.
        if validate_directory_image(&raw).is_ok() {
            return Err(WriteError::ImplausibleProfile("it is a directory image"));
        }
    } else {
        return Err(WriteError::ProfileSectorNotAllowed(target));
    }
    Ok(SectorImage::new(target, raw))
}

/// Image from a backup file for profile sector `target` (3, 4, 5): format 3,
/// valid CRC, not shaped like a directory, rate interval in {1, 2, 4, 8} and
/// both DPI indexes at most 4.
pub fn restore_profile_image(
    target: u16,
    bytes: &[u8],
    info: &OnboardInfo,
) -> Result<SectorImage, WriteError> {
    if !PROFILE_SECTORS.contains(&target) {
        return Err(WriteError::ProfileSectorNotAllowed(target));
    }
    let image = original_image(target, bytes, info)?;
    if validate_directory_image(image.bytes()).is_ok() {
        return Err(WriteError::ImplausibleProfile("it is a directory image"));
    }
    let profile = ProfileSector::parse(image.bytes(), info)?;
    if !ALLOWED_RATE_INTERVALS_MS.contains(&profile.report_rate) {
        return Err(WriteError::ImplausibleProfile("report rate byte"));
    }
    if profile.default_dpi_index > 4 || profile.shift_dpi_index > 4 {
        return Err(WriteError::ImplausibleProfile("DPI index above 4"));
    }
    Ok(image)
}

/// Image from a backup file for the directory 0x0000, checked against
/// `current` (the directory as read now) and `active` (fn4 sector): valid
/// directory image; the same entries in the same order as `current`; every
/// byte but the flag bytes and the CRC equal to `current`; the active profile
/// not disabled; at least one profile enabled (the `patch_directory` rules).
pub fn restore_directory_image(
    bytes: &[u8],
    current: &[u8],
    active: u16,
) -> Result<SectorImage, WriteError> {
    let raw: [u8; SECTOR_LEN] = bytes
        .try_into()
        .map_err(|_| WriteError::SectorLength { got: bytes.len() })?;
    let now: [u8; SECTOR_LEN] = current
        .try_into()
        .map_err(|_| WriteError::SectorLength { got: current.len() })?;
    check_input_crc(&now)?;
    validate_directory_image(&raw)?;
    let new_entries = directory_entries(&raw)?;
    let now_entries = directory_entries(&now)?;
    if new_entries.len() != now_entries.len()
        || new_entries
            .iter()
            .zip(&now_entries)
            .any(|(a, b)| a.sector != b.sector || a.offset != b.offset)
    {
        return Err(WriteError::DirectoryMismatch(
            "entry list differs from the current directory",
        ));
    }
    let flag_bytes: Vec<usize> = now_entries.iter().map(|e| e.offset + 2).collect();
    for i in 0..SECTOR_LEN - 2 {
        if raw[i] != now[i] && !flag_bytes.contains(&i) {
            return Err(WriteError::DirectoryMismatch(
                "bytes other than the flags differ",
            ));
        }
    }
    if new_entries.iter().any(|e| e.sector == active && !e.enabled) {
        return Err(WriteError::ActiveProfileDisabled(active));
    }
    if !new_entries.iter().any(|e| e.enabled) {
        return Err(WriteError::NoProfileEnabled);
    }
    Ok(SectorImage::new(DIRECTORY_SECTOR, raw))
}
