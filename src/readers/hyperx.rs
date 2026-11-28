//! HyperX wireless peripherals (HP vid 0x03F0, older Kingston vid 0x0951). Not verified on hardware
//! by wattray; every constant below comes from published sources, noted per protocol.
//!
//! Headsets: Cloud II Wireless, Cloud Alpha Wireless, Cloud Alpha 2 Wireless, Cloud Flight, Cloud
//! Stinger 2 Wireless, Cloud III Wireless, Cloud III S Wireless. Mice (NGENUITY protocol): Pulsefire
//! Haste 2 Wireless, Pulsefire Saga Pro, Pulsefire Dart. Offsets count from the report id byte.
//!
//! Sources: HeadsetControl hyperx_*.hpp (GPL-3.0: read for facts only) for the older headsets;
//! LennardKittner/HyperHeadset (MIT) for Cloud III / III S / Stinger 2; HeyOkay/HaloBattery docs and
//! providers (MIT) and battery-hub/battery-hub table.js (MIT) as cross-checks; PMarinov1994/
//! ngenuity-linux-cli (MIT) for the Haste 2 `50 02` / `51 02` exchange; notwaterbtl/hyperx-saga-control
//! (MIT) for the Saga Pro; radoslavchobanov/PlasmaNGenuity (MIT, from santeri3700's reverse
//! engineering) for the Dart. Only battery/link queries are sent, never a setting.

use std::thread::sleep;
use std::time::Duration;

use windows_sys::Win32::Devices::HumanInterfaceDevice::{HidP_Input, HidP_Output};

use crate::hid::{self, Dev, HidInfo};
use crate::reading::{Kind, Reading};

#[derive(Clone, Copy)]
enum Proto {
    Cloud2,
    Cloud2Kingston,
    AlphaWireless,
    Alpha2Wireless,
    Flight,
    Cloud3,
    Cloud3S,
    Ngenuity,
    Dart,
}

struct Model {
    vid: u16,
    pids: &'static [u16],
    name: &'static str,
    /// (usage page, usage) when the sources name one; None: first vendor interface that answers.
    usage: Option<(u16, u16)>,
    /// USB interface number (`&mi_XX`) the sources document; None: not constrained.
    mi: Option<u8>,
    /// Output report id the interface must declare before we write. With `usage: None` it replaces
    /// the vendor-page rule (Cloud III S: the sources disagree on its collection's usage page).
    report: Option<u8>,
    kind: Kind,
    proto: Proto,
}

#[rustfmt::skip]
const MODELS: &[Model] = &[
    Model { vid: 0x03f0, pids: &[0x0696, 0x018b], name: "HyperX Cloud II Wireless", usage: Some((0xff90, 0x0303)), mi: None, report: None, kind: Kind::Headset, proto: Proto::Cloud2 },
    // Stinger 2: HyperHeadset lists 0x0d93 with the Cloud II packets, and auto94's monitor (MIT) says "Stinger 2 data = Cloud II data".
    Model { vid: 0x03f0, pids: &[0x0d93], name: "HyperX Cloud Stinger 2 Wireless", usage: Some((0xff90, 0x0303)), mi: None, report: None, kind: Kind::Headset, proto: Proto::Cloud2 },
    Model { vid: 0x0951, pids: &[0x1718], name: "HyperX Cloud II Wireless", usage: None, mi: None, report: None, kind: Kind::Headset, proto: Proto::Cloud2Kingston },
    Model { vid: 0x03f0, pids: &[0x098d], name: "HyperX Cloud Alpha Wireless", usage: None, mi: None, report: None, kind: Kind::Headset, proto: Proto::AlphaWireless },
    Model { vid: 0x03f0, pids: &[0x08be], name: "HyperX Cloud Alpha 2 Wireless", usage: Some((0xff13, 0xff00)), mi: None, report: None, kind: Kind::Headset, proto: Proto::Alpha2Wireless },
    Model { vid: 0x0951, pids: &[0x16c4, 0x1723], name: "HyperX Cloud Flight", usage: None, mi: None, report: None, kind: Kind::Headset, proto: Proto::Flight },
    Model { vid: 0x03f0, pids: &[0x05b7, 0x0c9d, 0x0fcc], name: "HyperX Cloud III Wireless", usage: Some((0xff13, 0x0001)), mi: None, report: None, kind: Kind::Headset, proto: Proto::Cloud3 },
    Model { vid: 0x03f0, pids: &[0x02cc, 0x06be], name: "HyperX Cloud III S Wireless", usage: None, mi: None, report: Some(0x0c), kind: Kind::Headset, proto: Proto::Cloud3S },
    Model { vid: 0x03f0, pids: &[0x0f98, 0x0d97], name: "HyperX Pulsefire Haste 2 Wireless", usage: Some((0xff00, 0x0001)), mi: Some(2), report: Some(0x50), kind: Kind::Mouse, proto: Proto::Ngenuity },
    Model { vid: 0x03f0, pids: &[0x06bf], name: "HyperX Pulsefire Saga Pro", usage: None, mi: Some(3), report: Some(0x50), kind: Kind::Mouse, proto: Proto::Ngenuity },
    Model { vid: 0x03f0, pids: &[0x04bf], name: "HyperX Pulsefire Saga Pro", usage: None, mi: Some(2), report: Some(0x50), kind: Kind::Mouse, proto: Proto::Ngenuity },
    Model { vid: 0x0951, pids: &[0x16e1], name: "HyperX Pulsefire Dart", usage: None, mi: Some(2), report: None, kind: Kind::Mouse, proto: Proto::Dart },
    Model { vid: 0x0951, pids: &[0x16e2], name: "HyperX Pulsefire Dart", usage: None, mi: Some(1), report: None, kind: Kind::Mouse, proto: Proto::Dart },
];

