---
title: Getting Started
description: Install TonePush, set up USB access, and connect an HX pedal or StompStation PRO.
nav_order: 2
---

## Install

The [Download page](/download/) has the right file for every OS, with instructions: a drag-to-Applications app for macOS, zips for Windows, archives and AUR packages for Linux. `tonepush` is the CLI, `tonepush-gui` is the editor.

Or build from source, which also installs desktop integration:

```sh
git clone https://github.com/crmne/tonepush
cd tonepush
./install.sh
```

That builds everything, puts `tonepush` on your PATH, and installs the editor: a double-clickable app on macOS, a desktop entry with an icon on Linux. `./install.sh --cli-only` skips the GUI, and `--uninstall` removes it all again.

Building needs [Rust](https://rustup.rs). On Linux the GUI additionally needs the X11/Wayland development packages any egui application does. On Debian or Ubuntu:

```sh
sudo apt install libxkbcommon-dev libwayland-dev libgl1-mesa-dev
```

### USB access on Linux

A normal user cannot open a USB device on Linux without being granted access.
`install.sh` installs rules for both Line 6 USB interfaces and the StompStation
PRO serial interface, and asks for sudo once; without them, a connection can
fail with a permission error that looks like an application bug. Replug the
device after installing. To do it by hand:

```sh
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="0e41", MODE="0666", TAG+="uaccess"' \
  | sudo tee /etc/udev/rules.d/70-line6-hx.rules
echo 'SUBSYSTEM=="tty", ATTRS{manufacturer}=="SONULAB", ATTRS{product}=="StompStation PRO", MODE="0660", TAG+="uaccess"' \
  | sudo tee -a /etc/udev/rules.d/70-line6-hx.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```

### Model names and pictures

Names, parameter ranges, value formatting, and artwork come from HX Edit's own data files, which are Line 6's and are **not** redistributed here. The editor walks you through this the first time it opens: if HX Edit is installed on the machine it copies the data by itself, and otherwise it takes the HX Edit installer you download from [line6.com/software](https://line6.com/software/), either the Mac .dmg or the Windows .exe, on any OS.

Reading an installer needs 7-Zip on Linux and Windows; on most distros that is the `p7zip` package, and the AUR package already suggests it. On Windows, install [7-Zip](https://www.7-zip.org/) the ordinary way and the editor finds it where the installer left it — there is nothing to add to PATH. macOS needs nothing extra.

## Connect

**Quit the vendor editor first:** HX Edit for a Line 6 device, or VoidX Control
for a StompStation PRO. Only one editor can own a device connection at a time.

Plug in the pedal and start the editor:

```sh
tonepush-gui
```

It connects on launch. The signal chain runs across the top, presets down the left, and the selected block's knobs fill the middle, with the model browser on the right.

That layout, and the local Tones / Setlists / Cloud library below it, is the
same for both families. The available blocks and operations follow the pedal's
capabilities. The PRO does not need HX Edit resources; it supplies its names,
ranges, choices, NAM models, and IR names through its own schema. See the
[StompStation PRO guide](/stompstation-pro/) for its verified rollback guard.

A few things worth knowing on day one:

- **Edits are live but not saved.** The device edits a scratch copy: a changed parameter is audible immediately but vanishes on reload unless you press Save. The amber dot next to Save tells you when there is something to save.
- **Add a block** by clicking any gap in the wire, then picking a pedal. Type to search; the picker opens ready for it.
- **Make a parallel branch** by dragging a block onto the dashed branch below the line, or by clicking the + on it.
- **Move the fork and merge** by dragging their dots along the line.
- **Undo, redo, save** are Ctrl+Z, Ctrl+Shift+Z, and Ctrl+S (Cmd on macOS).

## Handle with care

These devices can lock up hard enough to need their 9V adapter pulled; a USB replug is not enough, because the unit is externally powered and keeps its session across re-enumeration. Every lock-up during development traced back to the client, not the hardware, and each cause is now understood, avoided, and pinned by a regression test. The full post-mortem is in [PROTOCOL.md](https://github.com/crmne/tonepush/blob/main/PROTOCOL.md).
