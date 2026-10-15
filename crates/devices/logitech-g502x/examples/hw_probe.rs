//! Supervised hardware probe for the G502 X: one action per run.
//!
//! Usage: `hw_probe [--verbose] [--sw-id <hex>] <command>`
//!
//! * `state`: 0x8100 fn2 mode (1 onboard / 2 host), fn4 active profile, current
//!   DPI and report rate. Read tier only.
//! * `host`: switch to host mode (0x8100 fn1), verify fn2 == 2, print the state.
//! * `onboard`: switch to onboard mode (0x8100 fn1), verify fn2 == 1, print the state.
//! * `dpi <N>`: refuses unless the mouse is already in host mode (no implicit
//!   switch); the value is checked against the device's DPI list, set, read back.
//! * `rate <HZ>`: same for the report rate (must be in the device's option list).
//! * `sectors <out.json>`: reads all 0x8100 sectors (read tier) and writes the
//!   hex of each to the file, so before and after can be diffed. Refuses to
//!   overwrite an existing file.
//!
//! * `battery-log [--interval <s>] [--duration <s>] [--out <file.csv>]`: read-only.
//!   Every interval (default 5 s) reads 0x1004 fn1 and logs time, elapsed seconds,
//!   raw bytes 0..3, percent, level and charging status; between reads it logs
//!   unsolicited 0x1004 broadcasts and 0x41 link up/down. It survives link loss
//!   (retries discovery every interval, never exits on a device error) and stops
//!   on Ctrl+C or after `--duration` seconds (default 0 = until Ctrl+C). The CSV
//!   is created with create_new and flushed after every line.
//! * `write-same <sector> --backup <file>`: FLASH WRITE of the identical image (forced)
//!   to a user sector 0x0003..=0x0005, then read back and compare.
//! * `write-profile <sector> --backup <file> [--dpi a,b,c,d,e] [--default N]
//!   [--shift N] [--rate HZ]`: FLASH WRITE of a patched image (DPI levels, default
//!   and shift DPI index 0..=4, report rate 1000/500/250/125 Hz). Without any of the
//!   options nothing changes and nothing is written.
//! * `restore <sector> <file>`: FLASH WRITE of a backup file made by `--backup`.
//!   Also accepts sector 0x0000 (directory image).
//! * `write-profile ... --from <sector>`: the base image is a copy of user sector
//!   1..=5 instead of the target's own bytes (the backup is still of the target).
//! * `enable-profile <sector> --backup <file>` / `disable-profile ...`: FLASH WRITE
//!   of directory 0x0000, changing only that entry's flag (sector 3..=5; never
//!   disables the active or the last enabled profile).
//! * `spy-capture [--seconds <n>]`: research. Needs host mode (`hw_probe host`).
//!   Starts the 0x8110 button spy (fn1), prints every notification (raw payload,
//!   first u16 as BE and LE, set bits) for n seconds (default 60) or until Ctrl+C,
//!   and always sends the stop (fn2), also after an error. Sends nothing else.
//!   Closing the console window or killing the process skips the stop: use
//!   `spy-stop` then.
//! * `spy-stop`: discover, then send only 0x8110 fn2 (zero params) and print the
//!   reply. No mode requirement.
//! * `select-profile <sector>`: fn3 SetCurrentProfile (sector 1..=5, enabled in the
//!   directory), verified with fn4. Not a flash write, but changes the profile.
//!
//! The three write commands need the user's explicit approval in chat before every
//! run. The backup file is created with create_new (an existing file is refused)
//! and holds JSON `{sector, hex, crc}`. A failed or mismatching write is followed
//! by one restore of the backup image; the error says whether it worked.
//!
//! Every frame goes through the session policy (`request_feature`,
//! `request_volatile` or `write_sector`); the 0x8100 functions used are
//! 0, 1, 2, 4, 5 and, for the write commands only, 6, 7 and 8. `--verbose` prints
//! every frame sent and received to stderr, including every fn6/fn7/fn8 frame. Exit codes: 0 ok, 1 no receiver, 2 bad arguments, 3 failure.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hidpp::Message;
use hidpp::battery;
use hidpp::dpi::{self, DpiList};
use hidpp::onboard_write::ProfilePatch;
use hidpp::report_rate;
use hidpp::{FeatureId, crc, onboard};
use logitech_g502x::{
    DEVICE_INDEX, ProfileSource, RestoreStatus, SectorWriteError, SequenceLog, SpyLine,
    WriteOutcome, onboard_mode, read_snapshot, restore_sector, select_profile, set_dpi,
    set_onboard_mode, set_report_rate, spy_capture, spy_stop, write_directory,
    write_profile_sector,
};
use myhub_core::OnboardMode;
use myhub_transport::{
    FrameDirection, HidApi, HidLink, HidppSession, LOGITECH_VID, UsageRule,
    connection_notification, find_receivers, list_candidates,
};