/// What a device said: a level, or charging without one (keep showing the last value).
enum State {
    Level(u8, bool),
    ChargingNoLevel,
}

pub fn read(out: &mut Vec<Reading>) {
    for vid in [0x03f0, 0x0951] {
        let mut done: Vec<u16> = Vec::new();
        let infos = hid::enumerate(vid);
        for info in &infos {
            if done.contains(&info.pid) {
                continue;
            }
            let Some(m) = MODELS.iter().find(|m| m.vid == vid && m.pids.contains(&info.pid)) else { continue };
            if !usable(m, info) {
                continue;
            }
            let answer = match m.proto {
                Proto::Cloud3S => cloud3s(info, &infos),
                _ => Dev::open(info).and_then(|d| query(m.proto, &d)),
            };
            let Some(answer) = answer else { continue };
            done.push(info.pid);
            let id = format!("hyperx:{:04x}", shared_pid(info.pid));
            let (name, kind) = (m.name.to_string(), m.kind);
            let r = match answer {
                Some(State::Level(percent, charging)) => Reading { id, name, percent, charging, kind, asleep: false },
                Some(State::ChargingNoLevel) => Reading { id, name, percent: 0, charging: true, kind, asleep: true },
                None => continue, // the interface answered: headset or mouse offline
            };
            match out.iter_mut().find(|o| o.id == r.id) {
                Some(o) if r.charging && !o.charging => *o = r, // dongle and cable both answer: charging wins
                Some(_) => {}
                None => out.push(r),
            }
        }
    }
}

/// A wired mouse shares its dongle's id so the pair shows once (Saga Pro, Dart, Haste 2).
fn shared_pid(pid: u16) -> u16 {
    match pid {
        0x04bf => 0x06bf,
        0x16e2 => 0x16e1,
        0x0d97 => 0x0f98,
        p => p,
    }
}

