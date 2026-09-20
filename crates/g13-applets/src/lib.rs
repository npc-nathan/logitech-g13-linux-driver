//! Applets: what to show, where, and from which source.
//!
//! An applet is a JSON file in `~/.config/g13/applets/`, written by its owner. This module reads that file,
//! resolves its sources through `g13-sources`, and draws the widgets into a `g13-screen::Frame`.
//!
//! The file format is not invented here: it is the shape the applets already have, so an existing set keeps
//! working. What is read is what those files use:
//!
//! ```json
//! {
//!   "name": "gpu", "title": "GPU", "interval": 1, "border": true,
//!   "sources": {"util": "cmd:nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits"},
//!   "widgets": [
//!     {"type": "text", "x": 3, "y": 12, "format": "GPU:"},
//!     {"type": "bar", "x": 30, "y": 12, "w": 100, "h": 9, "source": "util", "max": 100},
//!     {"type": "text", "x": 157, "y": 12, "align": "right", "format": "{util}%"}
//!   ]
//! }
//! ```
//!
//! Two rules are the point of the whole exercise:
//!
//! - **Ink only on blank pixels.** Text must not land on top of a picture or on a bar, or it cannot be read.
//!   Drawing reports every pixel it could not place, rather than laying ink on ink and leaving it unreadable.
//! - **A missing value leaves the pixels alone.** A source that cannot be read draws nothing rather than a
//!   zero, because a zero is a claim and nothing is not.

// a test may unwrap and may fail loudly: a test that cannot panic on a fixture cannot fail
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::print_stdout,
        clippy::print_stderr
    )
)]
use g13_screen::{ADVANCE_COLUMNS, Frame, TEXT_ROWS};
use g13_sources::{Spec, World, resolve};
use g13_values::Value;
use std::collections::BTreeMap;
use std::path::Path;

mod json;
pub mod theme;

pub use theme::{Theme, ThemeFrame};

pub use json::parse_applet;

/// Where text sits when it is anchored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Text starts at `x` and runs rightwards from it.
    Left,
    /// Text ends at `x`, laid out leftwards from it.
    Right,
    /// Text is centred on `x`.
    Centre,
}

impl Align {
    /// The alignment a file's own word names: `right`, `center` or `centre`, and anything else left.
    fn parse(text: &str) -> Self {
        match text {
            "right" => Align::Right,
            "center" | "centre" => Align::Centre,
            _ => Align::Left,
        }
    }
}

/// One thing to draw.
#[derive(Debug, Clone, PartialEq)]
pub enum Widget {
    /// A line of text, filled from this applet's own values.
    Text {
        /// Where the text is anchored, in pixels: its left edge, right edge or middle, as `align` says.
        x: usize,
        /// The row the text is drawn on, counted from the top of the panel.
        y: usize,
        /// Which part of the text `x` names.
        align: Align,
        /// The text to draw, with `{name}` replaced by the value of each source.
        format: String,
        /// The width a scrolling widget is allowed, in characters.
        scroll_width: Option<usize>,
        /// Whether text longer than `scroll_width` runs through it rather than being cut off.
        ///
        /// The previous stack's own two keys: `scroll` for whether, `scroll_speed` for how fast in characters a
        /// second. They were recognised and not acted on until the running text was built: an option beside
        /// scroll width that marquees the text within that scroll width rather than cutting it off. The previous
        /// stack's vocabulary is kept rather than a new spelling invented beside it.
        scroll: bool,
        /// How fast a scrolling text travels, in pixels a second.
        scroll_speed: f64,
        /// What the pad runs when this text is the chosen thing, if anything.
        command: Option<String>,
    },
    /// A bar filled from the left in proportion to one of the applet's values.
    Bar {
        /// The bar's left edge, in pixels.
        x: usize,
        /// The row the bar starts on.
        y: usize,
        /// How many pixels wide a full bar is.
        w: usize,
        /// How many rows tall the bar is.
        h: usize,
        /// Which of the applet's sources the fill is read from.
        source: String,
        /// The value that fills the bar completely; anything above it is drawn full rather than spilling.
        max: f64,
        /// What the pad runs when the bar is the chosen thing, if anything.
        command: Option<String>,
    },
    /// One lit row of pixels, as wide as it says.
    Line {
        /// The line's left end, in pixels.
        x: usize,
        /// The row the line is drawn on.
        y: usize,
        /// How many pixels long the line is.
        w: usize,
        /// What the pad runs when the line is the chosen thing, if anything.
        command: Option<String>,
    },
    /// A row of separate blocks, lit from the left up to the value - the shape a volume or a battery uses.
    Segments {
        /// The left edge of the first block, in pixels.
        x: usize,
        /// The row the blocks are drawn on.
        y: usize,
        /// The width the blocks and the gaps between them fit into, in pixels.
        w: usize,
        /// How many rows tall each block is.
        h: usize,
        /// How many blocks there are to light.
        count: usize,
        /// Which of the applet's sources the lit count is read from.
        source: String,
        /// The value at which every block is lit.
        max: f64,
        /// What the pad runs when the segments are the chosen thing, if anything.
        command: Option<String>,
    },
    /// A list of the source's own lines, with the chosen one marked.
    ///
    /// The items are read, not declared: a source that answers with one line per item is what makes a list a
    /// list, and which one is chosen belongs to the driver rather than to the applet - it is the pad's controls
    /// that move it, and it has to survive the screen being redrawn.
    List {
        /// The left edge the rows are drawn from, in pixels.
        x: usize,
        /// The row the first visible item is drawn on.
        y: usize,
        /// How many characters of each item fit before its end is taken.
        w: usize,
        /// How many items are shown at once, with the chosen one kept in view.
        rows: usize,
        /// Which of the applet's sources gives the items, one to a line.
        source: String,
        /// What the pad runs for the chosen row, which may name that row as `{screen_item}`.
        command: Option<String>,
    },
    /// A named bitmap, drawn where it says. The one way a picture is drawn - an icon is a bitmap.
    Bitmap {
        /// The left edge the picture is pasted at, in pixels.
        x: usize,
        /// The row the picture's top is pasted at.
        y: usize,
        /// Which picture to draw, named in the applet's own bitmaps file or the shared one.
        name: String,
        /// What the pad runs when the picture is the chosen thing, if anything.
        command: Option<String>,
    },
    /// A button: a box with a label in it, and the command the pad runs when it is the chosen one.
    ///
    /// It exists so a screen of buttons is a screen of buttons rather than four text widgets that happen to
    /// carry commands - the box is what says "this is something you press", and the pad's own border lands on it.
    Button {
        /// The box's left edge, in pixels.
        x: usize,
        /// The row the box's top is on.
        y: usize,
        /// How many pixels wide the box is.
        w: usize,
        /// How many rows tall the box is.
        h: usize,
        /// The label drawn in the box, filled the way a text widget's format is.
        format: String,
        /// What the pad runs when the button is chosen, and what makes the box something you press.
        command: Option<String>,
    },
    /// A decorative frame: `[` and `]` drawn `thick` pixels wide, `h` high.
    Brackets {
        /// The left edge of the left bracket, in pixels.
        x: usize,
        /// The row the brackets start at.
        y: usize,
        /// How many rows tall the brackets are.
        h: usize,
        /// How far apart the two brackets stand, in pixels.
        len: usize,
        /// How many pixels wide each bracket is drawn.
        thick: usize,
        /// What the pad runs when the brackets are the chosen thing, if anything.
        command: Option<String>,
    },
    /// A direction pointer, turning with its source.
    Arrow {
        /// The pixel the pointer turns about, across the panel.
        x: usize,
        /// The row the pointer turns about.
        y: usize,
        /// How far the pointer reaches from where it turns, in pixels.
        radius: usize,
        /// Which of the applet's sources the heading is read from: degrees clockwise from up.
        source: String,
        /// What the pad runs when the arrow is the chosen thing, if anything.
        command: Option<String>,
    },
}

/// What a widget does when it moves.
///
/// A scroll is the text widget's own field, because it needs a width; these need nothing but the clock, so they
/// are one word per widget. Which widgets blink is the widget's business; *how* the blink goes is the theme's, so
/// two applets with the same theme breathe together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Animate {
    /// Ink on during the theme's pulse, off the rest of the time.
    Blink,
    /// Reveal the text at the theme's typing speed, then start again.
    Typewriter,
}

impl Animate {
    /// The words a file may use. `pulse` is `blink`, said the way a theme says it.
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "blink" | "pulse" | "flash" => Some(Animate::Blink),
            "typewriter" | "type" => Some(Animate::Typewriter),
            _ => None,
        }
    }

    /// Every spelling `from_word` accepts, in the order a designer should offer them.
    pub const WORDS: [&'static str; 5] = ["blink", "pulse", "flash", "typewriter", "type"];
}

/// What a drawing knows about the moment: which widget the pad is on, what the clock says, and which profile is
/// in force. One thing rather than three parameters, because the last of them is what an alert asking about the
/// profile needs.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// Which of the things the pad can act on it is on, or `None` when nothing is.
    pub selected: Option<usize>,
    /// The clock the drawing is a function of - a scroll, an animation and an alert all read it.
    pub now_seconds: f64,
    /// Which binding set is in force, for an alert that asks about the profile.
    pub profile: u32,
}

impl Context {
    /// A drawing with nothing selected but the clock, which is what a preview wants.
    pub fn at(now_seconds: f64) -> Self {
        Context {
            selected: None,
            now_seconds,
            profile: 0,
        }
    }
}

/// What a widget does when a question about it is true.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertLook {
    /// Ink on and off quickly, and fixed rather than the theme's pulse: an alarm should not be slow because a
    /// theme is calm.
    Flash,
    /// Filled in with its words punched out, the way the pad marks the button it is on.
    Invert,
    /// Draw the named bitmap where the widget is.
    Show,
    /// A frame round the widget, flashing.
    Border,
}

impl AlertLook {
    /// The look a file's word names, or `None` for a word this build does not know.
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "flash" | "blink" => Some(AlertLook::Flash),
            "invert" | "inverted" => Some(AlertLook::Invert),
            "show" | "bitmap" => Some(AlertLook::Show),
            "border" | "frame" => Some(AlertLook::Border),
            _ => None,
        }
    }

    /// Every spelling `from_word` accepts, in the order a designer should offer them.
    pub const WORDS: [&'static str; 4] = ["flash", "invert", "show", "border"];
}

/// A question about a widget, and what to do while the answer is yes.
///
/// The condition is the macros' own - the same parser, the same evaluator - so there is one language for "when is
/// this true" in the whole program. It may name the values its applet declares, which is the rule a widget's
/// `source` already follows, and the active profile.
#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    /// The question asked, in the macros' own language: it may name this applet's values and its profile.
    pub condition: g13_values::Cond,
    /// What is drawn while the answer is yes.
    pub look: AlertLook,
}

/// The theme an applet says it wants, by name, or none for the look it had before themes existed.
pub const THEME_KEY: &str = "theme";
/// The key an applet names its font with, in the file: `"font": "runes"`.
pub const FONT_KEY: &str = "font";

/// The key an applet says "show me while my data is arriving" with, in the file: `"follow": {"seconds": 20}`.
pub const FOLLOW_KEY: &str = "follow";

/// An applet fed from outside, which takes the screen while its data is arriving.
///
/// A game HUD is written by the game, so the applet can say so:
///
/// ```json
/// "follow": {"seconds": 20, "profile": "Cyberpunk"}
/// ```
///
/// While the file behind one of this applet's own sources is being written, the applet is what is on the screen -
/// and, if it names a binding set, that set is the one in force. When the writing stops the screen and the profile
/// both go back to what they were. `"follow": true` uses the default window, a bare number is the same as
/// `{"seconds": n}`, and an applet with no `follow` is only ever shown because somebody chose it.
#[derive(Clone, Debug, PartialEq)]
pub struct Follow {
    /// How long it keeps the screen after the writing stops. A pause in a game is not a quit.
    pub seconds: f64,
    /// A binding set to switch to while it is live, by the name it has in `profiles.json`.
    pub profile: Option<String>,
}

impl Follow {
    /// How long an applet keeps the screen when it says `true` rather than a number.
    pub const DEFAULT_SECONDS: f64 = 20.0;
}

/// An applet as its file describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Applet {
    /// The file's stem, which is also how a visual names the applet: `applet:<name>`.
    pub name: String,
    /// What the menu calls it; the file's own `title`, or the name when it gives none.
    pub title: String,
    /// Seconds between readings of the sources.
    pub interval: f64,
    /// Whether the panel's own frame is drawn round this applet.
    pub border: bool,
    /// This applet's readings, by the name its widgets write in `{name}`: each holds one source spec.
    pub sources: BTreeMap<String, String>,
    /// The applet's widgets when it is a single screen, which is what an applet without `screens` is.
    pub widgets: Vec<Widget>,
    /// More than one screen, when the file says so. Empty means "one screen, made of `widgets`", so an applet
    /// written before this existed is unchanged and needs no rewriting.
    pub screens: Vec<Screen>,
    /// The pictures this applet can draw, put on it by `with_bitmaps` rather than read while parsing: the
    /// parser has the file's text and not the folder it lives in.
    pub bitmaps: BTreeMap<String, Bitmap>,
    /// The theme it named, if any, and what that theme is. Put on it by `with_theme`, for the same reason
    /// `bitmaps` is: the parser has the file's text, not the folder it lives in.
    pub theme_name: Option<String>,
    /// The theme itself, put on by `with_theme`; until then, the look everything had before themes existed.
    /// An applet that names no theme keeps that look.
    pub theme: Theme,
    /// The font it named, if any, and what that font is. Put on it by `with_font`, for the same reason `theme` is
    /// put on by `with_theme`: the parser has the file's text, not the folder it lives in.
    pub font_name: Option<String>,
    /// The font itself, put on by `with_font`; `None` draws the screen in the panel's own.
    pub font: Option<g13_screen::font::Font>,
    /// Every other font in the folder, in name order, tried after the named one.
    ///
    /// **This is what makes a value like `media_title` work in any language.** A title is arbitrary text from
    /// outside, such as a Japanese song in an English applet, and the applet did not name a font for it and could not.
    /// Every font loaded is a fallback for every screen, so the coverage is what the folder holds rather than what
    /// one file declared.
    pub fallbacks: Vec<g13_screen::font::Font>,
    /// Which widgets move, and how. One map per screen, in the numbering `screen_widgets` uses: a single-screen
    /// applet is map 0, and an applet with `screens` has them from 1, so the two numbering schemes cannot be
    /// confused for each other.
    pub animations: Vec<BTreeMap<usize, Animate>>,
    /// Which widgets react to something, and to what. Kept and numbered the same as `animations`, so there is one
    /// way of pointing at a widget of a screen.
    pub alerts: Vec<BTreeMap<usize, Alert>>,
    /// What each widget does when it is chosen, which may be more than one thing: play a track *and* go back to
    /// the screen that was showing. `command` in the file is one action written plainly, or a list of them.
    pub commands: Vec<BTreeMap<usize, Vec<String>>>,
    /// Keys this build recognises but does not yet act on, reported rather than silently dropped.
    pub unhandled: Vec<String>,
    /// Set when this applet is fed from outside and should take the screen while its data arrives.
    ///
    /// `"follow": {"seconds": 20, "profile": "Cyberpunk"}` - see [`Follow`].
    pub follow: Option<Follow>,
}

/// One screen of an applet: its own title and its own widgets. The sources are the applet's, because a screen is
/// a view of the same readings rather than a different set of them.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    /// What the menu and the designer call this screen, where the file gives it one.
    pub title: String,
    /// The things drawn on this screen, in the order they are drawn.
    pub widgets: Vec<Widget>,
}

/// How many screens this applet has. One, unless the file lists more.
pub fn screen_count(applet: &Applet) -> usize {
    if applet.screens.is_empty() {
        1
    } else {
        applet.screens.len()
    }
}

/// The widgets to draw for one of an applet's screens.
///
/// An index past the end is an empty screen rather than a panic: a number can arrive from a file or a pad and
/// neither is worth taking the driver down for.
impl Applet {
    /// Which animation this widget of this screen has, if any.
    pub fn animate(&self, screen: usize, index: usize) -> Option<Animate> {
        let at = if self.screens.is_empty() {
            0
        } else {
            screen + 1
        };
        self.animations
            .get(at)
            .and_then(|map| map.get(&index))
            .copied()
    }

