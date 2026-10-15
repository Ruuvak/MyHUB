//! [`HidLink`] over hidapi. Not constructible outside this crate: use
//! `HidppSession::open`.

use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice};

use crate::discovery::ReceiverPath;
use crate::link::{HidLink, LinkError, READ_BUF_LEN, WRITE_LEN};

/// Length of one poll slice while two collections are read.
const SLICE: Duration = Duration::from_millis(10);

/// A handle that can only read. It has no write method, so the short-report
/// collection can never be written to.
struct ReadOnlyDevice(HidDevice);

impl ReadOnlyDevice {
    fn poll(&self, buf: &mut [u8]) -> Result<usize, LinkError> {
        self.0
            .read_timeout(buf, 0)
            .map_err(|e| LinkError::Hid(e.to_string()))
    }
}

/// The receiver's long-report collection (read and write) and, when it could
/// be opened, the short-report collection (read only; Windows).
pub struct HidapiLink {
    long: HidDevice,
    short: Option<ReadOnlyDevice>,
}

impl HidapiLink {
    /// Opens the long collection; failure is an error. Opens the short
    /// collection when the receiver has one; failure there is not an error and
    /// shows up as `short_reports_available() == false`.
    pub(crate) fn open(api: &HidApi, receiver: &ReceiverPath) -> Result<Self, LinkError> {
        let long = api
            .open_path(receiver.path())
            .map_err(|e| LinkError::Hid(e.to_string()))?;
        let short = receiver
            .short_path()
            .and_then(|path| api.open_path(path).ok())
            .map(ReadOnlyDevice);
        Ok(Self { long, short })
    }
}

impl HidLink for HidapiLink {
    fn write_report(&mut self, report: &[u8; WRITE_LEN]) -> Result<(), LinkError> {
        let written = self
            .long
            .write(report)
            .map_err(|e| LinkError::Hid(e.to_string()))?;
        // hidapi 2.6.7 windows_native/mod.rs lines 147-154: when WriteFile
        // completes synchronously the backend returns Ok(0), not the byte
        // count. So on Windows 0 is success; otherwise accept the full length.
        let complete = written >= WRITE_LEN || (cfg!(windows) && written == 0);
        if !complete {
            return Err(LinkError::ShortWrite {
                expected: WRITE_LEN,
                written,
            });
        }
        Ok(())
    }

    fn read_report(
        &mut self,
        buf: &mut [u8; READ_BUF_LEN],
        timeout_ms: i32,
    ) -> Result<usize, LinkError> {
        let Some(short) = &self.short else {
            return self
                .long
                .read_timeout(buf, timeout_ms)
                .map_err(|e| LinkError::Hid(e.to_string()));
        };
        // Two collections: poll the short one without waiting, and wait for
        // the long one in small slices so neither starves the other.
        let timeout = Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(0));
        let started = Instant::now();
        loop {
            let n = short.poll(buf)?;
            if n > 0 {
                return Ok(n);
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            let slice = remaining.min(SLICE);
            let n = self
                .long
                .read_timeout(buf, i32::try_from(slice.as_millis()).unwrap_or(0))
                .map_err(|e| LinkError::Hid(e.to_string()))?;
            if n > 0 {
                return Ok(n);
            }
            if remaining <= SLICE {
                return Ok(0);
            }
        }
    }

    fn short_reports_available(&self) -> bool {
        // Linux has one node for both report IDs; Windows needs the sibling.
        self.short.is_some() || !cfg!(windows)
    }
}
