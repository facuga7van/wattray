#![windows_subsystem = "windows"]

mod hid;
mod reading;
mod readers;

use std::sync::atomic::Ordering;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--once" || a == "--debug" || a == "--list" || a == "--probe") {
        unsafe { windows_sys::Win32::System::Console::AttachConsole(u32::MAX) };
        hid::DEBUG.store(args.iter().any(|a| a == "--debug" || a == "--probe"), Ordering::Relaxed);
        if args.iter().any(|a| a == "--list") { list(); return; }
        if args.iter().any(|a| a == "--probe") { probe(); return; }
        for r in readers::read_all() {
            println!("{:<28} {:>3}%{}", r.name, r.percent, if r.charging { "  cargando" } else { "" });
        }
    }
}
#[allow(dead_code)]
fn probe() {
    for i in hid::enumerate(0x046D) {
        if i.usage_page != 0xFF00 || i.output_len != 7 { continue; }
        let d = hid::Dev::open(&i).unwrap();
        d.write(&[0x10, 0xFF, 0x80, 0x02, 0x02, 0, 0]);
        while let Some(_) = d.read(1500) {}
    }
}
#[allow(dead_code)]
fn list() { for i in hid::enumerate(0x046D) { println!("{:04x} mi={:?} up={:04x} u={:04x} in={} out={} feat={} {}", i.pid, i.mi, i.usage_page, i.usage, i.input_len, i.output_len, i.feature_len, i.product); } }
