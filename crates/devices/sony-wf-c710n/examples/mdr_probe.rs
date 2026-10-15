//! Probe for the Sony WF-C710N over Bluetooth RFCOMM (Windows).
//!
//! Usage:
//! `mdr_probe list | state | watch <seconds>` (read only)
//! `mdr_probe set nc|ambient [--voice] [--level n] [--no-restore]`
//! `mdr_probe set eq-preset <name|id> [--no-restore]`
//! `mdr_probe set eq-bands s1,s2,s3,s4,s5,s6 [--no-restore]`
//! `mdr_probe set sidetone on|off [--no-restore]`
//! `mdr_probe set multipoint on|off --confirm-disconnect [--no-restore]`
//!
//! * `list`: paired devices whose name matches WF-C710N or that offer the MDR service.
//!   Prints the name and the masked address (last byte only). Opens no connection.
//! * `state`: connects to the first candidate, runs the handshake and prints the
//!   snapshot as pretty JSON.
//! * `watch <seconds>`: connects, runs the handshake, then prints decoded notifications
//!   as they arrive for the given time. Every frame that needs an ACK gets one.
//! * `set ...`: WRITES TO THE HEADSET. Run it only with the user's approval. It reads the
//!   state, changes one setting (the write path verifies the value with a GET), prints
//!   before and after, then puts the original value back and reads it once more.
//!   `--no-restore` skips the restore. The restore also runs when the write failed.
//!   Multipoint drops the link: it needs `--confirm-disconnect`, reconnects by itself and
//!   restores after the reconnect.
//!
//! What the read commands send: only read-tier requests that pass `mdr::Policy::new()`
//! and ACK frames. `set` additionally sends the SET frames of one setting through a policy
//! that holds only the gate that setting needs. It never requests the capability info
//! (headset address), and never prints a full address, a paired-device list or track
//! metadata.
//!
//! Exit codes: 0 ok, 1 no headset found, 2 bad arguments, 3 failure.

use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use mdr::messages::{AmbientMode, EqParam, NcAsmMode, NcAsmParam, eq_preset_name};
use myhub_rfcomm::RfcommLink;
use sony_wf_c710n::{
    Change, MultipointOutcome, Session, SessionError, Snapshot, WriteError, candidates, describe,
    handshake, open, read_snapshot,
};

type Live = Session<RfcommLink>;

const CUSTOM: u8 = 0xA0;

enum Setter {
    NcAsm {
        mode: NcAsmMode,
        voice: bool,
        level: Option<u8>,
    },
    EqPreset(u8),
    EqBands(Vec<u8>),
    Sidetone(bool),
    Multipoint(bool),
}

struct SetArgs {
    setter: Setter,
    restore: bool,
}

enum Command {
    List,
    State,
    Watch(u64),
    Set(SetArgs),
}

fn parse_preset(text: &str) -> Option<u8> {
    if let Some(hex) = text.strip_prefix("0x") {
        return u8::from_str_radix(hex, 16).ok();
    }
    text.parse().ok().or_else(|| {
        (0..=u8::MAX).find(|id| eq_preset_name(*id).is_some_and(|n| n.eq_ignore_ascii_case(text)))
    })
}

fn parse_on_off(text: &str) -> Option<bool> {
    match text {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

fn parse_set(args: Vec<String>) -> Option<SetArgs> {
    let mut args = args.into_iter();
    let which = args.next()?;
    let mut restore = true;
    let mut confirm = false;
    let mut voice = false;
    let mut level = None;
    let mut positional = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-restore" => restore = false,
            "--confirm-disconnect" => confirm = true,
            "--voice" => voice = true,
            "--level" => level = Some(args.next()?.parse().ok()?),
            _ if positional.is_none() && !a.starts_with("--") => positional = Some(a),
            _ => return None,
        }
    }
    let setter = match which.as_str() {
        "nc" => Setter::NcAsm {
            mode: NcAsmMode::NoiseCancelling,
            voice,
            level: None,
        },
        "ambient" => Setter::NcAsm {
            mode: NcAsmMode::Ambient,
            voice,
            level,
        },
        "eq-preset" => Setter::EqPreset(parse_preset(&positional?)?),
        "eq-bands" => Setter::EqBands(
            positional?
                .split(',')
                .map(|s| s.trim().parse().ok())
                .collect::<Option<Vec<u8>>>()?,
        ),
        "sidetone" => Setter::Sidetone(parse_on_off(&positional?)?),
        "multipoint" if confirm => Setter::Multipoint(parse_on_off(&positional?)?),
        _ => return None,
    };
    Some(SetArgs { setter, restore })
}

