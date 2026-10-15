//! Handshake and state snapshot. Read-tier requests only.

use mdr::function::FunctionInfo;
use mdr::function::{EQ_TYPE_PRESET, NCASM_TYPE_DUAL_AMBIENT, function_id};
use mdr::messages::{
    self, AudioCodec, BatteryLr, Cradle, DeviceInfo, DeviceInfoKind, EqCapability, EqParam,
    GeneralSettingKind, Message, NcAsmCapability, NcAsmParam,
};
use mdr::{FunctionTable, Incoming, Request, Table};
use myhub_rfcomm::ByteLink;
use serde::Serialize;

use crate::session::{Session, SessionError};

/// Who the headset says it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub protocol_version: u32,
    pub model: String,
    pub firmware: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NcAsmSnapshot {
    pub capability: NcAsmCapability,
    pub param: NcAsmParam,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqSnapshot {
    pub capability: EqCapability,
    pub param: EqParam,
}

/// Everything read from the headset. Holds no address and no capability info string.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub identity: Identity,
    pub functions: Vec<FunctionInfo>,
    pub battery: Option<BatteryLr>,
    pub cradle: Option<Cradle>,
    pub ncasm: Option<NcAsmSnapshot>,
    pub eq: Option<EqSnapshot>,
    /// `Some(true)` when multipoint is on.
    pub multipoint: Option<bool>,
    /// `Some(true)` when sidetone is on.
    pub sidetone: Option<bool>,
    pub codec: Option<AudioCodec>,
}

/// Protocol info, model, firmware and both support function tables. Skips the capability
/// info request (`02 00`): its reply is the headset address.
pub fn handshake<L: ByteLink>(session: &mut Session<L>) -> Result<Identity, SessionError> {
    let Message::ProtocolInfo(info) = session
        .request(&messages::protocol_info(Table::One))?
        .message
    else {
        return Err(SessionError::Unexpected("protocol info"));
    };
    let model = match session
        .request(&messages::device_info(
            Table::One,
            DeviceInfoKind::ModelName,
        ))?
        .message
    {
        Message::DeviceInfo(DeviceInfo::Model(s)) => s,
        _ => return Err(SessionError::Unexpected("model name")),
    };
    let firmware = match session
        .request(&messages::device_info(
            Table::One,
            DeviceInfoKind::FirmwareVersion,
        ))?
        .message
    {
        Message::DeviceInfo(DeviceInfo::Firmware(s)) => s,
        _ => return Err(SessionError::Unexpected("firmware version")),
    };
    for table in [Table::One, Table::Two] {
        let reply = session.request(&messages::support_function(table))?;
        if !matches!(reply.message, Message::SupportFunction(_)) {
            return Err(SessionError::Unexpected("support function table"));
        }
    }
    Ok(Identity {
        protocol_version: info.version,
        model,
        firmware,
    })
}

/// Ask for an optional value. A missing reply is `None`; a dropped link is an error.
fn ask<L: ByteLink>(
    session: &mut Session<L>,
    request: &Request,
) -> Result<Option<Incoming>, SessionError> {
    match session.request(request) {
        Ok(reply) => Ok(Some(reply)),
        Err(SessionError::Timeout) => Ok(None),
        Err(e) => Err(e),
    }
}

fn has(functions: &FunctionTable, id: u8) -> bool {
    functions.contains(Table::One, id)
}

/// Handshake, then read only what the support function table lists.
pub fn read_snapshot<L: ByteLink>(session: &mut Session<L>) -> Result<Snapshot, SessionError> {
    let identity = handshake(session)?;

    let battery = if has(
        session.functions(),
        function_id::LR_BATTERY_LEVEL_WITH_THRESHOLD,
    ) {
        match ask(session, &messages::battery_lr())?.map(|i| i.message) {
            Some(Message::BatteryLr(b)) => Some(b),
            _ => None,
        }
    } else {
        None
    };
    let cradle = if has(
        session.functions(),
        function_id::CRADLE_BATTERY_LEVEL_WITH_THRESHOLD,
    ) {
        match ask(session, &messages::battery_cradle())?.map(|i| i.message) {
            Some(Message::Cradle(c)) => Some(c),
            _ => None,
        }
    } else {
        None
    };

    let ncasm = if session.functions().has_ncasm_type(NCASM_TYPE_DUAL_AMBIENT) {
        let cap = ask(
            session,
            &messages::ncasm_capability(NCASM_TYPE_DUAL_AMBIENT),
        )?;
        let param = ask(session, &messages::ncasm_get_param(NCASM_TYPE_DUAL_AMBIENT))?;
        match (cap.map(|i| i.message), param.map(|i| i.message)) {
            (Some(Message::NcAsmCapability(capability)), Some(Message::NcAsmParam(param))) => {
                Some(NcAsmSnapshot { capability, param })
            }
            _ => None,
        }
    } else {
        None
    };

    let eq = if session.functions().has_eq_type(EQ_TYPE_PRESET) {
        let cap = ask(session, &messages::eq_capability())?;
        let param = ask(session, &messages::eq_get_param())?;
        match (cap.map(|i| i.message), param.map(|i| i.message)) {
            (Some(Message::EqCapability(capability)), Some(Message::EqParam(param))) => {
                Some(EqSnapshot { capability, param })
            }
            _ => None,
        }
    } else {
        None
    };

    let general = |session: &mut Session<L>, kind: GeneralSettingKind| {
        if !has(session.functions(), kind.to_u8()) {
            return Ok(None);
        }
        Ok::<_, SessionError>(
            match ask(session, &messages::general_setting_get(kind))?.map(|i| i.message) {
                Some(Message::GeneralSetting(g)) if g.kind == kind => Some(g.enabled),
                _ => None,
            },
        )
    };
    let multipoint = general(session, GeneralSettingKind::Multipoint)?;
    let sidetone = general(session, GeneralSettingKind::Sidetone)?;

    let codec = if has(session.functions(), function_id::CODEC_INDICATOR) {
        match ask(session, &messages::codec_status())?.map(|i| i.message) {
            Some(Message::Codec(c)) => Some(c),
            _ => None,
        }
    } else {
        None
    };

    Ok(Snapshot {
        identity,
        functions: session.functions().entries(),
        battery,
        cradle,
        ncasm,
        eq,
        multipoint,
        sidetone,
        codec,
    })
}

