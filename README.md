# MyHUB

MyHUB is a lightweight peripheral manager for Windows and Linux that runs in the tray, starts with the system, and talks to devices directly over HID. It replaces vendor suites for the settings people change every day: DPI, report rate, onboard profiles and battery status. The first supported device is the Logitech G502 X LIGHTSPEED, driven over HID++ 2.0 through its LIGHTSPEED receiver, with no Logitech G HUB required.

## Status

Version 0.1.0, under active development. Windows is the tested platform. A Linux build for Kubuntu (KDE Plasma) is planned and not yet tested.

## What works on the G502 X LIGHTSPEED

| Area | Behaviour |
|---|---|
| Connection | Detects the receiver, follows the mouse when it is switched off or wakes up, and rediscovers features at runtime |
| Battery | Live percent, level and charging state from feature 0x1004, updated from the mouse's own events |
| Profile mode | Edits DPI levels, default and shift DPI index and report rate of the MyHUB profile and saves them to the mouse; buttons, profiles and lighting keep working on the mouse |
| Live state | DPI and profile changes made with the mouse buttons appear in the app immediately |
| App control mode | The app sets DPI and report rate at runtime; the mouse DPI buttons step through the profile levels via feature 0x8110 |
| Start up | Optional launch at login, hidden in the tray, restoring the last mode and DPI |
| Interface | Tray flyout and main window, light and dark themes, Polish and English |

Button assignments are shown read only.

## Hardware safety

The app never touches firmware. Every HID++ request passes a per function allowlist before a byte is sent, and feature indexes are resolved at runtime from the Root feature. Firmware update features and hidden vendor features are blocked outright. Flash writes are limited to DPI levels, DPI indexes and report rate in user profile sectors, run as whole sector writes with a backup to disk first, then a byte for byte read back and CRC check, with an automatic restore on mismatch. ROM profile sectors are never written. MyHUB uses its own HID++ software ID, so it can run next to G HUB without taking its replies.

## Architecture

| Path | Contents |
|---|---|
| `crates/hidpp` | HID++ protocol: frames, feature table, safety policy, CRC, onboard profile parsing and sector patching. No I/O |
| `crates/transport` | hidapi transport and HID++ session (Windows collections, Linux hidraw) |
| `crates/core` | Types shared between the backend and the UI |
| `crates/devices/logitech-g502x` | G502 X module: snapshot read, detail mapping, runtime settings, profile writes, button spy, `hw_probe` tool |
| `src-tauri` | Tauri 2 app: device worker, tray, flyout, settings, commands |
| `src` | React and TypeScript UI |

Stack: Tauri 2, Rust (edition 2024), React 19, TypeScript, Vite, Tailwind CSS v4, hidapi.

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

Tests run against recorded device frames and mocks only, and no test opens a real device. `cargo run -p logitech-g502x --example hw_probe -- state` reads the live state of a connected mouse without writing anything.

## Disclaimer

MyHUB is an independent project and is not affiliated with or endorsed by Logitech. Logitech, G HUB and LIGHTSPEED are trademarks of their owners. The repository contains no Logitech software, images or firmware.