enum Command {
    State,
    Host,
    Onboard,
    Dpi(u16),
    Rate(u16),
    Sectors(String),
    BatteryLog {
        interval: u64,
        duration: u64,
        out: Option<String>,
    },
    WriteSame {
        sector: u16,
        backup: String,
    },
    WriteProfile {
        sector: u16,
        backup: String,
        patch: ProfilePatch,
        source: ProfileSource,
    },
    EnableProfile {
        sector: u16,
        backup: String,
    },
    DisableProfile {
        sector: u16,
        backup: String,
    },
    SelectProfile(u16),
    SpyCapture {
        seconds: u64,
    },
    SpyStop,
    Restore {
        sector: u16,
        file: String,
    },
}

struct Args {
    verbose: bool,
    sw_id: Option<u8>,
    command: Command,
}

const USAGE: &str = "usage: hw_probe [--verbose] [--sw-id <hex>] state | host | onboard | dpi <N> | rate <HZ> | sectors <out.json> | write-same <sector> --backup <file> | write-profile <sector> --backup <file> [--from <sector>] [--dpi a,b,c,d,e] [--default N] [--shift N] [--rate HZ] | enable-profile <sector> --backup <file> | disable-profile <sector> --backup <file> | select-profile <sector> | restore <sector> <file> | spy-capture [--seconds <n>] (closing the window or killing the process skips the stop: use spy-stop) | spy-stop | battery-log [--interval <s>] [--duration <s>] [--out <file.csv>]";

