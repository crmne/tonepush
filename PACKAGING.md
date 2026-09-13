# Packaging

[`native-packages.yaml`](native-packages.yaml) is the packaging configuration:
it pins the shared CLI and nFPM versions and declares Linux amd64/arm64 inputs,
DEB/RPM contents, dependencies, recipe templates and downstream repositories.
Application assets and native recipes stay in `packaging/`.

```sh
gem install native-packages --version 0.2.0
native-packages validate
native-packages doctor
native-packages build --release v1.2.3
```

Replace `v1.2.3` with an existing stable application release. Local use also
requires nFPM 2.47.0, `bsdtar` and `readelf`; AUR generation needs `makepkg`
or Docker. CI installs its tooling. To package local release archives, put
every configured input and recipe asset under `dist/`, then run
`native-packages build --version 1.2.3`. Outputs go to
`dist/packages/1.2.3`; use `--output` for a fresh destination when rebuilding.

Stable tags run the existing native build jobs first. After binaries and
`checksums.txt` are published, the shared workflow verifies their hashes,
builds the configured packages, and attaches them to the GitHub release.
Configured recipes are attached as an archive. Package checksums are separate
from the original binary checksums. PR validation never publishes.

Review or publish an existing build with the same installed CLI:

```sh
native-packages publish --from dist/packages/1.2.3 --to github
native-packages repositories
native-packages status --offline
```

For applications with configured AUR or Homebrew destinations, stage the
recipes with `native-packages stage TARGET dist/packages/1.2.3/recipes`,
inspect `native-packages diff TARGET`, run native package validation, and
publish with `native-packages publish TARGET`. These destinations use ignored
managed Git clones, recorded in this application's YAML configuration.
AUR automation needs `PUBLISH_AUR=true`, `AUR_SSH_KEY` and `AUR_KNOWN_HOSTS`;
Homebrew automation needs `PUBLISH_HOMEBREW=true` and
`HOMEBREW_TAP_GITHUB_TOKEN`. Enable only configured destinations.

