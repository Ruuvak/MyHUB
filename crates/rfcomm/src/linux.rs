//! Linux backend: BlueZ over D-Bus for paired devices, kernel Bluetooth sockets for the link.
//!
//! The paired list and the `UUIDs` property come from `org.bluez` on the system bus
//! (`GetManagedObjects`). The RFCOMM channel is resolved by a small SDP client over an
//! L2CAP socket (the pure part is in `sdp.rs`), then an `AF_BLUETOOTH` RFCOMM stream
//! socket connects to it. Every `unsafe` block of this crate on Linux is in this file.
//! Each one passes pointers to locals or to caller owned buffers that outlive the call,
//! with sizes taken from the buffers themselves.
//!
//! D-Bus failures (no system bus, `org.bluez` not running, malformed reply) have no OS
//! code: they are `RfcommError::Os` with a fixed operation name and code -1.

use std::collections::HashMap;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::bluez::{bdaddr_bytes, lists_uuid, paired_device, parse_address, uuid_bytes, uuid_text};
use crate::sdp::{self, SdpError};
use crate::{ByteLink, PairedDevice, RfcommError, ServiceUuid};

const BTPROTO_L2CAP: libc::c_int = 0;
const BTPROTO_RFCOMM: libc::c_int = 3;
/// The SDP server's fixed L2CAP port.
const SDP_PSM: u16 = 1;
/// Address type BR/EDR (Bluetooth Classic).
const BDADDR_BREDR: u8 = 0;
/// Whole SDP exchange, connect included.
const SDP_TOTAL: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// At most this many continuation round trips and this many attribute bytes.
const SDP_MAX_ROUNDS: usize = 32;
const SDP_MAX_BYTES: usize = 64 * 1024;

/// `struct sockaddr_l2` from `<bluetooth/l2cap.h>`.
#[repr(C)]
struct SockaddrL2 {
    l2_family: libc::sa_family_t,
    l2_psm: u16,
    l2_bdaddr: [u8; 6],
    l2_cid: u16,
    l2_bdaddr_type: u8,
}

/// `struct sockaddr_rc` from `<bluetooth/rfcomm.h>`.
#[repr(C)]
struct SockaddrRc {
    rc_family: libc::sa_family_t,
    rc_bdaddr: [u8; 6],
    rc_channel: u8,
}

fn os(operation: &'static str, code: i32) -> RfcommError {
    RfcommError::Os { operation, code }
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
}

fn bus_error(operation: &'static str) -> impl Fn(zbus::Error) -> RfcommError {
    move |_| os(operation, -1)
}

type Properties = HashMap<String, OwnedValue>;
type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, Properties>>;

/// `org.bluez` objects with their interfaces and properties.
fn managed_objects() -> Result<ManagedObjects, RfcommError> {
    let conn = Connection::system().map_err(bus_error("D-Bus system bus"))?;
    let reply = conn
        .call_method(
            Some("org.bluez"),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .map_err(bus_error("BlueZ GetManagedObjects"))?;
    reply
        .body()
        .deserialize::<ManagedObjects>()
        .map_err(|_| os("BlueZ GetManagedObjects reply", -1))
}

fn string_prop(props: &Properties, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|v| <&str>::try_from(&**v).ok())
        .map(str::to_owned)
}

fn bool_prop(props: &Properties, key: &str) -> bool {
    props
        .get(key)
        .and_then(|v| bool::try_from(&**v).ok())
        .unwrap_or(false)
}

fn string_list_prop(props: &Properties, key: &str) -> Vec<String> {
    props
        .get(key)
        .and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok())
        .unwrap_or_default()
}

/// The `org.bluez.Device1` property maps of all known devices.
fn device_properties(objects: ManagedObjects) -> impl Iterator<Item = Properties> {
    objects
        .into_values()
        .filter_map(|mut interfaces| interfaces.remove("org.bluez.Device1"))
}

/// UUID lists seen by the last [`list_paired`], by address, so [`offers_service`] does not
/// open a bus connection per device.
static UUID_CACHE: Mutex<Option<HashMap<u64, Vec<String>>>> = Mutex::new(None);

/// Paired devices known to BlueZ. No discovery scan. A device that several adapters know
/// appears once (the connected entry wins).
pub fn list_paired() -> Result<Vec<PairedDevice>, RfcommError> {
    let mut devices: Vec<PairedDevice> = Vec::new();
    let mut cache: HashMap<u64, Vec<String>> = HashMap::new();
    for props in device_properties(managed_objects()?) {
        let Some(address) = string_prop(&props, "Address") else {
            continue;
        };
        let Some(device) = paired_device(
            &address,
            string_prop(&props, "Name").as_deref(),
            string_prop(&props, "Alias").as_deref(),
            bool_prop(&props, "Paired"),
            bool_prop(&props, "Connected"),
        ) else {
            continue;
        };
        let raw = device.address().raw();
        let uuids = string_list_prop(&props, "UUIDs");
        cache.entry(raw).or_default().extend(uuids);
        match devices.iter().position(|d| d.address().raw() == raw) {
            Some(i) if device.connected && !devices[i].connected => devices[i] = device,
            Some(_) => {}
            None => devices.push(device),
        }
    }
    *UUID_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(cache);
    Ok(devices)
}

