---
title: Download
description: Get TonePush for macOS, Windows, or Linux, with install instructions for each.
nav_order: 1
---

{% assign v = site.tonepush_version %}
{% assign base = "https://github.com/crmne/tonepush/releases/download/v" | append: v %}

The current version is **v{{ v }}**. Every file below, with its SHA-256, is
listed in [checksums.txt]({{ base }}/checksums.txt); older versions live on
the [releases page](https://github.com/crmne/tonepush/releases).

## macOS

One download for both Apple Silicon and Intel:

- [tonepush-v{{ v }}-macos-universal.dmg]({{ base }}/tonepush-v{{ v }}-macos-universal.dmg)

Open it and drag **TonePush** to Applications. The DMG also carries the
`tonepush` command-line tool; copy it somewhere on your PATH if you want it.

The app is signed and notarized by Apple, so it opens with a double-click.

Homebrew users can instead run:

```sh
brew install --cask crmne/tap/tonepush   # the app
brew install crmne/tap/tonepush          # the CLI
```

## Windows

Almost every PC wants the first one; the second is for Windows on ARM
(Surface and other Snapdragon machines):

- [tonepush-v{{ v }}-x86_64-pc-windows-msvc.zip]({{ base }}/tonepush-v{{ v }}-x86_64-pc-windows-msvc.zip)
- [tonepush-v{{ v }}-aarch64-pc-windows-msvc.zip]({{ base }}/tonepush-v{{ v }}-aarch64-pc-windows-msvc.zip)

Unpack and run `tonepush-gui.exe`. SmartScreen may warn about an unknown
publisher on first run; choose More info, then Run anyway.

## Linux

On Arch and its derivatives, install from the AUR, which also sets up the
udev rule and desktop entry:

```sh
yay -S tonepush-bin     # ready made; tonepush builds the release from source, tonepush-git the latest commit
```

On Debian, Ubuntu and their derivatives, or on Fedora and other RPM
distros, install the package for your machine, which also sets up the udev
rule and desktop entry:

- [tonepush_{{ v }}_amd64.deb]({{ base }}/tonepush_{{ v }}_amd64.deb) ·
  [tonepush_{{ v }}_arm64.deb]({{ base }}/tonepush_{{ v }}_arm64.deb)
- [tonepush-{{ v }}-1.x86_64.rpm]({{ base }}/tonepush-{{ v }}-1.x86_64.rpm) ·
  [tonepush-{{ v }}-1.aarch64.rpm]({{ base }}/tonepush-{{ v }}-1.aarch64.rpm)

To run it without installing anything, use the AppImage. It needs glibc 2.39
or newer and still needs the udev rule below to reach the pedal:

- [tonepush-{{ v }}-x86_64.AppImage]({{ base }}/tonepush-{{ v }}-x86_64.AppImage) ·
  [tonepush-{{ v }}-aarch64.AppImage]({{ base }}/tonepush-{{ v }}-aarch64.AppImage)

Or grab the archive for your machine:

- [tonepush-v{{ v }}-x86_64-unknown-linux-gnu.tar.gz]({{ base }}/tonepush-v{{ v }}-x86_64-unknown-linux-gnu.tar.gz)
- [tonepush-v{{ v }}-aarch64-unknown-linux-gnu.tar.gz]({{ base }}/tonepush-v{{ v }}-aarch64-unknown-linux-gnu.tar.gz)

Unpack it, put the two binaries on your PATH, and install the packaged udev
rule so you can open the device without root:

```sh
sudo install -m644 packaging/udev/70-line6-hx.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then replug the pedal once. For the first-launch extraction of HX Edit's
model data, install `p7zip` too; the AUR package already suggests it.

## Build from source

Any platform, with [Rust](https://rustup.rs) installed:

```sh
git clone https://github.com/crmne/tonepush
cd tonepush
./install.sh
```

Packagers should read
[PACKAGING.md](https://github.com/crmne/tonepush/blob/main/PACKAGING.md),
which covers offline builds from the vendored-dependencies archive.

## Updating

TonePush asks GitHub once a day whether a newer version exists; nothing else
is sent. When there is one, the foot of the sidebar says so in amber (for
example **0.8.0 is out**). Click it, or the settings button beside it, for the
offer.

- **The macOS app from the DMG, and the Windows and Linux archives:** click
  **Update to 0.8.0**. TonePush downloads the new version, checks it against
  its release signature, and offers **Restart to update**. It closes, lets the
  pedal go and opens the new version; if that does not start, the previous one
  comes back. From an archive only the editor updates itself: keep
  `tonepush-portable.txt` next to `tonepush-gui`, and replace the `tonepush`
  command-line tool from a new archive when you want it newer too.
- **Homebrew, the AUR, .deb or .rpm:** update through that package manager
  (`brew upgrade`, your AUR helper, apt or dnf). The settings say which.

## After installing

Whichever route you took, finish with [Getting Started](/getting-started/):
one extraction step gives you model names and artwork, and there are a few
things worth knowing before your first edit.
