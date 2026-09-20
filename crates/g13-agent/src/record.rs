//! The record wizard: MR, then which profile, then which control, then the keys.
//!
//! The pad's record button starts a two-question wizard rather than a raw recorder, because a macro nobody can
//! play is not worth recording: the answers are which profile the macro belongs to and which control plays it.
//! The profile question is answered *by* the pad's own profile keys - pressing M2 switches to profile 2 the way
//! it always does, so what the lights show is what gets written - which makes it optional, since pressing a
//! control straight away records into the profile already in force.
//!
//! What the wizard publishes (`Stage`, `Wizard`) is derived from what this module is actually doing rather than
//! set by hand at each step: a path that finished a recording and forgot to clear the published stage left the
//! pad saying `RECORDING` for ever, and the next press started a recording nobody asked for. One function means
//! no path can forget.

use crate::Event;

/// What the record wizard is asking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Which control will play the macro, and in which profile.
    Choosing,
    /// The keys are being taken from the real keyboards.
    Capturing,
}

/// A recording in progress, as the pad's screen and the terminal both describe it.
#[derive(Debug, Clone, PartialEq)]
pub struct Wizard {
    /// Which of the wizard's two questions is being asked.
    pub stage: Stage,
    /// The macro that will be written.
    pub id: u32,
    /// The profile whose file the binding will go into.
    pub profile: u32,
    /// The control that will play it, once one has answered.
    pub control: Option<String>,
    /// The profile key that answered, once one has. Shown on the screen as well as the profile itself,
    /// because pressing the key of the profile already in force changes nothing else: without this there is
    /// no way to tell a press that was taken from a press that never arrived.
    pub picked: Option<String>,
}
/// What the record button is doing.
///
/// MR starts it, and then it asks for the two things a person would say in that order: which profile the macro
/// belongs to, and which control plays it. The profile question is answered by the pad's own profile keys and
/// answered *by* them - pressing M2 switches to profile 2 the way it always does, so what the lights show is
/// what gets written. The M step is therefore optional: pressing a control straight away records into the
/// profile that is already active.
pub(crate) enum Recording {
    /// MR has been pressed; waiting to be told which control the macro should play from.
    ChoosingControl {
        /// When the wizard was started, so being left unanswered for `CHOOSING_SECONDS` gives up.
        started: std::time::Instant,
        /// Decided now rather than at the end, so the recording cannot land on an id that something else took
        /// while the keys were being pressed.
        id: u32,
        /// The name the macro already had, so recording over one does not rename it.
        name: String,
        /// The profile key that has answered, once one has. Set whether or not it changed anything: pressing
        /// the key of the profile already in force changes nothing else at all, and a press that was taken
        /// must never look like one that never arrived.
        picked: Option<String>,
    },
    /// The control is known and the keys are being taken from the real keyboards.
    Capturing {
        /// The macro being written.
        id: u32,
        /// The name the macro is written under: the one it already had, or a new one made from the control.
        name: String,
        /// The profile whose file the binding is written to: the active one, chosen by pressing a profile key.
        profile: u32,
        /// The control the macro will play from, as the pad prints it.
        control: String,
        /// The real keyboards' events, collected on another thread and drained here without waiting.
        keys: std::sync::mpsc::Receiver<g13_device::capture::KeyEvent>,
        /// What has been pressed so far: the whole recording, turned into steps when it ends.
        events: Vec<g13_device::capture::KeyEvent>,
        /// When the keys started being taken, so `RECORD_LONGEST` can end a recording that was forgotten.
        started: std::time::Instant,
        /// When the last key arrived, so `RECORD_IDLE` can decide the person has finished.
        last: std::time::Instant,
    },
}

/// What the screen and the values file publish, from what the loop is actually doing.
///
/// Derived rather than set by hand at each transition: a path that finished a recording and forgot to clear
/// this left the pad saying `RECORDING` for ever, and the next press started a recording nobody asked for.
/// One function means no path can forget.
pub(crate) fn published_stage(recording: &Option<Recording>, profile: u32) -> Option<Wizard> {
    match recording {
        None => None,
        Some(Recording::ChoosingControl { id, picked, .. }) => Some(Wizard {
            stage: Stage::Choosing,
            id: *id,
            profile,
            control: None,
            picked: picked.clone(),
        }),
        Some(Recording::Capturing { id, control, .. }) => Some(Wizard {
            stage: Stage::Capturing,
            id: *id,
            profile,
            control: Some(control.clone()),
            picked: None,
        }),
    }
}

