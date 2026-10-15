//! Support function discovery: which FunctionTypes the headset serves, and the
//! FunctionType to inquired type mapping that decides which requests are legal.
//!
//! Source: `PROTOCOL.md` section 5 (`CONNECT_RET_SUPPORT_FUNCTION`, mapping table)
//! and `function-types.md`. Reply layout `07 00 <n> [FunctionType][version] * n`
//! \[OBS on the WF-C710N, both tables\].

use std::collections::BTreeMap;

use serde::Serialize;

use crate::frame::{DataType, ReceivedFrame};

/// Which command table set a message belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Table {
    /// Table 1, data type `DATA_MDR` (0x0C).
    One,
    /// Table 2, data type `DATA_MDR_NO2` (0x0E).
    Two,
}

impl Table {
    /// The DATA data type of this table.
    pub fn data_type(self) -> DataType {
        match self {
            Table::One => DataType::DataMdr,
            Table::Two => DataType::DataMdrNo2,
        }
    }

    /// The table of a DATA frame, `None` for every other data type.
    pub fn from_data_type(dt: DataType) -> Option<Self> {
        match dt {
            DataType::DataMdr => Some(Table::One),
            DataType::DataMdrNo2 => Some(Table::Two),
            _ => None,
        }
    }
}

/// FunctionType ids (table 1) that this crate acts on.
pub mod function_id {
    pub const CODEC_INDICATOR: u8 = 0x12;
    pub const LR_BATTERY_LEVEL_WITH_THRESHOLD: u8 = 0x29;
    pub const CRADLE_BATTERY_LEVEL_WITH_THRESHOLD: u8 = 0x2A;
    pub const PRESET_EQ: u8 = 0x50;
    pub const MODE_NC_ASM_DUAL_AMBIENT_LEVEL: u8 = 0x6B;
    pub const GENERAL_SETTING_1: u8 = 0xD1;
    pub const GENERAL_SETTING_2: u8 = 0xD2;
    pub const RESET_SETTINGS: u8 = 0xF9;
}

/// Inquired type of NC/ASM for the WF-C710N (function 0x6B).
pub const NCASM_TYPE_DUAL_AMBIENT: u8 = 0x17;
/// Inquired type of preset EQ (function 0x50).
pub const EQ_TYPE_PRESET: u8 = 0x00;

/// FunctionType to NC/ASM inquired type, `PROTOCOL.md` section 5 table (\[OBS\] for 0x6B,
/// the rest is read from the app). `NC_AMB_TOGGLE` (0x69 to 0x30) and `ASM_SEAMLESS`
/// (0x67 to 0x22) take their values from `enums.md` `NcAsmInquiredType`.
pub fn ncasm_inquired_type(function: u8) -> Option<u8> {
    Some(match function {
        0x61 => 0x01,
        0x62 => 0x11,
        0x63 => 0x12,
        0x64 => 0x13,
        0x65 => 0x14,
        0x66 => 0x21,
        0x67 => 0x22,
        0x68 => 0x15,
        0x69 => 0x30,
        0x6A => 0x16,
        0x6B => 0x17,
        0x6C => 0x18,
        0x6D => 0x19,
        _ => return None,
    })
}

/// FunctionType to EQ inquired type, `PROTOCOL.md` section 5 table.
pub fn eq_inquired_type(function: u8) -> Option<u8> {
    Some(match function {
        0x50 => 0x00,
        0x51 => 0x01,
        0x52 => 0x02,
        0x53 => 0x03,
        0x54 => 0x30,
        0x55 => 0x31,
        0x57 => 0x04,
        _ => return None,
    })
}

/// Name of a FunctionType for the ids this crate cares about, `None` for the rest
/// (those stay raw).
pub fn function_name(table: Table, id: u8) -> Option<&'static str> {
    match (table, id) {
        (Table::One, 0x12) => Some("CODEC_INDICATOR"),
        (Table::One, 0x29) => Some("LR_BATTERY_LEVEL_WITH_THRESHOLD"),
        (Table::One, 0x2A) => Some("CRADLE_BATTERY_LEVEL_WITH_THRESHOLD"),
        (Table::One, 0x32) => Some("FW_UPDATE_MTK_TRANSFER_WITHOUT_DISCONNECTION"),
        (Table::One, 0x50) => Some("PRESET_EQ"),
        (Table::One, 0x61..=0x6D) => Some("NC_ASM"),
        (Table::One, 0x70) => Some("AUTO_NCASM"),
        (Table::One, 0xD1) => Some("GENERAL_SETTING_1"),
        (Table::One, 0xD2) => Some("GENERAL_SETTING_2"),
        (Table::One, 0xF9) => Some("RESET_SETTINGS"),
        (Table::Two, 0x31) => Some("SOURCE_SWITCH_CONTROL"),
        (Table::Two, 0x32) => Some("PAIRING_DEVICE_MANAGEMENT"),
        _ => None,
    }
}

