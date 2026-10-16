//! Read-only snapshot of everything v1 reads from the keyboard.
//!
//! Failure policy per section: a timeout or an undecodable reply on an optional section
//! gives `None` and a note in [`Snapshot::notes`], and the read goes on. VIA 01, A0,
//! A9 01, A9 10 and the profile buffers are required: any failure fails the snapshot.
//! Foreign traffic, back-off, link and policy errors always fail it. The macro buffer
//! is not read in v1.

use std::time::Instant;

use keychron_proto::Request::{self, *};
use keychron_proto::he::{
    Curve, HeInfo, ProfileBuffer, ProfilesInfo, decode_curve, decode_he_info, decode_profiles_info,
};
use keychron_proto::lighting::{
    Hsv, Indicators, LedCaps, LedRow, MixInfo, MixSlot, PerKeyMode, decode_indicators,
    decode_led_caps, decode_led_colors, decode_led_count, decode_led_row, decode_mix_info,
    decode_mix_regions, decode_mix_slots, decode_per_key_mode,
};
use keychron_proto::plan::{Keymap, KeymapPlan, ProfileBufferPlan};
use keychron_proto::system::{
    Features, KcVersion, Layer, LayoutLang, MiscInfo, Timers, decode_features, decode_firmware,
    decode_kc_version, decode_layer, decode_layout_lang, decode_misc_info, decode_timers,
};
use keychron_proto::via::{
    decode_layer_count, decode_light, decode_macro_buffer_size, decode_macro_count, decode_protocol,
};
use keychron_proto::{DecodeError, Frame, LightId, LightValue};
use serde::Serialize;

use crate::error::SessionError;
use crate::link::RawLink;
use crate::session::{Event, Session};
use crate::{MATRIX_COLS, MATRIX_ROWS};

/// VIA protocol version the keyboard must report [OBS 12].
pub const VIA_PROTOCOL: u8 = 12;

/// A section that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectionNote {
    pub section: &'static str,
    pub error: String,
}

/// VIA channel 3 values (VIA 08 03 01..04).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct LightSettings {
    pub brightness: Option<u8>,
    pub effect: Option<u8>,
    pub speed: Option<u8>,
    pub hue: Option<u8>,
    pub saturation: Option<u8>,
}

/// Mix effect slots of one region (A8 0E).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegionSlots {
    pub region: u8,
    pub slots: Vec<MixSlot>,
}

/// The A8 lighting group. Every part is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Lighting {
    pub caps: Option<LedCaps>,
    pub led_count: Option<u8>,
    /// LED index per matrix cell, one entry per matrix row (A8 06).
    pub rows: Option<Vec<LedRow>>,
    pub per_key_mode: Option<PerKeyMode>,
    /// Per-key colour of every LED (A8 09).
    pub colors: Option<Vec<Hsv>>,
    pub indicators: Option<Indicators>,
    pub mix_info: Option<MixInfo>,
    /// Mix region id of every LED (A8 0C).
    pub mix_regions: Option<Vec<u8>>,
    pub mix_slots: Option<Vec<RegionSlots>>,
}

/// Everything read from the keyboard in one pass. No serial number, no device path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub via_protocol: u8,
    pub kc: KcVersion,
    pub firmware: Option<String>,
    pub features: Option<Features>,
    pub layer: Option<Layer>,
    pub misc: Option<MiscInfo>,
    pub layout_lang: Option<LayoutLang>,
    pub timers: Option<Timers>,
    pub layer_count: Option<u8>,
    pub macro_count: Option<u8>,
    pub macro_buffer_size: Option<u16>,
    pub light: LightSettings,
    pub lighting: Lighting,
    pub he: HeInfo,
    pub profiles_info: ProfilesInfo,
    /// One parsed buffer per profile, index = profile number.
    pub profiles: Vec<ProfileBuffer>,
    pub curve: Option<Curve>,
    /// One keymap per layer.
    pub keymaps: Option<Vec<Keymap>>,
    pub elapsed_ms: u64,
    /// Frames written for this snapshot, resends included.
    pub requests: u32,
    pub notes: Vec<SectionNote>,
}

