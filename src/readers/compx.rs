//! CompX-chip wireless mice: Pulsar, ATK/VXE, LAMZU, VGN, Scyrox, Attack Shark, Redragon, Hitscan,
//! Zaopin, Cougar, Teevolution, G-Wolves... They share one vendor frame on the HID collection
//! usage page 0xFF02, usage 0x0002 (17 bytes, report id 0x08):
//!
//!   request  08 <cmd> 00 .. 00 <chk>      cmd 3 = is the mouse online, cmd 4 = power details
//!   reply    08 <cmd> 00 00 00 <len> <b6> <b7> <mv hi> <mv lo> .. <chk>
//!   cmd 4:   b6 = level 0..100, b7 = 0 battery / 1 charging / 2 full on cable, bytes 8-9 = millivolts
//!   cmd 3:   b6 = 1 online, 0 offline
//!   chk      = 0x55 - sum(bytes 0..15), so the whole frame sums to 0x55
//!
//! Sources: HeyOkay/HaloBattery (MIT; providers/pulsar.py, compx.py, lamzu.py, docs/protocols.md:
//! frame, offsets, checksum, PIDs, hardware-confirmed on R1 Pro Max, X2 CrazyLight, Atlantis V2,
//! R11 Ultra, Redragon M991, VGN Dragonfly, X1 Pro, G-Wolves), lavandiI/lamzu-battery (MIT,
//! src/protocol/compx.rs: same frame, byte 7 = 0/1/2), late-autumn-gif/MouseDock (MIT: device
//! tables, Scyrox/VGN online check then power). Not verified on hardware by wattray.
//!
//! A sleeping mouse either leaves the receiver silent or (MouseDock/Scyrox) answers cmd 3 with
//! "offline"; both give no Reading. Only ids from the table below, only on the exact collection.

use crate::hid::{self, Dev, HidInfo};
use crate::reading::{Kind, Reading};

const USAGE: (u16, u16) = (0xFF02, 0x0002);
const LEN: usize = 17;
const VIDS: [u16; 6] = [0x3554, 0x373B, 0x3710, 0x3770, 0x33E4, 0x35BB];

struct Model {
    vid: u16,
    pid: u16,
    name: &'static str,
    /// Id (vid, pid) of the receiver: a cable shares its mouse's icon with its receiver.
    key: (u16, u16),
}

const fn rx(vid: u16, pid: u16, name: &'static str) -> Model {
    Model { vid, pid, name, key: (vid, pid) }
}

const fn cable(vid: u16, pid: u16, name: &'static str, key: (u16, u16)) -> Model {
    Model { vid, pid, name, key }
}

// ponytail: names follow the receiver id, which some brands share between models.
#[rustfmt::skip]
const MODELS: &[Model] = &[
    rx(0x3554, 0xF508, "Pulsar X2 V2 Mini"), cable(0x3554, 0xF507, "Pulsar X2 V2 Mini", (0x3554, 0xF508)),
    rx(0x3710, 0x5406, "Pulsar X2 CrazyLight"), cable(0x3710, 0x3414, "Pulsar X2 CrazyLight", (0x3710, 0x5406)),
    cable(0x3710, 0x3510, "Pulsar X2 CrazyLight", (0x3710, 0x5406)),
    rx(0x3554, 0xF58A, "VXE R1 Pro Max"), cable(0x3554, 0xF58C, "VXE R1 Pro Max", (0x3554, 0xF58A)),
    cable(0x373B, 0x1273, "VXE R1 Pro Max", (0x3554, 0xF58A)),
    rx(0x373B, 0x1085, "ATK VXE R1 SE+"), cable(0x3554, 0xF58F, "ATK VXE R1 SE+", (0x373B, 0x1085)),
    rx(0x3554, 0xF58E, "ATK VXE R1 SE+"),
    rx(0x373B, 0x11D9, "ATK A9 Ultimate"), cable(0x373B, 0x11B6, "ATK A9 Ultimate", (0x373B, 0x11D9)),
    rx(0x373B, 0x10C9, "ATK A9 SE"), rx(0x373B, 0x101A, "ATK X1 Pro"), rx(0x373B, 0x101B, "ATK Fire X 8K"),
    rx(0x373B, 0x12E9, "ATK mouse"), rx(0x373B, 0x11D8, "ATK Zero"), rx(0x373B, 0x1278, "ATK F1 V3 Ultimate+"),
    rx(0x373B, 0x1031, "ATK F1 Ultimate"), rx(0x373B, 0x102E, "ATK F1 Ultimate"),
    rx(0x3554, 0xF505, "VGN Dragonfly 4K"), cable(0x3554, 0xF506, "VGN Dragonfly 4K", (0x3554, 0xF505)),
    rx(0x3554, 0xF503, "VGN F1 MOBA"), rx(0x3554, 0xF502, "VGN F1 Pro"),
    rx(0x3554, 0xFB3E, "VGN F2 Pro Max"), rx(0x3554, 0xFB3D, "VGN F2 Pro Max"),
    rx(0x3554, 0xF50D, "LAMZU Atlantis"), rx(0x3554, 0xF510, "LAMZU Thorn"),
    cable(0x3554, 0xF50F, "LAMZU Thorn", (0x3554, 0xF510)), // Atlantis Pro cable too: see resolve()
    rx(0x3554, 0xFB44, "Attack Shark R11 Ultra"),
    rx(0x3554, 0xFB08, "IO Aurora"), cable(0x3554, 0xFB09, "IO Aurora", (0x3554, 0xFB08)),
    rx(0x3554, 0xF5F7, "Scyrox V6/V8"), cable(0x3554, 0xF5F6, "Scyrox V6/V8", (0x3554, 0xF5F7)),
    rx(0x3554, 0xF5DF, "Cougar Revenger Pro"), cable(0x3554, 0xF5DE, "Cougar Revenger Pro", (0x3554, 0xF5DF)),
    rx(0x3554, 0xF5DC, "Cougar Revenger Pro"),
    rx(0x3554, 0xF5D5, "Redragon M991 / K1NG Max"), cable(0x3554, 0xF55E, "Redragon M991 / K1NG Max", (0x3554, 0xF5D5)),
    rx(0x3554, 0xF55D, "Redragon M916 Pro / M913"),
    rx(0x3554, 0xF520, "Teevolution Terra Pro"), rx(0x3554, 0xF523, "Teevolution Terra Pro"),
    rx(0x3554, 0xF527, "Zaopin Z2 4K"), cable(0x3554, 0xF526, "Zaopin Z2 4K", (0x3554, 0xF527)),
    rx(0x3554, 0xF517, "EWEADN Z7"),
    rx(0x3770, 0x0300, "Hitscan Hyperlight"), rx(0x3770, 0x0200, "Hitscan Hyperlight"),
    rx(0x3770, 0x0100, "Hitscan Hyperlight"),
    rx(0x33E4, 0x3854, "G-Wolves mouse"),
    rx(0x35BB, 0xD3FA, "HECATE mouse"), rx(0x35BB, 0xD3FC, "HECATE mouse"), rx(0x35BB, 0xD3FD, "HECATE mouse"),
];

