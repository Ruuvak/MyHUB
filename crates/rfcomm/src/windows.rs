//! Windows backend: BluetoothAPIs for paired devices, Winsock `AF_BTH` for the socket.
//!
//! Every `unsafe` block of this crate is in this file. Each one passes pointers to
//! locals or to caller owned buffers that outlive the call, with sizes taken from the
//! buffers themselves.

use std::mem::size_of;
use std::ptr::null_mut;
use std::sync::OnceLock;

use windows_sys::Win32::Devices::Bluetooth::{
    AF_BTH, BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS, BTHPROTO_RFCOMM,
    BluetoothEnumerateInstalledServices, BluetoothFindDeviceClose, BluetoothFindFirstDevice,
    BluetoothFindNextDevice, SOCKADDR_BTH,
};
use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, GetLastError};
use windows_sys::Win32::Networking::WinSock::{
    INVALID_SOCKET, POLLRDNORM, SOCK_STREAM, SOCKADDR, SOCKET, SOCKET_ERROR, WSADATA,
    WSAECONNABORTED, WSAECONNRESET, WSAENETDOWN, WSAENETRESET, WSAENOTCONN, WSAESHUTDOWN,
    WSAETIMEDOUT, WSAGetLastError, WSAPOLLFD, WSAPoll, WSAStartup, closesocket,
    connect as ws_connect, recv, send, socket,
};
use windows_sys::core::GUID;

use crate::{BtAddress, ByteLink, PairedDevice, RfcommError, ServiceUuid};

fn guid(u: &ServiceUuid) -> GUID {
    GUID {
        data1: u.data1,
        data2: u.data2,
        data3: u.data3,
        data4: u.data4,
    }
}

fn os(operation: &'static str, code: i32) -> RfcommError {
    RfcommError::Os { operation, code }
}

/// Paired (remembered or authenticated) devices known to Windows. No inquiry scan.
pub fn list_paired() -> Result<Vec<PairedDevice>, RfcommError> {
    let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
        dwSize: size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
        fReturnAuthenticated: 1,
        fReturnRemembered: 1,
        fReturnUnknown: 0,
        fReturnConnected: 1,
        fIssueInquiry: 0,
        cTimeoutMultiplier: 0,
        hRadio: null_mut(),
    };
    let mut info = BLUETOOTH_DEVICE_INFO {
        dwSize: size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `params` and `info` are valid, correctly sized structs that live across
    // the call; a null return is handled below.
    let find = unsafe { BluetoothFindFirstDevice(&params, &mut info) };
    if find.is_null() {
        // SAFETY: plain thread-local error read.
        let code = unsafe { GetLastError() };
        return if code == ERROR_NO_MORE_ITEMS {
            Ok(Vec::new())
        } else {
            Err(os("BluetoothFindFirstDevice", code as i32))
        };
    }
    let mut out = Vec::new();
    loop {
        let len = info
            .szName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szName.len());
        // SAFETY: the union is plain integer data, every bit pattern is valid.
        let raw = unsafe { info.Address.Anonymous.ullLong };
        out.push(PairedDevice::new(
            String::from_utf16_lossy(&info.szName[..len]),
            BtAddress::new(raw),
            info.fConnected != 0,
        ));
        // SAFETY: `find` is the live handle from BluetoothFindFirstDevice, `info` is
        // valid and keeps its dwSize.
        if unsafe { BluetoothFindNextDevice(find, &mut info) } == 0 {
            break;
        }
    }
    // SAFETY: `find` is closed exactly once.
    unsafe { BluetoothFindDeviceClose(find) };
    Ok(out)
}

/// True when Windows lists `uuid` among the installed (enabled) services of the device.
/// A false answer does not prove the headset lacks the service.
pub fn offers_service(device: &PairedDevice, uuid: &ServiceUuid) -> Result<bool, RfcommError> {
    let mut info = BLUETOOTH_DEVICE_INFO {
        dwSize: size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
        ..Default::default()
    };
    info.Address.Anonymous.ullLong = device.address().raw();
    let mut services = [GUID::from_u128(0); 128];
    let mut count = services.len() as u32;
    // SAFETY: `info` is valid, `services` holds `count` GUIDs, both outlive the call.
    let code = unsafe {
        BluetoothEnumerateInstalledServices(null_mut(), &info, &mut count, services.as_mut_ptr())
    };
    if code != 0 && code != ERROR_MORE_DATA {
        return Err(os("BluetoothEnumerateInstalledServices", code as i32));
    }
    let wanted = guid(uuid);
    let n = (count as usize).min(services.len());
    Ok(services[..n].iter().any(|g| {
        g.data1 == wanted.data1
            && g.data2 == wanted.data2
            && g.data3 == wanted.data3
            && g.data4 == wanted.data4
    }))
}

