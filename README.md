# pila

Battery level of your wireless peripherals, in the Windows tray. One icon per device: what it is on the left (headset, mouse, keyboard, gamepad, speaker), the percentage on the right.

- ~290 KB single `.exe`, no installer, no runtime, ~2 MB of private memory.
- Polls every 2 minutes and right away when a device is plugged or unplugged. Idle the rest of the time.
- Starts with Windows (disable it in Task Manager → Startup apps).
- Hover an icon for the device name. Left click: refresh now. Right click: Exit.
- One Windows notification when a device drops to 10 % (respects Do not disturb).
- Remembers the last values, so a sleeping mouse shows up right after boot. Rescans after the PC wakes from sleep.

Colors: white (black on a light taskbar), amber at 20 % or less, red at 10 % or less. `F` means full. At 100–125 % display scaling the icon is hand-drawn pixel art, so it stays sharp at 16 px.

## Supported devices

| Protocol | Devices | Tested on |
|---|---|---|
| Bluetooth | Anything Windows shows a battery for in Settings → Bluetooth | Xbox Wireless Controller |
| Logitech HID++ 2.0 | Logitech mice, keyboards and headsets on Lightspeed, Unifying or Bolt receivers, or plugged in | PRO X Superlight 2 (Lightspeed) |
| SteelSeries | Arctis Nova Pro, Nova 7, Nova 5, Nova 3P/3X, Arctis 7+, Arctis 7 / Pro, Arctis 1 / 7X / 7P, Arctis 9 | Arctis Nova Pro Wireless |
| Razer | Wireless mice and keyboards using Razer's standard protocol | not yet |

Corsair, HyperX and generic 2.4 GHz dongles are not supported yet. Protocol details come from [HeadsetControl](https://github.com/Sapd/HeadsetControl), [Solaar](https://github.com/pwr-Solaar/Solaar) and [OpenRazer](https://github.com/openrazer/openrazer).

A Logitech device that is paired but asleep keeps showing its last value, and pila checks every 20 s until it wakes.

## Install

1. Download `pila.exe` from [Releases](https://github.com/facuga7van/pila/releases) and put it somewhere permanent, e.g. `%LOCALAPPDATA%\Programs\pila\`.
2. Run it. The exe is not code-signed, so SmartScreen may warn the first time: *More info → Run anyway*.
3. pila puts its icons on the taskbar the first time each device shows up (Windows 11 would hide them behind `^`). Drag one into `^`, or switch it off in Settings → Personalization → Taskbar → Other system tray icons, and Windows remembers: pila never overrides that.

To uninstall: right click → Exit, delete the exe, and delete the `pila` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` and the key `HKCU\Software\pila`.

## Troubleshooting

```
pila.exe --once     # print what it reads and exit
pila.exe --debug    # same, plus every HID report in hex
```

Run them from a terminal. If a device reads wrong, open an issue with the `--debug` output.

## Build

```
cargo build --release
```

Rust stable, MSVC toolchain. The CRT is linked statically, so the exe runs without the VC++ redistributable.
