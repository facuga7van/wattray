//! Corsair headsets and wireless mice. Headset protocol data from HeadsetControl
//! (lib/devices/corsair_*.hpp); offsets count from the report id byte, as hid::query returns them.
//! Mice: the "Bragi"/Slipstream get-property exchange, facts from HeyOkay/HaloBattery (MIT,
//! docs/protocols.md + providers/corsair.py, with reply bytes captured on a Dark Core RGB Pro SE,
//! an M65 RGB Ultra Wireless and an M75), cross-checked against ckb-next and OpenLinkHub (GPL,
//! read for PIDs and byte layout only, nothing translated).
//! Mice: Slipstream receivers 1B7F (Dark Core RGB Pro SE), 1B81 (Dark Core RGB Pro), 1B65 (Harpoon
//! RGB Wireless), 1BA6 / 1BDC / 2B00 (generic receivers: M65 RGB Ultra, M75, Sabre RGB Pro,
//! Scimitar Elite, Nightsabre, Katar...), and the M65 on its cable (1BB5).
//! None of this is verified on hardware by wattray itself.

use crate::hid::{self, Dev};
use crate::reading::{Kind, Reading};

/// Void family (Void, Void Pro, HS70...): battery on usage page 0xFFC5.
const VOID: &[u16] = &[
    0x1b1c, 0x1b27, 0x0a14, 0x0a16, 0x0a17, 0x0a1d, 0x0a1a, 0x1b2a, 0x1b23, 0x1b29, 0x0a55, 0x0a51, 0x0a52, 0x0a38,
    0x0a4f, 0x0a2b, 0x0a75, 0x0a56,
];
/// Wireless V2 receivers, interface 4.
const V2W: &[(u16, &str)] = &[(0x2a08, "Corsair Void Wireless V2"), (0x2a02, "Corsair Virtuoso Max"), (0x0a97, "Corsair HS80 Max")];
/// Virtuoso XT / SE ("Bragi" protocol), usage page 0xFF42. The bool: wired XT, the only one that answers as itself.
const BRAGI: &[(u16, &str, bool)] = &[
    (0x0a64, "Corsair Virtuoso XT", false),
    (0x0a62, "Corsair Virtuoso XT", true),
    (0x0a3e, "Corsair Virtuoso SE", false),
    (0x0a3d, "Corsair Virtuoso SE", false),
];
/// Slipstream receivers ("Bragi", usage page 0xFF42:0001), mouse behind them on route 0x09.
/// The bool: also answers property 0x10 (charge state); measured on the 1BA6 only.
const SLIP: &[(u16, &str, bool)] = &[
    (0x1b7f, "Corsair Dark Core RGB Pro SE", false),
    (0x1b81, "Corsair Dark Core RGB Pro", false), // ckb-next + OpenLinkHub only
    (0x1b65, "Corsair Harpoon RGB Wireless", false), // ckb-next + OpenLinkHub only
    (0x1ba6, "Corsair Slipstream Mouse", true),
    (0x1bdc, "Corsair Slipstream Mouse", false),
    (0x2b00, "Corsair Slipstream Mouse", false), // OpenLinkHub only
];
/// M65 RGB Ultra Wireless on its cable: answers as itself (route 0x08, source 0x00) and shares the
/// receiver's icon id, since the receiver goes silent while the cable is in.
const M65_CABLE: u16 = 0x1bb5;

