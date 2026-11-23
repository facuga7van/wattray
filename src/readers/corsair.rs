//! Corsair headsets. Protocol data from HeadsetControl (lib/devices/corsair_*.hpp); offsets count
//! from the report id byte, as hid::query returns them. Not verified on hardware yet.

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

pub fn read(out: &mut Vec<Reading>) {
    let mut done: Vec<u16> = Vec::new();
    for info in hid::enumerate(0x1b1c) {
        if done.contains(&info.pid) {
            continue;
        }
        let usage = (info.usage_page, info.usage);
        let (name, result) = if VOID.contains(&info.pid) && usage == (0xffc5, 0x0001) {
            let name = if info.product.is_empty() { "Corsair Void".into() } else { info.product.clone() };
            (name, Dev::open(&info).map(|d| void(&d)))
        } else if let Some((_, name)) = V2W.iter().find(|m| m.0 == info.pid).filter(|_| info.mi == Some(4)) {
            (name.to_string(), Dev::open(&info).map(|d| v2w(&d)))
        } else if let Some((_, name, wired)) = BRAGI.iter().find(|m| m.0 == info.pid).filter(|_| usage == (0xff42, 0x0001)) {
            (name.to_string(), Dev::open(&info).map(|d| bragi(&d, *wired)))
        } else {
            continue;
        };
        let Some(result) = result else { continue }; // could not open: maybe another interface works
        done.push(info.pid);
        if let Some((percent, charging)) = result {
            out.push(Reading { id: format!("corsair:{:04x}", info.pid), name, percent, charging, kind: Kind::Headset, asleep: false });
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

    #[test]
    fn tenths_of_percent() {
        assert_eq!(tenths(935), Some(93));
        assert_eq!(tenths(1000), Some(100));
        assert_eq!(tenths(1001), None);
    }
}
