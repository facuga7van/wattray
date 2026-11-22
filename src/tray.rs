//! Tray icons: one per reading, battery number drawn with GDI into a 32-bpp alpha icon.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::reading::{label, Reading};

pub const WM_TRAY: u32 = WM_APP + 1;

pub struct Tray {
    hwnd: HWND,
    shown: Vec<Reading>,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Tray {
        Tray { hwnd, shown: Vec::new() }
    }

    /// Diff by id: add new devices, drop gone ones, redraw only changed ones.
    pub fn update(&mut self, new: Vec<Reading>) {
        let new = merge(&self.shown, new);
        for old in &self.shown {
            if !new.iter().any(|r| r.id == old.id) {
                self.notify(NIM_DELETE, old);
            }
        }
        for r in &new {
            match self.shown.iter().find(|o| o.id == r.id) {
                Some(o) if o == r => {}
                Some(_) => self.notify(NIM_MODIFY, r),
                None => self.notify(NIM_ADD, r),
            }
        }
        self.shown = new;
    }

    /// Explorer restarted ("TaskbarCreated"): all our icons are gone, add them back.
    pub fn readd_all(&self) {
        self.shown.iter().for_each(|r| self.notify(NIM_ADD, r));
    }

    /// Taskbar theme changed: redraw with the new text color.
    pub fn redraw_all(&self) {
        self.shown.iter().for_each(|r| self.notify(NIM_MODIFY, r));
    }

    pub fn clear(&mut self) {
        self.shown.iter().for_each(|r| self.notify(NIM_DELETE, r));
        self.shown.clear();
    }

    fn notify(&self, msg: NOTIFY_ICON_MESSAGE, r: &Reading) {
        unsafe {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = self.hwnd;
            nid.uID = uid(&r.id);
            if msg != NIM_DELETE {
                nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
                nid.uCallbackMessage = WM_TRAY;
                nid.hIcon = render(r.percent, color(r.percent, r.charging, light_taskbar()));
                let tip = tooltip(r);
                for (d, s) in nid.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
                    *d = s;
                }
            }
            Shell_NotifyIconW(msg, &nid);
            if msg == NIM_ADD {
                nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                Shell_NotifyIconW(NIM_SETVERSION, &nid);
            }
            if !nid.hIcon.is_null() {
                DestroyIcon(nid.hIcon); // the shell keeps its own copy
            }
        }
    }
}

/// Next set of icons: asleep devices keep their last shown value (hidden if never seen awake);
/// sorted and deduplicated by id.
fn merge(shown: &[Reading], new: Vec<Reading>) -> Vec<Reading> {
    let mut out: Vec<Reading> = new
        .into_iter()
        .filter_map(|r| if r.asleep { shown.iter().find(|o| o.id == r.id).cloned() } else { Some(r) })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

fn tooltip(r: &Reading) -> String {
    let charging = if !r.charging { "" } else if crate::spanish() { ", cargando" } else { ", charging" };
    format!("{} — {}%{}", r.name, r.percent, charging)
}

/// Stable icon id per device (FNV-1a), so icons keep their place across polls.
fn uid(id: &str) -> u32 {
    id.bytes().fold(0x811C9DC5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

fn light_taskbar() -> bool {
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0".encode_utf16().collect();
    let val: Vec<u16> = "SystemUsesLightTheme\0".encode_utf16().collect();
    let (mut data, mut size) = (0u32, 4u32);
    let st = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr(), RRF_RT_REG_DWORD, null_mut(), &mut data as *mut u32 as _, &mut size)
    };
    st == ERROR_SUCCESS && data == 1
}

/// Text color 0xRRGGBB: warning tints at <=20 / <=10 % (not while charging), per taskbar theme.
fn color(percent: u8, charging: bool, light: bool) -> u32 {
    let level = if charging { 2 } else if percent <= 10 { 0 } else if percent <= 20 { 1 } else { 2 };
    let palette = if light { [0xC42B1C, 0x9D5D00, 0x1A1A1A] } else { [0xFF6B6B, 0xFFB900, 0xFFFFFF] };
    palette[level]
}

/// Draws the number white-on-black, takes luminance as alpha, tints it, and builds an HICON.
/// One font size for every number: the largest at which "88" (widest label) fills the icon.
/// Glyphs are centered on their real ink bounds.
fn render(percent: u8, c: u32) -> HICON {
    unsafe {
        let s = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()).max(16);
        let scratch = s * 2;
        let dc = CreateCompatibleDC(null_mut());
        let (bmp, bits) = dib(dc, scratch);
        SelectObject(dc, bmp);
        SetTextColor(dc, 0xFFFFFF);
        SetBkMode(dc, TRANSPARENT as i32);
        let px = std::slice::from_raw_parts(bits, (scratch * scratch) as usize);

        let mut em = s * 3 / 2;
        while em > 6 && draw(dc, bits, scratch, em, "88").is_none_or(|(x0, y0, x1, y1)| x1 - x0 >= s || y1 - y0 >= s) {
            em -= 1;
        }
        let mut alpha = vec![0u8; (s * s) as usize];
        if let Some((x0, y0, x1, y1)) = draw(dc, bits, scratch, em, &label(percent)) {
            let (w, h) = ((x1 - x0 + 1).min(s), (y1 - y0 + 1).min(s));
            let (ox, oy) = ((s - w) / 2, (s - h) / 2);
            for y in 0..h {
                for x in 0..w {
                    alpha[((y + oy) * s + x + ox) as usize] = (px[((y + y0) * scratch + x + x0) as usize] & 0xFF) as u8;
                }
            }
        }
        DeleteObject(bmp);

        // Icon alpha is straight (not premultiplied), as in .ico files.
        let (icon_bmp, icon_bits) = dib(dc, s);
        for (i, a) in alpha.iter().enumerate() {
            *icon_bits.add(i) = (*a as u32) << 24 | c;
        }
        let mask_bytes = vec![0u8; ((s + 15) / 16 * 2 * s) as usize];
        let mask = CreateBitmap(s, s, 1, 1, mask_bytes.as_ptr() as _);
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: icon_bmp };
        let icon = CreateIconIndirect(&info);
        DeleteObject(mask);
        DeleteObject(icon_bmp);
        DeleteDC(dc);
        icon
    }
}

