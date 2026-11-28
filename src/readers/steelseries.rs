//! SteelSeries Arctis headsets and wireless mice. Offsets are hidapi-style: the leading 0x00 report
//! id is dropped from replies.
//!
//! Headsets: protocol data from HeadsetControl. Verified on hardware: Nova Pro only.
//!
//! Mice (VID 0x1038, vendor interface 3): Aerox 3/5/9 Wireless (+ CS2/Destiny 2/Diablo IV/WoW
//! editions), Prime Wireless, Prime Mini Wireless, Rival 3 Wireless (+ Gen 2). NOT verified on
//! hardware by wattray. Facts and sources:
//! - Aerox/Prime: output report `00 92` (cable) or `00 D2` (2.4 GHz dongle, 0x92 | 0x40); reply
//!   echoes the command, byte 1 = level step (bits 0-6, `(v - 1) * 5` %, 1..21) + charging bit 7.
//!   rivalcfg (WTFPL, devices/aerox*_wireless_*.py, prime_wireless_*.py, mouse.py) + HaloBattery
//!   (MIT, providers/steelseries.py and tests, one real Aerox 9 reply checked against GG) +
//!   Wireless-Device-Battery-Monitor (GPL, facts only: the `40 FF` "mouse unreachable" reply, the
//!   wired 0x92 echo). PIDs: rivalcfg (all) and WDBM (all but Prime Mini and the WoW edition).
//! - Rival 3 Wireless (+ Gen 2): `00 AA 01`, reply byte 0 = level %, byte 2 = charging. rivalcfg
//!   + WDBM + HaloBattery (which also suspects an `AA` echo in front; handled below).
//! - Mouse asleep/off: level 0, silence or `40 FF`: no reading.
//!
//! Skipped: Rival 650 (rivalcfg endpoint 0 is the mouse collection, Windows blocks writes there)
//! and Aerox 3 Wireless Gen 2 (percent layout, a single GPL source).

use crate::hid::{self, Dev};
use crate::reading::{scale, Kind, Reading};