    /// Everything this widget does when it is chosen, in order.
    ///
    /// A widget whose file says `"command": "cmd:x"` answers with that one, so every applet written before lists
    /// existed is unchanged; one that says `"command": ["cmd:x", "screen:2"]` answers with both.
    pub fn commands(&self, screen: usize, index: usize) -> Vec<&str> {
        let at = if self.screens.is_empty() {
            0
        } else {
            screen + 1
        };
        match self.commands.get(at).and_then(|map| map.get(&index)) {
            Some(list) => list.iter().map(String::as_str).collect(),
            None => self
                .screen_widgets(screen)
                .get(index)
                .and_then(Widget::command)
                .into_iter()
                .collect(),
        }
    }

    /// The alert this widget of this screen carries, if any.
    pub fn alert(&self, screen: usize, index: usize) -> Option<&Alert> {
        let at = if self.screens.is_empty() {
            0
        } else {
            screen + 1
        };
        self.alerts.get(at).and_then(|map| map.get(&index))
    }

    /// Whether anything on this screen moves by itself, so the drawing has to be faster than the reading.
    ///
    /// One place answers this: a scroll, a widget with an animation, or a theme with a screen-wide effect. The
    /// driver's draw rate, the window's preview and anything else that needs to know ask *this*, so a new effect
    /// is added in one place rather than in every surface that cares.
    pub fn moves(&self, screen: usize) -> bool {
        if self.theme.moves() {
            return true;
        }
        let widgets = self.screen_widgets(screen);
        // a picture with more than one frame is motion, in the same place every other kind of motion is answered
        let animated = widgets.iter().any(|widget| match widget {
            Widget::Bitmap { name, .. } => self
                .bitmaps
                .get(name)
                .map(Bitmap::animated)
                .unwrap_or(false),
            _ => false,
        });
        // and a widget that reacts to something can flash, which is motion too
        let reacting = widgets
            .iter()
            .enumerate()
            .any(|(index, _)| self.alert(screen, index).is_some());
        animated
            || reacting
            || widgets.iter().any(Widget::scrolls)
            || widgets
                .iter()
                .enumerate()
                .any(|(index, _)| self.animate(screen, index).is_some())
    }

    /// The widgets of one screen, in the numbering this applet uses for its animations.
    pub fn screen_widgets(&self, screen: usize) -> &[Widget] {
        screen_widgets(self, screen)
    }
}

/// The widgets of one of an applet's screens.
/// A single-screen applet answers only to 0, and a screen that does not exist is empty rather than a panic:
/// a number can arrive from a file or from the pad, and neither is worth taking the driver down for.
pub fn screen_widgets(applet: &Applet, screen: usize) -> &[Widget] {
    if applet.screens.is_empty() {
        // one screen, so any other number is a screen that does not exist - and an empty one is a truer answer
        // than showing the same widgets under a number nobody has
        return if screen == 0 { &applet.widgets } else { &[] };
    }
    applet
        .screens
        .get(screen)
        .map(|screen| screen.widgets.as_slice())
        .unwrap_or(&[])
}

/// What a screen is called, for the menu and the designer.
pub fn screen_title(applet: &Applet, screen: usize) -> String {
    match applet.screens.get(screen) {
        Some(found) if !found.title.trim().is_empty() => found.title.clone(),
        // no title of its own: say which one it is rather than showing nothing
        _ => format!("screen {}", screen + 1),
    }
}

/// A value for each source name, or `Missing`.
pub type Values = BTreeMap<String, Value>;

/// Every applet that exists, by name, in the directory applets live in.
///
/// The name is the file's stem, which is also how a visual refers to it: `applet:<name>`.
pub fn applet_names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if !is_applet_file(&path) {
                        return None;
                    }
                    path.file_stem()
                        .map(|stem| stem.to_string_lossy().to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Where one applet's file is.
pub fn applet_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    dir.join(format!("{name}.json"))
}

/// Read one applet's file as it is written, with every key it holds.
///
/// **The designer edits this rather than a parsed [`Applet`]**, and that is deliberate: reading into the struct
/// and writing it back would drop anything this build does not act on - an applet file is hand-written, and
/// `unhandled` exists precisely because some keys are recognised and not acted on yet. Editing the file's own
/// JSON keeps everything the person put there.
pub fn load_applet_json(dir: &std::path::Path, name: &str) -> Result<serde_json::Value, String> {
    let path = applet_path(dir, name);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))?;
    if !value.is_object() {
        return Err(format!(
            "{} should hold an object, one applet per file",
            path.display()
        ));
    }
    Ok(value)
}

/// Write an applet's file back, once what is written has been read as an applet.
/// The file lands whole - written beside its name and renamed over it - so a driver reading it on a change
/// never sees half an applet.
pub fn save_applet_json(
    dir: &std::path::Path,
    name: &str,
    value: &serde_json::Value,
) -> Result<(), String> {
    let text = applet_json_text(value)?;
    // whatever is written must read back as an applet, or the next redraw has nothing to draw
    parse_applet(&text)?;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("{} could not be created: {error}", dir.display()))?;
    // beside and renamed, so the driver reading it on a change never sees half an applet
    g13_files::write(&applet_path(dir, name), &text)
}

/// An applet, as its file is written: the same shape a person would type.
pub fn applet_json_text(value: &serde_json::Value) -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("this applet cannot be written: {error}"))?;
    text.push('\n');
    Ok(text)
}

/// The applet's values again, re-reading only the ones that move with the pad.
///
/// A screen is drawn many times between two readings of a slow source - a `cmd:` costs a hundred milliseconds
/// and up - so the values that cost are kept and these are asked for anew. `None` when the applet has no fast
/// source at all, which is every applet that was written before this and most after it: then the kept values are
/// the answer and nothing is re-read.
pub fn refresh_fast(
    applet: &Applet,
    kept: &Values,
    world: &World,
) -> Option<(Values, Vec<String>)> {
    let fast: Vec<(&String, &String)> = applet
        .sources
        .iter()
        .filter(|(_, source)| g13_sources::changes_every_frame(source.trim()))
        .collect();
    if fast.is_empty() {
        return None;
    }
    let mut values = kept.clone();
    let mut problems = Vec::new();
    for (name, source) in fast {
        match resolve(&Spec::parse(source), world) {
            Ok(value) => {
                values.insert(name.clone(), value);
            }
            Err(problem) => {
                values.insert(name.clone(), Value::Missing);
                problems.push(format!("{name} ({source}): {problem}"));
            }
        }
    }
    Some((values, problems))
}

/// Read every source an applet declares.
pub fn gather(applet: &Applet, world: &World) -> (Values, Vec<String>) {
    let mut values = BTreeMap::new();
    let mut problems = Vec::new();
    for (name, source) in &applet.sources {
        let spec = Spec::parse(source);
        match resolve(&spec, world) {
            Ok(value) => {
                values.insert(name.clone(), value);
            }
            Err(problem) => {
                values.insert(name.clone(), Value::Missing);
                problems.push(format!("{name} ({source}): {problem}"));
            }
        }
    }
    (values, problems)
}

/// The names the applet reads that nothing provides.
///
/// A format asks for `{cpu}` and a bar names its source; if no source of that name resolves, the pad draws the
/// name itself. That is a silent failure unless something says so, so `check` says it - the alternative is
/// finding out from the pad, which is how a widget went on showing `{gpu}%` with nothing to report it.
pub fn values_nothing_provides(applet: &Applet, values: &Values) -> Vec<String> {
    names_read(applet)
        .into_iter()
        .filter(|name| !values.contains_key(name))
        .collect()
}

/// Every name that any widget of this applet reads, across **all** of its screens.
///
/// The one rule for "what does this applet read". It walked only the applet's own `widgets` for a while, which
/// is nothing at all once an applet has `screens` - so an applet whose readings were all on a screen reported
/// that every source it declared was read by nothing, and the sources panel said `nothing reads it` beside names
/// the widgets below were using.
pub fn names_read(applet: &Applet) -> Vec<String> {
    let mut wanted: Vec<String> = Vec::new();
    for screen in 0..screen_count(applet) {
        for widget in screen_widgets(applet, screen) {
            let names: Vec<String> = match widget {
                Widget::Text { format, .. } => g13_sources::names_in_format(format),
                Widget::Bar { source, .. }
                | Widget::Segments { source, .. }
                | Widget::List { source, .. }
                | Widget::Arrow { source, .. } => vec![source.clone()],
                Widget::Button { format, .. } => g13_sources::names_in_format(format),
                _ => Vec::new(),
            };
            for name in names {
                if !wanted.contains(&name) {
                    wanted.push(name);
                }
            }
        }
    }
    wanted
}

/// Every widget kind this build reads, in the order the designer offers them.
///
/// One list, owned by the format: the window offers what is here and nothing else, so a kind the reader
/// accepts cannot be missing from the add button - which is exactly how `list` arrived with fields and no way
/// to add one.
/// One bitmap: `w` by `h` pixels, written as `h` strings of `w` characters, `#` lit and anything else not.
///
/// The one way this project draws a picture. An icon is a bitmap and so is a button's face; there is no glyph
/// set beside it, because two ways to draw the same thing is two ways to keep in step.
#[derive(Debug, Clone, PartialEq)]
pub struct Bitmap {
    /// How many pixels wide one frame is.
    pub w: usize,
    /// How many rows tall one frame is.
    pub h: usize,
    /// How long one frame is held, in milliseconds. A single-frame bitmap does not care.
    pub ms: f64,
    /// One picture per frame of the animation. A bitmap whose file has no `frames` is one frame, which is every
    /// bitmap written before frames existed - so no picture already drawn changes meaning.
    pub frames: Vec<Vec<Vec<bool>>>,
}

impl Bitmap {
    /// The picture to draw at this moment.
    ///
    /// Chosen by the clock and nothing else: the same moment draws the same frame every time, so a sprite cannot
    /// run at a speed that depends on how often the screen happened to be drawn. This is the lesson the running
    /// text taught twice - a picture that moves must not take its time from a counter or a reading.
    pub fn frame_at(&self, now_seconds: f64) -> &[Vec<bool>] {
        if self.frames.len() <= 1 {
            return self.frames.first().map(Vec::as_slice).unwrap_or(&[]);
        }
        // Exactly the hold the file asks for. It used to be snapped to a whole number of draws, so that an
        // animation could not step unevenly - and that was wrong: it made `"ms": 120` mean 100, and a value that
        // means something other than what it says is worse than an uneven step. A hold that runs faster than it
        // states must not be reported as exact, so a slight pulse in speed is accepted here.
        let hold = self.ms.clamp(20.0, 10_000.0) / 1000.0;
        let at = (now_seconds.max(0.0) / hold) as usize % self.frames.len();
        &self.frames[at]
    }

    /// Whether this bitmap has more than one frame, so a screen showing it has to be drawn faster than it is read.
    pub fn animated(&self) -> bool {
        self.frames.len() > 1
    }
}

impl Bitmap {
    /// Read one out of the file's own JSON.
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        // one picture, read from `rows`. `frames` is read below and replaces it, so a bitmap may say either -
        // `rows` alone is one frame, which is what every bitmap written before animations existed says.
        let as_rows =
            |rows: &serde_json::Value, where_at: &str| -> Result<Vec<Vec<bool>>, String> {
                let listed = rows
                    .as_array()
                    .ok_or_else(|| format!("{where_at} needs `rows`: one string per row"))?;
                let mut picture = Vec::new();
                for row in listed {
                    let text = row
                        .as_str()
                        .ok_or_else(|| format!("{where_at}: every row is a string"))?;
                    picture.push(text.chars().map(|c| c == '#').collect::<Vec<bool>>());
                }
                Ok(picture)
            };
        let rows = match value.get("rows") {
            Some(rows) => as_rows(rows, "a bitmap")?,
            None => Vec::new(),
        };
        // `frames` is an animation; `rows` alone is one frame, which is the same thing said shorter
        let mut frames = vec![rows];
        if let Some(animated) = value.get("frames").and_then(|frames| frames.as_array()) {
            if animated.is_empty() {
                return Err(
                    "`frames` is there but empty: a bitmap needs at least one frame".to_string(),
                );
            }
            frames = Vec::new();
            for (at, frame) in animated.iter().enumerate() {
                let Some(rows) = frame.get("rows") else {
                    return Err(format!(
                        "frame {at} needs `rows`, the same as a still bitmap"
                    ));
                };
                frames.push(as_rows(rows, &format!("frame {at}"))?);
            }
        }
        if frames.iter().all(Vec::is_empty) {
            return Err(
                "a bitmap needs `rows`, or `frames` with `rows` in each: it has neither"
                    .to_string(),
            );
        }
        let first = frames.first().cloned().unwrap_or_default();
        let h = value
            .get("h")
            .and_then(|h| h.as_u64())
            .map(|h| h as usize)
            .unwrap_or(first.len());
        let w = value
            .get("w")
            .and_then(|w| w.as_u64())
            .map(|w| w as usize)
            .unwrap_or_else(|| first.iter().map(Vec::len).max().unwrap_or(0));
        // every frame is the same size, so an animation cannot jump about when it turns over
        for (at, frame) in frames.iter().enumerate() {
            if frame.len() != h || frame.iter().any(|row| row.len() != w) {
                return Err(format!(
                    "frame {at} of a {w} by {h} bitmap has {} row(s) of {} to {} characters",
                    frame.len(),
                    frame.iter().map(Vec::len).min().unwrap_or(0),
                    frame.iter().map(Vec::len).max().unwrap_or(0)
                ));
            }
        }
        let ms = value
            .get("ms")
            .and_then(|ms| ms.as_f64())
            .unwrap_or(120.0)
            .clamp(20.0, 10_000.0);
        Ok(Bitmap { w, h, ms, frames })
    }
}

/// Every bitmap an applet can draw: the shared file, then that applet's own on top, **by name**.
///
/// `~/.config/g13/bitmaps.json` is shared by every applet, and `~/.config/g13/applets/<name>.bitmaps.json` is
/// that applet's own and wins. One source of truth per level: an icon changed in an
/// applet's own file changes everywhere that applet draws it and touches no other applet.
pub fn read_bitmaps(applets: &Path, name: &str) -> (BTreeMap<String, Bitmap>, Vec<String>) {
    let mut found: BTreeMap<String, Bitmap> = BTreeMap::new();
    let mut complaints = Vec::new();
    for path in [
        applets.parent().unwrap_or(applets).join("bitmaps.json"),
        applets.join(format!("{name}.bitmaps.json")),
    ] {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        for (key, one) in value.as_object().into_iter().flatten() {
            match Bitmap::from_value(one) {
                Ok(bitmap) => {
                    found.insert(key.clone(), bitmap);
                }
                Err(problem) => complaints.push(format!(
                    "{}: {key} is not a bitmap: {problem}",
                    path.display()
                )),
            }
        }
    }
    (found, complaints)
}

/// Whether a file in the applets folder is an applet, rather than something beside one.
///
/// `applets/<name>.bitmaps.json` is that applet's pictures: the applet `audio` has a sidecar called
/// `audio.bitmaps.json`, and a folder listing that treats every `.json` as an applet invents an applet called
/// `audio.bitmaps` - which showed up in the window as one, and would have been offered to the pad's rotation as
/// a screen to draw. One rule, asked by both.
pub fn is_applet_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string());
    match name {
        Some(name) => name.ends_with(".json") && !name.ends_with(".bitmaps.json"),
        None => false,
    }
}

/// Where an applet's own bitmaps are written.
pub fn bitmaps_path(applets: &Path, name: &str) -> std::path::PathBuf {
    applets.join(format!("{name}.bitmaps.json"))
}

