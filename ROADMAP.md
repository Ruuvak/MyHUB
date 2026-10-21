# MyHUB roadmap

Current release: 0.1.1 (Windows tested). Items marked hardware need a supervised run on the device before they count as done.

## 0.2: Linux and diagnostics

Goal: MyHUB runs in the background on Kubuntu (KDE Plasma) as it does on Windows, and problems can be diagnosed from an installed build.

| Area | Work |
|---|---|
| Linux device access | udev rule for the G502 X receiver and the Keychron K2 HE and Link, shipped in the `.deb` and offered as a file for AppImage users |
| Linux permissions | When a device is listed but cannot be opened, the app says so and shows how to install the rule, instead of showing nothing |
| Linux hotplug | udev monitor wakes the device workers on HID add and remove, as CM_Register_Notification does on Windows; the 60 s fallback scan applies on both platforms |
| Logs | Log lines go to a rotating file in the app data folder; Settings gets "Open logs folder" |
| Release notes | `CHANGELOG.md`; the release workflow takes the section for the tag as the release body |
| README | Current version, platforms, modules (G502 X, WF-C710N, K2 HE), updater, module switches |

Hardware after 0.2 lands: first Kubuntu run of the mouse, keyboard and headset; restart of the PC with the mouse in app control; autostart from the installed build.

## 0.2.x

| Module | Work |
|---|---|
| G502 X | Sniper and wheel tilt bits of the 0x8110 spy (needs a capture first, hardware) |
| G502 X | Cause of the occasional battery read timeout (hardware) |
| K2 HE | Key remap in the app (today only in `kc_probe`) |
| WF-C710N | Confirm every write path on the headset (hardware) |

## Later

- Windows code signing, to remove the SmartScreen prompt. Needs a paid certificate.
- Lighting editing for the G502 X. Needs a new decision on flash writes, which today cover only DPI, DPI indexes and report rate.