fn parse_args() -> Result<Args, String> {
    let mut verbose = false;
    let mut sw_id = None;
    let mut rest = Vec::new();
    let mut backup: Option<String> = None;
    let mut patch = ProfilePatch::default();
    // Subcommand-specific flags seen, to reject the ones a subcommand ignores.
    let mut used: Vec<String> = Vec::new();
    let mut from: Option<String> = None;
    let mut seconds = 60u64;
    let mut interval = 5u64;
    let mut duration = 0u64;
    let mut out: Option<String> = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        if matches!(
            arg.as_str(),
            "--backup"
                | "--dpi"
                | "--default"
                | "--shift"
                | "--rate"
                | "--interval"
                | "--duration"
                | "--out"
                | "--from"
                | "--seconds"
        ) {
            used.push(arg.clone());
        }
        match arg.as_str() {
            "--verbose" | "-v" => verbose = true,
            "--sw-id" => {
                let value = it.next().ok_or("--sw-id needs a hex value")?;
                let digits = value
                    .strip_prefix("0x")
                    .or_else(|| value.strip_prefix("0X"))
                    .unwrap_or(&value);
                sw_id = Some(
                    u8::from_str_radix(digits, 16)
                        .map_err(|_| format!("--sw-id: {value:?} is not a hex byte"))?,
                );
            }
            "--seconds" => {
                seconds = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| n >= 1)
                    .ok_or("--seconds needs a whole number of seconds >= 1")?;
            }
            "--from" => from = Some(it.next().ok_or("--from needs a sector")?),
            "--backup" => backup = Some(it.next().ok_or("--backup needs a file name")?),
            "--dpi" => {
                let value = it.next().ok_or("--dpi needs five comma separated values")?;
                let levels: Vec<u16> = value
                    .split(',')
                    .map(|v| v.trim().parse())
                    .collect::<Result<_, _>>()
                    .map_err(|_| format!("--dpi: {value:?} is not a list of numbers"))?;
                let levels: [u16; 5] = levels
                    .try_into()
                    .map_err(|_| "--dpi needs exactly five values".to_string())?;
                patch.dpi_levels = Some(levels);
            }
            "--default" => {
                patch.default_dpi_index = Some(small("--default", it.next())?);
            }
            "--shift" => {
                patch.shift_dpi_index = Some(small("--shift", it.next())?);
            }
            "--rate" => {
                let hz: u16 = it
                    .next()
                    .ok_or("--rate needs a number")?
                    .parse()
                    .map_err(|_| "--rate: not a number".to_string())?;
                let interval = [1u8, 2, 4, 8]
                    .into_iter()
                    .find(|&ms| report_rate::interval_to_hz(ms) == hz)
                    .ok_or("--rate must be 1000, 500, 250 or 125")?;
                patch.report_rate_interval_ms = Some(interval);
            }
            "--interval" => {
                interval = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| n >= 1)
                    .ok_or("--interval needs a whole number of seconds >= 1")?;
            }
            "--duration" => {
                duration = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--duration needs a whole number of seconds")?;
            }
            "--out" => out = Some(it.next().ok_or("--out needs a file name")?),
            "-h" | "--help" => return Err(USAGE.into()),
            _ => rest.push(arg),
        }
    }
    let sector_arg = |value: &str| -> Result<u16, String> {
        let parsed = match value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
        {
            Some(hex) => u16::from_str_radix(hex, 16),
            None => value.parse(),
        };
        parsed.map_err(|_| format!("sector: {value:?} is not a number"))
    };
    let number = |name: &str, value: Option<&String>| -> Result<u16, String> {
        value
            .ok_or_else(|| format!("{name} needs a number"))?
            .parse()
            .map_err(|_| format!("{name}: not a number between 0 and 65535"))
    };
    let command = match rest
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["state"] => Command::State,
        ["host"] => Command::Host,
        ["onboard"] => Command::Onboard,
        ["dpi", _] => Command::Dpi(number("dpi", rest.get(1))?),
        ["rate", _] => Command::Rate(number("rate", rest.get(1))?),
        ["sectors", path] => Command::Sectors((*path).to_owned()),
        ["battery-log"] => Command::BatteryLog {
            interval,
            duration,
            out: out.clone(),
        },
        ["write-same", sector] => Command::WriteSame {
            sector: sector_arg(sector)?,
            backup: backup.clone().ok_or("write-same needs --backup <file>")?,
        },
        ["write-profile", sector] => Command::WriteProfile {
            sector: sector_arg(sector)?,
            backup: backup
                .clone()
                .ok_or("write-profile needs --backup <file>")?,
            patch,
            source: match &from {
                Some(f) => ProfileSource::CloneFrom(sector_arg(f)?),
                None => ProfileSource::Same,
            },
        },
        ["enable-profile", sector] => Command::EnableProfile {
            sector: sector_arg(sector)?,
            backup: backup
                .clone()
                .ok_or("enable-profile needs --backup <file>")?,
        },
        ["disable-profile", sector] => Command::DisableProfile {
            sector: sector_arg(sector)?,
            backup: backup
                .clone()
                .ok_or("disable-profile needs --backup <file>")?,
        },
        ["spy-stop"] => Command::SpyStop,
        ["spy-capture"] => Command::SpyCapture { seconds },
        ["select-profile", sector] => Command::SelectProfile(sector_arg(sector)?),
        ["restore", sector, file] => Command::Restore {
            sector: sector_arg(sector)?,
            file: (*file).to_owned(),
        },
        _ => return Err(USAGE.into()),
    };
    let allowed: &[&str] = match &command {
        Command::WriteSame { .. }
        | Command::EnableProfile { .. }
        | Command::DisableProfile { .. } => &["--backup"],
        Command::WriteProfile { .. } => &[
            "--backup",
            "--dpi",
            "--default",
            "--shift",
            "--rate",
            "--from",
        ],
        Command::BatteryLog { .. } => &["--interval", "--duration", "--out"],
        Command::SpyCapture { .. } => &["--seconds"],
        _ => &[],
    };
    if let Some(flag) = used.iter().find(|f| !allowed.contains(&f.as_str())) {
        return Err(format!(
            "{flag} is not used by this command
{USAGE}"
        ));
    }
    Ok(Args {
        verbose,
        sw_id,
        command,
    })
}

fn small(name: &str, value: Option<String>) -> Result<u8, String> {
    value
        .ok_or_else(|| format!("{name} needs a number"))?
        .parse()
        .map_err(|_| format!("{name}: not a number between 0 and 255"))
}

