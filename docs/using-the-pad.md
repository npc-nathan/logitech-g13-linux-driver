# Using the pad

What the controls are, what they do before you change anything, and how to change any of it. This assumes `g13`
is installed  -  [installing](installing.md) covers that  -  and that the driver is running.

## The controls, and what this build calls them

Every control has a name, and that name is what your bindings file and the window use.

| on the pad | the name | what it is for |
|---|---|---|
| the 22 keys above the stick | `G1` … `G22` | whatever the profile binds them to |
| the three keys left of the row | `M1`, `M2`, `M3` | switch profile  -  unless the profile's file says otherwise |
| the fourth one | `MR` | record a macro  -  unless the file says otherwise |
| the four under the screen | `L1` … `L4` | the menu, and the pages of a screens-in-one applet |
| the round button beside the screen | `LR` | the next screen on the pad's walk; **held**, the menu |
| the stick | bound by sector | `J0` is up, and the sectors count clockwise |
| the two thumb buttons | `LMB`, `RMB` | mouse buttons  -  unless the file binds them |
| pressing the stick in | `JCLICK` | a middle-click |

The bit each one sets is in [bindings.md](bindings.md); nothing else in this build needs it.

## Pressing things, before you change anything

A control with no line of its own in the profile's bindings file does what the pad has always done by itself, and
**nothing is guessed** beyond these four:

- **`M1` / `M2` / `M3`** switch to profiles 1, 2 and 3. The light behind the key shows which profile is in force.
- **`MR`** records a macro  -  the sequence is below.
- **`LR`** shows the next screen in the rotation, and **holding** it opens the menu.
- **`L1` … `L4`** do nothing on their own: they are the four buttons *inside* a menu or an applet that has more than
  one screen, so what they do depends on what is showing.

Everything else sends nothing until you bind it.

## Changing what a control sends

In the window  -  **G13 Configuration** in your applications menu, or `g13 gui` → **Bindings**:

1. find the control's row (they are in pad order, with `G1` first);
2. click what it is bound to, or **`+ hold`** beside it for what it does when held;
3. press **`press to set`**, then press the key, mouse button or gamepad button you want it to send. That is the
   whole of it  -  the row updates and the file is written as you go;
4. or pick a **macro** from the dropdown instead, by name, or **`nothing`** to take the binding away.

There is no box to type an action into: the file's syntax is the file's business, and setting a binding by doing it
is the only way that does not involve looking up a number.

From a terminal, the same thing: `g13 bind G1 q`, and `g13 bindings` prints the whole map in words.

## Profiles: one pad, several games