fn usable(m: &Model, info: &HidInfo) -> bool {
    let usage_ok = match m.usage {
        Some(u) => u == (info.usage_page, info.usage),
        // the declared report id below decides; Cloud III S also needs a page its sources name (vendor or battery-hub's 0x01C0)
        None if m.report.is_some() => !matches!(m.proto, Proto::Cloud3S) || info.usage_page >= 0xff00 || info.usage_page == 0x01c0,
        None => info.usage_page >= 0xff00 && info.output_len >= 3, // vendor-defined, takes our request
    };
    usage_ok
        && m.mi.is_none_or(|n| info.mi == Some(n))
        && m.report.is_none_or(|id| hid::report_ids(info, HidP_Output).contains(&id))
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
        Proto::Cloud3 => {
            // `66 89` / `66 8a` as a vendor output report (HyperHeadset, battery-hub).
            let ask = |cmd: u8, ids: [u8; 2]| {
                if !dev.write(&[0x66, cmd]) {
                    return None;
                }
                sleep(Duration::from_millis(50));
                hid::read_until(dev, 450, |r| r.len() >= 5 && r[0] == 0x66 && ids.contains(&r[1]))
            };
            // ponytail: "headset off" is only the all-zero state gate; HyperHeadset's separate `66 82` link query is a single source.
            let Some(p) = cloud3_level(&ask(0x89, [0x89, 0x0d])?) else { return Some(None) };
            let charging = ask(0x8a, [0x8a, 0x0c]).and_then(|r| cloud3_charging(&r)).unwrap_or(false);
            Some(Some(State::Level(p, charging)))
        }
        Proto::Ngenuity => {
            // `52 01` first: a command sent to a sleeping mouse is queued by the dongle and answered on wake-up.
            // Silence is unknown (still ask for the battery); only an explicit "not connected" skips.
            let link = hid::query(dev, &[0x52, 0x01], 400, |r| r.len() >= 3 && r[..2] == [0x53, 0x01]);
            if link.is_some_and(|l| l[2] == 0) {
                return Some(None);
            }
            sleep(Duration::from_millis(10)); // NGENUITY paces packets at least 10 ms apart
            let r = hid::query(dev, &[0x50, 0x02], 400, |r| r.len() >= 4 && r[..2] == [0x51, 0x02])?;
            Some(ngenuity_state(&r))
        }
        Proto::Dart => {
            // Unnumbered output report: id 0, then the heartbeat command 0x51.
            let r = hid::query(dev, &[0x00, 0x51], 450, |r| r.len() >= 6 && r[0] == 0x51)?;
            Some(dart_state(&r))
        }
        Proto::Cloud3S => None, // handled by `cloud3s`: it needs the dongle's sibling collections
        Proto::Flight => {
            // ponytail: no reply check in HeadsetControl either; a plausible voltage is the only filter.
            let r = hid::query(dev, &[0x21, 0xff, 0x05], 2000, |r| r.len() >= 5 && flight_state(be(r)).is_some())?;
            Some(flight_state(be(&r)))
        }
    }
}

/// Cloud III `66 89` reply: percent at byte 4, only when byte 2 or 3 is non-zero (HyperHeadset treats an
/// all-zero state as "no reading yet", which is what a dongle without a linked headset gives).
fn cloud3_level(r: &[u8]) -> Option<u8> {
    (r.len() >= 5 && r[0] == 0x66 && matches!(r[1], 0x89 | 0x0d) && (r[2] | r[3]) != 0 && r[4] <= 100).then(|| r[4])
}

/// Cloud III `66 8a` reply: byte 2 is 0 not charging, 1 charging, 2 fully charged (still on the cable).
fn cloud3_charging(r: &[u8]) -> Option<bool> {
    if r.len() < 3 || r[0] != 0x66 || !matches!(r[1], 0x8a | 0x0c) {
        return None;
    }
    match r[2] {
        0 => Some(false),
        1 | 2 => Some(true),
        _ => None, // HyperHeadset's error value: no claim
    }
}

/// Cloud III S input report `0c .. .. .. .. <cmd> <value>`; 0xff is "no value" (headset off).
fn cloud3s_value(r: &[u8], cmd: u8) -> Option<u8> {
    (r.len() >= 7 && r[0] == 0x0c && r[5] == cmd).then(|| r[6])
}

