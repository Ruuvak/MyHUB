//! Probe for the Keychron K2 HE over USB raw HID. Read-only except the `set` commands.
//!
//! Usage:
//! `kc_probe list`
//! `kc_probe state [--json] [--trace]`
//! `kc_probe watch <seconds>`
//! `kc_probe profile`
//! `kc_probe travel`
//! `kc_probe set profile <1..3> [--save]`
//! `kc_probe set travel --mode static|rt --actuation <mm> [--press <mm>] [--release <mm>] [--save]`
//! `kc_probe set light [--effect N] [--brightness 0..255] [--speed 0..255] [--hue 0..255 --sat 0..255] [--save]`
//! `kc_probe set key --layer <0..3> --row <0..5> --col <0..15> --code <0x004C or KC_DEL or RGB_MOD>`
//! `kc_probe link [--trace]`
//!
//! * `travel`: read only. Prints the CURRENT profile's global mode, actuation and (for
//!   rapid trigger) press and release in mm. Sends A9 10 and one A9 12.
//! * `set travel`: edits the global travel word of the CURRENT profile only (A9 14, RAM).
//!   Millimetres with one decimal (0.1 mm steps): actuation 0.2..3.8, press and release
//!   0.1..3.8; checked before the keyboard is opened. `static` keeps the profile's stored
//!   press and release (`--press`/`--release` are ignored with a note); `rt` takes them
//!   from the flags, or from the current value when a flag is absent. Prints before,
//!   after, saved and frames written. Nothing is sent when the values already hold and
//!   `--save` is absent. `--save` also sends A9 1F, which WRITES FLASH.
//! * `set key`: VIA 05 sets ONE key of the keymap and WRITES FLASH at once [INF: VIA 05
//!   persists immediately, there is no save step]. Layer, row and column are 0-based (layer
//!   0..3, row 0..5, column 0..15); `--code` is a hex number or a QMK name (`KC_DEL`,
//!   `RGB_MOD`, `KC_NO`, `KC_TRNS`). Only basic keys, KC_NO, KC_TRNS, RGB and named
//!   Keychron keys are accepted; the QMK 0x7C00 block (boot, reboot, clear EEPROM) and any
//!   unknown code is refused before a byte is sent. Reads the key, writes nothing when it
//!   already holds the code, else sends one VIA 05 frame and reads the key back. Prints
//!   before and after as hex and name.
//! * `set light`: VIA 07 03 per changed field (RAM), in the order effect, brightness,
//!   speed, colour; effect 0..24; `--hue` and `--sat` go together. Prints before, after,
//!   saved and frames written. `--save` also sends VIA 09 03, which WRITES FLASH.
//!
//! * `list`: the HID interfaces with the keyboard's VID and PID, then those of the
//!   Keychron Link receiver (PID 0xD030): interface number, usage page, usage, product
//!   string, and how many of them match the raw HID rule (usage page 0xFF60, usage
//!   0x61). Opens nothing, sends nothing. No serial number and no device path is read or
//!   printed. Exit code 1 only when neither has a raw HID interface.
//! * `link [--trace]`: opens the receiver (not the keyboard) and sends B1, B2 and B3 once
//!   each: receiver protocol, flags, firmware and the linked keyboard. Read only; nothing
//!   else can be sent over a receiver session.
//! * `state`: opens the first matching interface and reads the full snapshot. Prints a
//!   human summary, or pretty JSON with `--json`. `--trace` prints every frame to stderr
//!   with its direction and, for input reports, the length the backend returned (32, or
//!   33 when a report ID came with it).
//! * `watch <seconds>`: opens the interface and only listens: it sends nothing. Prints
//!   unsolicited reports (profile and effect changes) with timestamps, and the first
//!   bytes of any other report (another program is talking to the keyboard).
//! * `profile`: one A9 10 read; prints the current profile (1-based). No snapshot.
//! * `set profile <n> [--save]`: makes profile n (1-based) the active HE profile (A9 11,
//!   RAM), reads it back, prints before, after and whether it was saved. `--save` also
//!   sends A9 1F, which WRITES FLASH. Nothing is sent when n is already current and
//!   `--save` is absent.
//!
//! WRITE COMMANDS REQUIRE EXPLICIT USER APPROVAL for each run. Never run `set` on your
//! own. A timeout on a write means the state is unknown: run `profile` before anything
//! else. Likewise after any other error that follows a write (ambiguous reply, foreign
//! traffic, bad report, link error).
//!
//! What it sends: `list`, `state`, `watch`, `profile` and `travel` send only Read-tier
//! requests (the stored session policy is read-only). `set profile` also sends A9 11,
//! `set travel` A9 14, `set light` VIA 07 03, `set key` VIA 05 (Persistent), and with `--save` A9 1F or VIA 09 03, each
//! through a policy that opens only that tier for that call. A write that fails to read
//! back as written (or times out) leaves the state unknown: run `travel` or `state`. It
//! stops with an error and sends nothing more when it sees foreign traffic (the launcher
//! is open); close the launcher first.
//!
//! Exit codes: 0 ok, 1 no keyboard found, 2 bad arguments, 3 failure.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use keychron_k2he::{
    DEFAULT_SENSITIVITY, DISPLAY_NAME, FrameDirection, Incoming, InterfaceInfo, LINK_PID,
    LightChange, LightState, LinkSnapshot, MATRIX_COLS, MATRIX_ROWS, Session, SessionError,
    Snapshot, TravelSettings, list_interfaces, list_receiver_interfaces, read_global_word,
    read_link, read_profiles_info, read_snapshot, select_profile, set_keycode, set_lighting,
    set_travel_global,
};
use keychron_proto::TravelMode;
use keychron_proto::he::{KeyMode, KeyWord};
use keychron_proto::keycode::{Keycode, check_writable, parse_code};
use keychron_proto::request::{ACTUATION_RANGE, EFFECT_MAX, SENSITIVITY_RANGE};

