//! Slice S4c, part 1: the global travel setting (actuation point and Rapid Trigger) of the
//! CURRENT HE profile. A9 14 (Volatile), optionally A9 1F (Persistent).
//!
//! Same rules as `writes.rs`: one-call policies, no resend of a write, writes at least
//! `WRITE_GAP` apart, every write verified by re-reading the profile's global word
//! (GET_PROFILE_BUFFER offset 0, 4 bytes). Any error after a write frame went out
//! (Timeout, AmbiguousReply, ForeignTraffic, BadReport, a link error) means the state is
//! unknown: the caller must re-read before it decides anything.
//!
//! [OBS] A9 15 (not used here) changes RAM only. [INF] A9 14 changes RAM and A9 1F saves
//! the current profile's RAM buffer to flash. [INF] A9 14 targets the profile in byte 2,
//! but the launcher writes the current profile; v1 edits only the CURRENT profile, so the
//! profile is always the one A9 10 reports.

use keychron_proto::he::{KeyMode, KeyWord};
use keychron_proto::{Request, Tier, TravelMode};
use serde::Serialize;

use crate::error::SessionError;
use crate::link::RawLink;
use crate::session::Session;
use crate::writes::read_profiles_info;

/// What to set. Values are in 0.1 mm, as on the wire. Actuation 2..=38; press and release
/// 1..=38 (Rapid Trigger only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TravelSettings {
    pub mode: TravelMode,
    pub actuation: u8,
    /// Rapid trigger press sensitivity. Ignored in Static mode.
    pub press: u8,
    /// Rapid trigger release sensitivity. Ignored in Static mode.
    pub release: u8,
}

/// What [`set_travel_global`] did. The profile is 0-based, as on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TravelOutcome {
    /// The profile that was edited: the current one.
    pub profile: u8,
    /// Global word of that profile before the call.
    pub before: KeyWord,
    /// Global word read back at the end.
    pub after: KeyWord,
    /// A9 1F was sent and acknowledged with status 0.
    pub saved: bool,
    /// Write-tier frames sent (A9 14 and A9 1F).
    pub frames_written: u32,
    /// Nothing was written: the settings already held and no save was asked.
    pub unchanged: bool,
}

/// The global word of `profile`: A9 12 offset 0, 4 bytes, little-endian. Needs the buffer
/// size limit, which `read_profiles_info` sets.
pub fn read_global_word<L: RawLink>(
    s: &mut Session<L>,
    profile: u8,
) -> Result<KeyWord, SessionError> {
    let r = s.request(Request::GetProfileBuffer {
        profile,
        offset: 0,
        len: 4,
    })?;
    Ok(KeyWord::from_u32(u32::from_le_bytes([
        r[6], r[7], r[8], r[9],
    ])))
}

fn key_mode(mode: TravelMode) -> KeyMode {
    match mode {
        TravelMode::Static => KeyMode::Static,
        TravelMode::RapidTrigger => KeyMode::RapidTrigger,
    }
}

/// What the global word must read after the write: (mode, actuation, press, release).
type Expected = (KeyMode, u8, u8, u8);

fn matches(word: &KeyWord, e: Expected) -> bool {
    (word.mode, word.distance, word.press, word.release) == e
}

fn mismatch(word: &KeyWord, e: Expected, saved: bool) -> SessionError {
    SessionError::VerifyMismatch {
        what: "travel",
        expected: format!("{e:?}"),
        got: format!("{:?}", (word.mode, word.distance, word.press, word.release)),
        saved,
    }
}

/// Default press/release in 0.1 mm (0.4 mm, emulator default, he.md 3.3), used in Static
/// mode when the stored value is outside the allowed 1..=38. Also used by the probe.
pub const DEFAULT_SENSITIVITY: u8 = 4;

/// Static mode sends the stored press/release back so they are not wiped; a stored value
/// outside 1..=38 would be refused by the policy, so [`DEFAULT_SENSITIVITY`] replaces it.
fn static_sensitivity(stored: u8) -> u8 {
    if (1..=38).contains(&stored) {
        stored
    } else {
        DEFAULT_SENSITIVITY
    }
}

/// Set the global travel mode, actuation and Rapid Trigger sensitivities of the current
/// profile; with `save`, also write that profile to flash.
///
/// Reads A9 10 (current profile, limits) and the profile's global word (`before`). With
/// the same mode, actuation and (Rapid Trigger only) press/release and no `save`, nothing
/// is written. Otherwise A9 14 is sent with all values (always all three, he.md 4.1; in
/// Static mode press and release come from `before` so they are not wiped, with 4 (0.4 mm)
/// substituted for a stored value outside 1..=38, and the read-back must show those), its
/// status byte must be 0 ([`SessionError::DeviceStatus`]), and the global word is read
/// back (`VerifyMismatch { saved: false }` on a difference). With `save`, A9 10 is read
/// again and the call refuses if the current profile changed
/// ([`SessionError::ProfileChanged`], nothing sent); then A9 1F, status 0, and a final
/// read of the word (`VerifyMismatch { saved: true }`).
///
/// A policy refusal (range, mode) happens before any byte of that frame is sent. The
/// he.md 4.1 rule "sensitivity <= 3.8 mm - actuation" [INF] is NOT enforced here; if the
/// firmware rejects such a value it shows as `DeviceStatus` or `VerifyMismatch`.
///
/// Decoding: the global word's fields are counted LSB first (mode, distance, press,
/// release), which matches the hardware reads of profiles 0..2. That the A9 14 read-back
/// decodes the same way is first confirmed on the supervised hardware run.
pub fn set_travel_global<L: RawLink>(
    s: &mut Session<L>,
    settings: TravelSettings,
    save: bool,
) -> Result<TravelOutcome, SessionError> {
    let profile = read_profiles_info(s)?.current;
    let before = read_global_word(s, profile)?;
    let (press, release) = match settings.mode {
        TravelMode::Static => (
            static_sensitivity(before.press),
            static_sensitivity(before.release),
        ),
        TravelMode::RapidTrigger => (settings.press, settings.release),
    };
    let expected: Expected = (key_mode(settings.mode), settings.actuation, press, release);
    let same = matches(&before, expected);
    if same && !save {
        return Ok(TravelOutcome {
            profile,
            before,
            after: before,
            saved: false,
            frames_written: 0,
            unchanged: true,
        });
    }
    let mut frames_written = 0;
    let mut after = before;
    if !same {
        let policy = s.policy_with(Tier::Volatile);
        let reply = s.request_with(
            &policy,
            Request::SetTravelGlobal {
                profile,
                mode: settings.mode,
                actuation: settings.actuation,
                press,
                release,
            },
        )?;
        frames_written += 1;
        if reply[2] != 0 {
            return Err(SessionError::DeviceStatus {
                command: 0xA9,
                sub: 0x14,
                status: reply[2],
            });
        }
        after = read_global_word(s, profile)?;
        if !matches(&after, expected) {
            return Err(mismatch(&after, expected, false));
        }
    }
    let mut saved = false;
    if save {
        // A9 1F saves the current profile: refuse unless that is still the one we edited.
        let current = read_profiles_info(s)?.current;
        if current != profile {
            return Err(SessionError::ProfileChanged {
                expected: profile,
                got: current,
            });
        }
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
        after = read_global_word(s, profile)?;
        if !matches(&after, expected) {
            return Err(mismatch(&after, expected, true));
        }
    }
    Ok(TravelOutcome {
        profile,
        before,
        after,
        saved,
        frames_written,
        unchanged: false,
    })
}