fn trace(direction: FrameDirection, frame: &[u8]) {
    let hex: String = frame.iter().map(|b| format!("{b:02x}")).collect();
    let tag = match direction {
        FrameDirection::Tx => "TX",
        FrameDirection::Rx => "RX",
    };
    eprintln!("{tag} {hex}");
}

fn mode_name(mode: Option<OnboardMode>) -> &'static str {
    match mode {
        Some(OnboardMode::Onboard) => "1 (onboard)",
        Some(OnboardMode::Host) => "2 (host)",
        None => "unknown",
    }
}

/// Reads and prints the state: mode, active profile, DPI and report rate.
fn print_state<L: HidLink>(session: &mut HidppSession<L>) -> Result<(), String> {
    let mode = onboard_mode(session).map_err(|e| format!("mode read failed: {e}"))?;
    println!("onboard mode (0x8100 fn2): {}", mode_name(mode));
    let active = session
        .request_feature(
            FeatureId::ONBOARD_PROFILES,
            onboard::FN_GET_ACTIVE_PROFILE,
            [0; 16],
        )
        .map_err(|e| format!("active profile read failed: {e}"))?;
    println!(
        "active profile (0x8100 fn4): sector {:#06x}",
        u16::from_be_bytes([active[0], active[1]])
    );
    let current = session
        .request_feature(
            FeatureId::ADJUSTABLE_DPI,
            dpi::FN_GET_SENSOR_DPI,
            dpi::sensor_params(0),
        )
        .map_err(|e| format!("DPI read failed: {e}"))?;
    let current = dpi::parse_sensor_dpi(&current).map_err(|e| e.to_string())?;
    println!("DPI (0x2201 fn2): {}", current.current);
    let rate = session
        .request_feature(
            FeatureId::REPORT_RATE,
            report_rate::FN_GET_REPORT_RATE,
            [0; 16],
        )
        .map_err(|e| format!("report rate read failed: {e}"))?;
    let interval = report_rate::parse_current_interval_ms(&rate).map_err(|e| e.to_string())?;
    println!(
        "report rate (0x8060 fn1): {} Hz ({interval} ms)",
        report_rate::interval_to_hz(interval)
    );
    Ok(())
}

/// Refuses unless the mouse is in host mode already (no implicit switch).
fn require_host<L: HidLink>(session: &mut HidppSession<L>) -> Result<(), String> {
    let mode = onboard_mode(session).map_err(|e| format!("mode read failed: {e}"))?;
    if mode == Some(OnboardMode::Host) {
        Ok(())
    } else {
        Err(format!(
            "refusing: the mouse is not in host mode (fn2 = {}); run `hw_probe host` first",
            mode_name(mode)
        ))
    }
}

/// Refuses unless the mouse is in onboard mode (flash writes are tested there).
fn require_onboard<L: HidLink>(session: &mut HidppSession<L>) -> Result<(), String> {
    let mode = onboard_mode(session).map_err(|e| format!("mode read failed: {e}"))?;
    if mode == Some(OnboardMode::Onboard) {
        Ok(())
    } else {
        Err(format!(
            "refusing: the mouse is not in onboard mode (fn2 = {}); run `hw_probe onboard` first",
            mode_name(mode)
        ))
    }
}

fn print_log(log: &SequenceLog) {
    println!("fn6 reply: {}", log.begin.as_deref().unwrap_or("-"));
    for (i, reply) in log.data.iter().enumerate() {
        println!("fn7 reply {i:2}: {reply}");
    }
    println!("fn8 reply: {}", log.end.as_deref().unwrap_or("-"));
}

/// Prints, also without --verbose, what a failed write sequence did: the fn6/7/8
/// reply payloads, the read-back check and the restore status with its own log.
fn report_failure(e: SectorWriteError) -> String {
    if let SectorWriteError::WriteFailed { detail, .. }
    | SectorWriteError::VerifyFailed { detail, .. } = &e
    {
        eprintln!("--- write sequence replies (hex payloads) ---");
        eprintln!("fn6 reply: {}", detail.log.begin.as_deref().unwrap_or("-"));
        for (i, reply) in detail.log.data.iter().enumerate() {
            eprintln!("fn7 reply {i:2}: {reply}");
        }
        eprintln!("fn8 reply: {}", detail.log.end.as_deref().unwrap_or("-"));
        eprintln!("read-back check: {:?}", detail.readback);
        eprintln!("restore: {}", detail.restore);
        match &detail.restore {
            RestoreStatus::Restored(log) | RestoreStatus::Failed(_, log) => {
                eprintln!("--- restore sequence replies ---");
                eprintln!("fn6 reply: {}", log.begin.as_deref().unwrap_or("-"));
                for (i, reply) in log.data.iter().enumerate() {
                    eprintln!("fn7 reply {i:2}: {reply}");
                }
                eprintln!("fn8 reply: {}", log.end.as_deref().unwrap_or("-"));
            }
            _ => {}
        }
    }
    e.to_string()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return Err("hex string has odd length or non-ASCII characters".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "bad hex digit".to_string()))
        .collect()
}

