//! Logitech "Centurion" headsets behind their USB transmitter: ASTRO A30 (codename Carbonite).
//! Protocol facts from the Logitech G app (its BLE side) and HeadsetControl's Centurion notes (USB
//! side; GPL, facts only). A message is `[feature index][function << 4 | sw id][params]`, sent in a
//! frame `[prefix][len][flags = 0][message]` (len counts flags + message). Requests for the headset
//! go through the transmitter's bridge feature 0x0003. Battery: feature 0x0104, function 0, answers
//! `[real %, shown %, charging state]`.
// ponytail: the A30 framing on its 271-byte collection is inferred, not confirmed on hardware; both
// known prefixes are tried and --debug shows every byte. Pin the right one once a user confirms it.

use std::time::{Duration, Instant};

use windows_sys::Win32::Devices::HumanInterfaceDevice::HidP_Output;

use crate::hid::{self, Dev};
use crate::reading::{Kind, Reading};

const MODELS: &[(u16, u16, &str)] = &[(0x9886, 0x0050, "ASTRO A30"), (0x9886, 0x004f, "ASTRO A30")];
const SW: u8 = 1;
const BRIDGE: u16 = 0x0003;
const BATTERY: u16 = 0x0104;
const TIMEOUT: Duration = Duration::from_millis(1000);

pub fn read(out: &mut Vec<Reading>) {
    let mut done: Vec<u16> = Vec::new();
    for &(vid, pid, name) in MODELS {
        for info in hid::enumerate(vid) {
            // Only the vendor collection that takes output reports (ff03:0001 on the A30).
            if info.pid != pid || info.usage_page < 0xff00 || info.output_len < 64 || done.contains(&pid) {
                continue;
            }
            let Some(dev) = Dev::open(&info) else { continue };
            let Some(&id) = hid::report_ids(&info, HidP_Output).first() else { continue };
            for prefix in [vec![id], vec![id, 0x23]] {
                let link = Link { dev: &dev, prefix };
                if let Some((percent, charging)) = link.battery() {
                    done.push(pid);
                    out.push(Reading { id: format!("centurion:{pid:04x}"), name: name.into(), percent, charging,
                        kind: Kind::Headset, asleep: false });
                    break;
                }
            }
        }
    }
}

struct Link<'a> {
    dev: &'a Dev,
    prefix: Vec<u8>,
}

