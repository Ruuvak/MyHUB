//! Flash writes of onboard profile sectors (0x8100 fn6, fn7, fn8).
//!
//! Every write goes through `HidppSession::write_sector`, the only entry point
//! for the onboard write tier. It takes a sealed `hidpp::onboard_write::SectorImage`
//! (sector baked in; made only by patching bytes read from the device, patching
//! the directory, or a validated restore) and builds every frame itself.
//! Callers must have the user's explicit approval; nothing here asks. Only whole
//! sectors in `hidpp::policy::WRITE_SECTOR_ALLOWLIST` are written.
//!
//! Sequence of [`write_profile_sector`]: read info (format 3, size 255), read
//! the sector and check its CRC, hand the raw bytes to the backup callback
//! (an error aborts before any write frame), patch, send fn6, up to 16 fn7
//! frames (stop at the first error), fn8 (skipped only when the link is lost mid-sequence), read the sector back and
//! compare it byte for byte plus CRC. On a write error or a mismatch the
//! flash is read once more: if it equals the original, no restore is needed; if
//! it differs or cannot be read, the backup image is written once with the same
//! sequence and verification. The typed error says which path ran and whether
//! the restore worked.
//!
//! A HID++ 1.0 0x41 or `NeedsRediscovery` in the middle of a sequence means the
//! link is down (the mouse is off or asleep). Then neither EndWrite nor the
//! restore nor the read-back is sent: the session would refuse them anyway and
//! nothing can be done until the device is back. The error reports
//! [`ReadbackCheck::Skipped`] and [`RestoreStatus::NotAttempted`]; the sector
//! must be read and checked by hand before anything else is written.
//!
//! Stage 5 (the MyHUB profile): [`write_profile_sector`] with
//! [`ProfileSource::CloneFrom`] copies a user profile into 0x0003..=0x0005 and
//! patches it, [`write_directory`] enables or disables such a profile in the
//! directory 0x0000 (never the active one, never the last enabled one),
//! [`select_profile`] selects an enabled profile with fn3 and verifies with fn4.
//! The policy only checks single frames, so these functions keep profile images
//! out of 0x0000 and directory images out of the profile sectors.

use hidpp::dpi::{self, DpiList};
use hidpp::onboard::{self, OnboardInfo, SectorAssembler};
use hidpp::onboard_write::{self as w, PatchedImages, ProfilePatch, SECTOR_LEN, SectorImage};
use hidpp::policy::WRITE_SECTOR_ALLOWLIST;
use hidpp::{FeatureId, ParseError, crc};
use myhub_transport::{HidLink, HidppSession, SessionError, WriteLog, link_lost};
use std::fmt;
use thiserror::Error;

/// Reply timeout of BeginWrite and EndWrite (a flash commit may be slow), owned by
/// the transport.
pub use myhub_transport::FLASH_COMMIT_TIMEOUT;

/// Reply payloads of one write sequence as hex, for logging the \[UNK\] replies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SequenceLog {
    pub begin: Option<String>,
    pub data: Vec<String>,
    pub end: Option<String>,
}

/// What a successful call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// The sector was written and verified.
    Written,
    /// The image equals the bytes read and `force_identical` was off: no write
    /// frame was sent.
    Unchanged,
}

/// Result of a successful [`write_profile_sector`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteReport {
    pub sector: u16,
    /// The sector as read from the device before the write.
    pub read_bytes: Vec<u8>,
    /// The image that was (or would have been) written.
    pub image: Vec<u8>,
    pub log: SequenceLog,
    pub outcome: WriteOutcome,
}

/// What happened to the automatic restore after a failed write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreStatus {
    /// No restore was attempted (restore_sector itself, or the link was lost).
    NotAttempted,
    /// The read-back showed the original bytes, so there was nothing to restore.
    NotNeeded,
    /// The backup image was written and read back identically.
    Restored(SequenceLog),
    /// The restore failed too; the sector may hold neither image.
    Failed(String, SequenceLog),
}

impl fmt::Display for RestoreStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAttempted => f.write_str("not attempted"),
            Self::NotNeeded => f.write_str("not needed, the sector reads back as the original"),
            Self::Restored(_) => f.write_str("backup restored and verified"),
            Self::Failed(why, _) => write!(f, "RESTORE FAILED: {why}"),
        }
    }
}

