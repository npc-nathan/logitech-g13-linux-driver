#!/usr/bin/env python3
"""Tests the Cyberpunk 2077 mod without the game.

Three things:
  1. the mod is valid Lua (luac -p), so it cannot fail silently in game with a syntax error;
  2. its logic, driven by stubs of the game API (tests/cet-mod-test.lua), produces the fields
     it should - including when a game build refuses calls the mod is not sure about;
  3. the file it writes is real JSON, and the applet the Linux side ships reads it and lays out
     without putting text on top of ink.

Needs lua5.4 (apt install lua5.4). Skips with a warning if it is missing.

    python3 tests/cet-mod-test.py
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

CET = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MOD = os.path.join(CET, "init.lua")
STUB = os.path.join(CET, "tests", "cet-mod-test.lua")
APPLET = os.path.join(CET, "applets", "cp2077-hud.json")

failures = 0


def check(what, expected, actual):
    global failures
    ok = expected == actual
    if not ok:
        failures += 1
    print("%-56s %-22s %s" % (what, "-> " + repr(actual), "ok" if ok else
                              "FAIL (expected %r)" % (expected,)))


def main():
    lua = shutil.which("lua5.4") or shutil.which("lua")
    luac = shutil.which("luac5.4") or shutil.which("luac")
    if not lua or not luac:
        print("lua5.4 is not installed (apt install lua5.4): skipping the mod tests")
        return 0

    # 1. Syntax: a broken mod fails inside the game with nothing to go on.
    result = subprocess.run([luac, "-p", MOD], capture_output=True, text=True)
    check("the mod is valid Lua", 0, result.returncode)
    if result.returncode != 0:
        print(result.stderr.strip())

    # 2. Logic, with the game stubbed out. The mod writes into the current directory, which is
    #    why this runs in a scratch one.
    scratch = tempfile.mkdtemp(prefix="g13-cet-")
    environment = dict(os.environ, G13_TEST_MOD=MOD)
    result = subprocess.run([lua, STUB], capture_output=True, text=True, cwd=scratch,
                            env=environment, timeout=60)
    print(result.stdout.strip())
    if result.returncode != 0:
        print(result.stderr.strip())
    check("the mod's own checks pass", 0, result.returncode)

    # 3. The file it wrote has to be JSON the Linux side can read, with the applet's fields.
    state_path = os.path.join(scratch, "hud.json")
    check("it wrote hud.json", True, os.path.exists(state_path))
    if os.path.exists(state_path):
        try:
            state = json.load(open(state_path))
        except ValueError as error:
            state = None
            print("hud.json is not valid JSON:", error)
        check("hud.json parses as JSON", True, state is not None)
        if state:
            for field in ("health", "level", "objective", "heading", "updated"):
                check("hud.json carries %s" % field, True, field in state)

        # The applet, pointed at that file, has to be one this driver can read: it parses, and
        # every source it names resolves. The layout and the text-on-ink rule belong to the driver,
        # whose own tests apply them to every applet, so this asks the driver for its verdict rather
        # than keeping a second opinion about it here.
        if os.path.exists(APPLET):
            binary = shutil.which("g13")
            if binary is None:
                print("g13 is not on PATH - skipping the applet check")
            else:
                config = os.path.join(scratch, "config")
                applets = os.path.join(config, "applets")
                os.makedirs(applets, exist_ok=True)
                definition = json.load(open(APPLET))
                for name, source in list((definition.get("sources") or {}).items()):
                    if "hud.json" in source:
                        definition["sources"][name] = "json:%s#%s" % (
                            state_path, source.rsplit("#", 1)[-1])
                name = os.path.basename(APPLET)[: -len(".json")]
                with open(os.path.join(applets, os.path.basename(APPLET)), "w") as handle:
                    json.dump(definition, handle)
                result = subprocess.run([binary, "applet", "check", name],
                                        capture_output=True, text=True,
                                        env=dict(os.environ, G13_CONFIG_DIR=config))
                check("the driver reads the applet", True, result.returncode == 0)
                if result.returncode != 0:
                    print("   " + (result.stderr or result.stdout).strip())

    print("CET MOD TEST: all checks passed" if failures == 0
          else "CET MOD TEST: %d FAILURES" % failures)
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
