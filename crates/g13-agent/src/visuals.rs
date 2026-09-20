//! What the screen shows, and when.
//!
//! The driver holds the pad, so it is also the thing that draws on it: one process owns the device, reads
//! the keys and writes the frames. That is why the driver and the visuals were separate processes before and
//! are one here  -  the socket between them only ever existed to bridge two languages.
//!
//! `visuals.json` says which visuals are enabled and which one is active. Two kinds of visual are known:
//!
//! - `applet:<name>`  -  an applet file in `~/.config/g13/applets/<name>.json`, drawn from its own sources.
//! - `clock`, `system` and `pad`  -  drawn from the machine and the pad itself, needing no file.
//!
//! The values the driver publishes are written to `g13-values.json`, with the same keys the previous stack
//! published: anything reading that file keeps working.

use crate::{Stage, State, Wizard};
use g13_screen::{Frame, TEXT_COLUMNS, VISIBLE_HEIGHT, WIDTH};
use g13_sources::World;
use g13_values::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Visuals;

/// Everything one frame needed to know.
pub struct Moment<'a> {
    /// The pad's published values, which every visual draws from.
    pub state: &'a State,
    /// Wall clock, for the visuals that show a time.
    pub clock: String,
    /// Today's date, as a visual shows it.
    pub date: String,
    /// The day of the week.
    pub day: String,
    /// The pad's own state, as the values file describes it.
    pub profile: u32,
    /// The stick's mode, as the values file names it.
    pub button_mode: String,
}

/// The visual one step from `current` in a list, wrapping round.
///
/// Pure, and tested, because "which screen comes next" is the sort of thing that is only wrong in an order nobody
/// tried: the current one not being in the list, a list of one, an empty list, and the two ends are all real.
pub fn step_visual(enabled: &[String], current: &str, step: isize) -> Option<String> {
    if enabled.is_empty() {
        return None;
    }
    let at = enabled.iter().position(|name| name == current);
    let count = enabled.len() as isize;
    let next = match at {
        Some(at) => (at as isize + step).rem_euclid(count),
        // nothing is showing that is in the list: the first one is the honest answer, whichever way we were going
        None => 0,
    };
    enabled.get(next as usize).cloned()
}

/// Names in the rotation that this build cannot draw, for saying so once rather than never.
///
/// A name that is in `enabled` and cannot be drawn is skipped by `next` and `prev`. That is the right behaviour
/// and it is also invisible, so the driver says it once at startup: an enabled screen that will never appear is
/// something to know about, not something to work out from a list.
pub fn undrawable_in(enabled: &[String], applets: &Path) -> Vec<String> {
    let drawable = drawable_visuals(applets);
    enabled
        .iter()
        .filter(|name| !drawable.contains(name))
        .map(|name| {
            format!(
                "{name} is one of the screens you have enabled and is not a screen this build can draw, so the \
                 rotation skips it"
            )
        })
        .collect()
}

/// What a control bound to `sv,<what>` should show, or why it should not change anything.
///
/// Outside the run loop on purpose: "the next screen" is a decision with four shapes - a list with the current
/// one in it, a list without it, a list of one, and nothing enabled - and a decision buried in a loop that reads
/// the pad is a decision nobody can test. The loop is left with the press and the write.
pub fn screen_target(
    what: &str,
    enabled: &[String],
    active: &str,
    applets: &Path,
) -> Result<Option<String>, String> {
    let target = match what {
        "next" | "prev" => {
            let step = if what == "next" { 1 } else { -1 };
            // A name in the rotation that cannot be drawn must not jam it. A `visuals.json` may carry `custom` -
            // a screen the previous stack had and this one does not - and asking for the next screen used to land
            // on it and stop there for ever, which reads as the button having broken.
            let drawable = drawable_visuals(applets);
            let mut from = active.to_string();
            let mut found = None;
            for _ in 0..enabled.len() {
                let Some(candidate) = step_visual(enabled, &from, step) else {
                    break;
                };
                if candidate == active {
                    break;
                }
                if drawable.contains(&candidate) {
                    found = Some(candidate);
                    break;
                }
                from = candidate;
            }
            match found {
                Some(target) => target,
                // Round the whole list and back to what is showing, because it is the only one that can be
                // drawn: nothing to change, and nothing to write.
                None if enabled.iter().any(|name| drawable.contains(name)) => return Ok(None),
                // Nothing in the rotation can be drawn at all, which is not the same thing and is worth saying.
                None => {
                    return Err(format!(
                        "`sv,{what}` has nothing to change to: none of the {} enabled screen(s) is one this \
                         build can draw",
                        enabled.len()
                    ));
                }
            }
        }
        // `sv,3` is the third screen in the rotation, counting from 1. It is how L1-L4 become four keys to four
        // particular screens rather than four copies of "next" - which is what "L1-L4 are also to be used for
        // changing applet and menus" needs from them.
        number if number.chars().all(|c| c.is_ascii_digit()) => {
            let wanted: usize = match number.parse() {
                Ok(wanted) if wanted >= 1 && wanted <= enabled.len() => wanted,
                _ => {
                    return Err(format!(
                        "`sv,{number}` asks for screen {number} and the rotation has {}: {}",
                        enabled.len(),
                        enabled.join(", ")
                    ));
                }
            };
            enabled[wanted - 1].clone()
        }
        name => name.to_string(),
    };
    // a name that cannot be drawn is said rather than showing a blank: asking for a screen that is not there
    // and getting an empty one is the worst of both
    if !drawable_visuals(applets).contains(&target) {
        return Err(format!(
            "`sv,{what}` asks for {target}, which is not a visual this build can draw, so the screen was left \
             alone"
        ));
    }
    // already showing it: nothing to do, and nothing to write
    if target == active {
        return Ok(None);
    }
    Ok(Some(target))
}

/// Point `visuals.json` at a visual, leaving everything else in the file exactly as it was.
///
/// Read and rewritten as data rather than formatted by hand, so `enabled`, `cycle`, `cycle_seconds` and any key
/// this build does not know survive. A file that will not parse is refused rather than replaced: the screen is
/// better off as it was than as nothing.
pub fn write_active_visual(path: &Path, visual: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not JSON: {error}", path.display()))?;
    let Some(object) = value.as_object_mut() else {
        return Err(format!("{} is not an object", path.display()));
    };
    object.insert(
        "active".to_string(),
        serde_json::Value::String(visual.to_string()),
    );
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("cannot write {visual} as JSON: {error}"))?;
    // beside and renamed: the window polls this file to show which screen is up
    g13_files::write(path, &format!("{text}\n"))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// The four screens that used to be built into this code, now files like everything else.
pub const ONCE_BUILT_IN: [&str; 4] = ["clock", "system", "pad", "media"];

/// Every visual this build can draw: one per applet in the folder.
///
/// It used to head this list with four names built into the code - `clock`, `system`, `pad`, `media` - which meant
/// those four could not be edited while everything else could. They are applet files now, so the head is gone: a
/// name in two lists is how a screen ends up in the walk twice, and the file is the one a user can change.
///
/// A screen that used to be built in keeps its **bare** name while its file is there, because that is the name a
/// `visuals.json` says (`"enabled": ["clock", "media"]`) and what the `sv,1` .. `sv,8` bindings count over. It is in
/// the walk once, not twice - the bare name stands in for the applet's own. Delete the file and the name goes too.
pub fn drawable_visuals(applets: &Path) -> Vec<String> {
    let mut names: Vec<String> = ONCE_BUILT_IN
        .iter()
        .filter(|name| applets.join(format!("{name}.json")).exists())
        .map(|name| name.to_string())
        .collect();
    if let Ok(entries) = std::fs::read_dir(applets) {
        let mut found: Vec<String> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                // an applet, not something beside one: `audio.bitmaps.json` is audio's pictures, and offering
                // it as a screen would put a name in the rotation that nothing can draw
                if g13_applets::is_applet_file(&path) {
                    let stem = path.file_stem()?.to_string_lossy().to_string();
                    // its bare name is already in the walk above
                    if ONCE_BUILT_IN.contains(&stem.as_str()) {
                        return None;
                    }
                    Some(format!("applet:{stem}"))
                } else {
                    None
                }
            })
            .collect();
        found.sort();
        names.extend(found);
    }
    names
}

/// The source behind a screen's first list widget, already looked up in the applet's own `sources`.
///
/// The driver needs this one thing - the command to run to see the rows - while the *walk* over what the pad
/// can act on belongs to `g13-applets`, which owns the order the screen is drawn in.
pub fn list_source(visual: &str, screen: usize, applets: &Path) -> Option<String> {
    let name = visual.strip_prefix("applet:")?;
    let value = g13_applets::load_applet_json(applets, name).ok()?;
    let screen_object = match value.get("screens").and_then(|s| s.as_array()) {
        // one screen of an applet without `screens` is the applet itself
        None => value.as_object()?.clone(),
        Some(screens) => screens.get(screen)?.as_object()?.clone(),
    };
    let named = screen_object
        .get("widgets")
        .and_then(|widgets| widgets.as_array())
        .into_iter()
        .flatten()
        .find(|widget| widget.get("type").and_then(|kind| kind.as_str()) == Some("list"))
        .and_then(|widget| widget.get("source"))
        .and_then(|source| source.as_str())?
        .to_string();
    // A widget names its source the way every other widget does: as a key in the applet's own `sources`, or as
    // a mechanism spelled out in full. The same two-step the reader that draws the widget takes.
    Some(
        value
            .get("sources")
            .and_then(|sources| sources.get(&named))
            .and_then(|spec| spec.as_str())
            .map(str::to_string)
            .unwrap_or(named),
    )
}

/// Everything on a screen the pad can act on, in the order the screen draws them.
pub fn screen_walk(
    visual: &str,
    screen: usize,
    applets: &Path,
    items: &[String],
    values: &BTreeMap<String, g13_values::Value>,
    profile: u32,
) -> Vec<g13_applets::Selectable> {
    let Some(name) = visual.strip_prefix("applet:") else {
        return Vec::new();
    };
    match g13_applets::load_applet_json(applets, name)
        .ok()
        .and_then(|value| g13_applets::parse_applet(&value.to_string()).ok())
    {
        Some(applet) => g13_applets::selectables(&applet, screen, items, values, profile),
        None => Vec::new(),
    }
}