/// What the check after a failed write found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadbackCheck {
    /// The sector reads back as the original: nothing to restore.
    Identical,
    /// The sector differs from the original: restore attempted.
    Differs,
    /// The sector could not be read (reason): restore attempted.
    Unreadable(String),
    /// The link was lost mid-sequence: no read-back, no restore.
    Skipped,
}

/// Reply log, read-back check and restore result of a failed write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureDetail {
    pub log: SequenceLog,
    pub readback: ReadbackCheck,
    pub restore: RestoreStatus,
}

/// Callback that stores the sector bytes read from the device before a write.
pub type BackupFn<'a> = dyn FnMut(u16, &[u8; SECTOR_LEN]) -> Result<(), String> + 'a;

/// Why a sector was not written, or not written correctly.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SectorWriteError {
    #[error("sector {0:#06x} is not in the write allowlist; nothing was sent")]
    SectorNotAllowed(u16),
    #[error("profile format {0} is not 3; nothing was written")]
    UnsupportedFormat(u8),
    #[error("sector size {0} is not 255; nothing was written")]
    SectorSize(u16),
    #[error("the sector read from the device has an invalid CRC; nothing was written")]
    ReadCrcInvalid,
    #[error(
        "clone source {0:#06x} is not a user sector 1..=5 other than the target; nothing was sent"
    )]
    BadSource(u16),
    #[error("profile {0:#06x} is not enabled in the directory; nothing was sent")]
    ProfileNotEnabled(u16),
    #[error(
        "profile select verification failed: expected sector {expected:#06x}, fn4 reports {got:#06x}"
    )]
    SelectVerifyFailed { expected: u16, got: u16 },
    #[error("the target sector does not hold a valid profile ({0}); nothing was written")]
    TargetProfileInvalid(String),
    #[error("backup failed, nothing was written: {0}")]
    Backup(String),
    #[error("patch refused, nothing was written: {0}")]
    Patch(#[from] w::WriteError),
    #[error("image refused, nothing was written: {0}")]
    ImageInvalid(String),
    #[error("write sequence failed: {cause}; restore: {}", .detail.restore)]
    WriteFailed {
        cause: SessionError,
        detail: Box<FailureDetail>,
    },
    #[error(
        "read-back differs from the image (first difference at {first_diff:?}, crc ok: {crc_ok}); restore: {}",
        .detail.restore
    )]
    VerifyFailed {
        first_diff: Option<usize>,
        crc_ok: bool,
        detail: Box<FailureDetail>,
    },
    #[error("could not parse a device reply: {0}")]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Session(#[from] SessionError),
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn read_info<L: HidLink>(session: &mut HidppSession<L>) -> Result<OnboardInfo, SectorWriteError> {
    let payload =
        session.request_feature(FeatureId::ONBOARD_PROFILES, onboard::FN_GET_INFO, [0; 16])?;
    Ok(OnboardInfo::parse(&payload)?)
}

fn read_sector<L: HidLink>(
    session: &mut HidppSession<L>,
    sector: u16,
    size: u16,
) -> Result<Vec<u8>, SessionOrParse> {
    let mut assembler = SectorAssembler::new(size)?;
    for offset in onboard::chunk_plan(size)? {
        let payload = session.request_feature(
            FeatureId::ONBOARD_PROFILES,
            onboard::FN_READ_DATA,
            onboard::read_data_params(sector, offset),
        )?;
        assembler.push_chunk(offset, &payload)?;
    }
    Ok(assembler.finish()?)
}

/// Internal: a failure while reading a sector back.
#[derive(Debug)]
enum SessionOrParse {
    Session(SessionError),
    Parse(ParseError),
}

impl From<SessionError> for SessionOrParse {
    fn from(e: SessionError) -> Self {
        Self::Session(e)
    }
}

impl From<ParseError> for SessionOrParse {
    fn from(e: ParseError) -> Self {
        Self::Parse(e)
    }
}

impl From<SessionOrParse> for SectorWriteError {
    fn from(e: SessionOrParse) -> Self {
        match e {
            SessionOrParse::Session(e) => Self::Session(e),
            SessionOrParse::Parse(e) => Self::Parse(e),
        }
    }
}

