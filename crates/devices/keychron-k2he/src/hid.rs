//! [`RawLink`] over hidapi. Not constructible outside this crate: use `Session::open`.

use std::ffi::CStr;

use hidapi::{HidApi, HidDevice};

use crate::link::{LinkError, READ_BUF_LEN, REPORT_LEN, RawLink};

/// The keyboard's or the receiver's raw HID interface, opened read and write.
pub struct HidapiLink {
    dev: HidDevice,
}

impl HidapiLink {
    pub(crate) fn open(api: &HidApi, path: &CStr) -> Result<Self, LinkError> {
        let dev = api
            .open_path(path)
            .map_err(|_| LinkError::Hid("could not open the raw HID interface".into()))?;
        Ok(Self { dev })
    }
}

impl RawLink for HidapiLink {
    fn write_report(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), LinkError> {
        let written = self
            .dev
            .write(report)
            .map_err(|_| LinkError::Hid("could not write to the raw HID interface".into()))?;
        // Same quirk as myhub-transport (hidapi 2.6.7 windows_native/mod.rs lines
        // 147-154): when WriteFile completes synchronously the backend returns Ok(0),
        // not the byte count. On Windows 0 is success; otherwise accept the full length.
        let complete = written >= REPORT_LEN || (cfg!(windows) && written == 0);
        if !complete {
            return Err(LinkError::ShortWrite {
                expected: REPORT_LEN,
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
        self.dev
            .read_timeout(buf, timeout_ms)
            .map_err(|_| LinkError::Hid("could not read from the raw HID interface".into()))
    }
}
