# Troubleshooting

Start here:

```bash
g13 doctor
```

One line per thing that matters  -  the programme, the input devices, whether a driver holds the pad, which service
is running, the active profile, and how many binding lines actually apply  -  each marked `ok`, `note`, `warn` or
`fail`, and a `fail` says what to do about it.

## The pad does nothing at all, and nothing even sees it

**A driver must be holding the pad for it to exist.** The G13 is not a plain keyboard: nothing will list it, not
even the kernel's own input devices, unless something has claimed it. So this symptom almost always means "no
driver is running" rather than "the pad is broken".

Check whether one is:

```bash
systemctl --user status g13-rs.service g13.service
g13 values          # asks the running driver what it is doing
```

Start this driver (`g13 run` in a terminal, or `systemctl --user start g13-rs.service` if it is installed), or
the older one if you are using that. **The two cannot both hold the pad**  -  starting either stops the other, by
design (`Conflicts=` in the unit).

## A control does nothing

1. `g13 bindings`  -  the map as the *driver* is applying it, which is not always the same as what the file says.
2. Is the control in the file **at all**? A control with no line in the active profile's file does nothing,
   except M1, M2 and M3.
3. Is the name one this build knows? A line naming something else is dropped and reported:
   `G23: not a control this project knows`. The real names are in [bindings.md](bindings.md); `g13 watch` shows
   you which bit each control actually sets.
4. Is the right **profile** active? `g13 profile`.

Set it by pressing rather than by hand  -  window **Bindings** tab → the row → **set by pressing**; or
`g13 record <control>` and press the key.

## The window shows nothing live

The window only edits files. Anything live comes from a running driver, so an empty **Values** tab means no driver
is running  -  see above.

## `g13 record` sits there doing nothing

Permission. Input devices carry no ACL on a default install, so every one answers *Permission denied* and the
tool listens to nothing while looking like it is waiting. The rule in this repository
(`packaging/70-g13-record.rules`) fixes it by granting `uaccess` to keyboards, mice and gamepads:

```bash
sudo cp packaging/70-g13-record.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
```

`g13 doctor` checks for this and says so if it is missing.

## The pad's screen is blank

- Is anything enabled? Window **Screen** tab: the **visuals** list, with a tick per screen and one of them
  showing. `visuals.json` holds the same thing, and `g13 lcd` prints what is on the pad now.
- Is it an applet that draws nothing? `g13 applet preview <name>` draws it as text at your desk, and
  `g13 applet check <name>` says which source gave nothing and what would be drawn over what.

## The pad's screen stops changing, or LR seems to do nothing

One screen in the rotation that this build cannot draw is enough to look like a broken button  -  asking for "next"
lands on it. It is now stepped over, and the driver says which names it steps over when it starts:

```
  custom is one of the screens you have enabled and is not a screen this build can draw, so the rotation skips it
```

The window says the same thing on the **Screen** tab, beside the tick. The fix is to untick it, or to spell the
applet's name as it really is (`applet:docker`, not `docker`).

To see the whole rotation and what each step would do, without a pad or a driver:

```bash
cargo run -p g13-agent --example screen-walk -- ~/.config/g13
```

It prints what this build can draw, the rotation with the undrawable names marked, and the next and previous
screen from every entry in it. It reads only.

## An applet shows a source's name instead of a value

That is what a widget draws when its source resolved to nothing. `g13 applet check <name>` names the source and
the reason: a missing file, a command that timed out, an endpoint that refused, a name that is not in the
catalogue at all. `Missing` is not zero  -  a bar with no reading draws empty rather than as a full bar.

## A macro does the wrong thing

- `g13 macro show <id>`  -  what it actually contains, box by box.
- `g13 macro play <id>`  -  plays it and prints what went wrong, including a question naming something nothing
  provides, and a source that would not read.
- **A question about a value is read when it is asked**, so a `Run` or a `Wait` before the question changes what
  it sees.
- **A recording holds keyboard keys, not pad presses.** If you press a control while recording, the *pad* does
  not re-emit its binding  -  the recorder is listening to the keyboards. To record a macro that presses `G7`, bind
  something to `p,k.<code>` and record the key.

## A macro plays more slowly than it was recorded

`g13 macro show <id>` prints the steps with their delays. A file written by an older tool can start with a long
delay before the first key, which is heard as "it takes half a second to start". Delete that first `d.` step in
the window's Macros tab (the `remove` button on the first row), or `g13 macro play` and watch which steps are
slow.

## The driver is behaving like a version from before

It is. The driver is a running process: **rebuild and restart it**  -  `systemctl --user restart g13-rs.service`,
or stop `g13 run` with Ctrl-C and start it again. The window is a separate process and needs its own restart.

## Keys arrive on the wrong keyboard

The driver sends through its own virtual keyboard. If a game or a window manager treats it differently, check
`g13 doctor` and the driver's output: it prints every control as it is pressed and what it sent, which is what
tells "the driver did not send it" apart from "something else did not receive it".

## Two drivers, or a service that keeps restarting

```bash
systemctl --user list-units 'g13*'
systemctl --user stop g13.service g13-visuals.service   # the older stack
```

`g13-visuals.service` is the older stack's screen programme. If it is running, it is drawing the pad's screen and
this driver's screen drawing will be fighting it. Stop it while using this driver.
