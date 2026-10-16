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
- [OBS] 2026-10-16 `kc_probe list` (Windows): first run saw 0 HID entries for 0x3434/0x0E20 (cause unknown, likely the keyboard was still enumerating); a run right after listed 7 entries, raw HID on interface 1, usage page 0xFF60 usage 0x61, product "Keychron K2 HE".
- [OBS] 2026-10-16 `kc_probe state --trace` (user-approved, read only): 176 requests in 388 ms, no retries, no foreign traffic, no notes. Every RX is 32 bytes (no leading report ID on Windows windows-native). Versions, timers, curve and profile globals equal the RE golden read; lighting effect now 6 (was 5). Profile 0 static 2.0 mm; profiles 1 and 2 rapid trigger 2.0 mm, 0.4/0.4. A8 06, 09 and 0C replies do not echo row or start (byte 2 = 0), so their match stays on bytes 0..1.
- [OBS] 2026-10-16 `kc_probe state --json`: 132 KB, no serial, no path.
- 2026-10-16 `kc_probe watch 30`: no unsolicited report in 30 s (no keys pressed). Second run while the user switched profile (Fn+P+Z/X/C) and effect: again no report. A `state` read right after: current profile 0, effect 6, the same as before. So either the final state equalled the start or the presses changed nothing; [UNK] whether the K2 HE sends A9 11 or 08 03 02 reports.
- [OBS] 2026-10-16 the user set a profile with the keyboard and changed the effect once, then `state`: current profile 1 (was 0), so A9 10 byte 2 follows the profile hotkey. Effect still 6 in VIA 08 03 02, consistent with what the user saw: the backlight flashed red briefly and returned to the previous effect (no effect change; the key pressed was probably not RGB_MOD, which sits on Fn+Q in the Win Fn layer, 0x7821 at r2c1). Combined with the watch runs: no unsolicited A9 11 seen while the user switched profiles [INF: the K2 HE does not push profile changes]; the app must re-read A9 10 to notice them.
