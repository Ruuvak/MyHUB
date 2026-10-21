//! Log file sink. `log_line!` prints to stderr as `eprintln!` did and, once
//! `init` ran, appends the same line with a UTC timestamp to `myhub.log` in the
//! app log dir. A release build on Windows has no stderr, so the file is the
//! only record of an installed build. Lines written to the file pass `redact`.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, Once, OnceLock, TryLockError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const LOG_FILE: &str = "myhub.log";
pub const OLD_LOG_FILE: &str = "myhub.1.log";
/// The file is rotated once it grows past this many bytes.
pub const MAX_BYTES: u64 = 1024 * 1024;
/// After a failed open the next try comes no sooner than this.
const REOPEN_PAUSE: Duration = Duration::from_secs(60);

/// Prints a line to stderr and, after `logfile::init`, to the log file.
/// Same arguments as `format!`; the text carries its own `myhub: ` prefix.
macro_rules! log_line {
    ($($arg:tt)*) => {
        $crate::logfile::write_line(::std::format_args!($($arg)*))
    };
}

struct Sink {
    dir: PathBuf,
    file: Option<File>,
    size: u64,
    cap: u64,
    /// When the last open of `myhub.log` failed; `None` while the file is open.
    failed_at: Option<Instant>,
}

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

impl Sink {
    fn open(dir: &Path, cap: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let file = open_append(&dir.join(LOG_FILE))?;
        let size = file.metadata()?.len();
        let mut sink = Self {
            dir: dir.to_path_buf(),
            file: Some(file),
            size,
            cap,
            failed_at: None,
        };
        if sink.size > sink.cap {
            sink.rotate();
        }
        Ok(sink)
    }

    /// Moves `myhub.log` to `myhub.1.log` (replacing it) and starts a new file. A
    /// failed rename (another process holds the file on Windows) is not fatal: the
    /// current file is reopened and keeps growing, with the next rotation one cap later.
    fn rotate(&mut self) {
        let current = self.dir.join(LOG_FILE);
        // The handle must be closed before the rename on Windows.
        self.file = None;
        let _ = fs::rename(&current, self.dir.join(OLD_LOG_FILE));
        self.size = 0;
        self.reopen(&current);
    }

    fn reopen(&mut self, path: &Path) {
        match open_append(path) {
            Ok(file) => {
                self.file = Some(file);
                self.failed_at = None;
            }
            Err(_) => {
                self.file = None;
                self.failed_at = Some(Instant::now());
            }
        }
    }

    /// Never fails: a line that cannot be written is lost (stderr has it) and the
    /// file is reopened on a later line, at most once per `REOPEN_PAUSE`.
    fn append(&mut self, line: &str) {
        if self.file.is_none() {
            if self.failed_at.is_some_and(|t| t.elapsed() < REOPEN_PAUSE) {
                return;
            }
            let path = self.dir.join(LOG_FILE);
            self.reopen(&path);
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if file.write_all(line.as_bytes()).is_err() {
            self.file = None;
            self.failed_at = Some(Instant::now());
            return;
        }
        self.size += line.len() as u64;
        if self.size > self.cap {
            self.rotate();
        }
    }
}

/// Starts the file sink in `dir` and installs the panic hook. A second call
/// replaces the sink.
pub fn init(dir: &Path) -> io::Result<()> {
    init_with_cap(dir, MAX_BYTES)?;
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("(no message)");
            let place = info
                .location()
                .map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
            // try_lock: the panic may have happened while this thread held the sink.
            emit(format_args!("myhub: panic: {message}{place}"), false);
            previous(info);
        }));
    });
    Ok(())
}

fn init_with_cap(dir: &Path, cap: u64) -> io::Result<()> {
    let sink = Sink::open(dir, cap)?;
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    Ok(())
}

/// Backend of `log_line!`.
pub fn write_line(args: fmt::Arguments<'_>) {
    emit(args, true);
}

