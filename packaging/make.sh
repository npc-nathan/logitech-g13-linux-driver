#!/bin/sh
# Build the shapes g13 ships in.
#
#   ./packaging/make.sh          a tarball with an install script, and a .deb
#
# The .deb lays the same files out under /usr and /usr/share and adds nothing of its own; the tarball runs
# packaging/install.sh. Both put the same defaults in the same places, so a machine installed either way is the
# same machine.

set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
OUT="${1:-$ROOT/target/package}"
VERSION=$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)
HOMEPAGE=$(grep -m1 '^repository' "$ROOT/Cargo.toml" | cut -d'"' -f2)
VERSION=${VERSION:-0.1.0}
NAME=g13

cd "$ROOT"
echo "Building $NAME $VERSION."
cargo build --release

rm -rf "$OUT"
mkdir -p "$OUT"

# ---- the tarball: the binary, the installer, its files, and the defaults beside them
STAGE="$OUT/$NAME-$VERSION"
mkdir -p "$STAGE"
install -m 0755 "$ROOT/target/release/g13" "$STAGE/g13"
install -m 0755 "$HERE/install.sh" "$STAGE/install.sh"
install -m 0644 "$HERE/g13-rs.service" "$STAGE/g13-rs.service"
install -m 0644 "$HERE/g13.desktop" "$STAGE/g13.desktop"
install -m 0644 "$HERE/70-g13-record.rules" "$STAGE/70-g13-record.rules"
cp -r "$ROOT/defaults" "$STAGE/../defaults"
for doc in README.md; do
    [ -f "$ROOT/$doc" ] && install -m 0644 "$ROOT/$doc" "$STAGE/$doc"
done
[ -d "$ROOT/docs" ] && cp -r "$ROOT/docs" "$STAGE/docs"
tar -czf "$OUT/$NAME-$VERSION.tar.gz" -C "$OUT" "$NAME-$VERSION" defaults
rm -rf "$STAGE" "$OUT/defaults"
find "$OUT" -maxdepth 1 -name 'defaults' -prune -exec rm -rf {} + 2>/dev/null || true
echo "  $OUT/$NAME-$VERSION.tar.gz"

# ---- the .deb: the same files, laid out for the package manager to own
DEB="$OUT/deb"
mkdir -p "$DEB/DEBIAN" "$DEB/usr/bin" "$DEB/usr/share/g13" "$DEB/usr/lib/udev/rules.d" "$DEB/usr/lib/systemd/user" "$DEB/usr/share/applications" "$DEB/usr/share/doc/$NAME"
install -m 0755 "$ROOT/target/release/g13" "$DEB/usr/bin/g13"
cp -r "$ROOT/defaults" "$DEB/usr/share/g13/defaults"
install -m 0644 "$HERE/70-g13-record.rules" "$DEB/usr/lib/udev/rules.d/70-g13-record.rules"
# the deb installs the binary to /usr/bin, so its unit must say so: the unit in packaging/ is written for the
# tarball, which puts the binary in the user's own ~/.local/bin. One unit, each shape's own path.
sed 's|%h/.local/bin/g13|/usr/bin/g13|' "$HERE/g13-rs.service" > "$DEB/usr/lib/systemd/user/g13-rs.service"
install -m 0644 "$HERE/g13.desktop" "$DEB/usr/share/applications/g13.desktop"
chmod 0644 "$DEB/usr/lib/systemd/user/g13-rs.service"
grep -q '^ExecStart=/usr/bin/g13 run$' "$DEB/usr/lib/systemd/user/g13-rs.service" || {
    echo "packaging: the deb's unit does not point at /usr/bin/g13" >&2
    exit 1
}
[ -f "$ROOT/README.md" ] && install -m 0644 "$ROOT/README.md" "$DEB/usr/share/doc/$NAME/README.md"
[ -f "$ROOT/LICENSE-MIT" ] && install -m 0644 "$ROOT/LICENSE-MIT" "$DEB/usr/share/doc/$NAME/LICENSE-MIT"
[ -f "$ROOT/LICENSE-APACHE" ] && install -m 0644 "$ROOT/LICENSE-APACHE" "$DEB/usr/share/doc/$NAME/LICENSE-APACHE"
# The three the binary links against, read off the binary rather than remembered. dpkg takes no comments in a
# control file, so the reason lives here: these are the libraries `ldd` reports, and without them the package
# installs and then cannot start.
SIZE=$(du -sk "$DEB" | cut -f1)
cat > "$DEB/DEBIAN/control" <<EOF
Package: $NAME
Version: $VERSION
Section: utils
Priority: optional
Architecture: $(dpkg --print-architecture 2>/dev/null || echo amd64)
Maintainer: Nathan Calow (NPC-IT) <nathan@npc-it.co.uk>
Description: Logitech G13 gameboard driver
 Reads the pad, sends keys through its own virtual keyboard, draws on the LCD,
 and renders applets of your own. The configuration window is g13 gui; the
 driver is g13 run or the g13-rs user service.
