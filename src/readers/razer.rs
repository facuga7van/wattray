//! Razer mice/keyboards: 90-byte feature report protocol (as documented by OpenRazer).
//! Not verified on hardware.

use std::thread::sleep;
use std::time::Duration;

use crate::hid::{self, Dev};
use crate::reading::{scale, Kind, Reading};

const TIDS: [u8; 4] = [0x1F, 0x3F, 0xFF, 0x9F]; // transaction ids used by different generations
const OK: u8 = 0x02;
const BUSY: u8 = 0x01;

pub fn read(out: &mut Vec<Reading>) {
    let mut done: Vec<u16> = Vec::new();
    for info in hid::enumerate(0x1532) {
        if info.feature_len != 91 || done.contains(&info.pid) {
            continue;
        }
        let Some(dev) = Dev::open(&info) else { continue };
        let Some((tid, raw)) = TIDS.iter().find_map(|&t| Some((t, command(&dev, t, 0x80)?))) else { continue };
        done.push(info.pid);
        if raw == 0 {
            continue; // asleep/off devices (and wired ones) answer OK with 0: not a real reading
        }
        let charging = command(&dev, tid, 0x84).is_some_and(|v| v == 1);
        let name = info.product.trim_end_matches(" Dongle").trim();
        out.push(Reading {
            id: format!("razer:{:04x}", info.pid),
            name: if name.is_empty() { "Razer".into() } else { name.into() },
            percent: scale(raw, 0, 255),
            charging,
            kind: match Kind::guess(name) {
                Kind::Other => Kind::Mouse, // most Razer battery devices are mice
                k => k,
            },
            asleep: false,
        });
    }
}

/// Sends power-class (0x07) command `id`; returns args[1] of a successful reply.
fn command(dev: &Dev, tid: u8, id: u8) -> Option<u8> {
    let req = report(tid, 0x07, id);
    let mut buf = [0u8; 91]; // report id 0 + 90-byte report
    buf[1..].copy_from_slice(&req);
    if !dev.set_feature(&buf) {
        return None;
    }
    for _ in 0..5 {
        sleep(Duration::from_millis(15));
        let r = dev.get_feature(0)?;
        match parse(&r[1..], 0x07, id) {
            Some(Ok(v)) => return Some(v),
            Some(Err(BUSY)) => continue,
            _ => return None,
        }
    }
    None
}

fn report(tid: u8, class: u8, id: u8) -> [u8; 90] {
    let mut r = [0u8; 90];
    r[1] = tid;
    r[5] = 0x02; // data size
    r[6] = class;
    r[7] = id;
    r[88] = r[2..88].iter().fold(0, |c, b| c ^ b);
    r
}

/// Some(Ok(args[1])) on success, Some(Err(status)) on a reply to us with another status.
fn parse(r: &[u8], class: u8, id: u8) -> Option<Result<u8, u8>> {
    if r.len() < 90 || r[6] != class || r[7] != id {
        return None;
    }
    Some(if r[0] == OK { Ok(r[9]) } else { Err(r[0]) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_layout_and_crc() {
        let r = report(0x1F, 0x07, 0x80);
        assert_eq!(&r[..8], &[0, 0x1F, 0, 0, 0, 0x02, 0x07, 0x80]);
        assert_eq!(r[88], 0x02 ^ 0x07 ^ 0x80);
    }

    #[test]
    fn parse_reply() {
        let mut r = report(0x1F, 0x07, 0x80);
        r[0] = OK;
        r[9] = 255;
        assert_eq!(parse(&r, 0x07, 0x80), Some(Ok(255)));
        r[0] = BUSY;
        assert_eq!(parse(&r, 0x07, 0x80), Some(Err(BUSY)));
        assert_eq!(parse(&r, 0x07, 0x84), None);
    }
}
