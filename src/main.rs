#![windows_subsystem = "windows"]

mod hid;
mod listen;
mod reading;
mod readers;
mod tray;

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::transmute;
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;
use std::thread::{self, Thread};
use std::time::Duration;

use windows_sys::core::{BOOL, GUID, HRESULT};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
use windows_sys::Win32::System::Console::{AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegDeleteTreeW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Shell::NIN_SELECT;

const NIN_KEYSELECT: u32 = NIN_SELECT | 1; // NINF_KEY
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use reading::Reading;
use tray::{Tray, WM_TRAY};

const WM_READINGS: u32 = WM_APP;
const POLL: Duration = Duration::from_secs(120);
/// While a device sleeps, look again sooner so it shows up soon after it wakes.
const POLL_ASLEEP: Duration = Duration::from_secs(20);
const RESCAN_TIMER: usize = 1;
const PROMOTE_TIMER: usize = 2;
const CMD_EXIT: usize = 1;

struct App {
    tray: Tray,
    worker: Thread,
    taskbar_created: u32,
    promote_tries: u32,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app state. None if the state is busy (re-entrant message during a modal loop).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

pub fn spanish() -> bool {
    unsafe { GetUserDefaultUILanguage() & 0x3FF == 0x0A } // LANG_SPANISH
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(k) = args.iter().position(|a| a == "--listen") {
        console();
        listen::run(args.get(k + 1).map(String::as_str), args.get(k + 2).map(String::as_str));
        return;
    }
    if args.iter().any(|a| a == "--once" || a == "--debug") {
        once(args.iter().any(|a| a == "--debug"));
        return;
    }
    if args.iter().any(|a| a == "--uninstall") {
        uninstall();
        return;
    }
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let name = w("Local\\wattray"); // must outlive the call: GetLastError is read right after
        CreateMutexW(null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        autostart();

        let hinst = GetModuleHandleW(null());
        let class = w("wattray");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        // Hidden top-level (not message-only) so it receives WM_DEVICECHANGE and TaskbarCreated broadcasts.
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), class.as_ptr(), WS_POPUP, 0, 0, 0, 0,
            null_mut(), null_mut(), hinst, null());
        if hwnd.is_null() {
            return;
        }

        let target = hwnd as usize; // HWND is not Send; the handle value is
        let worker = thread::spawn(move || loop {
            let readings = readers::read_all();
            let wait = if readings.iter().any(|r| r.asleep) { POLL_ASLEEP } else { POLL };
            let batch = Box::into_raw(Box::new(readings));
            if PostMessageW(target as HWND, WM_READINGS, 0, batch as LPARAM) == 0 {
                drop(Box::from_raw(batch));
            }
            thread::park_timeout(wait); // unpark() = rescan now
        });
        let _ = readers::RESCAN.set(worker.thread().clone());
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                tray: Tray::new(hwnd),
                worker: worker.thread().clone(),
                taskbar_created: RegisterWindowMessageW(w("TaskbarCreated").as_ptr()),
                promote_tries: 0,
            })
        });

        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
}

