#!/bin/bash
# Build and install TonePush.
#
#   ./install.sh              build and install everything
#   ./install.sh --cli-only   skip the GUI
#   ./install.sh --uninstall  remove what this installed
#
# Installs the `tonepush` command into the first writable directory already on your
# PATH, and on macOS builds the GUI into a double-clickable app. Nothing is
# written outside your home directory unless a system path is already writable
# and on PATH.
set -euo pipefail

APP_NAME="TonePush"
APP_SLUG="tonepush"
# Read from the workspace rather than written down here, because a version kept
# in two places is a version that disagrees with itself the release after next.
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
BIN_DIRS=("$HOME/.local/bin" "/usr/local/bin" "$HOME/bin")
MAC_APPS="$HOME/Applications"
LINUX_APPS="$HOME/.local/share/applications"
UDEV_RULE="/etc/udev/rules.d/70-line6-hx.rules"
LINE6_VENDOR="0e41"
HX_RESOURCES="${XDG_DATA_HOME:-$HOME/.local/share}/tonepush/hx-resources"

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33m warning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31m error:\033[0m %s\n' "$*" >&2; exit 1; }

bin_dir() {
    for d in "${BIN_DIRS[@]}"; do
        case ":$PATH:" in *":$d:"*) [ -d "$d" ] && [ -w "$d" ] && { echo "$d"; return; };; esac
    done
    # Nothing suitable on PATH: make the conventional one and say so.
    mkdir -p "${BIN_DIRS[0]}"
    echo "${BIN_DIRS[0]}"
}

# What the program was called before, so an upgrade can take it away.
#
# The rename left every old binary, launcher entry and icon exactly where it
# was, and the old ones are not harmlessly redundant: 0.2.x reads
# ~/.local/share/stompchain, which the current version moves to
# ~/.local/share/tonepush on its first run. So a launcher still pointing at the
# old entry opens an editor showing an empty library, which looks precisely
# like having lost everything. Nothing is lost - it is the wrong program - but
# nobody should have to work that out.
FORMER_SLUG="stompchain"
FORMER_NAME="stompchain"

retire_former_name() {
    local dir found=0
    for dir in "${BIN_DIRS[@]}"; do
        for bin in "$FORMER_SLUG" "$FORMER_SLUG-gui"; do
            [ -f "$dir/$bin" ] && { rm -f "$dir/$bin"; say "removed $dir/$bin (the old name)"; found=1; }
        done
    done
    [ -d "$MAC_APPS/$FORMER_NAME.app" ] && {
        rm -rf "$MAC_APPS/$FORMER_NAME.app"
        say "removed $MAC_APPS/$FORMER_NAME.app (the old name)"
        found=1
    }
    [ -f "$LINUX_APPS/$FORMER_SLUG.desktop" ] && {
        rm -f "$LINUX_APPS/$FORMER_SLUG.desktop"
        say "removed $LINUX_APPS/$FORMER_SLUG.desktop (the old name)"
        found=1
    }
    local icon="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps/$FORMER_SLUG.svg"
    [ -f "$icon" ] && { rm -f "$icon"; say "removed $icon (the old name)"; found=1; }
    [ "$found" = 1 ] && say "your library moved across with the name; nothing was lost"
    return 0
}

uninstall() {
    local dir
    for dir in "${BIN_DIRS[@]}"; do
        for bin in tonepush tonepush-gui; do
            [ -f "$dir/$bin" ] && { rm -f "$dir/$bin"; say "removed $dir/$bin"; }
        done
    done
    [ -d "$MAC_APPS/$APP_NAME.app" ] && {
        rm -rf "$MAC_APPS/$APP_NAME.app"
        say "removed $MAC_APPS/$APP_NAME.app"
    }
    [ -f "$LINUX_APPS/$APP_SLUG.desktop" ] && {
        rm -f "$LINUX_APPS/$APP_SLUG.desktop"
        say "removed $LINUX_APPS/$APP_SLUG.desktop"
    }
    local icon="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps/$APP_SLUG.svg"
    [ -f "$icon" ] && {
        rm -f "$icon"
        say "removed $icon"
    }
    retire_former_name
    if [ -f "$UDEV_RULE" ]; then
        warn "left $UDEV_RULE in place; remove it with sudo if you want it gone"
    fi
    say "done. Your presets and device are untouched."
    exit 0
}

make_app_bundle() {
    local gui="$1" app="$MAC_APPS/$APP_NAME.app"
    mkdir -p "$app/Contents/MacOS"
    cp "$gui" "$app/Contents/MacOS/$APP_SLUG"
    cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>$APP_NAME</string>
    <key>CFBundleDisplayName</key><string>$APP_NAME</string>
    <key>CFBundleIdentifier</key><string>rocks.tonepush.editor</string>
    <key>CFBundleExecutable</key><string>$APP_SLUG</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
    # Ad-hoc signing keeps Gatekeeper quiet about an unsigned local build.
    codesign --force --sign - "$app" >/dev/null 2>&1 || true
    echo "$app"
}

