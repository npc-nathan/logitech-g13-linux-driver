//! What the window's tabs are made of: the tab itself, and the rows and fields each one edits.
//!
//! A tab is a list of rows, a row is a set of fields, and a field is one of a few kinds of value. Those shapes
//! live here rather than in `state.rs` because they are what the drawing walks and what the edits read: the
//! window keeps the *state* (which tab, what is selected, what a change writes) and asks these types what a
//! given tab can hold.
//!
//! Nothing here touches a file or a display - it is the shape of the data, which is what makes the window's
//! editing testable without either.

/// Which part of the window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// The map: what each control on the pad does.
    Bindings,
    /// The pad's menu: its items, and what each one opens or runs.
    Menu,
    /// The endpoints applets read, and which applets need them. The file is `endpoints.json`, which is also
    /// where a credential lives - so the token is shown masked and never written into a status line.
    Sources,
    /// Designing an applet: its own fields, the sources it reads, and the screen as it will look.
    Applets,
    /// Themes: the effects a screen wears.
    Themes,
    /// Every value this build publishes: what it is, what it is now, and which applets read it.
    Values,
    /// The macros: what there is, what it does, and editing it where it lives.
    Macros,
}
impl Tab {
    /// The tab's name, as the button in the tab bar shows it.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Bindings => "Bindings",
            Tab::Menu => "Menu",
            Tab::Sources => "Endpoints",
            Tab::Applets => "Applets",
            Tab::Themes => "Themes",
            Tab::Values => "Values",
            Tab::Macros => "Macros",
        }
    }
}
/// One row of the bindings table.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The control's name: `G1`, `LR`, `stick up`.
    pub control: String,
    /// Which bit of the pad's report it is, or none when no bit carries it.
    pub bit: Option<usize>,
    /// The action as the file writes it, or empty when unbound.
    pub action: String,
    /// What the driver will do with it, in words.
    pub effect: String,
    /// What it does when it is held, in words, when it has a hold at all.
    pub held: Option<String>,
}

/// One sector of the stick: what the bindings file says it is bound to, and what the driver will press.
pub struct SectorRow {
    /// Which sector it is, counted from zero at the top and clockwise.
    pub index: u32,
    /// The name the file uses for it: the one already written there, or its number.
    pub name: String,
    /// The action as the file writes it, or empty when the sector has none of its own.
    pub action: String,
    /// What will actually be pressed, in words.
    pub effect: String,
    /// True when that comes from the cardinals beside it rather than from a line of its own.
    pub inherited: bool,
}

/// What kind of value a widget's field holds. Decides the control, and how the file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// A number the file always holds.
    Number,
    /// Yes or no, where leaving it out means no: `scroll` is off unless the file says otherwise.
    Tick,
    /// A number the file may leave out, where leaving it out means something: a text widget with no `x` is
    /// anchored by its alignment instead.
    OptionalNumber,
    /// Text the file always holds.
    Text,
    /// Text the file may leave out, where leaving it out means something: a list with no `select` is a list
    /// with nothing for L4 to run, which is not the same as one that runs an empty command.
    OptionalText,
    /// One of a fixed set of words.
    Align,
    /// What a widget does when it moves. On every kind, because any widget may blink or type itself, and the
    /// theme says how fast the rhythm goes: `animate` is the widget's part of that bargain.
    Animate,
}

/// The words the window offers for `animate`: the canonical spellings, while a file may use the other names
/// `Animate::from_word` accepts (`pulse`, `flash`, `type`).
/// An empty word is the first choice: it means the key is not written at all, which is how a widget goes
/// back to doing nothing special. A file may also use the other names `Animate::from_word` accepts.
pub const ANIMATE_WORDS: [&str; 3] = ["", "blink", "typewriter"];

/// What a menu item does, as one of three things rather than two sets of keys that can contradict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuDoes {
    /// It has items under it, which choosing it opens.
    List,
    /// It shows a screen.
    Screen,
    /// It runs a command.
    Command,
}

impl MenuDoes {
    /// What this does, in the words the menu offers.
    pub fn words(self) -> &'static str {
        match self {
            MenuDoes::List => "opens a list",
            MenuDoes::Screen => "shows a screen",
            MenuDoes::Command => "runs a command",
        }
    }
}

/// One row of the menu editor: an item, or one of an item's own items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRow {
    /// Where it is in the file: which top-level item, and which of its items when it is one.
    pub at: (usize, Option<usize>),
    /// What the item says on the pad.
    pub label: String,
    /// Whether it opens a list, shows a screen or runs a command.
    pub does: MenuDoes,
    /// The screen it shows, or the command it runs.
    pub value: String,
    /// Which screen of a shown applet, counted from one.
    pub screen: Option<usize>,
}

