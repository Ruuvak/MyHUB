# F4 IPC contract (Rust <-> UI). JSON is camelCase; enums are lowercase/camelCase strings as written.

## Types
DeviceSummary {
  id: string                      // stable, no serial numbers. G502 X: "logitech-g502x"
  name: string                    // "G502 X LIGHTSPEED"
  kind: "mouse" | "keyboard" | "headset" | "gamepad"
  connection: "connected" | "disconnected"   // disconnected = paired but switched off / asleep (HID++ 1.0 0x41 link down)
  batteryPercent: number | null
}

DeviceDetail = DeviceSummary & {
  vendor: string                  // "Logitech"
  transport: string               // "LIGHTSPEED"
  protocol: string                // "HID++ 2.0"
  readOnly: boolean               // true in F4 (no Set* calls exist yet)
  updatedAtMs: number             // unix ms of the last successful read
  illustration: string | null     // UI illustration key, "g502x" for this mouse
  battery: { percent: number } | null
  dpi: { min: number; max: number; step: number; current: number } | null
  reportRate: { optionsHz: number[]; currentHz: number } | null
  onboard: {
    mode: "host" | "onboard" | null          // 0x8100 fn2: 2 host, 1 onboard, else null
    activeProfile: number | null             // 0x8100 fn4 sector 0x0001..0x0005 -> 1..5, else null
    buttonCount: number                      // from 0x8100 info (11)
    profiles: ProfileInfo[]                  // user profiles 1..N, then ROM profiles
  } | null
}

ProfileInfo {
  id: string                      // "user-1".."user-5", "rom-1", "rom-2"
  number: number                  // 1..5 for user, 1..2 for rom
  rom: boolean
  enabled: boolean                // from directory sector 0x0000 (ROM: true)
  name: string | null             // UTF-16 name when present
  reportRateHz: number | null     // 1000 / interval byte (1,2,4,8 ms), null if invalid
  dpiLevels: number[]             // 5 values
  defaultDpiIndex: number
  shiftDpiIndex: number
  buttons: BindingInfo[]          // only the first buttonCount slots
  gshift: BindingInfo[]           // only the first buttonCount slots
  crcOk: boolean                  // ROM sectors store CRC 0, report crcOk=false but still show
}

BindingInfo {
  slot: number                    // 0-based slot index; label in UI is "G{slot+1}"
  kind: "mouse" | "keyboard" | "consumer" | "special" | "unassigned" | "unknown"
  mouseButtons: number | null     // bitmask: 0x01 left, 0x02 right, 0x04 middle, 0x08 back, 0x10 forward
  modifiers: number | null        // HID modifier bitmask (0x01 LCtrl, 0x02 LShift, 0x04 LAlt, 0x08 LGui, 0x10 RCtrl, 0x20 RShift, 0x40 RAlt, 0x80 RGui)
  usage: number | null            // keyboard: HID keyboard usage; consumer: consumer usage
  special: "wheelLeft" | "wheelRight" | "dpiUp" | "dpiDown" | "dpiShift" | "profileCycle" | "gShift" | null
  specialCode: number | null      // raw special code, also set for unknown specials (0x10, 0x11 are [UNK])
  rawHex: string                  // the 4 raw bytes, e.g. "90070000"
}

## Commands (Tauri invoke)
list_devices() -> DeviceSummary[]
get_device_detail({ id }) -> DeviceDetail | null
refresh_device({ id }) -> DeviceDetail | null     // re-reads the device now (read tier only)
open_main_window({ deviceId?: string })           // shows + focuses main window, optionally selects device (emits "select-device" with id)
hide_flyout()
(existing: app_info, get_settings, set_settings)

## Events
"devices-changed"        payload DeviceSummary[]
"device-detail-changed"  payload DeviceDetail
"select-device"          payload string (device id), sent to the main window
"settings-changed"       payload Settings (existing)