/// Reads a backup file made by `--backup` and checks its sector number and CRC.
fn read_backup_file(path: &str, sector: u16) -> Result<Vec<u8>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{path} is not valid JSON: {e}"))?;
    // A number (hw_probe, MyHUB) or a "0x0003" style string.
    let file_sector = json["sector"]
        .as_u64()
        .or_else(|| {
            let text = json["sector"].as_str()?;
            let digits = text
                .strip_prefix("0x")
                .or_else(|| text.strip_prefix("0X"))?;
            u64::from_str_radix(digits, 16).ok()
        })
        .ok_or_else(|| format!("{path}: missing or unreadable sector"))?;
    if file_sector != u64::from(sector) {
        return Err(format!(
            "{path} holds sector {file_sector:#06x}, not {sector:#06x}"
        ));
    }
    let bytes = from_hex(
        json["hex"]
            .as_str()
            .ok_or_else(|| format!("{path}: missing hex"))?,
    )?;
    if !crc::sector_crc_ok(&bytes) {
        return Err(format!("{path}: the image has an invalid CRC"));
    }
    let crc_text = json["crc"]
        .as_str()
        .ok_or_else(|| format!("{path}: missing crc"))?;
    let file_crc = u16::from_str_radix(
        crc_text
            .strip_prefix("0x")
            .or_else(|| crc_text.strip_prefix("0X"))
            .unwrap_or(crc_text),
        16,
    )
    .map_err(|_| format!("{path}: crc {crc_text:?} is not a hex number"))?;
    if Some(file_crc) != crc::sector_crc_computed(&bytes) {
        return Err(format!(
            "{path}: the crc field does not match the hex image"
        ));
    }
    Ok(bytes)
}

fn refuse_existing_backup(backup_path: &str) -> Result<(), String> {
    if Path::new(backup_path).exists() {
        return Err(format!(
            "{backup_path} exists; refusing to overwrite a backup"
        ));
    }
    Ok(())
}

fn print_outcome(report: &logitech_g502x::WriteReport) {
    let sector = report.sector;
    match report.outcome {
        WriteOutcome::Unchanged => {
            println!("image equals the sector: nothing written (use write-same to force)");
        }
        WriteOutcome::Written => {
            println!("sector {sector:#06x} written, read back identical, CRC ok");
            print_log(&report.log);
        }
    }
}

/// enable-profile and disable-profile: the directory entry of a profile sector.
fn directory_command<L: HidLink>(
    session: &mut HidppSession<L>,
    sector: u16,
    enabled: bool,
    backup_path: &str,
) -> Result<(), String> {
    refuse_existing_backup(backup_path)?;
    require_onboard(session)?;
    let mut backup = backup_writer(backup_path);
    let report =
        write_directory(session, sector, enabled, &mut backup, false).map_err(report_failure)?;
    print_outcome(&report);
    Ok(())
}

/// Shared by write-same and write-profile.
fn write_command<L: HidLink>(
    session: &mut HidppSession<L>,
    sector: u16,
    backup_path: &str,
    patch: ProfilePatch,
    source: ProfileSource,
    force_identical: bool,
) -> Result<(), String> {
    refuse_existing_backup(backup_path)?;
    require_onboard(session)?;
    let mut backup = backup_writer(backup_path);
    let report = write_profile_sector(
        session,
        sector,
        &patch,
        source,
        &mut backup,
        force_identical,
    )
    .map_err(report_failure)?;
    print_outcome(&report);
    Ok(())
}

