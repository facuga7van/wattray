#![windows_subsystem = "windows"]

mod hid;
mod reading;
mod readers;
mod tray;

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;
use std::thread::{self, Thread};
use std::time::Duration;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Shell::NIN_SELECT;

const NIN_KEYSELECT: u32 = NIN_SELECT | 1; // NINF_KEY
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use reading::Reading;
use tray::{Tray, WM_TRAY};

const WM_READINGS: u32 = WM_APP;
const POLL: Duration = Duration::from_secs(120);
const RESCAN_TIMER: usize = 1;
const CMD_EXIT: usize = 1;

struct App {
    tray: Tray,
    worker: Thread,
    taskbar_created: u32,
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
    if args.iter().any(|a| a == "--once" || a == "--debug") {
        once(args.iter().any(|a| a == "--debug"));
        return;
    }
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CreateMutexW(null(), 0, w("Local\\pila").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        autostart();

        let hinst = GetModuleHandleW(null());
        let class = w("pila");
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
            let batch = Box::into_raw(Box::new(readers::read_all()));
            if PostMessageW(target as HWND, WM_READINGS, 0, batch as LPARAM) == 0 {
                drop(Box::from_raw(batch));
            }
            thread::park_timeout(POLL); // unpark() = rescan now
        });
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                tray: Tray::new(hwnd),
                worker: worker.thread().clone(),
                taskbar_created: RegisterWindowMessageW(w("TaskbarCreated").as_ptr()),
            })
        });

        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
}

/// `--once` / `--debug`: print what every reader sees and exit.
fn once(debug: bool) {
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
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
            w("pila").as_ptr(), REG_SZ, value.as_ptr() as _, (value.len() * 2) as u32);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_READINGS => {
                let batch = lp as *mut Vec<Reading>;
                if with_app(|a| a.tray.update(*Box::from_raw(batch))).is_none() {
                    PostMessageW(hwnd, msg, wp, lp); // state busy: retry after the modal loop
                }
            }
            WM_TRAY => match (lp & 0xFFFF) as u32 {
                WM_CONTEXTMENU => menu(hwnd),
                NIN_SELECT | NIN_KEYSELECT => rescan(),
                _ => {}
            },
            WM_DEVICECHANGE => {
                SetTimer(hwnd, RESCAN_TIMER, 2000, None); // debounce bursts of device events
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