## F5 additions (volatile settings, route a)
Settings gains: allowDeviceWrites: boolean (default false), restoreOnConnect: boolean (default false).
DeviceDetail.readOnly = !(settings.allowDeviceWrites && connected). The mode is not part of it.
Host mode: a write while onboard.mode == "onboard" first switches the mouse to host (0x8100 fn1, verified), then writes; onboard.mode then shows "host". If the switch fails the error is "notHostMode". The app returns the mouse to onboard (best effort, one attempt) when allowDeviceWrites is turned off, on quit and on shutdown, but only if it switched it to host itself. If the mouse resets to onboard on its own (link-up), the app forgets it switched it.
New commands:
set_dpi({ id, dpi }) -> DeviceDetail          // error string on refusal/failure
set_report_rate({ id, hz }) -> DeviceDetail   // hz in reportRate.optionsHz
get_device_prefs({ id }) -> DevicePrefs | null
DevicePrefs { dpi: number | null; reportRateHz: number | null }   // last values the user applied, stored on the PC
Errors (string codes): "writesDisabled", "notHostMode", "disconnected", "outOfRange", "verifyFailed", "deviceError:<code>".
<code> is the HID++ error code in decimal ("deviceError:2") or one of the words "encode", "parse", "policy", "session", "timeout" ("timeout" = the worker did not run the command within 10 s, nothing was sent).
Rules: writes run in the worker one at a time with at least 250 ms between writes to one device; get_device_prefs only answers for known device ids.

## Stage 6 additions (onboard profile save, battery status)
DeviceDetail.battery becomes { percent: number; level: "full"|"good"|"low"|"critical"|null; charging: "discharging"|"charging"|"almostFull"|"full"|"slowCharging"|"error"|"unknown"; externalPower: boolean } | null
  // from 0x1004 fn1 bytes 0..3 (Solaar names); externalPower = byte3 != 0 [INF]. UI shows the charging word first while externalPower or charging != discharging, percent labelled as reported by the mouse.
DeviceDetail.onboard gains: writableProfile: number | null   // active profile number (3..5) when it is enabled, mode is onboard and the sector is in the write allowlist; else null
New command:
save_profile_to_mouse({ id, dpiLevels?: number[5], defaultDpiIndex?: number, shiftDpiIndex?: number, reportRateHz?: number }) -> { detail: DeviceDetail; outcome: "written" | "unchanged"; backupPath: string | null }
Rules: requires settings.allowDeviceWrites and connected; target = active profile only if onboard.writableProfile != null (never 0x0001/0x0002, never the directory); runs in the worker with the existing write rules; backup of the target sector to <app_data>/backups/<deviceId>/<utc timestamp>_sector_<hex>.json before any frame (keep the newest 50); identical image = no write; after a write the worker re-selects the same profile via fn3 and re-reads the detail [INF: the mouse may not reload the active sector otherwise].
New error codes: "profileNotWritable", "notOnboardMode", "backupFailed", "flashWriteFailed:<restored|restoreFailed|notNeeded|unknown>", "savedReselectFailed". "flashWriteFailed:unknown" = the link was lost mid-sequence or no restore was possible, sector state unknown, re-read before any further write. "savedReselectFailed:<active sector as 0x000N, or unknown>" = the new values are on the flash but the reload (switch to another profile and back) did not end on the saved profile; the suffix says which profile the mouse is on now. The save result gains reloaded: boolean; when false (no other enabled profile) the new values apply after a profile switch. A save that times out on the caller side ("deviceError:timeout") means state unknown, never "nothing written"; the UI re-reads. A request the mouse cannot take (levels not 5, rate not 1000/500/250/125, DPI not allowed, index above 4, level 0) is "outOfRange" before any frame. The backup path is returned for "unchanged" too (the backup is taken before the comparison). Backup JSON: sector is a number. Pruning keeps the newest 50 per device plus the oldest backup of each sector forever.
UI: the DPI and rate editors stage edits locally; "Zapisz na mysz" is the only action that writes flash, enabled only when writableProfile != null and there are changes. No write on slider drag.

