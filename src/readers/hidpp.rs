//! Logitech HID++ 2.0 over the "long" (0x11, 20 byte) vendor collection.
//! Works for Lightspeed / Unifying / Bolt receivers (slots 1..=6) and direct devices (0xFF).

use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicU8, Ordering};

use crate::hid::{self, Dev, HidInfo};
use crate::reading::{volt_to_pct, Kind, Reading};

static SW: AtomicU8 = AtomicU8::new(0);
// Generous: a sleeping link takes ~0.8 s to answer its first request. Only online devices get asked.
const TIMEOUT: Duration = Duration::from_millis(1500);

/// Software id for the next request, rotating 2..=15, so a late reply never matches a newer request
/// (getFeature replies do not echo which feature was asked for).
fn next_sw() -> u8 {
    2 + SW.fetch_add(1, Ordering::Relaxed) % 14
}

pub fn read(out: &mut Vec<Reading>) {
    let all = hid::enumerate(0x046D);
    for info in &all {
        if !matches!(info.usage_page, 0xFF00 | 0xFF43) || info.output_len != 20 {
            continue;
        }
        let Some(dev) = Dev::open(info) else { continue };
        let short = all.iter().find(|s| s.pid == info.pid && s.mi == info.mi && s.usage_page == 0xFF00 && s.output_len == 7);
        let (online, asleep) = match short.map(|s| receiver_slots(s).or_else(|| receiver_slots(s))) {
            None | Some(Some(Probe::Direct)) => (vec![0xFF], vec![]), // direct device
            Some(Some(Probe::Receiver(on, off))) => (on, off),
            Some(None) => {
                // Receiver did not answer twice: keep whatever we showed for it, re-check next poll.
                out.push(asleep_reading(format!("logi:{:04x}:*", info.pid)));
                continue;
            }
        };
        for idx in online {
            if let Some(r) = read_device(&dev, info.pid, idx) {
                out.push(r);
            }
        }
        for idx in asleep {
            out.push(asleep_reading(id(info.pid, idx)));
        }
    }
}

/// Asks a receiver (HID++ 1.0, short collection) to announce its devices: writing register 0x02
/// with 0x02 makes it send a 0x41 "device connection" notification per paired slot.
fn asleep_reading(id: String) -> Reading {
    Reading { id, name: String::new(), percent: 0, charging: false, kind: Kind::Other, asleep: true }
}

enum Probe {
    /// (slots with link up, paired slots asleep)
    Receiver(Vec<u8>, Vec<u8>),
    /// A HID++ 2.0 device that rejected the 1.0 register request.
    Direct,
}

/// None if nothing answered in time.
fn receiver_slots(short: &HidInfo) -> Option<Probe> {
    let dev = Dev::open(short)?;
    dev.write(&[0x10, 0xFF, 0x80, 0x02, 0x02, 0, 0]);
    let (mut online, mut asleep) = (Vec::new(), Vec::new());
    let mut acked = false;
    let mut deadline = Instant::now() + Duration::from_millis(300);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Some(r) = dev.read(left.as_millis().max(1) as u32) else { break };
        match r.as_slice() {
            [0x10, 0xFF, 0x8F, 0x80, 0x02, ..] => return Some(Probe::Direct),
            [0x10, 0xFF, 0x80, 0x02, ..] => {
                acked = true;
                deadline = deadline.min(Instant::now() + Duration::from_millis(50)); // stragglers
            }
            [0x10, idx @ 1..=6, 0x41, _, flags, ..] if !online.contains(idx) && !asleep.contains(idx) => {
                if flags & 0x40 == 0 { online.push(*idx) } else { asleep.push(*idx) }
            }
            _ => {}
        }
    }
    acked.then_some(Probe::Receiver(online, asleep))
}

fn read_device(dev: &Dev, pid: u16, idx: u8) -> Option<Reading> {
    let (percent, charging) = battery(dev, idx)?;
    let (name, kind) = describe(dev, idx).unwrap_or_else(|| ("Logitech".into(), Kind::Other));
    Some(Reading { id: id(pid, idx), name, percent, charging, kind, asleep: false })
}

fn id(pid: u16, idx: u8) -> String {
    format!("logi:{pid:04x}:{idx:02x}")
}

fn battery(dev: &Dev, idx: u8) -> Option<(u8, bool)> {
    if let Some(f) = feature(dev, idx, 0x1004) {
        let p = call(dev, idx, f, 1, &[])?;
        return Some((unified_pct(p[0], p[1]), matches!(p[2], 1 | 2)));
    }
    if let Some(f) = feature(dev, idx, 0x1000) {
        let p = call(dev, idx, f, 0, &[])?;
        return Some((p[0].min(100), matches!(p[2], 1 | 2 | 4)));
    }
    let f = feature(dev, idx, 0x1001)?;
    let p = call(dev, idx, f, 0, &[])?;
    Some((volt_to_pct(u16::from_be_bytes([p[0], p[1]])), p[2] & 0x80 != 0))
}