/// Backup callback that writes `{sector, hex, crc}` JSON with create_new.
fn backup_writer(backup_path: &str) -> impl FnMut(u16, &[u8; 255]) -> Result<(), String> + '_ {
    move |sector: u16, bytes: &[u8; 255]| -> Result<(), String> {
        let json = serde_json::json!({
            "sector": sector,
            "hex": to_hex(bytes),
            "crc": format!("{:#06x}", crc::sector_crc_computed(bytes).unwrap_or(0)),
        });
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(backup_path)
            .map_err(|e| format!("cannot create {backup_path} (it must not exist): {e}"))?;
        file.write_all(
            serde_json::to_string_pretty(&json)
                .unwrap_or_default()
                .as_bytes(),
        )
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("cannot write {backup_path}: {e}"))?;
        println!("backup of sector {sector:#06x} written to {backup_path}");
        Ok(())
    }
}

/// Set by Ctrl+C (SIGINT / console control event) so `battery-log` can end
/// cleanly.
static STOP: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
mod sys {
    use super::STOP;
    use std::sync::atomic::Ordering;

    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    type Handler = unsafe extern "system" fn(u32) -> i32;

    unsafe extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<Handler>, add: i32) -> i32;
        fn GetLocalTime(time: *mut SystemTime);
    }

    unsafe extern "system" fn on_ctrl(_event: u32) -> i32 {
        STOP.store(true, Ordering::SeqCst);
        1
    }

    pub fn install_stop_handler() {
        // SAFETY: registers a handler that only stores to an atomic.
        unsafe {
            SetConsoleCtrlHandler(Some(on_ctrl), 1);
        }
    }

    pub fn local_iso() -> String {
        let mut t = SystemTime {
            year: 0,
            month: 0,
            day_of_week: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
        };
        // SAFETY: `t` is a valid SYSTEMTIME the call fills in.
        unsafe { GetLocalTime(&mut t) };
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            t.year, t.month, t.day, t.hour, t.minute, t.second
        )
    }
}

#[cfg(unix)]
mod sys {
    use super::STOP;
    use std::sync::atomic::Ordering;

    /// glibc / macOS `struct tm` on 64-bit targets.
    #[repr(C)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
        gmtoff: i64,
        zone: *const u8,
    }

    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
        fn time(out: *mut i64) -> i64;
        fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    }

    extern "C" fn on_sigint(_sig: i32) {
        STOP.store(true, Ordering::SeqCst);
    }

    pub fn install_stop_handler() {
        const SIGINT: i32 = 2;
        // SAFETY: the handler only stores to an atomic (async-signal-safe).
        unsafe {
            signal(SIGINT, on_sigint as extern "C" fn(i32) as usize);
        }
    }

    pub fn local_iso() -> String {
        let mut tm = Tm {
            sec: 0,
            min: 0,
            hour: 0,
            mday: 0,
            mon: 0,
            year: 0,
            wday: 0,
            yday: 0,
            isdst: 0,
            gmtoff: 0,
            zone: std::ptr::null(),
        };
        // SAFETY: `time` and `localtime_r` fill the buffers passed in.
        unsafe {
            let now = time(std::ptr::null_mut());
            localtime_r(&now, &mut tm);
        }
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            tm.year + 1900,
            tm.mon + 1,
            tm.mday,
            tm.hour,
            tm.min,
            tm.sec
        )
    }
}

/// Appends and prints CSV lines of `battery-log`; flushes after every line.
struct BatteryCsv {
    file: Option<std::fs::File>,
    start: Instant,
}

impl BatteryCsv {
    fn line(
        &mut self,
        source: &str,
        raw: &str,
        percent: &str,
        level: &str,
        status: &str,
        note: &str,
    ) {
        let line = format!(
            "{},{:.1},{source},{raw},{percent},{level},{status},{note}",
            sys::local_iso(),
            self.start.elapsed().as_secs_f32()
        );
        println!("{line}");
        if let Some(file) = self.file.as_mut()
            && let Err(e) = writeln!(file, "{line}").and_then(|()| file.flush())
        {
            eprintln!("cannot write the CSV: {e}");
        }
    }

    /// A line for a 0x1004 status payload.
    fn status(&mut self, source: &str, payload: &[u8], note: &str) {
        match battery::parse_status(payload) {
            Ok(s) => {
                let raw = to_hex(&payload[..4]);
                let percent = s.percent.to_string();
                let level = s.level_name().unwrap_or("");
                let status = s.charging_state_name();
                self.line(source, &raw, &percent, level, &status, note);
            }
            Err(e) => self.line(source, "", "", "", "", &format!("unparsable: {e}")),
        }
    }
}