/// Read `visuals.json`. A missing or unreadable file gives the defaults rather than an error: the driver
/// still reads the pad, which is the thing it exists for.
pub fn load_visuals(path: &Path) -> Visuals {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Visuals::default();
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => Visuals {
            enabled: value
                .get("enabled")
                .and_then(|list| list.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            active: value
                .get("active")
                .and_then(|active| active.as_str())
                .unwrap_or("system")
                .to_string(),
            cycle: value
                .get("cycle")
                .and_then(|cycle| cycle.as_bool())
                .unwrap_or(false),
            cycle_seconds: value
                .get("cycle_seconds")
                .and_then(|seconds| seconds.as_f64())
                .unwrap_or(10.0),
        },
        Err(_) => Visuals::default(),
    }
}

/// Draw one visual into a frame, saying why it could not be drawn if it could not.
pub fn render(
    visual: &str,
    moment: &Moment<'_>,
    world: &World,
    applet_dir: &Path,
) -> (Frame, Vec<String>) {
    render_screen(visual, 0, moment, world, applet_dir)
}

/// Render one of an applet's screens. `screen` is ignored by the built-in visuals, which have one each.
pub fn render_screen(
    visual: &str,
    screen: usize,
    moment: &Moment<'_>,
    world: &World,
    applet_dir: &Path,
) -> (Frame, Vec<String>) {
    let (frame, problems, _) =
        render_screen_from(visual, screen, moment, world, applet_dir, None, None);
    (frame, problems)
}

/// The same, with the applet's own values from last time.
///
/// Two rates, because they answer different questions. Reading a source can cost - a `cmd:` is a hundred
/// milliseconds and up, and the machine's numbers are about a fifth of a second of `playerctl` - while drawing
/// is a local write. So the caller keeps what it gathered and hands it back, and only the values that move with
/// the pad are asked for again. `None` means gather everything, which is what happens once per interval.
///
/// The values used come back with the frame, so the caller can keep them for next time.
pub fn render_screen_from(
    visual: &str,
    screen: usize,
    moment: &Moment<'_>,
    world: &World,
    applet_dir: &Path,
    kept: Option<&BTreeMap<String, g13_values::Value>>,
    machine: Option<&BTreeMap<String, g13_values::Value>>,
) -> (
    Frame,
    Vec<String>,
    Option<BTreeMap<String, g13_values::Value>>,
) {
    let mut frame = Frame::new();
    let mut problems = Vec::new();

    // A recording is modal, so the screen is the wizard's own while one runs: the stage is the only way to see
    // what it is waiting for without a terminal open beside it. Drawn *instead of* the visual, never over it -
    // an applet's own drawing showing through the words would be worse than either on its own.
    if let Some(wizard) = moment.state.recording.as_ref() {
        draw_recording(&mut frame, wizard);
        return (frame, problems, None);
    }

    if let Some(name) = visual.strip_prefix("applet:") {
        let path = applet_dir.join(format!("{name}.json"));
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                problems.push(format!("{visual}: cannot read {}: {error}", path.display()));
                return (frame, problems, None);
            }
        };
        match g13_applets::parse_applet(&text) {
            Ok(applet) => {
                // its pictures, from the shared bitmaps file and its own, before anything is drawn
                let (faces, _) = g13_applets::fonts_cached(&fonts_beside(applet_dir));
                let applet = g13_applets::with_theme(
                    g13_applets::with_font(g13_applets::with_bitmaps(applet, applet_dir), &faces),
                    &themes_dir(applet_dir),
                );
                // An applet's values are exactly the ones it declares: nothing is fed to it behind its back.
                // A source whose spec is a bare name - `"cpu": "cpu"`, `"up": "uptime"` - reads one of the
                // machine's own numbers, so declaring it is how an applet says which of them it wants. Seeding
                // these made the driver and `g13 applet preview` disagree about the same applet: the driver
                // resolved `{cpu}` while the preview drew the word.
                // once per interval everything is read; between times only what moves with the pad
                let (values, own_problems) = match kept {
                    Some(kept) => g13_applets::refresh_fast(&applet, kept, world)
                        .unwrap_or_else(|| (kept.clone(), Vec::new())),
                    None => g13_applets::gather(&applet, world),
                };
                problems.extend(own_problems);
                for name in g13_applets::values_nothing_provides(&applet, &values) {
                    problems.push(format!(
                        "{name} is read by a widget and nothing provides it, so it is drawn as itself"
                    ));
                }
                let mut drawing = Vec::new();
                // The number comes from the pad and the applet comes from a file, so they can disagree: a screen
                // chosen in one applet means nothing in the next. Clamped here, where the count is known, so no
                // number can draw a blank screen.
                let count = g13_applets::screen_count(&applet);
                let screen = if count > 1 { screen % count } else { 0 };
                // the pad's own selection travels with the moment: an applet's values are exactly the ones it
                // declares, so a border read out of them would never be drawn
                g13_applets::draw_screen_at(
                    &applet,
                    screen,
                    g13_applets::Context {
                        selected: Some(moment.state.screen_selected),
                        // the drawing's own clock, never a reading: a picture that moves must not take its time
                        // from a value, which is the fault that made a scroll stand still
                        now_seconds: steady_seconds(),
                        // so an alert may ask `{"profile": 2}`
                        profile: moment.profile,
                    },
                    &mut frame,
                    &values,
                    &mut drawing,
                );
                problems.extend(drawing);
                return (frame, problems, Some(values));
            }
            Err(problem) => problems.push(format!("{visual}: {problem}")),
        }
        return (frame, problems, None);
    }

    match visual {
        "clock" => draw_clock(&mut frame, moment),
        "system" => draw_system(&mut frame, moment, world),
        "pad" => draw_pad(&mut frame, moment),
        // the machine's numbers as somebody else read them, when somebody did: they cost 51ms to gather and this
        // is the drawing thread
        "media" => match machine {
            Some(known) => draw_media(&mut frame, known),
            None => draw_media(&mut frame, &built_in_values(moment, world)),
        },
        other => problems.push(format!(
            "{other} is not a visual this build draws (it knows applet:<name>, clock, system and pad)"
        )),
    }
    (frame, problems, None)
}

/// What a `follow` applet asks for, worked out once per edit of its own file.
///
/// The loop asks this on every pass, and reading and parsing every enabled applet each time - one of them is 73 KB
/// and costs 0.35 ms to parse, measured - answers the same question over and over. A `follow` and a set of source
/// paths change only when the applet file changes, and the file's *stamp* says when: that, and the stamps of the
/// files behind the sources, are the only things worth asking again. Both are stats.
#[derive(Clone, Debug, Default)]
struct Fed {
    /// The window it keeps the screen for, or none when it does not follow.
    follow: Option<g13_applets::Follow>,
    /// The files behind its own sources: one of these being written means the program is running.
    files: Vec<std::path::PathBuf>,
}

/// Each applet's `follow`, with the stamp of the file it was read from.
#[derive(Debug, Default)]
pub struct FollowCache {
    /// By applet name: the stamp of the file it was read from, and what was read from it.
    known: std::collections::BTreeMap<String, (Option<std::time::SystemTime>, Fed)>,
}

impl FollowCache {
    /// An empty cache: nothing is known until an applet is first asked about.
    pub fn new() -> Self {
        Self::default()
    }

    /// What this applet asks for, reading its file only when it has changed since last time.
    fn of(&mut self, applets: &Path, name: &str) -> Fed {
        let path = applets.join(format!("{name}.json"));
        let stamp = std::fs::metadata(&path)
            .and_then(|data| data.modified())
            .ok();
        if let Some((known, fed)) = self.known.get(name) {
            if *known == stamp {
                return fed.clone();
            }
        }
        let fed = read_applet(applets, name)
            .map(|applet| Fed {
                files: applet
                    .sources
                    .values()
                    .filter_map(|spec| g13_sources::file_behind(spec))
                    .collect(),
                follow: applet.follow,
            })
            .unwrap_or_default();
        self.known.insert(name.to_string(), (stamp, fed.clone()));
        fed
    }

    /// Which of these applets are being fed right now.
    fn live(
        &mut self,
        enabled: &[String],
        applets: &Path,
        now: std::time::SystemTime,
    ) -> Vec<String> {
        let mut live = Vec::new();
        for visual in enabled {
            let name = visual.strip_prefix("applet:").unwrap_or(visual);
            let fed = self.of(applets, name);
            let Some(follow) = fed.follow else {
                continue;
            };
            if follow.seconds <= 0.0 {
                continue;
            }
            if fed
                .files
                .iter()
                .any(|file| fresh(file, follow.seconds, now))
            {
                live.push(visual.clone());
            }
        }
        live
    }

    /// The window this applet keeps the screen for, or none when it does not follow one.
    ///
    /// Read beside `live`, and for the same reason: the applet's own file is what says how long silence has to
    /// last before "it stopped" is the right reading of it.
    fn window(&mut self, applets: &Path, visual: &str) -> f64 {
        let name = visual.strip_prefix("applet:").unwrap_or(visual);
        self.of(applets, name)
            .follow
            .map(|follow| follow.seconds)
            .unwrap_or(0.0)
    }
}

/// Whether an applet that is not being fed right now has been quiet for longer than its own window.
///
/// Measured from the last *sighting*, which is the whole point: a file that is momentarily absent - a program
/// replacing it - is not a program that stopped, while one that has really gone quiet is handed back on time
/// and a file that disappears for good is still given up, one window after it was last seen.
fn quiet_for(last_fed: std::time::SystemTime, now: std::time::SystemTime, seconds: f64) -> bool {
    now.duration_since(last_fed)
        .map(|since| since.as_secs_f64())
        .unwrap_or(0.0)
        > seconds
}

/// Whether a file was written inside the window: a file being written *is* a program running.
fn fresh(file: &Path, seconds: f64, now: std::time::SystemTime) -> bool {
    let Ok(modified) = std::fs::metadata(file).and_then(|data| data.modified()) else {
        return false;
    };
    // a file written "in the future" (the clock moved) counts as fresh, which is the safe way round: it is better
    // to show a game's screen than to hide it
    now.duration_since(modified)
        .map(|since| since.as_secs_f64())
        .unwrap_or(0.0)
        <= seconds
}