/// Cloud III S: the request goes to the one collection that declares output report 0x0c; the answer
/// is an input report 0x0c that Windows may deliver to a sibling collection, so every collection of
/// this dongle that declares that input report is read while waiting (HaloBattery, MIT).
fn cloud3s(info: &HidInfo, all: &[HidInfo]) -> Option<Option<State>> {
    let w = Dev::open(info)?;
    let readers: Vec<Dev> = all
        .iter()
        .filter(|i| i.vid == info.vid && i.pid == info.pid && hid::report_ids(i, HidP_Input).contains(&0x0c))
        .filter_map(Dev::open)
        .collect();
    // ponytail: no collection declared input 0x0c (odd descriptor parse): listen on the writer's own handle only.
    let rd: Vec<&Dev> = if readers.is_empty() { vec![&w] } else { readers.iter().collect() };
    for d in &rd {
        hid::flush(d);
    }
    let ask = |cmd: u8| {
        // byte 3 = 01 reads; 00 would set a value and is never sent
        if !w.write(&[0x0c, 0x02, 0x03, 0x01, 0x00, cmd]) {
            return None;
        }
        let end = std::time::Instant::now() + Duration::from_millis(450);
        while std::time::Instant::now() < end {
            let pass = std::time::Instant::now();
            for d in &rd {
                if let Some(v) = d.read(30).and_then(|r| cloud3s_value(hid::strip(&r), cmd)) {
                    return Some(v);
                }
            }
            if pass.elapsed() < Duration::from_millis(10) {
                sleep(Duration::from_millis(20)); // every handle failed at once (access 0): do not spin
            }
        }
        None
    };
    let level = ask(0x06)?;
    if level > 100 {
        return Some(None);
    }
    let charging = ask(0x48).is_some_and(|v| matches!(v, 1 | 2));
    Some(Some(State::Level(level, charging)))
}

/// NGENUITY `51 02` battery reply: level at 2 (the mouse's own gauge), charge status at 3 (0 not
/// charging, 1 charging, 2 full on the charger; 3 charger error and 0xff unknown give no charging flag).
/// Bytes 6-7 (mV, little endian) are not used.
fn ngenuity_state(r: &[u8]) -> Option<State> {
    // ponytail: level 0 is refused too (an unfilled gauge looks the same); the mV bytes could cross-check it.
    (r.len() >= 4 && r[..2] == [0x51, 0x02] && (1..=100).contains(&r[2])).then(|| State::Level(r[2], matches!(r[3], 1 | 2)))
}

