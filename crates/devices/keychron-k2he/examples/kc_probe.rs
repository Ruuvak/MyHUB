//! Probe for the Keychron K2 HE over USB raw HID. Read-only except `set profile`.
//!
//! Usage:
//! `kc_probe list`
//! `kc_probe state [--json] [--trace]`
//! `kc_probe watch <seconds>`
//! `kc_probe profile`
//! `kc_probe set profile <1..3> [--save]`
//!
//! * `list`: the HID interfaces with the keyboard's VID and PID: interface number, usage
//!   page, usage, product string, and how many of them match the raw HID rule (usage
//!   page 0xFF60, usage 0x61). Opens nothing, sends nothing. No serial number and no
//!   device path is read or printed.
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
//! What it sends: `list`, `state`, `watch` and `profile` send only Read-tier requests
//! (the stored session policy is read-only). `set profile` also sends A9 11, and A9 1F
//! with `--save`, each through a policy that opens only that tier for that call. It
//! stops with an error and sends nothing more when it sees foreign traffic (the launcher
//! is open); close the launcher first.
//!
//! Exit codes: 0 ok, 1 no keyboard found, 2 bad arguments, 3 failure.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use keychron_k2he::{
    DISPLAY_NAME, FrameDirection, Incoming, Session, SessionError, Snapshot, list_interfaces,
    read_profiles_info, read_snapshot, select_profile,
};
use keychron_proto::he::{KeyMode, KeyWord};

enum Command {
    List,
    State { json: bool, trace: bool },
    Watch(u64),
    Profile,
    SetProfile { profile: u8, save: bool },
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
        "watch" => {
            let seconds = args.next()?.parse().ok()?;
            args.next().is_none().then_some(Command::Watch(seconds))
        }
        "profile" => args.next().is_none().then_some(Command::Profile),
        "set" => {
            if args.next()?.as_str() != "profile" {
                return None;
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
        SessionError::NotFound | SessionError::UsagePageUnavailable => ExitCode::from(1),
        _ => ExitCode::from(3),
    }
}

fn main() -> ExitCode {
    let Some(command) = parse_args() else {
        eprintln!(
            "usage: kc_probe list | state [--json] [--trace] | watch <seconds> | profile | set profile <1..3> [--save]"
        );
        return ExitCode::from(2);
    };
    match command {
        Command::List => list(),
        Command::State { json, trace: t } => run(|s| state(s, json), t),
        Command::Watch(seconds) => run(|s| watch(s, seconds), false),
        Command::Profile => run(current_profile, false),
        Command::SetProfile { profile, save } => run(|s| set_profile(s, profile, save), false),
    }
}

fn list() -> ExitCode {
    let found = match list_interfaces() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(3);
        }
    };
    for i in &found {
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
        "{} HID entries with the {DISPLAY_NAME} VID/PID, {matching} matching the raw HID rule",
        found.len()
    );
    if matching == 0 {
        eprintln!("no raw HID interface found (keyboard on the cable?)");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn run(
    f: impl FnOnce(&mut Session<keychron_k2he::HidapiLink>) -> Result<(), SessionError>,
    with_trace: bool,
) -> ExitCode {
    let mut session = match Session::open() {
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
