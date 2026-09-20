# Bindings, profiles, the stick and the screen

## The shape of it

Each profile has a file, `~/.config/g13/bindings-N.properties` (N is 0 to 3). It is a plain list of lines:

```
G1=p,k.30
G7=m,203,1
LR=mk,2
MR=rec
```

The name on the left is a control on the pad. The value on the right is what that control does in this profile.
Anything the file does not mention does nothing, except M1, M2 and M3 (see [profiles](#profiles)).

The active profile has its own file, `~/.config/g13/active-profile`, holding one number. The driver re-reads
these files while it runs, so an edit applies within a second  -  no restart.

## The controls

These are the names to use, measured off the pad itself. The number is the bit the control sets in the pad's
report, which is what makes each name mean one physical button.

| controls | bits |
|---|---|
| `G1`-`G22` | 16-37 |
| `LR` (the round button) | 40 |
| `L1`, `L2`, `L3`, `L4` | 41-44 |
| `M1`, `M2`, `M3` | 45, 46, 47 |
| `MR` (Macro Record) | 48 |
| `LMB`, `RMB` (the two buttons beside the stick) | 49, 50 |
| `JCLICK` (pressing the stick) | 51 |
| the stick's directions (`UP`, `DOWN`, `LEFT`, `RIGHT`) | its own two axes, not bits |

Bits 39 and 55 are flags, not controls, and are never bound. `g13 watch` prints every bit that changes, and
`g13 watch --map` walks the pad so you can press a control and see which bit it is.

## What a control can be bound to

| what you write | what it does |
|---|---|
| `x` | nothing  -  the control does nothing at all |
| `p,k.<code>` | sends the keyboard key with that keycode. `p,k.30` is the letter `a`, `p,k.57` is space |
| `m,<id>,<repeats>` | plays macro `<id>`, `<repeats>` times |
| `mk,<N>` | switches to profile `N` |
| `mb,<button>` (or `mb:<button>`) | presses a mouse button |
| `gb,<button>` (or `gb:<button>`) | presses a gamepad button  -  `gb,dpad-left`, `gb,lt`, `gb:a` |
| `sv,next` / `sv,prev` | shows the next or previous of the screens ticked in the window's Screen tab |
| `sv,<visual>` | shows that screen by name, e.g. `sv,clock` or `sv,applet:docker` |
| `sv,<number>` | shows the numbered screen of the ticked ones, counting from 1  -  the way to make L1-L4 four keys to four particular screens |
| `menu` | opens the menu on the pad's screen |
| `rec` | starts the macro recorder (see [macros.md](macros.md)) |
| `rec,<id>` | the same, but recording into macro `<id>` instead of the next free one |

Keycodes are the Linux ones (`/usr/include/linux/input-event-codes.h`). You should not need to know them: **set a
binding by pressing it**. In the window, **Bindings** tab → click the row → **set by pressing** → press the key,
mouse button or gamepad button you want. From a terminal:

```bash
g13 record G7       # then press the key, mouse button or gamepad button
g13 bind G1 a       # or set a key by name
g13 bindings        # what every control does now, as the driver sees it
```

### A hold: something else the same control does

A control can have a second job for when it is held, on its own line:

```
LR=sv,next
LR.hold=menu
```

A tap moves on; a hold opens the menu. A control with no `.hold` line behaves exactly as it did before  -  it fires
the moment it is pressed. One that has one waits for the release, because until then it might have been a hold,
and a hold must not also do the tap's work.

How long a hold is, is a setting in the same file, beside the actions:

```
long_press_ms=500
```

500 unless you say otherwise, and clamped to between 150 ms and 3 s.

**What the menu is.** It opens on the screens you have ticked, named the way you would say them (the applet's own
`title` where it has one)  -  that is its first level. While it is open:

| control | what it does |
|---|---|
| **L1** | back **one level**: it returns to the level this one was opened from, with the cursor where you left it. At the first level there is nothing above, so there it closes the menu |
| **L2** | previous item |
| **L3** | next item |
| **L4** | choose the highlighted one, or open it if it is a level |

**A menu of your own.** `~/.config/g13/menu.json` replaces the rotation when it is there  -  submenus, and an
action per item (show a screen, or run a command). It is built on the window's **Screen** tab, item by item, and
`menu.json` deleted puts the rotation back. Everything below about L1-L4 is unchanged by it, because the keys are
the menu's own wherever its items came from.

**Levels.** When the screen on the pad is an applet with more than one screen, the menu's first item is that
applet, with how many screens it has (`docker  3 screens`). L4 opens its screens as a second level, listed by
the name each screen carries, and L4 on one of those shows it. L1 comes back to the list you came from  -  not
straight out  -  and the list is exactly as you left it.

The bottom line says which of the two L1 will do: `L1 back` one level down, `L1 close` at the first. Both fit
the panel's 26 columns exactly, which matters because a line that does not fit is cut, not wrapped.

The menu is the only thing that takes L1-L4 away from your own bindings, and only while it is open: shut, every
control does exactly what the profile's file says.

### Changing what the pad is showing

The round button, **LR**, shows the next of the screens you have ticked  -  unless the profile's own file binds LR
to something else, exactly as M1-M3 default to profiles. It is written in the file like any other action, so you
can put it on any control:

```
LR=sv,next
G10=sv,prev
G11=sv,applet:docker
```

**L1-L4 are deliberately left free**, because which four of your screens they should be is your choice and not a
default's. To make them four direct keys, with your own list from `visuals.json`:

```
L1=sv,1
L2=sv,2
L3=sv,3
L4=sv,4
```

A number out of range is refused and says what the rotation has, rather than doing nothing.

**L1 while a multi-screen applet is showing.** An applet can hold several screens (see
[applets-and-sources.md](applets-and-sources.md)). While one of those is on the pad, **L1 moves to its next
screen**  -  that is the applet's own control and nothing has to be bound for it. It happens only when there is
something for it to do: the applet must have more than one screen, and no menu may be open. In every other case
L1 keeps whatever the profile's file binds it to, so binding `L1=sv,2` still works and is not quietly taken.

**L2, L3 and L4 while a screen has something to act on.** A widget may carry a `command` (see
[applets-and-sources.md](applets-and-sources.md)); those widgets are what **L2**/**L3** move between  -  the
chosen one gets a border  -  and **L4** runs the chosen one's command. A `list` with a command is walked by its
rows, so `{screen_item}` is the row you are on. As with L1 it happens only when there is something to do: a
screen where nothing carries a command leaves those three to your own bindings. The driver says what it did
each time (`2 of 4: Previous`), so a press is never silent.

Changing the screen takes a moment, because a screen is drawn by running the applet's own sources: a built-in
screen appears as the button is pressed, and an applet appears once it has read what it needs  -  a network read,
not a calculation.

A name that is not a screen this build can draw  -  a misspelt applet  -  is reported and the screen is left alone,
rather than switching to something blank. `next` and `prev` **step over** names that cannot be drawn, so one bad
name in the rotation cannot jam the button; the driver says which names it steps over when it starts.

## Profiles

Four profiles, 0 to 3. Everything about the pad changes with the profile: what the controls do and what colour the
screen is lit.

- **M1, M2 and M3 select profiles 1, 2 and 3** by default, unless the profile's own file binds them. A line in
  the file always wins.
- A key is only defaulted when there *is* a profile file to switch to. Switching to a profile that does not
  exist would replace the whole map with nothing.
- **`g13 profile`** prints the active one; **`g13 profile 2`** switches.

## The stick

The stick is two axes with a calibration of their own, and it has four modes. Everything is on the window's
**Controls** tab, and written to `~/.config/g13/stick.json`.

- **off**  -  the stick does nothing.
- **keyboard**  -  the *radial*: the direction the stick is held is turned into a sector, and each sector is bound
  to a key, a mouse button or a gamepad button. **Sector 0 is up and they count clockwise**, and the number of
  sectors is yours to set  -  four makes a d-pad, eight is the familiar one, sixteen is a wheel, and anything up to
  sixty-four is valid. The names `UP`, `DOWN`, `LEFT`, `RIGHT` and the diagonals stay aliases for the sectors
  nearest the direction they name, so a file written for an eight-sector stick keeps working when you change the
  count. A sector with no binding of its own falls back to the cardinals it leans on.
- **mouse**  -  the pointer. Movement starts from nothing at the deadzone edge rather than losing a fifth of the
  travel to it, and the speed is yours to set.
- **joystick**  -  the stick is reported as a gamepad's analogue stick, with the calibrated extremes. Each side of
  each axis can be sent somewhere different, so the stick can be split: left and right steering while forward and
  back drive the two triggers.

**Calibration** is under *calibrate the stick*: pick a point, hold the stick there, click **take reading**. The
nine points (centre, four extremes, four corners) are what make "half way" mean half way.

**The deadzone**  -  *how far it must move*  -  applies to all four modes.

## The screen's colour

Each profile has its own colour, written in its own file as a line:

```
color=0,153,255
```

The window's **Bindings** tab has a swatch and a **set a colour** button for the profile you are editing. From a
terminal:

```bash
g13 colour           # what it is now
g13 colour 255,0,0   # set it
g13 color 255,0,0    # the same command: the file says `color=`, so both spellings are accepted
```

It is sent when the driver starts, when the profile changes, and when the file changes. A `color=` line survives
everything else that touches those files  -  an edit in the window keeps it as it was.

## The four macro-key lights

M1, M2, M3 and MR light up:

- **M1, M2, M3** show which profile is active. The device keeps nothing across a power cycle, so all four are
  dark until the driver starts and sets them.
- **MR** is lit for as long as a recording is running, so you can see at a glance that it is still listening.