/// Begin the wizard: settle the macro it will write, and wait to be told the control.
///
/// The id is settled *here* rather than at the end, so a recording cannot land on an id that something else
/// took while the keys were being pressed. It is handed back as well, because the terminal says which macro is
/// about to be written before a key is touched.
pub(crate) fn start_choosing(target: Option<u32>) -> (u32, Recording) {
    let id = target.unwrap_or_else(g13_config::next_free_macro_id);
    let existing = std::fs::read_to_string(g13_config::macro_path(id)).unwrap_or_default();
    let choosing = Recording::ChoosingControl {
        started: std::time::Instant::now(),
        id,
        name: g13_config::parse_macro(&existing).name,
        picked: None,
    };
    (id, choosing)
}
/// How long the wizard waits to be told which control to use.
pub(crate) const CHOOSING_SECONDS: u64 = 15;
/// The longest a recording may run, so one that is forgotten cannot hold the pad's bindings off.
pub(crate) const RECORD_LONGEST: std::time::Duration = std::time::Duration::from_secs(60);

/// How long a recording waits for the next key before it decides the person has finished.
pub(crate) const RECORD_IDLE: std::time::Duration = std::time::Duration::from_secs(5);
/// The name a recording is given, so a list of macros says what they are for.
pub(crate) fn recording_name(profile: u32, control: &str) -> String {
    format!("{control} in profile {profile}")
}

/// Turn what was pressed into the steps a macro file holds.
///
/// Every event with a pause before it long enough to be meant becomes a `d.` step, and the recording ends with
/// one so playing it twice does not run the two together. A key that was held comes out as a down, the pause
/// and the up - which is the shape the previous tool wrote and its player reads.
pub fn steps_from_events(events: &[g13_device::capture::KeyEvent]) -> Vec<g13_config::MacroStep> {
    // below this a gap is the kernel's own jitter rather than a pause a person made
    const MEANINGFUL_MS: u32 = 12;
    // and beyond this it is a person being distracted, not a step of the macro
    const LONGEST_MS: u32 = 2_000;
    let mut steps = Vec::new();
    for (index, event) in events.iter().enumerate() {
        // The gap before the *first* key is the hand moving to it and not part of the pattern: a macro starts
        // when it is triggered, so time between pressing record and pressing the first key is dropped instead
        // of being played back as a pause before the first key.
        if index > 0 && event.after_ms >= MEANINGFUL_MS {
            steps.push(g13_config::MacroStep::Delay(event.after_ms.min(LONGEST_MS)));
        }
        steps.push(if event.down {
            g13_config::MacroStep::KeyDown(event.code)
        } else {
            g13_config::MacroStep::KeyUp(event.code)
        });
    }
    if !steps.is_empty() {
        steps.push(g13_config::MacroStep::Delay(100));
    }
    steps
}
/// Begin capturing: what is pressed from now on goes into a macro for `control` in `profile`.
pub(crate) fn start_capturing(id: u32, name: String, profile: u32, control: String) -> Recording {
    Recording::Capturing {
        id,
        name,
        profile,
        control,
        keys: g13_device::capture::record_keys(),
        events: Vec::new(),
        started: std::time::Instant::now(),
        last: std::time::Instant::now(),
    }
}

/// Write what was recorded, and put it on the control that was chosen.
///
/// Two things are written, which is the whole point of the wizard: the macro itself, and the binding that plays
/// it in the profile that was chosen. The binding goes in through `set_binding`, which edits the text line by
/// line, so nothing else in that profile's file moves. A recording with nothing in it writes neither: an empty
/// macro silently replacing a working binding would be worse than saying nothing happened.
pub(crate) fn finish_recording(
    dir: &std::path::Path,
    profile: u32,
    control: &str,
    id: u32,
    existing_name: &str,
    events: &[g13_device::capture::KeyEvent],
) -> (Event, Vec<String>) {
    let steps = steps_from_events(events);
    if steps.is_empty() {
        return (
            Event::MacroRecorded {
                id,
                name: String::new(),
                steps: 0,
                profile,
                control: control.to_string(),
            },
            Vec::new(),
        );
    }
    let mut complaints = Vec::new();
    let name = if existing_name.is_empty() {
        recording_name(profile, control)
    } else {
        existing_name.to_string()
    };
    let macro_file = g13_config::Macro {
        name: name.clone(),
        id,
        steps: steps.clone(),
    };
    let path = dir.join(format!("macro-{id}.properties"));
    let _ = std::fs::create_dir_all(dir);
    if let Err(error) = g13_files::write(&path, &g13_config::macro_to_text(&macro_file)) {
        complaints.push(format!("could not write {}: {error}", path.display()));
    }
    // and the binding, in the profile that was chosen rather than whichever is active now
    let bindings_path = dir.join(format!("bindings-{profile}.properties"));
    let text = std::fs::read_to_string(&bindings_path).unwrap_or_default();
    let updated = g13_config::set_binding(&text, control, &format!("m,{id},1"));
    if let Err(error) = g13_files::write(&bindings_path, &updated) {
        complaints.push(format!(
            "could not write {}: {error}",
            bindings_path.display()
        ));
    }
    (
        Event::MacroRecorded {
            id,
            name,
            steps: steps.len(),
            profile,
            control: control.to_string(),
        },
        complaints,
    )
}