/// Read-only battery logger. Only 0x1004 fn1 is ever sent (plus discovery).
fn battery_log<L: HidLink>(
    session: &mut HidppSession<L>,
    interval: u64,
    duration: u64,
    out: Option<String>,
) -> Result<(), String> {
    let file = match &out {
        Some(path) => Some(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|e| format!("cannot create {path} (it must not exist): {e}"))?,
        ),
        None => None,
    };
    sys::install_stop_handler();
    let start = Instant::now();
    let mut csv = BatteryCsv { file, start };
    println!("time,elapsed_s,source,raw,percent,level,status,note");
    if let Some(file) = csv.file.as_mut() {
        let _ = writeln!(file, "time,elapsed_s,source,raw,percent,level,status,note");
        let _ = file.flush();
    }

    let step = Duration::from_secs(interval);
    let mut battery_index: Option<u8> = None;
    let mut healthy = false;
    let mut next_tick = start;
    let done = |start: Instant| {
        STOP.load(Ordering::SeqCst) || (duration > 0 && start.elapsed().as_secs() >= duration)
    };
    while !done(start) {
        'tick: {
            if !healthy || session.needs_rediscovery() {
                match session.discover() {
                    Ok(()) => {
                        battery_index = session.table().index_of(FeatureId::UNIFIED_BATTERY);
                        csv.line("link", "", "", "", "", "discover ok");
                    }
                    Err(e) => {
                        healthy = false;
                        csv.line("link", "", "", "", "", &format!("link down {e}"));
                        break 'tick;
                    }
                }
            }
            match session.request_feature(
                FeatureId::UNIFIED_BATTERY,
                battery::FN_GET_STATUS,
                [0; 16],
            ) {
                Ok(payload) => {
                    healthy = true;
                    csv.status("poll", &payload, "");
                }
                Err(e) => {
                    healthy = false;
                    csv.line("link", "", "", "", "", &format!("link down {e}"));
                }
            }
        }
        next_tick += step;
        // Between ticks: listen without writing and log what arrives.
        while !done(start) && Instant::now() < next_tick {
            let wait = next_tick
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(500));
            if let Err(e) = session.poll_events(wait) {
                csv.line("link", "", "", "", "", &format!("link down {e}"));
                healthy = false;
                std::thread::sleep(wait.min(Duration::from_millis(250)));
            }
            for message in session.drain_notifications() {
                if let Some((device, up)) = connection_notification(&message) {
                    if device == DEVICE_INDEX {
                        let text = if up {
                            "link up (0x41)"
                        } else {
                            "link down (0x41)"
                        };
                        csv.line("link", "", "", "", "", text);
                        if !up {
                            healthy = false;
                        }
                    }
                } else if let Message::Notification {
                    feature_index,
                    function,
                    payload,
                    ..
                } = message
                {
                    if Some(feature_index) == battery_index {
                        csv.status("event", &payload, &format!("fn{function}"));
                    } else {
                        csv.line(
                            "event",
                            &to_hex(&payload[..4]),
                            "",
                            "",
                            "",
                            &format!("other notification index {feature_index} fn{function}"),
                        );
                    }
                }
            }
        }
    }
    println!("stopped after {:.0} s", start.elapsed().as_secs_f32());
    Ok(())
}

/// `spy-capture`: host mode only; 0x8110 start, print notifications, always stop.
fn spy_command<L: HidLink>(session: &mut HidppSession<L>, seconds: u64) -> Result<(), String> {
    sys::install_stop_handler();
    println!(
        "time,elapsed_s,event (stop with Ctrl+C or after {seconds} s; the spy is always stopped)"
    );
    let mut out = |line: SpyLine| {
        println!("{} {:7.2}s {}", sys::local_iso(), line.elapsed_s, line.text);
    };
    let report = spy_capture(
        session,
        Duration::from_secs(seconds),
        &|| STOP.load(Ordering::SeqCst),
        &mut out,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "done: {} spy events, {} other",
        report.spy_events, report.other_events
    );
    report
        .stop
        .map_err(|e| format!("the stop frame failed: {e}"))
}

