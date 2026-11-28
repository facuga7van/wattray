//! Logitech G HUB's local WebSocket (ws://localhost:9010). Unofficial; protocol as used by
//! LGSTrayBattery and LGBattery. Only answers while G HUB runs. Covers what G HUB knows but HID++
//! cannot reach: ASTRO headsets (vendor 0x9886), devices whose collections we cannot talk to.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::hid;
use crate::reading::{Kind, Reading};

pub fn read(out: &mut Vec<Reading>) {
    let Some(mut ws) = connect() else { return };
    let Some(list) = request(&mut ws, "/devices/list") else { return };
    let Some(Json::Arr(devices)) = list.get("payload").and_then(|p| p.get("deviceInfos")) else { return };
    // Kinds HID++ already read: G HUB would show the same device a second time.
    // ponytail: matched by kind, not identity; a 2nd Logitech mouse only G HUB reaches is skipped. Match by name if one shows up.
    let taken: Vec<Kind> = out.iter().filter(|r| r.id.starts_with("logi:") && !r.asleep).map(|r| r.kind).collect();
    for d in devices {
        if d.get("capabilities").and_then(|c| c.get("hasBatteryStatus")) != Some(&Json::Bool(true)) {
            continue;
        }
        let (Some(id), Some(name)) = (d.get("id").and_then(Json::str), d.get("extendedDisplayName").and_then(Json::str)) else {
            continue;
        };
        let kind = d.get("deviceType").and_then(Json::str).map_or(Kind::Other, Kind::guess);
        if taken.contains(&kind) {
            continue;
        }
        let Some(state) = request(&mut ws, &format!("/battery/{id}/state")) else { continue };
        let Some(p) = state.get("payload") else { continue };
        let Some(Json::Num(pct)) = p.get("percentage") else { continue };
        out.push(Reading {
            id: format!("ghub:{name}"), // G HUB's own ids change between sessions; the name does not
            name: name.into(),
            percent: pct.clamp(0.0, 100.0) as u8,
            charging: p.get("charging") == Some(&Json::Bool(true)),
            kind,
            asleep: false,
        });
    }
}

fn connect() -> Option<TcpStream> {
    let addr: SocketAddr = "127.0.0.1:9010".parse().ok()?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    // No permessage-deflate offered, so frames come uncompressed. The key is not checked by anyone here.
    s.write_all(
        b"GET / HTTP/1.1\r\nHost: localhost:9010\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
          Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
          Sec-WebSocket-Protocol: json\r\nOrigin: file://\r\n\r\n",
    )
    .ok()?;
    // Byte by byte up to the blank line, so no frame bytes are swallowed with the HTTP head.
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut b).ok()?;
        head.push(b[0]);
        if head.len() > 8192 {
            return None;
        }
    }
    head.starts_with(b"HTTP/1.1 101").then_some(s)
}