fn parse_args() -> Option<Command> {
    let mut args = std::env::args().skip(1);
    let command = match args.next()?.as_str() {
        "list" => Command::List,
        "state" => Command::State,
        "watch" => Command::Watch(args.next()?.parse().ok()?),
        "set" => return Some(Command::Set(parse_set(args.collect())?)),
        _ => return None,
    };
    args.next().is_none().then_some(command)
}

fn connect() -> Result<Result<Live, SessionError>, String> {
    let found = candidates().map_err(|e| e.to_string())?;
    let Some((device, _)) = found.into_iter().next() else {
        return Err("no paired WF-C710N found".to_string());
    };
    println!("connecting to {} ({})", device.name, device.address());
    Ok(open(&device))
}

fn main() -> ExitCode {
    let Some(command) = parse_args() else {
        eprintln!("usage: mdr_probe list | state | watch <seconds>");
        eprintln!("       mdr_probe set nc|ambient [--voice] [--level n] [--no-restore]");
        eprintln!("       mdr_probe set eq-preset <name|id> [--no-restore]");
        eprintln!("       mdr_probe set eq-bands s1,..,s6 [--no-restore]");
        eprintln!("       mdr_probe set sidetone on|off [--no-restore]");
        eprintln!("       mdr_probe set multipoint on|off --confirm-disconnect [--no-restore]");
        return ExitCode::from(2);
    };
    if let Command::List = command {
        return match candidates() {
            Ok(found) if found.is_empty() => {
                eprintln!("no paired WF-C710N found");
                ExitCode::from(1)
            }
            Ok(found) => {
                for (device, why) in found {
                    println!(
                        "{}  {}  name match: {}, MDR service listed: {}",
                        device.name,
                        device.address(),
                        why.by_name,
                        why.by_service
                    );
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(3)
            }
        };
    }
    let mut session = match connect() {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("connect failed: {e}");
            return ExitCode::from(3);
        }
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(1);
        }
    };
    let result = match command {
        Command::State => state(&mut session),
        Command::Watch(seconds) => watch(&mut session, seconds),
        Command::Set(args) => set(session, args),
        Command::List => Ok(()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("failed: {e}");
            ExitCode::from(3)
        }
    }
}

fn state(session: &mut Live) -> Result<(), String> {
    let snapshot = read_snapshot(session).map_err(|e| e.to_string())?;
    match serde_json::to_string_pretty(&snapshot) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("could not serialize the snapshot: {e}"),
    }
    Ok(())
}

fn watch(session: &mut Live, seconds: u64) -> Result<(), String> {
    let identity = handshake(session).map_err(|e| e.to_string())?;
    println!("{} firmware {}", identity.model, identity.firmware);
    for note in session.take_notifications() {
        println!("{}", describe(&note));
    }
    let start = Instant::now();
    let end = start + Duration::from_secs(seconds);
    while Instant::now() < end {
        session.poll(200).map_err(|e| e.to_string())?;
        for note in session.take_notifications() {
            println!(
                "{:>6.1}s {}",
                start.elapsed().as_secs_f32(),
                describe(&note)
            );
        }
    }
    Ok(())
}

fn eq_text(p: &EqParam) -> String {
    format!(
        "{} ({:#04x}) bands {:?}",
        eq_preset_name(p.preset).unwrap_or("?"),
        p.preset,
        p.bands
    )
}

