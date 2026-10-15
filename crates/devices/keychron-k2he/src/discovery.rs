//! Finding the keyboard's raw HID interface. Enumeration only; nothing is opened.
//!
//! The serial number and the device path are never read into anything that is printed,
//! logged or serialised: [`InterfaceInfo`] has neither, and [`Entry`] keeps the path
//! private to the crate with no `Debug`.

use std::ffi::CString;

use hidapi::HidApi;

use crate::error::SessionError;
use crate::link::LinkError;
use crate::{PID, RAW_HID_INTERFACE, USAGE, USAGE_PAGE, VID};

/// One enumerated HID entry of this keyboard (VID and PID already matched).
pub(crate) struct Entry {
    pub interface: i32,
    pub usage_page: u16,
    pub usage: u16,
    pub product: Option<String>,
    pub path: CString,
}

/// Why no entry could be chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectError {
    /// No entry with this VID and PID.
    NoDevice,
    /// The backend reports usage page 0 everywhere (Linux hidraw) and the raw HID
    /// interface number is not known.
    UsagePageUnavailable,
}

/// Indexes of the entries to use, in enumeration order. Rule: usage page 0xFF60 and
/// usage 0x61. When every entry reports usage page 0 (hidraw may do that), fall back to
/// the interface number `fallback_interface`; without one, refuse.
pub(crate) fn select(
    entries: &[Entry],
    fallback_interface: Option<i32>,
) -> Result<Vec<usize>, SelectError> {
    if entries.is_empty() {
        return Err(SelectError::NoDevice);
    }
    let by_usage: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.usage_page == USAGE_PAGE && e.usage == USAGE)
        .map(|(i, _)| i)
        .collect();
    if !by_usage.is_empty() {
        return Ok(by_usage);
    }
    if entries.iter().all(|e| e.usage_page == 0) {
        let interface = fallback_interface.ok_or(SelectError::UsagePageUnavailable)?;
        let by_interface: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.interface == interface)
            .map(|(i, _)| i)
            .collect();
        if !by_interface.is_empty() {
            return Ok(by_interface);
        }
    }
    Err(SelectError::NoDevice)
}

pub(crate) fn enumerate(api: &HidApi) -> Vec<Entry> {
    api.device_list()
        .filter(|d| d.vendor_id() == VID && d.product_id() == PID)
        .map(|d| Entry {
            interface: d.interface_number(),
            usage_page: d.usage_page(),
            usage: d.usage(),
            product: d.product_string().map(str::to_owned),
            path: d.path().to_owned(),
        })
        .collect()
}

pub(crate) fn select_error(e: SelectError) -> SessionError {
    match e {
        SelectError::NoDevice => SessionError::NotFound,
        SelectError::UsagePageUnavailable => SessionError::UsagePageUnavailable,
    }
}

/// One HID interface of the keyboard, as `kc_probe list` shows it. No serial, no path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceInfo {
    pub interface: i32,
    pub usage_page: u16,
    pub usage: u16,
    pub product: Option<String>,
    /// True when `Session::open` would use this interface.
    pub matched: bool,
}

/// Every HID entry with the keyboard's VID and PID, with a flag for the ones that match
/// the raw HID rule. Opens nothing.
pub fn list_interfaces() -> Result<Vec<InterfaceInfo>, SessionError> {
    let api =
        HidApi::new().map_err(|_| LinkError::Hid("could not enumerate HID devices".into()))?;
    let entries = enumerate(&api);
    let chosen = select(&entries, RAW_HID_INTERFACE).unwrap_or_default();
    Ok(entries
        .iter()
        .enumerate()
        .map(|(i, e)| InterfaceInfo {
            interface: e.interface,
            usage_page: e.usage_page,
            usage: e.usage,
            product: e.product.clone(),
            matched: chosen.contains(&i),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(interface: i32, usage_page: u16, usage: u16) -> Entry {
        Entry {
            interface,
            usage_page,
            usage,
            product: None,
            path: CString::default(),
        }
    }

    #[test]
    fn selects_the_vendor_collection_and_counts_all_matches() {
        let entries = [
            entry(0, 0x0001, 0x0006),
            entry(1, USAGE_PAGE, USAGE),
            entry(2, USAGE_PAGE, USAGE),
            entry(3, USAGE_PAGE, 0x0062),
        ];
        assert_eq!(select(&entries, None), Ok(vec![1, 2]));
    }

    #[test]
    fn usage_page_zero_needs_a_known_interface() {
        let entries = [entry(0, 0, 0), entry(2, 0, 0)];
        assert_eq!(
            select(&entries, None),
            Err(SelectError::UsagePageUnavailable)
        );
        assert_eq!(select(&entries, Some(2)), Ok(vec![1]));
        assert_eq!(select(&[], None), Err(SelectError::NoDevice));
        // Other usage pages present and none matching: not this keyboard's raw HID.
        assert_eq!(
            select(&[entry(0, 0x0001, 0x0006)], Some(0)),
            Err(SelectError::NoDevice)
        );
    }
}