/// Which applets are being fed right now.
///
/// An applet says it is fed from outside with `follow`, and "the program is running" is read as "the file behind
/// one of its own sources is being written" - a file being written *is* a program running. Nothing is scanned for
/// processes, nothing needs to be taught about any particular game, and Wine is no different from native.
/// For a loop that asks over and over, keep a [`FollowCache`]: this reads every applet it is asked about, and the
/// loop reads them on every pass.
pub fn live_applets(enabled: &[String], applets: &Path, now: std::time::SystemTime) -> Vec<String> {
    FollowCache::new().live(enabled, applets, now)
}

/// An applet's file, parsed, or none when it cannot be read or will not parse.
fn read_applet(applets: &Path, name: &str) -> Option<g13_applets::Applet> {
    let text = std::fs::read_to_string(applets.join(format!("{name}.json"))).ok()?;
    g13_applets::parse_applet(&text).ok()
}

/// What an applet asks to follow, or none when it does not follow at all.
fn follow_of(applets: &Path, name: &str) -> Option<g13_applets::Follow> {
    read_applet(applets, name).and_then(|applet| applet.follow)
}

/// What the pad should come up on, when the file names a screen that only makes sense while its program runs.
///
/// Asked once at start. Without it, a driver restarted after a game was closed would come up on the game's screen
/// with stale numbers on it and stay there. The answer is the first enabled visual that does not ask to follow.
pub fn start_on(
    enabled: &[String],
    active: &str,
    applets: &Path,
    now: std::time::SystemTime,
) -> Option<String> {
    let name = active.strip_prefix("applet:").unwrap_or(active);
    // an ordinary screen: the file is the truth and the pad comes up on what it names
    follow_of(applets, name)?;
    if live_applets(enabled, applets, now).contains(&active.to_string()) {
        // its program is running, so it is exactly where it should be
        return None;
    }
    // it asks to follow and its program is not running: come up on something that does not depend on it
    enabled
        .iter()
        .find(|visual| {
            let other = visual.strip_prefix("applet:").unwrap_or(visual);
            visual.as_str() != active && follow_of(applets, other).is_none()
        })
        .cloned()
}

/// An applet fed by a running program, and what it took.
#[derive(Clone, Debug, PartialEq)]
struct Taken {
    /// The applet that is on the screen.
    applet: String,
    /// What was showing before it, to hand back.
    previous: String,
    /// The profile that was in force before it, to hand back.
    profile_before: u32,
    /// The binding set it asked for, once it could be found.
    profile_set: Option<u32>,
    /// When this applet was last *seen* being fed, which is what "it has stopped" is measured from.
    ///
    /// A sighting, not this pass's reading of the file: a writer that replaces its file leaves the path missing
    /// for a moment, and a moment is not a stop.
    last_fed: std::time::SystemTime,
}

/// What the loop should do about the screen and the profile this pass.
#[derive(Clone, Debug, PartialEq)]
pub enum Wanted {
    /// Nothing to do.
    Nothing,
    /// A program started: show this applet, and make this binding set (by the name the applet gives it) active.
    Take {
        /// The applet to show, by the name it is enabled under.
        visual: String,
        /// The binding set the applet names, when it names one.
        profile: Option<String>,
    },
    /// It stopped: hand the screen back, and put the profile back if it is still the one that was taken.
    Give {
        /// The screen to hand back, when it is not the one that is showing. `None` leaves it alone.
        visual: Option<String>,
        /// The profile to put back, when this is what changed it. `None` leaves it alone.
        profile: Option<u32>,
    },
}

/// Lets an applet fed by a running program take the screen, and hand it back.
///
/// While the file behind one of the applet's own sources is being written it is what is on the screen - and if the
/// applet names a binding set, that set is in force too. When the writing stops, both go back to what they were.
///
/// Choosing something else while it is live is respected: this saves a button press, it does not fight you. And
/// following starts again the next time the program runs.
#[derive(Debug, Default)]
pub struct Follower {
    /// The applet that is on the screen because of this, and what it took, while one is live.
    following: Option<Taken>,
    /// What each applet asks for, read once per edit of its file rather than on every pass.
    cache: FollowCache,
    /// You picked a screen yourself while it was live, so the screen is not ours to change until it goes quiet.
    suppressed: bool,
}

impl Follower {
    /// A follower that is following nothing, which is how the driver starts.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether an applet is on the screen because of this.
    pub fn following(&self) -> Option<&str> {
        self.following.as_ref().map(|taken| taken.applet.as_str())
    }

    /// A screen was chosen for a reason that was not this: leave it alone.
    ///
    /// Only if it is not the applet we are following - choosing the game's own screen is not a disagreement.
    pub fn user_chose(&mut self, visual: &str) {
        let ours = self
            .following
            .as_ref()
            .map(|taken| taken.applet.as_str() == visual)
            .unwrap_or(false);
        if self.following.is_some() && !ours {
            self.suppressed = true;
        }
    }

    /// What should happen now, given what is enabled, what is showing, and the clock.
    pub fn wanted(
        &mut self,
        enabled: &[String],
        active: &str,
        applets: &Path,
        profile: u32,
        now: std::time::SystemTime,
    ) -> Wanted {
        let live = self.cache.live(enabled, applets, now);

        // It went quiet: hand back what was taken.
        //
        // Quiet is measured from the last time the applet was *seen* being fed, not from this pass's reading of
        // its file, and that is the whole of a fault worth naming here. The mod that feeds `applet:cp2077-hud`
        // replaces `hud.json` rather than writing into it, so the path is missing for a few milliseconds each
        // time - measured at 4 samples in 200, ten milliseconds apart, while the game was running. Read as "the
        // program stopped", the screen was handed back and taken again on the next pass: the pad flickering
        // between two applets every ten seconds or so, 115 times in thirteen minutes.
        //
        // A sighting refreshes the clock; only silence longer than the applet's own window is a stop. So a blink
        // holds, a game that quits still hands back on time, and a file that disappears for good is still given
        // up - `seconds` after the last time it was really there.
        if let Some(mut taken) = self.following.clone() {
            if live.contains(&taken.applet) {
                taken.last_fed = now;
                self.following = Some(taken);
            } else if quiet_for(
                taken.last_fed,
                now,
                self.cache.window(applets, &taken.applet),
            ) {
                self.following = None;
                self.suppressed = false;
                // the screen goes back to what it was, unless the applet is still the one showing - and the
                // profile only if *this* is what changed it, so a keypress of yours is not undone
                let visual = (active != taken.previous).then(|| taken.previous.clone());
                let profile = (taken.profile_set == Some(profile)).then_some(taken.profile_before);
                return Wanted::Give { visual, profile };
            } else {
                // Not fed this pass, and not quiet yet: the screen stays where it is. Without this the pass fell
                // through to the "nothing is being fed" branch below, which forgets the applet - so the file
                // being replaced for one pass was still a hand-back, just a silent one that the next pass undid.
                self.following = Some(taken);
                return Wanted::Nothing;
            }
        }

        if live.is_empty() {
            self.following = None;
            self.suppressed = false;
            return Wanted::Nothing;
        }
        if self.following.is_some() || self.suppressed {
            return Wanted::Nothing;
        }
        let visual = live[0].clone();
        // already showing it: remember it, so it can be handed back later
        if active == visual {
            self.following = Some(Taken {
                applet: visual,
                previous: active.to_string(),
                profile_before: profile,
                profile_set: None,
                last_fed: now,
            });
            return Wanted::Nothing;
        }
        let profile_named = follow_of(applets, visual.strip_prefix("applet:").unwrap_or(&visual))
            .and_then(|follow| follow.profile);
        self.following = Some(Taken {
            applet: visual.clone(),
            previous: active.to_string(),
            profile_before: profile,
            profile_set: None,
            last_fed: now,
        });
        Wanted::Take {
            visual,
            profile: profile_named,
        }
    }

    /// The profile the loop settled on, so the hand-back can tell whether it is still ours to undo.
    pub fn took_profile(&mut self, profile: Option<u32>) {
        if let Some(taken) = self.following.as_mut() {
            taken.profile_set = profile;
        }
    }
}

#[cfg(test)]
mod steady_clock_tests {
    use super::*;

    #[test]
    fn a_running_text_moves_between_frames_even_when_the_clock_value_is_minutes_only() {
        // the reported fault, exactly: `time` is published as `14:27`, so an offset worked out from it is
        // constant for a whole minute. Two frames taken 60ms apart must not be identical.
        let dir = std::env::temp_dir().join("g13-steady-clock-test");
        let _ = std::fs::create_dir_all(&dir);
        crate::write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("runner.json"),
            r#"{"name": "runner", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "a long title that has to run", "scroll_width": 12, "scroll": true, "scroll_speed": 30}]}"#,
        )
        .unwrap();
        let mut state = crate::State::new(0, &crate::Bindings::from_text(""), Vec::new());
        state.screen_selected = 0;
        let moment = Moment {
            state: &state,
            clock: "14:27".to_string(),
            date: "17 Sep".to_string(),
            day: "Wednesday".to_string(),
            profile: 0,
            button_mode: "auto".to_string(),
        };
        let world = World::default();
        let (first, _, _) =
            render_screen_from("applet:runner", 0, &moment, &world, &dir, None, None);
        std::thread::sleep(std::time::Duration::from_millis(60));
        let (second, _, _) =
            render_screen_from("applet:runner", 0, &moment, &world, &dir, None, None);
        assert_ne!(
            first, second,
            "the picture did not change in 60ms: the drawing is running off a reading again"
        );
    }
}