/// Write an applet's own bitmaps, whole or not at all.
///
/// The picture the window draws goes through here, so it is written the way everything else is: every bitmap is
/// checked by being read back as one, and the file lands in one piece (written beside its name and renamed over
/// it) so a reader never sees half of one.
pub fn write_bitmaps(
    applets: &Path,
    name: &str,
    bitmaps: &BTreeMap<String, Bitmap>,
) -> Result<(), String> {
    for (key, bitmap) in bitmaps {
        if bitmap.w == 0 || bitmap.h == 0 {
            return Err(format!(
                "{key} is {0} by {1}: a bitmap needs some pixels",
                bitmap.w, bitmap.h
            ));
        }
        for (at, frame) in bitmap.frames.iter().enumerate() {
            if frame.len() != bitmap.h || frame.iter().any(|row| row.len() != bitmap.w) {
                return Err(format!(
                    "{key} frame {at} is {0} by {1} and its rows do not match",
                    bitmap.w, bitmap.h
                ));
            }
        }
    }
    let mut object = serde_json::Map::new();
    for (key, bitmap) in bitmaps {
        let as_lines = |frames: &Vec<Vec<bool>>| -> Vec<serde_json::Value> {
            frames
                .iter()
                .map(|row| {
                    serde_json::Value::String(
                        row.iter()
                            .map(|lit| if *lit { '#' } else { '.' })
                            .collect::<String>(),
                    )
                })
                .collect()
        };
        // a still bitmap is written as `rows`, the way it always was; only an animation grows the `frames` key,
        // so opening a still picture in the editor and saving it leaves the file exactly as it was
        let written = match bitmap.animated() {
            false => serde_json::json!({
                "w": bitmap.w,
                "h": bitmap.h,
                "rows": as_lines(&bitmap.frames[0]),
            }),
            true => serde_json::json!({
                "w": bitmap.w,
                "h": bitmap.h,
                "ms": bitmap.ms,
                "frames": bitmap
                    .frames
                    .iter()
                    .map(|frame| serde_json::json!({"rows": as_lines(frame)}))
                    .collect::<Vec<_>>(),
            }),
        };
        object.insert(key.clone(), written);
    }
    let value = serde_json::Value::Object(object);
    let text = serde_json::to_string_pretty(&value).map_err(|problem| problem.to_string())?;
    // what is written must read back as bitmaps, or the applet draws a picture nobody can see
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|problem| format!("this would not read back: {problem}"))?;
    for (key, one) in value.as_object().into_iter().flatten() {
        Bitmap::from_value(one).map_err(|problem| format!("{key}: {problem}"))?;
    }
    // beside and renamed, so a driver reading it on a change never sees half a picture
    g13_files::write(&bitmaps_path(applets, name), &text)
}

/// Read the theme this applet named, if it named one.
///
/// The same shape as `with_bitmaps`, and for the same reason: the parser has the file's text, not the folder.
/// A theme that cannot be read is a *complaint*, never silence - a theme that does nothing looks exactly like a
/// theme that works and is subtle, and that is the worst of the two to debug.
pub fn with_theme(mut applet: Applet, themes_dir: &Path) -> Applet {
    let Some(name) = applet.theme_name.clone() else {
        return applet;
    };
    match theme::load(themes_dir, &name) {
        Ok(theme) => applet.theme = theme,
        Err(problem) => applet
            .unhandled
            .push(format!("theme {name}: it cannot be used - {problem}")),
    }
    applet
}

/// Read the font this applet named, if it named one.
///
/// The same shape as `with_theme` and for the same reason: the parser has the file's text, not the folder. **The font
/// goes *in front of* the panel's own** when a screen is drawn, so a character the named font lacks - a Latin letter
/// in a rune font, a digit anywhere - is still drawn rather than boxed.
pub fn with_font(mut applet: Applet, fonts: &BTreeMap<String, g13_screen::font::Font>) -> Applet {
    if let Some(name) = applet.font_name.clone() {
        match fonts.get(&name) {
            Some(font) => applet.font = Some(font.clone()),
            None => applet.unhandled.push(format!(
                "font {name}: there is no fonts/{name}.json - the panel's own font is being used, so this screen \
                 will look as it did before"
            )),
        }
    }
    // and everything else in the folder, in name order, as fallbacks. A named font was never going to cover Japanese
    // song titles, and the applet cannot know what a source will hand it.
    applet.fallbacks = fonts
        .iter()
        .filter(|(name, _)| Some(name.as_str()) != applet.font_name.as_deref())
        .map(|(_, font)| font.clone())
        .collect();
    applet
}

/// The same applet with its pictures on it, read from the shared file and its own.
///
/// The applet carries them because the drawing has no folder to look in: a widget names a bitmap, and the
/// names were resolved once, when the applet was loaded.
pub fn with_bitmaps(mut applet: Applet, applets_dir: &Path) -> Applet {
    let (bitmaps, complaints) = read_bitmaps(applets_dir, &applet.name);
    applet.bitmaps = bitmaps;
    // a picture that will not parse is said rather than left to look like a picture nobody defined
    applet.unhandled.extend(complaints);
    applet
}

/// A font from `fonts/<name>.json`, beside the applets and the themes.
///
/// **A font is a file, not a table in the binary**, for the same reason an applet is: a font can be added - a
/// script, a face of choice - without a rebuild. The glyphs are written the way every other picture in this project
/// is written, `#` for ink in strings of rows, so there is one picture format and not two.
///
/// ```json
/// {"name": "runes", "advance": 6, "line_height": 8, "glyph_rows": 7,
///  "from": "drawn here 2026-09-18 from the Unicode chart",
///  "glyphs": {"\u{16a0}": ["#.....", "#.....", ...]}}
/// ```
///
/// A file that will not read is *said*, never silently skipped: a font with a typo in it must not look like a screen
/// with no text.
pub fn load_font(path: &std::path::Path) -> Result<g13_screen::font::Font, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))?;
    let number = |key: &str, fallback: usize| -> usize {
        value
            .get(key)
            .and_then(|it| it.as_u64())
            .map(|it| it as usize)
            .unwrap_or(fallback)
    };
    let advance = number("advance", g13_screen::font::ADVANCE);
    let line_height = number("line_height", g13_screen::font::LINE_HEIGHT);
    let glyph_rows = number("glyph_rows", g13_screen::font::GLYPH_ROWS);
    let name = value
        .get("name")
        .and_then(|it| it.as_str())
        .map(str::to_string)
        .or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "font".to_string());
    let Some(glyphs_object) = value.get("glyphs").and_then(|it| it.as_object()) else {
        return Err(format!(
            "{} has no `glyphs`: it should hold one entry per character, each a list of row strings",
            path.display()
        ));
    };
    let mut glyphs = std::collections::BTreeMap::new();
    for (character, rows) in glyphs_object {
        let mut characters = character.chars();
        let Some(one) = characters.next() else {
            continue;
        };
        if characters.next().is_some() {
            return Err(format!(
                "{}: `{character}` is more than one character, and a glyph is one",
                path.display()
            ));
        }
        let Some(rows) = rows.as_array() else {
            return Err(format!(
                "{}: the glyph for `{character}` should be a list of row strings",
                path.display()
            ));
        };
        let lines: Vec<&str> = rows.iter().filter_map(|row| row.as_str()).collect();
        if lines.len() != glyph_rows {
            return Err(format!(
                "{}: the glyph for `{character}` has {} rows where the font says {glyph_rows}",
                path.display(),
                lines.len()
            ));
        }
        let mut ink = [0u8; 8];
        for (row, line) in lines.iter().enumerate() {
            for (column, cell) in line.chars().enumerate().take(8) {
                if cell == '#' {
                    ink[column] |= 1 << row;
                }
            }
        }
        glyphs.insert(one, ink);
    }
    Ok(g13_screen::font::Font::new(
        name,
        advance,
        line_height,
        glyph_rows,
        glyphs,
    ))
}

/// The fonts folder, read once and kept until a font file changes.
///
/// The same shape as the media readings' cache and for the same reason: this is asked for on the drawing side, twenty
/// times a second, and parsing every glyph of every font that often would be absurd. The folder's own newest
/// modification time and its file count are the test - reading a directory costs microseconds, parsing does not.
pub fn fonts_cached(
    dir: &std::path::Path,
) -> (
    std::collections::BTreeMap<String, g13_screen::font::Font>,
    Vec<String>,
) {
    /// The newest modification time in the folder, and how many font files it holds.
    fn stamp(dir: &std::path::Path) -> (std::time::SystemTime, usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return (std::time::SystemTime::UNIX_EPOCH, 0);
        };
        let mut newest = std::time::SystemTime::UNIX_EPOCH;
        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|it| it.to_str()) != Some("json") {
                continue;
            }
            count += 1;
            if let Ok(modified) = entry.metadata().and_then(|it| it.modified()) {
                newest = newest.max(modified);
            }
        }
        (newest, count)
    }
    type Kept = (
        std::time::SystemTime,
        usize,
        std::collections::BTreeMap<String, g13_screen::font::Font>,
        Vec<String>,
    );
    static KEPT: std::sync::OnceLock<std::sync::Mutex<Kept>> = std::sync::OnceLock::new();
    let now = stamp(dir);
    let kept = KEPT.get_or_init(|| {
        std::sync::Mutex::new((
            std::time::SystemTime::UNIX_EPOCH,
            0,
            std::collections::BTreeMap::new(),
            vec![String::new()],
        ))
    });
    if let Ok(held) = kept.lock()
        && held.0 == now.0
        && held.1 == now.1
        && held.3.is_empty()
    {
        return (held.2.clone(), Vec::new());
    }
    let (fonts, problems) = load_fonts(dir);
    if let Ok(mut held) = kept.lock() {
        *held = (now.0, now.1, fonts.clone(), problems.clone());
    }
    (fonts, problems)
}

/// Every font in the fonts folder, by name, with the ones that would not read and why.
///
/// A missing folder is nothing to do, not a fault: the built-in font is always there.
pub fn load_fonts(
    dir: &std::path::Path,
) -> (
    std::collections::BTreeMap<String, g13_screen::font::Font>,
    Vec<String>,
) {
    let mut fonts = std::collections::BTreeMap::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (fonts, problems);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|it| it.to_str()) != Some("json") {
            continue;
        }
        match load_font(&path) {
            Ok(font) => {
                fonts.insert(font.name.clone(), font);
            }
            Err(problem) => problems.push(problem),
        }
    }
    (fonts, problems)
}

/// Every widget kind this build reads, in the order the designer offers them.
pub const WIDGET_KINDS: [&str; 9] = [
    "text", "bar", "line", "segments", "brackets", "arrow", "list", "button", "bitmap",
];

/// How long a frame lasts on a screen that is being drawn as fast as it can be.
///
/// Kept as a documented fact rather than as a rule the drawing applies: a frame can only change when the screen is
/// drawn, so a speed that is not a whole number of pixels a frame *will* step unevenly. `docs/applets-and-sources.md`
/// says which speeds those are. The drawing honours the number in the file and nothing else.
pub const PANEL_FRAME_SECONDS: f64 = 0.05;

/// The name the driver publishes which of a screen's actionable things the pad is on, counted from one.
///
/// An applet declares it as a source if it wants to show it (`"sel": "screen_selected"`). The border the pad
/// draws itself does **not** come through here: it is handed to `draw_screen_at` as a parameter, because an
/// applet's values are exactly the ones it declares and a border read out of them would never be drawn.
pub const SELECTED_VALUE: &str = "screen_selected";

/// Replace `{name}` and `{name:.0f}` in a format string.
pub fn fill(format: &str, values: &Values) -> String {
    substitute(format, values, false)
}

/// Replace `{name}` in a command, quoting every value so the shell receives it as one argument.
///
/// A command is run by a shell, and a value is whatever the source answered with - a line of a file, a
/// field of an endpoint's reply, a track's title. Substituted as bare text, a value holding `;`, a backtick
/// or `$(...)` is not data any more: it is shell syntax, and it runs. Quoted, it stays one argument.
///
/// Not `fill`, and not the same job: a format is text for the panel and is never run, so quoting it would
/// put the quotes on the screen. The kind is read from the spec's own prefix, before anything is
/// substituted into it, so a `screen:` target keeps its spelling - quoting that would name a screen that
/// does not exist.
///
/// Literal text is passed through exactly as written, an awk body's own braces and quotes included. A name
/// that is not a value stays visible as `{name}`, so a typo shows up in the command rather than
/// disappearing from it.
pub fn fill_command(spec: &str, values: &Values) -> String {
    let runs = g13_sources::Spec::parse(spec).kind == "cmd";
    substitute(spec, values, runs)
}

/// The one scan behind `fill` and `fill_command`: `quoting` decides what happens to a value that is found.
///
/// Two copies of this would drift, and the drift would be a value quoted on the screen or a value run
/// unquoted - which is the fault this exists to prevent, arrived at from the other side.
fn substitute(text: &str, values: &Values, quoting: bool) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &rest[start + 1..start + end];
        let (name, spec) = match token.split_once(':') {
            Some((name, spec)) => (name, format!(":{spec}")),
            None => (token, String::new()),
        };
        match values.get(name) {
            Some(value) => {
                let text = value.formatted(&spec);
                match quoting {
                    true => out.push_str(&shell_quote(&text)),
                    false => out.push_str(&text),
                }
            }
            // an unknown name stays visible as itself, so a typo shows up instead of vanishing
            None => out.push_str(&format!("{{{token}}}")),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// One value, as one word for `sh`: wrapped in single quotes, with any quote of its own closed, escaped
/// and reopened, because a single quote is the one character single quotes do not carry.
fn shell_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for character in text.chars() {
        match character {
            '\'' => out.push_str("'\\''"),
            other => out.push(other),
        }
    }
    out.push('\'');
    out
}

impl Widget {
    /// Whether this widget moves on its own, so the screen it is on has to be drawn faster than it is read.
    ///
    /// One place answers this: the draw rate, the window's preview and anything else that needs to know ask the
    /// widget rather than keeping their own list of kinds.
    pub fn scrolls(&self) -> bool {
        matches!(
            self,
            Widget::Text {
                scroll: true,
                scroll_width: Some(width),
                ..
            } if *width > 0
        )
    }

    /// What this widget does when the pad's L4 is pressed on it, if anything.
    pub fn command(&self) -> Option<&str> {
        match self {
            Widget::Text { command, .. }
            | Widget::Bar { command, .. }
            | Widget::Line { command, .. }
            | Widget::Segments { command, .. }
            | Widget::List { command, .. }
            | Widget::Button { command, .. }
            | Widget::Bitmap { command, .. }
            | Widget::Brackets { command, .. }
            | Widget::Arrow { command, .. } => command.as_deref(),
        }
    }

    /// The same widget, carrying a command. Written once so a new kind cannot be the one that forgets it.
    pub fn with_command(self, command: Option<String>) -> Self {
        match self {
            Widget::Text {
                x,
                y,
                align,
                format,
                scroll_width,
                scroll,
                scroll_speed,
                ..
            } => Widget::Text {
                x,
                y,
                align,
                format,
                scroll_width,
                scroll,
                scroll_speed,
                command,
            },
            Widget::Bar {
                x,
                y,
                w,
                h,
                source,
                max,
                ..
            } => Widget::Bar {
                x,
                y,
                w,
                h,
                source,
                max,
                command,
            },
            Widget::Line { x, y, w, .. } => Widget::Line { x, y, w, command },
            Widget::Segments {
                x,
                y,
                w,
                h,
                count,
                source,
                max,
                ..
            } => Widget::Segments {
                x,
                y,
                w,
                h,
                count,
                source,
                max,
                command,
            },
            Widget::List {
                x,
                y,
                w,
                rows,
                source,
                ..
            } => Widget::List {
                x,
                y,
                w,
                rows,
                source,
                command,
            },
            Widget::Button {
                x, y, w, h, format, ..
            } => Widget::Button {
                x,
                y,
                w,
                h,
                format,
                command,
            },
            Widget::Bitmap { x, y, name, .. } => Widget::Bitmap {
                x,
                y,
                name,
                command,
            },
            Widget::Brackets {
                x,
                y,
                h,
                len,
                thick,
                ..
            } => Widget::Brackets {
                x,
                y,
                h,
                len,
                thick,
                command,
            },
            Widget::Arrow {
                x,
                y,
                radius,
                source,
                ..
            } => Widget::Arrow {
                x,
                y,
                radius,
                source,
                command,
            },
        }
    }
}

/// One thing the pad's L2/L3/L4 can act on, in the order they are walked.
///
/// A widget with a `command` is one thing. A list is one thing *per item*: its rows are what you move through
/// and its command runs on whichever is chosen. Both kinds are walked in the same order the screen is drawn,
/// because that is what makes the highlight land on the thing the keys moved to rather than on a neighbour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selectable {
    /// Which widget it is, and which of its items when it is a list.
    pub widget: usize,
    /// Which row of a list it is, or `None` when the thing is the widget itself.
    pub item: Option<usize>,
    /// Everything it does, in order: usually one command, sometimes a command and a screen to show after it.
    pub commands: Vec<String>,
    /// What it is called, for the driver's log: the row's text when it is one, or the widget's own written
    /// text. Worked out here because this is where the widget is in hand.
    pub label: String,
}