impl Link<'_> {
    fn battery(&self) -> Option<(u8, bool)> {
        hid::flush(self.dev);
        // Feature index 0 in a getFeature reply means "not there".
        let data = match *self.direct(0, 0, &BRIDGE.to_be_bytes())?.first()? {
            0 => {
                // No bridge: the headset itself is on the other end (plugged in by cable).
                let i = self.direct(0, 0, &BATTERY.to_be_bytes())?.first().copied().filter(|&i| i != 0)?;
                self.direct(i, 0, &[])?
            }
            b => {
                let i = self.bridged(b, 0, 0, &BATTERY.to_be_bytes())?.first().copied().filter(|&i| i != 0)?;
                self.bridged(b, i, 0, &[])?
            }
        };
        parse_battery(&data)
    }

    fn send(&self, msg: &[u8]) -> bool {
        self.dev.write(&frame(&self.prefix, msg))
    }

    /// Message of the next frame with our prefix, or None at `deadline`.
    fn recv(&self, deadline: Instant) -> Option<Vec<u8>> {
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            let r = self.dev.read(left.as_millis().max(1) as u32)?;
            if let Some(m) = message(&r, &self.prefix) {
                return Some(m);
            }
        }
    }

    /// A request to the transmitter itself; the reply's parameters.
    fn direct(&self, feat: u8, func: u8, params: &[u8]) -> Option<Vec<u8>> {
        let fsw = (func << 4) | SW;
        let mut msg = vec![feat, fsw];
        msg.extend_from_slice(params);
        if !self.send(&msg) {
            return None;
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let m = self.recv(deadline)?;
            if m.len() >= 3 && m[0] == 0xFF && m[1] == feat && m[2] == fsw {
                return None; // error reply
            }
            if m.len() >= 2 && m[0] == feat && m[1] == fsw {
                return Some(m[2..].to_vec());
            }
        }
    }

    /// A request to the headset through bridge feature index `b`; the reply's parameters.
    fn bridged(&self, b: u8, feat: u8, func: u8, params: &[u8]) -> Option<Vec<u8>> {
        let mut sub = vec![0, feat, (func << 4) | SW];
        sub.extend_from_slice(params);
        let mut msg = vec![b, 0x10 | SW, (sub.len() >> 8) as u8 & 0x0F, sub.len() as u8];
        msg.extend_from_slice(&sub);
        if !self.send(&msg) {
            return None;
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match bridge_reply(&self.recv(deadline)?, b, feat) {
                Reply::Data(d) => return Some(d),
                Reply::Rejected => return None,
                Reply::Other => continue,
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum Reply {
    Data(Vec<u8>),
    Rejected,
    /// The bridge's ack (our sw id), a connection event (function 0), someone else's message.
    Other,
}

/// Bridge message event: `[b][0x1 << 4 | sw][len hi][len lo][0][sub feature][fn | sw][data...]`.
fn bridge_reply(m: &[u8], b: u8, feat: u8) -> Reply {
    if m.len() >= 3 && m[0] == 0xFF && m[1] == b {
        return Reply::Rejected;
    }
    if m.len() < 7 || m[0] != b || m[1] >> 4 != 1 || m[1] & 0x0F == SW || m[4] != 0 {
        return Reply::Other;
    }
    match m[5] {
        0xFF if m[6] == feat => Reply::Rejected,
        f if f == feat => Reply::Data(m[7..].to_vec()),
        _ => Reply::Other,
    }
}

fn frame(prefix: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut f = prefix.to_vec();
    f.push(msg.len() as u8 + 1);
    f.push(0); // flags: single, unfragmented
    f.extend_from_slice(msg);
    f
}

/// The message inside a frame that starts with `prefix`.
fn message(r: &[u8], prefix: &[u8]) -> Option<Vec<u8>> {
    let rest = r.strip_prefix(prefix)?;
    let len = *rest.first()? as usize; // flags + message
    (len >= 2 && rest.len() > len).then(|| rest[2..1 + len].to_vec())
}

/// `[real %, shown %, state]`: the Logitech G app shows the second; states 1/2 = charging slow/fast.
fn parse_battery(d: &[u8]) -> Option<(u8, bool)> {
    let &[_, shown, state, ..] = d else { return None };
    (shown <= 100).then_some((shown, matches!(state, 1 | 2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let f = frame(&[0x51], &[0x00, 0x01, 0x00, 0x03]);
        assert_eq!(f, [0x51, 0x05, 0x00, 0x00, 0x01, 0x00, 0x03]);
        let mut padded = f.clone();
        padded.resize(64, 0);
        assert_eq!(message(&padded, &[0x51]), Some(vec![0x00, 0x01, 0x00, 0x03]));
        assert_eq!(message(&padded, &[0x51, 0x23]), None);
        assert_eq!(message(&[0x51, 0x09, 0x00], &[0x51]), None); // truncated
    }

    #[test]
    fn bridge_replies() {
        let b = 3;
        assert_eq!(bridge_reply(&[b, 0x11, 0, 3, 0, 5, 0x01], b, 5), Reply::Other); // ack: our sw id
        assert_eq!(bridge_reply(&[b, 0x00, 0, 1], b, 5), Reply::Other); // headset connected event
        assert_eq!(bridge_reply(&[b, 0x10, 0, 6, 0, 5, 0x01, 87, 85, 1], b, 5), Reply::Data(vec![87, 85, 1]));
        assert_eq!(bridge_reply(&[b, 0x10, 0, 6, 0, 0xFF, 5, 2], b, 5), Reply::Rejected);
        assert_eq!(bridge_reply(&[0xFF, b, 0x11, 4], b, 5), Reply::Rejected);
    }

    #[test]
    fn battery_payload() {
        assert_eq!(parse_battery(&[87, 85, 1]), Some((85, true)));
        assert_eq!(parse_battery(&[100, 100, 3]), Some((100, false)));
        assert_eq!(parse_battery(&[1, 200, 0]), None);
        assert_eq!(parse_battery(&[1, 2]), None);
    }
}