fn run<L: HidLink>(session: &mut HidppSession<L>, command: Command) -> Result<(), String> {
    if let Command::BatteryLog {
        interval,
        duration,
        out,
    } = command
    {
        // Survives link loss, so it does its own discovery.
        return battery_log(session, interval, duration, out);
    }
    session
        .discover()
        .map_err(|e| format!("discovery failed: {e}"))?;
    match command {
        Command::State => print_state(session),
        Command::Host => {
            set_onboard_mode(session, OnboardMode::Host).map_err(|e| e.to_string())?;
            println!("switched to host mode, fn2 verified");
            print_state(session)
        }
        Command::Onboard => {
            set_onboard_mode(session, OnboardMode::Onboard).map_err(|e| e.to_string())?;
            println!("switched to onboard mode, fn2 verified");
            print_state(session)
        }
        Command::Dpi(value) => {
            require_host(session)?;
            // Range and step are checked against the device's own list first.
            let list = session
                .request_feature(
                    FeatureId::ADJUSTABLE_DPI,
                    dpi::FN_GET_SENSOR_DPI_LIST,
                    dpi::sensor_params(0),
                )
                .map_err(|e| format!("DPI list read failed: {e}"))?;
            let list = DpiList::parse(&list).map_err(|e| e.to_string())?;
            if !list.contains(value) {
                return Err(format!(
                    "DPI {value} is not in the device list {:?}",
                    list.entries
                ));
            }
            let got = set_dpi(session, value).map_err(|e| e.to_string())?;
            println!("DPI set and read back: {got}");
            print_state(session)
        }
        Command::Rate(hz) => {
            require_host(session)?;
            let list = session
                .request_feature(
                    FeatureId::REPORT_RATE,
                    report_rate::FN_GET_REPORT_RATE_LIST,
                    [0; 16],
                )
                .map_err(|e| format!("rate list read failed: {e}"))?;
            let list = report_rate::ReportRateList::parse(&list).map_err(|e| e.to_string())?;
            if !list.rates_hz().contains(&hz) {
                return Err(format!(
                    "{hz} Hz is not in the device list {:?}",
                    list.rates_hz()
                ));
            }
            let got = set_report_rate(session, hz).map_err(|e| e.to_string())?;
            println!("report rate set and read back: {got} Hz");
            print_state(session)
        }
        Command::Sectors(path) => {
            // Create first so an existing file is never touched.
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| format!("cannot create {path} (it must not exist): {e}"))?;
            let snapshot = read_snapshot(session).map_err(|e| format!("read failed: {e}"))?;
            let sectors: &BTreeMap<String, String> = &snapshot.onboard.sectors;
            let json = serde_json::to_string_pretty(sectors).map_err(|e| e.to_string())?;
            file.write_all(json.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|e| format!("cannot write {path}: {e}"))?;
            println!("{} sectors written to {path}", sectors.len());
            Ok(())
        }
        Command::BatteryLog { .. } => unreachable!("handled before discovery"),
        Command::SpyCapture { seconds } => spy_command(session, seconds),
        Command::SpyStop => {
            let reply = spy_stop(session).map_err(|e| format!("stop failed: {e}"))?;
            println!("0x8110 fn2 (stop spy) sent; reply: {}", to_hex(&reply));
            Ok(())
        }
        Command::WriteSame { sector, backup } => write_command(
            session,
            sector,
            &backup,
            ProfilePatch::default(),
            ProfileSource::Same,
            true,
        ),
        Command::WriteProfile {
            sector,
            backup,
            patch,
            source,
        } => write_command(session, sector, &backup, patch, source, false),
        Command::EnableProfile { sector, backup } => {
            directory_command(session, sector, true, &backup)
        }
        Command::DisableProfile { sector, backup } => {
            directory_command(session, sector, false, &backup)
        }
        Command::SelectProfile(sector) => {
            require_onboard(session)?;
            let reply = select_profile(session, sector).map_err(|e| e.to_string())?;
            println!("profile {sector:#06x} selected, fn4 verified; fn3 reply: {reply}");
            Ok(())
        }
        Command::Restore { sector, file } => {
            let image = read_backup_file(&file, sector)?;
            require_onboard(session)?;
            let log = restore_sector(session, sector, &image).map_err(report_failure)?;
            println!("sector {sector:#06x} restored from {file}, read back identical");
            print_log(&log);
            Ok(())
        }
    }
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
    if args.verbose {
        session.set_frame_trace(trace);
    }
    match run(&mut session, args.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(3)
        }
    }
}
