# TonePush built from its release source, for Fedora COPR (paolino/tonepush).
# Cargo fetches the locked dependencies while building, so the COPR project
# needs network access during builds.

Name:           tonepush
Version:        @VERSION@
Release:        1%{?dist}
Summary:        Editor and tone library for Line 6 HX pedals and the StompStation PRO
License:        MIT
URL:            https://docs.tonepush.rocks
Source0:        https://github.com/crmne/tonepush/archive/refs/tags/v%{version}.tar.gz#/%{name}-%{version}.tar.gz

# ring compiles its own C, and LTO objects in its archive leave the link
# without ring_core_* symbols, as on Arch.
%global _lto_cflags %{nil}
# The binaries are stripped of their source paths below; debuginfo would only
# carry them back.
%global debug_package %{nil}

ExclusiveArch:  x86_64 aarch64
BuildRequires:  rust >= 1.98
BuildRequires:  cargo
BuildRequires:  clang
BuildRequires:  clang-devel
BuildRequires:  gcc
BuildRequires:  systemd-rpm-macros
Requires:       libglvnd-glx
Requires:       libglvnd-egl
Requires:       libxkbcommon
Requires:       libxkbcommon-x11
Requires:       libwayland-client
Requires:       libwayland-cursor
Requires:       libwayland-egl
Requires:       libX11
Requires:       libX11-xcb
Requires:       libXcursor
Requires:       libXi
Requires:       libXrandr
Recommends:     xdg-desktop-portal
# Extracts HX Edit's model data from inside the app.
Recommends:     7zip

%description
TonePush is an open-source editor for Line 6 HX-family devices and the
Sonulab StompStation PRO: one cross-platform GUI, one scriptable CLI, and a
tone library you can search. The package installs the udev rule that lets
the editor reach the pedals over USB.

%prep
%autosetup -n %{name}-%{version}

%build
export CARGO_TARGET_DIR=target
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=%{_builddir}=/"
# The editor and the CLI by name: the whole workspace would also build the
# Ruby extension, which links against libruby.
cargo build --locked --release -p tonepush-cli -p tonepush-gui

%check
# Needs no hardware; the tests that talk to a device are #[ignore]d.
export CARGO_TARGET_DIR=target
cargo test --locked --release -p hx-proto -p hx-catalog -p tonepush-cli -p tonepush-gui

%install
install -Dm755 target/release/tonepush %{buildroot}%{_bindir}/tonepush
install -Dm755 target/release/tonepush-gui %{buildroot}%{_bindir}/tonepush-gui
install -Dm644 packaging/applications/tonepush.desktop %{buildroot}%{_datadir}/applications/tonepush.desktop
install -Dm644 packaging/icons/tonepush.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/tonepush.svg
install -Dm644 packaging/udev/70-line6-hx.rules %{buildroot}%{_udevrulesdir}/70-line6-hx.rules

%post
%udev_rules_update

%postun
%udev_rules_update

%files
%license LICENSE
%doc README.md
%{_bindir}/tonepush
%{_bindir}/tonepush-gui
%{_datadir}/applications/tonepush.desktop
%{_datadir}/icons/hicolor/scalable/apps/tonepush.svg
%{_udevrulesdir}/70-line6-hx.rules

%changelog
* @DATE@ Carmine Paolino <carmine@paolino.me> - @VERSION@-1
- TonePush @VERSION@: https://github.com/crmne/tonepush/releases/tag/v@VERSION@
