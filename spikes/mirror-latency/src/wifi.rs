//! Asks Windows to tune the Wi-Fi adapter for streaming while the tool runs,
//! the way Miracast and game streaming do: media streaming mode (less power
//! saving), and no background scans (each one takes the radio off the
//! channel for tens of milliseconds). Both last as long as the handle is open.

use std::ffi::c_void;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::*;

pub struct Tuned(HANDLE);

impl Drop for Tuned {
    fn drop(&mut self) {
        unsafe {
            WlanCloseHandle(self.0, None);
        }
    }
}

pub fn tune() -> Option<Tuned> {
    unsafe {
        let mut version = 0u32;
        let mut handle = HANDLE::default();
        if WlanOpenHandle(2, None, &mut version, &mut handle) != 0 {
            println!("Wi-Fi: no WLAN service");
            return None;
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(handle, None, &mut list) != 0 || list.is_null() {
            WlanCloseHandle(handle, None);
            return None;
        }
        let n = (*list).dwNumberOfItems as usize;
        let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n);
        for item in items {
            if item.isState != wlan_interface_state_connected {
                continue;
            }
            let on: i32 = 1;
            let off: i32 = 0;
            let streaming = WlanSetInterface(
                handle,
                &item.InterfaceGuid,
                wlan_intf_opcode_media_streaming_mode,
                4,
                &on as *const i32 as *const c_void,
                None,
            );
            let scans = WlanSetInterface(
                handle,
                &item.InterfaceGuid,
                wlan_intf_opcode_background_scan_enabled,
                4,
                &off as *const i32 as *const c_void,
                None,
            );
            let name = String::from_utf16_lossy(&item.strInterfaceDescription).trim_end_matches('\0').to_string();
            println!(
                "Wi-Fi: {name}: streaming mode {}, background scans off {}",
                if streaming == 0 { "on" } else { "REFUSED" },
                if scans == 0 { "yes" } else { "REFUSED" },
            );
        }
        WlanFreeMemory(list as *const c_void);
        Some(Tuned(handle))
    }
}
