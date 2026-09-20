#!/bin/sh
# Install g13.
#
# One script for every shape: the tarball runs this, and the .deb lays the same files out and runs it too. It never
# overwrites anything of yours - the defaults are copied only where there is nothing already, so upgrading a machine
# that has been in use for months leaves every binding, applet and font exactly as it was.
#
# What it needs root for is one line: the udev rule, because the pad is a USB device and the kernel has to be told to
# let you write to it. Everything else goes in your own home. If you would rather place that rule yourself, run this
# without root and it will tell you the one command it could not run.

set -eu

PREFIX="${PREFIX:-$HOME/.local}"
CONFIG="${G13_CONFIG_DIR:-$HOME/.config/g13}"
BIN_DIR="$PREFIX/bin"
UNIT_DIR="$HOME/.config/systemd/user"
RULE=/etc/udev/rules.d/70-g13-record.rules

HERE=$(cd "$(dirname "$0")" && pwd)
# the tree is either a checkout (packaging/ beside defaults/) or the .deb's own layout
if [ -d "$HERE/../defaults" ]; then
    DEFAULTS="$HERE/../defaults"
elif [ -d "$HERE/../share/g13/defaults" ]; then
    DEFAULTS="$HERE/../share/g13/defaults"
else
    echo "g13: cannot find the defaults folder beside this script" >&2
    exit 1
fi

say() { printf '  %s\n' "$*"; }

echo "Installing g13."
echo

# 1. the binary
mkdir -p "$BIN_DIR"
if [ -f "$HERE/g13" ]; then
    install -m 0755 "$HERE/g13" "$BIN_DIR/g13"
elif [ -f "$HERE/../g13" ]; then
    install -m 0755 "$HERE/../g13" "$BIN_DIR/g13"
else
    echo "g13: the binary is not beside this script" >&2
    exit 1
fi
say "the driver:      $BIN_DIR/g13"

# 2. the defaults, and only where there is nothing already
copied=0
skipped=0
mkdir -p "$CONFIG"
for part in applets fonts themes; do
    [ -d "$DEFAULTS/$part" ] || continue
    mkdir -p "$CONFIG/$part"
    for file in "$DEFAULTS/$part"/*; do
        [ -e "$file" ] || continue
        name=$(basename "$file")
        if [ -e "$CONFIG/$part/$name" ]; then
            skipped=$((skipped + 1))
        else
            cp "$file" "$CONFIG/$part/$name"
            copied=$((copied + 1))
        fi
    done
done
for file in "$DEFAULTS"/*.json "$DEFAULTS"/*.properties; do
    [ -e "$file" ] || continue
    name=$(basename "$file")
    if [ -e "$CONFIG/$name" ]; then
        skipped=$((skipped + 1))
    else
        cp "$file" "$CONFIG/$name"
        copied=$((copied + 1))
    fi
done
say "your config:     $CONFIG  ($copied files added, $skipped you already had, none touched)"

# 3. the user service, which is how the driver is meant to run
if [ -f "$HERE/g13-rs.service" ]; then
    SERVICE="$HERE/g13-rs.service"
elif [ -f "$HERE/../packaging/g13-rs.service" ]; then
    SERVICE="$HERE/../packaging/g13-rs.service"
else
    SERVICE=""
fi
if [ -n "$SERVICE" ]; then
    mkdir -p "$UNIT_DIR"
    cp "$SERVICE" "$UNIT_DIR/g13-rs.service"
    say "the service:     $UNIT_DIR/g13-rs.service"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload >/dev/null 2>&1 || true
        say "                 start it with:  systemctl --user enable --now g13-rs"
    fi
fi

# 3b. the menu entry, so the window is in the applications list like anything else
if [ -f "$HERE/g13.desktop" ]; then
    APPS="$HOME/.local/share/applications"
    mkdir -p "$APPS"
    cp "$HERE/g13.desktop" "$APPS/g13.desktop"
    say "the menu entry:  $APPS/g13.desktop"
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS" 2>/dev/null || true
fi

# 4. the one line that wants root
if [ -f "$HERE/70-g13-record.rules" ]; then
    RULE_SRC="$HERE/70-g13-record.rules"
elif [ -f "$HERE/../packaging/70-g13-record.rules" ]; then
    RULE_SRC="$HERE/../packaging/70-g13-record.rules"
else
    RULE_SRC=""
fi
if [ -n "$RULE_SRC" ]; then
    if [ "$(id -u)" = 0 ]; then
        install -m 0644 "$RULE_SRC" "$RULE"
        udevadm control --reload-rules 2>/dev/null || true
        udevadm trigger 2>/dev/null || true
        say "the udev rule:   $RULE"
    elif [ -e "$RULE" ]; then
        say "the udev rule:   already at $RULE, left alone"
    else
        say "the udev rule:   needs root, and this is not root. run:"
        say "                 sudo install -m 0644 $RULE_SRC $RULE && sudo udevadm control --reload-rules"
    fi
fi

echo
echo "Done. Two things to do by hand, and then the pad works:"
echo "  1. log out and back in, so the udev rule applies to your session"
echo "  2. run:  g13 gui      to set the pad up, and  g13 run   to take it"
echo
if ! echo "$PATH" | grep -q "$BIN_DIR"; then
    echo "Note: $BIN_DIR is not on your PATH. Add it, or run $BIN_DIR/g13 directly."
fi