fn start_winsock() -> Result<(), RfcommError> {
    static STARTED: OnceLock<i32> = OnceLock::new();
    let code = *STARTED.get_or_init(|| {
        let mut data = WSADATA::default();
        // SAFETY: `data` is a valid WSADATA for the call. Version 2.2.
        unsafe { WSAStartup(0x0202, &mut data) }
    });
    if code == 0 {
        Ok(())
    } else {
        Err(os("WSAStartup", code))
    }
}

/// An open RFCOMM socket. Closed on drop.
#[derive(Debug)]
pub struct RfcommLink {
    socket: SOCKET,
}

/// Connect to `uuid` on a paired device. Windows resolves the RFCOMM channel through SDP
/// (port 0 with a service class id).
pub fn connect(device: &PairedDevice, uuid: &ServiceUuid) -> Result<RfcommLink, RfcommError> {
    start_winsock()?;
    // SAFETY: plain call, the result is checked.
    let s = unsafe { socket(i32::from(AF_BTH), SOCK_STREAM, BTHPROTO_RFCOMM as i32) };
    if s == INVALID_SOCKET {
        // SAFETY: plain thread-local error read.
        return Err(os("socket", unsafe { WSAGetLastError() }));
    }
    // From here the link owns the socket, so an early return closes it.
    let link = RfcommLink { socket: s };
    let addr = SOCKADDR_BTH {
        addressFamily: AF_BTH,
        btAddr: device.address().raw(),
        serviceClassId: guid(uuid),
        port: 0,
    };
    // SAFETY: `addr` is a valid SOCKADDR_BTH and the length is its size; the socket is open.
    let rc = unsafe {
        ws_connect(
            link.socket,
            (&raw const addr).cast::<SOCKADDR>(),
            size_of::<SOCKADDR_BTH>() as i32,
        )
    };
    if rc == SOCKET_ERROR {
        // SAFETY: plain thread-local error read.
        return Err(os("connect", unsafe { WSAGetLastError() }));
    }
    Ok(link)
}

fn is_disconnect(code: i32) -> bool {
    matches!(
        code,
        WSAECONNABORTED | WSAECONNRESET | WSAENOTCONN | WSAESHUTDOWN | WSAENETRESET | WSAENETDOWN
    )
}

impl ByteLink for RfcommLink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), RfcommError> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let len = rest.len().min(i32::MAX as usize) as i32;
            // SAFETY: `rest` is a live slice of at least `len` bytes; the socket is open.
            let n = unsafe { send(self.socket, rest.as_ptr(), len, 0) };
            if n == SOCKET_ERROR {
                // SAFETY: plain thread-local error read.
                let code = unsafe { WSAGetLastError() };
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

    /// Waits with `WSAPoll` until the socket is readable, then reads once. `Ok(0)` is the
    /// poll timeout. A reset, a hang-up, an error flag, a ready read of 0 bytes (orderly
    /// close) and a `WSAETIMEDOUT` from `recv` (the link timed out) are all `Disconnected`.
    fn read(&mut self, buf: &mut [u8], timeout_ms: u32) -> Result<usize, RfcommError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut fd = WSAPOLLFD {
            fd: self.socket,
            events: POLLRDNORM,
            revents: 0,
        };
        let wait = i32::try_from(timeout_ms).unwrap_or(i32::MAX);
        // SAFETY: `fd` is one valid WSAPOLLFD that lives across the call; count 1.
        let ready = unsafe { WSAPoll(&mut fd, 1, wait) };
        if ready == SOCKET_ERROR {
            // SAFETY: plain thread-local error read.
            let code = unsafe { WSAGetLastError() };
            return Err(if is_disconnect(code) {
                RfcommError::Disconnected
            } else {
                os("WSAPoll", code)
            });
        }
        if ready == 0 {
            return Ok(0);
        }
        if fd.revents & POLLRDNORM == 0 {
            // Only hang-up, error or invalid-socket flags are set.
            return Err(RfcommError::Disconnected);
        }
        let len = buf.len().min(i32::MAX as usize) as i32;
        // SAFETY: `buf` is a live, writable slice of at least `len` bytes; socket is open.
        let n = unsafe { recv(self.socket, buf.as_mut_ptr(), len, 0) };
        match n {
            0 => Err(RfcommError::Disconnected),
            SOCKET_ERROR => {
                // SAFETY: plain thread-local error read.
                let code = unsafe { WSAGetLastError() };
                if code == WSAETIMEDOUT || is_disconnect(code) {
                    Err(RfcommError::Disconnected)
                } else {
                    Err(os("recv", code))
                }
            }
            n => Ok(n as usize),
        }
    }
}

impl Drop for RfcommLink {
    fn drop(&mut self) {
        // SAFETY: the socket was opened by `connect` and is closed exactly once, here.
        unsafe { closesocket(self.socket) };
    }
}