pub fn read(out: &mut Vec<Reading>) {
    let mut done: Vec<u16> = Vec::new();
    for info in hid::enumerate(0x1b1c) {
        if done.contains(&info.pid) {
            continue;
        }
        let usage = (info.usage_page, info.usage);
        let (name, result, kind) = if VOID.contains(&info.pid) && usage == (0xffc5, 0x0001) {
            let name = if info.product.is_empty() { "Corsair Void".into() } else { info.product.clone() };
            (name, Dev::open(&info).map(|d| void(&d)), Kind::Headset)
        } else if let Some((_, name)) = V2W.iter().find(|m| m.0 == info.pid).filter(|_| info.mi == Some(4)) {
            (name.to_string(), Dev::open(&info).map(|d| v2w(&d)), Kind::Headset)
        } else if let Some((_, name, wired)) = BRAGI.iter().find(|m| m.0 == info.pid).filter(|_| usage == (0xff42, 0x0001)) {
            (name.to_string(), Dev::open(&info).map(|d| bragi(&d, *wired)), Kind::Headset)
        } else if let Some((_, name, charge)) = SLIP.iter().find(|m| m.0 == info.pid).filter(|_| usage == (0xff42, 0x0001)) {
            if out.iter().any(|r| r.id == format!("corsair:{:04x}", info.pid)) {
                continue; // the cable already answered for this mouse
            }
            // ponytail: mouse vs keyboard behind a generic receiver needs the device list (an
            // open/close endpoint session); everything is shown as a mouse, route 0x09 = first paired device.
            (name.to_string(), Dev::open(&info).map(|d| mouse(&d, 0x09, *charge)), Kind::Mouse)
        } else if info.pid == M65_CABLE && usage == (0xff42, 0x0001) {
            if let Some(Some((percent, charging))) = Dev::open(&info).map(|d| mouse(&d, 0x08, true)) {
                out.retain(|r| r.id != "corsair:1ba6");
                out.push(Reading { id: "corsair:1ba6".into(), name: "Corsair M65 RGB Ultra Wireless".into(), percent, charging, kind: Kind::Mouse, asleep: false });
            }
            done.push(info.pid);
            continue;
        } else {
            continue;
        };
        let Some(result) = result else { continue }; // could not open: maybe another interface works
        done.push(info.pid);
        if let Some((percent, charging)) = result {
            out.push(Reading { id: format!("corsair:{:04x}", info.pid), name, percent, charging, kind, asleep: false });
        }
    }
}

fn void(dev: &Dev) -> Option<(u8, bool)> {
    decode_void(&hid::query(dev, &[0xc9, 0x64], 1000, |r| r.first() == Some(&0x64) && r.len() >= 5)?)
}

/// [2] = percent (bit 7: mic up), [4] = 0 offline, 1 normal, 2 low, 4/5 charging.
fn decode_void(r: &[u8]) -> Option<(u8, bool)> {
    let status = *r.get(4)?;
    if !matches!(status, 1 | 2 | 4 | 5) {
        return None;
    }
    Some(((r[2] & 0x7f).min(100), status >= 4))
}

/// Wakes the receiver link without entering software mode (no audible pop), then asks the
/// headset (endpoint 0x09) for its battery in tenths of a percent. No charging flag in this protocol.
fn v2w(dev: &Dev) -> Option<(u8, bool)> {
    dev.write(&[0x00, 0x02, 0x08, 0x02, 0x13]);
    dev.write(&[0x00, 0x02, 0x08, 0x02, 0x12]);
    hid::flush(dev);
    dev.write(&[0x00, 0x02, 0x09, 0x02, 0x12]);
    dev.read(1000)?; // no answer: headset off
    for _ in 0..4 {
        hid::flush(dev);
        // The receiver sometimes answers with the paired headset's id instead: retry out-of-range values.
        let r = hid::query(dev, &[0x00, 0x02, 0x09, 0x02, 0x0f], 1000, |r| r.len() >= 6)?;
        if let Some(p) = tenths(u16::from_le_bytes([r[4], r[5]]) as u32).filter(|_| r[4] | r[5] != 0) {
            return Some((p, false));
        }
    }
    None
}

/// Wired XT answers as itself (target 0x08); everything else answers through the headset (0x09).
fn bragi(dev: &Dev, wired_xt: bool) -> Option<(u8, bool)> {
    let targets = if wired_xt { [0x08, 0x09] } else { [0x09, 0x08] };
    for target in targets {
        if let Some(level) = bragi_get(dev, target, 0x0f, 300) {
            let charging = bragi_get(dev, target, 0x10, 1000) == Some(1); // advisory
            return tenths(level).map(|p| (p, charging));
        }
    }
    None
}

/// GET `prop`: request [02 target 02 prop 00]; reply [01 source 02 status value(u32 LE)].
fn bragi_get(dev: &Dev, target: u8, prop: u8, ms: u64) -> Option<u32> {
    hid::flush(dev);
    let source = if target == 0x09 { 0x01 } else { 0x00 };
    let r = hid::query(dev, &[0x02, target, 0x02, prop, 0x00], ms, |r| r.len() >= 8 && r[..3] == [0x01, source, 0x02])?;
    (r[3] == 0x00).then(|| u32::from_le_bytes([r[4], r[5], r[6], r[7]])) // 0x05: no such property
}

