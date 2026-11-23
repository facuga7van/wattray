# wattray

Battery level of your wireless peripherals, right in the Windows tray.

Your headset, mouse, keyboard or gamepad gets its own tray icon: a glyph for the device type on the left, the battery percentage on the right. No vendor apps, no installer, nothing running in the background except one ~305 KB exe that uses about 2 MB of memory.

## Features

- One icon per device: headset, mouse, keyboard, gamepad or speaker.
- Reads battery straight from the device over Bluetooth, Logitech HID++, SteelSeries and Razer protocols.
- Checks every 2 minutes, and right away when a device is plugged in or removed. Idle the rest of the time.
- One Windows notification when a device drops to 10 % (respects Do not disturb).
- Remembers the last values, so a sleeping mouse shows up right after boot. Rescans when the PC wakes from sleep.
- Starts with Windows and adds itself to the Start menu.
- Sharp hand-drawn pixel-art digits at 100–125 % display scaling.

## Install

1. Download `wattray.exe` from [Releases](https://github.com/facuga7van/wattray/releases).
2. Put it somewhere permanent, e.g. `%LOCALAPPDATA%\Programs\wattray\`. wattray registers itself from wherever it runs, so moving it later is fine: just run it again from the new place.
3. Run it. The exe is not code-signed, so SmartScreen may warn the first time: *More info → Run anyway*.

That's it. From now on it starts with Windows.

## Usage

| Action | What it does |
|---|---|
| Hover an icon | Shows the device name and exact percentage |
| Left click | Refreshes all devices now |
| Right click | Menu with Exit |

**Reading the icon**

| Look | Meaning |
|---|---|
| White (black on a light taskbar) | Battery above 20 % |
| Amber | 20 % or less |
| Red | 10 % or less |
| `F` | Full (100 %) |

While a device charges the icon stays white and the tooltip says "charging".

**Closed it by accident?** Search "wattray" in the Start menu and open it again.

**Icons hidden behind `^`?** Windows 11 hides new tray icons, so wattray pins each device's icon to the taskbar the first time it shows up. If you'd rather hide one, drag it into `^` or turn it off in *Settings → Personalization → Taskbar → Other system tray icons*. Windows remembers your choice and wattray never overrides it.

**Don't want it at startup?** Disable it in *Task Manager → Startup apps*. wattray respects that.

**Device asleep?** A paired Logitech device that is asleep keeps showing its last value, and wattray checks every 20 s until it wakes.

## Supported devices

| Protocol | Devices | Tested on |
|---|---|---|
| Bluetooth | Anything Windows shows a battery for in *Settings → Bluetooth* | Xbox Wireless Controller |
| Logitech HID++ 2.0 | Logitech mice, keyboards and headsets on Lightspeed, Unifying or Bolt receivers, or plugged in | PRO X Superlight 2 (Lightspeed) |
| SteelSeries | Arctis Nova Pro, Nova 7, Nova 5, Nova 3P/3X, Arctis 7+, Arctis 7 / Pro, Arctis 1 / 7X / 7P, Arctis 9 | Arctis Nova Pro Wireless |
| Razer | Wireless mice and keyboards using Razer's standard protocol | not yet |

Corsair, HyperX and generic 2.4 GHz dongles are not supported yet. Protocol details come from [HeadsetControl](https://github.com/Sapd/HeadsetControl), [Solaar](https://github.com/pwr-Solaar/Solaar) and [OpenRazer](https://github.com/openrazer/openrazer).

## Troubleshooting

Run these from a terminal:

```
wattray.exe --once     # print what it reads and exit
wattray.exe --debug    # same, plus every HID report in hex
```

If a device is missing or reads wrong, [open an issue](https://github.com/facuga7van/wattray/issues) with the `--debug` output.

## Uninstall

1. Right click any wattray icon → Exit.
2. Delete `wattray.exe` and `%APPDATA%\Microsoft\Windows\Start Menu\Programs\wattray.lnk`.
3. In the registry, delete the `wattray` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` and the key `HKCU\Software\wattray`.

## Build

```
cargo build --release
```

Rust stable, MSVC toolchain. The CRT is linked statically, so the exe runs without the VC++ redistributable.

## License

[MIT](LICENSE)