/// Checks that run before any write frame: allowlist, format 3, size 255.
fn precheck<L: HidLink>(
    session: &mut HidppSession<L>,
    sector: u16,
) -> Result<OnboardInfo, SectorWriteError> {
    if !WRITE_SECTOR_ALLOWLIST.contains(&sector) {
        return Err(SectorWriteError::SectorNotAllowed(sector));
    }
    let info = read_info(session)?;
    if info.profile_format != onboard::SUPPORTED_PROFILE_FORMAT {
        return Err(SectorWriteError::UnsupportedFormat(info.profile_format));
    }
    if usize::from(info.sector_size) != SECTOR_LEN {
        return Err(SectorWriteError::SectorSize(info.sector_size));
    }
    Ok(info)
}

enum SeqFailure {
    Session(SessionError),
    Mismatch {
        first_diff: Option<usize>,
        crc_ok: bool,
    },
}

fn log_of(log: &WriteLog) -> SequenceLog {
    SequenceLog {
        begin: log.begin.as_ref().map(|r| hex(r)),
        data: log.data.iter().map(|r| hex(r)).collect(),
        end: log.end.as_ref().map(|r| hex(r)),
    }
}

/// `HidppSession::write_sector` (fn6, 16 x fn7, fn8; EndWrite always unless the
/// link is lost), then read back and compare. Returns the reply log in every
/// case.
fn write_and_verify<L: HidLink>(
    session: &mut HidppSession<L>,
    image: &SectorImage,
) -> (SequenceLog, Result<(), SeqFailure>) {
    let (log, result) = session.write_sector(image);
    let log = log_of(&log);
    if let Err(e) = result {
        return (log, Err(SeqFailure::Session(e)));
    }

    let back = match read_sector(session, image.sector(), SECTOR_LEN as u16) {
        Ok(back) => back,
        Err(SessionOrParse::Session(e)) => return (log, Err(SeqFailure::Session(e))),
        Err(SessionOrParse::Parse(_)) => {
            return (
                log,
                Err(SeqFailure::Mismatch {
                    first_diff: None,
                    crc_ok: false,
                }),
            );
        }
    };
    let first_diff = back.iter().zip(image.bytes()).position(|(a, b)| a != b);
    let crc_ok = crc::sector_crc_ok(&back);
    if first_diff.is_some() || back.len() != SECTOR_LEN || !crc_ok {
        return (log, Err(SeqFailure::Mismatch { first_diff, crc_ok }));
    }
    (log, Ok(()))
}

fn attempt_restore<L: HidLink>(
    session: &mut HidppSession<L>,
    rollback: &SectorImage,
) -> RestoreStatus {
    match write_and_verify(session, rollback) {
        (log, Ok(())) => RestoreStatus::Restored(log),
        (log, Err(SeqFailure::Session(e))) => RestoreStatus::Failed(e.to_string(), log),
        (log, Err(SeqFailure::Mismatch { first_diff, crc_ok })) => RestoreStatus::Failed(
            format!("read-back differs (first difference {first_diff:?}, crc ok: {crc_ok})"),
            log,
        ),
    }
}

/// After a failed sequence: read the sector back and restore the original only
/// if it differs or cannot be read. A lost link (0x41, `NeedsRediscovery`)
/// skips both.
fn failure_detail<L: HidLink>(
    session: &mut HidppSession<L>,
    rollback: &SectorImage,
    failure: &SeqFailure,
    log: SequenceLog,
) -> FailureDetail {
    let sector = rollback.sector();
    if matches!(failure, SeqFailure::Session(e) if link_lost(e)) {
        return FailureDetail {
            log,
            readback: ReadbackCheck::Skipped,
            restore: RestoreStatus::NotAttempted,
        };
    }
    let readback = match read_sector(session, sector, SECTOR_LEN as u16) {
        Ok(bytes) if bytes == rollback.bytes() => ReadbackCheck::Identical,
        Ok(_) => ReadbackCheck::Differs,
        Err(SessionOrParse::Session(e)) if link_lost(&e) => {
            return FailureDetail {
                log,
                readback: ReadbackCheck::Skipped,
                restore: RestoreStatus::NotAttempted,
            };
        }
        Err(SessionOrParse::Session(e)) => ReadbackCheck::Unreadable(e.to_string()),
        Err(SessionOrParse::Parse(e)) => ReadbackCheck::Unreadable(e.to_string()),
    };
    let restore = if readback == ReadbackCheck::Identical {
        RestoreStatus::NotNeeded
    } else {
        attempt_restore(session, rollback)
    };
    FailureDetail {
        log,
        readback,
        restore,
    }
}