/// One entry of a support function reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionEntry {
    pub id: u8,
    pub version: u8,
}

/// Parse `07 00 <n> [id][version] * n`. `None` when the payload is not exactly that.
pub fn parse_support_function(payload: &[u8]) -> Option<Vec<FunctionEntry>> {
    let [0x07, 0x00, n, rest @ ..] = payload else {
        return None;
    };
    if rest.len() != usize::from(*n) * 2 {
        return None;
    }
    Some(
        rest.as_chunks::<2>()
            .0
            .iter()
            .map(|p| FunctionEntry {
                id: p[0],
                version: p[1],
            })
            .collect(),
    )
}

/// A function of the table together with its table, for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionInfo {
    pub table: Table,
    pub id: u8,
    pub version: u8,
    pub name: Option<&'static str>,
}

/// Set of `(table, function id) -> version`. The policy reads it: a request for a
/// feature is legal only when the matching function is present.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionTable {
    map: BTreeMap<(Table, u8), u8>,
    epoch: Option<u64>,
}

impl FunctionTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the entries of `table` with a parsed reply.
    pub(crate) fn set_table(&mut self, table: Table, entries: &[FunctionEntry]) {
        self.map.retain(|(t, _), _| *t != table);
        for e in entries {
            self.map.insert((table, e.id), e.version);
        }
    }

    /// Take a received `07 00 ...` support function reply (table 1 on `DATA_MDR`,
    /// table 2 on `DATA_MDR_NO2`). This is the only way to fill the table. Returns the
    /// entry count, `None` for any other frame.
    pub fn absorb(&mut self, frame: &ReceivedFrame) -> Option<usize> {
        let table = Table::from_data_type(frame.data_type())?;
        let entries = parse_support_function(frame.payload())?;
        if self.epoch.is_some_and(|e| e != frame.epoch()) {
            self.map.clear();
        }
        self.epoch = Some(frame.epoch());
        self.set_table(table, &entries);
        Some(entries.len())
    }

    /// Link epoch of the frames this table was built from, `None` while empty. A frame
    /// from another epoch clears the table first. [`crate::policy::Policy::check`] denies
    /// requests that consult a table from a different epoch than the link's.
    pub fn epoch(&self) -> Option<u64> {
        self.epoch
    }

    pub fn contains(&self, table: Table, id: u8) -> bool {
        self.map.contains_key(&(table, id))
    }

    pub fn version(&self, table: Table, id: u8) -> Option<u8> {
        self.map.get(&(table, id)).copied()
    }

    /// Number of functions of one table.
    pub fn count(&self, table: Table) -> usize {
        self.map.keys().filter(|(t, _)| *t == table).count()
    }

    /// All entries, ordered by table then id.
    pub fn entries(&self) -> Vec<FunctionInfo> {
        self.map
            .iter()
            .map(|(&(table, id), &version)| FunctionInfo {
                table,
                id,
                version,
                name: function_name(table, id),
            })
            .collect()
    }

    /// Inquired types of NC/ASM reachable through present table 1 functions.
    pub fn ncasm_inquired_types(&self) -> Vec<u8> {
        self.map
            .keys()
            .filter(|(t, _)| *t == Table::One)
            .filter_map(|(_, id)| ncasm_inquired_type(*id))
            .collect()
    }

    /// True when a present table 1 function maps to NC/ASM inquired type `t`.
    pub fn has_ncasm_type(&self, t: u8) -> bool {
        self.ncasm_inquired_types().contains(&t)
    }

    /// True when a present table 1 function maps to EQ inquired type `t`.
    pub fn has_eq_type(&self, t: u8) -> bool {
        self.map
            .keys()
            .filter(|(tb, _)| *tb == Table::One)
            .any(|(_, id)| eq_inquired_type(*id) == Some(t))
    }
}
