//! HID++ report framing (decode side).
//!
//! Report IDs: `0x10` short (exactly 7 bytes), `0x11` long (exactly 20 bytes).
//! Byte layout: `[report id, device index, feature index, function << 4 | swId,
//! params...]`. swId 0 marks a device-originated notification; a non-zero swId
//! echoes the software ID of the request, which is how replies are matched.
//!
//! This module cannot produce outgoing bytes. The only encoder is
//! [`crate::policy::CheckedRequest::encode`], so every frame that can reach the
//! wire has passed the safety policy.

use serde::Serialize;
use thiserror::Error;

use crate::policy::CheckedRequest;

/// Short report ID.
pub const REPORT_ID_SHORT: u8 = 0x10;
/// Long report ID.
pub const REPORT_ID_LONG: u8 = 0x11;
/// Short report length in bytes.
pub const SHORT_LEN: usize = 7;
/// Long report length in bytes.
pub const LONG_LEN: usize = 20;
/// Feature index byte that marks a HID++ 2.0 error report.
pub const ERROR_FEATURE_INDEX: u8 = 0xFF;
/// Byte 2 values that are HID++ 1.0 notifications \[OBS\]. 0x41 is the device
/// connection notification: the transport must invalidate its
/// [`crate::feature::FeatureTable`] when it sees one.
pub const HIDPP10_NOTIFICATION_SUB_IDS: [u8; 4] = [0x40, 0x41, 0x49, 0x4B];
/// HID++ 1.0 error sub ID.
pub const HIDPP10_ERROR_SUB_ID: u8 = 0x8F;

/// Default software ID of this project. Proven by the read-only probe. Valid
/// software IDs are 2..=15: 0 is the notification marker and 1 is used by the
/// Linux kernel driver. The transport makes the ID configurable.
pub const DEFAULT_SW_ID: u8 = 0x0B;
/// Lowest software ID this crate will put on the wire.
pub const MIN_SW_ID: u8 = 2;
/// Highest software ID (one nibble).
pub const MAX_SW_ID: u8 = 15;

/// Framing errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FrameError {
    /// Report ID 0x10 needs exactly 7 bytes, 0x11 exactly 20.
    #[error("bad report length {0}")]
    BadLength(usize),
    /// Report ID is neither 0x10 nor 0x11.
    #[error("unsupported report id {0:#04x}")]
    UnsupportedReportId(u8),
}

/// A decoded incoming report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Message {
    /// Reply to a request (non-zero software ID).
    Response {
        device_index: u8,
        feature_index: u8,
        function: u8,
        sw_id: u8,
        payload: [u8; 16],
    },
    /// HID++ 2.0 error (`11 dev FF featIdx fn|sw err`).
    Error {
        device_index: u8,
        feature_index: u8,
        function: u8,
        sw_id: u8,
        code: u8,
    },
    /// HID++ 2.0 event (software ID 0). `function` is the event number.
    Notification {
        device_index: u8,
        feature_index: u8,
        function: u8,
        payload: [u8; 16],
    },
    /// HID++ 1.0 report: byte 2 in {0x40, 0x41, 0x49, 0x4B} or the error ID
    /// 0x8F. `payload` is bytes 3..20 of the (zero padded) report. For 0x8F,
    /// `payload[0]` is the failed request's feature index, `payload[1]` its
    /// `function << 4 | swId` and `payload[2]` the error code.
    Hidpp10 {
        device_index: u8,
        sub_id: u8,
        payload: [u8; 17],
    },
}

/// Decodes a report: `0x10` must be exactly 7 bytes (zero padded on decode),
/// `0x11` exactly 20 bytes.
pub fn decode(report: &[u8]) -> Result<Message, FrameError> {
    let len = report.len();
    match report.first() {
        None => return Err(FrameError::BadLength(0)),
        Some(&REPORT_ID_SHORT) if len == SHORT_LEN => {}
        Some(&REPORT_ID_LONG) if len == LONG_LEN => {}
        Some(&REPORT_ID_SHORT | &REPORT_ID_LONG) => return Err(FrameError::BadLength(len)),
        Some(&other) => return Err(FrameError::UnsupportedReportId(other)),
    }
    let mut buf = [0u8; LONG_LEN];
    buf[..len].copy_from_slice(report);

    let device_index = buf[1];
    let byte2 = buf[2];

    if byte2 == HIDPP10_ERROR_SUB_ID || HIDPP10_NOTIFICATION_SUB_IDS.contains(&byte2) {
        let mut payload = [0u8; 17];
        payload.copy_from_slice(&buf[3..]);
        return Ok(Message::Hidpp10 {
            device_index,
            sub_id: byte2,
            payload,
        });
    }

    if byte2 == ERROR_FEATURE_INDEX {
        return Ok(Message::Error {
            device_index,
            feature_index: buf[3],
            function: buf[4] >> 4,
            sw_id: buf[4] & 0x0F,
            code: buf[5],
        });
    }

    let function = buf[3] >> 4;
    let sw_id = buf[3] & 0x0F;
    let mut payload = [0u8; 16];
    payload.copy_from_slice(&buf[4..]);
    if sw_id == 0 {
        Ok(Message::Notification {
            device_index,
            feature_index: byte2,
            function,
            payload,
        })
    } else {
        Ok(Message::Response {
            device_index,
            feature_index: byte2,
            function,
            sw_id,
            payload,
        })
    }
}

/// `true` when `message` answers `request`: same device index, feature index,
/// function and software ID, as a response, as a HID++ 2.0 error, or as a
/// HID++ 1.0 `0x8F` error (sent for example when the device is offline). The
/// caller tells the forms apart by matching on the message.
pub fn is_reply_to(request: &CheckedRequest, message: &Message) -> bool {
    match message {
        Message::Response {
            device_index,
            feature_index,
            function,
            sw_id,
            ..
        }
        | Message::Error {
            device_index,
            feature_index,
            function,
            sw_id,
            ..
        } => {
            *device_index == request.device_index()
                && *feature_index == request.feature_index()
                && *function == request.function()
                && *sw_id == request.sw_id()
        }
        Message::Hidpp10 {
            device_index,
            sub_id: HIDPP10_ERROR_SUB_ID,
            payload,
        } => {
            *device_index == request.device_index()
                && payload[0] == request.feature_index()
                && payload[1] == ((request.function() << 4) | request.sw_id())
        }
        Message::Notification { .. } | Message::Hidpp10 { .. } => false,
    }
}
