# Installing g13

Where the binary comes from, what it puts on your machine, and how to take it away again. Once it is in,
[getting started](getting-started.md) takes over.

## The short answer

```bash
sudo apt install ./g13_0.1.0_amd64.deb
```

Then **G13 Configuration** in your applications menu, or `g13 gui`. The driver starts with your session, and the
first run sets your config up on its own.

## Debian, Ubuntu and anything else with dpkg

```bash
sudo apt install ./g13_0.1.0_amd64.deb
```

It puts five things on the machine, and nothing in your home:

| where | what |
|---|---|
| `/usr/bin/g13` | the one binary  -  the driver, the window, the manual's commands |
| `/usr/lib/systemd/user/g13-rs.service` | the driver as a **user** service, enabled for every user at their next login |
| `/usr/lib/udev/rules.d/70-g13-record.rules` | permission to claim the pad, to use `uinput`, and to read the devices a binding can be set from  -  seven lines, all `uaccess`, so it is the person at the seat and nobody else |
| `/usr/share/applications/g13.desktop` | **G13 Configuration** in the applications menu |
| `/usr/share/g13/defaults/` | what a first run copies into your config: **12 applets** and their pictures, **4 fonts**, a theme, the **16 named macros**, **4 binding sets**, and the rotation |

It declares two dependencies  -  `libudev1` and `libcap2`  -  which are the two libraries the binary links. The USB
library it speaks to the pad through is built into the binary rather than borrowed from the machine, so there is no
version of it to have installed. Every normal desktop already has both; on a bare machine apt fetches them.

**What the rule grants, exactly.** Four of its lines are about the pad and the driver's own devices: the pad as a USB
device, `uinput` for the keyboard and mouse it creates, the devices it creates, and the pad through `hidraw`. The other
three grant `uaccess` to **every keyboard, mouse and gamepad on the machine**, and that is on purpose: `g13 record` and
the window's "press to set" work by reading the real input devices, and a binding can be a key, a mouse button or a
gamepad button, so all three classes have to be readable. `uaccess` is what keeps it narrow  -  the person at the seat,
never a group, nothing to other users, and nothing at all while nobody is logged in. A machine with no such rule
answers "Permission denied" on every device, which is why this ships rather than being an optional extra.

**Your config is not written by the package.** The first time the driver runs it copies the defaults in, one file at
a time, and it never writes over a file that is already there  -  so installing this over an existing setup changes
nothing you have.

**Already have a version installed?** `apt` skips a package whose version is already on the machine, even if the
file is a new build of it. Say so explicitly:

```bash
sudo apt install --reinstall ./g13_0.1.0_amd64.deb
```

**Take it away:**

```bash
sudo apt remove g13
```

Everything in the table above goes, including the enable it created in `/etc/systemd/user/`  -  that is outside dpkg's
file list, and the package's `postrm` removes it by hand. One thing it cannot do: **stop a driver that is running
right now.** A user service belongs to your session and a package script has no session to talk to, so stop it
yourself first if you want it gone immediately:

```bash
systemctl --user stop g13-rs.service
```

## Anywhere else  -  the tarball

```bash
tar xzf g13-0.1.0.tar.gz
cd g13-0.1.0
./install.sh
```

`install.sh` seeds your config from the same defaults and tells you what it did; re-running it is safe and says
*"0 files added, … you already had"*. It installs nothing system-wide and puts no binary on your `PATH`  -  the binary
is the `g13` in that folder, so either run it by path or copy it somewhere of your own.

The udev rule is what gives the driver permission to claim the pad, and on a system without the package it has to be
installed by hand:

```bash
sudo cp packaging/70-g13-record.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

## From source

```bash
cargo build --release -p g13-cli
```

That leaves `target/release/g13`. It needs the same two libraries at build and run time, and a C compiler, because
the USB library is compiled in rather than borrowed. Then the udev rule above, a session that can see the device,
and nothing else. The window and the driver are the same binary; `g13 run` drives
the pad and `g13 gui` configures it, and they are separate processes on purpose: a window that writes files must
never be in the way of the loop that reads your keys.

## Running it without a terminal

The install enables the driver as a user service, so it comes up by itself at your next login and takes the pad.
`g13 service` drives that service from a terminal:

```bash
g13 service status     # what systemd says, and since when
g13 service start      # take the pad now
g13 service stop       # let go of it
g13 service restart    # after the binary or the unit changes
```

With no argument, `g13 service` prints the status. Only one driver can hold the pad, so a terminal `g13 run` and
the service are alternatives: stop one before starting the other, or the second finds the pad taken. Running it by
hand is what you want when something is wrong  -  the driver says more in a terminal than it does in the journal.

## Copying the defaults in yourself

The driver sets your config up the first time it runs, and so does `install.sh`. If you want it done on demand  - 
or you built from source and want the shipped applets, fonts, macros and binding sets without starting anything:

```bash
g13 setup
```

```
Setting up g13.
  from:  /usr/share/g13/defaults
  into:  /home/you/.config/g13

  added 22, kept 0, refused 0
```

It looks for the defaults in `/usr/share/g13/defaults` and beside the binary, and it **never writes over a file that
is already there**  -  anything you have is counted as *kept*, and anything it cannot write is *refused* by name rather
than passed over. Running it again after a package upgrade is how a new default applet reaches a machine that
already has a config.

## Is it working?

```bash
g13 doctor
```

```
  [ok  ] g13 on PATH        /usr/bin/g13
  [ok  ] virtual keyboard   /dev/uinput exists
  [note] pad claimed        no pad device node: nothing is holding the pad, which is why it is not a device right now
  [ok  ] our keyboard       g13 keyboard is registered
  [warn] g13.service        inactive
  [ok  ] g13-rs.service     active
  [ok  ] pad on USB         046d:c21c found
  [ok  ] bindings           17 controls bound, in ~/.config/g13/bindings-4.properties
  [ok  ] bindings applied   every line was used
  [ok  ] active profile     4
  [ok  ] endpoint tokens    every one is sent over a checked link

Nothing wrong. If keys still do nothing, run the driver in a terminal and watch it:
    systemctl --user stop g13.service && g13 run
```

The machine this was copied from runs the driver as `g13-rs.service` and not as `g13.service`, which is why one
of those two is a warning and the pad is a note rather than a claim. Those two levels are worth reading before
the words `ok` are: `note` is a fact about the moment, and `warn` is something worth knowing that is not
necessarily broken.

Anything that does not say `ok` is described in [troubleshooting](troubleshooting.md), by symptom.

|  |  |
|---|---|
| ![the clock](images/clock.png) | ![the pad's own state](images/pad.png) |
| **something is on the screen**  -  the clock, once the driver is running | **and the driver is drawing**  -  the profile, the stick's mode, the last control |
