//! Safety allowlist and the only way to build request bytes for the wire.
//!
//! A transport builds every outgoing frame with [`Policy::check`] (or
//! [`Policy::check_bytes`]), which returns a [`CheckedRequest`]. That type has private
//! fields, no public constructor and no `Clone`, and it is the only type with
//! `encode()`, so the bytes that are sent are exactly the bytes that were checked.
//!
//! Layers, in this order. Everything not listed is refused.
//! 1. Never-send list, refused with [`PolicyError::Forbidden`] whatever the tier:
//!    VIA 0x0A (EEPROM reset), VIA 0x0B (bootloader jump), A7 0x15 (bootloader jump),
//!    every 0xAB (factory test) and 0xBA (receiver DFU).
//! 2. Allowlist v1, exactly the variants of [`Request`]. Other known writes (A9 13, 15,
//!    16, 1E, 21; A8 writes; VIA 05, 06, 0F; A7 04, 0C) and unknown commands are
//!    [`PolicyError::NotAllowlisted`].
//! 3. Tier gate. Read is always open. Volatile (RAM: A9 11, A9 14, VIA 07 03) and
//!    Persistent (flash: A9 1F, VIA 09 03) are opened per call and are independent:
//!    neither implies the other.
//! 4. Range checks. The profile count is not hardcoded: the device layer reads it from
//!    GET_PROFILES_INFO (A9 10, byte 3) and passes it with
//!    [`Policy::with_profile_count`]. Buffer, matrix, layer and LED bounds work the same way
//!    (`with_profile_buffer_size`, `with_macro_buffer_size`, `with_matrix`,
//!    `with_layer_count`, `with_led_count`). Until a size is supplied, the requests that
//!    need it are refused.

use thiserror::Error;

use crate::frame::{Frame, pad};
use crate::request::Request;

/// How far a request reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Reads only.
    Read,
    /// Changes the keyboard's RAM state, lost on replug [INF for A9 11 and 14].
    Volatile,
    /// Writes flash.
    Persistent,
}

/// A request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PolicyError {
    #[error("never-send command {command:#04x} {sub:#04x}")]
    Forbidden { command: u8, sub: u8 },
    #[error("not allowlisted: command {command:#04x} {sub:#04x}")]
    NotAllowlisted { command: u8, sub: u8 },
    #[error("{tier:?} tier is not enabled")]
    TierNotEnabled { tier: Tier },
    #[error("{field} {value} is outside {min}..={max}")]
    OutOfRange {
        field: &'static str,
        value: u32,
        min: u32,
        max: u32,
    },
    #[error("{field}: {reason}")]
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
    #[error("profile count not known yet (read A9 10 first)")]
    ProfileCountUnknown,
    #[error("profile {profile} is not below the profile count {count}")]
    ProfileOutOfRange { profile: u8, count: u8 },
    #[error("command {command:#04x}: unexpected nonzero bytes after the request")]
    TrailingBytes { command: u8 },
    #[error("{what} not known yet (the device layer must read it first)")]
    LimitUnknown { what: &'static str },
    #[error("{field} ends at {end}, past the limit {limit}")]
    PastEnd {
        field: &'static str,
        end: u32,
        limit: u32,
    },
    #[error("frame of {0} bytes is longer than 32")]
    FrameTooLong(usize),
}

/// A request that passed the policy. Not `Clone`, no public constructor.
///
/// ```compile_fail
/// let _ = keychron_proto::CheckedRequest {
///     frame: [0u8; 32],
///     request: keychron_proto::Request::GetViaProtocol,
/// };
/// ```
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<keychron_proto::CheckedRequest>();
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct CheckedRequest {
    frame: Frame,
    request: Request,
}

impl CheckedRequest {
    /// The 32 bytes to send (report ID 0 is added by the transport).
    pub fn encode(&self) -> Frame {
        self.frame
    }

    pub fn request(&self) -> &Request {
        &self.request
    }

    pub fn tier(&self) -> crate::policy::Tier {
        self.request.tier()
    }

    /// See [`Request::matches`].
    pub fn matches(&self, reply: &Frame) -> bool {
        self.request.matches(reply)
    }
}

/// The allowlist with its gates. Read only by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Policy {
    volatile: bool,
    persistent: bool,
    pub(crate) profile_count: Option<u8>,
    pub(crate) profile_buffer_size: Option<u16>,
    pub(crate) macro_buffer_size: Option<u16>,
    pub(crate) matrix: Option<(u8, u8)>,
    pub(crate) layer_count: Option<u8>,
    pub(crate) led_count: Option<u8>,
}

impl Policy {
    /// Read tier only.
    pub fn read_only() -> Self {
        Self::default()
    }

    /// Read plus exactly one extra tier, for a single call.
    pub fn allowing(tier: Tier) -> Self {
        Self::read_only().with_tier(tier)
    }

    /// Also open `tier`. Volatile and Persistent are separate gates.
    pub fn with_tier(mut self, tier: Tier) -> Self {
        match tier {
            Tier::Read => {}
            Tier::Volatile => self.volatile = true,
            Tier::Persistent => self.persistent = true,
        }
        self
    }

    /// Profile count from GET_PROFILES_INFO. Profiles must be below it.
    pub fn with_profile_count(mut self, count: u8) -> Self {
        self.profile_count = Some(count);
        self
    }

    /// Profile buffer size from A9 10 bytes 4..5. Bounds A9 12 reads.
    pub fn with_profile_buffer_size(mut self, size: u16) -> Self {
        self.profile_buffer_size = Some(size);
        self
    }

    /// Macro buffer size from VIA 0D. Bounds VIA 0E reads.
    pub fn with_macro_buffer_size(mut self, size: u16) -> Self {
        self.macro_buffer_size = Some(size);
        self
    }

    /// Key matrix from the keyboard definition. Bounds VIA 12 reads (with the layer
    /// count) and A8 06 rows.
    pub fn with_matrix(mut self, rows: u8, cols: u8) -> Self {
        self.matrix = Some((rows, cols));
        self
    }

    /// Layer count from VIA 11. Bounds VIA 12 reads.
    pub fn with_layer_count(mut self, layers: u8) -> Self {
        self.layer_count = Some(layers);
        self
    }

    /// LED count from A8 05. Bounds A8 09 and A8 0C reads.
    pub fn with_led_count(mut self, count: u8) -> Self {
        self.led_count = Some(count);
        self
    }

    pub fn allows(&self, tier: Tier) -> bool {
        match tier {
            Tier::Read => true,
            Tier::Volatile => self.volatile,
            Tier::Persistent => self.persistent,
        }
    }

    /// Check a typed request and return the only thing that can be encoded.
    pub fn check(&self, request: &Request) -> Result<CheckedRequest, PolicyError> {
        let tier = request.tier();
        if !self.allows(tier) {
            return Err(PolicyError::TierNotEnabled { tier });
        }
        request.validate(self)?;
        Ok(CheckedRequest {
            frame: request.to_frame(),
            request: *request,
        })
    }

    /// Check raw bytes (zero-padded to 32). The bytes are parsed into a typed request
    /// first, so the never-send list gets [`PolicyError::Forbidden`] and unknown
    /// commands [`PolicyError::NotAllowlisted`]. The encoded frame is rebuilt from the
    /// parsed request, never copied from the input.
    pub fn check_bytes(&self, bytes: &[u8]) -> Result<CheckedRequest, PolicyError> {
        if let Some(e) = Request::forbidden(bytes) {
            return Err(e);
        }
        let frame = pad(bytes).ok_or(PolicyError::FrameTooLong(bytes.len()))?;
        self.check(&Request::parse(&frame)?)
    }
}
