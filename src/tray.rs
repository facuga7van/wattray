//! Tray icons: one per device, a glyph (what it is) on the left and the battery number on the
//! right, drawn with GDI into a 32-bpp alpha icon.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::reading::{label, Kind, Reading};

pub const WM_TRAY: u32 = WM_APP + 1;

pub struct Tray {
    hwnd: HWND,
    shown: Vec<Reading>,
    /// Icon ids whose taskbar visibility is settled (promoted by us, or chosen by the user).
    settled: Vec<u32>,
    /// Last awake reading of every device, persisted, so an asleep device shows its value even
    /// right after a restart.
    cache: Vec<Reading>,
    /// Devices already warned about for the current discharge.
    warned: Vec<String>,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Tray {
        Tray { hwnd, shown: Vec::new(), settled: Vec::new(), cache: load_cache(), warned: Vec::new() }
    }

    /// Diff by id: add new devices, drop gone ones, redraw only changed ones.
    /// A device whose icons the shell refused is not remembered, so the next poll retries it.
    pub fn update(&mut self, new: Vec<Reading>) {
        let new = merge(&self.shown, &self.cache, new);
        for old in &self.shown {
            if !new.iter().any(|r| r.id == old.id) {
                self.set(NIM_DELETE, old);
            }
        }
        let mut kept = Vec::with_capacity(new.len());
        for r in new {
            let ok = match self.shown.iter().find(|o| o.id == r.id) {
                Some(o) if *o == r => true,
                Some(_) => self.set(NIM_MODIFY, &r) || self.set(NIM_ADD, &r),
                None => self.set(NIM_ADD, &r),
            };
            if ok {
                kept.push(r);
            }
        }
        for r in &kept {
            let (notify, warned) = warn_step(self.warned.contains(&r.id), r);
            if notify {
                self.balloon(r);
            }
            self.warned.retain(|id| *id != r.id);
            if warned {
                self.warned.push(r.id.clone());
            }
        }
        let mut cache = self.cache.clone();
        for r in &kept {
            cache.retain(|c| c.id != r.id);
            cache.push(r.clone());
        }
        cache.sort_by(|a, b| a.id.cmp(&b.id));
        if cache != self.cache {
            save_cache(&cache);
            self.cache = cache;
        }
        self.shown = kept;
        self.promote();
    }