fn into_error(failure: SeqFailure, detail: FailureDetail) -> SectorWriteError {
    let detail = Box::new(detail);
    match failure {
        SeqFailure::Session(cause) => SectorWriteError::WriteFailed { cause, detail },
        SeqFailure::Mismatch { first_diff, crc_ok } => SectorWriteError::VerifyFailed {
            first_diff,
            crc_ok,
            detail,
        },
    }
}

/// Where the base image of a profile write comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    /// The target sector's own bytes (patch in place).
    Same,
    /// A copy of another user sector (1..=5, not the target), then the patch.
    /// The backup is still of the target.
    CloneFrom(u16),
}

/// Reads the target sector, checks its CRC and hands it to the backup callback.
fn read_original_and_backup<L: HidLink>(
    session: &mut HidppSession<L>,
    sector: u16,
    info: &OnboardInfo,
    backup: &mut BackupFn<'_>,
) -> Result<([u8; SECTOR_LEN], Vec<u8>), SectorWriteError> {
    let read_bytes = read_sector(session, sector, info.sector_size)?;
    let original: [u8; SECTOR_LEN] = read_bytes
        .as_slice()
        .try_into()
        .map_err(|_| SectorWriteError::SectorSize(info.sector_size))?;
    if !crc::sector_crc_ok(&original) {
        return Err(SectorWriteError::ReadCrcInvalid);
    }
    backup(sector, &original).map_err(SectorWriteError::Backup)?;
    Ok((original, read_bytes))
}

/// Writes `image` unless it equals `original` (and `force_identical` is off),
/// verifies, and on failure checks the sector and restores `original` if needed.
fn finish_write<L: HidLink>(
    session: &mut HidppSession<L>,
    read_bytes: Vec<u8>,
    patched: PatchedImages,
    force_identical: bool,
) -> Result<WriteReport, SectorWriteError> {
    let PatchedImages {
        new: image,
        rollback,
    } = patched;
    let sector = image.sector();
    if image.bytes() == rollback.bytes() && !force_identical {
        return Ok(WriteReport {
            sector,
            read_bytes,
            image: image.bytes().to_vec(),
            log: SequenceLog::default(),
            outcome: WriteOutcome::Unchanged,
        });
    }
    match write_and_verify(session, &image) {
        (log, Ok(())) => Ok(WriteReport {
            sector,
            read_bytes,
            image: image.bytes().to_vec(),
            log,
            outcome: WriteOutcome::Written,
        }),
        (log, Err(failure)) => {
            let detail = failure_detail(session, &rollback, &failure, log);
            Err(into_error(failure, detail))
        }
    }
}

/// Patches and writes one user profile sector (0x0003..=0x0005 only; the
/// directory 0x0000 is refused here, it has [`write_directory`]), see the
/// module docs for the sequence. `backup` receives the sector number and the
/// raw bytes read from the target; if it fails, nothing is written. With
/// `ProfileSource::CloneFrom(src)` the base image is a copy of user sector `src`
/// (1..=5) instead of the target's own bytes. With an image equal to the bytes
/// read and `force_identical` false, no write frame is sent and the outcome is
/// [`WriteOutcome::Unchanged`].
pub fn write_profile_sector<L: HidLink>(
    session: &mut HidppSession<L>,
    sector: u16,
    patch: &ProfilePatch,
    source: ProfileSource,
    backup: &mut BackupFn<'_>,
    force_identical: bool,
) -> Result<WriteReport, SectorWriteError> {
    // The write allowlist also holds the directory; a profile image never goes
    // there.
    if !w::PROFILE_SECTORS.contains(&sector) {
        return Err(SectorWriteError::SectorNotAllowed(sector));
    }
    if let ProfileSource::CloneFrom(src) = source
        && (!w::SELECTABLE_SECTORS.contains(&src) || src == sector)
    {
        return Err(SectorWriteError::BadSource(src));
    }
    let info = precheck(session, sector)?;
    let (original, read_bytes) = read_original_and_backup(session, sector, &info, backup)?;

    let base = match source {
        ProfileSource::Same => None,
        ProfileSource::CloneFrom(src) => {
            let bytes = read_sector(session, src, info.sector_size)?;
            let src_raw: [u8; SECTOR_LEN] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| SectorWriteError::SectorSize(info.sector_size))?;
            Some(w::clone_profile(&src_raw, &info)?)
        }
    };

    let list = if patch.dpi_levels.is_some() {
        let payload = session.request_feature(
            FeatureId::ADJUSTABLE_DPI,
            dpi::FN_GET_SENSOR_DPI_LIST,
            dpi::sensor_params(0),
        )?;
        DpiList::parse(&payload)?
    } else {
        // Not consulted without DPI levels in the patch.
        DpiList {
            sensor: 0,
            entries: Vec::new(),
        }
    };
    let patched = match &base {
        None => w::apply_patch(sector, &original, &info, &list, patch)?,
        Some(base) => w::apply_patch_over(sector, &original, base, &info, &list, patch)?,
    };
    finish_write(session, read_bytes, patched, force_identical)
}