enum Command {
    List,
    Link {
        trace: bool,
    },
    State {
        json: bool,
        trace: bool,
    },
    Watch(u64),
    Profile,
    Travel,
    SetProfile {
        profile: u8,
        save: bool,
    },
    SetTravel {
        mode: TravelMode,
        actuation: u8,
        press: Option<u8>,
        release: Option<u8>,
        save: bool,
    },
    SetLight {
        change: LightChange,
        save: bool,
    },
    SetKey {
        layer: u8,
        row: u8,
        col: u8,
        code: u16,
    },
}

/// Millimetres with at most one decimal to 0.1 mm units, within `(min, max)`.
fn parse_mm(text: &str, (min, max): (u8, u8)) -> Option<u8> {
    let v: f64 = text.parse().ok().filter(|v: &f64| v.is_finite())?;
    let tenths = (v * 10.0).round();
    if (v * 10.0 - tenths).abs() > 1e-6 || tenths < f64::from(min) || tenths > f64::from(max) {
        return None;
    }
    Some(tenths as u8)
}

fn parse_set_travel(mut args: impl Iterator<Item = String>) -> Option<Command> {
    let (mut mode, mut actuation, mut press, mut release, mut save) =
        (None, None, None, None, false);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--save" => save = true,
            "--mode" => {
                mode = Some(match args.next()?.as_str() {
                    "static" => TravelMode::Static,
                    "rt" => TravelMode::RapidTrigger,
                    _ => return None,
                })
            }
            "--actuation" => actuation = Some(parse_mm(&args.next()?, ACTUATION_RANGE)?),
            "--press" => press = Some(parse_mm(&args.next()?, SENSITIVITY_RANGE)?),
            "--release" => release = Some(parse_mm(&args.next()?, SENSITIVITY_RANGE)?),
            _ => return None,
        }
    }
    Some(Command::SetTravel {
        mode: mode?,
        actuation: actuation?,
        press,
        release,
        save,
    })
}

fn parse_set_light(mut args: impl Iterator<Item = String>) -> Option<Command> {
    let mut change = LightChange::default();
    let (mut hue, mut sat, mut save) = (None, None, false);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--save" => save = true,
            "--effect" => {
                change.effect = Some(args.next()?.parse().ok().filter(|&e| e <= EFFECT_MAX)?)
            }
            "--brightness" => change.brightness = Some(args.next()?.parse().ok()?),
            "--speed" => change.speed = Some(args.next()?.parse().ok()?),
            "--hue" => hue = Some(args.next()?.parse().ok()?),
            "--sat" => sat = Some(args.next()?.parse().ok()?),
            _ => return None,
        }
    }
    change.color = match (hue, sat) {
        (None, None) => None,
        (Some(h), Some(s)) => Some((h, s)),
        _ => return None,
    };
    Some(Command::SetLight { change, save })
}