/// True when BlueZ lists `uuid` in the `UUIDs` property of the device. Answers from the
/// last [`list_paired`] when it saw the device, else queries BlueZ. A false answer does
/// not prove the headset lacks the service.
pub fn offers_service(device: &PairedDevice, uuid: &ServiceUuid) -> Result<bool, RfcommError> {
    let wanted = uuid_text(uuid);
    let raw = device.address().raw();
    {
        let cache = UUID_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(uuids) = cache.as_ref().and_then(|c| c.get(&raw)) {
            return Ok(lists_uuid(uuids, &wanted));
        }
    }
    Ok(device_properties(managed_objects()?).any(|props| {
        string_prop(&props, "Address").and_then(|a| parse_address(&a)) == Some(raw)
            && lists_uuid(&string_list_prop(&props, "UUIDs"), &wanted)
    }))
}

fn timeval(d: Duration) -> libc::timeval {
    libc::timeval {
        tv_sec: d.as_secs() as libc::time_t,
        tv_usec: d.subsec_micros() as libc::suseconds_t,
    }
}

fn set_timeout(fd: RawFd, option: libc::c_int, d: Duration) -> Result<(), RfcommError> {
    let tv = timeval(d);
    // SAFETY: `tv` is a valid timeval that lives across the call and the length is its
    // size; `fd` is an open socket.
    let rc = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            option,
            (&raw const tv).cast::<libc::c_void>(),
            size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(os("setsockopt", last_errno()))
    }
}

fn open_socket(kind: libc::c_int, protocol: libc::c_int) -> Result<OwnedFd, RfcommError> {
    // SAFETY: plain call, the result is checked.
    let fd = unsafe { libc::socket(libc::AF_BLUETOOTH, kind | libc::SOCK_CLOEXEC, protocol) };
    if fd < 0 {
        return Err(os("socket", last_errno()));
    }
    // SAFETY: `fd` is a fresh descriptor that nothing else owns; the OwnedFd closes it once.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Blocking connect of `fd` to `addr` (any sockaddr struct of `len` bytes). An interrupted
/// connect is not repeated (the state of the attempt is unknown), it is an error.
/// `EINPROGRESS` is the kernel's answer when the send timeout ran out: it becomes `ETIMEDOUT`.
fn connect_socket<T>(fd: &OwnedFd, addr: &T, operation: &'static str) -> Result<(), RfcommError> {
    // SAFETY: `addr` is a valid repr(C) sockaddr and the length is its size; the socket is
    // open for the duration of the borrow.
    let rc = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (addr as *const T).cast::<libc::sockaddr>(),
            size_of::<T>() as libc::socklen_t,
        )
    };
    if rc == 0 {
        return Ok(());
    }
    let code = last_errno();
    Err(os(
        operation,
        if code == libc::EINPROGRESS {
            libc::ETIMEDOUT
        } else {
            code
        },
    ))
}

fn send_all(fd: RawFd, bytes: &[u8]) -> Result<(), RfcommError> {
    let mut rest = bytes;
    while !rest.is_empty() {
        // SAFETY: `rest` is a live slice of the given length; `fd` is an open socket.
        let n = unsafe {
            libc::send(
                fd,
                rest.as_ptr().cast::<libc::c_void>(),
                rest.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if n < 0 {
            let code = last_errno();
            if code == libc::EINTR {
                continue;
            }
            return Err(if is_disconnect(code) {
                RfcommError::Disconnected
            } else {
                os("send", code)
            });
        }
        if n == 0 {
            // A blocking send of a non-empty buffer never returns 0: do not spin.
            return Err(os("send", 0));
        }
        rest = &rest[(n as usize).min(rest.len())..];
    }
    Ok(())
}

fn recv_some(fd: RawFd, buf: &mut [u8]) -> isize {
    // SAFETY: `buf` is a live, writable slice of the given length; `fd` is an open socket.
    unsafe { libc::recv(fd, buf.as_mut_ptr().cast::<libc::c_void>(), buf.len(), 0) }
}

fn sdp_failure(e: SdpError) -> RfcommError {
    match e {
        SdpError::Truncated | SdpError::Malformed | SdpError::WrongTransaction => {
            os("SDP reply", libc::EPROTO)
        }
        SdpError::ErrorResponse(_) => os("SDP error response", libc::EPROTO),
    }
}

/// Ask the device's SDP server for the RFCOMM channel of `uuid`.
fn resolve_channel(bdaddr: [u8; 6], uuid: &ServiceUuid) -> Result<u8, RfcommError> {
    let fd = open_socket(libc::SOCK_SEQPACKET, BTPROTO_L2CAP)?;
    let deadline = Instant::now() + SDP_TOTAL;
    set_timeout(fd.as_raw_fd(), libc::SO_SNDTIMEO, CONNECT_TIMEOUT)?;
    let addr = SockaddrL2 {
        l2_family: libc::AF_BLUETOOTH as libc::sa_family_t,
        l2_psm: SDP_PSM.to_le(),
        l2_bdaddr: bdaddr,
        l2_cid: 0,
        l2_bdaddr_type: BDADDR_BREDR,
    };
    connect_socket(&fd, &addr, "SDP connect")?;

    let pattern = uuid_bytes(uuid);
    let mut attributes: Vec<u8> = Vec::new();
    let mut continuation: Vec<u8> = Vec::new();
    let mut reply = vec![0u8; 8192];
    for round in 0..SDP_MAX_ROUNDS {
        let transaction = round as u16 + 1;
        send_all(
            fd.as_raw_fd(),
            &sdp::encode_request(transaction, &pattern, &continuation),
        )?;
        let n = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(os("SDP recv", libc::ETIMEDOUT));
            }
            set_timeout(
                fd.as_raw_fd(),
                libc::SO_RCVTIMEO,
                left.max(Duration::from_millis(1)),
            )?;
            let n = recv_some(fd.as_raw_fd(), &mut reply);
            if n >= 0 {
                break n as usize;
            }
            let code = last_errno();
            match code {
                libc::EINTR => {}
                libc::EAGAIN => return Err(os("SDP recv", libc::ETIMEDOUT)),
                _ => return Err(os("SDP recv", code)),
            }
        };
        let response = sdp::parse_response(transaction, &reply[..n]).map_err(sdp_failure)?;
        attributes.extend_from_slice(response.data);
        if attributes.len() > SDP_MAX_BYTES {
            return Err(os("SDP reply", libc::EMSGSIZE));
        }
        if response.continuation.is_empty() {
            return match sdp::rfcomm_channel(&attributes).map_err(sdp_failure)? {
                Some(channel) => Ok(channel),
                None => Err(os("SDP service not found", libc::ENOENT)),
            };
        }
        continuation = response.continuation.to_vec();
    }
    Err(os("SDP continuation", libc::EPROTO))
}