/// What a widget is called in a log: its own written text where it has one, otherwise its kind.
fn widget_label(widget: &Widget) -> String {
    match widget {
        Widget::Text { format, .. } | Widget::Button { format, .. }
            if !format.trim().is_empty() =>
        {
            format.clone()
        }
        Widget::Text { .. } => "text".to_string(),
        Widget::Button { .. } => "button".to_string(),
        Widget::Bitmap { name, .. } => format!("bitmap {name}"),
        Widget::List { .. } => "list".to_string(),
        Widget::Bar { .. } => "bar".to_string(),
        Widget::Line { .. } => "line".to_string(),
        Widget::Segments { .. } => "segments".to_string(),
        Widget::Brackets { .. } => "brackets".to_string(),
        Widget::Arrow { .. } => "arrow".to_string(),
    }
}

/// Everything on a screen the pad can act on, in drawing order.
pub fn selectables(
    applet: &Applet,
    screen: usize,
    items: &[String],
    values: &Values,
    profile: u32,
) -> Vec<Selectable> {
    let mut found = Vec::new();
    for (index, widget) in screen_widgets(applet, screen).iter().enumerate() {
        let commands = applet.commands(screen, index);
        if commands.is_empty() {
            continue;
        };
        // A widget an alert is *hiding* is not something the pad can be on. The border and the keys have to agree
        // with what is drawn, or a press lands on something that is not there - and a button that only appears
        // when a playlist exists is exactly that kind of widget.
        if let Some(alert) = applet.alert(screen, index)
            && alert.look == AlertLook::Show
            && !g13_values::holds(&alert.condition, values, profile, None)
        {
            continue;
        }
        match widget {
            // a list is walked by its rows, so the border and the command go together - and a list whose rows
            // have not been read yet is not something to be on, because its command would run with no item
            Widget::List { .. } => {
                for (item, label) in items.iter().enumerate() {
                    found.push(Selectable {
                        widget: index,
                        item: Some(item),
                        commands: commands.iter().map(|one| one.to_string()).collect(),
                        label: label.clone(),
                    });
                }
            }
            _ => found.push(Selectable {
                widget: index,
                item: None,
                commands: commands.iter().map(|one| one.to_string()).collect(),
                label: widget_label(widget),
            }),
        }
    }
    found
}

/// Draw an applet into a frame, returning anything that went wrong.
///
/// Widgets are drawn in the order they appear, and each text widget is asked to stay off ink that is already
/// there: that is the ink rule, and it is enforced while drawing rather than checked afterwards.
pub fn draw(applet: &Applet, frame: &mut Frame, values: &Values, problems: &mut Vec<String>) {
    draw_screen_at(applet, 0, Context::at(0.0), frame, values, problems);
}

/// The same, with the clock a running text needs. Seconds since the machine came up is enough: a scroll only
/// has to be steady, and nothing depends on what the number means.
pub fn draw_now(
    applet: &Applet,
    now_seconds: f64,
    frame: &mut Frame,
    values: &Values,
    problems: &mut Vec<String>,
) {
    draw_screen_at(applet, 0, Context::at(now_seconds), frame, values, problems);
}

/// Draw one of an applet's screens. Screen 0 of an applet with no `screens` is the whole applet.
pub fn draw_screen(
    applet: &Applet,
    screen: usize,
    frame: &mut Frame,
    values: &Values,
    problems: &mut Vec<String>,
) {
    draw_screen_at(applet, screen, Context::at(0.0), frame, values, problems);
}