fn parse_set_key(mut args: impl Iterator<Item = String>) -> Option<Command> {
    let (mut layer, mut row, mut col, mut code) = (None, None, None, None);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--layer" => layer = Some(args.next()?.parse::<u8>().ok().filter(|&v| v < 4)?),
            "--row" => {
                row = Some(
                    args.next()?
                        .parse::<u8>()
                        .ok()
                        .filter(|&v| v < MATRIX_ROWS)?,
                )
            }
            "--col" => {
                col = Some(
                    args.next()?
                        .parse::<u8>()
                        .ok()
                        .filter(|&v| v < MATRIX_COLS)?,
                )
            }
            "--code" => {
                let text = args.next()?;
                let Some(c) = parse_code(&text) else {
                    eprintln!("unknown keycode {text:?} (hex like 0x004C or a name like KC_DEL)");
                    return None;
                };
                if let Err(reason) = check_writable(c) {
                    eprintln!("keycode {c:#06x} refused: {reason}");
                    return None;
                }
                code = Some(c);
            }
            _ => return None,
        }
    }
    Some(Command::SetKey {
        layer: layer?,
        row: row?,
        col: col?,
        code: code?,
    })
}

fn parse_args() -> Option<Command> {
    let mut args = std::env::args().skip(1);
    match args.next()?.as_str() {
        "list" => args.next().is_none().then_some(Command::List),
        "state" => {
            let (mut json, mut trace) = (false, false);
            for a in args {
                match a.as_str() {
                    "--json" => json = true,
                    "--trace" => trace = true,
                    _ => return None,
                }
            }
            Some(Command::State { json, trace })
        }
        "link" => {
            let mut trace = false;
            for a in args {
                match a.as_str() {
                    "--trace" => trace = true,
                    _ => return None,
                }
            }
            Some(Command::Link { trace })
        }
        "watch" => {
            let seconds = args.next()?.parse().ok()?;
            args.next().is_none().then_some(Command::Watch(seconds))
        }
        "profile" => args.next().is_none().then_some(Command::Profile),
        "travel" => args.next().is_none().then_some(Command::Travel),
        "set" => {
            match args.next()?.as_str() {
                "profile" => {}
                "travel" => return parse_set_travel(args),
                "light" => return parse_set_light(args),
                "key" => return parse_set_key(args),
                _ => return None,
            }
            let number: u8 = args.next()?.parse().ok()?;
            let save = match args.next().as_deref() {
                None => false,
                Some("--save") => true,
                Some(_) => return None,
            };
            if !(1..=3).contains(&number) {
                return None;
            }
            let profile = number - 1;
            args.next()
                .is_none()
                .then_some(Command::SetProfile { profile, save })
        }
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn trace(direction: FrameDirection, bytes: &[u8]) {
    let label = match direction {
        FrameDirection::Tx => "TX",
        FrameDirection::Rx => "RX",
    };
    eprintln!("{label} len {:>2}: {}", bytes.len(), hex(bytes));
}

fn exit_for(e: &SessionError) -> ExitCode {
    match e {
        SessionError::NotFound
        | SessionError::ReceiverNotFound
        | SessionError::UsagePageUnavailable => ExitCode::from(1),
        _ => ExitCode::from(3),
    }
}

fn main() -> ExitCode {
    let Some(command) = parse_args() else {
        eprintln!(
            "usage: kc_probe list | state [--json] [--trace] | watch <seconds> | profile | travel | set profile <1..3> [--save] | set travel --mode static|rt --actuation <mm> [--press <mm>] [--release <mm>] [--save] | set light [--effect 0..24] [--brightness 0..255] [--speed 0..255] [--hue 0..255 --sat 0..255] [--save] | set key --layer <0..3> --row <0..5> --col <0..15> --code <hex or KC_/RGB_ name> (layer, row, col 0-based) | link [--trace]"
        );
        return ExitCode::from(2);
    };
    match command {
        Command::List => list(),
        Command::Link { trace: t } => run_with(Session::open_receiver, link, t),
        Command::State { json, trace: t } => run(|s| state(s, json), t),
        Command::Watch(seconds) => run(|s| watch(s, seconds), false),
        Command::Profile => run(current_profile, false),
        Command::Travel => run(travel, false),
        Command::SetProfile { profile, save } => run(|s| set_profile(s, profile, save), false),
        Command::SetTravel {
            mode,
            actuation,
            press,
            release,
            save,
        } => run(
            |s| set_travel(s, mode, actuation, press, release, save),
            false,
        ),
        Command::SetLight { change, save } => run(|s| set_light(s, change, save), false),
        Command::SetKey {
            layer,
            row,
            col,
            code,
        } => run(|s| set_key(s, layer, row, col, code), false),
    }
}

/// Prints one device's interfaces; the number matching the raw HID rule, or `None` when
/// the enumeration failed.
fn print_interfaces(found: &Result<Vec<InterfaceInfo>, SessionError>, what: &str) -> Option<usize> {
    let found = match found {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return None;
        }
    };
    for i in found {
        println!(
            "interface {:>2}  usage page {:#06x}  usage {:#04x}  product {:?}{}",
            i.interface,
            i.usage_page,
            i.usage,
            i.product.as_deref().unwrap_or("?"),
            if i.matched { "  <- raw HID" } else { "" }
        );
    }
    let matching = found.iter().filter(|i| i.matched).count();
    println!(
        "{} HID entries with the {what} VID/PID, {matching} matching the raw HID rule",
        found.len()
    );
    Some(matching)
}

fn list() -> ExitCode {
    let keyboard = print_interfaces(&list_interfaces(), DISPLAY_NAME);
    let receiver = print_interfaces(
        &list_receiver_interfaces(),
        &format!("Keychron Link receiver ({LINK_PID:#06x})"),
    );
    match (keyboard, receiver) {
        (None, _) | (_, None) => ExitCode::from(3),
        (Some(0), Some(0)) => {
            eprintln!("no raw HID interface found (keyboard on the cable or receiver plugged in?)");
            ExitCode::from(1)
        }
        _ => ExitCode::SUCCESS,
    }
}

fn run(
    f: impl FnOnce(&mut Session<keychron_k2he::HidapiLink>) -> Result<(), SessionError>,
    with_trace: bool,
) -> ExitCode {
    run_with(Session::open, f, with_trace)
}

fn run_with(
    open: fn() -> Result<Session<keychron_k2he::HidapiLink>, SessionError>,
    f: impl FnOnce(&mut Session<keychron_k2he::HidapiLink>) -> Result<(), SessionError>,
    with_trace: bool,
) -> ExitCode {
    let mut session = match open() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("open failed: {e}");
            return exit_for(&e);
        }
    };
    if session.matched_interfaces() > 1 {
        eprintln!(
            "{} interfaces match, using the first",
            session.matched_interfaces()
        );
    }
    if with_trace {
        session.set_frame_trace(trace);
    }
    match f(&mut session) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("failed: {e}");
            exit_for(&e)
        }
    }
}

