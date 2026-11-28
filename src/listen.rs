//! `--listen VID:PID [seconds]`: read-only capture for a device whose protocol nobody documented.
//! Lists each HID interface with the report ids it declares, reads every declared feature report
//! once (GET_REPORT, never SET), then prints every input report the device sends on its own.

use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;

use crate::hid::{self, Dev, HidInfo};

pub fn run(target: Option<&str>, secs: Option<&str>) {
    let Some((vid, pid)) = target.and_then(parse_target) else {
        println!("usage: wattray.exe --listen VID:PID [seconds]    e.g. wattray.exe --listen 9886:0050 60");
        return;
    };
    let secs = secs.and_then(|s| s.parse().ok()).unwrap_or(60u64);
    let ifaces: Vec<HidInfo> = hid::enumerate(vid).into_iter().filter(|i| i.pid == pid).collect();
    if ifaces.is_empty() {
        println!("{vid:04x}:{pid:04x} not found. Is it plugged in?");
        return;
    }
    println!("{vid:04x}:{pid:04x} {}", ifaces[0].product);
    for (n, i) in ifaces.iter().enumerate() {
        let mi = i.mi.map_or("--".into(), |m| format!("mi{m}"));
        let ids = feature_ids(i);
        println!("[{n}] {mi} {:04x}:{:04x} {}/{}/{} feature ids {ids:02x?}", i.usage_page, i.usage, i.input_len,
            i.output_len, i.feature_len);
        let Some(dev) = Dev::open(i) else { continue };
        for id in ids {
            match dev.get_feature(id) {
                Some(r) => println!("    F<{id:02x} {}", hid::hex(&r)),
                None => println!("    F<{id:02x} (no answer)"),
            }
        }
    }
    println!("\nListening {secs} s. Now: turn the device off and on, plug and unplug its charging cable, use its buttons.\n");
    let start = Instant::now();
    let deadline = start + Duration::from_secs(secs);
    let threads: Vec<_> = ifaces
        .into_iter()
        .enumerate()
        .filter(|(_, i)| i.input_len > 0)
        .map(|(n, i)| {
            std::thread::spawn(move || {
                let Some(dev) = Dev::open(&i) else { return };
                while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                    if let Some(r) = dev.read(left.as_millis().clamp(1, 500) as u32) {
                        println!("{:>7}ms [{n}] {}", start.elapsed().as_millis(), hid::hex(&r));
                    } else if Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(20)); // unreadable collection: don't spin
                    }
                }
            })
        })
        .collect();
    for t in threads {
        let _ = t.join();
    }
    println!("\ndone");
}

fn parse_target(s: &str) -> Option<(u16, u16)> {
    let (v, p) = s.split_once(':')?;
    Some((u16::from_str_radix(v, 16).ok()?, u16::from_str_radix(p, 16).ok()?))
}

/// Report ids the interface declares for feature reports (0 when it uses none).
fn feature_ids(info: &HidInfo) -> Vec<u8> {
    let mut ids = Vec::new();
    if info.feature_len == 0 {
        return ids;
    }
    unsafe {
        let h = CreateFileW(info.path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut());
        if h == INVALID_HANDLE_VALUE {
            return ids;
        }
        let mut pp = 0;
        if HidD_GetPreparsedData(h, &mut pp) {
            let mut values: Vec<HIDP_VALUE_CAPS> = vec![std::mem::zeroed(); 256];
            let mut n = values.len() as u16;
            if HidP_GetValueCaps(HidP_Feature, values.as_mut_ptr(), &mut n, pp) == HIDP_STATUS_SUCCESS {
                ids.extend(values[..n as usize].iter().map(|c| c.ReportID));
            }
            let mut buttons: Vec<HIDP_BUTTON_CAPS> = vec![std::mem::zeroed(); 256];
            let mut n = buttons.len() as u16;
            if HidP_GetButtonCaps(HidP_Feature, buttons.as_mut_ptr(), &mut n, pp) == HIDP_STATUS_SUCCESS {
                ids.extend(buttons[..n as usize].iter().map(|c| c.ReportID));
            }
            HidD_FreePreparsedData(pp);
        }
        CloseHandle(h);
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_parsing() {
        assert_eq!(parse_target("9886:0050"), Some((0x9886, 0x0050)));
        assert_eq!(parse_target("9886"), None);
        assert_eq!(parse_target("zz:0050"), None);
    }
}