/// Reads fn4 GetActiveProfile: the active sector (BE16), 0 in host mode.
fn read_active<L: HidLink>(session: &mut HidppSession<L>) -> Result<u16, SectorWriteError> {
    let payload = session.request_feature(
        FeatureId::ONBOARD_PROFILES,
        onboard::FN_GET_ACTIVE_PROFILE,
        [0; 16],
    )?;
    Ok(u16::from_be_bytes([payload[0], payload[1]]))
}

/// Enables or disables the directory entry of profile `sector` (3, 4 or 5) in
/// sector 0x0000: reads fn4 and the directory, hands the directory to the backup
/// callback (sector 0), patches only that entry's flag, then writes and verifies
/// through the same sequence as a profile write, restoring on failure. Refuses
/// to disable the active profile or the last enabled one. Enabling also needs
/// the target sector to hold a valid format 3 profile (parse ok, CRC valid), or
/// nothing is written.
pub fn write_directory<L: HidLink>(
    session: &mut HidppSession<L>,
    sector: u16,
    enabled: bool,
    backup: &mut BackupFn<'_>,
    force_identical: bool,
) -> Result<WriteReport, SectorWriteError> {
    if !w::PROFILE_SECTORS.contains(&sector) {
        return Err(SectorWriteError::SectorNotAllowed(sector));
    }
    let info = precheck(session, onboard::DIRECTORY_SECTOR)?;
    if enabled {
        let target = read_sector(session, sector, info.sector_size)?;
        // Format 3, CRC, not directory-shaped, plausible rate and indexes.
        w::restore_profile_image(sector, &target, &info)
            .map_err(|e| SectorWriteError::TargetProfileInvalid(e.to_string()))?;
    }
    let active = read_active(session)?;
    let bytes = read_sector(session, onboard::DIRECTORY_SECTOR, info.sector_size)?;
    let raw: [u8; SECTOR_LEN] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| SectorWriteError::SectorSize(info.sector_size))?;
    if !crc::sector_crc_ok(&raw) {
        return Err(SectorWriteError::ReadCrcInvalid);
    }
    // Refuse before the backup if the directory would be refused anyway.
    let patched = w::patch_directory(&raw, sector, enabled, active)?;
    backup(onboard::DIRECTORY_SECTOR, &raw).map_err(SectorWriteError::Backup)?;
    finish_write(session, bytes, patched, force_identical)
}

/// Selects `sector` (1..=5) with fn3 SetCurrentProfile, only if the directory
/// shows it enabled, then verifies with fn4. Returns the fn3 reply payload as
/// hex for logging (\[UNK\]).
pub fn select_profile<L: HidLink>(
    session: &mut HidppSession<L>,
    sector: u16,
) -> Result<String, SectorWriteError> {
    let selectable = w::SelectableSector::new(sector)?;
    let info = read_info(session)?;
    if info.profile_format != onboard::SUPPORTED_PROFILE_FORMAT {
        return Err(SectorWriteError::UnsupportedFormat(info.profile_format));
    }
    if usize::from(info.sector_size) != SECTOR_LEN {
        return Err(SectorWriteError::SectorSize(info.sector_size));
    }
    let bytes = read_sector(session, onboard::DIRECTORY_SECTOR, info.sector_size)?;
    w::validate_directory_image(&bytes)?;
    let directory = hidpp::onboard::Directory::parse(&bytes)?;
    if !directory.enabled_sectors().contains(&sector) {
        return Err(SectorWriteError::ProfileNotEnabled(sector));
    }
    let reply = session.request_profile_select(selectable)?;
    let got = read_active(session)?;
    if got != sector {
        return Err(SectorWriteError::SelectVerifyFailed {
            expected: sector,
            got,
        });
    }
    Ok(hex(&reply))
}

