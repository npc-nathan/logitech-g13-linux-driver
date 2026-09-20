# The window

> **One design runs through every tab**: a list on the left, the selected thing on the right, one table shape
> wherever a table is what the content is, and one field grid for editing. The text below describes each tab as
> it stands.


```bash
g13 gui
```

The window edits your configuration files. It never touches the pad itself  -  a driver does that  -  so anything
you change here is written immediately and a running driver picks it up within a second. There are no Apply or
Save buttons anywhere, on purpose.

At the top there is a tab per subject. The line above the tabs tells you whether a driver is running, and if it
is not, there is a **start driver** button.

## Menu

What holding **LR** opens, from `~/.config/g13/menu.json`. With no file of your own this is the rotation of the
ticked screens; write one and it is yours, with submenus and an action per item. Items carry a **label** and then
either a **`show`**  -  a screen to open  -  or a **`command`**.

This tab used to be called Screen and held the list of every screen with a tick each, a `showing:` line with a
**save** button, the live preview and the cycle. Those are all on the **Applets** tab now, on each applet's own line,
where they are written as you change them instead of behind a save button:

- **`on the pad`**  -  whether the pad walks to this screen with **LR** and the menu offers it.
- **`show it now`**  -  puts it on the pad straight away.
- **`cycle the enabled ones`**, with its seconds box  -  the pad moves between the ticked screens by itself.
- **`delete`**  -  removes the applet and its pictures.

The screens that used to be built in  -  `clock`, `system`, `pad`, `media`  -  are ordinary applet files, so every one
of those controls works on them too.

![the Menu tab](images/window-menu.png)

## Endpoints

The hosts your applets and macros may talk to, from `~/.config/g13/endpoints.json`. An applet never carries a
host or a token itself; it names an endpoint, and this is where that name is defined.

The left column is **what exists**, one row an endpoint: its name, whether it has a **token** (and how long that
token is, never what it says), **who uses it**, and its url underneath. A url that cannot work is called out on
its own row instead of failing silently later inside an applet.

**A token on a link that cannot keep it is called out on the same row**: sent over `http://`, or to a host whose
certificate check is switched off. It is a warning and never a refusal  -  an `http://` host on your own network is a
real setup  -  because the one thing worth catching is a credential sent somewhere it can be read.

**Click a name to open it.** The right column is then that one endpoint: its **url**, its **token**, its
**timeout**, whether to **skip the certificate check**, and **remove**. Every change is written to the file as you
make it  -  there is nothing to save.

**To add one:** type a name and a url in the **add:** row under the list and press **add**. It appears in the list
and opens in the right column, ready for its token and timeout.

**The token** is masked as a row of asterisks with its length, with a **change** and a **clear** button, because a
credential belongs in the file and not on a screen. **change** opens it for typing; **done** puts it away again.

**If an applet names an endpoint that does not exist**, that is said at the bottom of the tab, with the applets
that name it  -  which is what makes it fixable rather than merely reported.

*This tab is shared by everything. An applet's own sources  -  `cmd:`, `file:`, built-in names  -  are on the
Applets tab.*

![the Endpoints tab](images/window-endpoints.png)

## Controls (now the right column of Bindings)

The stick's settings live on the **Bindings** tab now, in a vertical column down the right-hand side:
its live reading and raw numbers, the mode it drives with the pointer-speed and deadzone settings,
and the sectors table  -  the blocks stacked one under the other rather than side by side.

## Applets

Open an applet and its own line sits at the top, with two things on it:

* **`on the pad`**  -  ticked, the pad walks to this screen with **LR** and the menu offers it; unticked, it is still
  here to edit but the pad passes over it. This is the same switch that used to be a list on the Screen tab.
* **`show it now`**  -  puts this screen on the pad straight away, which is what the Screen tab's `showing:` did. The
  pad does the same thing when you press **LR**, and a binding can do it with `sv,<name>`.