/// Renders `text` at font size `em` into the cleared scratch bitmap; returns its ink bounds.
unsafe fn draw(dc: HDC, bits: *mut u32, scratch: i32, em: i32, text: &str) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let font = CreateFontW(-em, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32, ANTIALIASED_QUALITY as u32, DEFAULT_PITCH as u32, w("Segoe UI Semibold").as_ptr());
        let old = SelectObject(dc, font);
        std::ptr::write_bytes(bits, 0, (scratch * scratch) as usize);
        let t: Vec<u16> = text.encode_utf16().collect();
        TextOutW(dc, 2, 0, t.as_ptr(), t.len() as i32);
        GdiFlush();
        SelectObject(dc, old);
        DeleteObject(font);
        ink_bounds(std::slice::from_raw_parts(bits, (scratch * scratch) as usize), scratch)
    }
}

unsafe fn dib(dc: HDC, s: i32) -> (HBITMAP, *mut u32) {
    unsafe {
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = s;
        bi.bmiHeader.biHeight = -s; // top-down
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        let mut bits = null_mut();
        let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        (bmp, bits as *mut u32)
    }
}

/// Bounding box (inclusive) of non-black pixels.
fn ink_bounds(px: &[u32], s: i32) -> Option<(i32, i32, i32, i32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (s, s, -1, -1);
    for y in 0..s {
        for x in 0..s {
            if px[(y * s + x) as usize] & 0xFF > 24 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    (x1 >= 0).then_some((x0, y0, x1, y1))
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_by_level() {
        assert_eq!(color(50, false, false), 0xFFFFFF);
        assert_eq!(color(20, false, false), 0xFFB900);
        assert_eq!(color(10, false, true), 0xC42B1C);
        assert_eq!(color(5, true, false), 0xFFFFFF); // charging: no alarm
    }

    fn r(id: &str, percent: u8, asleep: bool) -> Reading {
        Reading { id: id.into(), name: id.into(), percent, charging: false, asleep }
    }

    #[test]
    fn asleep_keeps_last_value() {
        let shown = vec![r("mouse", 93, false)];
        assert_eq!(merge(&shown, vec![r("mouse", 0, true)]), vec![r("mouse", 93, false)]);
        assert_eq!(merge(&[], vec![r("mouse", 0, true)]), vec![]); // never seen awake
        assert_eq!(merge(&shown, vec![]), vec![]); // gone = removed
        assert_eq!(merge(&shown, vec![r("b", 1, false), r("a", 2, false)]), vec![r("a", 2, false), r("b", 1, false)]);
    }

    #[test]
    fn render_does_not_leak_gdi() {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS, GR_USEROBJECTS};
        let count = || unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) + GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS) };
        unsafe { DestroyIcon(render(50, 0xFFFFFF)) }; // warm up caches
        let before = count();
        for p in 0..=100 {
            let icon = render(p, 0xFFFFFF);
            assert!(!icon.is_null());
            unsafe { DestroyIcon(icon) };
        }
        assert!(count() <= before + 2, "GDI/USER objects grew: {before} -> {}", count());
    }

    #[test]
    fn uid_is_stable() {
        assert_eq!(uid("ss:12e0"), uid("ss:12e0"));
        assert_ne!(uid("ss:12e0"), uid("logi:c547:01"));
    }
}
