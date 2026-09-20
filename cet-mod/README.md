# g13-hud  -  Cyberpunk 2077's own numbers on a Logitech G13 screen

A Cyber Engine Tweaks mod that writes the game's numbers to a small JSON file, about five times a
second. The Linux side reads that file with the driver's `json:` data source and draws it, so the
screen is designed on the Linux side: **this mod is data, not a picture.**

It writes one file inside its own folder and changes nothing about the game. Nothing here is
installed by the `g13` package: it is separate, optional, and `install.sh` is the only thing in it
that touches a game folder.

## Install

```bash
./install.sh                          # default game: ~/Games/Heroic/Cyberpunk 2077
./install.sh "/path/to/Cyberpunk 2077"
./install.sh --remove                 # takes the mod and the applet away again
```

That copies `init.lua` into `<game>/bin/x64/plugins/cyber_engine_tweaks/mods/g13-hud/`, copies the
applet from `applets/` to `~/.config/g13/applets/cp2077-hud.json` with its `{GAME}` placeholder
replaced by wherever the game actually is, and adds that applet to `visuals.json` so the pad can
walk to it. Nothing outside those places is touched, and re-running it is how to update.

**From the published archive** - `./packaging.sh` builds `dist/g13-hud-<version>.zip` in the layout CET loads, with
**no scripts in it**: mod sites' automated scanners quarantine archives that carry executables or shell scripts, and
a `.sh` beside the Lua was enough to do it.

Copy the `bin` folder from inside it into your game folder (the one holding `Cyberpunk2077.exe`) and let it merge -
that is the whole install, and it puts the file at the same path `install.sh` does.

On Linux the pad also needs the screen, which is two short steps: copy `linux/applets/cp2077-hud.json` from the
archive into `~/.config/g13/applets/`, then open the g13 window, go to the **Menu** tab and tick **on the pad** for
`cp2077-hud`. That second step is the same one any applet needs - the window writes the rotation for you.

`install.sh` in this repository does both of those in one command, if you would rather run a script than copy two
files. It is kept out of the published archive on purpose.

Written and tested against **Cyberpunk 2077 3.0.80.51928** with **CET v1.37.1** (1.37 or newer is required).

Needs CET 1.37+ (written against 1.37.1) and `g13` (the driver) installed. If CET is not in the
game yet, the installer says so and stops.

## On the pad

**Menu** tab in the `g13` window → tick **`on the pad`** for **cp2077-hud** → the pad then walks to
it with **LR**, the way it walks to any other screen.

With the game closed the numbers simply stay where they were left  -  the file is the only thing the
two halves share, so nothing here can hold up the driver, and the driver cannot break the game.

## What it reads, and how sure that is

Every call below was checked against mods already running in this installation, not against
documentation:

| field | source | confidence |
| --- | --- | --- |
| `health`, `stamina`, `level`, `streetcred` | `Game.GetStatsSystem():GetStatValue(entityId, 'Health')` | verified in use |
| `objective`, `quest`, `objective_id`, `quest_id` | `Game.GetJournalManager():GetTrackedEntry()` then two `GetParentEntry()` steps | verified in use |
| `weapon` | the active weapon's `GetName()` loc key through `GetLocalizedText` | verified in use |
| `x`, `y`, `z`, `heading` | `GetWorldPosition()`, `GetWorldForward()` | verified in use |
| `district` | `PreventionSystem.districtManager:GetCurrentDistrict()` → its TweakDB record's `LocalizedName()` | pattern verified in use |
| `near` | the nearest map mappin that has a name | pattern verified in use |
| `pin_bearing`, `pin_distance`, `pin_relative`, `route` | `GetMappinSystem():GetMappins(Map)`, the mappin whose variant is `CustomPositionVariant` | **probed**: see `G13Probe()` |
| `ammo`, `ammo_total`, `ammo_max` | the weapon's magazine/ammo calls | **probed**: whichever the build answers |

Anything that cannot be read is simply **absent from the file**  -  a game build that refuses a call
gives a blank on the screen, never an error. `tests/cet-mod-test.py` runs the mod against a stub
that refuses those calls, to keep it that way.

## Finding out what your build answers

In the CET console:

```lua
G13Probe()
```

It prints what this build allows for the player, the held weapon, the four stats and the tracked
journal entry. If `ammo` is blank on the screen, that output shows which call to use and it is one
line to wire in.

## What is not there yet

- **No turn-by-turn.** The game's own route is not readable anywhere found, so there is no next
  corner and no road to follow on the screen. What there is instead: **an arrow to your map pin**  - 
  the game's own custom-position mappin  -  with the distance, pointing relative to which way you are
  facing, so straight up means straight ahead.
- **Street names are not available.** The district you are standing in is (`district`, from the
  PreventionSystem), and the nearest named place (`near`), but nothing found returns a street name.
  `G13Probe()` lists every mappin variant on your map with its distance, so if a street layer exists
  in your build it will show up there.
- **The distance to a pin is a straight line**, and assumes 100 world units to the metre. Drive a
  known distance once with the pin set; if the number is out by a factor, it is one constant
  (`PIN_UNITS_PER_METRE`).
- **The pin variant is matched by name** (`CustomPosition`) as well as by identity, because enum
  tables differ between builds. If no pin is found, `G13Probe()` prints the variants present and the
  name is one line to add.
- **The heading convention is worth a glance in game.** It is written as 0° = north, 90° = east. If
  it reads backwards, it is one line in `init.lua`.
- **`objective` is only the current phase's text.** Sub-objectives and the "N/M" counters some quests
  show in the HUD are not read.
- **`follow` is declared by the applet and acted on by the driver.** It means "while this applet's
  data is fresh, it takes the screen, and it gives it back when the game stops writing"  -  the applet
  carries `follow: {"seconds": 20}`, so the HUD takes the screen while the game is writing `hud.json`
  and hands it back twenty seconds after the writing stops. The screens can also be walked to with LR
  by hand.

## The file it writes

`<mod folder>/hud.json`, replaced whole each time (written beside itself and renamed over, so a
reader never sees half of it):

```json
{
  "mod": "g13-hud", "version": "1.0", "updated": "21:14:07",
  "health": 87, "stamina": 61, "level": 32, "streetcred": 50,
  "objective": "Deliver the package to Vex", "objective_id": "q005_rogue_obj_2",
  "quest": "Rogue's request", "quest_id": "q005_rogue",
  "weapon": "Overture", "ammo": 24, "ammo_total": 180, "ammo_max": 24,
  "x": 100.5, "y": -200.25, "z": 8.0, "heading": 137,
  "district": "Watson", "near": "Megabuilding H10",
  "pin_bearing": 42, "pin_distance": 240, "pin_distance_raw": 24013,
  "pin_relative": 265, "route": "PIN 240m  Watson"
}
```

`route` is one line already made up for a small screen: the pin and its distance when there is one,
otherwise the district and the nearest named place  -  so an applet does not have to choose between
fields it cannot test for itself.

Any program may read it, and any game that can write a file like it can drive the same applet  - 
`json:<path>#<field>` is all the Linux side needs.

## Checking it without the game

```bash
python3 tests/cet-mod-test.py     # valid Lua, and the right fields under a stub game API
g13 applet check cp2077-hud       # the applet parses and every source it names resolves
```

The first needs `lua5.4`; it skips with a warning if it is missing.