/// An open RFCOMM socket. Closed on drop.
#[derive(Debug)]
pub struct RfcommLink {
    fd: OwnedFd,
}

/// Connect to `uuid` on a paired device. The RFCOMM channel is resolved through SDP first.
pub fn connect(device: &PairedDevice, uuid: &ServiceUuid) -> Result<RfcommLink, RfcommError> {
    let bdaddr = bdaddr_bytes(device.address().raw());
    let channel = resolve_channel(bdaddr, uuid)?;
    let fd = open_socket(libc::SOCK_STREAM, BTPROTO_RFCOMM)?;
    // Bound the connect, then lift the limit so it does not apply to later sends.
    set_timeout(fd.as_raw_fd(), libc::SO_SNDTIMEO, CONNECT_TIMEOUT)?;
    let addr = SockaddrRc {
        rc_family: libc::AF_BLUETOOTH as libc::sa_family_t,
        rc_bdaddr: bdaddr,
        rc_channel: channel,
    };
    connect_socket(&fd, &addr, "connect")?;
    set_timeout(fd.as_raw_fd(), libc::SO_SNDTIMEO, Duration::ZERO)?;
    Ok(RfcommLink { fd })
}

fn is_disconnect(code: i32) -> bool {
    matches!(
        code,
        libc::ECONNRESET
            | libc::ENOTCONN
            | libc::EHOSTDOWN
            | libc::ETIMEDOUT
            | libc::ECONNABORTED
            | libc::EPIPE
            | libc::ENETDOWN
            | libc::ESHUTDOWN
    )
}

impl ByteLink for RfcommLink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), RfcommError> {
        send_all(self.fd.as_raw_fd(), bytes)
    }

    /// Waits with `poll` until the socket is readable, then reads once. `Ok(0)` is the poll
    /// timeout. A hang-up or error flag without data, a ready read of 0 bytes (orderly
    /// close) and the reset-like errnos are all `Disconnected`.
    fn read(&mut self, buf: &mut [u8], timeout_ms: u32) -> Result<usize, RfcommError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
        let mut fd = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            // Round up so a sub-millisecond rest still waits instead of busy looping.
            let wait = left.as_micros().div_ceil(1000).min(i32::MAX as u128) as libc::c_int;
            // SAFETY: `fd` is one valid pollfd that lives across the call; count 1.
            let ready = unsafe { libc::poll(&mut fd, 1, wait) };
            if ready < 0 {
                let code = last_errno();
                if code == libc::EINTR {
                    continue;
                }
                return Err(if is_disconnect(code) {
                    RfcommError::Disconnected
                } else {
                    os("poll", code)
                });
            }
            if ready == 0 {
                return Ok(0);
            }
            break;
        }
        if fd.revents & libc::POLLIN == 0 {
            // Only hang-up, error or invalid-descriptor flags are set.
            return Err(RfcommError::Disconnected);
        }
        loop {
            let n = recv_some(fd.fd, buf);
            if n > 0 {
                return Ok(n as usize);
            }
            if n == 0 {
                return Err(RfcommError::Disconnected);
            }
            let code = last_errno();
            match code {
                libc::EINTR => {}
                _ if is_disconnect(code) => return Err(RfcommError::Disconnected),
                _ => return Err(os("recv", code)),
            }
        }
    }
}