/// One item of the menu's JSON as a row the tab can draw.
pub(crate) fn menu_row(item: &serde_json::Value, at: (usize, Option<usize>)) -> MenuRow {
    let opens = item
        .get("items")
        .and_then(|items| items.as_array())
        .map(|items| !items.is_empty())
        .unwrap_or(false);
    let show = item.get("show").and_then(|show| show.as_str());
    let command = item.get("command").and_then(|command| command.as_str());
    MenuRow {
        at,
        label: item
            .get("label")
            .and_then(|label| label.as_str())
            .unwrap_or_default()
            .to_string(),
        does: match (opens, show.is_some(), command.is_some()) {
            (true, _, _) => MenuDoes::List,
            (_, true, _) => MenuDoes::Screen,
            _ => MenuDoes::Command,
        },
        value: show.or(command).unwrap_or_default().to_string(),
        screen: item
            .get("screen")
            .and_then(|screen| screen.as_u64())
            .map(|screen| screen as usize),
    }
}

/// The fields each kind of widget has, in the order the file writes them.
///
/// The keys are the file's own, including `r` for an arrow's radius - a thing worth reading once out of the
/// reader rather than guessing at.
pub fn widget_fields_for(kind: &str) -> Vec<(&'static str, FieldKind)> {
    // Every widget, of every kind, can be told what to do when it moves: one line here rather than a copy in
    // each kind's own list, which is how a field comes to exist on some widgets and not others.
    let mut fields = widget_fields_of_kind(kind);
    // Only a kind this build actually draws. An unknown widget is left exactly as it is - writing `animate`
    // into one would be this window editing a file it does not understand.
    if g13_applets::WIDGET_KINDS.contains(&kind) {
        fields.push(("animate", FieldKind::Animate));
    }
    fields
}

/// The fields a kind has of its own, before the `animate` line every kind gets.
pub(crate) fn widget_fields_of_kind(kind: &str) -> Vec<(&'static str, FieldKind)> {
    const TEXT: [(&str, FieldKind); 7] = [
        ("x", FieldKind::OptionalNumber),
        ("y", FieldKind::Number),
        ("align", FieldKind::Align),
        ("format", FieldKind::Text),
        ("scroll_width", FieldKind::OptionalNumber),
        // the previous stack's two keys, acted on now: run the text rather than cut it, and how fast
        ("scroll", FieldKind::Tick),
        ("scroll_speed", FieldKind::Number),
    ];
    const BAR: [(&str, FieldKind); 6] = [
        ("x", FieldKind::Number),
        ("y", FieldKind::Number),
        ("w", FieldKind::Number),
        ("h", FieldKind::Number),
        ("source", FieldKind::Text),
        ("max", FieldKind::Number),
    ];
    const BITMAP: [(&str, FieldKind); 3] = [
        // where it goes; the picture itself is a name, looked up in the shared file or the applet's own
        ("x", FieldKind::Number),
        ("y", FieldKind::Number),
        ("name", FieldKind::Text),
    ];
    const BUTTON: [(&str, FieldKind); 5] = [
        // a button is a box and its label; what it runs is the `command` every widget has
        ("x", FieldKind::Number),
        ("y", FieldKind::Number),
        ("w", FieldKind::Number),
        ("h", FieldKind::Number),
        ("format", FieldKind::Text),
    ];
    const LIST: [(&str, FieldKind); 5] = [
        // the source whose lines are the items, and how many of them fit on the screen
        ("x", FieldKind::Number),
        ("y", FieldKind::Number),
        ("w", FieldKind::Number),
        ("rows", FieldKind::Number),
        ("source", FieldKind::Text),
    ];
    const LINE: [(&str, FieldKind); 3] = [
        ("x", FieldKind::Number),
        ("y", FieldKind::Number),
        ("w", FieldKind::Number),
    ];
    const SEGMENTS: [(&str, FieldKind); 7] = [
        ("x", FieldKind::Number),
        ("y", FieldKind::Number),
        ("w", FieldKind::Number),
        ("h", FieldKind::Number),
        ("count", FieldKind::Number),
        ("source", FieldKind::Text),
        ("max", FieldKind::Number),
    ];
    const BRACKETS: [(&str, FieldKind); 5] = [
        ("x", FieldKind::Number),
        ("y", FieldKind::Number),
        ("h", FieldKind::Number),
        ("len", FieldKind::Number),
        ("thick", FieldKind::Number),
    ];
    const ARROW: [(&str, FieldKind); 4] = [
        ("x", FieldKind::Number),
        ("y", FieldKind::Number),
        ("r", FieldKind::Number),
        ("source", FieldKind::Text),
    ];
    let own: &[(&str, FieldKind)] = match kind {
        "text" => &TEXT,
        "bar" => &BAR,
        "line" => &LINE,
        "list" => &LIST,
        "button" => &BUTTON,
        "bitmap" => &BITMAP,
        "segments" => &SEGMENTS,
        "brackets" => &BRACKETS,
        "arrow" => &ARROW,
        _ => &[],
    };
    if own.is_empty() {
        // a type this build does not draw has no fields at all, so every key of it is left exactly as it is -
        // and a command field that appeared on it would make an unknown widget look editable
        return Vec::new();
    }
    // What every widget has on top of its own fields: the command the pad's L4 runs when this widget is the
    // chosen thing. Appended in one place, so a new kind cannot be the one that cannot be acted on.
    let mut all: Vec<(&'static str, FieldKind)> = own.to_vec();
    all.push(("command", FieldKind::OptionalText));
    all
}

/// The kinds of widget this build draws, which is what the designer can add.
/// The kinds the add button offers. Owned by the format, so this cannot fall behind the reader: a widget kind
/// the reader accepts and the window does not offer can only be made by hand.
pub use g13_applets::WIDGET_KINDS;

/// One field of one widget, with the value it holds right now.
pub struct WidgetField {
    /// The field's name in the file: `x`, `format`, `align`.
    pub key: &'static str,
    /// What it holds right now.
    pub value: FieldValue,
}

/// One published value, ready to be shown: what it is, what it is now, and who reads it.
pub struct ValueRow {
    /// The value's name, as an applet names it in `sources`.
    pub name: String,
    /// What it is, in a line - or, for one of the user's own, the spec it reads.
    pub meaning: String,
    /// True when it is read out of the machine here and now, false when only the running driver knows it.
    pub computed: bool,
    /// Its value at this moment, or empty when the driver has not published it (a stick nobody has touched).
    pub now: String,
    /// The applets and macros that read it, in the order they were found.
    pub used_by: Vec<String>,
    /// True when the user named it, in `values.json`. A value of the user's own is not "published by this build"
    /// and the table has to say which is which, or a value that can be deleted looks like one that cannot.
    pub mine: bool,
}

/// One value a widget reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueRead {
    /// The name the widget reads, as written between `{}`.
    pub name: String,
    /// Where that name is declared, if anywhere.
    pub origin: Origin,
}