    /// One native notification ("toast") for a device that just went low.
    fn balloon(&self, r: &Reading) {
        unsafe {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = self.hwnd;
            nid.uID = uid(&r.id);
            nid.uFlags = NIF_INFO;
            nid.dwInfoFlags = NIIF_WARNING | NIIF_RESPECT_QUIET_TIME;
            let title = if crate::spanish() { "Batería baja" } else { "Low battery" };
            let text = format!("{} — {}%", r.name, r.percent);
            for (d, s) in nid.szInfoTitle.iter_mut().zip(title.encode_utf16().take(63)) {
                *d = s;
            }
            for (d, s) in nid.szInfo.iter_mut().zip(text.encode_utf16().take(255)) {
                *d = s;
            }
            Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// Explorer restarted ("TaskbarCreated"): all our icons are gone, add them back.
    pub fn readd_all(&mut self) {
        let all = std::mem::take(&mut self.shown);
        self.update(all);
    }

    /// Taskbar theme changed: redraw with the new text color.
    pub fn redraw_all(&self) {
        self.shown.iter().for_each(|r| {
            self.set(NIM_MODIFY, r);
        });
    }

    pub fn clear(&mut self) {
        self.shown.iter().for_each(|r| {
            self.set(NIM_DELETE, r);
        });
        self.shown.clear();
    }

    /// True while some icon still waits for the shell to register it (see `promote`).
    pub fn unsettled(&self) -> bool {
        self.shown.iter().any(|r| !self.settled.contains(&uid(&r.id)))
    }

    /// Windows 11 hides new tray icons behind "^". Show ours on the taskbar the first time they
    /// appear: set IsPromoted=1 on our entries in HKCU\Control Panel\NotifyIconSettings, but only
    /// where the value is absent. Once the user drags an icon away, Windows writes 0 and we leave it.
    ///
    /// Explorer appends each newly promoted icon at the right end, in no reliable order when several
    /// arrive together, so promote ONE per call (the caller spaces calls ~1 s apart): devices then
    /// keep a stable left-to-right order.
    pub fn promote(&mut self) {
        let order: Vec<u32> = self.shown.iter().map(|r| uid(&r.id))
            .filter(|u| !self.settled.contains(u)).collect();
        let entries = notify_entries();
        for u in order {
            let Some((_, sub, has_value)) = entries.iter().find(|e| e.0 == u) else { return }; // not registered yet: wait
            self.settled.push(u);
            if !has_value {
                let one = 1u32;
                let path = w(&format!("Control Panel\\NotifyIconSettings\\{sub}"));
                unsafe {
                    RegSetKeyValueW(HKEY_CURRENT_USER, path.as_ptr(), w("IsPromoted").as_ptr(), REG_DWORD, &one as *const u32 as _, 4);
                }
                return; // one per call
            }
        }
    }

    fn set(&self, msg: NOTIFY_ICON_MESSAGE, r: &Reading) -> bool {
        unsafe {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = self.hwnd;
            nid.uID = uid(&r.id);
            if msg != NIM_DELETE {
                nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
                nid.uCallbackMessage = WM_TRAY;
                nid.hIcon = render(r, light_taskbar());
                for (d, s) in nid.szTip.iter_mut().zip(tooltip(r).encode_utf16().take(127)) {
                    *d = s;
                }
            }
            let ok = Shell_NotifyIconW(msg, &nid) != 0;
            if ok && msg == NIM_ADD {
                nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                Shell_NotifyIconW(NIM_SETVERSION, &nid);
            }
            if !nid.hIcon.is_null() {
                DestroyIcon(nid.hIcon); // the shell keeps its own copy
            }
            ok
        }
    }
}

/// Whether a NotifyIconSettings path is THIS exe. Windows keeps one entry per (exe path, icon id),
/// so other copies (an old install, a dev build) must not count. Paths under known folders may be
/// stored as "{folder-guid}\rest": compare the rest as a suffix.
fn same_exe(stored: &str) -> bool {
    static EXE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let exe = EXE.get_or_init(|| {
        let mut buf = [0u16; 1024];
        let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(null_mut(), buf.as_mut_ptr(), buf.len() as u32) };
        String::from_utf16_lossy(&buf[..n as usize]).to_lowercase()
    });
    path_matches(exe, &stored.to_lowercase())
}

fn path_matches(exe: &str, stored: &str) -> bool {
    match stored.strip_prefix('{').and_then(|r| r.split_once('}')) {
        Some((_, rest)) => !rest.is_empty() && exe.ends_with(rest),
        None => !stored.is_empty() && exe == stored,
    }
}

/// Our entries in HKCU\Control Panel\NotifyIconSettings: (icon uid, subkey name, has IsPromoted).
fn notify_entries() -> Vec<(u32, String, bool)> {
    let mut out = Vec::new();
    unsafe {
        let base = w("Control Panel\\NotifyIconSettings");
        let mut key = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, base.as_ptr(), 0, KEY_READ, &mut key) != ERROR_SUCCESS {
            return out;
        }
        let mut i = 0;
        loop {
            let mut name = [0u16; 64];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(key, i, name.as_mut_ptr(), &mut len, null_mut(), null_mut(), null_mut(), null_mut()) != ERROR_SUCCESS {
                break;
            }
            i += 1;
            let sub = name.as_ptr();
            let Some(uid) = reg_dword(key, sub, "UID") else { continue };
            if same_exe(&reg_string(key, sub, "ExecutablePath")) {
                out.push((uid, String::from_utf16_lossy(&name[..len as usize]), reg_dword(key, sub, "IsPromoted").is_some()));
            }
        }
        RegCloseKey(key);
    }
    out
}

/// Low-battery warning, once per discharge: fires when a device reaches 10 % (not charging) and
/// re-arms once it charges or climbs above 15 %. Returns (notify now, warned after this reading).
fn warn_step(warned: bool, r: &Reading) -> (bool, bool) {
    if r.percent <= 10 && !r.charging {
        (!warned, true)
    } else {
        (false, warned && !r.charging && r.percent <= 15)
    }
}

const CACHE_KEY: &str = "Software\\wattray";
const OLD_CACHE_KEY: &str = "Software\\pila"; // name of 0.1

/// Cache lines: id \t name \t percent \t kind index. Takes over the 0.1 ("pila") key once.
fn load_cache() -> Vec<Reading> {
    let mut text = unsafe { reg_string(HKEY_CURRENT_USER, w(CACHE_KEY).as_ptr(), "last") };
    if text.is_empty() {
        text = unsafe { reg_string(HKEY_CURRENT_USER, w(OLD_CACHE_KEY).as_ptr(), "last") };
    }
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, w(OLD_CACHE_KEY).as_ptr()) };
    parse_cache(&text)
}