/// Whether the screen being shown has anything on it that moves by itself.
///
/// A scrolling text does. Its reading does not have to be re-read any faster than the applet's own interval, but
/// the *drawing* does: at one frame a second a scroll is a slideshow, and four characters a step is the juddering
/// that the eye reads as the speed pulsing. So a screen with a scroll on it is drawn at the pad's own rate instead.
pub fn fast_draw_for(visual: &str, screen: usize, applets: &Path) -> bool {
    let Some(name) = visual.strip_prefix("applet:") else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(applets.join(format!("{name}.json"))) else {
        return false;
    };
    let Ok(applet) = g13_applets::parse_applet(&text) else {
        return false;
    };
    let (faces, _) = g13_applets::fonts_cached(&fonts_beside(applets));
    let applet = g13_applets::with_theme(
        g13_applets::with_font(g13_applets::with_bitmaps(applet, applets), &faces),
        &themes_dir(applets),
    );
    let count = g13_applets::screen_count(&applet);
    let screen = if count > 1 { screen % count } else { 0 };
    // asked of the applet, not of the widget kinds here: a scroll, a widget that blinks or types, and a theme
    // with a scanline or a glitch are all answered in one place
    applet.moves(screen)
}

/// Where the fonts live: beside the applets, as the themes are.
pub fn fonts_dir(applets: &Path) -> std::path::PathBuf {
    applets.parent().unwrap_or(applets).join("fonts")
}

/// Where the fonts live, for the drawing paths inside this module: the same place as `fonts_dir`.
fn fonts_beside(applets: &Path) -> std::path::PathBuf {
    applets.parent().unwrap_or(applets).join("fonts")
}

/// Where the themes live: beside the applets, so the two folders read the same way.
pub fn themes_dir(applets: &Path) -> std::path::PathBuf {
    applets.parent().unwrap_or(applets).join("themes")
}

/// The published values, read from the driver's own file.
///
/// A macro that asks about a value reads the same file an applet does, so the answer is the number the rest of
/// the system is showing rather than a second opinion. A file that is not there, or not readable, gives no
/// values at all: a macro that asks about one then gets "unknown", which is not a comparison and is not a zero.
/// The published values, from the file the driver writes.
pub fn read_published_values() -> BTreeMap<String, g13_values::Value> {
    read_published_values_from(&values_path())
}

/// The same, from a file of the caller's choosing, so a test can exercise what the driver does with one.
pub fn read_published_values_from(path: &Path) -> BTreeMap<String, g13_values::Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&text)
    else {
        return BTreeMap::new();
    };
    map.iter()
        .map(|(name, value)| {
            let value = match value {
                serde_json::Value::Number(number) => {
                    g13_values::Value::Number(number.as_f64().unwrap_or_default())
                }
                serde_json::Value::String(text) => g13_values::Value::Text(text.clone()),
                serde_json::Value::Null => g13_values::Value::Missing,
                other => g13_values::Value::Text(other.to_string()),
            };
            (name.clone(), value)
        })
        .collect()
}

/// How long to wait before drawing a visual again.
pub fn interval_for(visual: &str, applets: &Path) -> Duration {
    if let Some(name) = visual.strip_prefix("applet:") {
        if let Ok(text) = std::fs::read_to_string(applets.join(format!("{name}.json"))) {
            if let Ok(applet) = g13_applets::parse_applet(&text) {
                let seconds = applet.interval.clamp(0.1, 60.0);
                return Duration::from_secs_f64(seconds);
            }
        }
        return Duration::from_secs(1);
    }
    // the machine's own numbers move once a second, and a clock twice as often is pointless
    Duration::from_secs(1)
}

#[cfg(test)]
mod fast_draw_tests {
    use super::*;

    #[test]
    fn a_screen_with_something_running_on_it_is_drawn_at_the_pads_own_rate() {
        // the draw rate asks this rather than the applet telling it, so nothing new has to be learned to
        // make a scroll smooth. A screen with no scroll is drawn exactly as often as before.
        let dir = std::env::temp_dir().join("g13-fast-draw-test");
        let _ = std::fs::create_dir_all(&dir);
        crate::write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("scroller.json"),
            r#"{"name": "scroller", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "a long title indeed", "scroll_width": 10, "scroll": true}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("plain.json"),
            r#"{"name": "plain", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "a long title indeed", "scroll_width": 10}]}"#,
        )
        .unwrap();
        assert!(fast_draw_for("applet:scroller", 0, &dir));
        assert!(!fast_draw_for("applet:plain", 0, &dir));
        // a theme with motion in it moves the screen without any widget saying so
        std::fs::create_dir_all(dir.parent().unwrap().join("themes")).unwrap();
        std::fs::write(
            dir.parent().unwrap().join("themes").join("cyber.json"),
            r#"{"scanline": 60}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("themed.json"),
            r#"{"name": "themed", "theme": "cyber", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "still"}]}"#,
        )
        .unwrap();
        assert!(
            fast_draw_for("applet:themed", 0, &dir),
            "a theme with a scanline in it is motion: the screen must be drawn faster than it is read"
        );
        // and a widget that blinks, with a theme that says nothing at all
        std::fs::write(
            dir.join("warning.json"),
            r#"{"name": "warning", "theme": "cyber", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "ALERT", "animate": "blink"}]}"#,
        )
        .unwrap();
        assert!(fast_draw_for("applet:warning", 0, &dir));
        assert!(!fast_draw_for("applet:not-there", 0, &dir));
        // the machine's own visuals are drawn once a second and nothing on them moves
        assert!(!fast_draw_for("clock", 0, &dir));
    }
}

/// The record wizard: which macro, which stage, and what to press.
///
/// Five rows at eight pixels each is what the visible part of the screen holds, so this is the whole of it.
/// The words are the terminal's words, from the same state: the two cannot drift apart.
fn draw_recording(frame: &mut Frame, wizard: &Wizard) {
    frame.text_line(0, "RECORDING");
    frame.text_line(1, &clip(&format!("macro {}", wizard.id), TEXT_COLUMNS));
    match wizard.stage {
        Stage::Choosing => {
            // The profile key that answered is named beside the profile. Pressing the key of the profile
            // already in force changes nothing else, so without this a press that was taken cannot be told
            // apart from one that never arrived.
            let profile = match wizard.picked.as_deref() {
                Some(key) => format!("profile {} ({key})", wizard.profile),
                None => format!("profile {}", wizard.profile),
            };
            frame.text_line(2, &clip(&profile, TEXT_COLUMNS));
            frame.text_line(3, "M1 M2 M3 picks it");
            frame.text_line(4, "then the control");
        }
        Stage::Capturing => {
            let control = wizard.control.clone().unwrap_or_default();
            frame.text_line(
                2,
                &clip(
                    &format!("{control} in profile {}", wizard.profile),
                    TEXT_COLUMNS,
                ),
            );
            frame.text_line(3, "press the keys");
            frame.text_line(4, "MR ends it");
        }
    }
}

/// The time, day and date, filling the screen a `clock` visual has to itself.
fn draw_clock(frame: &mut Frame, moment: &Moment<'_>) {
    frame.text(4, 4, &moment.clock);
    frame.text(4, 20, &moment.day);
    frame.text(4, 32, &moment.date);
}

/// CPU, memory and uptime, with a bar each for the two shares that have one.
fn draw_system(frame: &mut Frame, moment: &Moment<'_>, world: &World) {
    let values = built_in_values(moment, world);
    frame.text(2, 2, &format!("CPU {}", values["cpu"].formatted(":0f")));
    frame.text(2, 12, &format!("MEM {}", values["memory"].formatted(":0f")));
    frame.text(2, 22, &format!("UP {}", values["uptime"].text()));
    frame.text(2, 32, &format!("{} {}", moment.day, moment.date));
    bar_for(frame, &values["cpu"], 60, 2, 90, 7);
    bar_for(frame, &values["memory"], 60, 12, 90, 7);
}

/// What is playing: title, artist, and how far through. The layout is this build's own, because the visual
/// has no file to follow.
fn draw_media(frame: &mut Frame, values: &BTreeMap<String, Value>) {
    let status = values["media_status"].text();
    if status.is_empty() {
        frame.text(6, 16, "nothing playing");
        return;
    }
    frame.text(2, 2, &status);
    frame.text(2, 14, &clip(&values["media_title"].text(), 26));
    frame.text(2, 26, &clip(&values["media_artist"].text(), 26));
    let position = values["media_position"].text();
    let duration = values["media_duration"].text();
    frame.text(2, 34, &format!("{position} / {duration}"));
    bar_for(frame, &values["media_percent"], 78, 34, 78, 5);
}

/// Shorten text to a number of characters, marking that it was shortened.
fn clip(text: &str, width: usize) -> String {
    let characters: Vec<char> = text.chars().collect();
    if characters.len() <= width {
        return text.to_string();
    }
    let mut out: String = characters[..width.saturating_sub(1)].iter().collect();
    out.push('\u{2026}');
    out
}

/// The profile in force, the button mode, and the last control the pad sent.
fn draw_pad(frame: &mut Frame, moment: &Moment<'_>) {
    frame.text(2, 2, &format!("PROFILE {}", moment.profile));
    frame.text(2, 12, &format!("MODE {}", moment.button_mode));
    let last = moment
        .state
        .last_keys
        .last()
        .map(|(control, pressed, _)| format!("{control} {}", if *pressed { "down" } else { "up" }))
        .unwrap_or_else(|| "nothing yet".to_string());
    frame.text(2, 22, &format!("LAST {last}"));
}

/// Fill the share of a bar a percentage covers, and nothing at all when the value is not a number.
fn bar_for(frame: &mut Frame, value: &Value, x: usize, y: usize, width: usize, height: usize) {
    let Some(share) = value.number() else {
        return;
    };
    let filled = ((share / 100.0) * width as f64)
        .round()
        .clamp(0.0, width as f64) as usize;
    frame.fill(x, y, filled, height, true);
}

