# Macros

A macro is a list of keyboard keys with the timing between them. It lives in `~/.config/g13/macro-<id>.properties`
and can be bound to any control, in any profile.

A macro can also **decide things**: ask about the CPU, the profile, another macro's value or an endpoint, take one
path or another, loop, and run a command. That is a second file beside the first, `macro-<id>.nodes.json`, and it
is called the macro's **graph**.

There are three ways to make one: record it on the pad, build it in the window, or write the file. All three
produce the same two files.

- [Recording on the pad](#recording-on-the-pad)  -  MR, and the wizard it starts.
- [The Macros tab](#the-macros-tab)  -  the list, the steps, the picture, the sources.
- [The graph](#the-graph)  -  nodes, ports, questions, and running commands.
- [The terminal](#the-terminal)  -  `g13 macro list`, `show`, `play`.
- [The files](#the-files)  -  exactly what is written.

## Recording on the pad

**MR** is the button labelled *Macro Record*, and it does what its label says. Pressing it starts a wizard, and
the pad's screen tells you which stage you are in the whole way through:

1. **Press MR.** The screen says `RECORDING / macro <n> / M1 M2 M3 picks it / then the control`. MR's own light
   comes on and stays on until you are done.
2. **Press M1, M2 or M3** to say which profile the macro belongs to. The screen names it, whether or not that
   changed the profile.
3. **Press the control** you want the macro on  -  `G7`, say. The screen says
   `RECORDING / macro <n> / G7 in profile 2 / press the keys / MR ends it`.
4. **Type the macro.** Every key you press is recorded, with its timing, from the real keyboard.
5. **Press MR again to finish.** That one press writes the macro *and* the binding that plays it: from then on
   `G7` in profile 2 plays it.

Things worth knowing:

- **While the wizard runs, your bindings do not fire**  -  a press is an answer to the wizard, not a keystroke. Key
  *releases* are still applied, so nothing is left held down.
- **It gives up rather than holding your pad hostage**: 15 seconds to pick a profile, 5 seconds of no keys, 60
  seconds in total. If it gives up, nothing is written.
- **A recording of no keys writes nothing at all**, and says so.
- **Macro ids**: the lowest free id from 1 upwards. Id 0 is never used  -  that one is the previous stack's
  `CTRL-ALT-DEL`.
- To write over a macro you already have, bind a control to `rec,<id>` instead of `rec`; the wizard then records
  into that id and keeps its name.
- **What a recording holds is keyboard keys, not pad presses.** That is the shape the file stores, and it is what
  the previous stack recorded too.

## The Macros tab

`g13 gui` → **Macros**.

- **The list** is every macro: its id and its name, and `[n nodes]` when it decides something. Empty slots are
  hidden by default  -  the previous stack left one for every id it could use  -  behind a checkbox that says how
  many there are.
- **new:** makes an empty macro. **stop editing** closes it.
- **The left side is the steps**: this macro is these steps, in this order. Each row has `^`, `v` and `remove`
  *first*, then the kind and its value, so a mis-click on what you were aiming for cannot remove a step. **add:**
  appends a press, a release or a wait.
- **The right side is the graph**, when it has one: the picture, plus a **play it now** button that types for
  real and reports which boxes it went through.
- **make it decide things** turns a recorded macro's steps into a chain of boxes  -  the same behaviour, in a shape
  that can be given an `if`, a `repeat`, a `type`, a `call` or a `run`. A recording is never rewritten by it; the
  steps file stays as it was.
- **delete the graph** goes back to plain steps.

## The graph

### The picture

Read it from the top. `▶ when this macro runs` is where it starts, and each box is what happens next.

- **Drag a box** to move it. Its position is written when you let go.
- **Drag from a dot** on a box's right-hand edge onto another box to join them. **Click a dot** to take a join
  away.
- **Click a box** to edit what it does. Its fields appear beside the canvas, and its ways out are listed as
  dropdowns underneath  -  a dot is quicker, a list is certain.
- The line out of a box goes to the box it is joined to. **`(nothing)`** there means the macro finishes down that
  way.
- A box with two dots on the right chooses between them: **`then`** is the way on when an `if` is true (and where
  a `repeat` goes once it has done its times), **`else`** is the way on when an `if` is false, and **`body`** is
  the bit a `repeat` does again.

### What the boxes are

| box | what it does | its fields |
|---|---|---|
| **Press / Hold / Release** | a key. *Press* taps it and releases after the hold; *Hold* leaves it down for a later box; *Release* lets it go | the key, how long it is held, and which of the three |
| **Wait** | does nothing for a while | milliseconds |
| **Type** | types a string, one character at a time | the text, and the gap between characters |
| **If** | asks a question and goes one of two ways | the question (below) |
| **Repeat** | does its `body` this many times, then takes `then` | how many times |
| **Play macro** | runs another macro, waits for it, then carries on | which macro |
| **Run** | does something outside the keyboard | what to do (below) |
| **Finish** | ends the macro here |  -  |

A recorded macro is folded where the file spells one action as three: `kd.29,d.20,ku.29` becomes **one** box,
`Press leftctrl for 20ms`. A chord stays holds and releases, because that is what a chord is.

### Questions

An **If** can ask about:

- **fired by `G7`**  -  which control started this macro. This is what lets one macro sit on several buttons and do
  something different on each.
- **profile `2` is active**.
- **a value**: pick a name, an operator (`==`, `!=`, `<`, `<=`, `>`, `>=`, `contains`) and what to compare it
  against. The names offered are the macro's own sources first, then every published value.

Rows can be joined with **all** or **any**, and each row can be negated with a **not**.

Three rules, all of them about not lying to you:

- **A value nobody has published is not zero.** Unknown is not a comparison, so the row is false rather than
  accidentally true.
- **A source that will not read is reported**, and its row is false.
- **A question naming something nothing provides is reported by name.** It is false  -  but it is false for ever,
  and renaming a source is exactly how you leave one behind. `g13 macro play` prints this.

### What a macro reads

A macro declares its own sources, in the same syntax an applet uses. On the Macros tab this is the
**what this macro reads** block; in the file it is the `sources` object:

```json
"sources": {"cpu_temp": "cmd:sensors | awk '/Package/ {print $4}'", "office": "http:home/temp#value"}
```

- They are read **when the question is asked**, not when the macro was fired. "Is the CPU over 70" has to be
  about now, and a macro looping through a question re-reads on every pass.
- A source **shadows a published value of the same name**  -  declaring it on the macro is the more specific thing
  to say, and the list shows the macro's own names first.
- Every kind of source is available: `cmd:`, `file:`, `json:`, `env:`, `regex:`, a built-in name, or a host by
  its endpoint. See [applets-and-sources.md](applets-and-sources.md).
- The editor refuses a source with no name or nothing to read: a source nothing can name is a question that
  cannot be asked. It never rewrites a question because you renamed something  -  it tells you the name is unknown
  when the macro runs instead.

### Doing something

A **Run** box is the output side. Its `do this:` field takes a source spec:

```
cmd:notify-send 'cpu is hot'
cmd:curl -s -X POST http://127.0.0.1:8080/light -d on
```

It runs at the point the flow reaches it, through `sh -c`, so pipes, redirections and `&&` work. What it returns
is thrown away  -  this is the doing side. **A command that fails stops the macro and says which one**: an action
that did not happen must not look like one that did.

## The terminal

```bash
g13 macros list             # `macros` and `macro` are the same command
g13 macro list              # every macro, and whether it decides anything
g13 macro show 203          # what it does, step by step or box by box
g13 macro play 203          # play it now
g13 macro play 203 --as G7  # play it as if G7 had fired it
```

`--as` matters for anything that asks which control fired it: from the terminal there is no pad, so a question
about the control is false unless you say what it should be.

## The files

**`~/.config/g13/macro-<id>.properties`**  -  the steps, and the shape the previous stack reads. Not ours to
change:

```properties
id=203
name=G7 in profile 2
sequence=kd.20,d.111,kd.18,d.25,ku.20,d.85,ku.18,d.100
```

`kd` is a key going down, `ku` a key coming up, `d` a delay in milliseconds, and the numbers are keycodes.

**`~/.config/g13/macro-<id>.nodes.json`**  -  the graph, when there is one. The player uses it instead of the
steps when it exists, and saving a straight-line graph writes the matching `sequence=` as well, so the two agree:

```json
{
  "start": "ask",
  "sources": {"cpu_temp": "cmd:sensors | awk '/Package/ {print $4}'"},
  "nodes": [
    {"id": "ask", "kind": "if", "when": {"all": [{"value": "cpu_temp", "op": ">", "to": 70},
                                                 {"control": "G1"}]}, "x": 60, "y": 40},
    {"id": "hot", "kind": "run", "spec": "cmd:notify-send 'cpu hot'", "x": 60, "y": 120},
    {"id": "cool", "kind": "wait", "ms": 0, "x": 300, "y": 120}
  ],
  "edges": [
    {"from": "ask", "port": "then", "to": "hot"},
    {"from": "ask", "port": "else", "to": "cool"}
  ]
}
```

- `x` and `y` are where the box sits and nothing else; the player ignores them, so a macro that is never opened
  in the window still plays.
- An edge leaves by `out` unless the box it leaves has more than one way to go, in which case it says which  - 
  `then`, `else` or `body`. A port carries exactly one line; two is an error rather than a silent choice.
- Every error names the box it is about: `node loop (flash): "kind" is flash; this build runs key, wait, type,
  if, repeat, call, run and stop`.
- A macro that loops for ever is stopped after 20,000 boxes and says so, rather than holding your keyboard.
- A character this build cannot type stops the macro rather than typing something else into whatever is focused.

Worked example  -  "G1 checks the CPU and does one thing or the other":

```json
{
  "start": "ask",
  "sources": {"cpu_temp": "file:/sys/class/hwmon/hwmon5/temp1_input"},
  "nodes": [
    {"id": "ask",  "kind": "if",  "when": {"value": "cpu_temp", "op": ">", "to": 70000}},
    {"id": "yes",  "kind": "run", "spec": "cmd:notify-send 'cpu is hot'"},
    {"id": "no",   "kind": "run", "spec": "cmd:notify-send 'cpu is fine'"}
  ],
  "edges": [
    {"from": "ask", "port": "then", "to": "yes"},
    {"from": "ask", "port": "else", "to": "no"}
  ]
}
```

Bind it and it runs on the pad: `G1=p,k.30` is not what you want here  -  bind the control to the macro that has
this graph, which is `G1=m,<id>,1`.
