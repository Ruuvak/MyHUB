//! Finding the receiver's HID++ interface.
//!
//! [`select_receivers`] is a pure function over [`HidCandidate`] values, so the
//! matching rules are unit-testable without hidapi. [`find_receivers`] only
//! feeds it the enumeration. Enumeration never opens a device.

use std::ffi::{CStr, CString};
use std::fmt;

use hidapi::{DeviceInfo, HidApi};

/// Logitech USB vendor ID.
pub const LOGITECH_VID: u16 = 0x046D;
/// Supported receiver product IDs. Adding a receiver is a one-line change.
pub const RECEIVER_PIDS: &[u16] = &[0xC547];
/// USB interface that carries HID++ on the receiver (observed).
pub const HIDPP_INTERFACE: i32 = 2;
/// Vendor usage page of the HID++ collections.
pub const HIDPP_USAGE_PAGE: u16 = 0xFF00;
/// Usage of the long-report (20 byte) collection on Windows.
pub const LONG_REPORT_USAGE: u16 = 0x0002;
/// Usage of the short-report (7 byte) collection on Windows. HID++ 1.0
/// messages (0x41 connection, 0x8F error) arrive as short reports there.
pub const SHORT_REPORT_USAGE: u16 = 0x0001;

/// One enumerated HID entry, reduced to the fields the matcher needs.
#[derive(Clone, PartialEq, Eq)]
pub struct HidCandidate {
    pub vid: u16,
    pub pid: u16,
    pub interface: i32,
    pub usage_page: u16,
    pub usage: u16,
    pub path: CString,
}

impl HidCandidate {
    pub fn from_device_info(info: &DeviceInfo) -> Self {
        Self {
            vid: info.vendor_id(),
            pid: info.product_id(),
            interface: info.interface_number(),
            usage_page: info.usage_page(),
            usage: info.usage(),
            path: info.path().to_owned(),
        }
    }
}

// The path can carry instance identifiers, so it is left out of Debug.
impl fmt::Debug for HidCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HidCandidate")
            .field("vid", &format_args!("{:#06x}", self.vid))
            .field("pid", &format_args!("{:#06x}", self.pid))
            .field("interface", &self.interface)
            .field("usage_page", &format_args!("{:#06x}", self.usage_page))
            .field("usage", &format_args!("{:#06x}", self.usage))
            .finish_non_exhaustive()
    }
}

/// Which usage values count as the HID++ entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageRule {
    /// Windows: every HID collection is its own path, so only the long-report
    /// collection (usage 0x0002, 20 byte reports) is accepted.
    LongReportCollection,
    /// Linux: hidraw exposes one node per interface that carries both report
    /// IDs 0x10 and 0x11, so any usage is accepted.
    AnyUsage,
}

impl UsageRule {
    /// The rule for the OS this crate is built for.
    pub const fn for_current_os() -> Self {
        if cfg!(target_os = "linux") {
            Self::AnyUsage
        } else {
            Self::LongReportCollection
        }
    }
}

/// A receiver HID++ entry that can be opened: the collection that carries long
/// reports (the only one ever written to), plus on Windows the sibling
/// collection that delivers short reports (read only).
#[derive(Clone, PartialEq, Eq)]
pub struct ReceiverPath {
    pub pid: u16,
    pub usage: u16,
    path: CString,
    short_path: Option<CString>,
}

impl ReceiverPath {
    /// The OS path to open for reading and writing. Not for display or logs.
    pub fn path(&self) -> &CStr {
        &self.path
    }

    /// The short-report collection of the same receiver, to be opened for
    /// reading only. `None` on Linux (one hidraw node carries both report IDs)
    /// and when the sibling collection was not found.
    pub fn short_path(&self) -> Option<&CStr> {
        self.short_path.as_deref()
    }
}

impl fmt::Debug for ReceiverPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiverPath")
            .field("pid", &format_args!("{:#06x}", self.pid))
            .field("usage", &format_args!("{:#06x}", self.usage))
            .field("has_short_collection", &self.short_path.is_some())
            .finish_non_exhaustive()
    }
}

/// Removes the `&colNN` collection token from a Windows path component.
fn strip_collection(component: &str) -> String {
    match component.find("&col") {
        Some(at) => {
            let rest = &component[at + 4..];
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            format!("{}{}", &component[..at], &rest[digits..])
        }
        None => component.to_string(),
    }
}

/// Key that is equal for all collections of one HID interface and differs
/// between receivers. Windows paths look like
/// `\\?\hid#vid_046d&pid_c547&mi_02&col02#7&2a3b4c5d&0&0001#{guid}` (\[INF\]
/// layout, not yet checked on this machine): the `&colNN` token and the last
/// component of the instance ID (the collection number) differ per collection,
/// everything else is shared. A path of another shape is its own key, so it
/// pairs with nothing.
fn interface_key(path: &CStr) -> String {
    let lower = path.to_string_lossy().to_lowercase();
    let parts: Vec<&str> = lower.split('#').collect();
    if parts.len() < 3 || !parts[1].contains("&col") {
        return lower;
    }
    let instance = parts[2]
        .rsplit_once('&')
        .map_or(parts[2], |(parent, _)| parent);
    format!("{}#{}", strip_collection(parts[1]), instance)
}

/// Pure matcher: receiver VID/PID, interface 2, usage page 0xFF00, usage per
/// `rule`. Duplicate paths are dropped. The order of `candidates` is kept.
///
/// With [`UsageRule::LongReportCollection`] each match also carries the
/// usage 0x0001 collection of the same interface as `short_path`, when the
/// enumeration contains one.
pub fn select_receivers(candidates: &[HidCandidate], rule: UsageRule) -> Vec<ReceiverPath> {
    let is_receiver_interface = |c: &HidCandidate| {
        c.vid == LOGITECH_VID
            && RECEIVER_PIDS.contains(&c.pid)
            && c.interface == HIDPP_INTERFACE
            && c.usage_page == HIDPP_USAGE_PAGE
    };
    let mut found: Vec<ReceiverPath> = Vec::new();
    for c in candidates {
        let usage_ok = match rule {
            UsageRule::LongReportCollection => c.usage == LONG_REPORT_USAGE,
            UsageRule::AnyUsage => true,
        };
        if !(is_receiver_interface(c) && usage_ok) || found.iter().any(|r| r.path == c.path) {
            continue;
        }
        let short_path = match rule {
            UsageRule::LongReportCollection => {
                let key = interface_key(&c.path);
                candidates
                    .iter()
                    .find(|s| {
                        is_receiver_interface(s)
                            && s.pid == c.pid
                            && s.usage == SHORT_REPORT_USAGE
                            && interface_key(&s.path) == key
                    })
                    .map(|s| s.path.clone())
            }
            UsageRule::AnyUsage => None,
        };
        found.push(ReceiverPath {
            pid: c.pid,
            usage: c.usage,
            path: c.path.clone(),
            short_path,
        });
    }
    found
}

/// All enumerated HID entries. Enumeration only; nothing is opened.
pub fn list_candidates(api: &HidApi) -> Vec<HidCandidate> {
    api.device_list()
        .map(HidCandidate::from_device_info)
        .collect()
}

/// Receivers on this machine, using the rule for the current OS.
pub fn find_receivers(api: &HidApi) -> Vec<ReceiverPath> {
    select_receivers(&list_candidates(api), UsageRule::for_current_os())
}