/// Draw one of an applet's screens with the pad's own selection on it.
///
/// The selection is a parameter rather than a value in `values`, and that is the whole point: an applet's values
/// are exactly the ones it declares, so nothing can be fed to it behind its back - which is also why a border
/// read out of that map never appeared. It is the pad's own state, like the recording, and it travels as a
/// parameter the way the recording does.
pub fn draw_screen_at(
    applet: &Applet,
    screen: usize,
    context: Context,
    frame: &mut Frame,
    values: &Values,
    problems: &mut Vec<String>,
) {
    // the two things this drawing asks the context for by their own names, because they are used everywhere
    // The fonts this screen is drawn in: the one it named, in front of the panel's own. In front, because a font
    // that lacks a character - a Latin letter in a rune font, a digit anywhere - must still be drawn: the stack is
    // what makes mixing scripts work, and it is what makes a partly-covered font usable.
    let mut fonts: Vec<g13_screen::font::Font> = Vec::new();
    // the named font first, for the look; then every other font in the folder, for the coverage; then the panel's
    // own, which is what a screen with no fonts at all is drawn in
    if let Some(font) = &applet.font {
        fonts.push(font.clone());
    }
    fonts.extend(applet.fallbacks.iter().cloned());
    fonts.push(g13_screen::font::panel().clone());
    frame.set_fonts(fonts);

    let selected = context.selected;
    let now_seconds = context.now_seconds;

    if applet.border {
        draw_border(frame);
    }
    // What the pad can act on, and which of them it is on. The walk is the same one the driver makes, in the
    // same order the widgets are drawn, so the border below lands on what the keys moved to.
    let items: Vec<String> = screen_widgets(applet, screen)
        .iter()
        .find_map(|widget| match widget {
            Widget::List { source, .. } => Some(
                values
                    .get(source)
                    .map(Value::text)
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(str::to_string)
                    .collect::<Vec<String>>(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    let walk = selectables(applet, screen, &items, values, context.profile);
    let chosen = selected;
    // where each of them was drawn, taken from the drawing itself rather than worked out a second time
    let mut places: Vec<(usize, Option<usize>, usize, usize, usize, usize)> = Vec::new();
    // What each widget's alert says, worked out once per drawing rather than per look: the answer is a question
    // about values that were gathered before this drawing began.
    let mut firing: Vec<bool> = Vec::new();
    for (index, _) in screen_widgets(applet, screen).iter().enumerate() {
        let now = match applet.alert(screen, index) {
            Some(alert) => g13_values::holds(&alert.condition, values, context.profile, None),
            None => false,
        };
        firing.push(now);
    }
    // An alarm flashes fast and at a fixed rate, not on the theme's pulse: an alarm should not be slow because a
    // theme is calm. The lit half is what is drawn.
    let alarm_lit = (now_seconds * 4.0).max(0.0) as usize % 2 == 0;

    for (index, widget) in screen_widgets(applet, screen).iter().enumerate() {
        // A widget that blinks is simply not drawn while the theme's pulse is off. In one bit a blink is ink
        // appearing and disappearing; there is no dimmer to turn down.
        if applet.animate(screen, index) == Some(Animate::Blink) && !applet.theme.lit(now_seconds) {
            continue;
        }
        // `show` is the **widget's own drawing**, shown only while the question is true. An alert belongs to an
        // existing widget and each widget carries one, not several: an applet defines several widgets with
        // different content, one for each alert. So several widgets in one place take turns: one for over 80, one
        // for under 50, and so on - and nothing is drawn over anything.
        if !firing.get(index).copied().unwrap_or(false)
            && applet.alert(screen, index).map(|alert| alert.look) == Some(AlertLook::Show)
        {
            continue;
        }
        // and an alert that flashes is the same idea at a rate it chooses for itself
        if firing.get(index).copied().unwrap_or(false)
            && applet.alert(screen, index).map(|alert| alert.look) == Some(AlertLook::Flash)
            && !alarm_lit
        {
            continue;
        }

        match widget {
            Widget::Text {
                x,
                y,
                align,
                format,
                scroll_width,
                scroll,
                scroll_speed,
                ..
            } => {
                let text = fill(format, values);
                if text.is_empty() {
                    continue;
                }
                // What to draw, how many pixels left of it we are, and whether it is running. A scroll moves
                // by pixels rather than by characters: a whole character a step is exactly the juddering a
                // character-stepped scroll shows. `scroll_speed` is pixels a second.
                // A widget that types itself reveals a character at a time at the theme's speed and then starts
                // again, so the reading is what it always was and only the drawing is patient.
                let text = match applet.animate(screen, index) {
                    Some(Animate::Typewriter) => {
                        let typed = (now_seconds * applet.theme.typewriter).max(0.0) as usize;
                        let cycle = text.chars().count() + 1;
                        text.chars().take(typed % cycle.max(1)).collect::<String>()
                    }
                    _ => text,
                };
                let (text, shift, scrolling) = match scroll_width {
                    Some(box_columns) if *scroll => {
                        let gap = "   ";
                        let run: Vec<char> = format!("{text}{gap}").chars().collect();
                        let span = run.len().max(1) * ADVANCE_COLUMNS;
                        let travelled = (now_seconds * scroll_speed).max(0.0) % span as f64;
                        let whole = (travelled / ADVANCE_COLUMNS as f64) as usize;
                        // the box, plus a character, since the run wraps and the gap has to be fillable
                        let mut showing = String::new();
                        for at in 0..box_columns.saturating_add(2) {
                            showing.push(run[(whole + at) % run.len().max(1)]);
                        }
                        (showing, travelled as usize % ADVANCE_COLUMNS, true)
                    }
                    Some(box_columns) => (fit(&text, *box_columns), 0, false),
                    None => (text, 0, false),
                };
                // in pixels: a character is ADVANCE_COLUMNS wide, not one column of the font
                let width = text.chars().count() * ADVANCE_COLUMNS;
                let start = match align {
                    Align::Left => *x,
                    Align::Right => x.saturating_sub(width),
                    Align::Centre => x.saturating_sub(width / 2),
                } as isize
                    - shift as isize;
                // A running text is stopped at the width it was given: without this it would run across
                // whatever is drawn beside it.
                if scrolling {
                    let box_pixels = scroll_width.unwrap_or(0) * ADVANCE_COLUMNS;
                    frame.clipped(*x, *y, box_pixels, g13_screen::font::LINE_HEIGHT);
                }
                let refused = frame.text_where_blank_from(start, *y, &text);
                if scrolling {
                    frame.unclipped();
                }
                if refused > 0 {
                    problems.push(format!(
                        "text at ({start}, {y}) could not draw {refused} pixel(s): ink was already there"
                    ));
                }
                places.push((
                    index,
                    None,
                    usize::try_from(start.max(0)).unwrap_or(0),
                    *y,
                    width,
                    g13_screen::font::LINE_HEIGHT,
                ));
            }
            Widget::Bar {
                x,
                y,
                w,
                h,
                source,
                max,
                ..
            } => {
                let Some(share) = values.get(source).and_then(Value::number) else {
                    continue;
                };
                let filled = ((share / max.max(0.001)) * *w as f64)
                    .round()
                    .clamp(0.0, *w as f64);
                let mut bar = Frame::new();
                // built at the origin and pasted at the widget's place: filling it at (x, y) and then
                // pasting at (x, y) draws it at twice its position
                bar.fill(0, 0, filled as usize, *h, true);
                let refused = frame.paste_where(&bar, *x, *y);
                if refused > 0 {
                    problems.push(format!(
                        "the bar at ({x}, {y}) could not draw {refused} pixel(s): ink was already there"
                    ));
                }
                places.push((index, None, *x, *y, *w, *h));
            }
            Widget::Line { x, y, w, .. } => {
                frame.fill(*x, *y, *w, 1, true);
                places.push((index, None, *x, *y, *w, 1));
            }
            Widget::Segments {
                x,
                y,
                w,
                h,
                count,
                source,
                max,
                ..
            } => {
                let Some(share) = values.get(source).and_then(Value::number) else {
                    continue;
                };
                let lit = ((share / max.max(0.001)) * *count as f64)
                    .ceil()
                    .clamp(0.0, *count as f64);
                let gap = 1;
                let segments = (*count).max(1);
                let segment = (*w).saturating_sub(gap * segments.saturating_sub(1)) / segments;
                for index in 0..*count {
                    if (index as f64) < lit {
                        frame.fill(*x + index * (segment + gap), *y, segment, *h, true);
                        places.push((index, None, *x, *y, *w, *h));
                    }
                }
            }
            Widget::Bitmap { x, y, name, .. } => {
                let Some(bitmap) = applet.bitmaps.get(name) else {
                    // said rather than drawn as nothing: a picture that is not there is a fault in a file
                    problems.push(format!(
                        "no bitmap called `{name}`: define it in applets/<applet>.bitmaps.json or the shared \
                         bitmaps.json"
                    ));
                    continue;
                };
                let mut picture = Frame::new();
                for (row, pixels) in bitmap.frame_at(now_seconds).iter().enumerate() {
                    for (column, lit) in pixels.iter().enumerate() {
                        if *lit {
                            picture.set(column, row, true);
                        }
                    }
                }
                let refused = frame.paste_where(&picture, *x, *y);
                if refused > 0 {
                    problems.push(format!(
                        "the bitmap {name} at ({x}, {y}) could not draw {refused} pixel(s): ink was already there"
                    ));
                }
                places.push((index, None, *x, *y, bitmap.w, bitmap.h));
            }
            Widget::Button {
                x, y, w, h, format, ..
            } => {
                let label = fill(format, values);
                // Is the pad on this one? A button says so by being **filled in with its words punched out**,
                // not by a border: the box is already a border, so outlining it says nothing, and a border drawn
                // on a border does not read as a selection. The fill inverts the inside, so it goes non-lit
                // letters on a lit background within the button.
                let selected = chosen
                    .and_then(|at| walk.get(at))
                    .map(|thing| thing.widget == index && thing.item.is_none())
                    .unwrap_or(false);
                frame.fill(*x, *y, *w, 1, true);
                frame.fill(*x, y + h.saturating_sub(1), *w, 1, true);
                for row in 0..*h {
                    frame.fill(*x, y + row, 1, 1, true);
                    frame.fill(x + w.saturating_sub(1), y + row, 1, 1, true);
                }
                if *h > 2 && *w > 2 {
                    // the inside of the box, filled when this is the chosen one
                    frame.fill(
                        *x + 1,
                        *y + 1,
                        w.saturating_sub(2),
                        h.saturating_sub(2),
                        selected,
                    );
                    if !label.is_empty() {
                        let room = w.saturating_sub(2);
                        let text = fit(&label, room);
                        // centred in the box, which is what makes a row of buttons look like a row of buttons
                        let width = text.chars().count() * ADVANCE_COLUMNS;
                        let start = x + 1 + room.saturating_sub(width) / 2;
                        let baseline = y + (h.saturating_sub(g13_screen::font::LINE_HEIGHT)) / 2;
                        if selected {
                            // the words are holes in the fill, so they read as letters that are not lit
                            frame.text_unlit(start, baseline, &text);
                        } else {
                            let refused = frame.text_where_blank(start, baseline, &text);
                            if refused > 0 {
                                problems.push(format!(
                                    "the button at ({x}, {y}) could not draw {refused} pixel(s): ink was already there"
                                ));
                            }
                        }
                    }
                }
                // and no place of its own: a button is never outlined, because the fill is its outline
            }
            Widget::List {
                x,
                y,
                w,
                rows,
                source,
                ..
            } => {
                let text = values.get(source).map(Value::text).unwrap_or_default();
                let items: Vec<&str> = text
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .collect();
                // nothing to list is not a list: the check reports a source nobody provides
                if items.is_empty() {
                    continue;
                }
                // Which of its rows the pad is on, when it is on one of them at all: the border says so, and
                // the window follows it, so the row the keys are on is always in view.
                let on_a_row = chosen
                    .and_then(|at| walk.get(at))
                    .filter(|thing| thing.widget == index)
                    .and_then(|thing| thing.item);
                let rows = (*rows).max(1);
                let first = match on_a_row {
                    Some(at) if at >= rows => at + 1 - rows,
                    _ => 0,
                };
                // The pitch follows the lines actually drawn rather than being eight pixels each. A list of Chinese
                // rows is taller per row than a list of Latin ones, and rows written over each other are worse than
                // rows that take more room.
                let mut baseline = *y;
                for (at, item) in items.iter().enumerate().skip(first).take(rows) {
                    let line = fit(item, *w);
                    let height = frame.line_height(&line);
                    frame.text(*x, baseline, &line);
                    if on_a_row == Some(at) {
                        places.push((index, Some(at), *x, baseline, *w, height));
                    }
                    baseline += height;
                }
                // and the whole list is a place of its own when nothing on it is chosen, so a list with a
                // command still gets a border around it
                if on_a_row.is_none() {
                    places.push((index, None, *x, *y, *w, baseline - *y));
                }
            }
            Widget::Brackets {
                x,
                y,
                h,
                len,
                thick,
                ..
            } => {
                for depth in 0..*thick {
                    frame.fill(*x + depth, *y, 1, *h, true);
                    frame.fill(*x + len.saturating_sub(1) - depth, *y, 1, *h, true);
                }
                frame.fill(*x, *y, *len, 1, true);
                frame.fill(*x, y + h.saturating_sub(1), *len, 1, true);
                places.push((index, None, *x, *y, *len, *h));
            }
            Widget::Arrow {
                x,
                y,
                radius,
                source,
                ..
            } => {
                // the source is a heading in degrees: up is zero, turning clockwise
                let Some(heading) = values.get(source).and_then(Value::number) else {
                    continue;
                };
                frame.arrow(*x, *y, *radius, heading);
            }
        }
    }

    // The border the pad's keys put on whatever they are on. Drawn after the widgets and over them, because a
    // highlight that is drawn under the thing it highlights is not a highlight.
    if let Some(thing) = chosen.and_then(|at| walk.get(at)) {
        if let Some((_, _, x, y, w, h)) = places
            .iter()
            .find(|(widget, item, ..)| *widget == thing.widget && *item == thing.item)
        {
            if *w > 0 && *h > 0 {
                frame.fill(*x, *y, *w, 1, true);
                frame.fill(*x, y + h.saturating_sub(1), *w, 1, true);
                for row in 0..*h {
                    frame.fill(*x, y + row, 1, 1, true);
                    frame.fill(x + w.saturating_sub(1), y + row, 1, 1, true);
                }
            }
        }
    }

    // What the alerts ask for, drawn over the places the widgets left. `Invert` needs the widget drawn first,
    // because it turns that drawing inside out rather than drawing something of its own.
    for (index, _, x, y, w, h) in places.iter() {
        if !firing.get(*index).copied().unwrap_or(false) {
            continue;
        }
        let Some(alert) = applet.alert(screen, *index) else {
            continue;
        };
        match alert.look {
            AlertLook::Flash => {}
            AlertLook::Invert => {
                if alarm_lit {
                    frame.invert_rect(*x, *y, *w, *h);
                }
            }
            AlertLook::Border => {
                if !alarm_lit {
                    continue;
                }
                let (left, top) = (x.saturating_sub(1), y.saturating_sub(1));
                let (width, height) = (w.saturating_add(2), h.saturating_add(2));
                frame.fill(left, top, width, 1, true);
                frame.fill(left, top + height - 1, width, 1, true);
                frame.fill(left, top, 1, height, true);
                frame.fill(left + width - 1, top, 1, height, true);
            }
            // `show` has already been answered, where the widget is drawn or not drawn
            AlertLook::Show => {}
        }
    }

    // The theme's boxing, round every widget it can see the place of. Drawn after the widgets and over them,
    // because a frame is meant to sit round its contents rather than beside them.
    match applet.theme.frame {
        ThemeFrame::None => {}
        ThemeFrame::Single | ThemeFrame::Brackets => {
            let brackets = applet.theme.frame == ThemeFrame::Brackets;
            for (_, _, x, y, w, h) in places.iter() {
                // a pixel of margin, so the frame reads as a frame rather than as part of what it holds
                let (left, top) = (x.saturating_sub(1), y.saturating_sub(1));
                let (width, height) = (w.saturating_add(2), h.saturating_add(2));
                let (right, bottom) = (left + width - 1, top + height - 1);
                if !brackets {
                    frame.fill(left, top, width, 1, true);
                    frame.fill(left, bottom, width, 1, true);
                    frame.fill(left, top, 1, height, true);
                    frame.fill(right, top, 1, height, true);
                } else {
                    // four corners, a few pixels each: the shape a heads-up display boxes things with
                    let arm = width.clamp(2, 4);
                    let side = height.clamp(2, 4);
                    for at in 0..arm {
                        frame.set(left + at, top, true);
                        frame.set(right.saturating_sub(at), top, true);
                        frame.set(left + at, bottom, true);
                        frame.set(right.saturating_sub(at), bottom, true);
                    }
                    for at in 0..side {
                        frame.set(left, top + at, true);
                        frame.set(right, top + at, true);
                        frame.set(left, bottom.saturating_sub(at), true);
                        frame.set(right, bottom.saturating_sub(at), true);
                    }
                }
            }
        }
    }

    // The theme's own two effects, drawn last and over everything: a scanline *is* a line across the picture,
    // so unlike a widget it is meant to cross what is already there.
    if let Some(speed) = applet.theme.scanline {
        let row = ((now_seconds * speed).max(0.0) as usize) % (g13_screen::VISIBLE_HEIGHT + 1);
        frame.fill(0, row, g13_screen::WIDTH, 1, true);
    }
    let shake = applet.theme.shake(now_seconds);
    if shake != 0 {
        frame.shift(shake);
    }
}

/// Fit text into a number of characters by taking the end, so `{line}` still shows the message it names.
fn fit(text: &str, width: usize) -> String {
    let characters: Vec<char> = text.chars().collect();
    if characters.len() <= width {
        return text.to_string();
    }
    characters[characters.len() - width..].iter().collect()
}

/// Draw the one-pixel frame an applet asks for with `border`, inside the edges of the visible panel.
fn draw_border(frame: &mut Frame) {
    let (width, height) = (g13_screen::WIDTH, g13_screen::VISIBLE_HEIGHT);
    frame.fill(0, 0, width, 1, true);
    frame.fill(0, height - 1, width, 1, true);
    frame.fill(0, 0, 1, height, true);
    frame.fill(width - 1, 0, 1, height, true);
}

/// How many rows of text fit on the panel.
pub fn lines_available() -> usize {
    TEXT_ROWS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many pixels of a frame are ink, inside a box. For asking "was more drawn here".
    fn count_ink(frame: &Frame, x: usize, y: usize, w: usize, h: usize) -> usize {
        let mut count = 0;
        for at in x..(x + w) {
            for row in y..(y + h) {
                if frame.get(at, row) {
                    count += 1;
                }
            }
        }
        count
    }

    fn applet(text: &str) -> Applet {
        parse_applet(text).expect("should parse")
    }

    #[test]
    fn a_bitmaps_file_beside_an_applet_is_not_an_applet() {
        // A bitmaps file beside an applet is not an applet: `<name>.bitmaps.json` was being listed as an applet
        // of its own, named after the applet with `.bitmaps` appended, when it only holds bitmaps.
        let dir = std::env::temp_dir().join(format!("g13-applet-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audio.json"), r#"{"name": "audio"}"#).unwrap();
        std::fs::write(dir.join("audio.bitmaps.json"), "{}").unwrap();
        std::fs::write(dir.join("gpu.json"), r#"{"name": "gpu"}"#).unwrap();
        std::fs::write(dir.join("notes.txt"), "not an applet").unwrap();

        assert_eq!(applet_names(&dir), vec!["audio", "gpu"]);
        assert!(is_applet_file(&dir.join("audio.json")));
        assert!(!is_applet_file(&dir.join("audio.bitmaps.json")));
        assert!(!is_applet_file(&dir.join("notes.txt")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bitmap_comes_from_the_shared_file_and_an_applets_own_wins() {
        // One source of truth per level: the shared file every applet may use, and the applet's own on top of it,
        // by name.
        let dir = std::env::temp_dir().join(format!("g13-bitmaps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("bitmaps.json"),
            "{\"play\": {\"rows\": [\"#.#\", \".#.\"]}, \"stop\": {\"rows\": [\"###\"]}}",
        )
        .unwrap();
        std::fs::write(
            dir.join("applets/mine.bitmaps.json"),
            "{\"play\": {\"rows\": [\"##\", \"##\"]}, \"broken\": {\"rows\": \"not rows\"}}",
        )
        .unwrap();

        let (found, complaints) = read_bitmaps(&dir.join("applets"), "mine");
        assert_eq!(found["play"].w, 2, "the applet's own play must win");
        assert_eq!(found["play"].h, 2);
        assert_eq!(found["stop"].w, 3, "the shared one is still there");
        assert_eq!(
            complaints.len(),
            1,
            "a bitmap that will not parse must be said: {complaints:?}"
        );
        assert!(complaints[0].contains("broken"), "{complaints:?}");
        // and another applet sees only the shared one
        let (other, _) = read_bitmaps(&dir.join("applets"), "other");
        assert_eq!(other["play"].w, 3, "it is a triangle for everyone else");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An applet with a theme from a folder, the way the driver loads one.
    /// `look` names the theme file, so each caller gets its own: these tests run in parallel, and two of them
    /// writing different theme text to one `look.json` made a passing test fail depending on the order.
    fn themed(look: &str, applet_text: &str, theme_text: &str) -> Applet {
        let dir = std::env::temp_dir().join("g13-themed-applet");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join(format!("{look}.json")), theme_text).unwrap();
        with_theme(applet(&applet_text.replace("THEME", look)), &dir)
    }

    /// An applet with one widget carrying an alert, and the values that answer its question.
    fn alerted(alert: &str, cpu: f64) -> (Applet, Values) {
        let applet = applet(&format!(
            r#"{{"name": "alarmed", "sources": {{"cpu": "cpu"}}, "widgets": [
                 {{"type": "text", "x": 2, "y": 2, "format": "CPU", "alert": {alert}}}]}}"#
        ));
        let mut values = Values::new();
        values.insert("cpu".to_string(), Value::Number(cpu));
        (applet, values)
    }

    #[test]
    fn an_alert_is_a_macro_condition_and_flashes_only_while_the_answer_is_yes() {
        // The question an alert asks is the macros' own language, asked by the macros' own evaluator, so there is
        // one way to say "when is this true" in this program.
        let alert = r#"{"when": {"value": "cpu", "op": ">", "to": 80}, "look": "flash"}"#;
        let (hot, hot_values) = alerted(alert, 92.0);
        let (cool, cool_values) = alerted(alert, 10.0);
        assert!(
            hot.alert(0, 0).is_some(),
            "the alert should be on the widget"
        );
        assert!(
            hot.moves(0),
            "a widget that can flash is motion, like a scroll or an animation"
        );

        let ink_at = |applet: &Applet, values: &Values, now: f64| {
            let mut frame = Frame::new();
            draw_now(applet, now, &mut frame, values, &mut Vec::new());
            (0..g13_screen::WIDTH)
                .flat_map(|x| (0..8).map(move |y| (x, y)))
                .filter(|(x, y)| frame.get(*x, *y))
                .count()
        };
        // while it is true the widget is drawn in the lit half of the alarm's own fast rate and not in the dark
        assert!(
            ink_at(&hot, &hot_values, 0.0) > 0,
            "the lit half should show it"
        );
        assert_eq!(
            ink_at(&hot, &hot_values, 0.25),
            0,
            "the dark half should not"
        );
        // and while it is false the widget is simply drawn, always
        assert!(ink_at(&cool, &cool_values, 0.0) > 0);
        assert!(
            ink_at(&cool, &cool_values, 0.25) > 0,
            "nothing to flash about"
        );
    }

    #[test]
    fn the_other_three_looks_invert_box_or_draw_a_picture() {
        let alert = |look: &str, extra: &str| {
            format!(
                r#"{{"when": {{"value": "cpu", "op": ">", "to": 80}}, "look": "{look}"{extra}}}"#
            )
        };
        let ink = |applet: &Applet, values: &Values, now: f64| {
            let mut frame = Frame::new();
            draw_now(applet, now, &mut frame, values, &mut Vec::new());
            (0..g13_screen::WIDTH)
                .flat_map(|x| (0..g13_screen::VISIBLE_HEIGHT).map(move |y| (x, y)))
                .filter(|(x, y)| frame.get(*x, *y))
                .count()
        };

        // invert: a filled box with the words punched out
        let (inverted, hot) = alerted(&alert("invert", ""), 92.0);
        let (plain, cool) = alerted(&alert("invert", ""), 10.0);
        // Counted pixels cannot tell a box from its complement - both have the same amount of ink to one bit - so
        // the question is whether the picture changed at all. The two applets differ in the answer to their own
        // question and in nothing else, and both have the same look.
        let picture = |applet: &Applet, values: &Values, now: f64| {
            let mut frame = Frame::new();
            let mut said = Vec::new();
            draw_now(applet, now, &mut frame, values, &mut said);
            (frame, said)
        };
        let (words, words_said) = picture(&plain, &cool, 0.0);
        let (flipped, said) = picture(&inverted, &hot, 0.0);
        assert!(
            words_said.is_empty() && said.is_empty(),
            "nothing should be complained about"
        );
        assert_ne!(
            words, flipped,
            "a true alert with `look: invert` should turn the widget inside out"
        );
        // and while the alarm is in its dark half the widget is left exactly as it was
        let (dark, _) = picture(&inverted, &hot, 0.25);
        assert_eq!(
            dark, words,
            "the dark half of the flash should leave it as it was"
        );

        // border: a frame round the widget, flashing with the alarm
        let (bordered, hot) = alerted(&alert("border", ""), 92.0);
        assert!(
            ink(&bordered, &hot, 0.0) > ink(&plain, &cool, 0.0),
            "the frame sits round the widget"
        );
        assert_eq!(
            ink(&bordered, &hot, 0.25),
            ink(&plain, &cool, 0.25),
            "the frame is dark in the dark half of the flash"
        );

        // show: the widget's own drawing, only while the question is true. Several widgets in one place take
        // turns this way, which is how an applet gives each of its alerts its own content.
        let (shown, hot) = alerted(&alert("show", ""), 92.0);
        let (hidden, cool) = alerted(&alert("show", ""), 10.0);
        assert!(
            ink(&shown, &hot, 0.0) > 0,
            "the widget should be drawn while the question is true"
        );
        assert_eq!(
            ink(&hidden, &cool, 0.0),
            0,
            "and not at all while it is false, so the next widget in that place shows instead"
        );
    }

    #[test]
    fn a_key_an_alert_no_longer_takes_is_said_rather_than_swallowed() {
        // The shape of an alert still on disk from when `show` drew a picture, with the picture named. It does
        // nothing now, and a key that does nothing silently is the kind of thing that wastes an afternoon.
        let applet = applet(
            r#"{"name": "a", "sources": {}, "widgets": [
                 {"type": "bitmap", "x": 0, "y": 0, "name": "Play",
                  "alert": {"when": {"profile": 0}, "look": "flash", "bitmap": "l"}}]}"#,
        );
        assert!(
            applet.alert(0, 0).is_some(),
            "the alert itself is still a real one"
        );
        assert!(
            applet
                .unhandled
                .iter()
                .any(|complaint| complaint.contains("bitmap")),
            "the picture an alert no longer takes should be said: {:?}",
            applet.unhandled
        );
    }

    #[test]
    fn an_alert_that_will_not_read_says_so_rather_than_doing_nothing() {
        // a question with no answer in it: a comparison with nothing to compare against
        let broken = applet(
            r#"{"name": "b", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "x", "alert": {"when": {"value": "cpu"}}},
                 {"type": "text", "x": 0, "y": 10, "format": "y", "alert": {"when": {"profile": 2}, "look": "show"}}]}"#,
        );
        assert!(
            broken
                .unhandled
                .iter()
                .any(|complaint| complaint.contains("alert")),
            "a condition with nothing to compare should be said: {:?}",
            broken.unhandled
        );
        assert!(
            broken.alert(0, 1).is_some(),
            "the second one is a real alert"
        );
    }

    #[test]
    fn a_bitmap_with_frames_is_an_animation_and_a_plain_one_is_still_a_picture() {
        // "would there theorhetically be a way to add custom gif style animations that could be placed? like a
        // bitmap" - this is that, and nothing already drawn changes: no `frames` is one frame
        let animated = Bitmap::from_value(&serde_json::json!({
            "w": 4, "h": 2, "ms": 100,
            "frames": [
                {"rows": ["##..", "..##"]},
                {"rows": ["..##", "##.."]},
            ]
        }))
        .expect("a two frame animation");
        assert_eq!(animated.w, 4);
        assert_eq!(animated.h, 2);
        assert_eq!(animated.ms, 100.0);
        assert!(animated.animated());
        assert_eq!(animated.frames.len(), 2);

        let still = Bitmap::from_value(&serde_json::json!({"rows": ["##", ".."]})).unwrap();
        assert!(!still.animated(), "no frames is a still picture");
        assert_eq!(still.frames.len(), 1);
        assert_eq!(still.w, 2);
        assert_eq!(still.h, 2);

        // every frame is the same size, or the animation would jump as it turned over
        let ragged = Bitmap::from_value(&serde_json::json!({
            "w": 4, "h": 2, "frames": [{"rows": ["####", "####"]}, {"rows": ["##", "##"]}]
        }));
        assert!(ragged.unwrap_err().contains("frame 1"));
        let empty = Bitmap::from_value(&serde_json::json!({"frames": []}));
        assert!(empty.unwrap_err().contains("at least one frame"));
    }

    #[test]
    fn a_widget_an_alert_is_hiding_is_not_something_the_pad_can_be_on() {
        // The case this guards: a playlist button that appears only when a playlist can be detected. If the walk
        // includes it while it is hidden, the pad's border and its keys disagree with what is drawn, and a press
        // lands on a button that is not there.
        let applet = applet(
            r#"{"name": "playlist", "sources": {"tracks": "tracks"}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "PLAYING"},
                 {"type": "button", "x": 0, "y": 20, "w": 60, "h": 9, "format": "PLAYLIST",
                  "command": "screen:2",
                  "alert": {"when": {"value": "tracks", "op": "contains", "text": "1"},
                            "look": "show"}}]}"#,
        );
        let with_tracks = |count: &str| {
            let mut values = Values::new();
            values.insert("tracks".to_string(), Value::Text(count.to_string()));
            values
        };
        // nothing to list: the button is not drawn, so it is not walkable either
        let none = selectables(&applet, 0, &[], &with_tracks(""), 0);
        assert_eq!(
            none.len(),
            0,
            "a hidden button should not be on the walk: {none:?}"
        );
        // and with a playlist, it is there and it is on the walk
        let some = selectables(&applet, 0, &[], &with_tracks("1"), 0);
        assert_eq!(some.len(), 1, "the button should be on the walk: {some:?}");
        assert_eq!(some[0].commands, vec!["screen:2".to_string()]);
    }

    #[test]
    fn the_speed_in_the_file_is_the_speed_it_runs_at_even_when_it_is_slow() {
        // The drawing used to take the speed to a whole number of pixels a frame, so anything under ten a second
        // ran at twenty - a speed reported as exact while running far faster. One pixel a second over twenty
        // drawings a second is a twentieth of a pixel a frame, so the text does not move at all in one frame, and
        // the file means what it says.
        let applet = applet(
            r#"{"name": "slow", "sources": {}, "widgets": [
                 {"type": "text", "x": 100, "y": 0, "format": "a long title that has to run",
                  "scroll_width": 12, "scroll": true, "scroll_speed": 1}]}"#,
        );
        let drawn = |now: f64| {
            let mut frame = Frame::new();
            draw_now(&applet, now, &mut frame, &Values::new(), &mut Vec::new());
            frame
        };
        assert_eq!(
            drawn(0.0),
            drawn(PANEL_FRAME_SECONDS),
            "one pixel a second should not move the text in a twentieth of a second"
        );
        // and over a whole second it has moved a pixel, which is what one a second means
        assert_ne!(
            drawn(0.0),
            drawn(1.0),
            "a second at one pixel a second is a pixel"
        );
    }

    #[test]
    fn a_screen_with_an_animation_on_it_is_drawn_at_the_pads_own_rate_and_written_back_in_its_own_shape()
     {
        let dir = std::env::temp_dir().join("g13-animated-bitmap");
        let _ = std::fs::create_dir_all(&dir);
        // built rather than written as a literal: a bitmap's rows are `#` characters, and Rust 2024 reserves a
        // closing `"##`, so no raw string can hold one
        let on_disk = serde_json::json!({
            "walk": {"w": 2, "h": 1, "frames": [{"rows": ["##"]}, {"rows": [".."]}]}
        });
        std::fs::write(dir.join("a.bitmaps.json"), on_disk.to_string()).unwrap();
        let applet = with_bitmaps(
            applet(
                r#"{"name": "a", "sources": {}, "widgets": [{"type": "bitmap", "x": 0, "y": 0, "name": "walk"}]}"#,
            ),
            &dir,
        );
        assert!(
            applet.moves(0),
            "an animation is motion, like a scroll or a blink"
        );
        // and it draws different pictures at different moments
        let ink_at = |now: f64| {
            let mut frame = Frame::new();
            draw_now(&applet, now, &mut frame, &Values::new(), &mut Vec::new());
            frame.get(0, 0)
        };
        assert!(ink_at(0.0));
        assert_ne!(
            ink_at(0.0),
            ink_at(0.15),
            "the animation should move with the clock"
        );

        // writing back: an animation keeps `frames`, a still picture keeps `rows`, so a file written back reads
        // the same
        let one = Bitmap::from_value(&serde_json::json!({"rows": ["##"]})).unwrap();
        let mut both = BTreeMap::new();
        both.insert(
            "walk".to_string(),
            applet.bitmaps.get("walk").unwrap().clone(),
        );
        both.insert("still".to_string(), one);
        write_bitmaps(&dir, "a", &both).expect("written");
        let back: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("a.bitmaps.json")).unwrap())
                .unwrap();
        assert!(
            back["walk"].get("frames").is_some(),
            "an animation stays an animation"
        );
        assert!(back["walk"].get("rows").is_none());
        assert!(
            back["still"].get("rows").is_some(),
            "a still bitmap is written as it always was"
        );
        assert!(back["still"].get("frames").is_none());
    }

    #[test]
    fn a_theme_boxes_the_widgets_and_brackets_only_the_corners() {
        // `frame` was read from the file and not drawn for a day, which the docs claimed and the code did not do
        let bar = |look: &str, frame: &str| {
            let dir = std::env::temp_dir().join("g13-frame-test");
            let _ = std::fs::create_dir_all(&dir);
            std::fs::write(dir.join(format!("{look}.json")), frame).unwrap();
            let applet = with_theme(
                applet(&format!(
                    r#"{{"name": "f", "sources": {{"n": "cmd:echo 5"}}, "theme": "{look}", "widgets": [
                         {{"type": "bar", "x": 20, "y": 20, "w": 30, "h": 6, "source": "n", "max": 10}}]}}"#
                )),
                &dir,
            );
            let mut values = Values::new();
            values.insert("n".to_string(), Value::Number(5.0));
            let mut frame = Frame::new();
            draw_now(&applet, 1.0, &mut frame, &values, &mut Vec::new());
            frame
        };
        let plain = bar("bare", r#"{}"#);
        let boxed = bar("boxed", r#"{"frame": "single"}"#);
        let bracketed = bar("bracketed", r#"{"frame": "brackets"}"#);

        // the widget's own ink is in all three, so boxing adds rather than replaces
        assert!(plain.get(20, 20) && boxed.get(20, 20) && bracketed.get(20, 20));
        // the frame is round the widget, with a pixel of margin
        for (x, y) in [(19, 19), (50, 19), (19, 26), (50, 26)] {
            assert!(boxed.get(x, y), "a single frame should reach ({x}, {y})");
            assert!(bracketed.get(x, y), "brackets still have their corners");
        }
        // and brackets are corners only: the middle of an edge is empty
        assert!(
            !bracketed.get(34, 19),
            "brackets should not draw a whole top edge"
        );
        assert!(boxed.get(34, 19), "a single frame draws the whole edge");
        // nothing extra for no frame at all
        assert!(!plain.get(19, 19) && !plain.get(34, 19));
    }

    #[test]
    fn a_theme_that_names_a_scanline_makes_the_screen_move() {
        let running = themed(
            "scan",
            r#"{"name": "t", "sources": {}, "theme": "THEME", "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "still"}]}"#,
            r#"{"scanline": 60}"#,
        );
        assert!(
            running.moves(0),
            "a scanline is drawn at the fast rate: the picture moves even though no widget does \
             (theme {:?}, complaints {:?})",
            running.theme_name,
            running.unhandled
        );
        // and the line is somewhere, moving with the clock
        let row_at = |now: f64| {
            let mut frame = Frame::new();
            draw_now(&running, now, &mut frame, &Values::new(), &mut Vec::new());
            (0..g13_screen::VISIBLE_HEIGHT)
                .find(|row| (0..g13_screen::WIDTH).all(|x| frame.get(x, *row)))
        };
        let first = row_at(0.0);
        let later = row_at(1.0);
        assert!(first.is_some(), "no scanline was drawn");
        assert_ne!(first, later, "the scanline is standing still");
    }

    #[test]
    fn a_widget_that_blinks_is_absent_while_the_pulse_is_off_and_present_when_it_is_on() {
        let blinking = themed(
            "blink",
            r#"{"name": "b", "sources": {}, "theme": "THEME", "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "ALERT", "animate": "blink"}]}"#,
            r#"{"pulse": {"on": 0.5, "off": 0.5}}"#,
        );
        assert!(blinking.moves(0), "a blink needs the fast rate too");
        let ink_at = |now: f64| {
            let mut frame = Frame::new();
            draw_now(&blinking, now, &mut frame, &Values::new(), &mut Vec::new());
            (0..g13_screen::WIDTH).any(|x| (0..8).any(|y| frame.get(x, y)))
        };
        assert!(
            ink_at(0.1),
            "the lit half of the pulse should show the words"
        );
        assert!(!ink_at(0.7), "the dark half should show nothing at all");
    }

    #[test]
    fn a_widget_that_types_itself_reveals_more_of_its_words_as_it_goes() {
        let typing = themed(
            "type",
            r#"{"name": "y", "sources": {}, "theme": "THEME", "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "PLAYING NOW", "animate": "typewriter"}]}"#,
            r#"{"typewriter": 4}"#,
        );
        let ink_count = |now: f64| {
            let mut frame = Frame::new();
            draw_now(&typing, now, &mut frame, &Values::new(), &mut Vec::new());
            (0..g13_screen::WIDTH)
                .flat_map(|x| (0..8).map(move |y| (x, y)))
                .filter(|(x, y)| frame.get(*x, *y))
                .count()
        };
        let none = ink_count(0.0);
        let some = ink_count(0.6);
        let more = ink_count(1.4);
        assert_eq!(none, 0, "nothing is revealed at the start");
        assert!(
            some > 0 && more > some,
            "it should reveal more as it types: {none} {some} {more}"
        );
    }

    #[test]
    fn only_the_values_that_move_with_the_pad_are_read_again() {
        // the slow source is a file this test rewrites, so "was it read again?" has an answer that is not a
        // guess: the kept value must come back while the fast one is the new one.
        let dir = std::env::temp_dir().join("g13-refresh-fast-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("slow.txt");
        std::fs::write(&path, "first").unwrap();
        let published = dir.join("g13-values.json");
        std::fs::write(&published, r#"{"stick_x": 1.0}"#).unwrap();
        let mix = applet(
            r#"{"name": "mix", "sources": {"slow": "file:SLOW", "fast": "stick_x"}, "widgets": []}"#
                .replace("SLOW", &path.display().to_string())
                .as_str(),
        );
        let world = g13_sources::World::default().with_values(&published);
        let (kept, problems) = gather(&mix, &world);
        assert!(problems.is_empty(), "{problems:?}");
        // the file changes, the pad moves, and only the fast name is asked for again
        std::fs::write(&path, "second").unwrap();
        std::fs::write(&published, r#"{"stick_x": 2.0}"#).unwrap();
        let world = g13_sources::World::default().with_values(&published);
        let (refreshed, problems) = refresh_fast(&mix, &kept, &world).expect("a fast source");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            refreshed.get("slow").map(|v| v.text()).unwrap_or_default(),
            "first",
            "the slow source was read again, which is the whole cost this avoids"
        );
        assert_eq!(
            refreshed.get("fast").map(|v| v.text()).unwrap_or_default(),
            "2",
            "the value that moves with the pad was not read again, so the pad would lag"
        );
        // and an applet with nothing fast on it is not re-read at all
        let still = applet(
            r#"{"name": "still", "sources": {"slow": "file:SLOW"}, "widgets": []}"#
                .replace("SLOW", &path.display().to_string())
                .as_str(),
        );
        assert!(refresh_fast(&still, &kept, &world).is_none());
    }

    #[test]
    fn a_widget_knows_whether_it_moves_by_itself() {
        // the draw rate asks this rather than keeping a list of kinds, so a new moving widget says so in one place
        let running = applet(
            r#"{"name": "r", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "a long title", "scroll_width": 10, "scroll": true}]}"#,
        );
        let cut = applet(
            r#"{"name": "c", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "a long title", "scroll_width": 10}]}"#,
        );
        assert!(running.widgets.iter().any(Widget::scrolls));
        assert!(!cut.widgets.iter().any(Widget::scrolls));
    }

    #[test]
    fn a_scrolling_text_runs_through_its_width_instead_of_being_cut_off() {
        // Text that is too long used to be cut off: the option beside scroll width marquees the text within that
        // scroll width rather than cutting it off. `scroll` and `scroll_speed` are the previous stack's own keys,
        // which this build now acts on.
        let applet = applet(
            r#"{"name": "media", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "w": 20, "format": "a very long track title indeed",
                  "scroll_width": 10, "scroll": true, "scroll_speed": 4}]}"#,
        );
        let ink_at = |now: f64| {
            let mut frame = Frame::new();
            draw_now(&applet, now, &mut frame, &Values::new(), &mut Vec::new());
            // what is on the first line, as characters: enough to see it has moved
            let mut seen = String::new();
            for column in 0..(10 * ADVANCE_COLUMNS) {
                seen.push(if frame.get(column, 0) { '#' } else { '.' });
            }
            seen
        };
        let start = ink_at(0.0);
        assert!(start.contains('#'), "nothing was drawn");
        // a second later it has moved on by about four characters, and none of the width is empty
        let later = ink_at(1.0);
        assert_ne!(start, later, "the text did not run: it is standing still");
        assert!(
            later.contains('#') && later.matches('.').count() < later.len(),
            "the running text left the width blank: {later}"
        );
        // and it comes round: the same clock as the start is the same picture
        assert_eq!(ink_at(0.0), start);
    }

    #[test]
    fn a_bitmap_widget_draws_its_picture_and_a_name_nothing_defines_is_said() {
        let applet = applet(
            r#"{"name": "audio", "sources": {}, "widgets": [
                 {"type": "bitmap", "x": 5, "y": 5, "name": "play", "command": "cmd:playerctl play-pause"},
                 {"type": "bitmap", "x": 5, "y": 20, "name": "missing"}]}"#,
        );
        // the pictures an applet draws are on it, resolved when it is loaded
        let mut with = applet.clone();
        let mut pictures = std::collections::BTreeMap::new();
        pictures.insert(
            "play".to_string(),
            Bitmap {
                w: 2,
                h: 2,
                ms: 120.0,
                frames: vec![vec![vec![true, true], vec![true, false]]],
            },
        );
        with.bitmaps = pictures;

        let mut frame = Frame::new();
        let mut problems = Vec::new();
        draw(&with, &mut frame, &Values::new(), &mut problems);
        assert!(frame.get(5, 5) && frame.get(6, 5) && frame.get(5, 6));
        assert!(!frame.get(6, 6), "not every pixel of the picture is lit");
        assert!(
            problems.iter().any(|problem| problem.contains("missing")),
            "a bitmap nobody defined is not reported: {problems:?}"
        );
        // and it is something the pad can be on, named by its picture
        let walk = selectables(&with, 0, &[], &Values::new(), 0);
        assert_eq!(walk.len(), 1);
        assert_eq!(walk[0].label, "bitmap play");
        let _ = applet;
    }

    #[test]
    fn a_button_is_a_box_with_a_label_and_the_pad_can_land_on_it() {
        // What an audio applet is made of: four of these instead of four text widgets that happen to carry
        // commands. The box is what says "press me", and the label's names are read like any text widget's.
        let applet = applet(
            r#"{"name": "audio", "sources": {}, "widgets": [
                 {"type": "button", "x": 4, "y": 2, "w": 60, "h": 12, "format": "Start/Pause",
                  "command": "cmd:playerctl play-pause"},
                 {"type": "button", "x": 4, "y": 20, "w": 60, "h": 12, "format": "{media_title}"}]}"#,
        );
        assert!(matches!(
            applet.widgets[0],
            Widget::Button { w: 60, h: 12, .. }
        ));
        // both the label and the box are drawn: the frame is ink at its corners
        let mut frame = Frame::new();
        draw(&applet, &mut frame, &Values::new(), &mut Vec::new());
        assert!(frame.get(4, 2), "no box around the button");
        assert!(
            frame.get(4 + 60 - 1, 2 + 12 - 1),
            "the box does not close on its far corner"
        );
        // a button with a command is something the pad can be on, and it is named by its label
        let walk = selectables(&applet, 0, &[], &Values::new(), 0);
        assert_eq!(
            walk.len(),
            1,
            "only the button with a command can be pressed"
        );
        assert_eq!(walk[0].label, "Start/Pause");
        assert_eq!(
            walk[0].commands,
            vec!["cmd:playerctl play-pause".to_string()]
        );
        // and the second button's label is a reading, said like a text widget's would be
        assert_eq!(names_read(&applet), vec!["media_title"]);

        // The chosen one is filled in with its words punched out, not outlined: a button is already a box, so a
        // border round it says nothing. The fill gives non-lit letters on a lit background.
        let draw_with = |selected: Option<usize>| {
            let mut frame = Frame::new();
            draw_screen_at(
                &applet,
                0,
                Context {
                    selected,
                    // no clock in a test: the scroll starts at its beginning
                    now_seconds: 0.0,
                    profile: 0,
                },
                &mut frame,
                &Values::new(),
                &mut Vec::new(),
            );
            frame
        };
        let plain = draw_with(None);
        let chosen = draw_with(Some(0));
        // the inside of the first button: lit when chosen, and the middle of it is a hole where a letter is
        let inside = (4 + 60 / 2, 2 + 12 / 2);
        assert!(
            !plain.get(inside.0, inside.1) || plain != chosen,
            "the chosen button was not filled in"
        );
        assert!(
            count_ink(&chosen, 4, 2, 60, 12) > count_ink(&plain, 4, 2, 60, 12),
            "the chosen button has no more ink in it than the plain one"
        );
        // and no outline: the band just outside the box has the same ink with it chosen as without, so nothing
        // was drawn round it on top of the fill
        let band = |frame: &Frame| {
            count_ink(frame, 3, 1, 1, 14)
                + count_ink(frame, 4 + 60, 1, 1, 14)
                + count_ink(frame, 4, 1, 60, 1)
                + count_ink(frame, 4, 2 + 12, 60, 1)
        };
        assert_eq!(
            band(&chosen),
            0,
            "something was drawn round the chosen button as well as filling it - a border round a box that is\n             already one says nothing, which is why it is filled instead"
        );
    }

    #[test]
    fn what_an_applet_reads_covers_every_screen_and_a_lists_source() {
        // Sources used only inside a screen were not shown as read, even though they are directly used in the
        // widgets below. The rule walked only the applet's own `widgets`, so an applet whose readings are on a
        // screen read as reading nothing.
        let many = applet(
            r#"{"name": "stats", "sources": {}, "screens": [
                 {"title": "one", "widgets": [{"type": "text", "x": 0, "y": 0, "format": "{cpu}C"}]},
                 {"title": "two", "widgets": [
                    {"type": "list", "x": 0, "y": 0, "w": 20, "source": "containers"},
                    {"type": "bar", "x": 0, "y": 10, "w": 40, "h": 4, "source": "mem"}]}]}"#,
        );
        assert_eq!(
            names_read(&many),
            vec!["cpu", "containers", "mem"],
            "a screen's widgets, and a list's source, are things the applet reads"
        );
        // and the same rule decides what nothing provides, so the pad and the panel cannot disagree
        let values = Values::new();
        assert_eq!(
            values_nothing_provides(&many, &values),
            vec!["cpu", "containers", "mem"]
        );

        let one = applet(
            r#"{"name": "x", "widgets": [{"type": "text", "x": 0, "y": 0, "format": "{cpu}"}]}"#,
        );
        let mut provided = Values::new();
        provided.insert("cpu".to_string(), Value::Number(1.0));
        assert!(values_nothing_provides(&one, &provided).is_empty());
        assert_eq!(names_read(&one), vec!["cpu"]);
    }

    #[test]
    fn the_walk_is_the_things_with_a_command_in_the_order_they_are_drawn() {
        // Four buttons and nothing else: this is the case the whole thing was built for, and there is no list
        // anywhere in it.
        let buttons = applet(
            r#"{"name": "audio", "sources": {}, "widgets": [
                 {"type": "text", "x": 4, "y": 2, "format": "Start/Pause", "command": "cmd:playerctl play-pause"},
                 {"type": "text", "x": 4, "y": 10, "format": "Previous", "command": "cmd:playerctl previous"},
                 {"type": "line", "x": 4, "y": 18, "w": 100},
                 {"type": "text", "x": 4, "y": 20, "format": "Next", "command": "cmd:playerctl next"},
                 {"type": "text", "x": 4, "y": 30, "format": "Stop", "command": "cmd:playerctl stop"}]}"#,
        );
        let walk = selectables(&buttons, 0, &[], &Values::new(), 0);
        assert_eq!(walk.len(), 4, "the line is not something you can act on");
        assert_eq!(
            walk.iter().map(|thing| thing.widget).collect::<Vec<_>>(),
            vec![0, 1, 3, 4],
            "the walk must be in the order the screen draws them"
        );
        assert_eq!(walk[0].label, "Start/Pause");
        assert_eq!(
            walk[0].commands,
            vec!["cmd:playerctl play-pause".to_string()]
        );
        assert!(walk[0].item.is_none(), "a button is not a row of a list");

        // and a list is one thing per row, with each row's text for the command to use
        let listing = applet(
            r#"{"name": "docker", "sources": {}, "widgets": [
                 {"type": "list", "x": 0, "y": 0, "w": 20, "rows": 3, "source": "containers",
                  "command": "cmd:docker restart {screen_item}"}]}"#,
        );
        let items = vec!["web-1".to_string(), "web-2".to_string()];
        let walk = selectables(&listing, 0, &items, &Values::new(), 0);
        assert_eq!(walk.len(), 2);
        assert_eq!(walk[1].item, Some(1));
        assert_eq!(walk[1].label, "web-2");

        // a list with nothing read yet is not walked by rows that are not there
        assert!(selectables(&listing, 0, &[], &Values::new(), 0).is_empty());
        // and a widget with no command is not something the pad can be on at all
        let plain = applet(
            r#"{"name": "x", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "just a reading"}]}"#,
        );
        assert!(selectables(&plain, 0, &[], &Values::new(), 0).is_empty());
    }

    #[test]
    fn the_chosen_thing_is_drawn_with_a_border_around_it() {
        let applet = applet(
            r#"{"name": "audio", "sources": {}, "widgets": [
                 {"type": "text", "x": 4, "y": 2, "format": "Start", "command": "cmd:one"},
                 {"type": "text", "x": 4, "y": 20, "format": "Stop", "command": "cmd:two"}]}"#,
        );
        let draw_with = |selected: Option<usize>| {
            let values = Values::new();
            let mut frame = Frame::new();
            draw_screen_at(
                &applet,
                0,
                Context {
                    selected,
                    now_seconds: 0.0,
                    profile: 0,
                },
                &mut frame,
                &values,
                &mut Vec::new(),
            );
            frame
        };
        let first = draw_with(Some(0));
        let second = draw_with(Some(1));
        assert!(
            !first.is_blank(0, 0, g13_screen::WIDTH, g13_screen::VISIBLE_HEIGHT),
            "nothing was drawn at all"
        );
        assert_ne!(
            first, second,
            "the border did not move when the pad moved to the second thing"
        );
        // The border is around the chosen widget's own box. Its bottom edge is the pixel to look at, because
        // the top left of a text widget is where its first glyph is - ink either way, and the same either way.
        let bottom_of_first = 2 + g13_screen::font::LINE_HEIGHT - 1;
        let bottom_of_second = 20 + g13_screen::font::LINE_HEIGHT - 1;
        assert!(
            first.get(4, bottom_of_first),
            "no border under the chosen widget"
        );
        assert!(
            second.get(4, bottom_of_second),
            "no border under the second widget when it is chosen"
        );
        assert!(
            !first.get(4, bottom_of_second),
            "the unchosen widget was outlined as well"
        );
        // and nothing chosen draws no border at all
        let none = draw_with(None);
        assert!(
            !none.get(4, bottom_of_first),
            "a selection of zero outlined the first thing"
        );
    }
    #[test]
    fn an_applet_can_have_more_than_one_screen() {
        // the shape that has no screens at all: one screen, its own widgets, nothing to rewrite
        let one = applet(
            r#"{"name": "gpu", "widgets": [{"type": "text", "x": 1, "y": 1, "format": "ONE"}]}"#,
        );
        assert_eq!(screen_count(&one), 1);
        assert_eq!(screen_widgets(&one, 0).len(), 1);
        assert_eq!(screen_title(&one, 0), "screen 1");
        // and an index past the end is an empty screen rather than a panic
        assert!(screen_widgets(&one, 7).is_empty());

        let many = applet(
            r#"{"name": "docker", "screens": [
                {"title": "containers", "widgets": [{"type": "text", "x": 1, "y": 1, "format": "ONE"}]},
                {"widgets": [{"type": "text", "x": 1, "y": 1, "format": "TWO"}]}
            ]}"#,
        );
        assert_eq!(screen_count(&many), 2);
        assert_eq!(screen_title(&many, 0), "containers");
        // a screen with no title says which one it is rather than showing nothing
        assert_eq!(screen_title(&many, 1), "screen 2");
        assert_eq!(screen_widgets(&many, 1).len(), 1);

        // and each screen draws its own
        let values = Values::new();
        let mut first = Frame::new();
        let mut second = Frame::new();
        draw_screen(&many, 0, &mut first, &values, &mut Vec::new());
        draw_screen(&many, 1, &mut second, &values, &mut Vec::new());
        assert_ne!(first, second, "both screens drew the same thing");

        // both keys at once is ambiguous, so it is said and the screens are what is drawn
        let both = applet(
            r#"{"name": "x", "widgets": [{"type": "text", "x": 1, "y": 1, "format": "TOP"}],
                "screens": [{"widgets": [{"type": "text", "x": 1, "y": 1, "format": "DEEP"}]}]}"#,
        );
        assert_eq!(screen_count(&both), 1);
        assert!(
            both.unhandled.iter().any(|note| note.contains("both")),
            "{:?}",
            both.unhandled
        );
    }

    #[test]
    fn an_applet_only_has_the_values_it_declares() {
        let applet = parse_applet(
            r#"{"name":"x","title":"X","interval":1,"border":false,
                "sources":{"gpu":"cmd:echo 22"},
                "widgets":[{"type":"text","x":3,"y":2,"format":"{gpu}% {cpu}"}]}"#,
        )
        .unwrap();
        let (values, problems) = gather(&applet, &World::default());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(values.get("gpu").map(Value::text), Some("22".to_string()));
        // `cpu` is a built-in name this applet never asks for, so there is no value and the pad draws the name
        assert!(!values.contains_key("cpu"), "a value arrived unasked for");
        assert_eq!(fill("{gpu}% {cpu}", &values), "22% {cpu}");
        assert_eq!(
            values_nothing_provides(&applet, &values),
            vec!["cpu".to_string()]
        );
    }

    #[test]
    fn a_name_out_of_the_catalogue_resolves_without_the_applet_saying_how() {
        // the catalogue is the framework: a value the driver publishes is usable by name, and what it means is
        // documented and shown in the window rather than being a hidden fallback
        let applet = parse_applet(
            r#"{"name":"x","title":"X","interval":1,"border":false,
                "sources":{"cpu":"cpu","up":"uptime"},
                "widgets":[{"type":"text","x":3,"y":2,"format":"{cpu:.0f}% up {up}"}]}"#,
        )
        .unwrap();
        let (values, problems) = gather(&applet, &World::default());
        assert!(problems.is_empty(), "{problems:?}");
        assert!(
            values.get("cpu").and_then(Value::number).is_some(),
            "cpu did not resolve: {:?}",
            values.get("cpu")
        );
        assert!(
            values
                .get("up")
                .map(Value::text)
                .is_some_and(|t| !t.is_empty())
        );
        assert!(values_nothing_provides(&applet, &values).is_empty());
    }

    #[test]
    fn the_shape_of_a_real_applet_parses() {
        let applet = applet(
            r#"{
              "name": "gpu", "title": "GPU", "interval": 1, "border": true,
              "sources": {"util": "cmd:printf 50"},
              "widgets": [
                {"type": "text", "x": 3, "y": 12, "format": "GPU:"},
                {"type": "bar", "x": 30, "y": 12, "w": 100, "h": 9, "source": "util", "max": 100},
                {"type": "text", "x": 157, "y": 12, "align": "right", "format": "{util}%"}
              ]
            }"#,
        );
        assert_eq!(applet.name, "gpu");
        assert_eq!(applet.title, "GPU");
        assert_eq!(applet.interval, 1.0);
        assert!(applet.border);
        assert_eq!(applet.widgets.len(), 3);
        assert_eq!(applet.sources["util"], "cmd:printf 50");
    }

    #[test]
    fn every_widget_type_the_applets_use_parses() {
        let applet = applet(
            r#"{"name":"x","widgets":[
              {"type":"text","x":1,"y":2,"format":"a"},
              {"type":"bar","x":1,"y":2,"w":3,"h":4,"source":"a","max":5},
              {"type":"line","x":1,"y":2,"w":3},
              {"type":"segments","x":3,"y":12,"w":56,"h":5,"count":11,"source":"health","max":100},
              {"type":"brackets","x":1,"y":28,"w":18,"h":13,"len":5,"thick":2},
              {"type":"arrow","x":10,"y":34,"r":6,"source":"pin_relative"}
            ]}"#,
        );
        assert_eq!(applet.widgets.len(), 6);
        assert!(matches!(applet.widgets[0], Widget::Text { .. }));
        assert!(matches!(
            applet.widgets[3],
            Widget::Segments { count: 11, .. }
        ));
        assert!(matches!(
            applet.widgets[4],
            Widget::Brackets {
                len: 5,
                thick: 2,
                ..
            }
        ));
        assert!(matches!(applet.widgets[5], Widget::Arrow { radius: 6, .. }));
    }

    #[test]
    fn a_widget_type_this_build_does_not_know_is_reported_not_ignored() {
        let applet = applet(r#"{"name":"x","widgets":[{"type":"hologram","x":1,"y":1}]}"#);
        assert!(
            applet.unhandled.iter().any(|key| key.contains("hologram")),
            "{:?}",
            applet.unhandled
        );
    }

    #[test]
    fn a_missing_value_draws_nothing_rather_than_a_zero() {
        let applet = applet(
            r#"{"name":"x","border":false,"sources":{},
                "widgets":[
                  {"type":"text","x":3,"y":3,"format":"{ammo} left"},
                  {"type":"bar","x":3,"y":20,"w":50,"h":5,"source":"ammo","max":100}]}"#,
        );
        let mut frame = Frame::new();
        let (values, _) = gather(&applet, &World::default());
        let mut problems = Vec::new();
        draw(&applet, &mut frame, &values, &mut problems);
        // nothing is not zero: no bar at all
        assert!(
            frame.is_blank(3, 20, 50, 5),
            "a missing source must not draw an empty bar"
        );
    }

    #[test]
    fn a_bar_is_as_long_as_its_share_and_starts_where_it_is_placed() {
        let applet = applet(
            r#"{"name":"x","border":false,
                "sources":{"util":"cmd:printf 50"},
                "widgets":[{"type":"bar","x":30,"y":12,"w":100,"h":4,"source":"util","max":100}]}"#,
        );
        let mut frame = Frame::new();
        let (values, _) = gather(&applet, &World::default());
        let mut problems = Vec::new();
        draw(&applet, &mut frame, &values, &mut problems);
        // half of a hundred, starting at thirty rather than sixty
        assert!(frame.get(30, 12), "the bar should start at its own x");
        assert!(frame.get(79, 12), "and reach halfway");
        assert!(!frame.get(81, 12), "and no further");
        assert!(!frame.get(10, 12), "and not before it");
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn format_specs_are_filled_the_way_the_applets_write_them() {
        let mut values = Values::new();
        values.insert("cpu".into(), Value::Number(42.4));
        values.insert("name".into(), Value::Text("night".into()));
        assert_eq!(fill("{cpu}", &values), "42.4", "a plain name");
        assert_eq!(fill("{cpu:.0f}", &values), "42", "a precision, on its own");
        assert_eq!(
            fill("{cpu:.0f}%", &values),
            "42%",
            "a precision with text after it"
        );
        assert_eq!(fill("city {name}", &values), "city night");
        // an unknown name stays visible rather than vanishing
        assert_eq!(fill("{nope}", &values), "{nope}");
        assert_eq!(fill("no braces", &values), "no braces");
    }
    #[test]
    fn a_value_in_a_command_is_quoted_and_a_value_in_a_format_is_not() {
        let mut values = Values::new();
        values.insert("item".into(), Value::Text("it's a track".into()));
        values.insert("cpu".into(), Value::Number(42.4));
        // a command: one argument, so nothing in the value is shell syntax
        assert_eq!(
            fill_command("cmd:printf %s {item}", &values),
            "cmd:printf %s 'it'\\''s a track'"
        );
        // a format is text for the panel and is never run
        assert_eq!(fill("{item}", &values), "it's a track");
        assert_eq!(
            fill_command("screen:playlist {item}", &values),
            "screen:playlist it's a track",
            "not a command, so nothing is quoted"
        );
        // the precision is applied first, and then the value is quoted
        assert_eq!(fill_command("cmd:echo {cpu:.0f}", &values), "cmd:echo '42'");
        // literal text is untouched: an awk body's own braces and quotes are not a value
        assert_eq!(
            fill_command("cmd:awk '{printf $1}' {item}", &values),
            "cmd:awk '{printf $1}' 'it'\\''s a track'"
        );
        // a value spliced into a word still makes one word
        assert_eq!(
            fill_command("cmd:cat /tmp/{cpu}.json", &values),
            "cmd:cat /tmp/'42.4'.json"
        );
        // a name that is not a value is left as it was written, in a command as in a format
        assert_eq!(fill_command("cmd:echo {nope}", &values), "cmd:echo {nope}");
    }

    #[test]
    fn text_that_would_land_on_ink_is_reported() {
        // a box is drawn, and then text over it: the ink rule must have something to say
        let applet = applet(
            r#"{"name":"x","border":false,"sources":{},
                "widgets":[
                  {"type":"line","x":2,"y":12,"w":60},
                  {"type":"text","x":2,"y":12,"format":"over the line"}]}"#,
        );
        let mut frame = Frame::new();
        let mut problems = Vec::new();
        draw(&applet, &mut frame, &Values::new(), &mut problems);
        assert!(
            !problems.is_empty(),
            "text sitting on a line must be reported"
        );
        assert!(
            problems[0].contains("ink was already there"),
            "{problems:?}"
        );
    }

    #[test]
    fn text_honours_the_advance_and_does_not_overlap_itself() {
        // twenty characters at six pixels each fit; the rule must not complain about a character
        // colliding with the one before it
        let applet = applet(
            r#"{"name":"x","border":false,"sources":{},
                "widgets":[{"type":"text","x":0,"y":0,"format":"0123456789012345678901"}]}"#,
        );
        let mut frame = Frame::new();
        let mut problems = Vec::new();
        draw(&applet, &mut frame, &Values::new(), &mut problems);
        assert!(
            problems.is_empty(),
            "characters must not overlap each other: {problems:?}"
        );
        assert!(
            !frame.is_blank(0, 0, g13_screen::WIDTH, 8),
            "and the text is drawn"
        );
    }

    #[test]
    fn scrolling_text_shows_its_end() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("0123456789abcdef", 4), "cdef");
    }
}

