//! Slice S4c, part 2: global lighting over VIA channel 3. VIA 07 03 id value (Volatile),
//! optionally VIA 09 03 (Persistent).
//!
//! Same rules as `writes.rs`: one-call policies, no resend of a write, writes at least
//! `WRITE_GAP` apart, every write verified by re-reading with VIA 08 03. Any error after
//! a write frame went out means the state is unknown: re-read before deciding anything.
//!
//! [OBS] VIA 07 echoes its frame (the session already requires the echo of bytes 0..=2).
//! [INF] VIA 07 changes RAM and needs VIA 09 03 to persist. [INF] VIA 09 03 has no known
//! status byte: the matching echo is accepted and the values are re-read instead.
//!
//! Tolerance: [OBS] the K2 HE stores brightness one lower than sent (120 reads back 119,
//! 119 reads back 118 via 08 03 01). Brightness, speed, hue and saturation therefore
//! verify within [`TOLERANCE`]; the effect must match exactly. `after` in the outcome is
//! what the keyboard actually holds. A requested value within tolerance of the current
//! one counts as unchanged and is not written.

/// Largest accepted difference between a value sent and read back (brightness, speed,
/// hue, saturation).
pub const TOLERANCE: u8 = 2;

fn close(a: u8, b: u8) -> bool {
    a.abs_diff(b) <= TOLERANCE
}

/// Does `got` satisfy a write of `sent`? Effect exact, the rest within [`TOLERANCE`].
fn accepts(sent: LightValue, got: LightValue) -> bool {
    match (sent, got) {
        (LightValue::Effect(a), LightValue::Effect(b)) => a == b,
        (LightValue::Brightness(a), LightValue::Brightness(b))
        | (LightValue::Speed(a), LightValue::Speed(b)) => close(a, b),
        (
            LightValue::Color {
                hue: h1,
                saturation: s1,
            },
            LightValue::Color {
                hue: h2,
                saturation: s2,
            },
        ) => close(h1, h2) && close(s1, s2),
        _ => false,
    }
}

use keychron_proto::via::decode_light;
use keychron_proto::{LightId, LightValue, Request, Tier};
use serde::Serialize;

use crate::error::SessionError;
use crate::link::RawLink;
use crate::session::Session;
use crate::writes::SaveMode;

/// What to change. `None` leaves a value alone. Brightness and speed are 0..=255, the
/// effect 0..=24 (refused by the policy otherwise), the colour is (hue, saturation).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct LightChange {
    pub effect: Option<u8>,
    pub brightness: Option<u8>,
    pub speed: Option<u8>,
    pub color: Option<(u8, u8)>,
}

/// The four VIA channel 3 values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LightState {
    pub brightness: u8,
    pub effect: u8,
    pub speed: u8,
    pub hue: u8,
    pub saturation: u8,
}

/// What [`set_lighting`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LightOutcome {
    pub before: LightState,
    /// Values read back at the end.
    pub after: LightState,
    /// VIA 09 03 was sent and acknowledged.
    pub saved: bool,
    /// Write-tier frames sent (VIA 07 per changed field, plus VIA 09 03).
    pub frames_written: u32,
    /// Nothing was written: every asked value already held and no save was asked.
    pub unchanged: bool,
}

fn read_value<L: RawLink>(s: &mut Session<L>, id: LightId) -> Result<LightValue, SessionError> {
    let r = s.request(Request::GetLight { id })?;
    Ok(decode_light(&r, id)?)
}

/// VIA 08 03 01..04.
pub fn read_light_state<L: RawLink>(s: &mut Session<L>) -> Result<LightState, SessionError> {
    let mut st = LightState {
        brightness: 0,
        effect: 0,
        speed: 0,
        hue: 0,
        saturation: 0,
    };
    for id in [
        LightId::Brightness,
        LightId::Effect,
        LightId::Speed,
        LightId::Color,
    ] {
        match read_value(s, id)? {
            LightValue::Brightness(v) => st.brightness = v,
            LightValue::Effect(v) => st.effect = v,
            LightValue::Speed(v) => st.speed = v,
            LightValue::Color { hue, saturation } => {
                st.hue = hue;
                st.saturation = saturation;
            }
        }
    }
    Ok(st)
}

fn apply(st: &mut LightState, v: LightValue) {
    match v {
        LightValue::Brightness(x) => st.brightness = x,
        LightValue::Effect(x) => st.effect = x,
        LightValue::Speed(x) => st.speed = x,
        LightValue::Color { hue, saturation } => {
            st.hue = hue;
            st.saturation = saturation;
        }
    }
}