pub fn read(out: &mut Vec<Reading>) {
    let mut tried: Vec<(u16, u16)> = Vec::new();
    for vid in VIDS {
        for info in hid::enumerate(vid) {
            let Some(m) = MODELS.iter().find(|m| (m.vid, m.pid) == (info.vid, info.pid)) else { continue };
            // Exact collection that can carry the 17-byte frame; one attempt per product id
            // (bounds the wait when the mouse sleeps).
            if (info.usage_page, info.usage) != USAGE || info.output_len as usize != LEN || info.input_len as usize != LEN
                || tried.contains(&(info.vid, info.pid))
            {
                continue;
            }
            tried.push((info.vid, info.pid));
            let Some((percent, charging)) = Dev::open(&info).and_then(|d| poll(&d)) else { continue };
            let (key, name) = resolve(m, &info);
            let id = format!("compx:{:04x}:{:04x}", key.0, key.1);
            let r = Reading { id: id.clone(), name: name.into(), percent, charging, kind: Kind::Mouse, asleep: false };
            match out.iter_mut().find(|o| o.id == id) {
                Some(o) if r.charging && !o.charging => *o = r, // receiver and cable both answer: charging wins
                Some(_) => {}
                None => out.push(r),
            }
        }
    }
}

/// 0xF50F is the cable of both the Thorn and the Atlantis Pro; only the product string tells.
fn resolve(m: &Model, info: &HidInfo) -> ((u16, u16), &'static str) {
    if m.pid == 0xF50F && info.product.to_ascii_lowercase().contains("atlantis") {
        ((0x3554, 0xF50D), "LAMZU Atlantis")
    } else {
        (m.key, m.name)
    }
}

/// Online check first (a receiver can answer from a cache while its mouse is off), then power.
fn poll(dev: &Dev) -> Option<(u8, bool)> {
    hid::flush(dev);
    let online = hid::query(dev, &request(3), 250, |r| frame_ok(r, 3)).and_then(|r| online(&r));
    // ponytail: a silent or unparseable online reply still tries power (hardware-confirmed
    // devices were only ever asked cmd 4); an explicit "offline" is the only thing that skips it.
    if online == Some(false) {
        return None;
    }
    hid::query(dev, &request(4), 500, |r| frame_ok(r, 4)).and_then(|r| parse(&r))
}

fn request(cmd: u8) -> [u8; LEN] {
    let mut f = [0u8; LEN];
    f[0] = 0x08;
    f[1] = cmd;
    f[LEN - 1] = 0x55u8.wrapping_sub(f[..LEN - 1].iter().fold(0, |s, b| s.wrapping_add(*b)));
    f
}

/// Header, command echo, zero status and padding, and checksum all check out (events use cmd 0x0a).
fn frame_ok(r: &[u8], cmd: u8) -> bool {
    r.len() >= LEN
        && r[0] == 0x08
        && r[1] == cmd
        && r[2..5] == [0, 0, 0]
        && r[LEN - 1] == 0x55u8.wrapping_sub(r[..LEN - 1].iter().fold(0, |s, b| s.wrapping_add(*b)))
}