fn emit(args: fmt::Arguments<'_>, blocking: bool) {
    eprintln!("{args}");
    let mut guard: MutexGuard<'_, Option<Sink>> = if blocking {
        SINK.lock().unwrap_or_else(|e| e.into_inner())
    } else {
        match SINK.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        }
    };
    if let Some(sink) = guard.as_mut() {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let line = format!("{} {}\n", format_utc(secs), redact(&args.to_string()));
        sink.append(&line);
    }
}

/// Masks what identifies the machine: `/dev/hidrawN` becomes `/dev/hidraw*`, a Windows
/// `\?\...` device path becomes `\?\*`, and the user's home directory becomes `~`.
fn redact(text: &str) -> String {
    static HOME: OnceLock<Option<String>> = OnceLock::new();
    let home = HOME.get_or_init(|| {
        ["HOME", "USERPROFILE"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| v.len() > 1)
    });
    redact_with(text, home.as_deref())
}

fn redact_with(text: &str, home: Option<&str>) -> String {
    let mut out = match home {
        Some(home) => text.replace(home, "~"),
        None => text.to_owned(),
    };
    out = mask_after(&out, "/dev/hidraw", |c| c.is_ascii_digit(), "*");
    mask_after(&out, r"\\?\", |c| !c.is_whitespace() && c != '"', "*")
}

/// Replaces the run of chars matching `run` that follows each `prefix` with `mask`.
fn mask_after(text: &str, prefix: &str, run: impl Fn(char) -> bool, mask: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(prefix) {
        let (head, tail) = rest.split_at(at + prefix.len());
        out.push_str(head);
        let taken = tail.find(|c| !run(c)).unwrap_or(tail.len());
        if taken > 0 {
            out.push_str(mask);
        }
        rest = &tail[taken..];
    }
    out.push_str(rest);
    out
}

/// `YYYY-MM-DD HH:MM:SS` in UTC for seconds since the Unix epoch.
pub fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_epochs() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        // 2024-02-29 12:34:56 UTC (leap day).
        assert_eq!(format_utc(1_709_210_096), "2024-02-29 12:34:56");
        // 2026-10-21 00:00:00 UTC.
        assert_eq!(format_utc(1_792_540_800), "2026-10-21 00:00:00");
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("myhub-logfile-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn rotates_past_the_cap() {
        let dir = test_dir("rotate");
        let mut sink = Sink::open(&dir, 100).unwrap();
        sink.append(&format!("{}\n", "a".repeat(60)));
        assert!(!dir.join(OLD_LOG_FILE).exists());
        // Crosses the cap: this line lands in the old file, the next in a new one.
        sink.append(&format!("{}\n", "b".repeat(60)));
        assert!(dir.join(OLD_LOG_FILE).exists());
        sink.append("c\n");
        let old = fs::read_to_string(dir.join(OLD_LOG_FILE)).unwrap();
        assert_eq!(old.lines().count(), 2);
        assert_eq!(fs::read_to_string(dir.join(LOG_FILE)).unwrap(), "c\n");
        // A second rotation replaces the old file.
        sink.append(&format!("{}\n", "d".repeat(120)));
        let old = fs::read_to_string(dir.join(OLD_LOG_FILE)).unwrap();
        assert_eq!(old.lines().count(), 2);
        assert!(old.starts_with('c'));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_rename_keeps_logging() {
        let dir = test_dir("rename");
        // A directory in the way makes the rename fail on every platform.
        fs::create_dir_all(dir.join(OLD_LOG_FILE)).unwrap();
        let mut sink = Sink::open(&dir, 10).unwrap();
        sink.append("0123456789ab\n");
        sink.append("next\n");
        let text = fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        assert_eq!(text, "0123456789ab\nnext\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn redacts_device_and_home_paths() {
        let text = r"open /dev/hidraw12 failed, /dev/hidraw3: no; \\?\hid#vid_046d&pid_c547#7&1 x; /home/ann/.config/a.json";
        assert_eq!(
            redact_with(text, Some("/home/ann")),
            r"open /dev/hidraw* failed, /dev/hidraw*: no; \\?\* x; ~/.config/a.json"
        );
        assert_eq!(redact_with("plain /dev/hidraw", None), "plain /dev/hidraw");
    }
}
