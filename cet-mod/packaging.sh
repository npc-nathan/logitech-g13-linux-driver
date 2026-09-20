#!/bin/sh
# Build the archive the mod is published as.
#
#   ./packaging.sh              writes dist/g13-hud-<version>.zip - one file, and the only one
#
# The layout is the one Cyber Engine Tweaks loads, so the archive can be dropped into a game folder and merge. Two
# things are deliberately absent:
#
#   * no shell script. Nexus quarantined the first upload because the installer travelled inside it, and their first
#     stated reason is "executables and similar file types". install.sh stays in the repository for anyone who wants
#     it; the archive carries the same two steps as instructions instead, which no scanner objects to.
#   * nothing compiled, nothing binary, nothing nested. Every entry below is text.
#
# The zip is written deterministically - entries in sorted order, a fixed timestamp, and no entry marked executable -
# so the same tree always produces the same archive and a download can be checked against a rebuild.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
VERSION=$(sed -n 's/^local VERSION = "\(.*\)"/\1/p' "$HERE/init.lua" | head -1)
[ -n "$VERSION" ] || { echo "packaging: no VERSION in init.lua" >&2; exit 1; }

OUT="$HERE/dist"
MODS="bin/x64/plugins/cyber_engine_tweaks/mods/g13-hud"
STAGE="$OUT/stage"

rm -rf "$STAGE"
mkdir -p "$STAGE/$MODS" "$STAGE/linux/applets"
install -m 0644 "$HERE/init.lua" "$STAGE/$MODS/init.lua"
install -m 0644 "$HERE/README.md" "$STAGE/README.md"
install -m 0644 "$HERE/applets/cp2077-hud.json" "$STAGE/linux/applets/cp2077-hud.json"

python3 - "$STAGE" "$OUT/g13-hud-$VERSION.zip" <<'PY'
import pathlib, sys, zipfile
stage, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    for path in sorted(stage.rglob("*")):
        if path.is_file():
            info = zipfile.ZipInfo(path.relative_to(stage).as_posix(), date_time=(1980, 1, 1, 0, 0, 0))
            info.external_attr = 0o644 << 16                 # never executable
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, path.read_bytes())
print(f"  {out}  ({out.stat().st_size / 1024:.1f} KB)")
PY

rm -rf "$STAGE"