/// Name and type via feature 0x0005 (Device Name & Type).
fn describe(dev: &Dev, idx: u8) -> Option<(String, Kind)> {
    let f = feature(dev, idx, 0x0005)?;
    let kind = call(dev, idx, f, 2, &[]).map_or(Kind::Other, |p| match p[0] {
        0 | 2 => Kind::Keyboard,
        3..=5 => Kind::Mouse,
        8 => Kind::Headset,
        11 | 12 => Kind::Gamepad,
        14 => Kind::Speaker,
        _ => Kind::Other,
    });
    let len = call(dev, idx, f, 0, &[])?[0] as usize;
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        let p = call(dev, idx, f, 1, &[bytes.len() as u8])?;
        let take = (len - bytes.len()).min(16);
        bytes.extend_from_slice(&p[..take]);
    }
    let s = String::from_utf8_lossy(&bytes).trim_end_matches('\0').trim().to_string();
    (!s.is_empty()).then_some((s, kind))
}

/// Feature index for `id`, or None if the device lacks it.
fn feature(dev: &Dev, idx: u8, id: u16) -> Option<u8> {
    let p = call(dev, idx, 0, 0, &id.to_be_bytes())?;
    (p[0] != 0).then_some(p[0])
}

/// Sends one request and waits for its matching reply; returns the 16 parameter bytes.
fn call(dev: &Dev, idx: u8, feat: u8, func: u8, params: &[u8]) -> Option<[u8; 16]> {
    let sw = next_sw();
    let mut req = [0u8; 20];
    req[..4].copy_from_slice(&[0x11, idx, feat, (func << 4) | sw]);
    req[4..4 + params.len()].copy_from_slice(params);
    if !dev.write(&req) {
        return None;
    }
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let r = dev.read(left.as_millis().max(1) as u32)?;
        match parse(&r, idx, feat, (func << 4) | sw) {
            Reply::Ok(p) => return Some(p),
            Reply::Err => return None,
            Reply::Other => continue, // notification or someone else's reply
        }
    }
}

#[derive(Debug, PartialEq)]
enum Reply {
    Ok([u8; 16]),
    Err,
    Other,
}

fn parse(r: &[u8], idx: u8, feat: u8, fsw: u8) -> Reply {
    if r.len() < 20 || r[0] != 0x11 || r[1] != idx {
        return Reply::Other;
    }
    if r[2] == 0xFF && r[3] == feat && r[4] == fsw {
        return Reply::Err;
    }
    if r[2] == feat && r[3] == fsw {
        return Reply::Ok(r[4..20].try_into().unwrap());
    }
    Reply::Other
}

/// 0x1004: state of charge if reported, else the coarse level flags.
fn unified_pct(soc: u8, flags: u8) -> u8 {
    if soc != 0 {
        return soc.min(100);
    }
    match flags {
        f if f & 8 != 0 => 100,
        f if f & 4 != 0 => 50,
        f if f & 2 != 0 => 20,
        f if f & 1 != 0 => 5,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(head: [u8; 4], p0: u8) -> Vec<u8> {
        let mut v = vec![0u8; 20];
        v[..4].copy_from_slice(&head);
        v[4] = p0;
        v
    }

    #[test]
    fn parse_matches_only_our_reply() {
        let fsw = 0x1A;
        assert_eq!(parse(&frame([0x11, 1, 5, fsw], 93), 1, 5, fsw), Reply::Ok({
            let mut p = [0; 16];
            p[0] = 93;
            p
        }));
        assert_eq!(parse(&frame([0x11, 2, 5, fsw], 93), 1, 5, fsw), Reply::Other); // other slot
        assert_eq!(parse(&frame([0x11, 1, 5, 0x11], 93), 1, 5, fsw), Reply::Other); // stale reply (other sw id)
        assert_eq!(parse(&frame([0x11, 1, 0xFF, 5], fsw), 1, 5, fsw), Reply::Err);
        assert_eq!(parse(&[0x10, 1, 0x8F, 0, 0, 0, 0], 1, 5, fsw), Reply::Other);
    }

    #[test]
    fn unified_levels() {
        assert_eq!(unified_pct(93, 0), 93);
        assert_eq!(unified_pct(0, 0b0100), 50);
        assert_eq!(unified_pct(0, 0b1000), 100);
        assert_eq!(unified_pct(0, 0), 0);
    }
}