* **`cycle the enabled ones`**  -  with a seconds box beside it, the pad moves on by itself through the screens ticked
  above. Both are written the moment you change them; the Screen tab's `save` button did not come with them.
* **`delete`**  -  removes the applet's file **and its pictures** (`<name>.bitmaps.json`), because those belong to
  that applet and nothing else reads them. There is no confirmation, the same as removing an endpoint: everything in
  this window writes as you change it.
* **`export`**  -  beside **`delete`**: saves this applet as **one file**, `<name>.g13applet.json`, in the folder you
  pick in the save dialog, so it can go to another machine or into a backup. The file carries the applet, its
  pictures, the fonts it names, the theme it wears, the endpoints its resources use, any value of your own it reads,
  and whether the pad walks to it. Two things are deliberately left out: **a token**  -  an endpoint travels with its
  token slot *empty*, and the machine it lands on fills that in on the Endpoints tab  -  and **a machine's own paths**,
  because a resource that reads a file names a path on the machine the applet came from, and only you can say where
  that file is on yours.
* **`import`**  -  beside **`make it`**: opens a file saved by **`export`**, through the desktop's own open dialog. The
  whole thing lands at once  -  the applet, its pictures, the fonts it names, its theme, the endpoints its resources use,
  any value of your own it reads, and whether the pad walks to it. **Anything you already have wins**: a font, a theme,
  an endpoint's token and a value of your own are never written over by an import, because what is on your machine is
  yours and the file cannot know better.
  If its name is one you already have, **nothing is written**. The window lists what is different  -  title, interval,
  widgets, screens, sources added (`+name`) and changed (`~name`), pictures added and changed, fonts, theme, endpoints
  and values it would bring that you do not have, and whether the pad walks to it  -  and you choose:
  **`replace mine with it`**, **`import as a copy`** (under the next free name, `weather-2`), or **`keep mine`**.

**`font:`** is the font this applet wears, listed from what is in `~/.config/g13/fonts/`, with **the panel's own** as
the default. A font is a file you drop in; the named one is drawn *in front of* the panel's own, so a character it
lacks still appears. Nothing has to be typed  -  pick one and the screen redraws in it.

**`follow:`** is for an applet that is fed from outside  -  a game writing a file. Ticked **`a program feeds this`**,
the applet takes the pad's screen while the file behind one of its own sources is being written, and hands it back
when the writing stops; the seconds box beside it is **how long it keeps the screen after the last write**. **`and
the set:`** names a binding set to switch to while it is live  -  pick one, or `none`  -  and the profile goes back to
what it was when the program stops. Untick it and the `follow` line is taken out of the file rather than written as
`false`, because an applet that does not follow has no business carrying a line that says nothing. The meaning of
the key, and the three shapes it can take in the file, are in [applets and sources](applets-and-sources.md).

**`theme:`** in the title area is a dropdown of the themes in `themes/`, with `none` for the look the applet had
before themes existed. Next to it, a name box and `make it` writes a new theme  -  every effect at a setting you can
see, so it is something to look at rather than an empty file  -  and the applet starts wearing it straight away, the
way everything else in this window applies as you change it. A name already taken is refused rather than written
over, and a name that is not a file name is refused with the character in it.

In the widget inspector, **`animate`** is offered on every widget this build draws  -  `none`, `blink` or
`typewriter`  -  and `none` takes the key out of the file. A widget this build does not understand is offered
nothing, because writing a key into a widget whose meaning is unknown is this window editing a file it cannot read.

The preview is redrawn at the driver's rate  -  twenty times a second  -  while the applet you are editing has
something on it that moves, such as a scrolling text; otherwise it follows the tab's own cheaper rate. A preview
that only repainted once a second showed one frame in twenty, which looks like jitter rather than travel.

A frame on the pad costs 26 ms to write and the device manages about 38 a second, measured with
`cargo run -p g13-agent --example frame-rate --release` while the pad is free; the driver draws what moves at
twenty a second, which leaves the rest of its time to read the pad.

One screen of your own. Pick an applet from the list to edit it, or make a new one: type a name in
**new applet** and press **make it**. The file is written straight away and opened for editing.