#[cfg(test)]
mod a_font_from_a_file {
    use super::*;

    /// Built rather than written as a literal: `#` rows inside `r#"..."#` close the literal, which is the trap this
    /// project has now hit five times. The rows are *built* from the pattern, so the file the test writes is the file
    /// a person would write.
    fn runes() -> String {
        let a = [
            "..##..", ".#..#.", "#....#", "#....#", "######", "#....#", "#.#..#.",
        ];
        let b = [
            "#####.", "#....#", "#....#", "#####.", "#....#", "#....#", "#####.",
        ];
        let rows = |glyph: &[&str]| {
            glyph
                .iter()
                .map(|row| format!("\"{row}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            r#"{{"name": "runes", "advance": 6, "line_height": 8, "glyph_rows": 7,
                "from": "drawn here for the test",
                "glyphs": {{"A": [{}], "B": [{}]}}}}"#,
            rows(&a),
            rows(&b)
        )
    }

    #[test]
    fn it_loads_and_draws_and_a_broken_one_is_said() {
        let dir = std::env::temp_dir().join(format!("g13-fonts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("runes.json");
        std::fs::write(&path, runes()).unwrap();

        let font = load_font(&path).expect("it reads");
        assert_eq!(font.name, "runes");
        assert_eq!((font.advance, font.height, font.rows), (6, 8, 7));
        assert!(font.known('A').is_some());
        assert!(
            font.known('Z').is_none(),
            "a character the font lacks came back known"
        );

        // and it draws: the crossbar row of A is solid, which is the check that the rows went in the right way up
        let mut frame = g13_screen::Frame::new().with_fonts(vec![font.clone()]);
        frame.text(0, 0, "A");
        assert!(
            frame.get(0, 3) && frame.get(1, 3) && frame.get(5, 3),
            "A's crossbar is missing"
        );
        assert!(
            !frame.get(2, 3),
            "A's crossbar is solid where it should be hollow"
        );
        assert!(frame.get(2, 0) && frame.get(3, 0), "A's first row is wrong");
        // the art, so a wrong way up is visible when this is read with --nocapture
        println!("--- A from the file ---");
        for row in 0..8 {
            println!(
                "   {}",
                (0..8)
                    .map(|col| if frame.get(col, row) { '#' } else { '.' })
                    .collect::<String>()
            );
        }

        // a glyph with the wrong number of rows is refused with the reason, not drawn wrongly
        std::fs::write(dir.join("bad.json"), "{\"glyphs\": {\"A\": [\"##\"]}}").unwrap();
        let problem =
            load_font(&dir.join("bad.json")).expect_err("two rows where the font says seven");
        assert!(problem.contains("rows"), "{problem}");

        // and the folder: one good font in, with the bad one reported rather than swallowed
        let (fonts, problems) = load_fonts(&dir);
        assert!(fonts.contains_key("runes"), "{:?}", fonts.keys());
        assert_eq!(
            problems.len(),
            1,
            "one file will not read, and it should say so: {problems:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod an_applet_that_names_a_font {
    use super::*;

    /// A row of pixels, with ink where the pattern says `X`.
    ///
    /// **Built rather than written.** A `#` inside a raw Rust string closes it - the trap this project has hit six
    /// times now - so the patterns here use `X` and this turns them into the `#` the font format wants.
    fn row(pattern: &str) -> String {
        pattern
            .chars()
            .map(|cell| if cell == 'X' { '#' } else { '.' })
            .collect()
    }

    #[test]
    fn the_named_font_draws_it_and_the_panel_font_fills_in_the_rest() {
        let dir = std::env::temp_dir().join(format!("g13-applet-font-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("fonts")).unwrap();
        let rune_rows = [
            "......", "XXXXX.", "X....X", "XXXXX.", "X..X..", "X...X.", "X....X",
        ]
        .iter()
        .map(|pattern| format!("\"{}\"", row(pattern)))
        .collect::<Vec<_>>()
        .join(", ");
        std::fs::write(
            dir.join("fonts/runes.json"),
            format!(
                "{{\"name\": \"runes\", \"advance\": 6, \"line_height\": 8, \"glyph_rows\": 7, \"glyphs\": {{\"R\": [{rune_rows}]}}}}"
            ),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("applets/rune.json"),
            "{\"name\": \"rune\", \"font\": \"runes\", \"widgets\": [{\"type\": \"text\", \"x\": 0, \"y\": 0, \"format\": \"R\"}]}",
        )
        .unwrap();

        let (fonts, problems) = load_fonts(&dir.join("fonts"));
        assert!(problems.is_empty(), "{problems:?}");
        let text = std::fs::read_to_string(dir.join("applets/rune.json")).unwrap();
        let applet = with_font(parse_applet(&text).expect("it parses"), &fonts);
        assert_eq!(applet.font_name.as_deref(), Some("runes"));
        assert!(
            applet.font.is_some(),
            "the named font was not put on the applet"
        );
        assert!(applet.unhandled.is_empty(), "{:?}", applet.unhandled);

        let mut frame = Frame::new();
        draw_screen_at(
            &applet,
            0,
            Context::at(0.0),
            &mut frame,
            &Values::default(),
            &mut Vec::new(),
        );
        println!("--- the R the font file draws ---");
        for line in 0..8 {
            println!(
                "   {}",
                (0..8)
                    .map(|column| if frame.get(column, line) { '#' } else { '.' })
                    .collect::<String>()
            );
        }
        // the glyph is the file's own: its top bar is a row of ink
        assert!(
            (0..5).all(|column| frame.get(column, 1)),
            "the rune's top bar is missing"
        );

        // a name nothing has is said, and the panel font is used instead
        let applet = with_font(
            parse_applet("{\"name\": \"rune\", \"font\": \"absent\", \"widgets\": []}")
                .expect("it parses"),
            &fonts,
        );
        assert!(applet.font.is_none());
        assert!(
            applet.unhandled.iter().any(|it| it.contains("absent")),
            "a missing font was swallowed: {:?}",
            applet.unhandled
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod the_runes_that_ship {
    use super::*;

    /// The font that ships, loaded through the same loader a driver uses.
    fn runes() -> g13_screen::font::Font {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../defaults/fonts/runes.json");
        load_font(&path).expect("the shipped rune font reads")
    }

    #[test]
    fn all_twenty_four_load_fit_their_cells_and_draw_a_line() {
        let runes = runes();
        assert_eq!(runes.name, "runes");
        // the Elder Futhark row, all of it
        let elder = "ᚠᚢᚦᚨᚱᚲᚷᚹᚺᚾᛁᛃᛇᛈᛉᛊᛏᛒᛖᛗᛚᛜᛞᛟ";
        assert_eq!(elder.chars().count(), 24);
        for character in elder.chars() {
            let ink = runes
                .known(character)
                .unwrap_or_else(|| panic!("{character} is missing from the font"));
            assert!(
                ink[runes.advance..].iter().all(|byte| *byte == 0),
                "{character} draws past the advance"
            );
            assert!(ink.iter().any(|byte| *byte != 0), "{character} is blank");
        }

        // a line of them, in the applet, drawn with the panel font behind: nothing needs the panel font here
        let mut frame =
            Frame::new().with_fonts(vec![runes.clone(), g13_screen::font::panel().clone()]);
        // characters, not bytes: a rune is three bytes in UTF-8, which is how a slice of twelve came out as four
        let twelve: String = elder.chars().take(12).collect();
        let width = frame.text_in(std::slice::from_ref(&runes), 0, 0, &twelve);
        assert_eq!(
            width,
            12 * runes.advance,
            "twelve runes did not measure twelve advances"
        );
        for line in 0..8 {
            println!(
                "   {}",
                (0..72)
                    .map(|column| if frame.get(column, line) { '#' } else { '.' })
                    .collect::<String>()
            );
        }
        // and a rune the font has not got - a kanji - is a box rather than silence
        assert!(runes.known('中').is_none());
    }
}

#[cfg(test)]
mod a_title_in_more_than_one_language {
    use super::*;

    /// `X` for ink, so no `#` ever sits in a Rust literal.
    fn row(pattern: &str) -> String {
        pattern
            .chars()
            .map(|cell| if cell == 'X' { '#' } else { '.' })
            .collect()
    }

    #[test]
    fn a_value_the_applet_could_not_know_about_is_still_drawn_in_the_fonts_that_cover_it() {
        // `media_title` is arbitrary text from outside. The applet did not name a font for it and could not - so
        // every font in the folder is a fallback for every screen.
        let dir = std::env::temp_dir().join(format!("g13-title-mixed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("fonts")).unwrap();
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        let rune_rows = [
            "..X...", ".X.X..", "X...X.", "..X...", "..X...", "..X...", "..X...",
        ]
        .iter()
        .map(|pattern| format!("\"{}\"", row(pattern)))
        .collect::<Vec<_>>()
        .join(", ");
        std::fs::write(
            dir.join("fonts/runes.json"),
            format!(
                "{{\"name\": \"runes\", \"advance\": 6, \"line_height\": 8, \"glyph_rows\": 7, \"glyphs\": {{\"ᛉ\": [{rune_rows}]}}}}"
            ),
        )
        .unwrap();
        // an applet that names no font at all, drawing a value
        std::fs::write(
            dir.join("applets/title.json"),
            "{\"name\": \"title\", \"sources\": {\"song\": \"cmd:echo mixed\"}, \"widgets\": [{\"type\": \"text\", \"x\": 0, \"y\": 0, \"format\": \"{song}\"}]}",
        )
        .unwrap();

        let (faces, problems) = fonts_cached(&dir.join("fonts"));
        assert!(problems.is_empty(), "{problems:?}");
        assert!(faces.contains_key("runes"));
        let text = std::fs::read_to_string(dir.join("applets/title.json")).unwrap();
        let applet = with_font(parse_applet(&text).expect("it parses"), &faces);
        assert!(
            applet.font.is_none(),
            "the applet named no font, so it should have none of its own"
        );
        assert_eq!(
            applet.fallbacks.len(),
            1,
            "the folder's fonts should be fallbacks"
        );
        assert!(applet.unhandled.is_empty(), "{:?}", applet.unhandled);

        // a title in three scripts: Latin, Cyrillic, and a rune the panel font has never heard of
        let mut values = Values::default();
        values.insert(
            "song".to_string(),
            g13_values::Value::Text("DJ — Ж — ᛉ".to_string()),
        );
        let mut frame = Frame::new();
        draw_screen_at(
            &applet,
            0,
            Context::at(0.0),
            &mut frame,
            &values,
            &mut Vec::new(),
        );
        println!("--- a mixed title, drawn ---");
        for line in 0..8 {
            println!(
                "   {}",
                (0..70)
                    .map(|column| if frame.get(column, line) { '#' } else { '.' })
                    .collect::<String>()
            );
        }
        // The proof that the fallback did the work: the same title, drawn with the folder's fonts and then without
        // them. Whatever the runes add is what the applet could not have drawn for itself.
        let mut without = Frame::new();
        draw_screen_at(
            &with_font(parse_applet(&text).expect("it parses"), &Default::default()),
            0,
            Context::at(0.0),
            &mut without,
            &values,
            &mut Vec::new(),
        );
        let difference: usize = (0..8)
            .map(|line| {
                (0..70)
                    .filter(|column| frame.get(*column, line) != without.get(*column, line))
                    .count()
            })
            .sum();
        assert!(
            difference > 0,
            "the rune in the title changed nothing: the folder's fonts are not being used as fallbacks"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod every_font_that_ships_is_a_font_that_loads {
    use super::*;

    fn shipped() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../defaults/fonts")
    }

    /// The three the binary has always carried, now also files. If a file and the table it was written out from ever
    /// disagree, one of them is wrong and the user cannot tell which - so they are compared glyph by glyph.
    #[test]
    fn the_shipped_fonts_load_and_agree_with_the_binary() {
        let dir = shipped();
        let (fonts, problems) = load_fonts(&dir);
        assert!(
            problems.is_empty(),
            "shipped fonts that did not load: {problems:?}"
        );
        let names: Vec<&str> = fonts.keys().map(|name| name.as_str()).collect();
        for expected in ["panel", "greek", "cyrillic", "runes"] {
            assert!(
                names.contains(&expected),
                "{expected} is not on the list: {names:?}"
            );
        }

        // the panel font is the one the whole project draws with, so it gets the strictest check
        let panel = fonts
            .iter()
            .find(|(name, _)| name.as_str() == "panel")
            .map(|(_, f)| f)
            .expect("panel loads");
        let built_in = g13_screen::font::panel();
        assert_eq!(
            panel.advance, built_in.advance,
            "the panel file's advance differs from the binary's"
        );
        for character in 'A'..='Z' {
            assert_eq!(
                panel.glyph(character),
                built_in.glyph(character),
                "the panel file and the binary disagree about {character}"
            );
        }
        // the full range, which is the thing the file makes checkable for the first time
        for code in 32u32..127 {
            let character = char::from_u32(code).unwrap();
            assert!(
                panel.known(character).is_some(),
                "the panel file stops short of ASCII at {character:?}"
            );
        }
    }

    #[test]
    fn greek_and_cyrillic_kept_every_glyph_and_two_of_them_moved_to_where_they_belonged() {
        let dir = shipped();
        let (fonts, _) = load_fonts(&dir);
        let greek = fonts
            .iter()
            .find(|(name, _)| name.as_str() == "greek")
            .map(|(_, f)| f)
            .expect("greek loads");
        let cyrillic = fonts
            .iter()
            .find(|(name, _)| name.as_str() == "cyrillic")
            .map(|(_, f)| f)
            .expect("cyrillic loads");
        assert_eq!(greek.glyph_count(), 56, "the greek table holds 56 entries");
        assert_eq!(
            cyrillic.glyph_count(),
            130,
            "the cyrillic table holds 130 entries"
        );
        // the diaeresis forms, which used to be sitting on the codepoints of the plain letters
        for (character, what) in [
            ('\u{03aa}', "I with diaeresis"),
            ('\u{03ab}', "Y with diaeresis"),
        ] {
            assert!(greek.known(character).is_some(), "greek has no {what}");
        }
        // the count is the claim that matters: two letters sitting on the same codepoint would show up here as a
        // font that is short of the fifty-six its table holds, which is exactly how the pair got noticed
    }
}
