//! ASTRO A30 (codename Carbonite) through its USB transmitter. The headset speaks Logitech's
//! "Centurion" protocol (facts from the Logitech G app): a message is
//! `[feature index][function << 4 | sw id][params]`; battery is feature 0x0104, function 0, which
//! answers `[real %, shown %, charging state]`. The transmitter tunnels those messages on its
//! ff03:0001 collection, report 0xF2, inside a fixed header, as captured from a user's A30 with
//! `wattray --listen` (no public project documents it):
//!   f2 | 00 00 0c 1e 82 40 00 00 | f2 00 LL 00 00 00 00 | message      (LL = 1 + message length)
//! Battery changes also arrive unasked as events (sw id 0): `02 00 43 3c 02` = index 2, 67 %, 60 %, charging.
// ponytail: requests reuse the header of the headset's own messages (one capture, one unit); if the
// transmitter wants another one for host -> headset, --debug shows the silence.

use std::time::{Duration, Instant};

use crate::hid::{self, Dev};
use crate::reading::{Kind, Reading};

const MODELS: &[(u16, u16, &str)] = &[(0x9886, 0x0050, "ASTRO A30"), (0x9886, 0x004f, "ASTRO A30")];
const REPORT: u8 = 0xF2;
const HEADER: [u8; 8] = [0x00, 0x00, 0x0c, 0x1e, 0x82, 0x40, 0x00, 0x00];
/// Bytes before the message: report, header, `f2 00 LL 00 00 00 00`.
const OFFSET: usize = 16;
const SW: u8 = 1;
const BATTERY: u16 = 0x0104;
/// Battery feature index in the captured events, used if the lookup gets no answer.
const SEEN_BATTERY_INDEX: u8 = 2;
const TIMEOUT: Duration = Duration::from_millis(1000);

pub fn read(out: &mut Vec<Reading>) {
    let mut done: Vec<u16> = Vec::new();
    for &(vid, pid, name) in MODELS {
        for info in hid::enumerate(vid) {
            if info.pid != pid || (info.usage_page, info.usage) != (0xff03, 0x0001) || info.output_len < 64
                || done.contains(&pid)
            {
                continue;
            }
            let Some(dev) = Dev::open(&info) else { continue };
            if let Some((percent, charging)) = battery(&dev) {
                done.push(pid);
                out.push(Reading { id: format!("centurion:{pid:04x}"), name: name.into(), percent, charging,
                    kind: Kind::Headset, asleep: false });
            }
        }
    }
}

fn battery(dev: &Dev) -> Option<(u8, bool)> {
    // Root (index 0) getFeature(0x0104) -> [index, type, version]; index 0 = not there.
    let idx = request(dev, &[0, SW, (BATTERY >> 8) as u8, BATTERY as u8])
        .and_then(|p| p.first().copied())
        .filter(|&i| i != 0)
        .unwrap_or(SEEN_BATTERY_INDEX);
    parse_battery(&request(dev, &[idx, SW])?)
}

/// Sends one message to the headset; the parameters of its reply (or of a matching event).
fn request(dev: &Dev, msg: &[u8]) -> Option<Vec<u8>> {
    if !dev.write(&frame(msg)) {
        return None;
    }
    let (feat, fsw) = (msg[0], msg[1]);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let Some(m) = message(&dev.read(left.as_millis().max(1) as u32)?) else { continue };
        if m.len() >= 3 && m[0] == 0xFF && m[1] == feat && m[2] == fsw {
            return None; // error reply
        }
        // Our reply, or the same feature's event (sw id 0, e.g. a battery change) arriving meanwhile.
        if m.len() >= 2 && m[0] == feat && (m[1] == fsw || (feat != 0 && m[1] == fsw & 0xF0)) {
            return Some(m[2..].to_vec());
        }
    }
}

fn frame(msg: &[u8]) -> Vec<u8> {
    let mut f = vec![REPORT];
    f.extend_from_slice(&HEADER);
    f.extend_from_slice(&[REPORT, 0x00, msg.len() as u8 + 1, 0x00, 0x00, 0x00, 0x00]);
    f.extend_from_slice(msg);
    f
}

/// The Centurion message inside a transmitter report. The header bytes are not compared: only one
/// unit has been seen, they may be an address.
fn message(r: &[u8]) -> Option<Vec<u8>> {
    if r.len() < OFFSET || r[0] != REPORT || r[9] != REPORT || r[10] != 0 {
        return None;
    }
    let len = r[11] as usize; // 1 + message length
    (len >= 2 && r.len() >= OFFSET + len - 1).then(|| r[OFFSET..OFFSET + len - 1].to_vec())
}

/// `[real %, shown %, state]`: the Logitech G app shows the second; states 1/2 = charging slow/fast.
fn parse_battery(d: &[u8]) -> Option<(u8, bool)> {
    let &[_, shown, state, ..] = d else { return None };
    (shown <= 100).then_some((shown, matches!(state, 1 | 2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split(' ').map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn captured_reports() {
        // From a user's A30 (`wattray --listen 9886:0050`), padding trimmed.
        let battery = hex("f2 00 00 0c 1e 82 40 00 00 f2 00 06 00 00 00 00 02 00 43 3c 02 00 00");
        assert_eq!(message(&battery), Some(vec![0x02, 0x00, 0x43, 0x3c, 0x02]));
        let other = hex("f2 00 00 0c 1e 82 40 00 00 f2 00 08 00 00 00 00 09 10 02 00 00 01 01 00");
        assert_eq!(message(&other), Some(vec![0x09, 0x10, 0x02, 0x00, 0x00, 0x01, 0x01]));
        assert_eq!(message(&battery[..19]), None); // truncated
        assert_eq!(message(&hex("f0 00 41 00 53 00 54 00 52 00 4f 00 20 00 00 00 00")), None); // other report
    }

    #[test]
    fn requests_use_the_captured_framing() {
        let f = frame(&[0x02, 0x01]);
        assert_eq!(f, hex("f2 00 00 0c 1e 82 40 00 00 f2 00 03 00 00 00 00 02 01"));
        assert_eq!(message(&f), Some(vec![0x02, 0x01]));
    }

    #[test]
    fn battery_payload() {
        assert_eq!(parse_battery(&[0x43, 0x3c, 0x02]), Some((60, true))); // the captured event
        assert_eq!(parse_battery(&[100, 100, 3]), Some((100, false)));
        assert_eq!(parse_battery(&[1, 200, 0]), None);
        assert_eq!(parse_battery(&[1, 2]), None);
    }
}
