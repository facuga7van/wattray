//! Glorious Model O / O- / D / D- Wireless on their 2.4 GHz receivers (Sinowealth firmware, VID 0x258A).
//!
//! Protocol: a 65-byte feature report (id 0 + 64) `00 | 00 00 02 02 00 83 | 00...` is sent to
//! interface 2 (DevSel 0x02 = wireless mouse, CmdType 0x02 = battery, Func 0x83 = battery status);
//! after ~100 ms (receiver <-> mouse RF round trip) the same report is read back:
//! `[0]` id, `[1]` status (A1 awake, A2 waking, A4 asleep), `[6]` 0x83, `[7]` charging (non-zero),
//! `[8]` level 0..=100. Only an awake A1 reply is a reading: asleep/waking/echo/foreign frames give none.
//!
//! Sources (2 independent for the request, offsets, interface and PIDs unless noted):
//! - korkje/mxw `src/report/battery.rs` + `src/util/devices.rs` (MIT): request bytes, offsets, interface 2,
//!   wireless PIDs 2022/2024/2023/2025, status codes. Its wait is 50 ms.
//! - Rodrigo-200/GloriousBatteryMonitor-Go `HidDeviceService.cs` / `DeviceDatabase.cs` (no license file:
//!   facts only, no code taken): same request and offsets, 100 ms wait, same four wireless PIDs.
//! - HeyOkay/HaloBattery `providers/glorious.py` (MIT): same, plus the 65-byte feature length and the
//!   "only A1 is live" rule. Confirmed on hardware there for the Model O; D and D- untried.
//!
//! Single source only: the 65-byte feature length filter (Halo); that 0x83 is read-only (all three
//! sources only ever send this one query; none documents it as anything else).
//! Not verified on hardware by wattray; the test replies are built from the documented layout,
//! not raw captures.

use std::thread::sleep;
use std::time::Duration;

use crate::hid::{self, Dev};
use crate::reading::{Kind, Reading};

const VID: u16 = 0x258A;
/// Wireless receivers only. 0x258A is shared with unrelated Sinowealth keyboards: exact PIDs only.
// ponytail: wired ids (2011/2013/2012/2014) and Model I/O2/D2/Pro are skipped: the wired mouse leaves the
// receiver link and the other models have no cross-checked source.
const MODELS: [(u16, &str); 4] = [
    (0x2022, "Glorious Model O Wireless"),
    (0x2024, "Glorious Model O- Wireless"),
    (0x2023, "Glorious Model D Wireless"),
    (0x2025, "Glorious Model D- Wireless"),
];
const IFACE: u8 = 2;
const FEATURE_LEN: u16 = 65;
const AWAKE: u8 = 0xA1;
const WAKING: u8 = 0xA2;
const FUNC: u8 = 0x83;

pub fn read(out: &mut Vec<Reading>) {
    let mut done: Vec<u16> = Vec::new();
    for info in hid::enumerate(VID) {
        let Some(&(_, name)) = MODELS.iter().find(|m| m.0 == info.pid) else { continue };
        if info.mi != Some(IFACE) || info.feature_len != FEATURE_LEN || done.contains(&info.pid) {
            continue;
        }
        let Some(dev) = Dev::open(&info) else { continue };
        let Some((percent, charging)) = query(&dev) else { continue };
        done.push(info.pid);
        out.push(Reading {
            id: format!("sino:{:04x}:{:04x}", info.vid, info.pid),
            name: name.into(),
            percent,
            charging,
            kind: Kind::Mouse,
            asleep: false,
        });
    }
}

/// Sends the battery query, waits for the RF round trip, parses the reply (one retry while it wakes).
fn query(dev: &Dev) -> Option<(u8, bool)> {
    let mut req = [0u8; FEATURE_LEN as usize];
    req[3] = 0x02;
    req[4] = 0x02;
    req[6] = FUNC;
    for _ in 0..2 {
        if !dev.set_feature(&req) {
            return None;
        }
        sleep(Duration::from_millis(100));
        let r = dev.get_feature(0)?;
        if let Some(v) = parse(&r) {
            return Some(v);
        }
        if !(r.len() > 8 && r[6] == FUNC && r[1] == WAKING) {
            return None; // asleep, foreign frame (another app wrote the report) or garbage: no retry
        }
    }
    None
}

/// `(percent, charging)` from a full reply (byte 0 = report id), only for an awake mouse.
fn parse(r: &[u8]) -> Option<(u8, bool)> {
    if r.len() < 9 || r[0] != 0 || r[6] != FUNC || r[1] != AWAKE || r[8] > 100 {
        return None;
    }
    Some((r[8], r[7] != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u8, charging: u8, level: u8) -> Vec<u8> {
        let mut r = vec![0, status, 0, 0, 0, 0, FUNC, charging, level];
        r.resize(65, 0);
        r
    }

    #[test]
    fn awake_reply() {
        assert_eq!(parse(&reply(AWAKE, 0, 75)), Some((75, false)));
        assert_eq!(parse(&reply(AWAKE, 1, 75)), Some((75, true)));
        assert_eq!(parse(&reply(AWAKE, 2, 100)), Some((100, true))); // 2/3 = full on cable, still on charger
        assert_eq!(parse(&reply(AWAKE, 0, 0)), Some((0, false))); // genuine awake 0%
    }

    #[test]
    fn asleep_waking_and_unknown_status_are_no_reading() {
        for s in [0x00, 0xA0, WAKING, 0xA3, 0xA4, 0xFF] {
            assert_eq!(parse(&reply(s, 0, 80)), None, "status {s:#04x}");
        }
    }

    #[test]
    fn request_echo_and_foreign_frames_are_no_reading() {
        // our own request read back: 83 at byte 6, zeros elsewhere
        let mut echo = vec![0u8; 65];
        echo[3] = 0x02;
        echo[4] = 0x02;
        echo[6] = FUNC;
        assert_eq!(parse(&echo), None);
        // SignalRGB status frame seen in HaloBattery #281: a1 00 02 08 02 00 01 ff (id prepended)
        let mut rgb = vec![0, 0xA1, 0x00, 0x02, 0x08, 0x02, 0x00, 0x01, 0xFF];
        rgb.resize(65, 0);
        assert_eq!(parse(&rgb), None);
    }

    #[test]
    fn malformed_replies() {
        assert_eq!(parse(&reply(AWAKE, 0, 101)), None);
        assert_eq!(parse(&reply(AWAKE, 0, 255)), None);
        let mut bad_func = reply(AWAKE, 0, 50);
        bad_func[6] = 0x81;
        assert_eq!(parse(&bad_func), None);
        let mut bad_id = reply(AWAKE, 0, 50);
        bad_id[0] = 4;
        assert_eq!(parse(&bad_id), None);
        for n in 0..9 {
            assert_eq!(parse(&reply(AWAKE, 0, 50)[..n]), None);
        }
    }
}