## Temporary mode (HOST) behind an explicit switch (user decision 2026-10-15, replaces the lazy host switch)
- Default is the onboard profile. The app never enters HOST on its own: no lazy switch in set_dpi/set_report_rate, and nothing on app start or connect.
- New commands: enter_temporary_mode({ id }) -> DeviceDetail (requires allowDeviceWrites and connected; switches to host via 0x8100 fn1 02 and verifies, sets the host mark); leave_temporary_mode({ id }) -> DeviceDetail (explicit user action: if fn2 reads host, return to onboard and verify even without the mark; no-op success when already onboard; errors "disconnected" when the outcome is unknown, "verifyFailed" when the mouse stayed in host).
- set_dpi / set_report_rate: only while the mouse is in host mode and the host mark is set (we entered it), else "notHostMode". No implicit switch.
- restoreOnConnect: only re-applies the stored temporary values after a link-up while temporary mode is active (mark set) in this app run. Never enters host by itself. Default false.
- Turning allowDeviceWrites off, quitting, or shutdown still returns to onboard when the mark is set.
- UI: the performance tab shows only the onboard profile editor by default. A separate "Tryb tymczasowy" card has a button "Włącz tryb tymczasowy" with the warning that the mouse buttons, profiles and LEDs stop working until it is left. While onboard.mode == "host", a clear banner "Tryb tymczasowy (HOST): przyciski i diody myszy wyłączone" with "Wróć do profilu" shows in the main window and the flyout; the temporary DPI and rate editors show only then. The restoreOnConnect option lives inside the temporary mode card.

## Windows
"main"   existing window.
"flyout" frameless, transparent, 380x640, not resizable, skip taskbar, always on top, hidden by default, same frontend (index.html).
         The UI detects it with getCurrentWindow().label === "flyout" and renders the compact card.
         Hides itself when it loses focus. Positioned above the tray icon (bottom right on Windows).

## Mode naming and autostart (user decision 2026-10-15)
- Both modes are equal. Names: onboard = "Profil w myszy", host = "Sterowanie z aplikacji". No warning colours or warning banners for host; a neutral info line says what the mouse does and what the app does in the current mode (host today: the app sets DPI and report rate; mouse buttons other than left/right, profiles and LEDs are not handled yet). A plain segmented mode switch replaces "Włącz tryb tymczasowy" / "Wróć do profilu" (calls enter_temporary_mode / leave_temporary_mode).
- Settings gains launchAtLogin: boolean (default true). Backed by tauri-plugin-autostart (Windows registry Run key, Linux XDG autostart .desktop). A launch from autostart passes "--autostart" and starts hidden in the tray, whatever startMinimized says.