/// The values an applet or a built-in visual can use without declaring them, and the ones published.
pub fn built_in_values(moment: &Moment<'_>, world: &World) -> BTreeMap<String, Value> {
    let mut values = BTreeMap::new();
    // The driver's own readings, read by the driver for its own visuals and published for anything that wants
    // them. They are not names an applet can use: an applet reads this file with `json:` and says so.
    let mut gathered: Vec<&str> = g13_sources::BUILT_IN_NAMES.to_vec();
    // two names that are not in the list above because only this function asks for them
    gathered.extend(["media_duration", "media_percent"]);
    for name in gathered {
        values.insert(name.to_string(), g13_sources::built_in_value(name, world));
    }
    // The seconds this world's clock was read at. It used to be parsed back out of the `HH:MM` display string,
    // which meant a value claiming seconds could only ever be a whole number of minutes.
    values.insert(
        "time_seconds".into(),
        Value::Number(world.clock.seconds as f64),
    );
    values.insert("uptime_seconds".into(), uptime_seconds(world));
    values.insert("profile".into(), Value::Number(moment.profile as f64));
    // counted from one, because a person reads it: the first thing a screen can act on is 1. This is the name
    // an applet declares to read it; the pad's own border is handed to the drawing directly.
    values.insert(
        g13_applets::SELECTED_VALUE.into(),
        Value::Number(moment.state.screen_selected as f64 + 1.0),
    );
    values.insert(
        "screen_item".into(),
        Value::Text(moment.state.screen_item.clone()),
    );
    values.insert(
        "button_mode".into(),
        Value::Text(moment.button_mode.clone()),
    );
    values.insert(
        "recording".into(),
        if moment.state.recording.is_some() {
            Value::Text("1".into())
        } else {
            Value::Text("0".into())
        },
    );
    values.insert(
        "last_key".into(),
        Value::Text(
            moment
                .state
                .last_keys
                .last()
                .map(|(control, _, _)| control.clone())
                .unwrap_or_default(),
        ),
    );
    values.insert(
        "recent_keys".into(),
        Value::Text(
            moment
                .state
                .last_keys
                .iter()
                .rev()
                .take(8)
                .map(|(control, pressed, _)| {
                    format!("{control}{}", if *pressed { "+" } else { "-" })
                })
                .collect::<Vec<_>>()
                .join(" "),
        ),
    );
    values.insert("screen_width".into(), Value::Number(WIDTH as f64));
    values.insert("screen_height".into(), Value::Number(VISIBLE_HEIGHT as f64));
    values.insert("screen_owner".into(), Value::Text(screen_owner()));
    // keys the previous stack published that this build does not know yet: present, and empty, so anything
    // reading the file finds the key it expects rather than a surprise
    for name in [
        "media_title",
        "media_artist",
        "media_album",
        "media_status",
        "media_position",
        "media_duration",
        "media_percent",
        "sdk_client",
    ] {
        values
            .entry(name.to_string())
            .or_insert_with(|| Value::Text(String::new()));
    }
    values
}

/// Write the values the previous stack published, under the same names.
/// Who owns the screen: the driver, or a program that has taken it.
///
/// The predecessor had `g13-buttons` for this - `auto`, `visuals`, `sdk` - and it is the last of the parity gaps the
/// audit found. Ours is a file rather than a running process's memory, for the same reason everything else here is:
/// the driver, the window and any program can all see it without being told.
///
/// The driver is the owner unless somebody says otherwise, which is what a screen with nothing else running should be.
pub const DRIVER_OWNS_IT: &str = "g13 run";

/// Where the owner is written: beside the configuration, so it survives a restart and can be read by anything.
pub fn screen_owner_path() -> PathBuf {
    g13_config::config_dir().join("screen-owner")
}

/// Who owns the screen now. No file, or an empty one, means the driver does.
pub fn screen_owner() -> String {
    std::fs::read_to_string(screen_owner_path())
        .map(|text| text.trim().to_string())
        .ok()
        .filter(|owner| !owner.is_empty())
        .unwrap_or_else(|| DRIVER_OWNS_IT.to_string())
}

/// Take the screen for a program, or give it back with `None` or an empty name.
///
/// Written at once, because the driver re-reads it as it draws - there is nothing to restart.
pub fn set_screen_owner(owner: Option<&str>) -> std::io::Result<()> {
    let path = screen_owner_path();
    match owner.map(str::trim).filter(|owner| !owner.is_empty()) {
        Some(owner) => std::fs::write(path, format!("{owner}\n")),
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
    }
}

/// Write the gathered values where the window and the CLI read them.
pub fn publish_values(values: &BTreeMap<String, Value>, path: &Path) -> std::io::Result<()> {
    let mut out = String::from("{\n");
    let mut first = true;
    let mut keys: Vec<&String> = values.keys().collect();
    keys.sort();
    for key in keys {
        if !first {
            out.push_str(",\n");
        }
        first = false;
        let value = &values[key];
        let rendered = match value {
            Value::Number(number) => number.to_string(),
            Value::Missing => "null".to_string(),
            // Escaped by serde rather than by hand. Doing it by hand covered backslashes and quotes and
            // missed control characters, so a value holding a newline, a tab or a NUL produced a file that
            // could not be parsed at all - and the reader that failed was the window.
            Value::Text(text) => serde_json::Value::String(text.clone()).to_string(),
        };
        out.push_str(&format!("  \"{key}\": {rendered}"));
    }
    out.push_str("\n}\n");
    // written beside the real file and moved into place, because a reader refreshing as fast as this does
    // can otherwise catch an empty or half-written file and lose that frame
    g13_files::write(path, &out).map_err(std::io::Error::other)
}

/// Where the values file lives, alongside the other runtime files.
pub fn values_path() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let _ = user;
    Path::new(&dir).join("g13-values.json")
}

/// Seconds since this process started, steady and with no resolution to speak of.
///
/// A drawing that moves by itself must not take its time from a *reading*. It did: the scroll used
/// `seconds_of_day(moment.clock)`, and the published `time` is `14:27` - hours and minutes - so the offset was
/// constant for a whole minute and then jumped eighteen hundred pixels, so a title did not appear to move at
/// all and read as chunky. Time for a picture is the machine's own clock, sub-millisecond and hard to make
/// stutter, and no value in any file can change its resolution.
fn steady_seconds() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
}

/// The machine's uptime as a number, from `proc/uptime` under the world's root rather than the `UP` text.
fn uptime_seconds(world: &World) -> Value {
    match std::fs::read_to_string(world.root.join("proc/uptime")) {
        Ok(text) => Value::Number(
            text.split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0),
        ),
        Err(_) => Value::Missing,
    }
}

#[cfg(test)]
mod tests {

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn the_next_screen_is_the_next_one_in_the_list_and_wraps_round() {
        let list = names(&["clock", "applet:gpu", "system"]);
        assert_eq!(
            step_visual(&list, "clock", 1).as_deref(),
            Some("applet:gpu")
        );
        assert_eq!(
            step_visual(&list, "applet:gpu", 1).as_deref(),
            Some("system")
        );
        // round the end, which is the case a person notices
        assert_eq!(step_visual(&list, "system", 1).as_deref(), Some("clock"));
        assert_eq!(step_visual(&list, "clock", -1).as_deref(), Some("system"));

        // one entry means staying on it rather than nothing happening at all
        let one = names(&["system"]);
        assert_eq!(step_visual(&one, "system", 1).as_deref(), Some("system"));

        // a visual showing that is not in the list: the first one, whichever way we were going
        assert_eq!(step_visual(&list, "media", 1).as_deref(), Some("clock"));
        assert_eq!(step_visual(&list, "media", -1).as_deref(), Some("clock"));

        // nothing enabled is nothing to show, and says so by returning nothing
        assert_eq!(step_visual(&[], "clock", 1), None);
    }