fn state(session: &mut Session<keychron_k2he::HidapiLink>, json: bool) -> Result<(), SessionError> {
    let snapshot = read_snapshot(session)?;
    if json {
        match serde_json::to_string_pretty(&snapshot) {
            Ok(text) => println!("{text}"),
            Err(e) => eprintln!("could not serialize the snapshot: {e}"),
        }
    } else {
        summary(&snapshot);
    }
    Ok(())
}

fn link(session: &mut Session<keychron_k2he::HidapiLink>) -> Result<(), SessionError> {
    let snapshot = read_link(session)?;
    link_summary(&snapshot);
    Ok(())
}

fn link_summary(s: &LinkSnapshot) {
    match &s.info {
        Some(i) => println!(
            "receiver protocol {}, flags {:#04x} (state notify {}, multi device {}, mouse driver {}, gamepad input disable {}), byte 4 {}",
            i.version,
            i.flags.raw,
            i.flags.state_notify_over_via,
            i.flags.multi_device,
            i.flags.mouse_driver_over_via,
            i.flags.via_disable_gamepad_input,
            i.byte4
        ),
        None => println!("receiver protocol: no answer"),
    }
    println!(
        "receiver firmware {}",
        s.firmware.as_deref().unwrap_or("no answer")
    );
    let st = &s.state;
    if st.is_k2he() {
        println!("{DISPLAY_NAME} connected over 2.4 GHz");
    } else if st.connected {
        println!(
            "other device linked: {:04x}:{:04x}",
            st.linked_vid, st.linked_pid
        );
    } else {
        println!("no keyboard linked");
    }
    println!(
        "link state: connected {}, linked {:04x}:{:04x}, byte 1 {}",
        st.connected, st.linked_vid, st.linked_pid, st.raw_byte1
    );
    for n in &s.notes {
        println!("note: {}: {}", n.section, n.error);
    }
    println!("{} requests", s.requests);
}

