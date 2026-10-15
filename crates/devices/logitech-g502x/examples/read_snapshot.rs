//! Read-only G502 X snapshot against real hardware.
//!
//! Usage: `read_snapshot [--out <file>] [--sw-id <hex>]`
//!
//! Finds the receiver, opens the first HID++ entry, reads the snapshot with
//! read-tier requests only and prints JSON to stdout (or writes it to the file
//! given with `--out`). There are no write-capable options. Serial numbers and
//! device paths are never printed, stored or serialized (hidapi itself reads
//! serial strings while it enumerates devices; this program does not use them).
//!
//! Exit codes: 0 ok, 1 no receiver found, 2 bad arguments, 3 failure while
//! reading.

use std::process::ExitCode;

use logitech_g502x::{DEVICE_INDEX, read_snapshot};
use myhub_transport::{
    HidApi, HidppSession, LOGITECH_VID, UsageRule, find_receivers, list_candidates,
};

struct Args {
    out: Option<String>,
    sw_id: Option<u8>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        out: None,
        sw_id: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" => args.out = Some(it.next().ok_or("--out needs a file name")?),
            "--sw-id" => {
                let value = it.next().ok_or("--sw-id needs a hex value")?;
                let digits = value
                    .strip_prefix("0x")
                    .or_else(|| value.strip_prefix("0X"))
                    .unwrap_or(&value);
                args.sw_id = Some(
                    u8::from_str_radix(digits, 16)
                        .map_err(|_| format!("--sw-id: {value:?} is not a hex byte"))?,
                );
            }
            "-h" | "--help" => {
                return Err("usage: read_snapshot [--out <file>] [--sw-id <hex>]".into());
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            eprintln!("could not initialise hidapi: {e}");
            return ExitCode::from(3);
        }
    };

    let receivers = find_receivers(&api);
    let Some(receiver) = receivers.first() else {
        eprintln!(
            "no receiver HID++ interface found (rule: {:?}). Logitech candidates:",
            UsageRule::for_current_os()
        );
        for c in list_candidates(&api)
            .iter()
            .filter(|c| c.vid == LOGITECH_VID)
        {
            eprintln!(
                "  vid={:#06x} pid={:#06x} interface={} usage_page={:#06x} usage={:#06x}",
                c.vid, c.pid, c.interface, c.usage_page, c.usage
            );
        }
        return ExitCode::from(1);
    };

    let mut session = match HidppSession::open(&api, receiver) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("could not open the receiver: {e}");
            return ExitCode::from(3);
        }
    };
    session.set_device_index(DEVICE_INDEX);
    if let Some(sw_id) = args.sw_id
        && let Err(e) = session.set_sw_id(sw_id)
    {
        eprintln!("{e}");
        return ExitCode::from(2);
    }

    let snapshot = match read_snapshot(&mut session) {
        Ok(snapshot) => snapshot,
        Err(e) => {
            eprintln!("read failed: {e}");
            return ExitCode::from(3);
        }
    };
    if !snapshot.short_reports_available {
        eprintln!(
            "warning: the short-report collection could not be opened, so HID++ 1.0 messages (device connection, errors) were not visible"
        );
    }
    let json = match serde_json::to_string_pretty(&snapshot) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("could not serialise the snapshot: {e}");
            return ExitCode::from(3);
        }
    };
    match args.out {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("could not write {path}: {e}");
                return ExitCode::from(3);
            }
            eprintln!("snapshot written to {path}");
        }
        None => println!("{json}"),
    }
    ExitCode::SUCCESS
}
