#!/bin/sh
# Build the archive the mod is published as.
#
#   ./packaging.sh              writes dist/g13-hud-<version>.zip
#
# It contains one file, at the path Cyber Engine Tweaks loads it from:
#
#   bin/x64/plugins/cyber_engine_tweaks/mods/g13-hud/init.lua
#
# That is the whole mod. Everything in an archive is deployed into the game folder, so anything else put in here -
# a README, an installer, the pad screen - would land in the player's game directory as a stray file. A mod manager
# installs it as it stands: the archive's paths are game-root relative, which is what Amethyst, Vortex and a manual
# drag-and-drop all expect. The comparable Nexus mods are single-file archives exactly like this one.
#
# The zip is written deterministically - a fixed timestamp and no entry marked executable - so the same tree always
# produces the same archive, and nothing in it can be cited as an executable by a mod site's scanner.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
VERSION=$(sed -n 's/^local VERSION = "\(.*\)"/\1/p' "$HERE/init.lua" | head -1)
[ -n "$VERSION" ] || { echo "packaging: no VERSION in init.lua" >&2; exit 1; }

OUT="$HERE/dist"
NAME="g13-hud-$VERSION.zip"
STAGE="$OUT/stage"

rm -rf "$STAGE"
mkdir -p "$STAGE/bin/x64/plugins/cyber_engine_tweaks/mods/g13-hud"
install -m 0644 "$HERE/init.lua" "$STAGE/bin/x64/plugins/cyber_engine_tweaks/mods/g13-hud/init.lua"

python3 - "$STAGE" "$OUT/$NAME" <<'PY'
import pathlib, sys, zipfile
stage, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    for path in sorted(stage.rglob("*")):
        if path.is_file():
            info = zipfile.ZipInfo(path.relative_to(stage).as_posix(), date_time=(1980, 1, 1, 0, 0, 0))
            info.external_attr = 0o644 << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, path.read_bytes())
print(f"  {out}  ({out.stat().st_size / 1024:.1f} KB, {len(zipfile.ZipFile(out).infolist())} file)")
PY

rm -rf "$STAGE"