Installed-Size: $SIZE
License: MIT OR Apache-2.0
Homepage: $HOMEPAGE
Depends: libusb-1.0-0, libudev1, libcap2
EOF
cat > "$DEB/DEBIAN/postinst" <<'EOF'
#!/bin/sh
# The package owns the udev rule and the user service; the defaults in /usr/share/g13 are copied into a user's
# own config the first time they run `g13 setup`, not here, because root has no business writing into a home.
set -e
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger 2>/dev/null || true
# A user unit cannot be started from here - there is no session of the user's to start it in, and pretending
# otherwise is how packages end up doing nothing quietly. What root *can* do is enable it for every user, so the
# driver comes up on its own the next time they log in. Starting it stays the user's own act, which is also the
# only way `g13 gui` and `g13 run` stay two separate things.
if command -v systemctl >/dev/null 2>&1; then
    systemctl --global enable g13-rs.service 2>/dev/null || true
fi
echo "g13 is installed. The driver sets itself up the first time you run it."
echo "Start the window with:  g13 gui"
echo "It is enabled for every user, so it also takes the pad by itself from your next login."
echo "To start it now instead:  systemctl --user start g13-rs"
EOF
chmod 0755 "$DEB/DEBIAN/postinst"
cat > "$DEB/DEBIAN/prerm" <<'EOF'
#!/bin/sh
# Best effort, and honestly so: this runs as root, and a user unit belongs to that user's own session, so a
# `systemctl --user` from here has no bus to talk to and does nothing. It is kept because it is correct wherever
# it can reach one - and either way the unit file going away means the driver does not come back at the next
# login.
set -e
systemctl --user stop g13-rs.service 2>/dev/null || true
EOF
chmod 0755 "$DEB/DEBIAN/prerm"
cat > "$DEB/DEBIAN/postrm" <<'EOF'
#!/bin/sh
# Undo what the postinst did. `systemctl --global enable` writes a symlink under /etc/systemd/user/, which is
# outside the file list dpkg knows about: without this the service still starts at every login after the
# package has gone.
#
# On remove and purge only. dpkg calls this on an upgrade too, where the postinst is about to enable it again,
# and disabling there would leave an upgraded machine with no driver at its next login.
set -e
case "$1" in
remove|purge)
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --global disable g13-rs.service 2>/dev/null || true
    fi
    rm -f /etc/systemd/user/default.target.wants/g13-rs.service
    rm -f /etc/systemd/user/g13-rs.service
    # and drop the rule from the running set: the file going away does not un-apply it, so until this the
    # device keeps the access the package granted it
    udevadm control --reload-rules 2>/dev/null || true
    udevadm trigger 2>/dev/null || true
    ;;
esac
exit 0
EOF
chmod 0755 "$DEB/DEBIAN/postrm"
dpkg-deb --build --root-owner-group "$DEB" "$OUT/${NAME}_${VERSION}_$(dpkg --print-architecture 2>/dev/null || echo amd64).deb" >/dev/null
rm -rf "$DEB"
echo "  $OUT/${NAME}_${VERSION}_$(dpkg --print-architecture 2>/dev/null || echo amd64).deb"