/// Where such a value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Named in this applet's own `sources`.
    Declared,
    /// Nothing declares it, so the pad draws the name itself. There is nothing prebaked to fall back on: a
    /// source says how it reads, and a name with no source behind it has nothing.
    Missing,
}

impl FieldValue {
    /// What this field holds, as text. A number is not one of these: the pickers ask about names only.
    pub fn text(&self) -> &str {
        match self {
            FieldValue::Text(held) | FieldValue::OptionalText(held) => held,
            FieldValue::Choice(held, _) => held,
            FieldValue::Number(_) | FieldValue::OptionalNumber(_) | FieldValue::Tick(_) => "",
        }
    }
}

/// What a widget's field holds, in the shape the control that sets it needs.
pub enum FieldValue {
    /// A number the file always holds.
    Number(f64),
    /// Yes or no.
    Tick(bool),
    /// A number the file may leave out, where leaving it out means something.
    OptionalNumber(Option<f64>),
    /// Text the file always holds.
    Text(String),
    /// Text that may be left out of the file, where empty means the key is not written at all.
    OptionalText(String),
    /// One of a fixed set of words, with the set it is chosen from.
    Choice(String, &'static [&'static str]),
}

/// One widget in the applet being designed, ready to be drawn.
pub struct WidgetRow {
    /// Where it is in the applet's list of widgets.
    pub index: usize,
    /// Its kind, as the file's `type` spells it.
    pub kind: String,
    /// One entry per field the kind has.
    pub fields: Vec<WidgetField>,
    /// Every value the widget reads, and where each one comes from. A widget showing its own name on the pad
    /// - `{gpu}` - is a widget whose value came from nowhere, and nothing on the tab used to say so.
    pub reads: Vec<ValueRead>,
    /// True when the type is one this build does not draw. Every key it has is left exactly as it is: a widget
    /// from a newer file must survive being looked at.
    pub unknown: bool,
}

/// A whole number as the file should hold it, rather than `2.0` for two.
pub(crate) fn number_value(value: f64) -> serde_json::Value {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        serde_json::json!(value as i64)
    } else {
        serde_json::json!(value)
    }
}

/// The window's state.
/// An applet file that has been read and is waiting for a decision, because its name is already
/// here. Nothing is written while this exists: what is different is on the window, and you say.
#[derive(Clone, Debug)]
pub struct WaitingImport {
    /// The applet as it was packed up.
    pub bundle: crate::bundle::Bundle,
    /// What is different about the applet you already have: (what, yours, its).
    pub lines: Vec<(String, String, String)>,
    /// The name a copy would take, worked out when the file was read so it cannot drift while you look.
    pub copy_name: String,
}
