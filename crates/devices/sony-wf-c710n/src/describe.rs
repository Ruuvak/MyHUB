//! One line per incoming message for the probe's `watch`. Never prints raw payloads:
//! unknown commands show the command byte and the length only, and playback metadata
//! (command 0xA9, track titles) is hidden.

use mdr::messages::{Message, Origin, eq_preset_name};
use mdr::{Incoming, Table};

/// Command byte of the playback metadata notifications (track information).
const PLAYBACK_METADATA: u8 = 0xA9;

pub fn describe(incoming: &Incoming) -> String {
    let kind = match incoming.origin {
        Origin::Reply => "reply",
        Origin::Notification => "notification",
        Origin::Other => "other",
    };
    let table = match incoming.table {
        Table::One => 1,
        Table::Two => 2,
    };
    let what = match &incoming.message {
        Message::ProtocolInfo(p) => format!("protocol info {:#010x}", p.version),
        Message::DeviceInfo(d) => format!("device info {d:?}"),
        Message::SupportFunction(f) => format!("support function table, {} entries", f.len()),
        Message::BatteryLr(b) => format!(
            "battery left {}% ({:?}), right {}% ({:?})",
            b.left.level, b.left.charging, b.right.level, b.right.charging
        ),
        Message::Cradle(c) => format!("case battery {}% ({:?})", c.cell.level, c.cell.charging),
        Message::NcAsmCapability(_) => "NC/ASM capability".to_string(),
        Message::NcAsmParam(p) => format!(
            "NC/ASM {:?}, ambient {:?}, level {}, {:?}",
            p.mode, p.ambient_mode, p.level, p.status
        ),
        Message::EqCapability(_) => "EQ capability".to_string(),
        Message::EqParam(p) => format!(
            "EQ preset {} bands {:?}",
            eq_preset_name(p.preset).unwrap_or("unknown"),
            p.bands
        ),
        Message::EqBandInfo(i) => format!("EQ band info, {} entries", i.entries.len()),
        Message::EqChanged => "EQ changed".to_string(),
        Message::GeneralSetting(g) => {
            format!("{:?} {}", g.kind, if g.enabled { "on" } else { "off" })
        }
        Message::Alert(a) => format!("alert {:#04x} action {:#04x}", a.message_id, a.action_type),
        Message::Codec(c) => format!("codec {c:?}"),
        Message::Unknown(u) if u.command == PLAYBACK_METADATA => {
            "playback metadata (hidden)".to_string()
        }
        Message::Unknown(u) => format!(
            "unhandled command {:#04x} ({} bytes)",
            u.command,
            u.raw.len()
        ),
    };
    format!("[table {table} {kind}] {what}")
}