A profile is a numbered bindings file  -  `bindings-0.properties` to `bindings-3.properties` and onwards  -  and the
**M keys** are the usual way to move between them. A profile can also be named and pinned to an M key in the window
(**Bindings** → **`binding set:`** → the set's **`name:`** box and **`M key:`** buttons), which is what makes
"Cyberpunk" a thing you can say rather than a number you have to remember.

Each profile carries its own **screen colour** (`color=R,G,B` in its own file), so the pad's backlight tells you
which profile is in force as well as the M-key lights do. `g13 colour 0,153,255` sets the current one.

An applet can ask for a profile by name while it is being fed  -  see [the `follow` key](applets-and-sources.md)  - 
which is how the pad can move to a game's profile by itself while that game is running, and back when it stops.

## The screen: what is on it, and how to move

The pad's screen shows one *visual* at a time. The list of them, and their order, is `~/.config/g13/visuals.json`;
the window's **Menu** tab is where you tick which ones the pad walks and how often it cycles on its own.

- **`LR`**  -  the next screen in the walk. **Hold it** and the menu opens, and `L1`-`L4` then choose from it.
- **`L1`-`L4`**  -  inside an applet with more than one screen, they change page.
- **`g13 lcd`**  -  what is on the pad right now, as text in your terminal.
- **`g13 lcd --visual applet:weather`**  -  put a named screen on it.
- **`g13 applet list`**  -  the applets installed, what each one reads, and how often.

## Recording a macro on the pad

A macro is a recorded sequence of keys that a control can play back. `MR` starts it, and the screen tells you what
it is waiting for at every step:

1. press **`MR`**  -  the screen says `RECORDING`;
2. **press a profile key** to say which profile the macro belongs to, or skip this by pressing the control straight
   away (the profile in force is used);
3. **press the control** the macro should be played by  -  `G7`, a thumb button, anything;
4. **type the keys** you want recorded. Presses and holds are both kept, with their timing;
5. press **`MR`** again to finish. The macro is written *and* bound to the control you chose.

While a recording runs, the pad's other bindings do not fire, so the keys you type are recorded rather than sent to
whatever is on screen. A recording with nothing in it writes nothing, and the **record light stays lit** the whole
time so you can see it is still listening. An unstarted hand  -  the gap before your first key  -  is not part of the
pattern.

What was written is `~/.config/g13/macro-<id>.properties`, and it can be edited, given conditions, or played from a
terminal afterwards: [macros.md](macros.md).

## Playing a macro without the pad

- `g13 macro list`  -  every macro, and whether it decides anything;
- `g13 macro show 4`  -  what it does, step by step;
- `g13 macro play 4`  -  play it now;
- `g13 macro play 4 --as G7`  -  play it as if `G7` had fired it, so a macro that asks which control fired it gives
  the same answer the pad would.

## The stick

The stick is read as a direction with a **number of sectors** you choose  -  4 is a d-pad, 8 is what the original
software did, 16 and up are a wheel. Each sector is bound like any other control, under its own name (`J0` up, then
clockwise), and a sector with no line of its own falls back to the two directions beside it.

Its **mode** is in the window's **Controls** tab: `off`, `keyboard` (the stick sends the keys its sectors are bound
to), `mouse` (it moves the pointer, at a speed beside the slider), or `joystick` (it is a controller's analogue
stick, which is what a game that reads a gamepad wants).

There is a **deadzone**  -  how far the stick must move before a direction counts  -  and a **calibration** you can
record: rest it, then hold it at each point the tab asks for. Until you do, the directions are read from the
defaults, and the tab says so.

## The four lights, and the backlight

- **`M1`, `M2`, `M3`**  -  the key of the profile in force is lit. Profile 0 has no key of its own, so it lights none.
- **`MR`**  -  lit for as long as a recording is running.
- **the backlight**  -  the profile's own `color=R,G,B`.

## Where your settings live

Everything is under `~/.config/g13/`, one file per thing, all plain text:

| file | what it is |
|---|---|
| `bindings-<n>.properties` | a profile: what each control sends, its holds, its colour |
| `profiles.json`, `active-profile` | the sets' names and M keys, and which one is in force |
| `macro-<id>.properties`, `macro-<id>.nodes.json` | a macro's steps, and the graph if it decides anything |
| `applets/<name>.json`, `applets/<name>.bitmaps.json` | a screen you designed, and its pictures |
| `fonts/`, `themes/` | fonts and effects an applet can wear |
| `endpoints.json` | hosts that `http:` sources read, with their tokens |
| `values.json` | names you have given to readings of your own |
| `visuals.json` | the screens, their order, and which is showing |
| `stick.json` | the stick's mode, sectors, deadzone and calibration |
| `menu.json` | your own menu for a hold of `LR` |

Nothing here is a database: editing a file by hand works, and the driver picks the change up within a second.

## When something does nothing

- `g13 doctor`  -  the device, the permissions, the config, the services, in one screen. Anything that is not `ok`
  is described by symptom in [troubleshooting.md](troubleshooting.md).
- The driver says what it is doing: run it in a terminal with `systemctl --user stop g13-rs.service && g13 run` and
  watch the messages. A complaint is printed once, not once a second.
- If it is the *bindings* that surprise you, `g13 bindings` prints each control and what it will actually do, in
  words  -  including the defaults a control that the profile does not mention will fall back to.
