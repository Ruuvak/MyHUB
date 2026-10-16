//! Writes to the keyboard. Slice S4a: select the active HE profile (A9 11, Volatile),
//! optionally save it to flash (A9 1F, Persistent).
//!
//! Each write frame is checked against a policy built for that one call: the session's
//! read-only policy with its runtime limits plus exactly one tier. The session's stored
//! policy never changes. Writes are never resent and are at least `WRITE_GAP` apart.
//!
//! Any error after a write frame went out (Timeout, AmbiguousReply, ForeignTraffic,
//! BadReport, a link error) means the state is unknown: the keyboard may or may not have
//! applied the write. The caller must re-read (A9 10) before it decides anything.
//!
//! [INF] A9 11 changes RAM only; A9 1F writes the current profile buffer to flash.
//! [UNK] Whether the selected profile survives a replug without A9 1F.

use keychron_proto::he::{ProfilesInfo, decode_profiles_info};
use keychron_proto::{Request, Tier};
use serde::Serialize;

use crate::error::SessionError;
use crate::link::RawLink;
use crate::session::Session;

/// What [`select_profile`] did. Profile numbers are 0-based, as on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SelectOutcome {
    /// Current profile before the call.
    pub before: u8,
    /// Current profile read back at the end.
    pub after: u8,
    /// A9 1F was sent and acknowledged with status 0.
    pub saved: bool,
    /// Write-tier frames sent (A9 11 and A9 1F); 0 when nothing needed writing.
    pub frames_written: u32,
    /// Nothing was written: the profile was already current and no save was asked.
    pub unchanged: bool,
}

/// When a travel or lighting write also saves to flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    No,
    /// Always save, even when the values already held.
    Yes,
    /// Save only when a write frame was needed; nothing is sent when nothing changes.
    IfChanged,
}

impl From<bool> for SaveMode {
    fn from(save: bool) -> Self {
        if save { Self::Yes } else { Self::No }
    }
}

/// One A9 10 read (Read tier). Also fills the policy's profile count and buffer size,
/// which a profile request needs.
pub fn read_profiles_info<L: RawLink>(s: &mut Session<L>) -> Result<ProfilesInfo, SessionError> {
    let reply = s.request(Request::GetProfilesInfo)?;
    let info = decode_profiles_info(&reply)?;
    s.set_limits(|p| {
        p.with_profile_count(info.count)
            .with_profile_buffer_size(info.size)
    });
    Ok(info)
}

/// Make `profile` (0-based) the active HE profile; with `save`, also write it to flash.
///
/// Reads A9 10 first. If `profile` is already current and `save` is false, nothing is
/// written. If it is current and `save` is true, A9 11 is skipped. After A9 11 the profile
/// is read back; a different value gives [`SessionError::VerifyFailed`]. Before A9 1F the
/// current profile is read again and must equal `profile`, or nothing is sent
/// (`VerifyFailed`). A9 1F with a nonzero status byte gives
/// [`SessionError::DeviceStatus`]; after it the profile is read back once more
/// (`VerifyFailed { saved: true, .. }` on a mismatch).
///
/// Errors are never retried, and the reads inside the call are sent once (no resend after a
/// timeout). Any error after a write frame went out (Timeout,
/// AmbiguousReply, ForeignTraffic, BadReport, a link error) leaves the keyboard state
/// unknown: the caller must re-read A9 10 before it decides anything.
pub fn select_profile<L: RawLink>(
    s: &mut Session<L>,
    profile: u8,
    save: bool,
) -> Result<SelectOutcome, SessionError> {
    // Reads inside the sequence are sent once: the worst case is one reply timeout per frame.
    s.single_reads(|s| select_inner(s, profile, save))
}

fn select_inner<L: RawLink>(
    s: &mut Session<L>,
    profile: u8,
    save: bool,
) -> Result<SelectOutcome, SessionError> {
    let before = read_profiles_info(s)?.current;
    if before == profile && !save {
        return Ok(SelectOutcome {
            before,
            after: before,
            saved: false,
            frames_written: 0,
            unchanged: true,
        });
    }
    let mut frames_written = 0;
    let mut after = before;
    if before != profile {
        let policy = s.policy_with(Tier::Volatile);
        s.request_with(&policy, Request::SelectProfile { profile })?;
        frames_written += 1;
        after = verify(s, profile, false)?;
    }
    let mut saved = false;
    if save {
        // A9 1F saves the current buffer: refuse unless the current profile is ours.
        verify(s, profile, false)?;
        let policy = s.policy_with(Tier::Persistent);
        let reply = s.request_with(&policy, Request::SaveProfile { profile })?;
        frames_written += 1;
        if reply[2] != 0 {
            return Err(SessionError::DeviceStatus {
                command: 0xA9,
                sub: 0x1F,
                status: reply[2],
            });
        }
        saved = true;
        after = verify(s, profile, true)?;
    }
    Ok(SelectOutcome {
        before,
        after,
        saved,
        frames_written,
        unchanged: false,
    })
}

fn verify<L: RawLink>(s: &mut Session<L>, expected: u8, saved: bool) -> Result<u8, SessionError> {
    let got = read_profiles_info(s)?.current;
    if got == expected {
        Ok(got)
    } else {
        Err(SessionError::VerifyFailed {
            expected,
            got,
            saved,
        })
    }
}