/// Which part of the snapshot to read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Battery,
    NcAsm,
    Eq,
}

/// What [`Snapshot::apply`] did with a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// A value of the snapshot was replaced (compare before and after for a change).
    Updated,
    /// The message only says that something changed: read the item with a GET.
    Reread(Item),
    /// Not about the snapshot.
    Ignored,
}

impl Snapshot {
    /// Folds one incoming message (battery, NC/ASM, EQ, multipoint, sidetone) into the
    /// snapshot. A bare EQ change and an unparsable battery, NC/ASM or EQ message ask for a
    /// re-read of that item.
    pub fn apply(&mut self, incoming: &Incoming) -> Applied {
        match &incoming.message {
            Message::BatteryLr(b) => self.battery = Some(*b),
            Message::Cradle(c) => self.cradle = Some(*c),
            Message::NcAsmParam(p) => match self.ncasm.as_mut() {
                Some(nc) => nc.param = *p,
                None => return Applied::Reread(Item::NcAsm),
            },
            Message::EqParam(p) => match self.eq.as_mut() {
                Some(eq) => eq.param = p.clone(),
                None => return Applied::Reread(Item::Eq),
            },
            Message::EqChanged => return Applied::Reread(Item::Eq),
            Message::GeneralSetting(g) => match g.kind {
                GeneralSettingKind::Multipoint => self.multipoint = Some(g.enabled),
                GeneralSettingKind::Sidetone => self.sidetone = Some(g.enabled),
            },
            Message::Unknown(u) => {
                return match u.command {
                    messages::command::POWER_NTFY_STATUS => Applied::Reread(Item::Battery),
                    messages::command::NCASM_NTFY_PARAM => Applied::Reread(Item::NcAsm),
                    messages::command::EQEBB_NTFY_PARAM => Applied::Reread(Item::Eq),
                    _ => Applied::Ignored,
                };
            }
            _ => return Applied::Ignored,
        }
        Applied::Updated
    }
}

/// Reads one item again with a read-tier GET and stores it. A missing reply keeps the old
/// value; a dropped link is an error.
pub fn reread<L: ByteLink>(
    session: &mut Session<L>,
    snapshot: &mut Snapshot,
    item: Item,
) -> Result<(), SessionError> {
    match item {
        Item::Battery => {
            if has(
                session.functions(),
                function_id::LR_BATTERY_LEVEL_WITH_THRESHOLD,
            ) && let Some(Message::BatteryLr(b)) =
                ask(session, &messages::battery_lr())?.map(|i| i.message)
            {
                snapshot.battery = Some(b);
            }
            if has(
                session.functions(),
                function_id::CRADLE_BATTERY_LEVEL_WITH_THRESHOLD,
            ) && let Some(Message::Cradle(c)) =
                ask(session, &messages::battery_cradle())?.map(|i| i.message)
            {
                snapshot.cradle = Some(c);
            }
        }
        Item::NcAsm => {
            if let Some(nc) = snapshot.ncasm.as_mut()
                && let Some(Message::NcAsmParam(p)) =
                    ask(session, &messages::ncasm_get_param(NCASM_TYPE_DUAL_AMBIENT))?
                        .map(|i| i.message)
            {
                nc.param = p;
            }
        }
        Item::Eq => {
            if let Some(eq) = snapshot.eq.as_mut()
                && let Some(Message::EqParam(p)) =
                    ask(session, &messages::eq_get_param())?.map(|i| i.message)
            {
                eq.param = p;
            }
        }
    }
    Ok(())
}