fn parse_cache(text: &str) -> Vec<Reading> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let (id, name) = (f.next()?.to_string(), f.next()?.to_string());
            let percent = f.next()?.parse::<u8>().ok()?.min(100);
            let kind = *Kind::ALL.get(f.next()?.parse::<usize>().ok()?)?;
            Some(Reading { id, name, percent, charging: false, kind, asleep: false })
        })
        .collect()
}

fn format_cache(cache: &[Reading]) -> String {
    cache.iter().map(|r| format!("{}\t{}\t{}\t{}\n", r.id, r.name.replace(['\t', '\n'], " "), r.percent, r.kind as usize)).collect()
}

fn save_cache(cache: &[Reading]) {
    let value = w(&format_cache(cache));
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, w(CACHE_KEY).as_ptr(), w("last").as_ptr(), REG_SZ, value.as_ptr() as _, (value.len() * 2) as u32);
    }
}

/// Next set of icons: asleep devices keep their last known value (from this run or the persisted
/// cache; hidden if never seen awake); an asleep id ending in '*' keeps every shown device with
/// that prefix. Sorted, deduplicated.
fn merge(shown: &[Reading], cache: &[Reading], new: Vec<Reading>) -> Vec<Reading> {
    let mut out = Vec::new();
    for r in new {
        if !r.asleep {
            out.push(r);
        } else if let Some(prefix) = r.id.strip_suffix('*') {
            out.extend(shown.iter().filter(|o| o.id.starts_with(prefix)).cloned());
        } else {
            // asleep + charging: a headset that reports no level while charging. Last value, shown green.
            let last = shown.iter().chain(cache).find(|o| o.id == r.id);
            out.extend(last.map(|o| Reading { charging: o.charging || r.charging, ..o.clone() }));
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

fn tooltip(r: &Reading) -> String {
    let charging = if !r.charging { "" } else if crate::spanish() { ", cargando" } else { ", charging" };
    format!("{} — {}%{}", r.name, r.percent, charging)
}

/// Stable icon id per device (FNV-1a). Windows remembers placement and visibility by it.
fn uid(id: &str) -> u32 {
    id.bytes().fold(0x811C9DC5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

const NUMBER_FONT: &str = "Segoe UI Semibold";
const GLYPH_FONT: &str = "Segoe MDL2 Assets"; // on Windows 10 and 11, same code points as Segoe Fluent Icons

fn glyph(kind: Kind) -> &'static str {
    match kind {
        Kind::Headset => "\u{E7F6}",  // Headphone
        Kind::Mouse => "\u{E962}",    // Mouse
        Kind::Keyboard => "\u{E765}", // KeyboardClassic
        Kind::Gamepad => "\u{E7FC}",  // Game
        Kind::Speaker => "\u{E7F5}",  // Speakers
        Kind::Other => "\u{E702}",    // Bluetooth
    }
}

fn light_taskbar() -> bool {
    let key = w("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    unsafe { reg_dword(HKEY_CURRENT_USER, key.as_ptr(), "SystemUsesLightTheme") == Some(1) }
}

unsafe fn reg_dword(key: HKEY, sub: *const u16, value: &str) -> Option<u32> {
    let (mut data, mut size) = (0u32, 4u32);
    let st = unsafe { RegGetValueW(key, sub, w(value).as_ptr(), RRF_RT_REG_DWORD, null_mut(), &mut data as *mut u32 as _, &mut size) };
    (st == ERROR_SUCCESS).then_some(data)
}

unsafe fn reg_string(key: HKEY, sub: *const u16, value: &str) -> String {
    let mut buf = vec![0u16; 8192];
    let mut size = (buf.len() * 2) as u32;
    let st = unsafe { RegGetValueW(key, sub, w(value).as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as _, &mut size) };
    if st != ERROR_SUCCESS {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)])
}

/// Text color 0xRRGGBB: warning tints at <=20 / <=10 %, green while charging, per taskbar theme.
fn color(percent: u8, charging: bool, light: bool) -> u32 {
    let level = if charging { 3 } else if percent <= 10 { 0 } else if percent <= 20 { 1 } else { 2 };
    let palette = if light { [0xC42B1C, 0x9D5D00, 0x1A1A1A, 0x107C10] } else { [0xFF6B6B, 0xFFB900, 0xFFFFFF, 0x6CCB5F] };
    palette[level]
}

/// Builds the device icon: glyph left, number right-aligned, sharing one square.
/// The number gets the room "88" needs (so every value has one size); the glyph gets the rest.
fn render(r: &Reading, light: bool) -> HICON {
    unsafe {
        let s = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()).max(16);
        if s < 24 {
            return icon(s, &pixel_art(r, light, s)); // fonts turn to mush below 24 px
        }
        let gap = (s / 16).max(1);
        let digits_w = s * 9 / 16;
        let (num, nw, nh) = raster(&label(r.percent), NUMBER_FONT, "88", digits_w, s);
        let (gly, gw, gh) = raster(glyph(r.kind), GLYPH_FONT, glyph(r.kind), s - digits_w - gap, s * 3 / 4);

        // Straight (not premultiplied) BGRA, as in .ico files.
        let mut px = vec![0u32; (s * s) as usize];
        let mut blit = |a: &[u8], w: i32, h: i32, ox: i32, c: u32| {
            let oy = (s - h) / 2;
            for y in 0..h {
                for x in 0..w {
                    let v = a[(y * w + x) as usize] as u32;
                    if v > 0 {
                        px[((y + oy) * s + x + ox) as usize] = v << 24 | c;
                    }
                }
            }
        };
        blit(&gly, gw, gh, 0, color(100, false, light));
        blit(&num, nw, nh, s - nw, color(r.percent, r.charging, light));
        icon(s, &px)
    }
}

// Hand-made 16 px art, using the full height the tray gives us (10 of 16 rows; the rest is
// Explorer's own padding look). Rows top to bottom, most significant used bit = leftmost pixel.
const ART_H: i32 = 10;

#[rustfmt::skip]
const DIGITS: [[u8; 10]; 11] = [
    [0b0110, 0b1001, 0b1001, 0b1001, 0b1001, 0b1001, 0b1001, 0b1001, 0b1001, 0b0110], // 0
    [0b0010, 0b0110, 0b1010, 0b0010, 0b0010, 0b0010, 0b0010, 0b0010, 0b0010, 0b0111], // 1
    [0b0110, 0b1001, 0b0001, 0b0001, 0b0010, 0b0010, 0b0100, 0b1000, 0b1000, 0b1111], // 2
    [0b1110, 0b0001, 0b0001, 0b0001, 0b0110, 0b0001, 0b0001, 0b0001, 0b0001, 0b1110], // 3
    [0b1001, 0b1001, 0b1001, 0b1001, 0b1111, 0b0001, 0b0001, 0b0001, 0b0001, 0b0001], // 4
    [0b1111, 0b1000, 0b1000, 0b1000, 0b1110, 0b0001, 0b0001, 0b0001, 0b0001, 0b1110], // 5
    [0b0110, 0b1000, 0b1000, 0b1000, 0b1110, 0b1001, 0b1001, 0b1001, 0b1001, 0b0110], // 6
    [0b1111, 0b0001, 0b0001, 0b0010, 0b0010, 0b0010, 0b0100, 0b0100, 0b0100, 0b0100], // 7
    [0b0110, 0b1001, 0b1001, 0b1001, 0b0110, 0b1001, 0b1001, 0b1001, 0b1001, 0b0110], // 8
    [0b0110, 0b1001, 0b1001, 0b1001, 0b1001, 0b0111, 0b0001, 0b0001, 0b0001, 0b0110], // 9
    [0b1111, 0b1000, 0b1000, 0b1000, 0b1110, 0b1000, 0b1000, 0b1000, 0b1000, 0b1000], // F
];

/// (width, rows) of each device glyph, at most ART_H rows; centered vertically.
#[rustfmt::skip]
fn glyph_art(kind: Kind) -> (i32, &'static [u8]) {
    match kind {
        Kind::Headset => (6, &[0b011110, 0b100001, 0b100001, 0b100001, 0b100001, 0b100001, 0b110011, 0b110011, 0b110011, 0b110011]),
        Kind::Mouse => (5, &[0b00100, 0b00100, 0b01110, 0b10101, 0b10101, 0b11111, 0b10001, 0b10001, 0b10001, 0b01110]), // cable + buttons
        Kind::Keyboard => (6, &[0b111111, 0b101011, 0b110101, 0b101011, 0b100001, 0b111111]),
        Kind::Gamepad => (6, &[0b011110, 0b101101, 0b111111, 0b110011, 0b100001]),
        Kind::Speaker => (6, &[0b000100, 0b001100, 0b111100, 0b111101, 0b111101, 0b111100, 0b001100, 0b000100]),
        Kind::Other => (5, &[0b00100, 0b00110, 0b10101, 0b01110, 0b00100, 0b01110, 0b10101, 0b00110, 0b00100]), // Bluetooth
    }
}

/// 16 px layout centered in an `s` px icon: glyph in columns 0..6, number right-aligned.
fn pixel_art(r: &Reading, light: bool, s: i32) -> Vec<u32> {
    let mut px = vec![0u32; (s * s) as usize];
    let pad = (s - 16) / 2;
    let top = (16 - ART_H) / 2;
    let mut put = |x0: i32, y0: i32, rows: &[u8], w: i32, c: u32| {
        for (y, bits) in rows.iter().enumerate() {
            for x in 0..w {
                if bits >> (w - 1 - x) & 1 == 1 {
                    px[((pad + y0 + y as i32) * s + pad + x0 + x) as usize] = 0xFF00_0000 | c;
                }
            }
        }
    };
    let (gw, g) = glyph_art(r.kind);
    put((6 - gw) / 2, top + (ART_H - g.len() as i32) / 2, g, gw, color(100, false, light));
    let text = label(r.percent);
    let c = color(r.percent, r.charging, light);
    let mut x = 16 - (text.len() as i32 * 5 - 1);
    for ch in text.chars() {
        let d = ch.to_digit(10).map_or(10, |d| d as usize);
        put(x, top, &DIGITS[d], 4, c);
        x += 5;
    }
    px
}

/// Rasterizes `text` and crops it to its ink. Font size: the largest at which `fit` stays within
/// `max_w` x `max_h`. Returns (alpha, width, height).
fn raster(text: &str, face: &str, fit: &str, max_w: i32, max_h: i32) -> (Vec<u8>, i32, i32) {
    unsafe {
        let scratch = max_h.max(max_w) * 3;
        let dc = CreateCompatibleDC(null_mut());
        let (bmp, bits) = dib(dc, scratch);
        let old_bmp = SelectObject(dc, bmp);
        SetTextColor(dc, 0xFFFFFF);
        SetBkMode(dc, TRANSPARENT as i32);
        let mut em = max_h * 3 / 2;
        while em > 5 && draw(dc, bits, scratch, em, face, fit).is_none_or(|(x0, y0, x1, y1)| x1 - x0 >= max_w || y1 - y0 >= max_h) {
            em -= 1;
        }
        let px = std::slice::from_raw_parts(bits, (scratch * scratch) as usize);
        let out = match draw(dc, bits, scratch, em, face, text) {
            Some((x0, y0, x1, y1)) => {
                let (w, h) = ((x1 - x0 + 1).min(max_w), (y1 - y0 + 1).min(max_h));
                let a = (0..h).flat_map(|y| (0..w).map(move |x| (y, x)))
                    .map(|(y, x)| (px[((y + y0) * scratch + x + x0) as usize] & 0xFF) as u8).collect();
                (a, w, h)
            }
            None => (Vec::new(), 0, 0),
        };
        SelectObject(dc, old_bmp);
        DeleteObject(bmp);
        DeleteDC(dc);
        out
    }
}

/// s x s straight-alpha BGRA pixels -> HICON.
unsafe fn icon(s: i32, px: &[u32]) -> HICON {
    unsafe {
        let dc = CreateCompatibleDC(null_mut());
        let (color_bmp, bits) = dib(dc, s);
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits, px.len());
        let mask_bytes = vec![0u8; ((s + 15) / 16 * 2 * s) as usize];
        let mask = CreateBitmap(s, s, 1, 1, mask_bytes.as_ptr() as _);
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color_bmp };
        let icon = CreateIconIndirect(&info);
        DeleteObject(mask);
        DeleteObject(color_bmp);
        DeleteDC(dc);
        icon
    }
}