An applet's **name is its file's name**, so the name you type is the name of `applets/<name>.json`. Letters,
digits, spaces, `-` and `_` only, and a name that is already taken is refused rather than written over  - 
nothing here ever writes over one of your files. A new applet is a blank screen with no widgets, redrawing every
2 seconds; tick it on the **Screen** tab to put it in the pad's rotation.

- **title**, **every N seconds** (how often it re-reads its sources), and whether it has a **border**.
- **the sources it reads**  -  a row per source: its name and what it reads. A `cmd:`, `file:` or built-in name
  goes here; endpoints (hosts) are on the Endpoints tab. A source no widget reads is flagged, because a `cmd:`
  or a fetch costs something every redraw and draws nothing. The row under them, **`add:`**, is where a source
  that isn't one of the machine's own is declared: a name, a spec, then **add**.
- **the screens**  -  the applet's screens, one button each. Click one to edit it: everything below belongs to the
  screen you picked. **add a screen** gives the applet a second one (your existing widgets move into screen 1, so
  nothing is lost), and **remove this screen** takes one away. A screen's name is the box beside the row, and it
  is what the pad's menu shows for that applet. While a multi-screen applet is showing, **L1 on the pad moves to
  its next screen**  -  that needs no binding. An applet with one screen says so and behaves exactly as before.
- **the widgets**  -  a table: `#`, `type`, `what it reads` and the buttons, one row each, in the order they are
  drawn (a later widget is drawn over an earlier one). Click a row's number or its type to open **that widget's
  fields in the pane on the right**, beside the preview. One at a time: five widgets' fields at once was the wall
  this replaced.
  - **`source`** is a **picker**, and **add a resource… is its first entry**  -  it takes a name and a `cmd:`,
    `file:`, `json:` or `http:` spec, declares the source and points this widget at it. Below that are the names
    this applet declares and then the names the machine itself reports; picking one of the machine's own declares
    it for you.
  - **`format`** is text with names in it: the box stays, and **insert a name** beside it puts `{name}` in. That
    list starts with **add a resource…** as well, because a text widget has no `source` field to add one from  - 
    it declares the source and appends `{name}` to the format.
  - **a `bitmap` widget's picture is drawn here**, under its fields: a grid of cells, one per pixel, lit when you
    click them. `w` and `h` set the size (1 to 32), **clear** empties it, and it is written as you click  -  into
    this applet's own `applets/<name>.bitmaps.json`. The shared `bitmaps.json` is never written from here: it
    belongs to every applet at once. A name you haven't drawn yet starts as a blank 8 by 8, so you are never sent
    to a file to spell dots and hashes.
  - Every widget's fields end with **`command`**: what the pad's L4 runs when that widget is the chosen one,
    e.g. `cmd:playerctl play-pause`. Leave it empty and the widget is not something the pad can be on; empty it
    and the key is taken out of the file rather than written as an empty command.
  - Each number says what it is measured in (`px`, `rows`, `the bar's full width`), and a widget's own readings
    are listed with where each name comes from: *declared above*, or *nothing provides this*. Add one with
  **add**, remove one with **remove**, and edit every field in place. A widget whose type this build does not
  draw is shown with every key of it left exactly as it is.
- Beside it, the pad's screen as it will look, redrawn as you type  -  **and it follows the screen you pick**, so
  screen 2 can be checked by eye as easily as screen 1.

The editor writes the file's own JSON rather than a parsed applet, so keys this build does not act on are kept
rather than dropped, and a file that would not read back is not written at all.

**Making four buttons the pad can drive**  -  the whole procedure, start to finish:

1. Pick the applet in the list (or **new** one), then **add** a `text` widget for each button.
2. Give each one its words: the **`format`** field, e.g. `Start/Pause`, `Previous`, `Next`, `Stop`.
3. Put them where you want them with **`x`** and **`y`**  -  the panel beside it shows the result as you type.
4. In each button's **`command`** field, the command it runs, e.g. `cmd:playerctl play-pause`.
5. Add anything that is *not* a button  -  a reading, a rule  -  leaving its `command` empty. Only widgets with a
   command can be landed on.
