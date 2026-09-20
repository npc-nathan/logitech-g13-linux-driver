# Cyberpunk 2077 on the pad

The pad can show the game's own numbers while you play  -  health, level, the tracked objective, and
the distance and direction to your map pin. It is two halves sharing one file:

```
the game ──[ the CET mod, g13-hud ]──▶ <game>/…/mods/g13-hud/hud.json ──[ json: ]──▶ the applet
```

The applet is `cp2077-hud`, and its 14 sources all read that one file. The data source that reads
JSON is described in [Applets and sources](applets-and-sources.md); this page is just the two halves
and the order to do them in.

**The game side is not part of this package.** It is a Cyber Engine Tweaks mod kept in
[`cet-mod/`](../cet-mod) in this repository. Nothing installs it for you, no package ships it, and
the driver works exactly the same without it.

## Install it

You need Cyberpunk 2077 with **Cyber Engine Tweaks 1.37 or newer** already installed in it  -  if CET
is not there, the installer says so and stops.

```bash
cd cet-mod
./install.sh                          # looks in the usual places for the game
./install.sh "/path/to/Cyberpunk 2077"    # or say where it is
```

It does two things, and it says which:

- copies the mod into `<game>/bin/x64/plugins/cyber_engine_tweaks/mods/g13-hud/`;
- copies the applet to `~/.config/g13/applets/cp2077-hud.json`, with the game's path filled in, and
  adds it to `visuals.json` so the pad can walk to it.

Re-running it is how you update. Nothing outside those two places is touched.

## Put it on the pad

Open the window  -  **G13 Configuration** in your applications menu, or `g13 gui`  -  go to the
**Menu** tab, and tick **`on the pad`** for **cp2077-hud**.

The pad then walks to it with **LR**, the same way it walks to any other screen. The numbers move
while the game is running; with the game closed they stay where they were left, because the file is
the only thing the two halves share.

![the game's own numbers on the pad](images/cp2077-hud.png)

## Take it away

```bash
cd cet-mod
./install.sh --remove
```

That removes the mod and the applet and takes the applet back out of the rotation. It never touches
your game's other mods or your other applets.

## When a number is blank

The mod only writes what the game will answer, and a build that refuses a call leaves the field out
of the file rather than writing a wrong number  -  so a blank on the screen is a reading your game
did not give, not a broken applet. `ammo`, `ammo_total`, `weapon` and the pin are the ones that
differ between builds.

To find out what your build answers, open the CET console in game and run:

```lua
G13Probe()
```

It prints what the game allows for the player, the held weapon, the four stats and the tracked
journal entry. `cet-mod/README.md` says which line in `init.lua` to wire to it.

## Checking it without starting the game

```bash
python3 cet-mod/tests/cet-mod-test.py    # valid Lua, and the right fields from a stub game API
g13 applet check cp2077-hud              # the applet parses and every source it names resolves
g13 applet preview cp2077-hud            # what each of its sources reads right now
```

The applet's fields are the mod's own file, so with the game closed `applet preview` shows whatever
was written last  -  which is also the easiest way to see which fields your game is filling in.
