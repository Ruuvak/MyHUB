//! Targets without an RFCOMM backend: everything is `Unsupported`.

use crate::{ByteLink, PairedDevice, RfcommError, ServiceUuid};

/// Never constructed on this target.
#[derive(Debug)]
pub struct RfcommLink(());

pub fn list_paired() -> Result<Vec<PairedDevice>, RfcommError> {
    Err(RfcommError::Unsupported)
}

pub fn offers_service(_device: &PairedDevice, _uuid: &ServiceUuid) -> Result<bool, RfcommError> {
    Err(RfcommError::Unsupported)
}

pub fn connect(_device: &PairedDevice, _uuid: &ServiceUuid) -> Result<RfcommLink, RfcommError> {
    Err(RfcommError::Unsupported)
}

impl ByteLink for RfcommLink {
    fn write_all(&mut self, _bytes: &[u8]) -> Result<(), RfcommError> {
        Err(RfcommError::Unsupported)
    }

    fn read(&mut self, _buf: &mut [u8], _timeout_ms: u32) -> Result<usize, RfcommError> {
        Err(RfcommError::Unsupported)
    }
}