/// One request, decoded.
pub(crate) fn ask<L, T>(
    s: &mut Session<L>,
    request: Request,
    decode: impl FnOnce(&Frame) -> Result<T, DecodeError>,
) -> Result<T, SessionError>
where
    L: RawLink,
{
    let reply = s.request(request)?;
    Ok(decode(&reply)?)
}

/// Turn a recoverable failure of an optional section into `None` plus a note.
pub(crate) fn soft<T>(
    notes: &mut Vec<SectionNote>,
    section: &'static str,
    result: Result<T, SessionError>,
) -> Result<Option<T>, SessionError> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(e @ (SessionError::Timeout | SessionError::Decode(_))) => {
            notes.push(SectionNote {
                section,
                error: e.to_string(),
            });
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn skipped(notes: &mut Vec<SectionNote>, section: &'static str, why: &str) {
    notes.push(SectionNote {
        section,
        error: format!("skipped: {why}"),
    });
}

/// Run every request of a plan and collect the replies in order.
fn run_all<L: RawLink>(
    s: &mut Session<L>,
    requests: &[Request],
) -> Result<Vec<Frame>, SessionError> {
    requests.iter().map(|r| s.request(*r)).collect()
}

/// `(start, count)` chunks of at most `max` over `total` items.
fn chunks(total: u8, max: u8) -> impl Iterator<Item = (u8, u8)> {
    (0..total)
        .step_by(usize::from(max))
        .map(move |start| (start, max.min(total - start)))
}

fn read_led_colors<L: RawLink>(s: &mut Session<L>, leds: u8) -> Result<Vec<Hsv>, SessionError> {
    let mut out = Vec::with_capacity(usize::from(leds));
    for (start, count) in chunks(leds, 9) {
        out.extend(
            ask(s, GetLedColors { start, count }, |r| {
                decode_led_colors(r, count)
            })?
            .colors,
        );
    }
    Ok(out)
}

fn read_mix_regions<L: RawLink>(s: &mut Session<L>, leds: u8) -> Result<Vec<u8>, SessionError> {
    let mut out = Vec::with_capacity(usize::from(leds));
    for (start, count) in chunks(leds, 29) {
        out.extend(
            ask(s, GetMixRegions { start, count }, |r| {
                decode_mix_regions(r, count)
            })?
            .regions,
        );
    }
    Ok(out)
}

fn read_mix_slots<L: RawLink>(
    s: &mut Session<L>,
    info: MixInfo,
) -> Result<Vec<RegionSlots>, SessionError> {
    let mut out = Vec::new();
    for region in 0..info.regions {
        let mut slots = Vec::new();
        for (start, count) in chunks(info.slots_per_region, 3) {
            let req = GetMixSlots {
                region,
                start,
                count,
            };
            slots.extend(ask(s, req, |r| decode_mix_slots(r, count))?.slots);
        }
        out.push(RegionSlots { region, slots });
    }
    Ok(out)
}

fn read_rows<L: RawLink>(s: &mut Session<L>) -> Result<Vec<LedRow>, SessionError> {
    (0..MATRIX_ROWS)
        .map(|row| {
            ask(s, GetLedRow { row }, |r| {
                decode_led_row(r, usize::from(MATRIX_COLS))
            })
        })
        .collect()
}

fn read_keymaps<L: RawLink>(s: &mut Session<L>, layers: u8) -> Result<Vec<Keymap>, SessionError> {
    (0..layers)
        .map(|layer| {
            let plan = KeymapPlan::new(layer, MATRIX_ROWS, MATRIX_COLS)?;
            let replies = run_all(s, plan.requests())?;
            Ok(plan.assemble(&replies)?)
        })
        .collect()
}

fn read_lighting<L: RawLink>(
    s: &mut Session<L>,
    notes: &mut Vec<SectionNote>,
) -> Result<Lighting, SessionError> {
    let mut l = Lighting {
        caps: soft(notes, "A8 01", ask(s, GetLedCaps, decode_led_caps))?,
        ..Lighting::default()
    };
    l.led_count = soft(
        notes,
        "A8 05",
        ask(s, GetLedCount, decode_led_count).map(|c| c.count),
    )?;
    if let Some(count) = l.led_count {
        s.set_limits(|p| p.with_led_count(count));
    }
    l.rows = soft(notes, "A8 06", read_rows(s))?;
    l.per_key_mode = soft(notes, "A8 07", ask(s, GetPerKeyMode, decode_per_key_mode))?;
    match l.led_count {
        Some(n) => l.colors = soft(notes, "A8 09", read_led_colors(s, n))?,
        None => skipped(notes, "A8 09", "LED count unknown"),
    }
    l.indicators = soft(notes, "A8 03", ask(s, GetIndicators, decode_indicators))?;
    l.mix_info = soft(notes, "A8 0B", ask(s, GetMixInfo, decode_mix_info))?;
    match l.led_count {
        Some(n) => l.mix_regions = soft(notes, "A8 0C", read_mix_regions(s, n))?,
        None => skipped(notes, "A8 0C", "LED count unknown"),
    }
    if let Some(info) = l.mix_info {
        l.mix_slots = soft(notes, "A8 0E", read_mix_slots(s, info))?;
    }
    Ok(l)
}

fn read_light<L: RawLink>(
    s: &mut Session<L>,
    notes: &mut Vec<SectionNote>,
) -> Result<LightSettings, SessionError> {
    let mut out = LightSettings::default();
    for (section, id) in [
        ("VIA 08 03 01", LightId::Brightness),
        ("VIA 08 03 02", LightId::Effect),
        ("VIA 08 03 03", LightId::Speed),
        ("VIA 08 03 04", LightId::Color),
    ] {
        match soft(
            notes,
            section,
            ask(s, GetLight { id }, |r| decode_light(r, id)),
        )? {
            Some(LightValue::Brightness(v)) => out.brightness = Some(v),
            Some(LightValue::Effect(v)) => out.effect = Some(v),
            Some(LightValue::Speed(v)) => out.speed = Some(v),
            Some(LightValue::Color { hue, saturation }) => {
                out.hue = Some(hue);
                out.saturation = Some(saturation);
            }
            None => {}
        }
    }
    Ok(out)
}

/// Read everything. Sends only Read-tier requests, one at a time.
pub fn read_snapshot<L: RawLink>(s: &mut Session<L>) -> Result<Snapshot, SessionError> {
    let started = Instant::now();
    let sent_before = s.frames_sent();
    let mut notes = Vec::new();
    // The matrix is not queryable over VIA; it comes from the keyboard definition.
    s.set_limits(|p| p.with_matrix(MATRIX_ROWS, MATRIX_COLS));

    let via_protocol = ask(s, GetViaProtocol, decode_protocol)?;
    if via_protocol != VIA_PROTOCOL {
        return Err(SessionError::UnsupportedVia(via_protocol));
    }
    let kc = ask(s, GetKcVersion, decode_kc_version)?;
    let features = soft(
        &mut notes,
        "A2",
        ask(s, GetFeatures, |r| decode_features(r, kc.instruction_set)),
    )?;
    let layer = soft(&mut notes, "A3", ask(s, GetLayer, decode_layer))?;
    let firmware = soft(&mut notes, "A1", ask(s, GetFirmware, decode_firmware))?;
    let misc = soft(&mut notes, "A7 01", ask(s, GetMiscInfo, decode_misc_info))?;
    let layout_lang = soft(
        &mut notes,
        "A7 03",
        ask(s, GetLayoutLang, decode_layout_lang),
    )?;
    let timers = soft(&mut notes, "A7 0B", ask(s, GetTimers, decode_timers))?;

    let layer_count = soft(
        &mut notes,
        "VIA 11",
        ask(s, GetLayerCount, decode_layer_count),
    )?;
    if let Some(n) = layer_count {
        s.set_limits(|p| p.with_layer_count(n));
    }
    let macro_count = soft(
        &mut notes,
        "VIA 0C",
        ask(s, GetMacroCount, decode_macro_count),
    )?;
    let macro_buffer_size = soft(
        &mut notes,
        "VIA 0D",
        ask(s, GetMacroBufferSize, decode_macro_buffer_size),
    )?;
    if let Some(n) = macro_buffer_size {
        s.set_limits(|p| p.with_macro_buffer_size(n));
    }

    let lighting = read_lighting(s, &mut notes)?;
    let light = read_light(s, &mut notes)?;

    let he = match ask(s, GetHeVersion, decode_he_info) {
        Err(SessionError::Decode(DecodeError::UnsupportedHeVersion(v))) => {
            return Err(SessionError::UnsupportedHeVersion(v));
        }
        other => other?,
    };
    let profiles_info = ask(s, GetProfilesInfo, decode_profiles_info)?;
    s.set_limits(|p| {
        p.with_profile_count(profiles_info.count)
            .with_profile_buffer_size(profiles_info.size)
    });
    let mut profiles = Vec::with_capacity(usize::from(profiles_info.count));
    for profile in 0..profiles_info.count {
        let plan = ProfileBufferPlan::new(profile, profiles_info.size)?;
        let replies = run_all(s, plan.requests())?;
        let bytes = plan.assemble(&replies)?;
        profiles.push(ProfileBuffer::parse(
            &bytes,
            MATRIX_ROWS,
            MATRIX_COLS,
            profiles_info.dks_count,
            profiles_info.snap_count,
        )?);
    }
    let curve = soft(&mut notes, "A9 20", ask(s, GetCurve, decode_curve))?;

    let keymaps = match layer_count {
        Some(n) => soft(&mut notes, "keymap", read_keymaps(s, n))?,
        None => {
            skipped(&mut notes, "keymap", "layer count unknown");
            None
        }
    };

    Ok(Snapshot {
        via_protocol,
        kc,
        firmware,
        features,
        layer,
        misc,
        layout_lang,
        timers,
        layer_count,
        macro_count,
        macro_buffer_size,
        light,
        lighting,
        he,
        profiles_info,
        profiles,
        curve,
        keymaps,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        requests: s.frames_sent() - sent_before,
        notes,
    })
}

impl Snapshot {
    /// Fold an unsolicited report into the cached values. `true` when something changed.
    pub fn apply_event(&mut self, event: Event) -> bool {
        match event {
            Event::ProfileChanged { profile } => {
                let changed = self.profiles_info.current != profile;
                self.profiles_info.current = profile;
                changed
            }
            Event::EffectChanged { effect } => {
                let changed = self.light.effect != Some(effect);
                self.light.effect = Some(effect);
                changed
            }
        }
    }
}

/// Light re-read for the UI: the current HE profile (A9 10) and the VIA channel 3
/// lighting values (08 03 01..04). Read-tier requests only; the cached snapshot is
/// patched in place. A lighting value that times out keeps its cached value. The profile
/// info is required: its failure is returned and nothing is patched.
pub fn refresh_state<L: RawLink>(
    s: &mut Session<L>,
    snapshot: &mut Snapshot,
) -> Result<(), SessionError> {
    let info = ask(s, GetProfilesInfo, decode_profiles_info)?;
    let mut notes = Vec::new();
    let fresh = read_light(s, &mut notes)?;
    snapshot.profiles_info.current = info.current;
    let light = &mut snapshot.light;
    light.brightness = fresh.brightness.or(light.brightness);
    light.effect = fresh.effect.or(light.effect);
    light.speed = fresh.speed.or(light.speed);
    light.hue = fresh.hue.or(light.hue);
    light.saturation = fresh.saturation.or(light.saturation);
    Ok(())
}