/// Writes a backup image back with the same write and verify sequence. Accepts
/// only an allowlisted sector (0x0000 or 0x0003..=0x0005) and a 255 byte image
/// with a valid CRC that fits the sector:
/// - 0x0000: a directory image with the same entries as the directory read now
///   (only the flag bytes may differ), keeping the active profile (fn4) enabled
///   and at least one profile enabled;
/// - 0x0003..=0x0005: a format 3 profile (rate and DPI indexes plausible, not
///   shaped like a directory).
///
/// No further restore is attempted if this fails.
pub fn restore_sector<L: HidLink>(
    session: &mut HidppSession<L>,
    sector: u16,
    image: &[u8],
) -> Result<SequenceLog, SectorWriteError> {
    let info = precheck(session, sector)?;
    let image = if sector == onboard::DIRECTORY_SECTOR {
        let active = read_active(session)?;
        let current = read_sector(session, onboard::DIRECTORY_SECTOR, info.sector_size)?;
        w::restore_directory_image(image, &current, active)
    } else {
        w::restore_profile_image(sector, image, &info)
    }
    .map_err(|e| SectorWriteError::ImageInvalid(e.to_string()))?;
    match write_and_verify(session, &image) {
        (log, Ok(())) => Ok(log),
        (log, Err(failure)) => {
            let detail = FailureDetail {
                log,
                readback: ReadbackCheck::Skipped,
                restore: RestoreStatus::NotAttempted,
            };
            Err(into_error(failure, detail))
        }
    }
}

/// Result of a successful [`save_active_profile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveReport {
    /// The active profile sector that was saved (3, 4 or 5).
    pub sector: u16,
    /// What the write did (written, or unchanged with no frame sent).
    pub write: WriteReport,
    /// fn3 reply payload as hex when the sector was re-selected after a write,
    /// `None` when nothing was written (so nothing was re-selected).
    pub reselected: Option<String>,
    /// `true` when the profile was reloaded by switching to another enabled
    /// profile and back (fn3 to the active sector alone does not reload it).
    /// `false` when nothing was written, or when no other profile is enabled and
    /// only a plain re-select was possible.
    pub reloaded: bool,
    /// fn3 reply payloads as hex, in order: the switch to the other profile (if
    /// any), then the switch to the saved sector.
    pub reload_log: Vec<String>,
}

/// Why [`save_active_profile`] did not save.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SaveError {
    /// fn2 did not report onboard mode (host mode or an unknown value). Nothing
    /// was written.
    #[error("the mouse is not in onboard mode; nothing was written")]
    NotOnboardMode,
    /// The active profile (fn4) is not a user sector 3..=5 that is enabled in
    /// the directory, so it is not saved. Nothing was written.
    #[error("active profile {active:#06x} cannot be saved: {reason}; nothing was written")]
    ProfileNotWritable { active: u16, reason: &'static str },
    /// The write itself failed or was refused (see [`SectorWriteError`]).
    #[error(transparent)]
    Write(SectorWriteError),
    /// The sector was written and verified, but reloading it failed. The new
    /// content is on the mouse; the mouse may still run the old copy.
    /// `active_now` is the sector fn4 reports afterwards (`None` if fn4 could not
    /// be read), so the caller knows which profile the mouse runs.
    #[error(
        "the profile was written but reloading it failed: {cause} (active profile now: {active_now:?})"
    )]
    Reselect {
        report: Box<WriteReport>,
        cause: SectorWriteError,
        active_now: Option<u16>,
    },
    #[error(transparent)]
    Session(#[from] SessionError),
}

