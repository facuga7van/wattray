//! Thin Win32 HID layer: enumerate interfaces, open shared, write/read with timeout, feature reports.

use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::*;
use windows_sys::Win32::System::Threading::*;

pub static DEBUG: AtomicBool = AtomicBool::new(false);
static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn dump(tag: &str, b: &[u8]) {
    if DEBUG.load(Ordering::Relaxed) {
        let hex: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
        println!("  {:>5}ms {tag} {}", START.get_or_init(std::time::Instant::now).elapsed().as_millis(), hex.join(" "));
    }
}

#[derive(Debug)]
pub struct HidInfo {
    pub path: Vec<u16>,
    pub pid: u16,
    pub mi: Option<u8>,
    pub usage_page: u16,
    pub usage: u16,
    pub input_len: u16,
    pub output_len: u16,
    pub feature_len: u16,
    pub product: String,
}

/// All present HID interfaces with vendor id `vid`.
pub fn enumerate(vid: u16) -> Vec<HidInfo> {
    let mut out = Vec::new();
    unsafe {
        let mut guid = std::mem::zeroed();
        HidD_GetHidGuid(&mut guid);
        let set = SetupDiGetClassDevsW(&guid, null(), null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if set as isize == INVALID_HANDLE_VALUE as isize {
            return out;
        }
        let mut i = 0;
        loop {
            let mut ifd: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            ifd.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(set, null(), &guid, i, &mut ifd) == 0 {
                break;
            }
            i += 1;
            // Detail struct: u32 cbSize + WCHAR path[]; aligned buffer of u32.
            let mut buf = [0u32; 256];
            let det = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*det).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &ifd, det, size_of_val(&buf) as u32, null_mut(), null_mut()) == 0 {
                continue;
            }
            let p = (*det).DevicePath.as_ptr();
            let len = (0..).take_while(|&k| *p.add(k) != 0).count();
            let path: Vec<u16> = std::slice::from_raw_parts(p, len).iter().copied().chain([0]).collect();
            let lower = String::from_utf16_lossy(&path).to_ascii_lowercase();
            if !lower.contains(&format!("vid_{vid:04x}")) {
                continue;
            }
            if let Some(info) = probe(path, &lower) {
                out.push(info);
            }
        }
        SetupDiDestroyDeviceInfoList(set);
    }
    out
}

unsafe fn probe(path: Vec<u16>, lower: &str) -> Option<HidInfo> {
    unsafe {
        // Access 0: lets us read attributes/caps even of collections the OS holds exclusively.
        let h = CreateFileW(path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut());
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
        attr.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
        let mut pp = 0;
        let mut caps: HIDP_CAPS = std::mem::zeroed();
        let ok = HidD_GetAttributes(h, &mut attr)
            && HidD_GetPreparsedData(h, &mut pp)
            && {
                let st = HidP_GetCaps(pp, &mut caps);
                HidD_FreePreparsedData(pp);
                st == HIDP_STATUS_SUCCESS
            };
        let mut name = [0u16; 128];
        let product = if HidD_GetProductString(h, name.as_mut_ptr() as _, size_of_val(&name) as u32) {
            String::from_utf16_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(0)]).trim().to_string()
        } else {
            String::new()
        };
        CloseHandle(h);
        if !ok {
            return None;
        }
        let mi = lower.find("&mi_").and_then(|k| u8::from_str_radix(lower.get(k + 4..k + 6)?, 16).ok());
        Some(HidInfo {
            path,
            pid: attr.ProductID,
            mi,
            usage_page: caps.UsagePage,
            usage: caps.Usage,
            input_len: caps.InputReportByteLength,
            output_len: caps.OutputReportByteLength,
            feature_len: caps.FeatureReportByteLength,
            product,
        })
    }
}

pub struct Dev {
    h: HANDLE,
    ev: HANDLE,
    out_len: usize,
    in_len: usize,
    feat_len: usize,
}

impl Dev {
    pub fn open(info: &HidInfo) -> Option<Dev> {
        unsafe {
            let h = CreateFileW(
                info.path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                return None;
            }
            let ev = CreateEventW(null(), 1, 0, null());
            Some(Dev {
                h,
                ev,
                out_len: info.output_len as usize,
                in_len: info.input_len as usize,
                feat_len: info.feature_len as usize,
            })
        }
    }

    /// Overlapped read or write that gives up after `ms`.
    fn io(&self, write: bool, buf: &mut [u8], ms: u32) -> Option<usize> {
        unsafe {
            let mut ov: OVERLAPPED = std::mem::zeroed();
            ov.hEvent = self.ev;
            ResetEvent(self.ev);
            let ok = if write {
                WriteFile(self.h, buf.as_ptr(), buf.len() as u32, null_mut(), &mut ov)
            } else {
                ReadFile(self.h, buf.as_mut_ptr(), buf.len() as u32, null_mut(), &mut ov)
            };
            if ok == 0 && GetLastError() != ERROR_IO_PENDING {
                return None;
            }
            let mut n = 0;
            if WaitForSingleObject(self.ev, ms) != WAIT_OBJECT_0 {
                CancelIoEx(self.h, &ov);
                GetOverlappedResult(self.h, &ov, &mut n, 1); // wait for the cancel before buf/ov drop
                return None;
            }
            (GetOverlappedResult(self.h, &ov, &mut n, 0) != 0).then_some(n as usize)
        }
    }

    /// Writes an output report; `data[0]` is the report id, padded to the report length.
    pub fn write(&self, data: &[u8]) -> bool {
        let mut buf = vec![0u8; self.out_len.max(data.len())];
        buf[..data.len()].copy_from_slice(data);
        dump(">", &buf);
        self.io(true, &mut buf, 500).is_some()
    }

    /// Reads one input report (byte 0 = report id), or None on timeout.
    pub fn read(&self, ms: u32) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; self.in_len.max(1)];
        let n = self.io(false, &mut buf, ms)?;
        buf.truncate(n);
        dump("<", &buf);
        Some(buf)
    }

    pub fn set_feature(&self, data: &[u8]) -> bool {
        let mut buf = vec![0u8; self.feat_len.max(data.len())];
        buf[..data.len()].copy_from_slice(data);
        dump("F>", &buf);
        unsafe { HidD_SetFeature(self.h, buf.as_ptr() as _, buf.len() as u32) }
    }

    pub fn get_feature(&self, report_id: u8) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; self.feat_len.max(1)];
        buf[0] = report_id;
        let ok = unsafe { HidD_GetFeature(self.h, buf.as_mut_ptr() as _, buf.len() as u32) };
        dump("F<", &buf);
        ok.then_some(buf)
    }
}

impl Drop for Dev {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.ev);
            CloseHandle(self.h);
        }
    }
}