struct Model {
    name: &'static str,
    pids: &'static [u16],
    mi: u8,
    usage: Option<(u16, u16)>,
    req: &'static [u8],
    bat: usize,
    min: u8,
    max: u8,
    offline: Option<(usize, u8)>,
    charging: Option<(usize, &'static [u8])>,
}

const FFC0: Option<(u16, u16)> = Some((0xFFC0, 0x0001));
const FF43: Option<(u16, u16)> = Some((0xFF43, 0x0202));

#[rustfmt::skip]
const MODELS: &[Model] = &[
    Model { name: "Arctis Nova Pro", pids: &[0x12e0, 0x12e5], mi: 4, usage: None, req: &[0x06, 0xb0],
            bat: 6, min: 0, max: 8, offline: Some((15, 0x01)), charging: Some((15, &[0x02])) },
    Model { name: "Arctis 1/7X/7P", pids: &[0x12b3, 0x12b6, 0x12d7, 0x12d5], mi: 3, usage: FF43, req: &[0x06, 0x12],
            bat: 3, min: 0, max: 100, offline: Some((2, 0x01)), charging: None },
    Model { name: "Arctis 7", pids: &[0x1260, 0x12ad, 0x1252, 0x1280], mi: 5, usage: FF43, req: &[0x06, 0x18],
            bat: 2, min: 0, max: 100, offline: None, charging: None },
    Model { name: "Arctis 7+", pids: &[0x220e, 0x2212, 0x2216, 0x2236], mi: 3, usage: FFC0, req: &[0x00, 0xb0],
            bat: 2, min: 0, max: 4, offline: Some((1, 0x01)), charging: Some((3, &[0x01])) },
    Model { name: "Arctis 9", pids: &[0x12c2, 0x12c4], mi: 0, usage: None, req: &[0x00, 0x20],
            bat: 3, min: 0x64, max: 0x9A, offline: None, charging: Some((4, &[0x01])) },
    Model { name: "Arctis Nova 3/5", pids: &[0x2269, 0x226d, 0x2232, 0x2253], mi: 3, usage: FFC0, req: &[0x00, 0xb0],
            bat: 3, min: 0, max: 100, offline: Some((1, 0x02)), charging: Some((4, &[0x01])) },
    Model { name: "Arctis Nova 7", pids: &[0x2202, 0x2206, 0x220a, 0x223a, 0x227a, 0x22a4], mi: 3, usage: FFC0,
            req: &[0x00, 0xb0], bat: 2, min: 0, max: 4, offline: Some((3, 0x00)), charging: Some((3, &[0x01, 0x02])) },
    Model { name: "Arctis Nova 7", pids: &[0x22a1, 0x227e, 0x2258, 0x229e, 0x22ad, 0x22a9, 0x22a5, 0x22a7, 0x2298],
            mi: 3, usage: FFC0, req: &[0x00, 0xb0], bat: 2, min: 0, max: 100, offline: Some((3, 0x00)),
            charging: Some((3, &[0x01, 0x02])) },
];

struct Mouse {
    name: &'static str,
    /// Dongle then cable PID. Both get the dongle's id so a mouse seen both ways shows once.
    pids: [u16; 2],
    rival: bool,
}

#[rustfmt::skip]
const MICE: &[Mouse] = &[
    Mouse { name: "Aerox 3 Wireless", pids: [0x1838, 0x183a], rival: false },
    Mouse { name: "Aerox 3 Wireless CS2 Dragon Lore", pids: [0x1878, 0x187a], rival: false },
    Mouse { name: "Aerox 5 Wireless", pids: [0x1852, 0x1854], rival: false },
    Mouse { name: "Aerox 5 Wireless Destiny 2", pids: [0x185c, 0x185e], rival: false },
    Mouse { name: "Aerox 5 Wireless Diablo IV", pids: [0x1860, 0x1862], rival: false },
    Mouse { name: "Aerox 9 Wireless", pids: [0x1858, 0x185a], rival: false },
    Mouse { name: "Aerox 9 Wireless WoW", pids: [0x1874, 0x1876], rival: false },
    Mouse { name: "Prime Wireless", pids: [0x1840, 0x1842], rival: false },
    Mouse { name: "Prime Mini Wireless", pids: [0x1848, 0x184a], rival: false },
    Mouse { name: "Rival 3 Wireless", pids: [0x1830, 0x1830], rival: true },
    Mouse { name: "Rival 3 Wireless Gen 2", pids: [0x1872, 0x1872], rival: true },
];

const MOUSE_MI: u8 = 3;

pub fn read(out: &mut Vec<Reading>) {
    let mut done: Vec<u16> = Vec::new();
    for info in hid::enumerate(0x1038) {
        if done.contains(&info.pid) {
            continue;
        }
        if let Some(m) = MICE.iter().find(|m| m.pids.contains(&info.pid)) {
            read_mouse(m, &info, &mut done, out);
            continue;
        }
        let Some(m) = MODELS.iter().find(|m| m.pids.contains(&info.pid)) else { continue };
        if info.mi != Some(m.mi)
            || m.usage.is_some_and(|u| u != (info.usage_page, info.usage))
            || (info.output_len as usize) < m.req.len()
        {
            continue;
        }
        let Some(r) = Dev::open(&info).and_then(|d| query(&d, m.req)) else { continue };
        done.push(info.pid);
        if let Some((percent, charging)) = decode(m, &r) {
            out.push(Reading { id: format!("ss:{:04x}", info.pid), name: m.name.into(), percent, charging, kind: Kind::Headset, asleep: false });
        }
    }
}

fn read_mouse(m: &Mouse, info: &hid::HidInfo, done: &mut Vec<u16>, out: &mut Vec<Reading>) {
    let id = m.pids[0];
    let cable = info.pid == m.pids[1] && !m.rival;
    let cmd: &[u8] = if m.rival { &[0x00, 0xaa, 0x01] } else if cable { &[0x00, 0x92] } else { &[0x00, 0xd2] };
    // ponytail: any vendor-defined usage page on interface 3 (reports show 0xFFC0 for Aerox 9 only).
    if done.contains(&id) || info.mi != Some(MOUSE_MI) || info.usage_page < 0xFF00 || (info.output_len as usize) < cmd.len() {
        return;
    }
    let Some(dev) = Dev::open(info) else { return };
    hid::flush(&dev);
    let reply = if m.rival {
        hid::query(&dev, cmd, 500, |r| parse_rival(r).is_some())
    } else {
        hid::query(&dev, cmd, 500, |r| r.first() == Some(&cmd[1]) || r.starts_with(&[0x40, 0xff]))
    };
    // `done` only on a real reading: `40 FF` / step 0 on one interface must not hide the other.
    let Some((percent, charging)) = reply.and_then(|r| if m.rival { parse_rival(&r) } else { parse_aerox(cmd[1], &r) }) else { return };
    done.push(id);
    out.push(Reading { id: format!("ss:{id:04x}"), name: format!("SteelSeries {}", m.name), percent, charging, kind: Kind::Mouse, asleep: false });
}

/// `<cmd> <step | 0x80 charging> ...`; step 1..=21 -> 0..=100 %, rivalcfg's `(v - 1) * 5`.
/// Step 0 (asleep), `40 FF` (dongle cannot reach the mouse) and steps above 21 give nothing.
fn parse_aerox(cmd: u8, r: &[u8]) -> Option<(u8, bool)> {
    if *r.first()? != cmd {
        return None;
    }
    let b = *r.get(1)?;
    let step = b & 0x7f;
    (1..=21).contains(&step).then(|| ((step - 1) * 5, b & 0x80 != 0))
}

/// Rival 3: `<level %> <?> <charging>`. HaloBattery suspects an `AA` echo first (level can never
/// be 0xAA, so shifting is unambiguous). Level 0 or an out-of-range value: no reading.
fn parse_rival(r: &[u8]) -> Option<(u8, bool)> {
    let r = r.strip_prefix(&[0xaa]).unwrap_or(r);
    let (level, charging) = (*r.first()?, *r.get(2)?);
    // ponytail: a 0 % mouse is hidden too, indistinguishable from asleep; charging must be 0 or 1.
    if !(1..=100).contains(&level) || charging > 1 {
        return None;
    }
    Some((level, charging == 1))
}

/// The reply echoes the request.
fn query(dev: &Dev, req: &[u8]) -> Option<Vec<u8>> {
    hid::query(dev, req, 500, |r| r.starts_with(hid::strip(req)))
}

fn decode(m: &Model, r: &[u8]) -> Option<(u8, bool)> {
    if let Some((i, v)) = m.offline {
        if *r.get(i)? == v {
            return None;
        }
    }
    let charging = m.charging.is_some_and(|(i, vs)| r.get(i).is_some_and(|b| vs.contains(b)));
    Some((scale(*r.get(m.bat)?, m.min, m.max), charging))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nova_pro_reply() {
        let m = &MODELS[0];
        let mut r = vec![0u8; 31];
        r[..2].copy_from_slice(&[0x06, 0xb0]);
        r[6] = 2;
        r[15] = 0x08;
        assert_eq!(decode(m, &r), Some((25, false)));
        r[15] = 0x02;
        assert_eq!(decode(m, &r), Some((25, true)));
        r[15] = 0x01;
        assert_eq!(decode(m, &r), None);
        assert_eq!(decode(m, &r[..4]), None); // short reply
    }

    #[test]
    fn aerox_levels() {
        // Real Aerox 9 reply (HaloBattery issue #79, GG showed 15 %).
        assert_eq!(parse_aerox(0xd2, &[0xd2, 0x04, 0x00, 0x2e, 0x34, 0, 0, 0]), Some((15, false)));
        // Derived from rivalcfg's formula ((b & 0x7f) - 1) * 5, bit 7 = charging.
        assert_eq!(parse_aerox(0xd2, &[0xd2, 0x01]), Some((0, false)));
        assert_eq!(parse_aerox(0xd2, &[0xd2, 0x15]), Some((100, false)));
        assert_eq!(parse_aerox(0x92, &[0x92, 0x8b]), Some((50, true)));
    }

    #[test]
    fn aerox_rejects() {
        assert_eq!(parse_aerox(0xd2, &[0xd2, 0x00]), None); // asleep
        assert_eq!(parse_aerox(0xd2, &[0xd2, 0x80]), None); // asleep, charging bit only
        assert_eq!(parse_aerox(0xd2, &[0xd2, 0x16]), None); // step > 21
        assert_eq!(parse_aerox(0xd2, &[0xd2, 0xff]), None);
        assert_eq!(parse_aerox(0xd2, &[0x40, 0xff]), None); // mouse unreachable
        assert_eq!(parse_aerox(0xd2, &[0x92, 0x05]), None); // wrong echo
        assert_eq!(parse_aerox(0xd2, &[0xd2]), None); // short
        assert_eq!(parse_aerox(0xd2, &[]), None);
    }

    #[test]
    fn rival3_layouts() {
        // Derived from rivalcfg: level = data[0], charging = bool(data[2]).
        assert_eq!(parse_rival(&[75, 0, 0, 0]), Some((75, false)));
        assert_eq!(parse_rival(&[100, 0, 1]), Some((100, true)));
        assert_eq!(parse_rival(&[0xaa, 40, 0, 1]), Some((40, true))); // with echo
    }

    #[test]
    fn rival3_rejects() {
        assert_eq!(parse_rival(&[0, 0, 0]), None);
        assert_eq!(parse_rival(&[101, 0, 0]), None);
        assert_eq!(parse_rival(&[50, 0, 2]), None); // stray report
        assert_eq!(parse_rival(&[50, 0]), None);
        assert_eq!(parse_rival(&[0xaa]), None);
        assert_eq!(parse_rival(&[]), None);
    }

    #[test]
    fn mice_pids_do_not_clash_with_headsets() {
        for m in MICE {
            assert!(MODELS.iter().all(|h| !h.pids.iter().any(|p| m.pids.contains(p))), "{}", m.name);
        }
    }

    #[test]
    fn strip_unnumbered_report_id() {
        assert_eq!(hid::strip(&[0x00, 0xb0, 1]), &[0xb0, 1]);
        assert_eq!(hid::strip(&[0x06, 0xb0]), &[0x06, 0xb0]);
    }
}
