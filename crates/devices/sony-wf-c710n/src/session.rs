//! The session: transport, `mdr::Link`, function table, limits and the read-tier policy.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use mdr::messages::{EqCapability, Message, NcAsmCapability};
use mdr::{
    CheckedRequest, DataType, DeviceLimits, FunctionTable, Incoming, Link, LinkEvent, Policy,
    PolicyError, Request, SendError, Tick, parse_received,
};
use myhub_rfcomm::{ByteLink, RfcommError};
use thiserror::Error;

/// Read timeout of one transport read inside the loop.
pub const READ_POLL_MS: u32 = 50;
/// Default wait for the reply to a request.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// Default wait for the NTFY that follows a SET, before the value is read back.
pub const VERIFY_WAIT: Duration = Duration::from_secs(2);

/// Why a session call failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionError {
    /// The Bluetooth link dropped. Open a new transport and call [`Session::reconnect`].
    #[error("Bluetooth link disconnected")]
    Disconnected,
    #[error("transport error: {0}")]
    Transport(RfcommError),
    /// The request did not pass the policy; nothing was sent.
    #[error("request refused by the policy: {0}")]
    Policy(#[from] PolicyError),
    #[error("request refused by the link: {0}")]
    Send(#[from] SendError),
    #[error("no reply within the timeout")]
    Timeout,
    /// The headset stopped acknowledging our frames.
    #[error("the headset stopped acknowledging")]
    LinkDead,
    /// The reply had another shape than the request promises.
    #[error("unexpected reply: {0}")]
    Unexpected(&'static str),
}

impl From<RfcommError> for SessionError {
    fn from(e: RfcommError) -> Self {
        match e {
            RfcommError::Disconnected => Self::Disconnected,
            other => Self::Transport(other),
        }
    }
}

struct Rx {
    data_type: DataType,
    payload: Vec<u8>,
    incoming: Incoming,
}

/// Reply command is the request command + 1 and the inquired type byte is the same.
fn is_reply(request: &Request, rx: &Rx) -> bool {
    let (Some(&cmd), Some(&kind)) = (request.payload.first(), request.payload.get(1)) else {
        return false;
    };
    rx.data_type == request.data_type
        && rx.payload.first() == Some(&cmd.wrapping_add(1))
        && rx.payload.get(1) == Some(&kind)
}

/// One connection to the headset. The session's own policy is the read tier and never
/// changes: a write builds a policy with only the gate it needs for that one call
/// (see the `writes` module) and passes it to [`Session::request_with`].
pub struct Session<L: ByteLink> {
    transport: L,
    link: Link,
    functions: FunctionTable,
    limits: DeviceLimits,
    ncasm_capability: Option<NcAsmCapability>,
    eq_capability: Option<EqCapability>,
    policy: Policy,
    clock: Instant,
    reply_timeout: Duration,
    verify_wait: Duration,
    notifications: VecDeque<Incoming>,
    primed: bool,
    buf: Vec<u8>,
}

impl<L: ByteLink> Session<L> {
    pub fn new(transport: L) -> Self {
        Self {
            transport,
            link: Link::new(),
            functions: FunctionTable::new(),
            limits: DeviceLimits::new(),
            ncasm_capability: None,
            eq_capability: None,
            policy: Policy::new(),
            clock: Instant::now(),
            reply_timeout: REPLY_TIMEOUT,
            verify_wait: VERIFY_WAIT,
            notifications: VecDeque::new(),
            primed: false,
            buf: vec![0; 1024],
        }
    }

    /// Continue on a new transport after a drop: `Link::reset`, and the function table,
    /// limits, capabilities and queued notifications start over.
    pub fn reconnect(&mut self, transport: L) {
        self.transport = transport;
        self.link.reset();
        self.functions = FunctionTable::new();
        self.limits = DeviceLimits::new();
        self.ncasm_capability = None;
        self.eq_capability = None;
        self.notifications.clear();
        self.primed = false;
    }

    pub fn set_reply_timeout(&mut self, timeout: Duration) {
        self.reply_timeout = timeout;
    }

    /// How long a write waits for the matching notification before it reads the value
    /// back (default [`VERIFY_WAIT`]).
    pub fn set_verify_wait(&mut self, wait: Duration) {
        self.verify_wait = wait;
    }

    pub(crate) fn verify_wait(&self) -> Duration {
        self.verify_wait
    }

    pub(crate) fn ncasm_capability(&self) -> Option<&NcAsmCapability> {
        self.ncasm_capability.as_ref()
    }

    pub(crate) fn eq_capability(&self) -> Option<&EqCapability> {
        self.eq_capability.as_ref()
    }

    pub(crate) fn notifications_mut(&mut self) -> &mut VecDeque<Incoming> {
        &mut self.notifications
    }

    pub fn transport(&self) -> &L {
        &self.transport
    }

    pub fn functions(&self) -> &FunctionTable {
        &self.functions
    }

    pub fn limits(&self) -> &DeviceLimits {
        &self.limits
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Resend or give up when the ACK timer ran out.
    fn drive_timer(&mut self) -> Result<(), SessionError> {
        match self.link.on_tick(self.now_ms()) {
            Some(Tick::Resend(bytes)) => self.transport.write_all(&bytes)?,
            Some(Tick::LinkDead) => return Err(SessionError::LinkDead),
            None => {}
        }
        Ok(())
    }

    /// One loop turn: timer, one read of at most `wait_ms`, ACK every frame that needs
    /// one, absorb tables and limits, parse. Returns what was delivered.
    fn pump(&mut self, wait_ms: u32) -> Result<Vec<Rx>, SessionError> {
        self.drive_timer()?;
        let wait = match self.link.next_deadline() {
            Some(deadline) => {
                let left = deadline.saturating_sub(self.now_ms());
                u32::try_from(left).unwrap_or(u32::MAX).min(wait_ms)
            }
            None => wait_ms,
        };
        let n = self.transport.read(&mut self.buf, wait)?;
        let mut delivered = Vec::new();
        if n == 0 {
            return Ok(delivered);
        }
        let now = self.now_ms();
        for event in self.link.on_bytes(&self.buf[..n], now) {
            match event {
                LinkEvent::Outgoing(ack) => self.transport.write_all(&ack)?,
                LinkEvent::Delivered(frame) => {
                    self.functions.absorb(&frame);
                    self.limits.absorb(&frame);
                    let data_type = frame.data_type();
                    let payload = frame.payload().to_vec();
                    let incoming = parse_received(frame);
                    match &incoming.message {
                        Message::NcAsmCapability(c) => self.ncasm_capability = Some(c.clone()),
                        Message::EqCapability(c) => self.eq_capability = Some(c.clone()),
                        _ => {}
                    }
                    delivered.push(Rx {
                        data_type,
                        payload,
                        incoming,
                    });
                }
                LinkEvent::Acked => {}
            }
        }
        Ok(delivered)
    }

    /// Read before the first send: the headset pushes frames right after connect.
    fn prime(&mut self) -> Result<(), SessionError> {
        if !self.primed {
            self.primed = true;
            self.poll(READ_POLL_MS)?;
        }
        Ok(())
    }

    /// Read and ACK for at most `wait_ms` (one transport read). Delivered messages
    /// are queued as notifications.
    pub fn poll(&mut self, wait_ms: u32) -> Result<(), SessionError> {
        for rx in self.pump(wait_ms)? {
            self.notifications.push_back(rx.incoming);
        }
        Ok(())
    }

    /// Messages that were not the reply to a request, oldest first.
    pub fn take_notifications(&mut self) -> Vec<Incoming> {
        self.notifications.drain(..).collect()
    }

    /// Check the request against the read tier, send it and wait for the ACK and the
    /// reply. Other incoming messages are queued as notifications.
    pub fn request(&mut self, request: &Request) -> Result<Incoming, SessionError> {
        self.request_with(self.policy, request)
    }

    /// Wait until the previous frame is acknowledged (at most the reply timeout).
    pub(crate) fn wait_idle(&mut self) -> Result<(), SessionError> {
        self.prime()?;
        let busy_deadline = Instant::now() + self.reply_timeout;
        while self.link.is_busy() && Instant::now() < busy_deadline {
            self.poll(READ_POLL_MS)?;
        }
        Ok(())
    }

    /// Send a checked frame that has no reply (a SET or an alert answer) and wait for its
    /// ACK. Anything the headset sends meanwhile is queued as a notification.
    pub(crate) fn send_checked(&mut self, checked: CheckedRequest) -> Result<(), SessionError> {
        self.wait_idle()?;
        let bytes = self.link.send(checked, self.now_ms())?;
        self.transport.write_all(&bytes)?;
        let deadline = Instant::now() + self.reply_timeout;
        while self.link.is_busy() {
            if Instant::now() >= deadline {
                return Err(SessionError::Timeout);
            }
            self.poll(READ_POLL_MS)?;
        }
        Ok(())
    }

    /// Check a request that needs no reply against `policy` and send it.
    pub(crate) fn send_with(
        &mut self,
        policy: Policy,
        request: &Request,
    ) -> Result<(), SessionError> {
        let checked = policy.check(self.link.epoch(), &self.functions, &self.limits, request)?;
        self.send_checked(checked)
    }

    /// Like [`Session::request`] with an explicit policy. Writes pass a policy that holds
    /// only the gate they need; the session never keeps an opened gate.
    pub(crate) fn request_with(
        &mut self,
        policy: Policy,
        request: &Request,
    ) -> Result<Incoming, SessionError> {
        self.wait_idle()?;
        let checked = policy.check(self.link.epoch(), &self.functions, &self.limits, request)?;
        let bytes = self.link.send(checked, self.now_ms())?;
        self.transport.write_all(&bytes)?;
        let deadline = Instant::now() + self.reply_timeout;
        let mut reply = None;
        loop {
            if !self.link.is_busy()
                && let Some(incoming) = reply.take()
            {
                return Ok(incoming);
            }
            if Instant::now() >= deadline {
                return Err(SessionError::Timeout);
            }
            for rx in self.pump(READ_POLL_MS)? {
                if reply.is_none() && is_reply(request, &rx) {
                    reply = Some(rx.incoming);
                } else {
                    self.notifications.push_back(rx.incoming);
                }
            }
        }
    }
}