/// Saves `patch` into the profile that is active on the mouse. Reads fn2 (must
/// be onboard) and fn4 (the active sector must be 3, 4 or 5 and enabled in the
/// directory 0x0000), calls [`write_profile_sector`] in place
/// (`ProfileSource::Same`, `force_identical` false) and, when the sector was
/// written, reloads it: fn3 to another enabled profile and back to this one (each verified
/// with fn4; the switch back is retried once), because fn3 to the active sector
/// alone does not reload it. With no other enabled profile it falls back to a
/// plain re-select and reports `reloaded: false`. If the switch away fails, the
/// switch back still runs (a failed switch away reports `reloaded: false`, noted
/// in the log); `Reselect` is returned only when the mouse is still not on the
/// saved profile afterwards. A profile-button press during a save is overridden
/// by the switch back. An unchanged image sends no write frame and no fn3.
///
/// The caller needs the user's explicit approval. `backup` receives the target
/// sector and its bytes before any write.
pub fn save_active_profile<L: HidLink>(
    session: &mut HidppSession<L>,
    patch: &ProfilePatch,
    backup: &mut BackupFn<'_>,
) -> Result<SaveReport, SaveError> {
    if crate::write::onboard_mode(session)? != Some(myhub_core::OnboardMode::Onboard) {
        return Err(SaveError::NotOnboardMode);
    }
    let active = read_active(session).map_err(SaveError::Write)?;
    let not_writable = |reason| SaveError::ProfileNotWritable { active, reason };
    if !w::PROFILE_SECTORS.contains(&active) {
        return Err(not_writable(
            "only the user sectors 3, 4 and 5 can be saved",
        ));
    }
    let info = read_info(session).map_err(SaveError::Write)?;
    if info.profile_format != onboard::SUPPORTED_PROFILE_FORMAT
        || usize::from(info.sector_size) != SECTOR_LEN
    {
        return Err(not_writable(
            "the profile format is not 3 or the sector is not 255 bytes",
        ));
    }
    let dir = read_sector(session, onboard::DIRECTORY_SECTOR, info.sector_size)
        .map_err(|e| SaveError::Write(e.into()))?;
    w::validate_directory_image(&dir).map_err(|_| not_writable("the directory is not valid"))?;
    let directory = onboard::Directory::parse(&dir).map_err(|e| SaveError::Write(e.into()))?;
    if !directory.enabled_sectors().contains(&active) {
        return Err(not_writable("it is not enabled in the directory"));
    }

    let write = write_profile_sector(session, active, patch, ProfileSource::Same, backup, false)
        .map_err(SaveError::Write)?;
    if write.outcome == WriteOutcome::Unchanged {
        return Ok(SaveReport {
            sector: active,
            write,
            reselected: None,
            reloaded: false,
            reload_log: Vec::new(),
        });
    }
    // fn3 to the sector that is already active does not reload it [OBS]; a
    // switch to another profile and back does. Prefer 0x0001 or 0x0002.
    let enabled = directory.enabled_sectors();
    let other = [1u16, 2]
        .into_iter()
        .chain(enabled.iter().copied())
        .find(|s| *s != active && enabled.contains(s) && w::SELECTABLE_SECTORS.contains(s));
    let mut reload_log = Vec::new();
    // A failed switch away (its fn3 may or may not have landed) does not stop
    // the switch back: the mouse must end on the saved profile.
    let mut away_failed = false;
    if let Some(other) = other {
        match select_profile(session, other) {
            Ok(reply) => reload_log.push(reply),
            Err(cause) => {
                away_failed = true;
                reload_log.push(format!("switch away to {other:#06x} failed: {cause}"));
            }
        }
    }
    // The switch back is retried once.
    let back = select_profile(session, active).or_else(|_| select_profile(session, active));
    match back {
        Ok(reply) => {
            reload_log.push(reply.clone());
            Ok(SaveReport {
                sector: active,
                write,
                reselected: Some(reply),
                reloaded: other.is_some() && !away_failed,
                reload_log,
            })
        }
        Err(cause) => {
            let active_now = read_active(session).ok();
            if active_now == Some(active) {
                // The switch back did land (only its verification failed): the
                // mouse is on the saved profile, but nothing proves a reload.
                reload_log.push(format!(
                    "switch back to {active:#06x} reported an error but fn4 shows it active: {cause}"
                ));
                return Ok(SaveReport {
                    sector: active,
                    write,
                    reselected: None,
                    reloaded: false,
                    reload_log,
                });
            }
            Err(SaveError::Reselect {
                report: Box::new(write),
                cause,
                active_now,
            })
        }
    }
}
