# Changelog

All notable changes to MyHUB are listed here, newest first. The format follows Keep a Changelog and the versions follow SemVer.

## 0.2.0 (unreleased)

### Added

* Linux: a udev rule for the Logitech LIGHTSPEED receiver and for Keychron keyboards and the Link receiver. The .deb installs it to /usr/lib/udev/rules.d/70-myhub.rules and reloads udev.
* Linux: an access banner. When a device is listed but cannot be opened because of missing permissions, the app says so and offers the rule file to save, with the install command.
* Linux: hotplug through a udev monitor on hidraw, so a device that is plugged in or removed wakes the device workers, as on Windows.
* Log file: backend log lines are also written to myhub.log in the app log folder, each with a UTC timestamp. The file moves to myhub.1.log at 1 MiB.
* Settings: a Logs row with the Open logs folder button.
* Release notes: this file. The release workflow takes the section for the tag as the release body.
* README brought up to date with the current devices, platforms and tools.

## 0.1.1

### Changed

* Hardware is touched only when needed. The Sony headset worker connects only while the headset is connected. The Keychron worker does no HID enumeration while the keyboard is connected over USB. The mouse battery is updated from the mouse's own events.
* Windows: hotplug wakes the device workers.
* Settings: a module switch for each device family (Logitech, Sony, Keychron). A module that is off does not talk to its device.
* App control mode keeps the profile levels after an app start.

## 0.1.0

First release.

### Added

* Logitech G502 X LIGHTSPEED mouse over HID++ 2.0: battery, DPI levels, report rate and onboard profile saving.
* Sony WF-C710N earbuds over Bluetooth RFCOMM on Windows: battery, noise cancelling and ambient sound, equalizer, sidetone and multipoint.
* Keychron K2 HE keyboard: HE profile selection, actuation point, Rapid Trigger and lighting.
* Tray icon with a flyout, and optional launch at login.
* Built-in updater that installs signed releases from GitHub.