fn field(st: &LightState, id: LightId) -> LightValue {
    match id {
        LightId::Brightness => LightValue::Brightness(st.brightness),
        LightId::Effect => LightValue::Effect(st.effect),
        LightId::Speed => LightValue::Speed(st.speed),
        LightId::Color => LightValue::Color {
            hue: st.hue,
            saturation: st.saturation,
        },
    }
}

fn mismatch(expected: &LightState, got: &LightState, saved: bool) -> SessionError {
    SessionError::VerifyMismatch {
        what: "lighting",
        expected: format!("{expected:?}"),
        got: format!("{got:?}"),
        saved,
    }
}

/// Change global lighting; with `save`, also persist it (VIA 09 03).
///
/// Reads VIA 08 03 01..04 (`before`). Unchanged fields are skipped. One VIA 07 03 frame is
/// sent per changed field, in the order effect, brightness, speed, colour; each must echo.
/// All four values are then read back: written fields must be within [`TOLERANCE`] of
/// what was sent (effect exact), the others unchanged (`VerifyMismatch { saved: false }`
/// otherwise). `after` is that read-back. With `save`, VIA 09 03 follows and all four
/// values are read again and must equal that read-back exactly
/// (`VerifyMismatch { saved: true }`). With nothing to change and no `save`, nothing is
/// written.
///
/// If a later VIA 07 frame fails, the earlier fields stay applied in RAM; the caller must
/// re-read all four values (`read_light_state`) before it decides anything.
///
/// An effect above 24 is refused by the policy before any frame is sent (the effect is
/// the first frame).
pub fn set_lighting<L: RawLink>(
    s: &mut Session<L>,
    change: LightChange,
    save: bool,
) -> Result<LightOutcome, SessionError> {
    set_lighting_with(s, change, save.into())
}

/// [`set_lighting`] with a [`SaveMode`]: `IfChanged` sends VIA 09 03 only after a VIA 07 frame.
pub fn set_lighting_with<L: RawLink>(
    s: &mut Session<L>,
    change: LightChange,
    save: SaveMode,
) -> Result<LightOutcome, SessionError> {
    // Reads inside the sequence are sent once: the worst case is one reply timeout per frame.
    s.single_reads(|s| lighting_inner(s, change, save))
}

fn lighting_inner<L: RawLink>(
    s: &mut Session<L>,
    change: LightChange,
    save: SaveMode,
) -> Result<LightOutcome, SessionError> {
    let before = read_light_state(s)?;
    let wanted = [
        change
            .effect
            .filter(|&v| v != before.effect)
            .map(LightValue::Effect),
        change
            .brightness
            .filter(|&v| !close(v, before.brightness))
            .map(LightValue::Brightness),
        change
            .speed
            .filter(|&v| !close(v, before.speed))
            .map(LightValue::Speed),
        change
            .color
            .filter(|&(h, sat)| !(close(h, before.hue) && close(sat, before.saturation)))
            .map(|(hue, saturation)| LightValue::Color { hue, saturation }),
    ];
    let writes: Vec<LightValue> = wanted.into_iter().flatten().collect();
    if writes.is_empty() && save != SaveMode::Yes {
        return Ok(LightOutcome {
            before,
            after: before,
            saved: false,
            frames_written: 0,
            unchanged: true,
        });
    }
    let mut frames_written = 0;
    for &value in &writes {
        let policy = s.policy_with(Tier::Volatile);
        s.request_with(&policy, Request::SetLight { value })?;
        frames_written += 1;
    }
    let mut after = before;
    if !writes.is_empty() {
        // Written fields within tolerance of what was sent, the others exactly as before.
        after = read_light_state(s)?;
        let mut want = before;
        for &value in &writes {
            apply(&mut want, value);
        }
        let ok = writes.iter().all(|&w| accepts(w, field(&after, w.id())))
            && [
                LightId::Brightness,
                LightId::Effect,
                LightId::Speed,
                LightId::Color,
            ]
            .into_iter()
            .filter(|id| !writes.iter().any(|w| w.id() == *id))
            .all(|id| field(&after, id) == field(&before, id));
        if !ok {
            return Err(mismatch(&want, &after, false));
        }
    }
    let mut saved = false;
    if save != SaveMode::No {
        let policy = s.policy_with(Tier::Persistent);
        s.request_with(&policy, Request::SaveLighting)?;
        frames_written += 1;
        saved = true;
        // Exact against the read-back after the VIA 07 writes (what the keyboard holds).
        let held = after;
        after = read_light_state(s)?;
        if after != held {
            return Err(mismatch(&held, &after, true));
        }
    }
    Ok(LightOutcome {
        before,
        after,
        saved,
        frames_written,
        unchanged: false,
    })
}
