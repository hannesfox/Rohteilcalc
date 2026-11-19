//! Windows: 3D-Mäuse über Plug and Play finden, ohne andere Geräte zu
//! berühren (SolidCraft #2614).
//!
//! hidapis Enumeration öffnet jedes HID-Interface und liest seine
//! Seriennummer-, Hersteller- und Produkt-Strings: drei Control-Requests an
//! jede Tastatur, Maus und jeden Touchscreen. Der Leser hat das alle zwei
//! Sekunden wiederholt, solange keine 3D-Maus gesteckt war — auf manchen
//! Maschinen fiel die Tastatur dabei kurz vom Bus und der Cursor sprang.
//! Plug and Play benennt aber bereits jedes Interface-Top-Level-Usage in
//! seinen Hardware-IDs (`HID_DEVICE_UP:0001_U:0008`: Generic Desktop,
//! Multi-Axis Controller); der Leser fragt nur das, was kein Gerät berührt,
//! und öffnet dann ausschließlich die 3D-Maus-Interfaces.
//!
//! Portiert aus SolidCraft
//! `apps/solidcraft/src/space_mouse/windows_hid.rs`
//! (https://github.com/tangency-labs/solidcraft, MIT OR Apache-2.0).
//! Der Configuration Manager hat keine sichere API — daher wie im Original
//! die unsafe-Freigabe in diesem Modul.
#![allow(unsafe_code)]

use std::ffi::CString;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_Get_DevNode_PropertyW, CM_Get_Device_Interface_List_SizeW, CM_Get_Device_Interface_ListW,
    CM_Get_Device_Interface_PropertyW, CM_LOCATE_DEVNODE_NORMAL, CM_Locate_DevNodeW, CONFIGRET, CR_BUFFER_SMALL, CR_SUCCESS,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::GUID_DEVINTERFACE_HID;
use windows_sys::Win32::Devices::Properties::{
    DEVPKEY_Device_HardwareIds, DEVPKEY_Device_InstanceId, DEVPROP_TYPE_STRING, DEVPROP_TYPE_STRING_LIST, DEVPROPTYPE,
};
use windows_sys::Win32::Foundation::DEVPROPKEY;

/// Die Hardware-ID, die Windows einer Top-Level-Collection mit dem
/// Generic-Desktop-Usage „Multi-Axis Controller“ gibt.
const MULTI_AXIS_CONTROLLER: &str = "HID_DEVICE_UP:0001_U:0008";
/// Größte Property-Lesung in Bytes: Hardware-IDs sind ein paar hundert.
const MAX_PROPERTY_BYTES: u32 = 64 * 1024;
/// Versuche bei der Interface-Liste, die zwischen Größenanfrage und Lesung
/// wachsen kann.
const LIST_TRIES: usize = 4;

/// Die Pfade der anwesenden HID-Interfaces, deren Top-Level-Collection ein
/// Multi-Axis-Controller ist.
pub(crate) fn multi_axis_controllers() -> Vec<CString> {
    interfaces()
        .into_iter()
        .filter(|path| {
            let Some(instance) = interface_strings(path, &DEVPKEY_Device_InstanceId).and_then(|ids| ids.into_iter().next()) else {
                return false;
            };
            devnode_strings(&instance, &DEVPKEY_Device_HardwareIds).is_some_and(|ids| is_multi_axis(&ids))
        })
        .filter_map(|path| CString::new(path).ok())
        .collect()
}

/// Ist ein Gerät mit diesen Hardware-IDs ein Multi-Axis-Controller?
fn is_multi_axis(hardware_ids: &[String]) -> bool {
    hardware_ids.iter().any(|id| id.eq_ignore_ascii_case(MULTI_AXIS_CONTROLLER))
}