    #[test]
    fn asking_for_the_next_screen_does_what_the_directions_say() {
        let dir = std::env::temp_dir().join(format!("g13-screen-target-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        std::fs::write(dir.join("gpu.json"), "{}").unwrap();
        let list = names(&["clock", "applet:gpu", "system"]);

        assert_eq!(
            screen_target("next", &list, "clock", &dir)
                .unwrap()
                .as_deref(),
            Some("applet:gpu")
        );
        assert_eq!(
            screen_target("prev", &list, "clock", &dir)
                .unwrap()
                .as_deref(),
            Some("system")
        );
        // a name, which is the other way to say it
        assert_eq!(
            screen_target("applet:gpu", &list, "clock", &dir)
                .unwrap()
                .as_deref(),
            Some("applet:gpu")
        );
        // asking for what is already showing is nothing to do, and nothing gets written
        assert_eq!(screen_target("clock", &list, "clock", &dir).unwrap(), None);
        assert_eq!(
            screen_target("next", &names(&["clock"]), "clock", &dir).unwrap(),
            None
        );

        // nothing enabled, and a name that cannot be drawn: both are said, and neither changes the screen
        let problem = screen_target("next", &[], "clock", &dir).unwrap_err();
        assert!(problem.contains("nothing to change to"), "{problem}");
        let problem = screen_target("applet:nope", &list, "clock", &dir).unwrap_err();
        assert!(problem.contains("applet:nope"), "{problem}");
        assert!(problem.contains("not a visual"), "{problem}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_number_names_one_of_the_screens_in_the_rotation() {
        let dir = std::env::temp_dir().join(format!("g13-numbered-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        let list = names(&["clock", "media", "pad"]);

        // counting from 1, and asking for the one already showing is nothing to do
        assert_eq!(
            screen_target("1", &list, "pad", &dir).unwrap().as_deref(),
            Some("clock")
        );
        assert_eq!(
            screen_target("3", &list, "clock", &dir).unwrap().as_deref(),
            Some("pad")
        );
        assert_eq!(screen_target("3", &list, "pad", &dir).unwrap(), None);

        // out of range says how many there are, rather than doing nothing quietly
        let problem = screen_target("4", &list, "clock", &dir).unwrap_err();
        assert!(problem.contains("4"), "{problem}");
        assert!(problem.contains("the rotation has 3"), "{problem}");
        assert!(problem.contains("clock, media, pad"), "{problem}");
        // and zero is not a screen
        let problem = screen_target("0", &list, "clock", &dir).unwrap_err();
        assert!(problem.contains("0"), "{problem}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_rotation_steps_over_a_screen_this_build_cannot_draw() {
        // A hand-written file: `custom` is a screen the previous stack had and this one does not, sitting in the
        // middle of the rotation. Landing on it and stopping there is what "LR got stuck" was.
        let dir = std::env::temp_dir().join(format!("g13-skip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        let list = names(&["clock", "custom", "pad"]);

        assert_eq!(
            screen_target("next", &list, "clock", &dir)
                .unwrap()
                .as_deref(),
            Some("pad"),
            "the rotation stopped on a screen that cannot be drawn"
        );
        // and it wraps round past it, from either side
        assert_eq!(
            screen_target("next", &list, "pad", &dir)
                .unwrap()
                .as_deref(),
            Some("clock")
        );
        assert_eq!(
            screen_target("prev", &list, "pad", &dir)
                .unwrap()
                .as_deref(),
            Some("clock")
        );
        assert_eq!(
            screen_target("prev", &list, "clock", &dir)
                .unwrap()
                .as_deref(),
            Some("pad")
        );

        // asking for it by name is still refused, because that is a person naming a screen rather than a
        // rotation stepping past one
        let problem = screen_target("custom", &list, "clock", &dir).unwrap_err();
        assert!(problem.contains("custom"), "{problem}");

        // nothing drawable at all says so rather than doing nothing
        let problem = screen_target("next", &names(&["custom"]), "clock", &dir).unwrap_err();
        assert!(
            problem.contains("not one this build can draw")
                || problem.contains("nothing to change to"),
            "{problem}"
        );

        // and the note the driver says once names it
        let notes = undrawable_in(&list, &dir);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("custom"), "{notes:?}");
        assert!(undrawable_in(&names(&["clock", "pad"]), &dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pointing_at_a_visual_leaves_the_rest_of_the_file_exactly_as_it_was() {
        let dir = std::env::temp_dir().join(format!("g13-visual-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        let path = dir.join("visuals.json");
        // a file with every key the window writes, and one this build does not know
        std::fs::write(
            &path,
            r#"{"enabled": ["clock", "media"], "active": "clock", "cycle": true, "cycle_seconds": 30,
                "something_from_the_future": "kept"}"#,
        )
        .unwrap();

        write_active_visual(&path, "media").expect("wrote it");
        let text = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["active"], "media");
        // nothing else moved: not the rotation, not the cycling, not a key this build has never heard of
        assert_eq!(value["enabled"][0], "clock");
        assert_eq!(value["cycle"], true);
        assert_eq!(value["cycle_seconds"], 30);
        assert_eq!(value["something_from_the_future"], "kept");

        // and it reads back as the driver would read it
        let visuals = load_visuals(&path);
        assert_eq!(visuals.active, "media");
        assert_eq!(visuals.enabled, names(&["clock", "media"]));

        // a file that is not JSON is refused rather than replaced: the screen is better off as it was
        std::fs::write(&path, "not json at all").unwrap();
        let problem = write_active_visual(&path, "clock").expect_err("refused");
        assert!(problem.contains("not JSON"), "{problem}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json at all");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pads_selection_reaches_the_frame_the_driver_draws() {
        // The border is a drawing input, and it was being read out of the applet's own values - which are exactly
        // the ones it declares, so nothing could put it there: `g13 run` showed the buttons working while no
        // border appeared. This is the test that would have caught it: the same applet, the same moment, two
        // different selections, two different frames.
        let dir = std::env::temp_dir().join(format!("g13-selection-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("audio.json"),
            r#"{"name": "audio", "sources": {}, "widgets": [
                 {"type": "text", "x": 4, "y": 2, "format": "Start/Pause", "command": "cmd:playerctl play-pause"},
                 {"type": "text", "x": 4, "y": 20, "format": "Stop", "command": "cmd:playerctl stop"}]}"#,
        )
        .unwrap();

        let draw_on = |selected: usize| {
            let mut state = crate::State::new(0, &crate::Bindings::from_text(""), Vec::new());
            state.screen_selected = selected;
            let moment = Moment {
                state: &state,
                clock: "00:00".to_string(),
                date: "1 Jan".to_string(),
                day: "Mon".to_string(),
                profile: 0,
                button_mode: "auto".to_string(),
            };
            render_screen("applet:audio", 0, &moment, &World::default(), &dir).0
        };
        let on_first = draw_on(0);
        let on_second = draw_on(1);
        assert_ne!(
            on_first, on_second,
            "the pad moved to the second thing to act on and the screen drew the same frame"
        );
        assert!(
            !on_first.is_blank(0, 0, g13_screen::WIDTH, g13_screen::VISIBLE_HEIGHT),
            "the applet drew nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_applets_bitmaps_are_not_offered_as_a_screen() {
        let dir = std::env::temp_dir().join(format!("g13-drawable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        std::fs::write(dir.join("audio.json"), r#"{"name": "audio"}"#).unwrap();
        std::fs::write(dir.join("audio.bitmaps.json"), "{}").unwrap();
        let names = drawable_visuals(&dir);
        assert!(names.contains(&"applet:audio".to_string()));
        assert!(
            !names.contains(&"applet:audio.bitmaps".to_string()),
            "an applet's pictures are offered as a screen: {names:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_screen_number_from_the_pad_cannot_draw_a_blank_screen() {
        let dir = std::env::temp_dir().join(format!("g13-screens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("two.json"),
            r#"{"name": "two", "sources": {}, "screens": [
                 {"title": "one", "widgets": [{"type": "text", "x": 2, "y": 2, "format": "FIRST"}]},
                 {"title": "two", "widgets": [{"type": "text", "x": 2, "y": 2, "format": "SECOND"}]}]}"#,
        )
        .unwrap();

        let state = crate::State::new(0, &crate::Bindings::from_text(""), Vec::new());
        let moment = Moment {
            state: &state,
            clock: "00:00".to_string(),
            date: "1 Jan".to_string(),
            day: "Mon".to_string(),
            profile: 0,
            button_mode: "auto".to_string(),
        };
        let world = World::default();
        let (first, _) = render_screen("applet:two", 0, &moment, &world, &dir);
        let (second, _) = render_screen("applet:two", 1, &moment, &world, &dir);
        assert_ne!(first, second, "both screens drew the same thing");
        assert!(
            !second.is_blank(0, 0, g13_screen::WIDTH, g13_screen::VISIBLE_HEIGHT),
            "screen 1 drew nothing"
        );

        // a number bigger than the applet has: it wraps rather than showing an empty screen, because the number
        // can come from another applet entirely
        let (wrapped, _) = render_screen("applet:two", 5, &moment, &world, &dir);
        assert_eq!(
            wrapped, second,
            "a number past the end did not land on a real screen"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_visuals_this_build_can_draw_are_its_own_plus_the_applets() {
        let dir = std::env::temp_dir().join(format!("g13-drawable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        for name in ["gpu", "docker"] {
            std::fs::write(dir.join(format!("{name}.json")), "{}").unwrap();
        }
        // something that is not an applet and must not appear as one
        std::fs::write(dir.join("notes.txt"), "not an applet").unwrap();

        let names = drawable_visuals(&dir);
        assert!(names.contains(&"clock".to_string()), "{names:?}");
        assert!(names.contains(&"applet:gpu".to_string()), "{names:?}");
        assert!(names.contains(&"applet:docker".to_string()), "{names:?}");
        assert!(
            !names.iter().any(|name| name.contains("notes")),
            "{names:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;

    fn moment<'a>(state: &'a State) -> Moment<'a> {
        Moment {
            state,
            clock: "14:30".into(),
            date: "15/09/2026".into(),
            day: "Tue".into(),
            profile: 1,
            button_mode: "auto".into(),
        }
    }

    fn a_state() -> State {
        let bindings = crate::Bindings::from_text("G1=a\n");
        let mut state = State::new(1, &bindings, Vec::new());
        state.note("G1", true);
        state
    }

    #[test]
    fn visuals_json_is_read_the_way_the_file_writes_it() {
        let dir = std::env::temp_dir().join("g13-visuals-test");
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        let path = dir.join("visuals.json");
        std::fs::write(
            &path,
            r#"{"enabled":["system","applet:gpu"],"active":"applet:gpu","cycle":true,"cycle_seconds":5}"#,
        )
        .unwrap();
        let visuals = load_visuals(&path);
        assert_eq!(visuals.active, "applet:gpu");
        assert_eq!(visuals.enabled.len(), 2);
        assert!(visuals.cycle);
        assert_eq!(visuals.cycle_seconds, 5.0);

        // a missing file is the defaults, not a failure
        let visuals = load_visuals(&dir.join("absent.json"));
        assert_eq!(visuals.active, "system");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_built_in_visuals_draw_something_and_say_nothing_wrong() {
        let state = a_state();
        let moment = moment(&state);
        let world = World::default();
        for visual in ["clock", "system", "pad"] {
            let (frame, problems) = render(visual, &moment, &world, Path::new("/nonexistent"));
            assert!(problems.is_empty(), "{visual} complained: {problems:?}");
            assert!(
                !frame.is_blank(0, 0, WIDTH, VISIBLE_HEIGHT),
                "{visual} drew nothing"
            );
        }
    }

    #[test]
    fn an_applet_gets_the_values_it_declares_and_no_others() {
        let dir = std::env::temp_dir().join("g13-agent-applet-values");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("stats.json"),
            r#"{"name":"stats","title":"STATS","interval":1,"border":true,
                "sources":{"cpu":"cmd:echo 11","gpu":"cmd:echo 22"},
                "widgets":[{"type":"text","x":3,"y":2,"format":"{gpu}% cpu {cpu} mem {memory}"}]}"#,
        )
        .unwrap();
        let state = a_state();
        let (frame, problems) = render("applet:stats", &moment(&state), &World::default(), &dir);
        assert!(
            !frame.is_blank(0, 0, WIDTH, VISIBLE_HEIGHT),
            "it drew nothing"
        );
        // declared: the command and the machine's own reading both resolve
        assert!(
            !problems.iter().any(|problem| problem.contains("gpu")),
            "a declared source was complained about: {problems:?}"
        );
        // not declared: `memory` is a built-in name, and this applet never asked for it
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("memory is read by a widget")),
            "a value nothing provides was not reported: {problems:?}"
        );
    }

    #[test]
    fn the_screen_says_which_stage_the_wizard_is_in() {
        // The stage is the only way to see what the wizard is waiting for without a terminal beside it, so
        // these assertions are the promise: the words, drawn instead of the visual.
        let dir = std::env::temp_dir().join(format!("g13-wizard-screen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("stats.json"),
            r#"{"name":"stats","interval":1,"sources":{},"widgets":[{"type":"text","x":2,"y":2,"format":"an applet"}]}"#,
        )
        .unwrap();

        let mut state = a_state();
        state.recording = Some(Wizard {
            stage: Stage::Choosing,
            id: 12,
            profile: 3,
            control: None,
            picked: None,
        });
        let (frame, problems) = render("applet:stats", &moment(&state), &World::default(), &dir);
        assert!(problems.is_empty(), "{problems:?}");
        let mut expected = Frame::new();
        expected.text_line(0, "RECORDING");
        expected.text_line(1, "macro 12");
        expected.text_line(2, "profile 3");
        expected.text_line(3, "M1 M2 M3 picks it");
        expected.text_line(4, "then the control");
        assert_eq!(
            frame, expected,
            "the choosing stage does not say what to press"
        );
        let wizard_frame = frame.clone();

        // the profile key that answered is named beside the profile: pressing the key of the profile already
        // in force changes nothing else, so without this a press that was taken looks like one that never came
        state.recording = Some(Wizard {
            stage: Stage::Choosing,
            id: 12,
            profile: 3,
            control: None,
            picked: Some("M3".into()),
        });
        let (frame, _) = render("applet:stats", &moment(&state), &World::default(), &dir);
        let mut expected = Frame::new();
        expected.text_line(0, "RECORDING");
        expected.text_line(1, "macro 12");
        expected.text_line(2, "profile 3 (M3)");
        expected.text_line(3, "M1 M2 M3 picks it");
        expected.text_line(4, "then the control");
        assert_eq!(
            frame, expected,
            "the profile key that answered is not shown"
        );

        // a control has answered: the screen names it and says how to finish
        state.recording = Some(Wizard {
            stage: Stage::Capturing,
            id: 12,
            profile: 3,
            control: Some("G7".into()),
            picked: None,
        });
        let (frame, _) = render("applet:stats", &moment(&state), &World::default(), &dir);
        let mut expected = Frame::new();
        expected.text_line(0, "RECORDING");
        expected.text_line(1, "macro 12");
        expected.text_line(2, "G7 in profile 3");
        expected.text_line(3, "press the keys");
        expected.text_line(4, "MR ends it");
        assert_eq!(
            frame, expected,
            "the capturing stage does not say what to do next"
        );

        // and with no wizard the applet is drawn again, so a finished recording gives the screen back
        state.recording = None;
        let (frame, _) = render("applet:stats", &moment(&state), &World::default(), &dir);
        assert!(
            !frame.is_blank(0, 0, WIDTH, VISIBLE_HEIGHT),
            "the applet was not drawn once the recording finished"
        );
        assert_ne!(
            frame, wizard_frame,
            "the screen still shows the wizard after the recording finished"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_published_recording_value_follows_a_wizard() {
        // `recording` is one of the keys the previous stack published, so its shape stays a 0 or a 1
        let mut state = a_state();
        assert_eq!(
            built_in_values(&moment(&state), &World::default())["recording"].text(),
            "0"
        );
        state.recording = Some(Wizard {
            stage: Stage::Capturing,
            id: 3,
            profile: 1,
            control: Some("G1".into()),
            picked: None,
        });
        assert_eq!(
            built_in_values(&moment(&state), &World::default())["recording"].text(),
            "1",
            "a running wizard is not published as a recording"
        );
    }

    #[test]
    fn an_unknown_visual_is_named_rather_than_ignored() {
        let state = a_state();
        let (_, problems) = render(
            "hologram",
            &moment(&state),
            &World::default(),
            Path::new("/x"),
        );
        assert!(problems[0].contains("hologram"), "{problems:?}");
        assert!(
            problems[0].contains("clock"),
            "it should say what it does know"
        );
    }

    #[test]
    fn an_applet_that_is_not_there_is_reported_with_its_path() {
        let state = a_state();
        let (_, problems) = render(
            "applet:missing",
            &moment(&state),
            &World::default(),
            Path::new("/nonexistent/applets"),
        );
        assert!(problems[0].contains("missing.json"), "{problems:?}");
    }

    #[test]
    fn the_published_values_keep_the_keys_the_previous_stack_used() {
        let state = a_state();
        let values = built_in_values(&moment(&state), &World::default());
        for key in [
            "button_mode",
            "cpu",
            "date",
            "day",
            "last_key",
            "load",
            "media_artist",
            "media_duration",
            "media_percent",
            "media_position",
            "media_status",
            "media_title",
            "memory",
            "profile",
            "recent_keys",
            "recording",
            g13_applets::SELECTED_VALUE,
            "screen_height",
            "screen_item",
            "screen_owner",
            "screen_width",
            "sdk_client",
            "time",
            "time_seconds",
            "uptime",
            "uptime_seconds",
        ] {
            assert!(values.contains_key(key), "the values lost {key}");
        }
        assert_eq!(values["profile"].number(), Some(1.0));
        assert_eq!(values["last_key"].text(), "G1");
        assert_eq!(values["screen_width"].number(), Some(160.0));
    }

    #[test]
    fn a_value_holding_a_control_character_still_makes_a_readable_file() {
        // The window parses this file, so an unescaped control character in a value makes the whole file
        // unreadable and every value in it disappears - not just the one that held it. Text is escaped by
        // serde now; this is the test that says so.
        let mut values = BTreeMap::new();
        values.insert(
            "awkward".to_string(),
            Value::Text("a\nb\tc\u{0}d\"e\\f".to_string()),
        );
        values.insert("fine".to_string(), Value::Text("ordinary".to_string()));
        let dir = std::env::temp_dir().join("g13-values-escape-test");
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        let path = dir.join("g13-values.json");
        publish_values(&values, &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("a control character must not break the file");
        // the awkward value comes back exactly as it went in, and the ordinary one is still there
        assert_eq!(parsed["awkward"], "a\nb\tc\u{0}d\"e\\f");
        assert_eq!(parsed["fine"], "ordinary");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_values_file_is_json_that_can_be_read_back() {
        let state = a_state();
        let values = built_in_values(&moment(&state), &World::default());
        let dir = std::env::temp_dir().join("g13-values-test");
        std::fs::create_dir_all(&dir).unwrap();
        crate::write_the_once_built_in_screens(&dir);
        let path = dir.join("g13-values.json");
        publish_values(&values, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("the file must be valid JSON");
        assert_eq!(parsed["screen_width"], 160.0);
        assert!(parsed["cpu"].is_number());
        // the media keys are strings whether or not anything is playing: asserting they are empty would only
        // pass on a machine with no player, which is not the machine this runs on
        // Present, and a string or null. Asserting a string would only pass on a machine with a player that
        // always answers; asserting null would only pass with none. What the contract promises is that the
        // key is there and a reader finds what it expects.
        for key in ["media_title", "media_artist", "media_status"] {
            assert!(
                parsed.get(key).is_some(),
                "the values lost {key}: {}",
                parsed[key]
            );
            assert!(
                parsed[key].is_string() || parsed[key].is_null(),
                "{key} is {}",
                parsed[key]
            );
        }
        std::fs::remove_file(&path).ok();
    }
}

#[cfg(test)]
mod a_program_that_writes_a_file_takes_the_screen {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn an_applets_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A file the mod writes: it exists, so its age is zero - "a program is running right now".
    fn a_file_being_written(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"{}").unwrap();
        path
    }

    fn an_applet(dir: &Path, name: &str, follow: &str, source: &str) {
        std::fs::write(
            dir.join(format!("{name}.json")),
            format!(
                r#"{{"name":"{name}","follow":{follow},"sources":{{"health":"{source}"}},"widgets":[]}}"#
            ),
        )
        .unwrap();
    }

    fn later(seconds: u64) -> SystemTime {
        SystemTime::now() + Duration::from_secs(seconds)
    }

    #[test]
    fn a_file_being_replaced_is_not_the_program_stopping() {
        // The reported fault, exactly. The mod that feeds the game's applet writes a new `hud.json` and moves it
        // into place, so the path is missing for a few milliseconds every time it writes - measured at 4 samples
        // in 200, ten milliseconds apart, while the game ran. Read as "the program stopped", the screen went back
        // and was taken again on the next pass: the pad flickering between two applets, 115 times in thirteen
        // minutes. A sighting has to be what the hand-back is measured from.
        let dir = an_applets_dir("g13-follow-replaced");
        let hud = a_file_being_written(&dir, "hud.json");
        an_applet(
            &dir,
            "game",
            r#"{"seconds":20}"#,
            &format!("json:{}#health", hud.display()),
        );
        let enabled = vec!["clock".to_string(), "applet:game".to_string()];
        let mut follower = Follower::new();

        // the program is running: the game's screen comes up
        assert!(matches!(
            follower.wanted(&enabled, "clock", &dir, 1, SystemTime::now()),
            Wanted::Take { .. }
        ));

        // and now it writes again, which is a remove and a rename: the file is gone for this pass
        std::fs::remove_file(&hud).unwrap();
        assert_eq!(
            follower.wanted(&enabled, "applet:game", &dir, 1, SystemTime::now()),
            Wanted::Nothing,
            "a file being replaced was read as the program stopping: this is the flicker"
        );
        assert_eq!(follower.following(), Some("applet:game"));

        // it is there again, and the screen stays where it is
        std::fs::write(&hud, "{\"health\":50}").unwrap();
        assert_eq!(
            follower.wanted(&enabled, "applet:game", &dir, 1, SystemTime::now()),
            Wanted::Nothing
        );

        // and when the program really is quiet - the file still there, and old - the screen goes back
        match follower.wanted(&enabled, "applet:game", &dir, 1, later(60)) {
            Wanted::Give { visual, .. } => assert_eq!(visual, Some("clock".to_string())),
            other => panic!("expected the screen back after a real stop, got {other:?}"),
        }
        assert_eq!(follower.following(), None);
    }

    #[test]
    fn a_file_that_disappears_for_good_is_given_up_one_window_later() {
        // The other side of it: a file that is never written again must not hold the screen for ever. The window
        // is the applet's own `seconds`, counted from the last time the file was really there.
        let dir = an_applets_dir("g13-follow-vanished");
        let hud = a_file_being_written(&dir, "hud.json");
        an_applet(
            &dir,
            "game",
            r#"{"seconds":20}"#,
            &format!("json:{}#health", hud.display()),
        );
        let enabled = vec!["clock".to_string(), "applet:game".to_string()];
        let mut follower = Follower::new();

        assert!(matches!(
            follower.wanted(&enabled, "clock", &dir, 1, SystemTime::now()),
            Wanted::Take { .. }
        ));

        std::fs::remove_file(&hud).unwrap();
        assert_eq!(
            follower.wanted(&enabled, "applet:game", &dir, 1, SystemTime::now()),
            Wanted::Nothing,
            "one missing reading is not yet a stop"
        );
        match follower.wanted(&enabled, "applet:game", &dir, 1, later(60)) {
            Wanted::Give { visual, .. } => assert_eq!(visual, Some("clock".to_string())),
            other => panic!("a file gone for a whole window is a stop, got {other:?}"),
        }
    }

    #[test]
    fn a_file_being_written_takes_the_screen_and_hands_it_back() {
        let dir = an_applets_dir("g13-follow-basic");
        let hud = a_file_being_written(&dir, "hud.json");
        an_applet(
            &dir,
            "game",
            r#"{"seconds":20}"#,
            &format!("json:{}#health", hud.display()),
        );
        let enabled = vec!["clock".to_string(), "applet:game".to_string()];
        let mut follower = Follower::new();

        // the mod is writing: the game's screen comes up
        match follower.wanted(&enabled, "clock", &dir, 1, SystemTime::now()) {
            Wanted::Take { visual, profile } => {
                assert_eq!(visual, "applet:game");
                assert_eq!(profile, None, "it named no binding set");
            }
            other => panic!("expected it to take the screen, got {other:?}"),
        }
        assert_eq!(follower.following(), Some("applet:game"));

        // and once it is showing, it stops asking
        assert_eq!(
            follower.wanted(&enabled, "applet:game", &dir, 1, SystemTime::now()),
            Wanted::Nothing
        );

        // the game quits: the writing stops, and the clock comes back
        match follower.wanted(&enabled, "applet:game", &dir, 1, later(60)) {
            Wanted::Give { visual, profile } => {
                assert_eq!(visual, Some("clock".to_string()));
                assert_eq!(profile, None, "it never changed the profile");
            }
            other => panic!("expected it to hand the screen back, got {other:?}"),
        }
        assert_eq!(follower.following(), None);
    }

    #[test]
    fn a_pause_shorter_than_its_own_seconds_is_not_a_quit() {
        let dir = an_applets_dir("g13-follow-pause");
        let hud = a_file_being_written(&dir, "hud.json");
        an_applet(
            &dir,
            "game",
            r#"{"seconds":120}"#,
            &format!("json:{}#health", hud.display()),
        );
        let enabled = vec!["clock".to_string(), "applet:game".to_string()];
        let mut follower = Follower::new();

        assert!(matches!(
            follower.wanted(&enabled, "clock", &dir, 1, SystemTime::now()),
            Wanted::Take { .. }
        ));
        // a minute of quiet, with the applet asking for two: still live, still showing
        assert_eq!(
            follower.wanted(&enabled, "applet:game", &dir, 1, later(60)),
            Wanted::Nothing,
            "a pause inside its own window was treated as the program stopping"
        );
    }

    #[test]
    fn an_applet_without_follow_never_takes_the_screen() {
        let dir = an_applets_dir("g13-follow-none");
        let hud = a_file_being_written(&dir, "hud.json");
        // the same applet, the same fresh file, but no `follow`: it is only ever shown because somebody chose it
        an_applet(
            &dir,
            "game",
            "null",
            &format!("json:{}#health", hud.display()),
        );
        let enabled = vec!["clock".to_string(), "applet:game".to_string()];
        let mut follower = Follower::new();
        assert_eq!(
            follower.wanted(&enabled, "clock", &dir, 1, SystemTime::now()),
            Wanted::Nothing
        );
    }

    #[test]
    fn the_file_behind_a_source_is_what_is_watched_not_a_process() {
        let dir = an_applets_dir("g13-follow-source");
        a_file_being_written(&dir, "hud.json");
        // a source that reads nothing that can be written (a command, a fetch) is never "live" on its own
        an_applet(&dir, "game", r#"{"seconds":20}"#, "cmd:echo hi");
        let enabled = vec!["applet:game".to_string()];
        assert!(live_applets(&enabled, &dir, SystemTime::now()).is_empty());

        // and one that reads a file is
        let hud = dir.join("hud.json");
        an_applet(
            &dir,
            "game",
            r#"{"seconds":20}"#,
            &format!("json:{}#health", hud.display()),
        );
        assert_eq!(live_applets(&enabled, &dir, SystemTime::now()), enabled);
    }

    #[test]
    fn the_binding_set_it_names_comes_with_it_and_goes_back() {
        let dir = an_applets_dir("g13-follow-profile");
        let hud = a_file_being_written(&dir, "hud.json");
        an_applet(
            &dir,
            "game",
            r#"{"seconds":20,"profile":"Cyberpunk"}"#,
            &format!("json:{}#health", hud.display()),
        );
        let enabled = vec!["clock".to_string(), "applet:game".to_string()];
        let mut follower = Follower::new();

        match follower.wanted(&enabled, "clock", &dir, 0, SystemTime::now()) {
            Wanted::Take { visual, profile } => {
                assert_eq!(visual, "applet:game");
                assert_eq!(
                    profile,
                    Some("Cyberpunk".to_string()),
                    "the applet named a binding set and it did not come with it"
                );
            }
            other => panic!("expected it to take the screen, got {other:?}"),
        }

        // the loop switches to set 3 because that is what "Cyberpunk" is, and tells the follower
        follower.took_profile(Some(3));
        // the game quits with set 3 still in force: that one was ours, so set 0 comes back
        match follower.wanted(&enabled, "applet:game", &dir, 3, later(60)) {
            Wanted::Give { visual, profile } => {
                assert_eq!(visual, Some("clock".to_string()));
                assert_eq!(
                    profile,
                    Some(0),
                    "the profile before the game did not come back"
                );
            }
            other => panic!("expected it to hand everything back, got {other:?}"),
        }
    }

    #[test]
    fn a_profile_you_changed_yourself_is_not_undone() {
        let dir = an_applets_dir("g13-follow-profile-yours");
        let hud = a_file_being_written(&dir, "hud.json");
        an_applet(
            &dir,
            "game",
            r#"{"seconds":20,"profile":"Cyberpunk"}"#,
            &format!("json:{}#health", hud.display()),
        );
        let enabled = vec!["clock".to_string(), "applet:game".to_string()];
        let mut follower = Follower::new();

        assert!(matches!(
            follower.wanted(&enabled, "clock", &dir, 0, SystemTime::now()),
            Wanted::Take { .. }
        ));
        follower.took_profile(Some(3));
        // you pressed M1 mid-game, so set 1 is in force, not the one the applet asked for
        match follower.wanted(&enabled, "applet:game", &dir, 1, later(60)) {
            Wanted::Give { profile, .. } => {
                assert_eq!(profile, None, "a profile you chose yourself was undone");
            }
            other => panic!("expected it to hand the screen back, got {other:?}"),
        }
    }

    #[test]
    fn your_own_choice_of_screen_is_not_fought() {
        let dir = an_applets_dir("g13-follow-user");
        let hud = a_file_being_written(&dir, "hud.json");
        an_applet(
            &dir,
            "game",
            r#"{"seconds":20}"#,
            &format!("json:{}#health", hud.display()),
        );
        let enabled = vec!["clock".to_string(), "applet:game".to_string()];
        let mut follower = Follower::new();

        assert!(matches!(
            follower.wanted(&enabled, "clock", &dir, 1, SystemTime::now()),
            Wanted::Take { .. }
        ));
        // you flipped to the media screen while the game was running: it stays where you put it
        follower.user_chose("media");
        assert_eq!(
            follower.wanted(&enabled, "media", &dir, 1, SystemTime::now()),
            Wanted::Nothing,
            "the game took the screen back off you mid-game"
        );
        // and choosing the game's own screen is not a disagreement
        let mut follower = Follower::new();
        assert!(matches!(
            follower.wanted(&enabled, "clock", &dir, 1, SystemTime::now()),
            Wanted::Take { .. }
        ));
        follower.user_chose("applet:game");
        assert_eq!(follower.following(), Some("applet:game"));
    }

    #[test]
    fn an_applet_edited_underneath_is_read_again() {
        // The cache is keyed on the applet file's stamp, so an edit has to be noticed: a cache that outlives the
        // file it describes is a screen that never changes when the file does.
        let dir = an_applets_dir("g13-follow-edited");
        let hud = a_file_being_written(&dir, "hud.json");
        an_applet(
            &dir,
            "game",
            r#"{"seconds":20}"#,
            &format!("json:{}#health", hud.display()),
        );
        let enabled = vec!["clock".to_string(), "applet:game".to_string()];
        let mut follower = Follower::new();

        assert!(matches!(
            follower.wanted(&enabled, "clock", &dir, 1, SystemTime::now()),
            Wanted::Take { .. }
        ));

        // the file is edited underneath, taking `follow` away
        std::thread::sleep(Duration::from_millis(20));
        an_applet(
            &dir,
            "game",
            "null",
            &format!("json:{}#health", hud.display()),
        );
        assert_eq!(
            follower.wanted(&enabled, "applet:game", &dir, 1, SystemTime::now()),
            Wanted::Give {
                visual: Some("clock".to_string()),
                profile: None
            },
            "an edit that took `follow` away was not noticed"
        );
    }

    #[test]
    fn the_pad_does_not_come_up_on_a_screen_whose_program_is_not_running() {
        let dir = an_applets_dir("g13-follow-start");
        let hud = a_file_being_written(&dir, "hud.json");
        an_applet(
            &dir,
            "game",
            r#"{"seconds":20}"#,
            &format!("json:{}#health", hud.display()),
        );
        let enabled = vec![
            "clock".to_string(),
            "applet:game".to_string(),
            "media".to_string(),
        ];

        // the game is running: nothing to change
        assert_eq!(
            start_on(&enabled, "applet:game", &dir, SystemTime::now()),
            None
        );

        // the game is not: the pad comes up on something that does not depend on it
        assert_eq!(
            start_on(&enabled, "applet:game", &dir, later(60)),
            Some("clock".to_string())
        );

        // and a screen that does not follow is left alone
        assert_eq!(start_on(&enabled, "clock", &dir, later(60)), None);
    }
}