fn current_profile(session: &mut Session<keychron_k2he::HidapiLink>) -> Result<(), SessionError> {
    let info = read_profiles_info(session)?;
    println!(
        "current profile {} of {}",
        u16::from(info.current) + 1,
        info.count
    );
    Ok(())
}

fn set_profile(
    session: &mut Session<keychron_k2he::HidapiLink>,
    profile: u8,
    save: bool,
) -> Result<(), SessionError> {
    let out = select_profile(session, profile, save)?;
    println!(
        "before {}, after {}, saved {}, frames written {}{}",
        u16::from(out.before) + 1,
        u16::from(out.after) + 1,
        out.saved,
        out.frames_written,
        if out.unchanged {
            " (already current, nothing written)"
        } else {
            ""
        }
    );
    Ok(())
}

fn travel(session: &mut Session<keychron_k2he::HidapiLink>) -> Result<(), SessionError> {
    let profile = read_profiles_info(session)?.current;
    let w = read_global_word(session, profile)?;
    println!("profile {}: {}", u16::from(profile) + 1, word(&w));
    Ok(())
}

fn set_travel(
    session: &mut Session<keychron_k2he::HidapiLink>,
    mode: TravelMode,
    actuation: u8,
    press: Option<u8>,
    release: Option<u8>,
    save: bool,
) -> Result<(), SessionError> {
    let (press, release) = match mode {
        TravelMode::Static => {
            if press.is_some() || release.is_some() {
                println!("note: --press and --release are ignored in static mode");
            }
            // Ignored by the device layer in Static; the stored values stay.
            (SENSITIVITY_RANGE.0, SENSITIVITY_RANGE.0)
        }
        TravelMode::RapidTrigger => match (press, release) {
            (Some(p), Some(r)) => (p, r),
            _ => {
                // A missing value keeps what the profile has now (read only).
                let profile = read_profiles_info(session)?.current;
                let w = read_global_word(session, profile)?;
                let keep = |given: Option<u8>, stored: u8| {
                    given.unwrap_or(if (SENSITIVITY_RANGE.0..=SENSITIVITY_RANGE.1).contains(&stored)
                    {
                        stored
                    } else {
                        println!(
                            "note: stored value {stored} is outside 1..=38, using the default {DEFAULT_SENSITIVITY} (0.4 mm)"
                        );
                        DEFAULT_SENSITIVITY
                    })
                };
                (keep(press, w.press), keep(release, w.release))
            }
        },
    };
    let settings = TravelSettings {
        mode,
        actuation,
        press,
        release,
    };
    let out = set_travel_global(session, settings, save)?;
    println!("profile {} (current)", u16::from(out.profile) + 1);
    println!("before: {}", word(&out.before));
    println!("after:  {}", word(&out.after));
    println!(
        "saved {}, frames written {}{}",
        out.saved,
        out.frames_written,
        if out.unchanged {
            " (already set, nothing written)"
        } else {
            ""
        }
    );
    Ok(())
}

fn light_line(l: &LightState) -> String {
    format!(
        "effect {}, brightness {}, speed {}, hue {}, saturation {}",
        l.effect, l.brightness, l.speed, l.hue, l.saturation
    )
}

fn set_light(
    session: &mut Session<keychron_k2he::HidapiLink>,
    change: LightChange,
    save: bool,
) -> Result<(), SessionError> {
    let out = set_lighting(session, change, save)?;
    println!("before: {}", light_line(&out.before));
    println!("after:  {}", light_line(&out.after));
    println!(
        "saved {}, frames written {}{}",
        out.saved,
        out.frames_written,
        if out.unchanged {
            " (already set, nothing written)"
        } else {
            ""
        }
    );
    Ok(())
}

/// Set when a VIA 05 frame went out (seen on the TX trace).
static KEY_WRITE_SENT: AtomicBool = AtomicBool::new(false);

fn watch_key_write(direction: FrameDirection, bytes: &[u8]) {
    if matches!(direction, FrameDirection::Tx) && bytes.first() == Some(&0x05) {
        KEY_WRITE_SENT.store(true, Ordering::SeqCst);
    }
}

fn key_label(code: u16) -> String {
    format!("{code:#06x} {}", Keycode::decode(code))
}