fn nc_text(p: &NcAsmParam) -> String {
    format!("{:?} {:?} level {}", p.mode, p.ambient_mode, p.level)
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// Opens a new link after the headset dropped it, for up to 40 s.
fn reconnect() -> Result<(Live, Snapshot), String> {
    thread::sleep(Duration::from_secs(1));
    for _ in 0..40 {
        if let Ok(Ok(mut session)) = connect()
            && let Ok(snapshot) = read_snapshot(&mut session)
        {
            return Ok((session, snapshot));
        }
        thread::sleep(Duration::from_secs(1));
    }
    Err("the headset did not come back within 40 s".to_owned())
}

/// Prints the after value of a write, or its error.
fn show<T>(label: &str, result: &Result<Change<T>, WriteError>, text: impl Fn(&T) -> String) {
    match result {
        Ok(c) => println!("{label} {}", text(&c.after)),
        Err(e) => eprintln!("{label} FAILED: {e}"),
    }
}

fn finish<T>(write: Result<T, WriteError>) -> Result<(), String> {
    write.map(|_| ()).map_err(|e| format!("write: {e}"))
}

fn set(mut session: Live, args: SetArgs) -> Result<(), String> {
    let snapshot = read_snapshot(&mut session).map_err(|e| e.to_string())?;
    let restore = args.restore;
    match args.setter {
        Setter::NcAsm { mode, voice, level } => {
            let before = snapshot.ncasm.ok_or("no NC/ASM on this headset")?.param;
            let ambient = if voice {
                AmbientMode::Voice
            } else {
                AmbientMode::Normal
            };
            println!("before: {}", nc_text(&before));
            // The device stores the ambient level apart from the mode. Starting in NC the
            // level field of the reply is taken as that stored level [INF, RE state `67 17 01 01 00 00 14`].
            if before.mode == NcAsmMode::NoiseCancelling {
                println!(
                    "recorded ambient level {} (inferred from the NC reading)",
                    before.level
                );
            }
            let result = session.set_nc_asm(mode, ambient, level.unwrap_or(before.level));
            show("after: ", &result, nc_text);
            if restore {
                // RE: set ASM with the old level first, then NC.
                let back =
                    session.set_nc_asm(NcAsmMode::Ambient, before.ambient_mode, before.level);
                show("restored ambient:", &back, nc_text);
                if before.mode == NcAsmMode::NoiseCancelling {
                    let back = session.set_nc_asm(
                        NcAsmMode::NoiseCancelling,
                        before.ambient_mode,
                        before.level,
                    );
                    show("restored NC:", &back, nc_text);
                }
            }
            finish(result)
        }
        Setter::EqPreset(id) => {
            let before = snapshot.eq.ok_or("no EQ on this headset")?.param;
            println!("before: {}", eq_text(&before));
            let result = session.select_eq_preset(id);
            show("after: ", &result, eq_text);
            if restore {
                let back = session.select_eq_preset(before.preset);
                show("restored:", &back, eq_text);
            }
            finish(result)
        }
        Setter::EqBands(steps) => {
            let before = snapshot.eq.ok_or("no EQ on this headset")?.param;
            println!("before: {}", eq_text(&before));
            // The restore copy of CUSTOM. When another preset is active, select CUSTOM
            // first (the headset restores its stored bands) and keep what it reports.
            let mut custom_copy = before.bands.clone();
            if before.preset != CUSTOM {
                match session.select_eq_preset(CUSTOM) {
                    Ok(c) => {
                        println!("selected CUSTOM, bands {:?}", c.after.bands);
                        custom_copy = c.after.bands;
                    }
                    Err(e) => {
                        eprintln!("selecting CUSTOM FAILED: {e}");
                        if restore {
                            show(
                                "restored preset:",
                                &session.select_eq_preset(before.preset),
                                eq_text,
                            );
                        }
                        return Err(format!("write: {e}"));
                    }
                }
            }
            let result = session.set_eq_bands(&steps);
            match &result {
                Ok(c) => println!("after:  {}", eq_text(&c.after)),
                Err(e) => eprintln!("after:  FAILED: {e}"),
            }
            if restore {
                // Also after a failure: bands first, then the original preset.
                match session.set_eq_bands(&custom_copy) {
                    Ok(c) => println!("restored bands: {}", eq_text(&c.after)),
                    Err(e) => eprintln!("restored bands FAILED: {e}"),
                }
                if before.preset != CUSTOM {
                    show(
                        "restored preset:",
                        &session.select_eq_preset(before.preset),
                        eq_text,
                    );
                }
            }
            finish(result)
        }
        Setter::Sidetone(on) => {
            let before = snapshot.sidetone.ok_or("no sidetone on this headset")?;
            println!("before: sidetone {}", on_off(before));
            let result = session.set_sidetone(on);
            show("after: ", &result, |v| format!("sidetone {}", on_off(*v)));
            if restore {
                let back = session.set_sidetone(before);
                show("restored:", &back, |v| format!("sidetone {}", on_off(*v)));
            }
            finish(result)
        }
        Setter::Multipoint(on) => {
            let before = snapshot.multipoint.ok_or("no multipoint on this headset")?;
            println!("before: multipoint {}", on_off(before));
            let result = session.set_multipoint(on);
            // The session to restore on: a new one after the link dropped.
            let mut live = Some(session);
            match &result {
                Ok(MultipointOutcome::PendingReconnect { .. }) | Err(WriteError::Disconnected) => {
                    println!("link dropped, reconnecting");
                    let (s, snap) = reconnect()?;
                    println!(
                        "after:  multipoint {}",
                        on_off(snap.multipoint.unwrap_or(false))
                    );
                    live = Some(s);
                }
                Ok(MultipointOutcome::Unchanged) => println!("after:  unchanged"),
                Err(e) => eprintln!("after:  FAILED: {e}"),
            }
            if restore
                && on != before
                && let Some(mut s) = live
            {
                match s.set_multipoint(before) {
                    Ok(MultipointOutcome::PendingReconnect { .. })
                    | Err(WriteError::Disconnected) => {
                        println!("link dropped, reconnecting");
                        let (_, snap) = reconnect()?;
                        println!(
                            "restored: multipoint {}",
                            on_off(snap.multipoint.unwrap_or(false))
                        );
                    }
                    other => println!("restore: {other:?}"),
                }
            }
            finish(result)
        }
    }
}
