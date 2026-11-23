/// What the device is; picks the glyph icon shown next to the number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Headset,
    Mouse,
    Keyboard,
    Gamepad,
    Speaker,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 6] = [Kind::Headset, Kind::Mouse, Kind::Keyboard, Kind::Gamepad, Kind::Speaker, Kind::Other];

    /// Best guess from free text (product names, Windows device categories).
    pub fn guess(text: &str) -> Kind {
        let t = text.to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
        if has(&["mouse", "viper", "deathadder", "basilisk", "orochi", "naga", "cobra", "mamba", "trackball"]) {
            Kind::Mouse
        } else if has(&["keyboard", "blackwidow", "huntsman", "ornata", "deathstalker", "keypad"]) {
            Kind::Keyboard
        } else if has(&["gamepad", "gaming", "controller", "joystick", "wolverine"]) {
            Kind::Gamepad
        } else if has(&["headset", "headphone", "earbud", "kraken", "blackshark", "barracuda", "nari"]) {
            Kind::Headset
        } else if has(&["speaker"]) {
            Kind::Speaker
        } else {
            Kind::Other
        }
    }
}

/// One battery reading. `id` is stable across polls and identifies the tray icon.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub id: String,
    pub name: String,
    pub percent: u8,
    pub charging: bool,
    pub kind: Kind,
    /// Paired but asleep: no fresh value, keep showing the last one.
    pub asleep: bool,
}

/// Maps a raw device value in `min..=max` to 0..=100, rounded and clamped.
pub fn scale(v: u8, min: u8, max: u8) -> u8 {
    if max <= min {
        return 0;
    }
    let (v, min, max) = (v.clamp(min, max) as u32, min as u32, max as u32);
    (((v - min) * 100 + (max - min) / 2) / (max - min)) as u8
}

/// Li-ion voltage (mV) to percent, piecewise linear (Logitech feature 0x1001).
pub fn volt_to_pct(mv: u16) -> u8 {
    const CURVE: [(u16, u8); 13] = [
        (4186, 100), (4067, 90), (3989, 80), (3922, 70), (3859, 60), (3811, 50), (3778, 40),
        (3751, 30), (3717, 20), (3671, 10), (3646, 5), (3579, 2), (3500, 0),
    ];
    if mv >= CURVE[0].0 {
        return 100;
    }
    for w in CURVE.windows(2) {
        let ((hv, hp), (lv, lp)) = (w[0], w[1]);
        if mv >= lv {
            let span = (hv - lv) as u32;
            return lp + (((mv - lv) as u32 * (hp - lp) as u32 + span / 2) / span) as u8;
        }
    }
    0
}

/// Text drawn inside the tray icon: two digits fit, 100 does not.
pub fn label(percent: u8) -> String {
    if percent >= 100 { "F".into() } else { percent.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_maps_and_clamps() {
        assert_eq!(scale(4, 0, 8), 50);
        assert_eq!(scale(8, 0, 8), 100);
        assert_eq!(scale(0x9A, 0x64, 0x9A), 100);
        assert_eq!(scale(0x50, 0x64, 0x9A), 0);
        assert_eq!(scale(200, 0, 100), 100);
        assert_eq!(scale(5, 3, 3), 0);
    }

    #[test]
    fn voltage_curve() {
        assert_eq!(volt_to_pct(4300), 100);
        assert_eq!(volt_to_pct(4186), 100);
        assert_eq!(volt_to_pct(3835), 55);
        assert_eq!(volt_to_pct(3500), 0);
        assert_eq!(volt_to_pct(3000), 0);
    }

    #[test]
    fn kind_guess() {
        assert_eq!(Kind::guess("Input.Mouse"), Kind::Mouse);
        assert_eq!(Kind::guess("Input.Gaming.Gamepad"), Kind::Gamepad);
        assert_eq!(Kind::guess("Communication.Headset.Bluetooth"), Kind::Headset);
        assert_eq!(Kind::guess("Razer Viper V2 Pro"), Kind::Mouse);
        assert_eq!(Kind::guess("Input.Keyboard"), Kind::Keyboard);
        assert_eq!(Kind::guess("Thing"), Kind::Other);
    }

    #[test]
    fn icon_label() {
        assert_eq!(label(100), "F");
        assert_eq!(label(93), "93");
        assert_eq!(label(7), "7");
    }
}
