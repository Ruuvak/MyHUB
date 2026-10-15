# MyHUB

MyHUB is a lightweight peripheral manager for Windows and Linux that runs in the tray, starts with the system, and talks to devices directly. It replaces vendor suites for the settings people change every day: DPI, report rate, onboard profiles, noise cancelling, equalizer and battery status. Two devices are supported: the Logitech G502 X LIGHTSPEED mouse over HID++ 2.0, with no Logitech G HUB required, and the Sony WF-C710N earbuds over Bluetooth RFCOMM, with no Sony Headphones Connect phone app required.

<img width="1411" height="1391" alt="obraz" src="https://github.com/user-attachments/assets/55d35734-fc16-4b00-b07a-20b36d13561e" />


## Status

Version 0.1.0, under active development. Windows is the tested platform. A Linux build for Kubuntu (KDE Plasma) is planned and not yet tested; the Bluetooth transport for the Sony earbuds is Windows only for now.

## What works on the G502 X LIGHTSPEED

| Area | Behaviour |
|---|---|
| Connection | Detects the receiver, follows the mouse when it is switched off or wakes up, and rediscovers features at runtime |
| Battery | Live percent, level and charging state from feature 0x1004, updated from the mouse's own events |
| Profile mode | Edits DPI levels, default and shift DPI index and report rate of the MyHUB profile and saves them to the mouse; buttons, profiles and lighting keep working on the mouse |
| Live state | DPI and profile changes made with the mouse buttons appear in the app immediately |
| App control mode | The app sets DPI and report rate at runtime; the mouse DPI buttons step through the profile levels via feature 0x8110 |
| Start up | Optional launch at login, hidden in the tray, restoring the last mode and DPI |

Button assignments are shown read only.

## What works on the Sony WF-C710N

| Area | Behaviour |
|---|---|
| Connection | Finds the paired earbuds, connects to the control channel, and reconnects on its own after the earbuds go back in the case |
| Battery | Left, right and case percent with charging state, updated from the earbuds' own notifications and shown in the tray menu |
| Sound mode | Noise cancelling or ambient sound, ambient mode normal or voice, ambient level 1 to 20 |
| Equalizer | Preset selection, and the five bands from 400 Hz to 16 kHz plus Clear Bass on a -10 to +10 dB scale, saved to the Custom equalizer |
| Settings | Sidetone on or off, multipoint on or off (the earbuds reconnect after a multipoint change), codec and firmware shown |

Every change is read back from the earbuds before the app reports it as saved.

## Common to both devices

Tray flyout with a device switcher, main window, tray menu with the battery of each device, light and dark themes, Polish and English. Device changes are off until "Allow changes on the device" is switched on in Settings.

## Hardware safety

The app never touches firmware, and every request passes a per device allowlist before a byte is sent.

G502 X: feature indexes are resolved at runtime from the Root feature. Firmware update features and hidden vendor features are blocked outright. Flash writes are limited to DPI levels, DPI indexes and report rate in user profile sectors, run as whole sector writes with a backup to disk first, then a byte for byte read back and CRC check, with an automatic restore on mismatch. ROM profile sectors are never written. MyHUB uses its own HID++ software ID, so it can run next to G HUB without taking its replies.

WF-C710N: the supported functions and their variants are read from the earbuds at connect time. Firmware update, factory reset, factory power off and paired device management commands are blocked outright. Reads are the default; setting changes and the multipoint change, which disconnects the earbuds for a moment, each need their own gate, opened only for that one request. Multipoint is changed only after the user confirms it in the app.

## Architecture

| Path | Contents |
|---|---|
| `crates/hidpp` | HID++ protocol: frames, feature table, safety policy, CRC, onboard profile parsing and sector patching. No I/O |
| `crates/transport` | hidapi transport and HID++ session (Windows collections, Linux hidraw) |
| `crates/mdr` | Sony headphone control protocol: frames, acknowledgement and sequence handling, supported function table, messages, safety policy. No I/O |
| `crates/rfcomm` | Bluetooth RFCOMM byte link (Windows Winsock; Linux not yet implemented) |
| `crates/core` | Types shared between the backend and the UI |
| `crates/devices/logitech-g502x` | G502 X module: snapshot read, detail mapping, runtime settings, profile writes, button spy, `hw_probe` tool |
| `crates/devices/sony-wf-c710n` | WF-C710N module: session, snapshot read, detail mapping, setting writes with read back, `mdr_probe` tool |
| `src-tauri` | Tauri 2 app: one worker per device, tray, flyout, settings, commands |
| `src` | React and TypeScript UI |

Stack: Tauri 2, Rust (edition 2024), React 19, TypeScript, Vite, Tailwind CSS v4, hidapi, windows-sys.

## Development

Requirements: Rust stable (MSVC on Windows), Bun, Node 24.

```bash
bun install
bun run tauri dev
```

Checks:

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
bun run typecheck
```

Tests run against recorded device frames and mocks only, and no test opens a real device. Two tools read a connected device without writing anything: `cargo run -p logitech-g502x --example hw_probe -- state` for the mouse and `cargo run -p sony-wf-c710n --example mdr_probe -- state` for the earbuds. Browser previews of the UI without a device: `?demo=g502x`, `?demo=sony` and `?demo=all` on the Vite dev server.

## Disclaimer

MyHUB is an independent project and is not affiliated with or endorsed by Logitech or Sony. Logitech, G HUB, LIGHTSPEED, Sony, Headphones Connect and Clear Bass are trademarks of their owners. The repository contains no Logitech or Sony software, images or firmware.
