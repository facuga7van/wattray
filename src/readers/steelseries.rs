//! SteelSeries Arctis headsets. Protocol data from HeadsetControl (offsets are hidapi-style:
//! the leading 0x00 report id is dropped from replies). Verified on hardware: Nova Pro only.

use std::time::{Duration, Instant};

use crate::hid::{self, Dev};
use crate::reading::{scale, Reading};

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

pub fn read(out: &mut Vec<Reading>) {
    let mut done: Vec<u16> = Vec::new();
    for info in hid::enumerate(0x1038) {
        if done.contains(&info.pid) {
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
            out.push(Reading { id: format!("ss:{:04x}", info.pid), name: m.name.into(), percent, charging, asleep: false });
        }
    }
}

/// Writes the request and returns the reply that echoes it (or the first reply if none does).
fn query(dev: &Dev, req: &[u8]) -> Option<Vec<u8>> {
    if !dev.write(req) {
        return None;
    }
    let echo = strip(req);
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut first = None;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Some(r) = dev.read(left.as_millis().max(1) as u32) else { break };
        let r = strip(&r).to_vec();
        if r.starts_with(echo) {
            return Some(r);
        }
        first.get_or_insert(r);
    }
    first
}

fn strip(r: &[u8]) -> &[u8] {
    if r.first() == Some(&0) { &r[1..] } else { r }
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
    fn strip_unnumbered_report_id() {
        assert_eq!(strip(&[0x00, 0xb0, 1]), &[0xb0, 1]);
        assert_eq!(strip(&[0x06, 0xb0]), &[0x06, 0xb0]);
    }
}