/// Eine NUL-getrennte Liste (`REG_MULTI_SZ` oder ein String) als Strings,
/// ohne leere.
fn split_multi_sz(buffer: &[u16]) -> Vec<String> {
    buffer.split(|c| *c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

/// `s` als NUL-terminierter Wide-String.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Die Pfade der anwesenden HID-Interfaces.
fn interfaces() -> Vec<String> {
    for _ in 0..LIST_TRIES {
        let mut len = 0u32;
        // SAFETY: `len` ist ein gültiger Platz für die Größe; die Klassen-GUID
        // ist statisch; kein Gerätefilter.
        let sized = unsafe { CM_Get_Device_Interface_List_SizeW(&mut len, &GUID_DEVINTERFACE_HID, null(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT) };
        if sized != CR_SUCCESS || len == 0 {
            return Vec::new();
        }
        let mut buffer = vec![0u16; len as usize];
        // SAFETY: `buffer` hält `len` Wide-Zeichen, die Länge, die der Aufruf bekommt.
        let listed =
            unsafe { CM_Get_Device_Interface_ListW(&GUID_DEVINTERFACE_HID, null(), buffer.as_mut_ptr(), len, CM_GET_DEVICE_INTERFACE_LIST_PRESENT) };
        match listed {
            CR_SUCCESS => return split_multi_sz(&buffer),
            CR_BUFFER_SMALL => continue,
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

/// Eine String- oder Stringlisten-Property, gelesen von `get` (property type,
/// Buffer, seine Größe in Bytes) in zwei Aufrufen: erst die Größe, dann der Wert.
fn property_strings(get: impl Fn(*mut DEVPROPTYPE, *mut u8, *mut u32) -> CONFIGRET) -> Option<Vec<String>> {
    let mut kind: DEVPROPTYPE = 0;
    let mut size = 0u32;
    if get(&mut kind, null_mut(), &mut size) != CR_BUFFER_SMALL || size == 0 || size > MAX_PROPERTY_BYTES {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    let mut size = (buffer.len() * 2) as u32;
    if get(&mut kind, buffer.as_mut_ptr().cast(), &mut size) != CR_SUCCESS || (kind != DEVPROP_TYPE_STRING && kind != DEVPROP_TYPE_STRING_LIST) {
        return None;
    }
    Some(split_multi_sz(&buffer))
}

/// Eine String-Property des Geräte-Interfaces unter `path`.
fn interface_strings(path: &str, key: &DEVPROPKEY) -> Option<Vec<String>> {
    let path = wide(path);
    property_strings(|kind, buffer, size| {
        // SAFETY: `path` ist NUL-terminiert und outlebt den Aufruf; `buffer`
        // ist null (Größenanfrage) oder hält `*size` Bytes, wie
        // `property_strings` es einrichtet.
        unsafe { CM_Get_Device_Interface_PropertyW(path.as_ptr(), key, kind, buffer, size, 0) }
    })
}

/// Eine String-Property des Geräts mit der Instance-ID `instance`.
fn devnode_strings(instance: &str, key: &DEVPROPKEY) -> Option<Vec<String>> {
    let instance = wide(instance);
    let mut devnode = 0u32;
    // SAFETY: `instance` ist NUL-terminiert und outlebt den Aufruf; `devnode`
    // ist ein gültiger Platz für das Ergebnis.
    if unsafe { CM_Locate_DevNodeW(&mut devnode, instance.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) } != CR_SUCCESS {
        return None;
    }
    property_strings(|kind, buffer, size| {
        // SAFETY: `buffer` ist null (Größenanfrage) oder hält `*size` Bytes,
        // wie `property_strings` es einrichtet.
        unsafe { CM_Get_DevNode_PropertyW(devnode, key, kind, buffer, size, 0) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Die Hardware-IDs, die Windows einem kabelgebundenen 3Dconnexion-Gerät
    /// an der Bewegungs-Schnittstelle gibt, und einer Tastatur: nur die
    /// ersten sind ein Multi-Axis-Controller.
    #[test]
    fn only_the_multi_axis_usage_is_a_3d_mouse() {
        let ids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let space_mouse = ids(&["HID\\VID_256F&PID_C635&REV_0437&MI_00", "HID\\VID_256F&UP:0001_U:0008", "HID_DEVICE_UP:0001_U:0008", "HID_DEVICE"]);
        let keyboard = ids(&[
            "HID\\VID_046D&PID_C31C&REV_6400&MI_00",
            "HID\\VID_046D&UP:0001_U:0006",
            "HID_DEVICE_SYSTEM_KEYBOARD",
            "HID_DEVICE_UP:0001_U:0006",
            "HID_DEVICE",
        ]);
        let vendor = ids(&["HID\\VID_256F&UP:FF00_U:0001", "HID_DEVICE_UP:FF00_U:0001", "HID_DEVICE_UPR:FF00-FFFF", "HID_DEVICE"]);
        assert!(is_multi_axis(&space_mouse));
        assert!(is_multi_axis(&ids(&["hid_device_up:0001_u:0008"])), "IDs vergleichen Groß-/Kleinschreibung-frei");
        assert!(!is_multi_axis(&keyboard) && !is_multi_axis(&vendor) && !is_multi_axis(&[]));
    }

    #[test]
    fn nul_separated_lists_split_into_their_strings() {
        let buffer: Vec<u16> = "a\\b\0HID_DEVICE\0\0".encode_utf16().collect();
        assert_eq!(split_multi_sz(&buffer), ["a\\b", "HID_DEVICE"]);
        assert_eq!(split_multi_sz(&wide("one")), ["one"]);
        assert!(split_multi_sz(&[0, 0]).is_empty() && split_multi_sz(&[]).is_empty());
    }
}