/// Mouse level (and charge state where known) through `route`: 0x09 the paired mouse behind a
/// receiver, 0x08 the device itself. Read-only property GETs, no session or software mode.
/// A sleeping mouse does not answer within the window (the Dark Core can take seconds), so it
/// simply gets no reading.
fn mouse(dev: &Dev, route: u8, charge: bool) -> Option<(u8, bool)> {
    let in_range = |v: u16| (10..=1000).contains(&v);
    let a = ask(dev, route, 0x0f, in_range)?;
    // The reply does not echo the property, so a stray answer to another request could pass as a
    // level: ask twice and refuse a disagreement (a silent second ask keeps the first).
    if ask(dev, route, 0x0f, in_range).is_some_and(|b| b != a) {
        return None;
    }
    let charging = charge && ask(dev, route, 0x10, |v| (1..=3).contains(&v)) == Some(1); // 1 charging, 2 battery, 3 full
    Some(((a / 10) as u8, charging))
}

/// GET `prop`: request [00 route 02 prop] (64 bytes behind report id 0).
fn ask(dev: &Dev, route: u8, prop: u8, ok: impl Fn(u16) -> bool) -> Option<u16> {
    let source = route & 1; // 0x09 answers as source 01, 0x08 as 00
    hid::flush(dev);
    let r = hid::query(dev, &[0x00, route, 0x02, prop], 500, |r| parse_get(r, source).is_some_and(&ok))?;
    parse_get(&r, source)
}

/// Reply `[source, 02, status 00, value u16 LE, ...]`. Anything else on that collection (error
/// status `01 02 06`, the receiver's own `00 02 00 <its pid>`, `00 12 ..` device records) is no value.
fn parse_get(r: &[u8], source: u8) -> Option<u16> {
    (r.len() >= 5 && r[..3] == [source, 0x02, 0x00]).then(|| u16::from_le_bytes([r[3], r[4]]))
}

/// Tenths of a percent (0..=1000) to percent; anything larger is not a battery value.
fn tenths(v: u32) -> Option<u8> {
    (v <= 1000).then(|| (v / 10) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn void_reply() {
        assert_eq!(decode_void(&[0x64, 0, 0x80 | 55, 0xb1, 1]), Some((55, false))); // mic up bit masked
        assert_eq!(decode_void(&[0x64, 0, 80, 0xb1, 5]), Some((80, true)));
        assert_eq!(decode_void(&[0x64, 0, 80, 0xb1, 0]), None); // offline
        assert_eq!(decode_void(&[0x64, 0, 80]), None); // short
    }

    // Real replies from HaloBattery issues #56/#411 (MIT), hid::strip applied where noted.
    #[test]
    fn mouse_receiver_replies() {
        assert_eq!(parse_get(&[0x01, 0x02, 0x00, 0x26, 0x02, 0, 0], 1), Some(550)); // 55 %
        assert_eq!(parse_get(&[0x01, 0x02, 0x00, 0xd4, 0x03, 0, 0], 1), Some(980));
        assert_eq!(parse_get(&[0x01, 0x02, 0x00, 0x02, 0x00, 0, 0], 1), Some(2)); // charge: on battery
        assert_eq!(parse_get(hid::strip(&[0x00, 0x01, 0x02, 0x00, 0x26, 0x02]), 1), Some(550));
    }

    #[test]
    fn mouse_cable_replies() {
        assert_eq!(parse_get(&[0x00, 0x02, 0x00, 0x3e, 0x03, 0], 0), Some(830)); // 83 %
        assert_eq!(parse_get(&[0x00, 0x02, 0x00, 0xe8, 0x03, 0], 0), Some(1000));
        assert_eq!(parse_get(&[0x00, 0x02, 0x00, 0x01, 0x00, 0], 0), Some(1)); // charging
        assert_eq!(parse_get(hid::strip(&[0x00, 0x00, 0x02, 0x00, 0x48, 0x03]), 0), Some(840));
    }

    #[test]
    fn mouse_refuses_other_frames() {
        assert_eq!(parse_get(&[0x01, 0x02, 0x06, 0, 0], 1), None); // error status
        assert_eq!(parse_get(&[0x00, 0x02, 0x00, 0x7f, 0x1b], 1), None); // receiver's own answer: wrong source
        assert_eq!(parse_get(&[0x00, 0x12, 0x00, 0x21, 0x00, 0x01, 0x1c, 0x1b], 0), None); // device record
        assert_eq!(parse_get(&[0x01, 0x02, 0x00, 0x26], 1), None); // short
        assert_eq!(parse_get(&[], 1), None);
        // a charge answer (2) or a pid (0x1b7f) is outside the level range used by `mouse`
        assert!(![2u16, 0x1b7f].iter().any(|v| (10..=1000).contains(v)));
    }

    #[test]
    fn tenths_of_percent() {
        assert_eq!(tenths(935), Some(93));
        assert_eq!(tenths(1000), Some(100));
        assert_eq!(tenths(1001), None);
    }
}
