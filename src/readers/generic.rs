//! Any HID interface that reports its battery the standard way: Generic Device Controls "Battery
//! Strength" (0x06/0x20) or Battery System state of charge (0x85/0x65, 0x85/0x64), in a feature or
//! input report. Read-only: opened with no access rights, only GET_REPORT requests are sent.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;

use crate::hid::{self, HidInfo};
use crate::reading::{Kind, Reading};

/// Vendors with their own reader: their devices are read there, not twice.
// ponytail: whole-vendor skip; a Logitech/HP/... device that only speaks the standard is missed, narrow to pids if one shows up.
const OWN_READER: [u16; 6] = [0x046d, 0x1038, 0x1532, 0x1b1c, 0x03f0, 0x0951];
/// (usage page, usage) that carry the battery level, most specific first.
const USAGES: [(u16, u16); 3] = [(0x06, 0x20), (0x85, 0x65), (0x85, 0x64)];
/// Bluetooth HID (classic and LE): bt.rs already reads Windows' own battery value for these.
const BT: [&str; 2] = ["{00001124-0000-1000-8000-00805f9b34fb}", "{00001812-0000-1000-8000-00805f9b34fb}"];

pub fn read(out: &mut Vec<Reading>) {
    let mut done: Vec<String> = Vec::new();
    for info in hid::enumerate_all() {
        let id = format!("hid:{:04x}:{:04x}", info.vid, info.pid);
        // compx and sinowealth ids end in vid:pid: a device they already showed is not shown again
        let tail = format!("{:04x}:{:04x}", info.vid, info.pid);
        if OWN_READER.contains(&info.vid) || done.contains(&id) || out.iter().any(|o| o.id.ends_with(&tail)) || bluetooth(&info) {
            continue;
        }
        let Some(percent) = battery(&info) else { continue };
        done.push(id.clone());
        let name = if info.product.is_empty() { format!("{:04x}:{:04x}", info.vid, info.pid) } else { info.product.clone() };
        // ponytail: charging (0x85/0x44) not read; add it when a device that reports it shows up.
        out.push(Reading { id, kind: Kind::guess(&name), name, percent, charging: false, asleep: false });
    }
}

fn bluetooth(info: &HidInfo) -> bool {
    let path = String::from_utf16_lossy(&info.path).to_ascii_lowercase();
    BT.iter().any(|g| path.contains(g))
}

/// Percent from the first battery usage the interface declares that answers, or None.
fn battery(info: &HidInfo) -> Option<u8> {
    unsafe {
        // Access 0 is enough for GET_REPORT and works on collections the OS holds exclusively.
        let h = CreateFileW(info.path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut());
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut pp = 0;
        let mut found = None;
        if HidD_GetPreparsedData(h, &mut pp) {
            found = [(HidP_Feature, info.feature_len), (HidP_Input, info.input_len)]
                .into_iter()
                .find_map(|(kind, len)| read_value(h, pp, kind, len));
            HidD_FreePreparsedData(pp);
        }
        CloseHandle(h);
        found
    }
}

unsafe fn read_value(h: HANDLE, pp: PHIDP_PREPARSED_DATA, kind: HIDP_REPORT_TYPE, len: u16) -> Option<u8> {
    if len == 0 {
        return None;
    }
    unsafe {
        let mut caps: Vec<HIDP_VALUE_CAPS> = vec![std::mem::zeroed(); 128];
        let mut n = caps.len() as u16;
        if HidP_GetValueCaps(kind, caps.as_mut_ptr(), &mut n, pp) != HIDP_STATUS_SUCCESS {
            return None;
        }
        for &(page, usage) in &USAGES {
            for c in caps[..n as usize].iter().filter(|c| c.UsagePage == page) {
                let declared = if c.IsRange {
                    (c.Anonymous.Range.UsageMin..=c.Anonymous.Range.UsageMax).contains(&usage)
                } else {
                    c.Anonymous.NotRange.Usage == usage
                };
                if !declared {
                    continue;
                }
                let mut buf = vec![0u8; len as usize];
                buf[0] = c.ReportID;
                let ok = if kind == HidP_Feature {
                    HidD_GetFeature(h, buf.as_mut_ptr() as _, len as u32)
                } else {
                    HidD_GetInputReport(h, buf.as_mut_ptr() as _, len as u32)
                };
                hid::dump(if kind == HidP_Feature { "F<" } else { "I<" }, &buf);
                let mut v = 0u32;
                if ok
                    && HidP_GetUsageValue(kind, page, c.LinkCollection, usage, &mut v, pp, buf.as_ptr(), len as u32)
                        == HIDP_STATUS_SUCCESS
                {
                    if let Some(p) = pct(v, c.LogicalMin, c.LogicalMax, c.BitSize) {
                        return Some(p);
                    }
                }
            }
        }
        None
    }
}

/// Raw value to percent over the declared logical range; None for a null (out of range) value.
fn pct(v: u32, min: i32, max: i32, bits: u16) -> Option<u8> {
    let (v, min) = (v as i64, min as i64);
    // An 8-bit "0..255" often parses as max -1 (sign-extended); fall back to the field's full range.
    let max = if (max as i64) > min { max as i64 } else { (1i64 << bits.min(32)) - 1 };
    if v < min || v > max || max <= min {
        return None;
    }
    Some((((v - min) * 100 + (max - min) / 2) / (max - min)) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_over_logical_range() {
        assert_eq!(pct(73, 0, 100, 8), Some(73));
        assert_eq!(pct(128, 0, 255, 8), Some(50));
        assert_eq!(pct(255, 0, -1, 8), Some(100)); // 0..255 sign-extended by the parser
        assert_eq!(pct(101, 0, 100, 8), None); // null value: unknown
        assert_eq!(pct(5, 0, 0, 0), None);
    }
}