6. On the pad: **L3** moves the border down the buttons, **L2** back up, **L4** runs the one with the border.

Full detail, including every widget type and every kind of source: [applets-and-sources.md](applets-and-sources.md).

**The menu your own file makes.** Under the list of screens on this tab is the menu the pad's button opens.
With no `menu.json` it is the ticked screens above; add an item here and the file is written and the menu becomes
yours.

1. Type a name in **new item** and press **add it**. It shows the clock to start, so it works before you edit it.
2. **what it says** is the label on the pad  -  `Watch`, `Music`, `Docker`.
3. **opens a list** / **shows a screen** / **runs a command** is what choosing it does:
   * *shows a screen*  -  the value is `clock`, `media`, `pad` or `applet:docker`, and **screen** is which screen of
     an applet (1 is its first).
   * *runs a command*  -  the value is a command, e.g. `cmd:playerctl play-pause`.
   * *opens a list*  -  it has items under it; press **+ item under it** to add one, and those are what the pad
     moves through when it is opened.
4. **^** and **v** move an item among its neighbours, **remove** takes it away, and **remove the whole menu**
   deletes the file so the menu goes back to the rotation.
5. On the pad: hold **LR** to open it, **L2**/**L3** move, **L4** chooses  -  a list opens one level deeper, and
   **L1** goes back one level, closing it at the top.

The file is written as you edit it, and it is refused rather than written if an item could not be read back  - 
for example *"broken: an item needs something to do - a `show` screen or a `command` to run"*, named by
its label. A
`menu.json` that will not parse is shown as a problem and **nothing is written over it**, because whatever is
wrong with it is the thing worth seeing.

**`runs:`** above a widget's fields offers the screens of this applet as buttons  -  `go to playlist` writes
`"command": "screen:playlist"` for you, so a screen is picked rather than remembered. A single-screen applet is
offered nothing, because there is nowhere to go.

![the Applets tab](images/window-applets.png)

### Giving a widget an alert

An alert makes one widget react while something is true  -  the CPU over 80, a track playing, a profile being active.

1. On the **Applets** tab, click the widget's number in the list, so its fields are in the inspector.
2. Set **`alert:`** to `flash`, `invert`, `show` or `border`. `none` takes the alert off again.
3. Press **`write the question`**. A question to start from appears  -  the first thing this applet reads, compared
   with 80  -  and the rows under it are **the same rows the Macros tab uses for a macro's `if`**:
   - `not` inverts that row; the value box offers **this applet's own sources first**, marked `(declared here)`,
     then everything the machine publishes;
   - the operator is `==`, `!=`, `<`, `<=`, `>`, `>=` or `contains` for text;
   - the box after it holds the number or the words to compare with;
   - **`add a question`** adds another row, and once there is more than one, `all of` / `any of` says how they join;
   - **`remove`** takes a row away;
   - the line ends with the question in words (`now: cpu > 80`), which is what will be in the file.
4. For **`show`**, a `drawing` box takes **one of your own pictures, by name**  -  the same names the `bitmap` widget
   uses, from `applets/<applet>.bitmaps.json` or the shared `bitmaps.json`. It offers every picture this applet can
   draw, so you pick one rather than remembering one, and it can still be typed for a picture you have not drawn
   yet. A picture with **frames** animates there: it is drawn from the same clock as everywhere else.

   **`show` draws the picture *instead of* the widget, not over it.** While the question is true the widget's place
   is cleared and the picture goes there, so a line of text can become a spinner or a warning triangle and
   everything that was underneath is gone. The widget comes back the moment the question is false, and a picture
   drawn *over* a widget would have been a mangled overlap: the panel will not lay ink on ink.
5. To stop editing, press `write the question` again.

Every change is written to the applet's file as you make it, and the driver re-reads that file as it draws  -  so the
first flash, invert or frame appears on the pad without restarting anything. A widget with **no** alert behaves
exactly as it did before alerts existed, and taking the alert back to `none` removes it from the file rather than
leaving an empty one behind.

The question is the **macros' own condition**  -  the same language, the same parser, the same evaluator  -  so a
question written here can be cut and pasted into a macro's `if` and mean the same thing. An alert may ask about the
values its applet declares and about the active profile; a question about a *control* is always false here, because
that is the one thing only a pad can answer.

## Themes

The **Themes** tab is the themes themselves: what is in `themes/`, one line each saying what it asks for
(`scanline 50, glitch every 6s by 1, typewriter 12`), and the chosen one's own settings on the right  -  `scanline`,
`glitch every`, `glitch shake`, `pulse on`, `pulse off`, `typewriter` and `frame`.

Every change is **written to the file at once**, because the driver re-reads a theme as it draws: an edit here is
on the pad without restarting anything. A number taken to **zero takes the key out of the file** rather than
writing a zero, because absent is what "off" means for these effects; and a theme that will not read is never
written over  -  the file it could not read is the evidence of what is wrong with it.

`new theme:` in this tab is the same thing the Applets tab's name box and `make it` do: it writes a theme with
every effect at a setting you can see and applies it to the applet you are editing. Both are here because that is
the shortcut most people reach for first, and this tab is where a theme is looked at afterwards.

A theme with a mistake in it is listed with the reason, and an applet naming a theme that cannot be read says so in
that applet's own problems.

![the Themes tab](images/window-themes.png)

## Values

Every value this build publishes, what it means, and who provides it  -  as two columns, like the other tabs: the
list on the left, and the chosen value's detail on the right. Click a value and the right-hand side says what it
is, which half of the system answers for it, what it reads *now*, who uses it, and the line to paste into an
applet (`{"sources": {"cpu": "cpu"}}`). An applet or a macro uses one by naming
it, e.g. `{cpu}` in a widget's format, or a question about `cpu` in a macro.

`g13 values --catalogue` prints the same list in a terminal, and `g13 values` prints what the running driver is
reporting right now.

![the Values tab](images/window-values.png)

## Macros

Every macro, one **card** each, in a list that scrolls: the card says its number, its name and how many steps it
has, and the one being edited is the lit one. **Click a card to edit that macro**  -  its steps as rows, and, when it
decides something, the graph as a picture you can drag and join. The previous stack left a slot for every id it could
use, so **show the N empty one(s)** is off until you ask for it.

**To make one:** type what it is for in the **new:** box and press **make it**  -  written straight away, so a
recording or a hand-built graph can follow.

This tab has its own document: [macros.md](macros.md).

![the Macros tab](images/window-macros.png)

## Bindings

What each control does, for the profile you pick at the top.

- The picker shows which profile is active, and lets you edit any of them.
- One row per control: its name, its bit, **what it is bound to**  -  the key's own name (`q`, never `p,k.16`), a
  macro's own name, or the action as it stands  -  and **what the driver will do with it in words**.
- Click that to edit it. A key, a mouse button and a gamepad button are set by **`press to set`**: press the button,
  then press the thing you want that control to send. A **macro** is picked from a list of your own, by name, and
  `nothing` takes the binding away. **There is no box to type an action into**  -  the file's syntax is the file's
  business, and setting a binding by doing it is the only way that does not involve looking up a number.
- Beside it, a **held:** button: what that control does when you hold it. `+ hold` sets one. It is the same
  editor  -  `LR=sv,next` is the tap and `LR.hold=menu` is the hold  -  so a control can do one thing when tapped
  and another when held.
- A control the profile does not mention is shown as such, with the default the driver will apply  -  M1, M2 and M3
  select profiles 1, 2 and 3, MR records a macro, and LR shows the next screen, unless the file says otherwise.

The action syntax in full, and the bit map: [bindings.md](bindings.md).

![the Bindings tab](images/window-bindings.png)