/// GET `path`; the reply carrying the same path, if G HUB reports SUCCESS.
fn request(ws: &mut TcpStream, path: &str) -> Option<Json> {
    send(ws, format!(r#"{{"msgId":"","verb":"GET","path":"{path}"}}"#).as_bytes())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let text = recv(ws)?;
        if hid::DEBUG.load(Ordering::Relaxed) {
            println!("  ghub {path} < {}", text.chars().take(400).collect::<String>());
        }
        let Some(j) = Json::parse(&text) else { continue };
        if j.get("path").and_then(Json::str) == Some(path) {
            let ok = j.get("result").and_then(|r| r.get("code")).and_then(Json::str) == Some("SUCCESS");
            return ok.then_some(j);
        }
    }
    None
}

/// One masked text frame (clients must mask; any key works).
fn send(s: &mut TcpStream, data: &[u8]) -> Option<()> {
    s.write_all(&frame(data)).ok()
}

fn frame(data: &[u8]) -> Vec<u8> {
    let mask = [0x37, 0xfa, 0x21, 0x3d];
    let mut f = vec![0x81]; // FIN + text
    match data.len() {
        n if n < 126 => f.push(0x80 | n as u8),
        n if n < 65536 => {
            f.push(0x80 | 126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            f.push(0x80 | 127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    f.extend_from_slice(&mask);
    f.extend(data.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

/// Next text message, fragments joined, control frames skipped. None on close, error or timeout.
fn recv(s: &mut TcpStream) -> Option<String> {
    let mut msg = Vec::new();
    loop {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).ok()?;
        if h[1] & 0x80 != 0 {
            return None; // servers never mask
        }
        let len = match h[1] {
            126 => {
                let mut b = [0u8; 2];
                s.read_exact(&mut b).ok()?;
                u16::from_be_bytes(b) as usize
            }
            127 => {
                let mut b = [0u8; 8];
                s.read_exact(&mut b).ok()?;
                u64::from_be_bytes(b) as usize
            }
            n => n as usize,
        };
        if len > 16 << 20 {
            return None;
        }
        let mut data = vec![0u8; len];
        s.read_exact(&mut data).ok()?;
        match h[0] & 0x0f {
            8 => return None,
            0..=2 => {
                msg.extend_from_slice(&data);
                if h[0] & 0x80 != 0 {
                    return String::from_utf8(msg).ok();
                }
            }
            _ => {} // ping/pong: a session this short needs no reply
        }
    }
}

/// Just enough JSON for G HUB's replies.
#[derive(Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn parse(s: &str) -> Option<Json> {
        let mut p = Parser { b: s.as_bytes(), i: 0 };
        let v = p.value()?;
        p.ws();
        (p.i == p.b.len()).then_some(v)
    }

    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(o) => o.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        let hit = self.b.get(self.i) == Some(&c);
        if hit {
            self.i += 1;
        }
        hit
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut o = Vec::new();
                if self.eat(b'}') {
                    return Some(Json::Obj(o));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    if !self.eat(b':') {
                        return None;
                    }
                    o.push((k, self.value()?));
                    if !self.eat(b',') {
                        return self.eat(b'}').then_some(Json::Obj(o));
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                if self.eat(b']') {
                    return Some(Json::Arr(a));
                }
                loop {
                    a.push(self.value()?);
                    if !self.eat(b',') {
                        return self.eat(b']').then_some(Json::Arr(a));
                    }
                }
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            b'n' => self.word("null", Json::Null),
            _ => {
                let start = self.i;
                while self.b.get(self.i).is_some_and(|c| matches!(c, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[start..self.i]).ok()?.parse().ok().map(Json::Num)
            }
        }
    }

    fn word(&mut self, w: &str, v: Json) -> Option<Json> {
        if !self.b[self.i..].starts_with(w.as_bytes()) {
            return None;
        }
        self.i += w.len();
        Some(v)
    }

    fn string(&mut self) -> Option<String> {
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            // ponytail: surrogate pairs (emoji) become U+FFFD; names and ids never need them.
                            let ch = char::from_u32(u32::from_str_radix(hex, 16).ok()?).unwrap_or('\u{fffd}');
                            out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        other => out.push(other), // \" \\ \/
                    }
                }
                _ => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ghub_replies() {
        let list = Json::parse(
            r#"{"msgId":"","verb":"GET","path":"/devices/list","origin":"backend","result":{"code":"SUCCESS"},
            "payload":{"deviceInfos":[{"id":"dev00000003","extendedDisplayName":"ASTRO A30","deviceType":"HEADSET",
            "capabilities":{"hasBatteryStatus":true,"hasLeds":false},"pid":-1.5e1,"tag":"a\"bé"}]}}"#,
        )
        .unwrap();
        let Some(Json::Arr(d)) = list.get("payload").and_then(|p| p.get("deviceInfos")) else { panic!() };
        assert_eq!(d[0].get("extendedDisplayName").and_then(Json::str), Some("ASTRO A30"));
        assert_eq!(d[0].get("capabilities").and_then(|c| c.get("hasBatteryStatus")), Some(&Json::Bool(true)));
        assert_eq!(d[0].get("pid"), Some(&Json::Num(-15.0)));
        assert_eq!(d[0].get("tag").and_then(Json::str), Some("a\"bé"));
        assert_eq!(Kind::guess("HEADSET"), Kind::Headset);
        assert_eq!(Json::parse(r#"{"a":[1,2,]}"#), None);
        assert_eq!(Json::parse("[] x"), None);
    }

    #[test]
    fn frames_are_masked_with_length() {
        let f = frame(b"hi");
        assert_eq!(&f[..2], &[0x81, 0x82]);
        assert_eq!(f[6] ^ f[2], b'h');
        assert_eq!(frame(&[0; 300])[1..4], [0x80 | 126, 0x01, 0x2c]);
    }
}
