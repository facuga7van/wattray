//! HyperX headsets (HP vid 0x03F0, older Kingston vid 0x0951). Protocol data from HeadsetControl
//! (lib/devices/hyperx_*.hpp); offsets count from the report id byte. Not verified on hardware yet.

use std::thread::sleep;
use std::time::Duration;

use crate::hid::{self, Dev, HidInfo};
use crate::reading::{Kind, Reading};

#[derive(Clone, Copy)]
enum Proto {
    Cloud2,
    Cloud2Kingston,
    AlphaWireless,
    Alpha2Wireless,
    Flight,
}

struct Model {
    vid: u16,
    pids: &'static [u16],
    name: &'static str,
    /// (usage page, usage) when HeadsetControl names one; None: first vendor interface that answers.
    usage: Option<(u16, u16)>,
    proto: Proto,
}

#[rustfmt::skip]
const MODELS: &[Model] = &[
    Model { vid: 0x03f0, pids: &[0x0696, 0x018b], name: "HyperX Cloud II Wireless", usage: Some((0xff90, 0x0303)), proto: Proto::Cloud2 },
    Model { vid: 0x0951, pids: &[0x1718], name: "HyperX Cloud II Wireless", usage: None, proto: Proto::Cloud2Kingston },
    Model { vid: 0x03f0, pids: &[0x098d], name: "HyperX Cloud Alpha Wireless", usage: None, proto: Proto::AlphaWireless },
    Model { vid: 0x03f0, pids: &[0x08be], name: "HyperX Cloud Alpha 2 Wireless", usage: Some((0xff13, 0xff00)), proto: Proto::Alpha2Wireless },
    Model { vid: 0x0951, pids: &[0x16c4, 0x1723], name: "HyperX Cloud Flight", usage: None, proto: Proto::Flight },
];

/// What a headset said: a level, or charging without one (keep showing the last value).
enum State {
    Level(u8, bool),
    ChargingNoLevel,
}

pub fn read(out: &mut Vec<Reading>) {
    for vid in [0x03f0, 0x0951] {
        let mut done: Vec<u16> = Vec::new();
        for info in hid::enumerate(vid) {
            if done.contains(&info.pid) {
                continue;
            }
            let Some(m) = MODELS.iter().find(|m| m.vid == vid && m.pids.contains(&info.pid)) else { continue };
            if !usable(m, &info) {
                continue;
            }
            let Some(answer) = Dev::open(&info).and_then(|d| query(m.proto, &d)) else { continue };
            done.push(info.pid);
            let id = format!("hyperx:{:04x}", info.pid);
            let name = m.name.to_string();
            match answer {
                Some(State::Level(percent, charging)) => out.push(Reading { id, name, percent, charging, kind: Kind::Headset, asleep: false }),
                Some(State::ChargingNoLevel) => out.push(Reading { id, name, percent: 0, charging: true, kind: Kind::Headset, asleep: true }),
                None => {} // the interface answered: headset offline
            }
        }
    }
}

fn usable(m: &Model, info: &HidInfo) -> bool {
    match m.usage {
        Some(u) => u == (info.usage_page, info.usage),
        None => info.usage_page >= 0xff00 && info.output_len >= 3, // vendor-defined, takes our request
    }
}

/// Outer None: this interface did not answer (try the next one). Inner None: headset offline.
fn query(proto: Proto, dev: &Dev) -> Option<Option<State>> {
    match proto {
        Proto::Cloud2 => {
            let cmd = |c: u8| {
                dev.write(&[0x06, 0xff, 0xbb, c, 0x00]);
                sleep(Duration::from_millis(100));
                hid::read_until(dev, 1000, |r| r.len() >= 8 && r[..4] == [0x06, 0xff, 0xbb, c])
            };
            let level = cmd(0x02)?;
            let charging = cmd(0x03).is_some_and(|r| r[4] == 1);
            Some(Some(State::Level(level[7].min(100), charging)))
        }
        Proto::Cloud2Kingston => {
            let cmd = |c: u8| {
                let mut req = [0u8; 16];
                req.copy_from_slice(&[0x06, 0x00, 0x02, 0x00, 0x9a, 0x00, 0x00, 0x68, 0x4a, 0x8e, 0x0a, 0x00, 0x00, 0x00, 0xbb, c]);
                dev.write(&req);
                sleep(Duration::from_millis(100));
                hid::read_until(dev, 1000, |r| r.len() >= 8 && r[0] == 0x0b && r[2] == 0xbb && r[3] == c)
            };
            let level = cmd(0x02)?;
            let charging = cmd(0x03).is_some_and(|r| r[4] == 1);
            Some(Some(State::Level(level[7].min(100), charging)))
        }
        Proto::AlphaWireless => {
            // ponytail: HeadsetControl does not check replies here; requiring the echo [21 bb sub] is a guess.
            let get = |sub: u8| hid::query(dev, &[0x21, 0xbb, sub], 2000, |r| r.len() >= 4 && r[..3] == [0x21, 0xbb, sub]).map(|r| r[3]);
            if get(0x03)? == 0x01 {
                return Some(None);
            }
            if get(0x0c) == Some(0x01) {
                return Some(Some(State::ChargingNoLevel));
            }
            Some(get(0x0b).map(|p| State::Level(p.min(100), false)))
        }
        Proto::Alpha2Wireless => {
            let conn = hid::query(dev, &[0x52, 0x01], 1000, |r| r.len() >= 3 && r[0] == 0x53)?;
            if conn[2] == 0x00 {
                return Some(None);
            }
            let r = hid::query(dev, &[0x50, 0x02], 1000, |r| r.len() >= 4 && r[..2] == [0x51, 0x02])?;
            Some((r[2] <= 100).then(|| State::Level(r[2], r[3] == 0x01)))
        }
        Proto::Flight => {
            // ponytail: no reply check in HeadsetControl either; a plausible voltage is the only filter.
            let r = hid::query(dev, &[0x21, 0xff, 0x05], 2000, |r| r.len() >= 5 && flight_state(be(r)).is_some())?;
            Some(flight_state(be(&r)))
        }
    }
}

fn be(r: &[u8]) -> u16 {
    u16::from_be_bytes([r[3], r[4]])
}

/// Cloud Flight voltage (mV): above 0x100B means charging (no level); HeadsetControl's curve otherwise.
fn flight_state(mv: u16) -> Option<State> {
    let v = mv as f64;
    match mv {
        0x100c.. => Some(State::ChargingNoLevel),
        3976.. => Some(State::Level(100, false)),
        3000..=3648 => Some(State::Level((0.00125 * v).round() as u8, false)),
        3649..=3975 => {
            let p = 0.00000002547505 * v.powi(4) - 0.0003900299 * v.powi(3) + 2.238321 * v.powi(2) - 5706.256 * v + 5452299.0;
            Some(State::Level(p.round().clamp(0.0, 100.0) as u8, false))
        }
        _ => None, // not a battery report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(s: Option<State>) -> Option<u8> {
        match s {
            Some(State::Level(p, _)) => Some(p),
            _ => None,
        }
    }

    #[test]
    fn flight_curve() {
        assert_eq!(level(flight_state(4000)), Some(100));
        assert!(matches!(flight_state(0x1010), Some(State::ChargingNoLevel)));
        let mid = level(flight_state(3800)).unwrap();
        assert!((20..=80).contains(&mid), "{mid}");
        assert!(level(flight_state(3700)) < level(flight_state(3900)));
        assert!(flight_state(100).is_none()); // unrelated report
    }
}
