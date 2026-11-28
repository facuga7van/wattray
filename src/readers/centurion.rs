//! ASTRO A30 (codename Carbonite) through its USB transmitter. The headset speaks Logitech's
//! "Centurion" protocol (facts from the Logitech G app): a message is
//! `[feature index][function << 4 | sw id][params]`; battery is feature 0x0104, function 0, which
//! answers `[real %, shown %, charging state]`. The transmitter tunnels those messages on its
//! ff03:0001 collection, report 0xF2, inside a fixed header, as captured from a user's A30 with
//! `wattray --listen` (no public project documents it):
//!   f2 | 00 00 0c 1e 82 40 00 00 | f2 00 LL 00 00 00 00 | message      (LL = 1 + message length)
//! Battery changes also arrive unasked as events (sw id 0): `02 00 43 3c 02` = index 2, 67 %, 60 %, charging.

use std::sync::{Mutex, Once};
use std::thread;
use std::time::{Duration, Instant};

use crate::hid::{self, Dev, HidInfo};
use crate::reading::{Kind, Reading};

const MODELS: &[(u16, u16, &str)] = &[(0x9886, 0x0050, "ASTRO A30"), (0x9886, 0x004f, "ASTRO A30")];
const REPORT: u8 = 0xF2;
const OFFSET: usize = 16;
const BATTERY_INDEX: u8 = 2;

static LAST: Mutex<Option<(u16, u8, bool)>> = Mutex::new(None);
static LISTENER: Once = Once::new();

pub fn read(out: &mut Vec<Reading>) {
    let Some((pid, name)) = present() else { return };
    LISTENER.call_once(|| {
        thread::spawn(listen);
    });
    let id = format!("centurion:{pid:04x}");
    let reading = match *LAST.lock().unwrap_or_else(|e| e.into_inner()) {
        Some((p, percent, charging)) if p == pid => {
            Reading { id, name: name.into(), percent, charging, kind: Kind::Headset, asleep: false }
        }
        _ => Reading { id, name: name.into(), percent: 0, charging: false, kind: Kind::Headset, asleep: true },
    };
    out.push(reading);
}

fn tunnel(i: &HidInfo) -> bool {
    (i.usage_page, i.usage) == (0xff03, 0x0001) && i.input_len >= 64
}

fn present() -> Option<(u16, &'static str)> {
    MODELS
        .iter()
        .find_map(|&(vid, pid, name)| hid::enumerate(vid).iter().any(|i| i.pid == pid && tunnel(i)).then_some((pid, name)))
}

fn listen() {
    loop {
        let found = MODELS
            .iter()
            .find_map(|&(vid, pid, _)| hid::enumerate(vid).into_iter().find(|i| i.pid == pid && tunnel(i)));
        if let Some(info) = found {
            if let Some(dev) = Dev::open(&info) {
                pump(&dev, info.pid);
            }
        }
        thread::sleep(Duration::from_secs(5));
    }
}

fn pump(dev: &Dev, pid: u16) {
    let mut failures = 0;
    loop {
        let started = Instant::now();
        match dev.read(5000) {
            Some(r) => {
                failures = 0;
                let Some((percent, charging)) = message(&r).as_deref().and_then(battery_event) else { continue };
                let changed = {
                    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
                    let new = Some((pid, percent, charging));
                    let changed = *last != new;
                    *last = new;
                    changed
                };
                if changed {
                    crate::readers::rescan();
                }
            }
            None if started.elapsed() < Duration::from_millis(100) => {
                failures += 1;
                if failures > 3 {
                    return;
                }
                thread::sleep(Duration::from_millis(200));
            }
            None => {}
        }
    }
}

fn message(r: &[u8]) -> Option<Vec<u8>> {
    if r.len() < OFFSET || r[0] != REPORT || r[9] != REPORT || r[10] != 0 {
        return None;
    }
    let len = r[11] as usize;
    (len >= 2 && r.len() >= OFFSET + len - 1).then(|| r[OFFSET..OFFSET + len - 1].to_vec())
}

fn battery_event(m: &[u8]) -> Option<(u8, bool)> {
    let &[BATTERY_INDEX, fsw, _, shown, state, ..] = m else { return None };
    (fsw & 0x0F == 0 && shown <= 100).then_some((shown, matches!(state, 1 | 2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split(' ').map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn captured_reports() {
        let battery = hex("f2 00 00 0c 1e 82 40 00 00 f2 00 06 00 00 00 00 02 00 43 3c 02 00 00");
        assert_eq!(message(&battery), Some(vec![0x02, 0x00, 0x43, 0x3c, 0x02]));
        let other = hex("f2 00 00 0c 1e 82 40 00 00 f2 00 08 00 00 00 00 09 10 02 00 00 01 01 00");
        assert_eq!(message(&other), Some(vec![0x09, 0x10, 0x02, 0x00, 0x00, 0x01, 0x01]));
        assert_eq!(message(&battery[..19]), None);
        assert_eq!(message(&hex("f0 00 41 00 53 00 54 00 52 00 4f 00 20 00 00 00 00")), None);
    }

    #[test]
    fn battery_events() {
        assert_eq!(battery_event(&[0x02, 0x00, 0x43, 0x3c, 0x02]), Some((60, true)));
        assert_eq!(battery_event(&[0x02, 0x00, 0x50, 0x50, 0x00]), Some((80, false)));
        assert_eq!(battery_event(&[0x02, 0x01, 0x43, 0x3c, 0x02]), None);
        assert_eq!(battery_event(&[0x09, 0x00, 0x02, 0x00, 0x00, 0x01, 0x00]), None);
        assert_eq!(battery_event(&[0x02, 0x00, 0x43, 0xc8, 0x00]), None);
        assert_eq!(battery_event(&[0x02, 0x00, 0x43]), None);
    }
}