fn online(r: &[u8]) -> Option<bool> {
    match frame_ok(r, 3).then(|| r[6])? {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}

/// Percent and on-cable flag of a valid power reply; None for anything else.
fn parse(r: &[u8]) -> Option<(u8, bool)> {
    if !frame_ok(r, 4) || r[6] > 100 || r[7] > 2 {
        return None;
    }
    let charging = r[7] != 0;
    // ponytail: 0 % on battery is how a cold/asleep receiver can look; show nothing instead.
    (r[6] > 0 || charging).then_some((r[6], charging))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(head: &[u8], chk: u8) -> Vec<u8> {
        let mut f = head.to_vec();
        f.resize(LEN - 1, 0);
        f.push(chk);
        f
    }

    #[test]
    fn request_frames() {
        // 08 04 00..00 49 is the literal request in lamzu-battery and MouseDock.
        assert_eq!(request(4)[..2], [0x08, 0x04]);
        assert_eq!(request(4)[16], 0x49);
        assert_eq!(request(3)[16], 0x4a);
        assert!(request(4)[2..16].iter().all(|&b| b == 0));
    }

    #[test]
    fn g_wolves_reply() {
        // HaloBattery docs (owner probe, G-Wolves 33e4:3854): 80 %, battery, 4013 mV.
        let r = frame(&[8, 4, 0, 0, 0, 2, 0x50, 0, 0x0f, 0xad], 0x3b);
        assert_eq!(parse(&r), Some((80, false)));
    }

    #[test]
    fn r1_pro_max_cable_reply() {
        // HaloBattery pulsar.py docstring (R1 Pro Max cable 373b:1273): 40 %, on cable, 3783 mV.
        let r = frame(&[8, 4, 0, 0, 0, 2, 0x28, 1, 0x0e, 0xc7], 0x49);
        assert_eq!(parse(&r), Some((40, true)));
        assert!(frame_ok(&r, 4) && !frame_ok(&r, 3));
    }

    #[test]
    fn full_on_cable_and_trailing_bytes() {
        // Level 100 with byte 7 = 2 (full, lamzu-battery); a longer buffer is fine.
        let mut r = frame(&[8, 4, 0, 0, 0, 2, 100, 2, 0x10, 0x73], 0);
        r[16] = 0x55u8.wrapping_sub(r[..16].iter().fold(0, |s, b| s.wrapping_add(*b)));
        r.push(0xee);
        assert_eq!(parse(&r), Some((100, true)));
    }

    #[test]
    fn rejects_invalid() {
        let good = frame(&[8, 4, 0, 0, 0, 2, 0x50, 0, 0x0f, 0xad], 0x3b);
        let mut bad = good.clone();
        bad[16] ^= 1; // checksum
        assert_eq!(parse(&bad), None);
        assert_eq!(parse(&good[..16]), None); // short
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&frame(&[8, 0x0a, 0, 0, 0, 2, 0x50, 0], 0x3b)), None); // wrong command
        let fix = |mut r: Vec<u8>| {
            r[16] = 0x55u8.wrapping_sub(r[..16].iter().fold(0, |s, b| s.wrapping_add(*b)));
            r
        };
        let mut r = good.clone();
        r[6] = 101; // level out of range
        assert_eq!(parse(&fix(r)), None);
        let mut r = good.clone();
        r[7] = 4; // unknown power state
        assert_eq!(parse(&fix(r.clone())), None);
        r[7] = 3; // not documented by any source
        assert_eq!(parse(&fix(r)), None);
        let mut r = good.clone();
        r[0] = 9; // header
        assert_eq!(parse(&fix(r)), None);
        let mut r = good.clone();
        r[2] = 1; // non-zero status
        assert_eq!(parse(&fix(r)), None);
        let mut r = good.clone();
        r[6] = 0; // 0 % on battery
        assert_eq!(parse(&fix(r.clone())), None);
        r[7] = 1; // ... but 0 % on the cable is a real reading
        assert_eq!(parse(&fix(r)), Some((0, true)));
    }

    #[test]
    fn online_reply() {
        // Shape from iRocks M31R captures in HaloBattery docs (09 03 .. 01 .. 47 online,
        // .. 00 .. 48 offline), re-checksummed for header 08.
        assert_eq!(online(&frame(&[8, 3, 0, 0, 0, 1, 1], 0x48)), Some(true));
        assert_eq!(online(&frame(&[8, 3, 0, 0, 0, 1, 0], 0x49)), Some(false));
        assert_eq!(online(&frame(&[8, 3, 0, 0, 0, 1, 2], 0x47)), None);
        assert_eq!(online(&frame(&[8, 3, 0, 0, 0, 1, 1], 0x47)), None); // checksum
        assert_eq!(online(&frame(&[8, 4, 0, 0, 0, 1, 1], 0x47)), None); // other command
    }

    #[test]
    fn table_has_no_duplicate_ids() {
        for (i, m) in MODELS.iter().enumerate() {
            assert!(MODELS[i + 1..].iter().all(|n| (n.vid, n.pid) != (m.vid, m.pid)));
            assert!(VIDS.contains(&m.vid));
        }
    }
}