fn set_key(
    session: &mut Session<keychron_k2he::HidapiLink>,
    layer: u8,
    row: u8,
    col: u8,
    code: u16,
) -> Result<(), SessionError> {
    session.set_frame_trace(watch_key_write);
    let out = set_keycode(session, layer, row, col, code).inspect_err(|e| {
        let unknown = matches!(
            e,
            SessionError::Timeout
                | SessionError::AmbiguousReply
                | SessionError::ForeignTraffic
                | SessionError::BadReport(_)
                | SessionError::Link(_)
                | SessionError::VerifyMismatch { .. }
        );
        if unknown && KEY_WRITE_SENT.load(Ordering::SeqCst) {
            eprintln!("the VIA 05 frame was sent: key state unknown, run `kc_probe state`");
        }
    })?;
    println!(
        "layer {}, row {}, col {} (0-based)",
        out.layer, out.row, out.col
    );
    println!("before: {}", key_label(out.before));
    println!("after:  {}", key_label(out.after));
    println!(
        "frames written {}{}",
        out.frames_written,
        if out.unchanged {
            " (already set, nothing written)"
        } else {
            ""
        }
    );
    Ok(())
}

fn watch(
    session: &mut Session<keychron_k2he::HidapiLink>,
    seconds: u64,
) -> Result<(), SessionError> {
    println!("listening for {seconds} s, sending nothing");
    let start = Instant::now();
    let end = start + Duration::from_secs(seconds);
    while Instant::now() < end {
        let left = end.saturating_duration_since(Instant::now());
        match session.poll(left.min(Duration::from_millis(200)))? {
            Some(Incoming::Event(e)) => println!("{:>6.1}s {e:?}", start.elapsed().as_secs_f32()),
            Some(Incoming::Foreign(f)) => println!(
                "{:>6.1}s foreign report (another program is talking to the keyboard): {} ...",
                start.elapsed().as_secs_f32(),
                hex(&f[..4])
            ),
            None => {}
        }
    }
    Ok(())
}

fn mm(tenths: u8) -> String {
    format!("{:.1} mm", f32::from(tenths) / 10.0)
}

fn word(w: &KeyWord) -> String {
    match w.mode {
        KeyMode::Global => "follows global".to_owned(),
        KeyMode::Static => format!("static, actuation {}", mm(w.distance)),
        KeyMode::RapidTrigger => format!(
            "rapid trigger, actuation {}, press {}, release {}",
            mm(w.distance),
            mm(w.press),
            mm(w.release)
        ),
        KeyMode::Unknown(n) => format!("unknown mode {n}"),
    }
}

fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map_or_else(|| "?".to_owned(), |v| v.to_string())
}

fn summary(s: &Snapshot) {
    println!("{DISPLAY_NAME}");
    println!(
        "VIA protocol {}, kcVersion {}, instruction set {}, firmware {}",
        s.via_protocol,
        s.kc.kc_version,
        s.kc.instruction_set,
        s.firmware.as_deref().unwrap_or("?")
    );
    println!(
        "HE version {}, current profile {}, {} profiles x {} B, {} DKS, {} snap",
        s.he.version,
        s.profiles_info.current,
        s.profiles_info.count,
        s.profiles_info.size,
        s.profiles_info.dks_count,
        s.profiles_info.snap_count
    );
    println!(
        "layers {} (default {}), macros {} (buffer {} B)",
        opt(s.layer_count),
        opt(s.layer.map(|l| l.default_layer)),
        opt(s.macro_count),
        opt(s.macro_buffer_size)
    );
    println!("LEDs {}", opt(s.lighting.led_count));
    let l = &s.light;
    println!(
        "lighting: brightness {}, effect {}, speed {}, hue {}, saturation {}",
        opt(l.brightness),
        opt(l.effect),
        opt(l.speed),
        opt(l.hue),
        opt(l.saturation)
    );
    if let Some(t) = &s.timers {
        println!(
            "timers (s): backlight off {}, sleep {}, HE scan idle {}",
            t.backlight_off_s, t.sleep_s, t.he_scan_idle_s
        );
    }
    for (i, p) in s.profiles.iter().enumerate() {
        let name = if p.name.is_empty() { "-" } else { &p.name };
        println!("profile {i} {name:?}: {}", word(&p.global));
    }
    if let Some(c) = &s.curve {
        let pts: Vec<String> = c
            .points
            .iter()
            .map(|p| format!("({},{})", p.x, p.y))
            .collect();
        println!("curve: {}", pts.join(" "));
    }
    if let Some(k) = &s.keymaps {
        println!("keymap: {} layers read", k.len());
    }
    for n in &s.notes {
        println!("note: {}: {}", n.section, n.error);
    }
    println!("{} requests, {} ms", s.requests, s.elapsed_ms);
}
