//! Feature IDs, Root / FeatureSet reply parsers and the runtime feature table.
//!
//! Feature indexes are device specific and must always be resolved at runtime
//! through Root (0x0000) GetFeature or FeatureSet (0x0001) GetFeatureID. Never
//! hardcode an index outside tests.

use std::fmt;

use serde::Serialize;
use thiserror::Error;

use crate::error::ParseError;

/// A 16-bit HID++ 2.0 feature ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct FeatureId(pub u16);

impl FeatureId {
    /// IRoot. Always at index 0.
    pub const ROOT: Self = Self(0x0000);
    /// IFeatureSet.
    pub const FEATURE_SET: Self = Self(0x0001);
    /// Unified battery.
    pub const UNIFIED_BATTERY: Self = Self(0x1004);
    /// Adjustable DPI.
    pub const ADJUSTABLE_DPI: Self = Self(0x2201);
    /// Report rate.
    pub const REPORT_RATE: Self = Self(0x8060);
    /// Onboard profiles.
    pub const ONBOARD_PROFILES: Self = Self(0x8100);
    /// Mouse button spy (public name MOUSE_BUTTON_SPY): fn 1 start, fn 2 stop.
    pub const MOUSE_BUTTON_SPY: Self = Self(0x8110);
}

impl fmt::Display for FeatureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:04X}", self.0)
    }
}

/// Root function 0: GetFeature(featureId) -> index, type, version.
pub const ROOT_FN_GET_FEATURE: u8 = 0;
/// FeatureSet function 0: GetCount.
pub const FEATURE_SET_FN_GET_COUNT: u8 = 0;
/// FeatureSet function 1: GetFeatureID(index).
pub const FEATURE_SET_FN_GET_FEATURE_ID: u8 = 1;

/// Request parameters for Root GetFeature.
pub fn root_get_feature_params(id: FeatureId) -> [u8; 16] {
    let mut params = [0u8; 16];
    params[..2].copy_from_slice(&id.0.to_be_bytes());
    params
}

/// Request parameters for FeatureSet GetFeatureID.
pub fn feature_set_get_feature_id_params(index: u8) -> [u8; 16] {
    let mut params = [0u8; 16];
    params[0] = index;
    params
}

fn need(payload: &[u8], need: usize) -> Result<(), ParseError> {
    if payload.len() < need {
        Err(ParseError::TooShort {
            need,
            got: payload.len(),
        })
    } else {
        Ok(())
    }
}

/// Reply of Root GetFeature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RootFeatureReply {
    /// Feature index on the device. 0 means "not found" (index 0 is Root).
    pub index: u8,
    /// Feature type flags. Bits 0x40 and 0x20 mark hidden/engineering features.
    pub feature_type: u8,
    pub version: u8,
}

impl RootFeatureReply {
    /// `true` when the device implements the feature.
    pub fn is_present(&self) -> bool {
        self.index != 0
    }
}

/// Parses a Root GetFeature reply payload.
pub fn parse_root_get_feature(payload: &[u8]) -> Result<RootFeatureReply, ParseError> {
    need(payload, 3)?;
    Ok(RootFeatureReply {
        index: payload[0],
        feature_type: payload[1],
        version: payload[2],
    })
}

/// Parses a FeatureSet GetCount reply payload. The count excludes Root, so
/// valid indexes are `1..=count`.
pub fn parse_feature_count(payload: &[u8]) -> Result<u8, ParseError> {
    need(payload, 1)?;
    Ok(payload[0])
}

/// Reply of FeatureSet GetFeatureID: id (BE16), type, version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FeatureIdReply {
    pub id: FeatureId,
    pub feature_type: u8,
    pub version: u8,
}

/// Parses a FeatureSet GetFeatureID reply payload.
pub fn parse_feature_id(payload: &[u8]) -> Result<FeatureIdReply, ParseError> {
    need(payload, 4)?;
    Ok(FeatureIdReply {
        id: FeatureId(u16::from_be_bytes([payload[0], payload[1]])),
        feature_type: payload[2],
        version: payload[3],
    })
}

/// One row of the feature table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FeatureEntry {
    pub index: u8,
    pub id: FeatureId,
    pub feature_type: u8,
    pub version: u8,
}

/// A feature table entry was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FeatureTableError {
    /// Index 0 is Root and nothing else (a Root GetFeature reply of 0 means
    /// "not found"). Root is never stored at another index.
    #[error("feature {id} cannot be stored at index {index}: index 0 is Root only")]
    RootIndexMismatch { index: u8, id: FeatureId },
}

/// Index <-> feature mapping of one device, built incrementally.
///
/// Indexes are only valid for the device session they were read in. The
/// transport must discard (or rebuild) the table on a HID++ 1.0 `0x41` device
/// connection notification, because the device may have reset or been replaced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FeatureTable {
    entries: Vec<FeatureEntry>,
}

impl FeatureTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an entry from a GetFeatureID reply for `index`. An existing entry
    /// with the same index is replaced. Index 0 is accepted only for Root.
    pub fn insert(&mut self, index: u8, reply: FeatureIdReply) -> Result<(), FeatureTableError> {
        self.insert_entry(FeatureEntry {
            index,
            id: reply.id,
            feature_type: reply.feature_type,
            version: reply.version,
        })
    }

    /// Adds or replaces a full entry. Index 0 is accepted only for Root, and
    /// Root only at index 0.
    pub fn insert_entry(&mut self, entry: FeatureEntry) -> Result<(), FeatureTableError> {
        if (entry.index == 0) != (entry.id == FeatureId::ROOT) {
            return Err(FeatureTableError::RootIndexMismatch {
                index: entry.index,
                id: entry.id,
            });
        }
        match self.entries.iter_mut().find(|e| e.index == entry.index) {
            Some(slot) => *slot = entry,
            None => self.entries.push(entry),
        }
        Ok(())
    }

    /// First entry with this feature ID.
    pub fn by_id(&self, id: FeatureId) -> Option<&FeatureEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Entry at a device feature index.
    pub fn by_index(&self, index: u8) -> Option<&FeatureEntry> {
        self.entries.iter().find(|e| e.index == index)
    }

    /// Feature index for `id`. Root is always at index 0 by protocol.
    pub fn index_of(&self, id: FeatureId) -> Option<u8> {
        if id == FeatureId::ROOT {
            return Some(0);
        }
        self.by_id(id).map(|e| e.index)
    }

    /// Entries in insertion order.
    pub fn entries(&self) -> &[FeatureEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