## DPI buttons in app control mode (user decision 2026-10-15)
- enter_temporary_mode: after the verified switch to host, the worker starts the 0x8110 button spy (fn1). Every path that leaves host (leave_temporary_mode, writes off, quit, shutdown, last-resort return, save that returns to onboard) stops the spy (fn2) first, then sends 0x8100 fn1 01. Leave without the mark also sends a spy stop first (harmless if not running).
- Spy notifications (0x8110 fn0, bytes 0..1 LE u16): on a press edge of bit1 (DPI up) or bit2 (DPI down), the worker picks the next higher / lower level from the levels of the profile that was active before host was entered (remembered at enter; fall back to the snapshot's active user profile), clamped at the ends, and applies it with the existing set_dpi path (rate limit; if presses come faster, the latest target wins). It updates dpi.current and emits device-detail-changed. Other bits are ignored for now.
- After a link-up while the mark is set, the spy is started again.
- UI info line in app control mode: "Przyciski DPI na myszy przełączają poziomy profilu {n}."

## Restore last mode at start (user decision 2026-10-15, replaces restoreOnConnect)
- Settings: restoreLastMode: boolean, default true. restoreOnConnect is removed (an old settings file with it still loads; the old field is ignored).
- DevicePrefs gains mode: "onboard" | "host" (default "onboard"). enter_temporary_mode stores "host", leave_temporary_mode stores "onboard". set_dpi / set_report_rate keep storing dpi / reportRateHz (the app-control values). Turning writes off, quit and shutdown return the mouse to onboard but do not change the stored mode.
- On the first successful read after app start, and after each link-up: if restoreLastMode && allowDeviceWrites && prefs.mode == "host", the worker enters app control (host switch + spy, same path as enter_temporary_mode) and applies the stored dpi and reportRateHz. Otherwise nothing is sent.
- Shutdown and quit still return the mouse to onboard, so it works before the app starts.
- UI: Settings switch "Przywracaj ostatni tryb po starcie" (hint: "Po starcie aplikacja wraca do ostatniego trybu i ustawień DPI."). The restoreOnConnect toggle in the app-control card is removed.

## Sony headset
Device id "sony-wf-c710n", name "WF-C710N", kind "headset", vendor "Sony", transport "Bluetooth", protocol "Sony MDR", illustration "wf-c710n", readOnly = !(settings.allowDeviceWrites && connected) (the headset worker republishes on a settings change). No new events: the headset uses list_devices, get_device_detail, refresh_device, "devices-changed" and "device-detail-changed" like the mouse. Write commands are listed below. list_devices returns the G502 first, then the headset.
DeviceDetail gains headset: HeadsetView | null (null for the G502; dpi, reportRate and onboard are null for the headset).
HeadsetView {
  battery: { left: Cell | null; right: Cell | null; case: Cell | null } | null
  ncAsm: { mode: "noiseCancelling" | "ambient"; ambientMode: "normal" | "voice"; level: number; levelMin: number; levelMax: number } | null   // range of the current ambient mode
  eq: { preset: number; presetName: string | null; presets: { id: number; name: string | null }[]; bands: number[]; stepCount: number; bandInfo: { kind: "hz" | "clearBass" | "special" | "unknown"; hz: number | null }[] | null } | null
  multipoint: boolean | null
  sidetone: boolean | null
  codec: string | null            // "SBC" | "AAC" | "LDAC" | "aptX" | "aptX HD" | "LC3", null while unsettled or unknown
  firmware: string | null
}
EQ bandInfo (read with `5a 00`, DEVICE-WF-C710N.md Round 6): one entry per element of eq.bands, same order and length; null when the headset did not answer or the length differs. kind "hz" carries hz (400, 1000, 2500, 6300, 16000 on the WF-C710N), "clearBass" is the band the Sony app draws as a separate slider, "special" and "unknown" are other entries. Clear Bass rule: find the entry of kind "clearBass" at runtime, never hardcode its index (it is index 0 on the WF-C710N). All bands use the same scale (stepCount 21, step 10 = 0 dB). Staging and set_eq_bands stay one array of all bands; the UI edits Clear Bass at its own index.
Cell { percent: number; charging: "notCharging" | "charging" | "charged" | "unknown" }
Preset names are stable lowercase keys the UI translates: off, bright, excited, mellow, relaxed, vocal, treble, bass, speech, custom, user1, user2; null for an id not in that list.
Summary and battery: batteryPercent and battery.percent are the lower of left and right; battery.charging is "charging" when either bud charges, "full" when both are charged, "discharging" when neither charges, else "unknown"; level null, externalPower false. The case is only in headset.battery.case.
Disconnected semantics: the headset worker keeps the last values and sets connection "disconnected" when the link drops (case closed, switched off, out of range); it retries after 2 s, 5 s, then every 10 s. Before the first successful connect (not paired, never reachable) the device is absent from list_devices and get_device_detail/refresh_device answer null.
refresh_device("sony-wf-c710n") re-reads battery, NC/ASM and EQ with GETs and answers the new detail; null while disconnected or absent. Live changes (battery, NC/ASM, EQ, multipoint, sidetone) arrive as "device-detail-changed".
Backoff: after 3 failed connects in a row the retry interval grows to 60 s (buds in the case); a refresh_device call for the headset, or a successful connect, returns to the short schedule. A refresh the headset worker cannot answer within 5 s (it is busy connecting or in slow reads) is answered with the cached detail instead of null, so the UI keeps the last values; the worker still finishes and emits "device-detail-changed". A worker panic republishes the headset as disconnected.
Mouse commands with the headset id (set_dpi, set_report_rate, enter_temporary_mode, leave_temporary_mode, save_profile_to_mouse) answer the error "writesDisabled" without reaching any worker. get_device_prefs answers null (no prefs yet; the id is in the known list for the prefs file only).
The tray shows the G502 while it is known, otherwise the headset ("WF-C710N: 80%"). A disconnected headset reads "WF-C710N: asleep" (en) or "WF-C710N: uśpiona" (pl), the same wording as the mouse.
KNOWN_DEVICE_IDS includes the headset id, so get_device_prefs would parse a Sony prefs file as the mouse DevicePrefs. Harmless now (no such file is written); revisit before headset prefs exist.

### Headset write commands
set_nc_asm({ id, mode: "noiseCancelling" | "ambient", ambientMode: "normal" | "voice", level: number })
set_eq_preset({ id, preset: number })           // preset id from headset.eq.presets
set_eq_bands({ id, bands: number[] })           // steps 0..stepCount-1, one per band (6), written to CUSTOM
set_headset_toggle({ id, setting: "sidetone" | "multipoint", on: boolean, confirmDisconnect?: boolean })
All return the new DeviceDetail or an error string. Only the headset id is accepted: with the G502 id they answer "notSupported", and the mouse commands with the headset id keep answering "writesDisabled".
Rules: allowDeviceWrites (shared with the mouse) must be on and the headset connected. The worker runs one write at a time, at least 250 ms apart, and each write is verified with a GET before it answers. noiseCancelling sends level 0 (the device keeps a level per mode) and ignores `level`; ambient needs a level inside levelMin..levelMax of ambientMode. set_eq_bands selects CUSTOM first when another preset is active (verified), then writes the bands, so the active preset becomes CUSTOM. Sidetone is a plain verified toggle. Multipoint drops the Bluetooth link: it needs confirmDisconnect: true, the answer is a detail that may already be "disconnected" (the headset raised an alert, the app confirmed it, the link dropped), and the final state arrives via "device-detail-changed" after the worker reconnects with the short schedule (2 s, 5 s, 10 s) and reads again. Setting multipoint to its current value sends nothing and answers the current detail.
Errors: "writesDisabled", "disconnected", "outOfRange" (also a number that does not fit 0..255), "notSupported" (function missing, or the G502 id), "verifyFailed" (the headset did not keep the value; the worker reads it again and publishes it), "confirmRequired" (multipoint without confirmDisconnect, nothing sent), "alertMissing" (no confirmation alert within 3 s, nothing else sent), "alertRefused" (another alert came up and was answered negative), "deviceError:timeout" (the worker did not answer within 40 s, or a multipoint confirmation went out and the link stayed up, which makes the worker reconnect and read everything; state unknown, the UI re-reads), "deviceError:<other>" (transport, policy, send, unexpected).
Safety gates in the device crate: every write builds a Policy with only the gate it needs (setting tier for NC/ASM, EQ, sidetone; disconnecting tier for multipoint and its positive alert answer; negative alert answers use the setting tier). The session itself stays on the read tier.
Budget: the caller waits 40 s and the worker deadline is 38 s (the G502 margin of 2 s). The worker starts a write only when enough of that time is left (every wait at its limit, rate-limit pause not counted) and otherwise answers "deviceError:timeout" without sending anything: 14 s for NC/ASM, EQ preset and sidetone, 28 s for EQ bands (it may select CUSTOM first), 25 s for multipoint (alert 3 s + link drop 10 s + four reply waits). A "verifyFailed" on sidetone or multipoint re-reads that value with its GET and publishes it. If a write ends the link, writes queued behind it answer "disconnected".
allowDeviceWrites is checked once per write, before it starts; a write already running completes even if the setting is turned off meanwhile.