/// Renders `text` at font size `em` into the cleared scratch bitmap; returns its ink bounds.
unsafe fn draw(dc: HDC, bits: *mut u32, scratch: i32, em: i32, face: &str, text: &str) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let font = CreateFontW(-em, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32, ANTIALIASED_QUALITY as u32, DEFAULT_PITCH as u32, w(face).as_ptr());
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
        assert_eq!(color(5, true, false), 0x6CCB5F); // charging: green, no alarm
        assert_eq!(color(5, true, true), 0x107C10);
    }

    fn r(id: &str, percent: u8, asleep: bool) -> Reading {
        Reading { id: id.into(), name: id.into(), percent, charging: false, kind: Kind::Mouse, asleep }
    }

    #[test]
    fn asleep_keeps_last_value() {
        let shown = vec![r("mouse", 93, false)];
        assert_eq!(merge(&shown, &[], vec![r("mouse", 0, true)]), vec![r("mouse", 93, false)]);
        assert_eq!(merge(&[], &[], vec![r("mouse", 0, true)]), vec![]); // never seen awake
        assert_eq!(merge(&[], &shown, vec![r("mouse", 0, true)]), vec![r("mouse", 93, false)]); // after restart
        assert_eq!(merge(&shown, &[], vec![]), vec![]); // gone = removed
        let charging_no_level = Reading { charging: true, ..r("mouse", 0, true) };
        assert_eq!(merge(&shown, &[], vec![charging_no_level]), vec![Reading { charging: true, ..r("mouse", 93, false) }]);
        assert_eq!(merge(&shown, &[], vec![r("b", 1, false), r("a", 2, false)]), vec![r("a", 2, false), r("b", 1, false)]);
    }

    #[test]
    fn asleep_wildcard_keeps_whole_receiver() {
        let shown = vec![r("logi:c547:01", 93, false), r("logi:c547:02", 40, false), r("ss:12e0", 80, false)];
        let new = vec![r("logi:c547:*", 0, true), r("ss:12e0", 79, false)];
        assert_eq!(merge(&shown, &[], new), vec![r("logi:c547:01", 93, false), r("logi:c547:02", 40, false), r("ss:12e0", 79, false)]);
    }

    #[test]
    fn low_battery_warns_once_per_discharge() {
        let at = |p: u8, charging: bool| Reading { charging, ..r("m", p, false) };
        assert_eq!(warn_step(false, &at(50, false)), (false, false));
        assert_eq!(warn_step(false, &at(10, false)), (true, true)); // reaches 10 %: warn
        assert_eq!(warn_step(true, &at(9, false)), (false, true)); // still low: quiet
        assert_eq!(warn_step(true, &at(12, false)), (false, true)); // jitter: no re-arm
        assert_eq!(warn_step(true, &at(8, true)), (false, false)); // charging: re-arm
        assert_eq!(warn_step(true, &at(16, false)), (false, false)); // above 15 %: re-arm
    }

    #[test]
    fn exe_path_matching() {
        let exe = r"c:\users\me\appdata\local\programs\wattray\wattray.exe";
        assert!(path_matches(exe, exe));
        assert!(path_matches(exe, r"{f1b32785-6fba-4fcf-9d55-7b8e7f157091}\programs\wattray\wattray.exe"));
        assert!(!path_matches(exe, r"d:\code\wattray\target\release\wattray.exe")); // dev build
        assert!(!path_matches(exe, r"c:\users\me\appdata\local\programs\pila\wattray.exe"));
        assert!(!path_matches(exe, ""));
    }

    #[test]
    fn cache_roundtrip() {
        let c = vec![r("logi:c547:01", 97, false), Reading { kind: Kind::Headset, ..r("ss:12e0", 88, false) }];
        assert_eq!(parse_cache(&format_cache(&c)), c);
        assert_eq!(parse_cache("garbage\nbad\tline\n"), vec![]);
    }

    #[test]
    fn render_does_not_leak_gdi() {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS, GR_USEROBJECTS};
        let count = || unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) + GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS) };
        unsafe { DestroyIcon(render(&r("x", 50, false), false)) }; // warm up caches
        let before = count();
        for p in 0..=100u8 {
            let icon = render(&r("x", p, false), p % 2 == 0);
            assert!(!icon.is_null());
            unsafe { DestroyIcon(icon) };
        }
        assert!(count() <= before + 2, "GDI/USER objects grew: {before} -> {}", count());
    }

    #[test]
    fn pixel_art_fits_every_value() {
        for size in [16, 20] {
            for p in 0..=100u8 {
                let px = pixel_art(&r("x", p, false), false, size);
                assert!(px.iter().any(|&v| v != 0));
            }
        }
        // glyph and number never overlap: column 6 (the gap) stays empty for "88"
        let px = pixel_art(&r("x", 88, false), false, 16);
        assert!((0..16).all(|y| px[y * 16 + 6] == 0));
    }

    #[test]
    fn uid_is_stable() {
        assert_eq!(uid("ss:12e0"), uid("ss:12e0"));
        assert_ne!(uid("ss:12e0"), uid("logi:c547:01"));
    }
}