/// Pulsefire Dart heartbeat reply `51 00 00 03 <percent> <charging>`.
fn dart_state(r: &[u8]) -> Option<State> {
    // ponytail: only 1 counts as charging (both sources); a full battery on the cable may read as not charging.
    (r.len() >= 6 && r[0] == 0x51 && (1..=100).contains(&r[4])).then(|| State::Level(r[4], r[5] == 1))
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

    fn charging(s: Option<State>) -> Option<bool> {
        match s {
            Some(State::Level(_, c)) => Some(c),
            _ => None,
        }
    }

    /// Real frames: HeyOkay/HaloBattery #155 (USBPcap of NGENUITY, Cloud II Core, same `66 <cmd>` exchange
    /// as the Cloud III): `66 89` -> `66 89 0e d7 30` (48 %), `66 8a` -> `66 8a 00 00`.
    #[test]
    fn cloud3_real_frames() {
        let mut battery = vec![0x66, 0x89, 0x0e, 0xd7, 0x30];
        battery.resize(62, 0);
        assert_eq!(cloud3_level(&battery), Some(48));
        let mut off_cable = vec![0x66, 0x8a, 0x00, 0x00];
        off_cable.resize(62, 0);
        assert_eq!(cloud3_charging(&off_cable), Some(false));
    }

    #[test]
    fn cloud3_rejects() {
        assert_eq!(cloud3_level(&[0x66, 0x89, 0, 0, 55]), None); // all-zero state: no reading yet
        assert_eq!(cloud3_level(&[0x66, 0x89, 1, 0, 101]), None);
        assert_eq!(cloud3_level(&[0x66, 0x0d, 0, 1, 70]), Some(70)); // response id spelling
        assert_eq!(cloud3_level(&[0x67, 0x89, 1, 1, 50]), None); // wrong report id
        assert_eq!(cloud3_level(&[0x66, 0x8a, 1, 1, 50]), None); // a charging reply is not a level
        assert_eq!(cloud3_level(&[0x66, 0x89, 1, 1]), None); // short
        assert_eq!(cloud3_charging(&[0x66, 0x8a, 1]), Some(true));
        assert_eq!(cloud3_charging(&[0x66, 0x0c, 2]), Some(true)); // fully charged: on the cable
        assert_eq!(cloud3_charging(&[0x66, 0x8a, 3]), None); // error value
        assert_eq!(cloud3_charging(&[0x66, 0x89, 1]), None);
    }

    /// Layout from HyperHeadset cloud_iii_s_wireless.rs and HaloBattery (no raw capture is published):
    /// `0c 02 03 01 00 06 <percent>` for the battery, `... 48 <state>` for charging, 0xff = off.
    #[test]
    fn cloud3s_frames() {
        let mut f = vec![0x0c, 0x02, 0x03, 0x01, 0x00, 0x06, 89];
        f.resize(64, 0);
        assert_eq!(cloud3s_value(&f, 0x06), Some(89));
        assert_eq!(cloud3s_value(&f, 0x48), None); // other command
        f[6] = 0xff;
        assert_eq!(cloud3s_value(&f, 0x06), Some(0xff)); // the caller turns >100 into "offline"
        assert_eq!(cloud3s_value(&[0x0d, 0x02, 0x03, 0x01, 0x00, 0x06, 50], 0x06), None); // 0x0d notification
        assert_eq!(cloud3s_value(&[0x0c, 0x02, 0x03, 0x01, 0x00, 0x06], 0x06), None); // short
    }

    /// Layout from ngenuity-linux-cli's NGENUITY capture of a Haste 2 Wireless: level 54 %, not charging,
    /// 26 C, 0x0f00 = 3840 mV; and hyperx-saga-control's `51 02 64 02 .. 00 bc 10` (full on the charger).
    #[test]
    fn ngenuity_frames() {
        let mut f = vec![0x51, 0x02, 0x36, 0x00, 0x1a, 0x00, 0x00, 0x0f, 0x01, 0x00, 0x00];
        f.resize(64, 0);
        assert_eq!((level(ngenuity_state(&f)), charging(ngenuity_state(&f))), (Some(54), Some(false)));
        f[2..4].copy_from_slice(&[0x64, 0x02]);
        assert_eq!((level(ngenuity_state(&f)), charging(ngenuity_state(&f))), (Some(100), Some(true)));
        f[3] = 0x01;
        assert_eq!(charging(ngenuity_state(&f)), Some(true));
        f[3] = 0x03; // charger error: level still real, no charging claim
        assert_eq!((level(ngenuity_state(&f)), charging(ngenuity_state(&f))), (Some(100), Some(false)));
    }

    #[test]
    fn ngenuity_rejects() {
        assert!(ngenuity_state(&[0x50, 0x03, 0x01, 0x00]).is_none()); // dongle ACK, not the battery
        assert!(ngenuity_state(&[0x51, 0x01, 50, 0]).is_none()); // other sub-command
        assert!(ngenuity_state(&[0x51, 0x02, 0, 0]).is_none());
        assert!(ngenuity_state(&[0x51, 0x02, 101, 0]).is_none());
        assert!(ngenuity_state(&[0x51, 0x02, 50]).is_none());
    }

    /// Frame shape from battery-hub's test (santeri3700's reverse engineering): `51 00 00 03 <pct> <chg>`.
    #[test]
    fn dart_frames() {
        let on_cable = [0x51, 0, 0, 3, 45, 1];
        assert_eq!((level(dart_state(&on_cable)), charging(dart_state(&on_cable))), (Some(45), Some(true)));
        assert_eq!(charging(dart_state(&[0x51, 0, 0, 3, 45, 0])), Some(false));
        assert!(dart_state(&[0x51, 0, 0, 3, 0, 0]).is_none());
        assert!(dart_state(&[0x51, 0, 0, 3, 101, 0]).is_none());
        assert!(dart_state(&[0x52, 0, 0, 3, 45, 0]).is_none());
        assert!(dart_state(&[0x51, 0, 0, 3, 45]).is_none());
    }

    #[test]
    fn wired_mice_share_the_dongle_id() {
        assert_eq!((shared_pid(0x04bf), shared_pid(0x16e2), shared_pid(0x0d97)), (0x06bf, 0x16e1, 0x0f98));
        assert_eq!((shared_pid(0x0f98), shared_pid(0x05b7)), (0x0f98, 0x05b7)); // dongles and headsets keep theirs
    }

    #[test]
    fn models_are_unambiguous() {
        for (i, a) in MODELS.iter().enumerate() {
            for b in &MODELS[i + 1..] {
                assert!(a.vid != b.vid || !a.pids.iter().any(|p| b.pids.contains(p)), "{}", a.name);
            }
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
