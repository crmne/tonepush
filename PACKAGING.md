# Packaging

[`native-packages.yaml`](native-packages.yaml) is the packaging configuration:
it pins the shared CLI and nFPM versions and declares Linux amd64/arm64 inputs,
DEB/RPM contents, dependencies, recipe templates and downstream repositories.
Application assets and native recipes stay in `packaging/`.

```sh
gem install native-packages --version 0.5.1
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

The native macOS configuration, Windows and Flatpak build steps remain responsible
for their native artifacts. Additional nFPM formats require suitable platform
inputs and dependencies; adding a format does not port the application.
See the [shared CLI documentation](https://github.com/crmne/native-packages/tree/v0.5.1)
for commands and supported formats.

To upgrade the tool, change `tool.version` in both `native-packages.yaml` and
`native-packages.macos.yaml`, the matching immutable workflow reference, and any release-job gem installation
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
- `checksums.txt.sig` (an Ed25519 signature over `checksums.txt`, made with the
  key whose public half is `assets/update-public-key.hex`)
- GitHub's automatic source archive for the tag

The Linux binary archives contain:

- `tonepush`
- `tonepush-gui`
- `README.md`
- `LICENSE`
- `packaging/applications/tonepush.desktop`
- `packaging/icons/tonepush.svg`
- `packaging/udev/70-line6-hx.rules`

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

## Updates

The editor checks `api.github.com` for a newer release once a day and says so
in its status bar. Only the macOS app from the DMG replaces itself, after the
user asks: it downloads the DMG, verifies `checksums.txt.sig` against the key
compiled into the app, checks the bundle's identifier (`rocks.tonepush.editor`),
version and signing team, and restarts into it, rolling back if the new
version does not start. The editor answers `--version` with
`tonepush <version>`; the updater asks the downloaded bundle's executable
before installing it, so that answer must not change.

Package-managed copies never replace themselves. The updater recognises
Homebrew (cask and formula), pacman and the AUR, dpkg, rpm, Flatpak, Snap, Nix
and `cargo install`, and anything else under `/usr`, and tells the user which
tool updates it. Packages need no patch to turn updates off.

The Linux and Windows archives carry no portable marker, so those copies do
not replace themselves either: they point at the release page. The updater
installs the executable named after the slug from an archive, and in
TonePush's archives `tonepush` is the command-line tool, not the editor.

## Package Status

Configured packaging destinations (publication is a separate step):

| Channel | Status | Notes |
|---|---|---|
| Arch AUR | Configured | `tonepush`, `tonepush-bin` and `tonepush-git` templates live in `packaging/arch/`. |
| Homebrew | Configured | The `tonepush` cask (the app, from the DMG) and the `tonepush` formula (the universal binaries) in `crmne/homebrew-tap`; templates live in `packaging/homebrew/`. |
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

## Automatic macOS notarization

The macOS release job builds the app first, then uses
`native-packages.macos.yaml` and `packaging/macos/dmg.rb` to package it.
The shared gem signs its owned input copy, notarizes the DMG, staples and validates
Apple's ticket, and only then records final checksums. Configure these repository
secrets, which the job exposes as environment variables:

- `APPLE_CERTIFICATE_P12`: base64 PKCS#12 Developer ID Application certificate and private key.
- `APPLE_CERTIFICATE_PASSWORD`: the export password.
- `APPLE_SIGNING_IDENTITY`: exact `Developer ID Application: Name (TEAMID)` identity.
- `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`: Apple email, Team ID and app-specific password.

A complete set enables notarization automatically. An incomplete set fails;
no values retain local builds without Developer ID signing. Application inputs
and the user's normal keychains remain unchanged. See the shared
[Apple setup and phase contract](https://github.com/crmne/native-packages/blob/v0.5.1/docs/apple-notarization.md).

After preparing `dist/macos-input` on a Mac, test packaging without publishing:

```sh
native-packages --config native-packages.macos.yaml build \
  --version 1.2.3 --target macos-universal --output dist/macos-packages-test
```

Secret configuration applies to future builds. Existing published DMGs retain
their original signatures; this setup does not replace release assets.

The universal portable tarball also runs through `native-packages notarize-macos`
before archiving. The shared command signs and submits the exact executable and
dylib tree in a temporary ZIP; only its returned signed copy enters the tarball.
Raw executables use Gatekeeper's online ticket lookup because they cannot carry
a stapled ticket. Both the CLI and GUI binary remain in the portable archive.
