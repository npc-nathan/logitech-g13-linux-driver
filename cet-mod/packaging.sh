#!/bin/sh
# Build the archive the mod is published as.
#
#   ./packaging.sh              writes dist/g13-hud-<version>.zip
#
# The layout is the one Cyber Engine Tweaks loads: the Lua goes where CET looks for mods, so the archive can be
# dropped into a game folder and merge. The Linux half rides along under linux/ - the pad screen and the installer
# that puts both halves in place - because a Windows user only needs the bin/ folder and a Linux user needs the rest.
#
# The zip is written deterministically: entries in sorted order with a fixed timestamp, so the same tree always
# produces the same archive and a download can be checked against a rebuild.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
VERSION=$(sed -n 's/^local VERSION = "\(.*\)"/\1/p' "$HERE/init.lua" | head -1)
[ -n "$VERSION" ] || { echo "packaging: no VERSION in init.lua" >&2; exit 1; }

OUT="$HERE/dist"
NAME="g13-hud-$VERSION.zip"
STAGE="$OUT/stage"
MODS="bin/x64/plugins/cyber_engine_tweaks/mods/g13-hud"

rm -rf "$STAGE"
mkdir -p "$STAGE/$MODS" "$STAGE/linux/applets"
install -m 0644 "$HERE/init.lua" "$STAGE/$MODS/init.lua"
install -m 0644 "$HERE/README.md" "$STAGE/README.md"
install -m 0755 "$HERE/install.sh" "$STAGE/linux/install.sh"
install -m 0644 "$HERE/applets/cp2077-hud.json" "$STAGE/linux/applets/cp2077-hud.json"

python3 - "$STAGE" "$OUT/$NAME" <<'PY'
import pathlib, sys, zipfile
stage, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    for path in sorted(stage.rglob("*")):
        if path.is_file():
            info = zipfile.ZipInfo(path.relative_to(stage).as_posix(), date_time=(1980, 1, 1, 0, 0, 0))
            info.external_attr = (0o755 if path.suffix == ".sh" else 0o644) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, path.read_bytes())
PY

rm -rf "$STAGE"
echo "  $OUT/$NAME"