# A normal user cannot open a USB device on Linux without being granted access.
# Without this rule everything fails with a permission error that looks like a
# bug in this program, so it is worth doing at install time.
install_udev_rule() {
    if [ -f "$UDEV_RULE" ]; then
        say "udev rule already present"
        return
    fi
    # The canonical rule ships in packaging/, where distro packages take it
    # from; the inline fallback keeps a bare checkout working.
    local rule
    if [ -f "packaging/udev/70-line6-hx.rules" ]; then
        rule="$(grep -v '^#' packaging/udev/70-line6-hx.rules)"
    else
        rule="SUBSYSTEM==\"usb\", ATTR{idVendor}==\"$LINE6_VENDOR\", MODE=\"0666\", TAG+=\"uaccess\"
SUBSYSTEM==\"tty\", ATTRS{manufacturer}==\"SONULAB\", ATTRS{product}==\"StompStation PRO\", MODE=\"0660\", TAG+=\"uaccess\""
    fi

    if [ "$(id -u)" = 0 ]; then
        printf '%s\n' "$rule" >"$UDEV_RULE"
    elif command -v sudo >/dev/null; then
        say "USB access needs a udev rule; asking for sudo"
        printf '%s\n' "$rule" | sudo tee "$UDEV_RULE" >/dev/null || {
            warn "could not write $UDEV_RULE - run the installer as root, or create it by hand:"
            printf '      %s\n' "$rule" >&2
            return
        }
    else
        warn "no sudo available. Create $UDEV_RULE containing:"
        printf '      %s\n' "$rule" >&2
        return
    fi

    sudo udevadm control --reload-rules >/dev/null 2>&1 || true
    sudo udevadm trigger >/dev/null 2>&1 || true
    say "installed $UDEV_RULE (replug the device to apply)"
}

make_desktop_entry() {
    local gui="$1"
    local icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps"
    mkdir -p "$LINUX_APPS" "$icons"
    # The Exec path is written absolute: desktop launchers do not share the
    # shell's PATH, and a bare command name quietly fails there.
    cat >"$LINUX_APPS/$APP_SLUG.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Version=1.0
Name=$APP_NAME
GenericName=Guitar Processor Editor
Comment=Editor for Line 6 HX and Sonulab StompStation PRO hardware
Exec=$gui
Terminal=false
Categories=AudioVideo;Audio;
Keywords=Line 6;HX;Helix;Sonulab;StompStation;VoidX;stomp;pedal;guitar;preset;tone;
Icon=$APP_SLUG
StartupNotify=false
DESKTOP
    [ -f "packaging/icons/$APP_SLUG.svg" ] &&
        install -m 644 "packaging/icons/$APP_SLUG.svg" "$icons/$APP_SLUG.svg"
    update-desktop-database "$LINUX_APPS" >/dev/null 2>&1 || true
    gtk-update-icon-cache "${icons%/hicolor*}/hicolor" >/dev/null 2>&1 || true
    echo "$LINUX_APPS/$APP_SLUG.desktop"
}

main() {
    local cli_only=0
    case "${1:-}" in
    --uninstall) uninstall ;;
    --cli-only) cli_only=1 ;;
    --help | -h)
        sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    esac

    command -v cargo >/dev/null || die "Rust is not installed. Get it from https://rustup.rs"

    say "building (this takes a few minutes the first time)"
    if [ "$cli_only" = 1 ]; then
        cargo build --release -p tonepush-cli
    else
        cargo build --release
    fi

    # Before installing, not after: leaving both on PATH for even a moment is
    # what lets somebody launch the wrong one.
    retire_former_name

    local dir
    dir="$(bin_dir)"
    install -m 755 target/release/tonepush "$dir/tonepush"
    say "installed $dir/tonepush"

    if [ "$cli_only" = 0 ] && [ "$(uname)" = "Darwin" ]; then
        mkdir -p "$MAC_APPS"
        say "installed $(make_app_bundle target/release/tonepush-gui)"
    elif [ "$cli_only" = 0 ]; then
        install -m 755 target/release/tonepush-gui "$dir/tonepush-gui"
        say "installed $dir/tonepush-gui"
        say "installed $(make_desktop_entry "$dir/tonepush-gui")"
    fi

    if [ "$(uname)" = "Linux" ]; then
        install_udev_rule
    fi

    # Model names, parameter ranges and artwork all come from HX Edit's own
    # data files. Set them up now so the editor is useful the first time it
    # opens rather than showing bare numbers.
    if [ ! -d "$HX_RESOURCES" ]; then
        if ./tools/hxresources/extract.sh >/dev/null 2>&1; then
            say "extracted HX Edit's model data to $HX_RESOURCES"
        else
            warn "no HX Edit found, so models will show as numbers without names or pictures.
      Fix it with: tools/hxresources/extract.sh /path/to/HX_Edit.dmg
      (or .exe - download from https://line6.com/software/)"
        fi
    fi

    case ":$PATH:" in
    *":$dir:"*) ;;
    *) warn "$dir is not on your PATH. Add it with:
      echo 'export PATH=\"$dir:\$PATH\"' >> ~/.zshrc" ;;
    esac

    echo
    say "ready. Quit any other pedal editor first - device sessions are exclusive - then:"
    echo "      tonepush list      find your device"
    echo "      tonepush chain     show the loaded preset"
    if [ "$cli_only" = 0 ]; then
        if [ "$(uname)" = "Darwin" ]; then
            echo "      open -a $APP_NAME    the editor"
        else
            echo "      tonepush-gui       the editor"
        fi
    fi
    echo
}

main "$@"
