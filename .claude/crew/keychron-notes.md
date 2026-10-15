# Keychron K2 HE module notes

Reference materials (read-only, never copy into the repo): `D:\!KodowankoAI\REA\.claude\keychron-k2he\`. Start at FEATURES.md; protocol in features/he.md, lighting.md, keymap-macros.md, system.md; golden replies in hw-read-2026-10-15.md. Never copy `k2he-v3.json`, `keycodes.json`, `he-codec-v4.js`, `he-region.js` or anything from the launcher bundle. Golden device replies may be transcribed into test fixtures. Write the keycode table and the 84-key drawing from scratch (QMK public values).

Evidence tags: [OBS] seen on the real keyboard, [INF] launcher code or inference (verify on hardware before code relies on it), [UNK] gap.

## Decisions (user, 2026-10-16)
- Scope v1: read everything; writes only for HE profile select, global actuation and Rapid Trigger, global lighting (VIA channel 3). No calibration. Keychron Link receiver (monitor only) later.
- Configuration over USB only. Hardware tests with the keyboard on the cable.
- Order: S0 device_manager generalisation, S1 `crates/keychron-proto` (pure), S2 `crates/devices/keychron-k2he` (hidapi link, session, `kc_probe` read-only), S3 app read-only, S4 writes one area at a time.

## Protocol summary
- [OBS] VID 0x3434, PID 0x0E20, raw HID usagePage 0xFF60 usage 0x61. 32-byte frames, report ID 0, byte 0 = command, reply echoes byte 0 (usually byte 1). No sequence number.
- [OBS] Over Keychron Link every keyboard frame times out.
- [INF] Launcher: one frame in flight, 1 s timeout, 3 resends. Retry reads only.
- Parallel launcher: replies cannot be told apart. MyHUB never polls; reads at connect and on user action; an unrequested report means foreign traffic, back off and send nothing.
- [OBS] VIA 12, kcVersion 2, instruction set 2, HE version 4 feature byte 0, firmware v1.2.1; 4 layers, 6x16, 84 LEDs, 3 HE profiles x 860 B, 20 DKS, 20 snap. A7 05, 0D, 12 give no reply (unsupported). No battery command.
- [OBS] A9 15 changes RAM only; replug discards it. GET_PROFILE_BUFFER reads RAM.
- [INF] A9 14/11/13 RAM, A9 1F saves the current profile to flash; VIA 07 needs 09 03; A8 writes need A8 02; VIA 05 keymap persists immediately; A9 16 saves itself.

## Allowlist tiers
- Read: VIA 01, 02 04, 02 03, 08 03 xx, 0C, 0D, 0E, 11, 12; A0, A1, A2, A3; A7 01, 03, 0B; A8 01, 03, 05, 06, 07, 09, 0B, 0C, 0E; A9 01, 10, 12, 20, 22, 30, 41.
- Volatile: A9 11, 13, 14, 15; VIA 07; A8 04, 08, 0A, 0D, 0F.
- Persistent: A9 1F, 16, 1E, 21; VIA 09 03, 05, 06, 0F; A8 02; A7 04, 0C.
- Never: VIA 0A, 0B; A7 15; every 0xAB; 0xBA; the 0x8C wireless collection. Unsupported on K2 HE and blocked: A7 05/06, 07..0A, 0D/0E, 0F/10, 11..13, 14, 16..1C; A9 1D, 23, 40, 42. Anything unknown is refused before a byte is sent.

## Hardware log
(empty)