The existing macOS, Windows and Flatpak build/signing steps remain responsible
for their native artifacts. Additional nFPM formats require suitable platform
inputs and dependencies; adding a format does not port the application.
See the [shared CLI documentation](https://github.com/crmne/native-packages/tree/v0.2.0)
for commands and supported formats.

To upgrade the tool, change `tool.version` in `native-packages.yaml`, the
matching immutable workflow reference, and any release-job gem installation
pin together. Applications need no packaging Gemfile, lockfile or Ruby wrapper.

## Upstream Release Assets

Each tagged release publishes:

- `tonepush-v<version>-x86_64-unknown-linux-gnu.tar.gz`
- `tonepush-v<version>-aarch64-unknown-linux-gnu.tar.gz`
- `tonepush-v<version>-macos-universal.dmg` (the app, drag to Applications, plus the CLI binary)
- `tonepush-v<version>-macos-universal.tar.gz` (bare universal binaries, for Homebrew and scripts)
- `tonepush-v<version>-x86_64-pc-windows-msvc.zip`
- `tonepush-v<version>-aarch64-pc-windows-msvc.zip`
- `tonepush-v<version>-vendor.tar.xz`
- `checksums.txt`
- GitHub's automatic source archive for the tag

The Linux binary archives contain:

- `tonepush`
- `tonepush-gui`
- `README.md`
- `LICENSE`
- `packaging/applications/tonepush.desktop`
- `packaging/icons/tonepush.svg`
- `packaging/udev/70-line6-hx.rules`

## macOS Signing and Notarization

The macOS job signs and notarizes the DMG when these repository secrets
exist; without them it ships the same DMG unsigned:

- `APPLE_CERTIFICATE_P12`: a Developer ID Application certificate with its
  key, exported as .p12 and base64-encoded
- `APPLE_CERTIFICATE_PASSWORD`: the .p12 password
- `APPLE_SIGNING_IDENTITY`: e.g. `Developer ID Application: Name (TEAMID)`
- `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`: notarytool credentials;
  the password is an app-specific password from appleid.apple.com

## Dependencies

Runtime:

- glibc and libgcc are the only linked libraries; the CLI needs nothing else
- the GUI loads the display stack at runtime: libGL, libxkbcommon, and
  Wayland or X11 client libraries, all present on any desktop install
- model names, parameter ranges, and artwork come from HX Edit's own data
  files, which are Line 6's and are not redistributable. The app walks the
  user through extracting them on first launch; packages must not bundle
  them.
- reading an HX Edit installer in-app needs 7-Zip (`7z`, `7za`, or `7zz` on
  PATH, or an ordinary Windows install of 7-Zip, which the app finds where
  the installer left it): package it as an optional dependency on Linux
  (`p7zip`) and Windows. macOS needs nothing extra; it uses hdiutil and pkgutil. A machine
  with HX Edit already installed needs no extraction at all: the app copies
  from the installation by itself.

Build time:

- Rust `1.87` or newer (the pinned toolchain in `rust-toolchain.toml` is what
  CI uses; any newer stable works)
- on Linux, the GUI needs the usual egui build packages: `libxkbcommon-dev`,
  `libwayland-dev`, `libgl1-mesa-dev` or your distro's equivalents

## Build From Source

```sh
cargo build --release --locked
```

The two binaries land in `target/release/tonepush` and
`target/release/tonepush-gui`. `cargo test --workspace --locked` needs no
hardware; tests that do talk to a device are `#[ignore]`d by default.

For offline builds, unpack the vendor archive and build against it:

```sh
tar -xf tonepush-v<version>-vendor.tar.xz
cp -r tonepush-v<version>-vendor/.cargo .
cp tonepush-v<version>-vendor/Cargo.lock .
ln -s tonepush-v<version>-vendor/vendor vendor
cargo build --release --offline
```

The archive's `.cargo/config.toml` redirects crates.io to the vendored
sources; it is the file `cargo vendor` printed at archive time.

## Installed Files

Recommended installed files:

```text
/usr/bin/tonepush
/usr/bin/tonepush-gui
/usr/share/applications/tonepush.desktop
/usr/share/icons/hicolor/scalable/apps/tonepush.svg
/usr/lib/udev/rules.d/70-line6-hx.rules
/usr/share/licenses/tonepush/LICENSE
/usr/share/doc/tonepush/README.md
```

The udev rule is what lets a normal user open the USB device; without it every
connection fails with a permission error that looks like an application bug.
Do not force a udev reload from package scripts beyond the packaging norm for
your distro; tell the user to replug the device after installing.

## Package Status

Configured packaging destinations (publication is a separate step):

| Channel | Status | Notes |
|---|---|---|
| Arch AUR | Configured | `tonepush`, `tonepush-bin` and `tonepush-git` templates live in `packaging/arch/`. |
| Fedora COPR | Not started | |
| Nixpkgs | Not started | |
| Gentoo GURU | Not started | |
| Alpine aports | Not started | |
| Debian and Ubuntu | Configured | nFPM DEB files attached after stable release packaging succeeds. |
| RPM downloads | Configured | nFPM RPM files attached after stable release packaging succeeds. |
| openSUSE OBS | Not started | |

Edit recipes in this repository, then stage the generated files into their
downstream repositories. Upstream distribution acceptance remains separate from
building downloadable packages.

## Smoke Tests

After packaging, run:

```sh
tonepush --version
tonepush --help
tonepush models >/dev/null && echo "catalog ok"   # only with HX Edit resources extracted
test -f /usr/share/applications/tonepush.desktop
test -f /usr/share/icons/hicolor/scalable/apps/tonepush.svg
test -f /usr/lib/udev/rules.d/70-line6-hx.rules
```

With an HX device on USB and HX Edit closed, also verify:

```sh
tonepush list
tonepush chain
```