/// Prints to the terminal that started us. `> file` already gave us a stdout: attaching would replace it.
fn console() {
    unsafe {
        if GetStdHandle(STD_OUTPUT_HANDLE).is_null() {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// `--once` / `--debug`: print what every reader sees and exit.
fn once(debug: bool) {
    console();
    hid::DEBUG.store(debug, Ordering::Relaxed);
    let all = readers::read_all();
    if all.is_empty() {
        println!("no devices found");
    }
    for r in all {
        if r.asleep {
            println!("{:<28} asleep", r.id);
            continue;
        }
        println!("{:<28} {:>3}%{}", r.name, r.percent, if r.charging { "  charging" } else { "" });
    }
    if debug {
        // Every HID interface, recognized or not: tells a new device's protocol apart from a standard one.
        println!("
HID interfaces (vid:pid  interface  usage page:usage  input/output/feature bytes  name):");
        for i in hid::enumerate_all() {
            let mi = i.mi.map_or("--".into(), |m| format!("mi{m}"));
            println!("  {:04x}:{:04x}  {mi:<4}  {:04x}:{:04x}  {}/{}/{}  {}", i.vid, i.pid, i.usage_page, i.usage,
                i.input_len, i.output_len, i.feature_len, i.product);
        }
    }
}

/// HKCU Run entry. Rewritten on every start so a moved exe keeps working;
/// disabling it from Task Manager → Startup apps is respected (that lives in a separate key).
fn autostart() {
    let mut path = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(null_mut(), path.as_mut_ptr(), path.len() as u32) } as usize;
    if n == 0 || n >= path.len() {
        return;
    }
    let value = w(&format!("\"{}\"", String::from_utf16_lossy(&path[..n])));
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, w("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
            w("wattray").as_ptr(), REG_SZ, value.as_ptr() as _, (value.len() * 2) as u32);
        // 0.1 was called "pila": drop its startup entry so it does not launch too.
        RegDeleteKeyValueW(HKEY_CURRENT_USER, w("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(), w("pila").as_ptr());
    }
    start_menu_shortcut(&path[..=n]); // buffer is zeroed: [..=n] keeps the terminator
}

/// Start menu shortcut, so Windows search finds wattray again after Exit.
/// Rewritten on every start for the same reason as the Run entry.
fn start_menu_shortcut(exe: &[u16]) {
    // windows-sys has no COM interfaces: call IShellLinkW / IPersistFile through their vtables.
    const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
    const IID_SHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
    const IID_PERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
    type QueryInterface = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
    type Release = unsafe extern "system" fn(*mut c_void) -> u32;
    type SetPath = unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT;
    type Save = unsafe extern "system" fn(*mut c_void, *const u16, BOOL) -> HRESULT;
    let slot = |obj: *mut c_void, i: usize| unsafe { *(*(obj as *const *const usize)).add(i) };

    let Some(lnk) = shortcut_path() else { return };
    let lnk = w(&lnk);
    unsafe {
        CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
        let mut link = null_mut();
        if CoCreateInstance(&CLSID_SHELL_LINK, null_mut(), CLSCTX_INPROC_SERVER, &IID_SHELL_LINK_W, &mut link) < 0 {
            return;
        }
        transmute::<usize, SetPath>(slot(link, 20))(link, exe.as_ptr());
        let mut file = null_mut();
        if transmute::<usize, QueryInterface>(slot(link, 0))(link, &IID_PERSIST_FILE, &mut file) >= 0 {
            transmute::<usize, Save>(slot(file, 6))(file, lnk.as_ptr(), 1);
            transmute::<usize, Release>(slot(file, 2))(file);
        }
        transmute::<usize, Release>(slot(link, 2))(link);
    }
}

// ponytail: %APPDATA% instead of SHGetKnownFolderPath; only differs with roaming-profile redirection.
fn shortcut_path() -> Option<String> {
    Some(std::env::var("APPDATA").ok()? + "\\Microsoft\\Windows\\Start Menu\\Programs\\wattray.lnk")
}

/// `--uninstall`: close the running instance and remove everything wattray wrote. The exe stays:
/// a running process cannot delete itself.
fn uninstall() {
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
        let hwnd = FindWindowW(w("wattray").as_ptr(), null());
        if !hwnd.is_null() {
            PostMessageW(hwnd, WM_CLOSE, 0, 0); // DefWindowProc destroys it; WM_DESTROY removes the icons
        }
        let run = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
        let approved = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
        for key in [run, approved] {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, w(key).as_ptr(), w("wattray").as_ptr());
        }
        RegDeleteTreeW(HKEY_CURRENT_USER, w("Software\\wattray").as_ptr());
    }
    if let Some(lnk) = shortcut_path() {
        let _ = std::fs::remove_file(lnk);
    }
    println!("wattray removed. Delete wattray.exe to finish.");
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_READINGS => {
                let batch = lp as *mut Vec<Reading>;
                match with_app(|a| {
                    a.tray.update(*Box::from_raw(batch));
                    a.promote_tries = 0;
                    a.tray.unsettled()
                }) {
                    None => {
                        PostMessageW(hwnd, msg, wp, lp); // state busy: retry after the modal loop
                    }
                    Some(true) => {
                        SetTimer(hwnd, PROMOTE_TIMER, 1000, None); // shell registers new icons lazily
                    }
                    Some(false) => {}
                }
            }
            WM_TRAY => match (lp & 0xFFFF) as u32 {
                WM_CONTEXTMENU => menu(hwnd),
                NIN_SELECT | NIN_KEYSELECT => rescan(),
                _ => {}
            },
            WM_POWERBROADCAST if wp == PBT_APMRESUMEAUTOMATIC as usize => {
                SetTimer(hwnd, RESCAN_TIMER, 5000, None); // back from sleep: give radios time to relink
            }
            WM_DEVICECHANGE => {
                SetTimer(hwnd, RESCAN_TIMER, 2000, None); // debounce bursts of device events
            }
            WM_TIMER if wp == PROMOTE_TIMER => {
                let again = with_app(|a| {
                    a.tray.promote();
                    a.promote_tries += 1;
                    a.tray.unsettled() && a.promote_tries < 30
                });
                if again != Some(true) {
                    KillTimer(hwnd, PROMOTE_TIMER);
                }
            }
            WM_TIMER if wp == RESCAN_TIMER => {
                KillTimer(hwnd, RESCAN_TIMER);
                rescan();
            }
            WM_SETTINGCHANGE if lp != 0 && wide_eq(lp as *const u16, "ImmersiveColorSet") => {
                with_app(|a| a.tray.redraw_all());
            }
            WM_DESTROY => {
                with_app(|a| a.tray.clear());
                PostQuitMessage(0);
            }
            _ if with_app(|a| a.taskbar_created == msg).unwrap_or(false) => {
                with_app(|a| a.tray.readd_all());
            }
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
        0
    }
}

fn rescan() {
    with_app(|a| a.worker.unpark());
}

unsafe fn menu(hwnd: HWND) {
    unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, CMD_EXIT, w(if spanish() { "Salir" } else { "Exit" }).as_ptr());
        let mut pt = std::mem::zeroed();
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd); // otherwise the menu does not close when clicking elsewhere
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, null());
        DestroyMenu(m);
        if cmd as usize == CMD_EXIT {
            DestroyWindow(hwnd);
        }
    }
}

unsafe fn wide_eq(p: *const u16, s: &str) -> bool {
    unsafe { s.encode_utf16().chain([0]).enumerate().all(|(i, c)| *p.add(i) == c) }
}
