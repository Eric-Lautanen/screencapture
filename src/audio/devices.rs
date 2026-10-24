//! Audio endpoints: the Windows devices a take can record, by name for the
//! settings and by id for the take itself.
//!
//! Every enumeration lists every endpoint the machine remembers, not just
//! the connected ones. A paired Bluetooth headset that is switched off still
//! has an id, so a microphone chosen while it was away is the same choice
//! when it comes back — and a take opens it as soon as it is there.

use std::ffi::c_void;
use std::io;

use super::abi::{
    CollectionVt, DEVICE_ENUMERATOR, DEVICE_NAME, DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED,
    DEVICE_STATE_NOTPRESENT, DEVICE_STATE_UNPLUGGED, DEVICE_STATEMASK_ALL, DeviceVt, ENUMERATOR,
    EnumeratorVt, PropVariant, PropertyStoreVt, STGM_READ,
};
use super::wasapi::activate;
use crate::dxgi::com::Com;

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(block: *mut c_void);
    fn PropVariantClear(value: *mut PropVariant) -> i32;
}

/// `eRender`: playback endpoints, the ones the system mix comes from.
pub(crate) const PLAYBACK: i32 = super::abi::RENDER;
/// `eCapture`: recording endpoints, the microphones.
pub(crate) const RECORDING: i32 = super::abi::CAPTURE;

/// One audio endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Device {
    /// The endpoint id, which is what settings remember: names change, ids
    /// survive a rename and a re-pairing.
    pub id: String,
    /// The name Sound settings shows.
    pub name: String,
    /// The `DEVICE_STATE_*` the enumeration reported.
    pub state: u32,
}

impl Device {
    /// Whether the endpoint can be opened right now.
    pub(crate) const fn ready(&self) -> bool {
        self.state & DEVICE_STATE_ACTIVE != 0
    }

    /// The list label: the name, and why it cannot be used when it cannot.
    pub(crate) fn label(&self) -> String {
        let suffix = match self.state {
            _ if self.ready() => "",
            state if state & DEVICE_STATE_DISABLED != 0 => " (disabled)",
            state if state & DEVICE_STATE_NOTPRESENT != 0 => " (not connected)",
            _ if self.state & DEVICE_STATE_UNPLUGGED != 0 => " (unplugged)",
            _ => " (unavailable)",
        };
        format!("{}{suffix}", self.name)
    }
}

/// Every endpoint `flow` remembers, connected or not, in the order Windows
/// lists them.
pub(crate) fn list(flow: i32) -> io::Result<Vec<Device>> {
    let enumerator = enumerator()?;
    let Some(table) = enumerator.view() else {
        return Err(io::Error::other("WASAPI: the device enumerator went away"));
    };
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: live enumerator, a mask of known bits, and a local out slot.
    let hr = unsafe {
        (table.enum_endpoints)(enumerator.raw(), flow, DEVICE_STATEMASK_ALL, &raw mut raw)
    };
    if hr < 0 || raw.is_null() {
        return Err(native("endpoint list", hr));
    }
    // SAFETY: a non-null collection carries the reference handed to us.
    let collection = unsafe { Com::<CollectionVt>::own(raw) };
    let Some(table) = collection.view() else {
        return Err(io::Error::other("WASAPI: the endpoint list went away"));
    };
    let mut count = 0_u32;
    // SAFETY: live collection and a local out slot.
    let hr = unsafe { (table.get_count)(collection.raw(), &raw mut count) };
    if hr < 0 {
        return Err(native("endpoint count", hr));
    }
    let mut devices = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    let mut index = 0_u32;
    while index < count {
        let mut raw: *mut c_void = std::ptr::null_mut();
        // SAFETY: live collection, an index below its own count, and a
        // local out slot; a device answers with one reference.
        let hr = unsafe { (table.item)(collection.raw(), index, &raw mut raw) };
        index = index.saturating_add(1);
        if hr < 0 || raw.is_null() {
            continue;
        }
        // SAFETY: the call answered a non-null device carrying its
        // reference.
        let device = unsafe { Com::<DeviceVt>::own(raw) };
        if let Some(entry) = describe(&device) {
            devices.push(entry);
        }
    }
    Ok(devices)
}

/// The endpoint `id` names, whether or not it is connected: opening the
/// client is what fails while the device is away.
pub(crate) fn find(id: &str) -> io::Result<Com<DeviceVt>> {
    let enumerator = enumerator()?;
    let Some(table) = enumerator.view() else {
        return Err(io::Error::other("WASAPI: the device enumerator went away"));
    };
    let wanted = crate::winapi::wide(id);
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: live enumerator, a NUL-terminated id that outlives the call,
    // and a local out slot.
    let hr = unsafe { (table.get_device)(enumerator.raw(), wanted.as_ptr(), &raw mut raw) };
    if hr < 0 || raw.is_null() {
        return Err(native("chosen device", hr));
    }
    // SAFETY: a non-null device carries the reference handed to us.
    Ok(unsafe { Com::<DeviceVt>::own(raw) })
}

/// Whether the endpoint `id` names can be opened now. Cheap enough for the
/// take to ask again while it waits for a device to come back.
pub(crate) fn present(id: &str) -> bool {
    find(id)
        .is_ok_and(|device| state(&device).is_some_and(|state| state & DEVICE_STATE_ACTIVE != 0))
}

