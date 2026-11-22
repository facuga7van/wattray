//! Bluetooth (any brand): Windows already exposes the battery level as a device property.

use std::ptr::{null, null_mut};

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Foundation::{DEVPROPKEY, INVALID_HANDLE_VALUE};

use crate::reading::Reading;

const BATTERY: DEVPROPKEY = key(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5, 2);
const CONNECTED: DEVPROPKEY = key(0x83DA6326_97A6_4088_9453_A1923F573B29, 15);
const FRIENDLY_NAME: DEVPROPKEY = key(0xA45C254E_DF1C_4EFD_8020_67D146A850E0, 14);

const fn key(fmtid: u128, pid: u32) -> DEVPROPKEY {
    DEVPROPKEY { fmtid: GUID::from_u128(fmtid), pid }
}

pub fn read(out: &mut Vec<Reading>) {
    for enumerator in ["BTHLE", "BTHENUM"] {
        unsafe { read_enum(enumerator, out) };
    }
}

unsafe fn read_enum(enumerator: &str, out: &mut Vec<Reading>) {
    unsafe {
        let wide: Vec<u16> = enumerator.encode_utf16().chain([0]).collect();
        let set = SetupDiGetClassDevsW(null(), wide.as_ptr(), null_mut(), DIGCF_ALLCLASSES | DIGCF_PRESENT);
        if set as isize == INVALID_HANDLE_VALUE as isize {
            return;
        }
        let mut i = 0;
        loop {
            let mut dev: SP_DEVINFO_DATA = std::mem::zeroed();
            dev.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
            if SetupDiEnumDeviceInfo(set, i, &mut dev) == 0 {
                break;
            }
            i += 1;
            let mut id = [0u16; 256];
            if SetupDiGetDeviceInstanceIdW(set, &dev, id.as_mut_ptr(), id.len() as u32, null_mut()) == 0 {
                continue;
            }
            let id = String::from_utf16_lossy(&id[..id.iter().position(|&c| c == 0).unwrap_or(0)]);
            if !id.contains("\\DEV_") {
                continue; // service/profile child nodes
            }
            let (Some(b), Some(c)) = (prop(set, &dev, &BATTERY), prop(set, &dev, &CONNECTED)) else { continue };
            if c.first() != Some(&0xFF) || b.is_empty() {
                continue; // DEVPROP_BOOLEAN true is 0xFF; disconnected devices keep a stale level
            }
            let name = prop(set, &dev, &FRIENDLY_NAME)
                .map(|n| {
                    let w: Vec<u16> = n.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    String::from_utf16_lossy(&w).trim_end_matches('\0').to_string()
                })
                .unwrap_or_else(|| "Bluetooth".into());
            out.push(Reading { id: format!("bt:{id}"), name, percent: b[0].min(100), charging: false, asleep: false });
        }
        SetupDiDestroyDeviceInfoList(set);
    }
}

unsafe fn prop(set: HDEVINFO, dev: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<Vec<u8>> {
    unsafe {
        let mut ty = 0;
        let mut buf = [0u8; 512];
        let mut need = 0;
        let ok = SetupDiGetDevicePropertyW(set, dev, key, &mut ty, buf.as_mut_ptr(), buf.len() as u32, &mut need, 0);
        (ok != 0).then(|| buf[..need as usize].to_vec())
    }
}
