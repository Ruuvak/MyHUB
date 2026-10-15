//! Read-only probe for the Sony WF-C710N over Bluetooth RFCOMM (Windows).
//!
//! Usage: `mdr_probe list | state | watch <seconds>`
//!
//! * `list`: paired devices whose name matches WF-C710N or that offer the MDR service.
//!   Prints the name and the masked address (last byte only). Opens no connection.
//! * `state`: connects to the first candidate, runs the handshake and prints the
//!   snapshot as pretty JSON.
//! * `watch <seconds>`: connects, runs the handshake, then prints decoded notifications
//!   as they arrive for the given time. Every frame that needs an ACK gets one.
//!
//! What it sends: only read-tier requests that pass `mdr::Policy::new()` (protocol
//! info, device info model and firmware, support function tables, battery, NC/ASM and
//! EQ capability and parameter, multipoint and sidetone state, codec) and ACK frames.
//! No setting is ever changed. It never requests the capability info (headset address),
//! and never prints a full address, a paired-device list or track metadata.
//!
//! Exit codes: 0 ok, 1 no headset found, 2 bad arguments, 3 failure.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use sony_wf_c710n::{Session, SessionError, candidates, describe, handshake, open, read_snapshot};

enum Command {
    List,
    State,
    Watch(u64),
}

fn parse_args() -> Option<Command> {
    let mut args = std::env::args().skip(1);
    let command = match args.next()?.as_str() {
        "list" => Command::List,
        "state" => Command::State,
        "watch" => Command::Watch(args.next()?.parse().ok()?),
        _ => return None,
    };
    args.next().is_none().then_some(command)
}

fn connect() -> Result<Result<Session<myhub_rfcomm::RfcommLink>, SessionError>, String> {
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

fn state(session: &mut Session<myhub_rfcomm::RfcommLink>) -> Result<(), SessionError> {
    let snapshot = read_snapshot(session)?;
    match serde_json::to_string_pretty(&snapshot) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("could not serialize the snapshot: {e}"),
    }
    Ok(())
}

fn watch(
    session: &mut Session<myhub_rfcomm::RfcommLink>,
    seconds: u64,
) -> Result<(), SessionError> {
    let identity = handshake(session)?;
    println!("{} firmware {}", identity.model, identity.firmware);
    for note in session.take_notifications() {
        println!("{}", describe(&note));
    }
    let start = Instant::now();
    let end = start + Duration::from_secs(seconds);
    while Instant::now() < end {
        session.poll(200)?;
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
