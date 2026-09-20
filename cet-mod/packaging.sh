#!/bin/sh
# Build the archives the mod is published as.
#
#   ./packaging.sh              writes dist/g13-hud-<version>.zip and dist/g13-hud-<version>-linux.zip
#
# The main archive is the mod and nothing else: the Lua where Cyber Engine Tweaks loads it, and the README. No
# scripts, no executables, nothing for an automated scanner to quarantine - which is what happened when the Linux
# installer travelled inside it. It is also the only file Windows, Steam and GOG users need: they copy the bin folder
# in and they are done.
#
# The Linux half rides in its own archive, as an optional file: the pad screen, and the installer that puts both
# halves in place. That script is stored without the executable bit - it is a text file that happens to be valid sh -
# so it is run as `sh install.sh`, and there is no "executable" in the archive for a scanner to flag.
#
# Both are written deterministically: entries in sorted order with a fixed timestamp, so the same tree always
# produces the same archives and a download can be checked against a rebuild.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
VERSION=$(sed -n 's/^local VERSION = "\(.*\)"/\1/p' "$HERE/init.lua" | head -1)
[ -n "$VERSION" ] || { echo "packaging: no VERSION in init.lua" >&2; exit 1; }

OUT="$HERE/dist"
MODS="bin/x64/plugins/cyber_engine_tweaks/mods/g13-hud"
rm -rf "$OUT/stage-main" "$OUT/stage-linux"

# ---- the mod: pure data, the file everyone downloads
mkdir -p "$OUT/stage-main/$MODS"
install -m 0644 "$HERE/init.lua" "$OUT/stage-main/$MODS/init.lua"
install -m 0644 "$HERE/README.md" "$OUT/stage-main/README.md"

# ---- the Linux half: the pad screen, and the installer, as an optional file
mkdir -p "$OUT/stage-linux/linux/applets"
install -m 0644 "$HERE/install.sh" "$OUT/stage-linux/linux/install.sh"
install -m 0644 "$HERE/applets/cp2077-hud.json" "$OUT/stage-linux/linux/applets/cp2077-hud.json"
cat > "$OUT/stage-linux/linux/README.md" <<'TXT'
# g13-hud - the Linux half

The main archive is the mod. This one is for Linux, where the pad also has to be told about the new screen: it
carries the installer, which puts the HUD's screen on the G13 and adds it to the rotation.

    sh install.sh                            # or: sh install.sh "/path/to/Cyberpunk 2077"
    sh install.sh --remove                   # takes the screen and the applet away again

It copies the same `init.lua` to the same place as the manual install - so if you have already copied the `bin`
folder in, running this only adds the pad screen. Nothing else is touched, and re-running it is how to update.

The leading `sh` is not a typo: the script is stored as a plain text file, deliberately, so the archive contains
nothing marked executable.

Needs the `g13` driver, on Linux, with Cyber Engine Tweaks already installed in the game:
https://github.com/npc-nathan/logitech-g13-linux-driver
TXT

python3 - "$OUT" "$VERSION" <<'PY'
import pathlib, sys, zipfile
out = pathlib.Path(sys.argv[1]); version = sys.argv[2]
for stage, name in ((out / "stage-main", f"g13-hud-{version}.zip"),
                    (out / "stage-linux", f"g13-hud-{version}-linux.zip")):
    target = out / name
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as z:
        for path in sorted(stage.rglob("*")):
            if path.is_file():
                info = zipfile.ZipInfo(path.relative_to(stage).as_posix(), date_time=(1980, 1, 1, 0, 0, 0))
                info.external_attr = 0o644 << 16          # never executable, in either archive
                info.compress_type = zipfile.ZIP_DEFLATED
                z.writestr(info, path.read_bytes())
    print(f"  {target}  ({target.stat().st_size / 1024:.1f} KB)")
PY

rm -rf "$OUT/stage-main" "$OUT/stage-linux"