/// The default endpoint for `flow`, the one Windows would use.
pub(crate) fn default_device(flow: i32) -> io::Result<Com<DeviceVt>> {
    enumerator()?.default_endpoint(flow)
}

/// The process-wide device enumerator.
fn enumerator() -> io::Result<Com<EnumeratorVt>> {
    super::wasapi::apartment();
    activate::<EnumeratorVt>(&DEVICE_ENUMERATOR, &ENUMERATOR)
}

/// What one device is called and whether it is there.
fn describe(device: &Com<DeviceVt>) -> Option<Device> {
    let id = device_id(device)?;
    let state = state(device)?;
    Some(Device {
        name: device_name(device).unwrap_or_else(|| "Unknown device".to_owned()),
        id,
        state,
    })
}

/// The endpoint id, copied out of the `CoTaskMemAlloc`ed string the system
/// hands over.
fn device_id(device: &Com<DeviceVt>) -> Option<String> {
    let table = device.view()?;
    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: live device and a local out slot.
    let hr = unsafe { (table.get_id)(device.raw(), &raw mut raw) };
    if hr < 0 || raw.is_null() {
        return None;
    }
    // SAFETY: the call answered a live, NUL-terminated string this handle
    // owns; it is copied before the buffer is freed.
    let id = unsafe { take_wide(raw) };
    // SAFETY: the buffer came from the task allocator, which is what this
    // returns it to.
    unsafe {
        CoTaskMemFree(raw.cast());
    }
    Some(id)
}

/// The name Sound settings shows for this device.
fn device_name(device: &Com<DeviceVt>) -> Option<String> {
    let table = device.view()?;
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: live device, a read-only access flag, and a local out slot.
    let hr = unsafe { (table.open_property_store)(device.raw(), STGM_READ, &raw mut raw) };
    if hr < 0 || raw.is_null() {
        return None;
    }
    // SAFETY: a non-null store carries the reference handed to us.
    let store = unsafe { Com::<PropertyStoreVt>::own(raw) };
    let table = store.view()?;
    let mut value = PropVariant::empty();
    let key = DEVICE_NAME;
    // SAFETY: live store, the key is a live local the call only reads, and
    // the value is a local it fills.
    let hr = unsafe { (table.get_value)(store.raw(), &raw const key, &raw mut value) };
    let name = if hr < 0 {
        None
    } else {
        // SAFETY: a tagged `VT_LPWSTR` the variant owns until it is
        // cleared below; the copy happens first.
        value.as_string().map(|text| unsafe { take_wide(text) })
    };
    // SAFETY: a live variant this scope owns; clearing it frees the string
    // whether or not the call filled one in.
    unsafe {
        PropVariantClear(&raw mut value);
    }
    name.filter(|name| !name.is_empty())
}

/// The `DEVICE_STATE_*` bits of one endpoint.
fn state(device: &Com<DeviceVt>) -> Option<u32> {
    let table = device.view()?;
    let mut state = 0_u32;
    // SAFETY: live device and a local out slot.
    let hr = unsafe { (table.get_state)(device.raw(), &raw mut state) };
    (hr >= 0).then_some(state)
}

/// Copy a NUL-terminated wide string out of a system buffer.
///
/// # Safety
/// `pointer` must name a NUL-terminated UTF-16 string that stays alive for
/// the call.
unsafe fn take_wide(pointer: *const u16) -> String {
    let mut length = 0_usize;
    // SAFETY: the caller promises a terminated string; the walk stops at
    // its first NUL.
    while unsafe { *pointer.add(length) } != 0 {
        length = length.saturating_add(1);
        if length > 4096 {
            break;
        }
    }
    // SAFETY: `length` counts the units before the terminator, all inside
    // the same string.
    let units = unsafe { std::slice::from_raw_parts(pointer, length) };
    crate::winapi::wide_to_string(units)
}

/// One failed `WASAPI` step, named, with the raw code so the log can tell a
/// missing device from a refused one.
fn native(stage: &str, hr: i32) -> io::Error {
    io::Error::other(format!("WASAPI: {stage}: 0x{hr:08X}"))
}

#[cfg(test)]
mod tests {
    use super::{DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED, DEVICE_STATE_NOTPRESENT, Device};

    fn device(state: u32) -> Device {
        Device {
            id: "{0.0.0.00000000}.{abc}".to_owned(),
            name: "Headset Earphone".to_owned(),
            state,
        }
    }

    #[test]
    fn a_connected_endpoint_is_just_its_name() {
        assert_eq!(device(DEVICE_STATE_ACTIVE).label(), "Headset Earphone");
        assert!(device(DEVICE_STATE_ACTIVE).ready());
    }

    #[test]
    fn a_missing_endpoint_says_why() {
        assert_eq!(
            device(DEVICE_STATE_NOTPRESENT).label(),
            "Headset Earphone (not connected)"
        );
        assert_eq!(
            device(DEVICE_STATE_DISABLED).label(),
            "Headset Earphone (disabled)"
        );
        assert!(!device(DEVICE_STATE_NOTPRESENT).ready());
    }
}
