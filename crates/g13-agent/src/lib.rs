//! The driver: the pad, a binding map, and a keyboard, running in a loop.
//!
//! This is what replaces the previous stack. It reads the pad, looks each control up in the binding map
//! that came from the configuration, and sends the key that map names  -  then does it again, forever, and
//! notices when the map changes.
//!
//! It switches profile for a control bound to `mk,N`, plays a macro for one bound to `m,<id>,<repeats>`,
//! and writes what it is doing to a small state file so the driver can be inspected without reading a log.

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
use g13_device::keyboard::{self, KeyCode};
use g13_device::{CONTROLS, Device};
use g13_proto::{Control, bit_for_control, control_from_name};
use g13_screen::Frame;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Which visual the screen is showing, from `visuals.json`.
#[derive(Debug, Clone)]
pub struct Visuals {
    /// The screens in the rotation, in the order they are shown.
    pub enabled: Vec<String>,
    /// Which of them is on the screen.
    pub active: String,
    /// Whether the rotation moves on by itself.
    pub cycle: bool,
    /// How long each screen is shown before the rotation moves on.
    pub cycle_seconds: f64,
}

impl Default for Visuals {
    fn default() -> Self {
        Self {
            enabled: vec!["system".to_string()],
            active: "system".to_string(),
            cycle: false,
            cycle_seconds: 10.0,
        }
    }
}

/// Renders the screen on its own thread.
///
/// Drawing a visual means running its sources, and an applet's sources can be slow: measured on this machine,
/// gpu 105 ms, docker 166, ci 862 and weather 848, because they run commands and make network reads. Anything
/// drawing on the thread that reads the pad would stall the driver - a keypress would wait for a weather
/// report - and anything drawing on the window's thread stalls the window the same way.
///
/// So there is one implementation and both use it: the driver takes frames from it and writes them to the
/// pad, the window takes frames from it and draws them on screen. Neither ever waits for a source.
pub mod playing;
pub use playing::{Playback, condition_holds, play_graph, play_graph_reporting};

pub mod record;
use record::{
    CHOOSING_SECONDS, RECORD_IDLE, RECORD_LONGEST, Recording, finish_recording, published_stage,
    start_capturing, start_choosing,
};
pub use record::{Stage, Wizard, steps_from_events};

pub mod menu;
#[cfg(test)]
use menu::menu_for;
pub use menu::{
    Menu, MenuKey, MenuMove, MenuTarget, child_level, level_from, menu_frame, menu_key, menu_label,
    menu_lines, menu_move, screens_level,
};
use menu::{open_menu, step_selection};

pub mod macros;
pub use macros::{play_macro, play_macro_reporting};

pub mod stick;
use stick::set_bound;
pub use stick::{Bound, DeviceButton, StickWorker, apply_stick};
#[cfg(test)]
use stick::{axis_for_target, gamepad_axes};

pub mod workers;
pub use workers::Workers;

/// What the screen needs from the driver, which the worker cannot read for itself.
///
/// The worker draws on a thread of its own and cannot borrow the driver's state, so the few things the screen
/// shows from it are handed over as they change. Without this the wizard's stage never reached the screen and
/// the built-in `pad` visual's LAST line always said "nothing yet", which was a lie.
#[derive(Debug, Default, Clone)]
pub struct ScreenStatus {
    /// The record wizard, while one is running: the stage the screen draws. None the rest of the time.
    pub recording: Option<Wizard>,
    /// The last controls the pad sent, newest last.
    pub last_keys: Vec<(String, bool, u64)>,
    /// The menu, while one is open. The screen draws it *instead of* the visual: it is what the pad is showing,
    /// so it is what the panel shows, and the thing underneath is not what the controls are driving.
    pub menu: Option<Menu>,
    /// Which screen of the applet is showing. L1 moves it. Zero for anything that has only one screen.
    pub screen: usize,
    /// Which of that screen's actionable things is chosen, and its item's text, both counted from zero here.
    ///
    /// They travel with the handover rather than being read out of the published values, because the worker
    /// builds its own `State` each pass: a drawing input that only exists in the driver's copy of the state is
    /// a drawing input the pad never sees.
    pub screen_selected: usize,
    /// The text of the chosen item, when the chosen thing is a row of a list. Empty otherwise.
    pub screen_item: String,
}

/// The handover, and a wake-up with it.
///
/// The worker cannot borrow the driver's state, so it is told what to show; and it must be *woken* when that
/// changes. On a timer alone the pad showed a stage up to a second after the key that caused it - and after a
/// recording finished it went on saying `RECORDING`, which is how finishing came to look like it had not
/// happened, and how pressing the record button again started a second recording nobody asked for.
#[derive(Clone, Default)]
pub struct ScreenWake {
    /// The status and the condvar that wakes its readers, in one allocation so clones share the handover.
    pair: Arc<(Mutex<ScreenStatus>, Condvar)>,
}

impl ScreenWake {
    /// Hand over what the screen shows, and wake the worker to draw it now.
    pub fn set(&self, status: ScreenStatus) {
        if let Ok(mut held) = self.pair.0.lock() {
            *held = status;
        }
        self.pair.1.notify_all();
    }

    /// What the screen should be showing.
    pub fn get(&self) -> ScreenStatus {
        self.pair
            .0
            .lock()
            .map(|held| held.clone())
            .unwrap_or_default()
    }

    /// Wait for a change or for `timeout`, whichever comes first.
    pub fn wait(&self, timeout: Duration) {
        if let Ok(held) = self.pair.0.lock() {
            let _ = self.pair.1.wait_timeout(held, timeout);
        }
    }

    /// Wake a waiter without changing anything, for a worker being told to stop.
    pub fn wake(&self) {
        self.pair.1.notify_all();
    }
}

/// Somewhere a finished frame can go.
///
/// The driver's worker writes the panel with this. A test gives it something that records frames instead, which
/// is the only way to check "the screen thread writes, the input loop does not" without a pad in the room.
///
/// Not public, and that is deliberate: the only way to hand a worker a writer is [`Panel`], which nothing
/// outside this crate can make. A window that could name a sink could install one.
pub(crate) trait FrameSink: Send + Sync {
    /// Write a finished frame where this sink sends it: the pad's panel, or a test's recorder.
    fn show(&self, report: &[u8]) -> Result<usize, g13_device::DeviceError>;
}

impl FrameSink for Device {
    fn show(&self, report: &[u8]) -> Result<usize, g13_device::DeviceError> {
        self.write_lcd(report)
    }
}

/// The driver's own panel: the one writer a preview must never be given.
///
/// The field is private and nothing public builds one, so a `Panel` exists only where this crate made it - in
/// the driver, out of the pad it has claimed. The window runs this same worker to draw into a pane and passes
/// `None`, and it could not pass a `Panel` even by mistake: that is the difference between a preview that does
/// not take the panel and a preview that *cannot* take it. Before this the rule was a comment and one call
/// site, which is a convention rather than a rule.
pub struct Panel(Arc<dyn FrameSink>);

impl Panel {
    /// The panel of the pad this driver holds.
    pub(crate) fn claimed(device: &Arc<Device>) -> Self {
        Self(Arc::clone(device) as Arc<dyn FrameSink>)
    }
}

/// What the gathering thread has read, and which visual it read it for.
///
/// A pair rather than a bare map so the drawing thread can tell whether these values are the ones for the screen
/// it is drawing: another applet's names are different names.
type ReadSoFar = Option<(String, BTreeMap<String, g13_values::Value>)>;

/// Runs one visual's sources and draws its frames, on a thread of its own.
///
/// The driver keeps one and writes its frames to the panel; the window keeps another and draws them in a
/// pane. Neither ever waits for a source.
pub struct ScreenWorker {
    /// So a shutdown does not wait out an interval.
    wake: ScreenWake,
    /// The newest frame, waiting to be taken; `take` is what clears it.
    latest: Arc<Mutex<Option<Frame>>>,
    /// The visual to draw, read at the top of each pass: a change here is why `show` also wakes the worker.
    wanted: Arc<Mutex<String>>,
    /// The values the frame was drawn from, gathered here because gathering is the slow part.
    gathered: Arc<Mutex<BTreeMap<String, g13_values::Value>>>,
    /// What the worker had to say about the last frame, replaced on every pass.
    problems: Arc<Mutex<Vec<String>>>,
    /// Set on drop, so both of the worker's threads leave their waits and can be joined.
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// The thread that runs the readings, held so a drop can join it.
    gathering: Option<std::thread::JoinHandle<()>>,
    /// The thread that draws, held so a drop can join it.
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ScreenWorker {
    /// `panel` is the pad, when this worker is the driver's own and its frames are the panel.
    ///
    /// The window runs the same worker to draw into a pane and passes `None` - and it has no way to pass
    /// anything else, because a [`Panel`] can only be made in this crate. What used to keep a preview off the
    /// panel was a comment here and the single call site that honoured it.
    pub fn start(
        visual: String,
        applet_dir: PathBuf,
        config_dir: PathBuf,
        panel: Option<Panel>,
        wake: ScreenWake,
    ) -> Self {
        // the sink the drawing thread writes to, if this worker was given the panel at all
        let writer = panel.map(|panel| panel.0);
        let latest = Arc::new(Mutex::new(None));
        let wanted = Arc::new(Mutex::new(visual));
        let gathered: Arc<Mutex<BTreeMap<String, g13_values::Value>>> =
            Arc::new(Mutex::new(BTreeMap::new()));
        let its_problems: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // The applet's own values, gathered by the thread below and only read here. It is a pair so the reader can
        // tell which visual they are for.
        let kept: Arc<Mutex<ReadSoFar>> = Arc::new(Mutex::new(None));

        // The readings, on a thread of their own.
        //
        // Every reading an applet or a machine visual wants is a *process*: `playerctl` costs about 7.5ms and the
        // machine's numbers are seven of them, so a gather is about 51ms, measured. Done on the drawing thread
        // that is one frame in twenty lost every second, which shows up as a stutter about once a second on the pad
        // and in the window's preview. Done here, the drawing thread only locks and clones a map, and no frame
        // is ever lost to a process.
        let gathering = {
            let wanted = Arc::clone(&wanted);
            let gathered = Arc::clone(&gathered);
            let kept = Arc::clone(&kept);
            let stop = Arc::clone(&stop);
            let wake = wake.clone();
            let config_dir = config_dir.clone();
            let applet_dir = applet_dir.clone();
            std::thread::spawn(move || {
                let mut last_visual = String::new();
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let visual = wanted
                        .lock()
                        .map(|name| name.clone())
                        .unwrap_or_else(|_| "clock".to_string());
                    let interval = visuals::interval_for(&visual, &applet_dir);
                    let mut state = State::new(
                        g13_config::read_active_profile(),
                        &Bindings::from_text(""),
                        Vec::new(),
                    );
                    let handed = wake.get();
                    state.recording = handed.recording;
                    state.last_keys = handed.last_keys;
                    state.screen_selected = handed.screen_selected;
                    state.screen_item = handed.screen_item.clone();
                    let world = applet_world(&config_dir).0;
                    let moment = visuals::Moment {
                        state: &state,
                        clock: world.clock.time.clone(),
                        date: world.clock.date.clone(),
                        day: world.clock.day.clone(),
                        profile: state.profile,
                        button_mode: "auto".to_string(),
                    };
                    // the machine's numbers, for the driver's own visuals and the publish path
                    let numbers = visuals::built_in_values(&moment, &world);
                    if let Ok(mut slot) = gathered.lock() {
                        *slot = numbers.clone();
                    }
                    // and the applet's own sources, read once here: a source may be a `cmd:` that costs the same
                    // as one of the machine's numbers
                    let own = match visual.strip_prefix("applet:") {
                        None => BTreeMap::new(),
                        Some(name) => {
                            match std::fs::read_to_string(applet_dir.join(format!("{name}.json"))) {
                                Err(_) => BTreeMap::new(),
                                Ok(text) => match g13_applets::parse_applet(&text) {
                                    Err(_) => BTreeMap::new(),
                                    Ok(applet) => g13_applets::gather(&applet, &world).0,
                                },
                            }
                        }
                    };
                    if let Ok(mut slot) = kept.lock() {
                        // a different visual is a different set of names, so nothing is carried across
                        let same = last_visual == visual;
                        *slot = Some((
                            visual.clone(),
                            match same {
                                true => {
                                    let mut merged = own;
                                    if let Some((_, before)) = slot.as_ref() {
                                        for (name, value) in before {
                                            merged
                                                .entry(name.clone())
                                                .or_insert_with(|| value.clone());
                                        }
                                    }
                                    merged
                                }
                                false => own,
                            },
                        ));
                    }
                    last_visual = visual;
                    // and it waits its interval, because that is the reading cadence and nothing here is urgent
                    let woke = std::time::Instant::now();
                    while woke.elapsed() < interval
                        && !stop.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
            })
        };

        let handle = {
            let latest = Arc::clone(&latest);
            let wanted = Arc::clone(&wanted);
            let gathered = Arc::clone(&gathered);
            let stop = Arc::clone(&stop);
            let problems = Arc::clone(&its_problems);
            let wake = wake.clone();
            let writer = writer.clone();
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let visual = wanted
                        .lock()
                        .map(|name| name.clone())
                        .unwrap_or_else(|_| "clock".to_string());
                    let mut state = State::new(
                        g13_config::read_active_profile(),
                        &Bindings::from_text(""),
                        Vec::new(),
                    );
                    let handed = wake.get();
                    state.recording = handed.recording;
                    state.last_keys = handed.last_keys;
                    state.screen_selected = handed.screen_selected;
                    state.screen_item = handed.screen_item.clone();
                    let world = applet_world(&config_dir).0;
                    let moment = visuals::Moment {
                        state: &state,
                        clock: world.clock.time.clone(),
                        date: world.clock.date.clone(),
                        day: world.clock.day.clone(),
                        profile: state.profile,
                        button_mode: "auto".to_string(),
                    };
                    // Two cadences, because they answer different questions. Reading costs - seven
                    // `playerctl` processes and several /proc reads are about a fifth of a second - and drawing
                    // is a local write. So the readings are taken once per the applet's own interval and the
                    // drawing may run far faster, asking again only for the values that move with the pad.
                    let interval = visuals::interval_for(&visual, &applet_dir);
                    // The readings come from the gathering thread. This thread locks a map and clones it, which
                    // is microseconds, instead of running seven processes and losing a frame doing it.
                    let values = match gathered.lock() {
                        Ok(values) => values.clone(),
                        Err(_) => BTreeMap::new(),
                    };
                    // Only what the gathering thread has already read, and never a gather of this thread's own. If
                    // this pass comes before the first gather is ready the values are simply not there yet, and a
                    // widget that reads one draws the value's name for a frame - a still picture rather than a
                    // stall, which is the whole point: this thread must never wait for a process.
                    let own = match kept.lock() {
                        // only for the visual being drawn: another applet's names are different names
                        Ok(slot) => slot
                            .clone()
                            .filter(|(name, _)| name == &visual)
                            .map(|(_, values)| values)
                            .unwrap_or_default(),
                        Err(_) => BTreeMap::new(),
                    };
                    // While the menu is open it is what the pad is showing, so it is what the panel shows: the
                    // thing underneath is not what the controls are driving.
                    let (frame, found, used) = match handed.menu.as_ref() {
                        Some(menu) => (menu_frame(menu), Vec::new(), None),
                        None => visuals::render_screen_from(
                            &visual,
                            handed.screen,
                            &moment,
                            &world,
                            &applet_dir,
                            // always given, never `None`: `None` means "gather it here", and gathering here is the
                            // stutter this whole piece is about
                            Some(&own),
                            // and the machine's numbers as the gathering thread read them, for the same reason
                            Some(&values),
                        ),
                    };
                    let _ = used;
                    if let Ok(mut problems) = problems.lock() {
                        problems.clear();
                        problems.extend(found);
                    }
                    // The write is here, on this thread, and never on the driver's input loop: one frame costs
                    // about 26ms on this device (measured), and the input loop is the pad's responsiveness.
                    // The input loop has to stay clean and as fast as possible: that loop is the pad's
                    // responsiveness, and it is the one thing a change here must never cost.
                    if let Some(writer) = writer.as_ref() {
                        let report = frame.to_report();
                        match writer.show(&report) {
                            Ok(written) if written == report.len() => {}
                            Ok(written) => {
                                if let Ok(mut problems) = problems.lock() {
                                    problems.push(format!(
                                        "the screen took only {written} of {} bytes",
                                        report.len()
                                    ));
                                }
                            }
                            Err(error) => {
                                if let Ok(mut problems) = problems.lock() {
                                    problems.push(format!("could not draw on the screen: {error}"));
                                }
                            }
                        }
                    }
                    if let Ok(mut latest) = latest.lock() {
                        *latest = Some(frame);
                    }
                    // A wizard's stage is read as instructions and a finished recording must stop saying
                    // `RECORDING` at once, so the wait ends the moment the driver's state changes rather than
                    // at the end of an interval. The timeout is the ordinary cadence: 200ms while a wizard
                    // runs, and whatever the visual asks for the rest of the time.
                    // a screen with something that moves on it is drawn at the pad's own rate; everything else
                    // waits its interval. The cap is twenty a second: a panel that is redrawn faster than the
                    // eye can follow is work for nothing, and the write itself is not free.
                    let wait = if state.recording.is_some() {
                        Duration::from_millis(200)
                    } else if visuals::fast_draw_for(&visual, handed.screen, &applet_dir) {
                        Duration::from_millis(50)
                    } else {
                        interval
                    };
                    wake.wait(wait);
                    if stop.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                }
            })
        };

        Self {
            wake,
            latest,
            wanted,
            gathered,
            problems: its_problems,
            gathering: Some(gathering),
            stop,
            handle: Some(handle),
        }
    }

    /// Ask for a different visual, and wake the worker to draw it.
    ///
    /// The wake is not an optimisation. The worker renders and then waits for its own interval, which for an
    /// applet is a second or more, and it re-reads what to draw only when that wait ends. Setting the name
    /// without waking it is what made every screen change take up to the interval to appear - "LR works if a
    /// little slow on the reaction".
    pub fn show(&self, visual: &str) {
        if let Ok(mut wanted) = self.wanted.lock() {
            *wanted = visual.to_string();
        }
        self.wake.wake();
    }

    /// Which screen of the applet to draw.
    ///
    /// Separate from `show` because the two are set by different things: the window's designer changes the
    /// screen while the visual stays the same, and the driver writes the screen as part of the whole status.
    /// Written here rather than by hand at each caller, so the preview cannot end up drawing a screen nobody
    /// asked for - which is how the designer came to show screen 1 no matter which screen was picked.
    pub fn show_screen(&self, screen: usize) {
        let mut status = self.wake.get();
        status.screen = screen;
        self.wake.set(status);
    }

    /// The status the worker is drawing from, for a caller that has to know.
    pub fn status(&self) -> ScreenStatus {
        self.wake.get()
    }

    /// The newest frame, if one has been drawn since it was last taken.
    pub fn take(&self) -> Option<Frame> {
        self.latest.lock().ok().and_then(|mut latest| latest.take())
    }

    /// Anything the worker had to say about the last frame.
    pub fn problems(&self) -> Vec<String> {
        self.problems
            .lock()
            .map(|list| list.clone())
            .unwrap_or_default()
    }

    /// The values the last frame was drawn from.
    pub fn values(&self) -> BTreeMap<String, g13_values::Value> {
        self.gathered
            .lock()
            .map(|values| values.clone())
            .unwrap_or_default()
    }
}

impl Drop for ScreenWorker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        // otherwise a worker waiting out its interval holds the shutdown up for as long as it is waiting
        self.wake.wake();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        // the gathering thread too, so nothing is left reading a file after the window it belonged to is gone
        if let Some(gathering) = self.gathering.take() {
            let _ = gathering.join();
        }
    }
}

/// What a binding says a control should send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send this key while the control is held.
    Key(KeyCode),
    /// Switch to this profile.
    SwitchProfile(u32),
    /// Play this macro, this many times.
    Macro(u32, u32),
    /// Press this mouse button while the control is held.
    MouseButton(g13_device::mouse::Button),
    /// Press this gamepad button while the control is held.
    GamepadButton(g13_device::joystick::Button),
    /// Start recording a macro. `Some(id)` writes over that macro; `None` takes the lowest free id.
    Record(Option<u32>),
    /// Open the menu on the pad's screen.
    ///
    /// This is what a held LR does (`LR.hold=menu`): a tap moves on, a hold opens the menu, and the two have to
    /// be told apart before either happens.
    Menu,
    /// Change what the pad's screen is showing: `next`, `prev`, or the name of a visual.
    ///
    /// The pad has controls for this - LR and L1-L4 - and this is what the vocabulary carries them as. It is an
    /// action like any other, so a line in the profile's file decides what LR does, and the default only applies
    /// to a file that says nothing.
    Screen(String),
    /// An action this build recognises but cannot perform yet.
    Unsupported(String),
    /// `x`: deliberately nothing.
    Nothing,
}

/// The world an applet resolves against, in one place.
///
/// Six call sites built this by hand, and a value the user named that worked in five of them would be the kind of
/// fault nobody finds. Those values come from `values.json`, resolved and cached until that file changes, so this
/// cheap enough to call wherever a world is wanted.
///
/// **Never called on the input loop.** Resolving a value runs a command; the loop's job is reading the pad. The
/// complaints come back rather than being printed, because this is called from threads, and the driver's own
/// reporting is the only thing allowed to speak.
fn applet_world(dir: &std::path::Path) -> (g13_sources::World, Vec<String>) {
    let (named, complaints) =
        g13_sources::named_values(&dir.join("values.json"), &visuals::values_path());
    // One reading of the clock, for everything built from this world: the visuals and the published values
    // then share one instant rather than asking the clock again for each of them.
    let world = g13_sources::World::default()
        .with_endpoints(&dir.join("endpoints.json"))
        .with_values(&visuals::values_path())
        .with_named(named)
        .with_clock(g13_sources::Clock::read());
    (world, complaints)
}

/// A binding map: what each control does, by the bit the control sets.
#[derive(Debug, Default, Clone)]
pub struct Bindings {
    /// The key each bound control sends, by the bit that control sets.
    keys: BTreeMap<usize, KeyCode>,
    /// The stick's sectors, by the name they are written under. A sector is not a control with a bit: it is a
    /// calculation on two axes, so it is kept separately and asked for by sector.
    ///
    /// Kept by *name* rather than by index because the count is a setting: `JUP` means the same direction
    /// whatever the turn is broken into, and a file written for the eight-sector stick keeps working when the
    /// count changes.
    sectors: BTreeMap<String, Bound>,
    /// Controls that switch profile, as `mk,<profile>`.
    switches: BTreeMap<usize, u32>,
    /// Controls that change what the screen shows, as `sv,<what>`.
    screens: BTreeMap<usize, String>,
    /// What a control does when it is held, as `LR.hold=menu`. The raw action, so it is parsed where it is used
    /// and the vocabulary stays in one place.
    holds: BTreeMap<usize, String>,
    /// Controls that open the menu on a tap, as `menu`.
    menus: BTreeSet<usize>,
    /// Controls that play a macro, as `m,<id>,<repeats>`.
    macros: BTreeMap<usize, (u32, u32)>,
    /// Controls that press a mouse button, as `mb,<name>`.
    mouse: BTreeMap<usize, g13_device::mouse::Button>,
    /// Controls that press a gamepad button, as `gb,<name>`.
    gamepad: BTreeMap<usize, g13_device::joystick::Button>,
    /// Controls that start a recording, as `rec` or `rec,<id>`.
    records: BTreeMap<usize, Option<u32>>,
    /// Lines the file asked for that cannot be done: the name as written, and why.
    unsupported: Vec<(String, String)>,
}

impl Bindings {
    /// Whether anything at all is bound to this bit.
    pub fn bound(&self, bit: usize) -> bool {
        self.keys.contains_key(&bit)
            || self.switches.contains_key(&bit)
            || self.screens.contains_key(&bit)
            || self.menus.contains(&bit)
            || self.macros.contains_key(&bit)
            || self.mouse.contains_key(&bit)
            || self.gamepad.contains_key(&bit)
            || self.records.contains_key(&bit)
    }

    /// Whether this control starts a recording, and the macro id it names.
    ///
    /// `Some(None)` is a control that records into the lowest free id  -  `rec`. `None` is a control that does
    /// not record at all, which is a different thing from one that records somewhere in particular.
    pub fn record_for(&self, bit: usize) -> Option<Option<u32>> {
        self.records.get(&bit).copied()
    }

    /// Give a control the `rec` action, for the default role of the record button.
    pub fn set_record(&mut self, bit: usize) {
        self.records.insert(bit, None);
    }

    /// Give a control a profile switch it was not given by the file.
    ///
    /// Only used for the M keys' default role, and only when the file says nothing about them.
    pub fn set_switch(&mut self, bit: usize, profile: u32) {
        self.switches.insert(bit, profile);
    }

    /// Give a control a screen change it was not given by the file.
    ///
    /// Only used for LR's default role, and only when the file says nothing about it.
    pub fn set_screen(&mut self, bit: usize, what: &str) {
        self.screens.insert(bit, what.to_string());
    }

    /// Build a map from a bindings file's contents.
    ///
    /// Controls are resolved from the names printed on the pad, which is what this project measured. A name
    /// that is not one of those is reported, not guessed at.
    pub fn from_text(text: &str) -> Self {
        let mut keys = BTreeMap::new();
        let mut sectors = BTreeMap::new();
        let mut switches = BTreeMap::new();
        let mut screens = BTreeMap::new();
        let mut holds = BTreeMap::new();
        let mut menus = BTreeSet::new();
        let mut macros = BTreeMap::new();
        let mut mouse = BTreeMap::new();
        let mut gamepad = BTreeMap::new();
        let mut records = BTreeMap::new();
        let mut unsupported = Vec::new();
        for (name, action) in binding_lines(text) {
            // `LR.hold=menu`: the same control, a second thing it does. The modifier is stripped here so that
            // every check below - is this a control, has it a bit - is about LR and not about a name with a dot
            // in it that no pad ever printed.
            let (name, is_hold) = g13_config::control_and_hold(&name);
            let name = name.to_string();
            // The stick's sectors come first. JUP and its seven siblings name the stick, not a control on the
            // pad, and treating them as controls once rejected them: they arrived here as "not a control this
            // project knows" and the stick did nothing.
            if g13_config::stick::Sectors::is_sector_name(&name) {
                match parse_action(&action) {
                    Action::Key(key) => {
                        sectors.insert(name, Bound::Key(key));
                    }
                    Action::MouseButton(button) => {
                        sectors.insert(name, Bound::Mouse(button));
                    }
                    Action::GamepadButton(button) => {
                        sectors.insert(name, Bound::Gamepad(button));
                    }
                    Action::Nothing => {
                        sectors.insert(name, Bound::Nothing);
                    }
                    Action::Unsupported(kind) => unsupported.push((name, kind)),
                    _ => unsupported.push((
                        name,
                        "a stick sector sends a key, a mouse button or a gamepad button, not this"
                            .to_string(),
                    )),
                }
                continue;
            }
            let Some(control) = control_from_name(&name) else {
                unsupported.push((name, "not a control this project knows".to_string()));
                continue;
            };
            let Some(bit) = bit_for_control(control) else {
                unsupported.push((name, "has no bit".to_string()));
                continue;
            };
            let parsed = parse_action(&action);
            // A hold is not a kind of action, it is *when* one happens, so it is taken first and reported here if
            // it is something this build cannot do: a `.hold` line that only fails when the key is held would be
            // a surprise at the least useful moment.
            if is_hold {
                match &parsed {
                    // a hold line this build cannot read at all is still a mistake
                    Action::Unsupported(kind) => unsupported.push((name, kind.clone())),
                    // **anything else may be held.** A hold is not a kind of action, it is *when* one happens, so
                    // every action this build can do can be the one a hold does - a key, a macro, a mouse button,
                    // a profile, a screen. It used to be `menu` and a screen change only, which was the list as it
                    // stood when those were the only two that had anywhere to go.
                    _ => {
                        holds.insert(bit, action.clone());
                    }
                }
                continue;
            }
            match parsed {
                Action::Key(key) => {
                    keys.insert(bit, key);
                }
                Action::SwitchProfile(profile) => {
                    switches.insert(bit, profile);
                }
                Action::Macro(id, repeats) => {
                    macros.insert(bit, (id, repeats));
                }
                Action::MouseButton(button) => {
                    mouse.insert(bit, button);
                }
                Action::GamepadButton(button) => {
                    gamepad.insert(bit, button);
                }
                Action::Record(target) => {
                    records.insert(bit, target);
                }
                Action::Screen(wanted) => {
                    screens.insert(bit, wanted);
                }
                // a tap can open the menu too, if that is what somebody wants
                Action::Menu => {
                    menus.insert(bit);
                }
                Action::Unsupported(kind) => unsupported.push((name, kind)),
                Action::Nothing => {}
            }
        }
        Self {
            keys,
            sectors,
            switches,
            screens,
            macros,
            mouse,
            gamepad,
            records,
            holds,
            menus,
            unsupported,
        }
    }

    /// What a sector does while the stick is held in it.
    ///
    /// Its own binding if it has one, and otherwise the cardinals it points between - which is what a keyboard
    /// can express, and what the eight-sector stick has always done: a diagonal pressed the two keys beside it.
    /// So binding only the four cardinals still gives all eight directions, and binding a sector on its own
    /// overrides that for that sector alone.
    pub fn bounds_for(&self, sectors: &g13_config::stick::Sectors, index: u32) -> Vec<Bound> {
        for name in sectors.names(index) {
            if let Some(bound) = self.sectors.get(&name) {
                return match bound {
                    Bound::Nothing => Vec::new(),
                    other => vec![*other],
                };
            }
        }
        // nothing of its own: the cardinals it leans on, in a fixed order so the same press is always the
        // same press
        let (leans_up, leans_right, leans_down, leans_left) = sectors.cardinals(index);
        let (up, right, down, left) = sectors.cardinal_sectors();
        let mut bounds: Vec<Bound> = Vec::new();
        for (leans, cardinal) in [
            (leans_up, up),
            (leans_right, right),
            (leans_down, down),
            (leans_left, left),
        ] {
            if !leans {
                continue;
            }
            for name in sectors.names(cardinal) {
                match self.sectors.get(&name) {
                    None | Some(Bound::Nothing) => continue,
                    Some(bound) => {
                        if !bounds.contains(bound) {
                            bounds.push(*bound);
                        }
                        break;
                    }
                }
            }
        }
        bounds
    }

    /// Every sector and what it will actually do, for showing. A sector with no binding of its own reports the
    /// cardinals it will fall back to, because that is what will happen.
    pub fn bound_sectors(&self, sectors: &g13_config::stick::Sectors) -> Vec<(u32, Bound)> {
        (0..sectors.count)
            .filter_map(|index| {
                self.bounds_for(sectors, index)
                    .first()
                    .map(|bound| (index, *bound))
            })
            .collect()
    }

    /// Every distinct thing a sector is bound to, for letting go of them all at once.
    pub fn all_sector_bounds(&self) -> Vec<Bound> {
        let mut bounds: Vec<Bound> = Vec::new();
        for bound in self.sectors.values() {
            if !matches!(bound, Bound::Nothing) && !bounds.contains(bound) {
                bounds.push(*bound);
            }
        }
        bounds
    }

    /// The profile a control switches to, if that is what it does.
    pub fn switch_for(&self, bit: usize) -> Option<u32> {
        self.switches.get(&bit).copied()
    }

    /// What a control does when it is held, if it has a hold: the action as the file writes it.
    pub fn hold_for(&self, bit: usize) -> Option<&str> {
        self.holds.get(&bit).map(String::as_str)
    }

    /// Whether a control opens the menu on a tap.
    pub fn opens_menu(&self, bit: usize) -> bool {
        self.menus.contains(&bit)
    }

    /// What a control changes the screen to, if that is what it does: `next`, `prev`, or a visual's name.
    pub fn screen_for(&self, bit: usize) -> Option<&str> {
        self.screens.get(&bit).map(String::as_str)
    }

    /// The macro a control plays, with how many times, if that is what it does.
    pub fn macro_for(&self, bit: usize) -> Option<(u32, u32)> {
        self.macros.get(&bit).copied()
    }

    /// The key a control sends, if that is what it does.
    pub fn key_for(&self, bit: usize) -> Option<KeyCode> {
        self.keys.get(&bit).copied()
    }

    /// The mouse button a control presses, if that is what it does.
    pub fn mouse_for(&self, bit: usize) -> Option<g13_device::mouse::Button> {
        self.mouse.get(&bit).copied()
    }

    /// The gamepad button a control presses, if that is what it does.
    pub fn gamepad_for(&self, bit: usize) -> Option<g13_device::joystick::Button> {
        self.gamepad.get(&bit).copied()
    }

    /// Whether anything in this map needs a pointer to exist, so it is only made when it is wanted.
    pub fn needs_mouse(&self) -> bool {
        !self.mouse.is_empty()
            || self
                .sectors
                .values()
                .any(|bound| matches!(bound, Bound::Mouse(_)))
    }

    /// Whether anything in this map needs a gamepad to exist.
    pub fn needs_gamepad(&self) -> bool {
        !self.gamepad.is_empty()
            || self
                .sectors
                .values()
                .any(|bound| matches!(bound, Bound::Gamepad(_)))
    }

    /// Everything bound to something this build can do.
    pub fn len(&self) -> usize {
        self.keys.len()
            + self.switches.len()
            + self.screens.len()
            + self.holds.len()
            + self.menus.len()
            + self.macros.len()
            + self.mouse.len()
            + self.gamepad.len()
            + self.sectors.len()
    }

    /// Whether the map has no key bindings at all, which is the shape a file that loaded nothing leaves.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The controls in the file this build could not use, each with the reason, for saying so once at startup.
    pub fn unsupported(&self) -> &[(String, String)] {
        &self.unsupported
    }

    /// How many controls in this file the map could not use, for reporting at startup.
    pub fn problems(&self) -> Vec<String> {
        self.unsupported
            .iter()
            .map(|(name, why)| format!("{name}: {why}"))
            .collect()
    }
}

/// What a bound action will do, in words.
///
/// One copy, shared by the window's table and `g13 bindings`, because two copies of "what this control does" is
/// how the CLI came to say `sv,next  -  not applied by this build` about an action this build applies. Nothing
/// here may be the only place an action is mentioned without words: a test walks every shape.
pub fn describe_action(action: &str) -> String {
    match parse_action(action) {
        Action::Key(code) => match g13_device::keyboard::name_for(code.code()) {
            // the key it is, not the number it is: `sends q`, because a code is a property of the key
            Some(name) => format!("sends {name}"),
            None => format!("sends keycode {}", code.code()),
        },
        Action::Macro(id, repeats) => format!("plays macro {id}, {repeats} time(s)"),
        Action::SwitchProfile(profile) => format!("switches to profile {profile}"),
        Action::Menu => "opens the menu".to_string(),
        Action::Screen(wanted) => match wanted.as_str() {
            "next" => "shows the next screen".to_string(),
            "prev" => "shows the previous screen".to_string(),
            // both of these say what they are without knowing the rotation: the driver checks the number when it
            // is used, and says what the rotation has if it is out of range
            number if number.chars().all(|c| c.is_ascii_digit()) => {
                format!("shows screen {number} of the rotation")
            }
            name => format!("shows {name}"),
        },
        Action::MouseButton(button) => format!("presses the {} mouse button", button.name()),
        Action::GamepadButton(button) => format!("presses {} on the gamepad", button.name()),
        Action::Nothing => "nothing".to_string(),
        Action::Record(None) => {
            "records a macro: press it, then a profile key, then this control. Press it again to finish."
                .to_string()
        }
        Action::Record(Some(id)) => format!(
            "records a macro into macro {id}: press it, then a profile key, then this control. Press it again to finish."
        ),
        Action::Unsupported(problem) => format!("cannot be done: {problem}"),
    }
}

/// The hold clock, with its threshold read from the profile's own file.
///
/// Read with the bindings, not once at startup: `long_press_ms=800` is a setting like `color=`, and a driver that
/// only read it at startup would be a driver you have to restart to tune.
fn holds_for(path: &Path) -> Holds {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    Holds::new(Duration::from_millis(g13_config::long_press_ms_in(&text)))
}

/// Point the screen at a visual, now: write the file, take it up in memory, and tell the worker.
///
/// One copy, because a pad press, a menu choice and a click in the window all have to land the same way, and the
/// order matters - the write first, then the local take-up, then the stamp so the watcher does not reload and
/// redraw what has already been drawn, then the wake.
fn show_target(
    target: &str,
    visuals: &mut Visuals,
    visual: &mut String,
    visuals_path: &Path,
    visuals_stamp: &mut Option<std::time::SystemTime>,
    screen: &ScreenWorker,
) -> Result<(), String> {
    visuals::write_active_visual(visuals_path, target)?;
    visuals.active = target.to_string();
    *visual = target.to_string();
    *visuals_stamp = std::fs::metadata(visuals_path)
        .and_then(|m| m.modified())
        .ok();
    screen.show(visual);
    Ok(())
}

/// Which screen of an applet a `screen:` action means.
///
/// A number counted from one, as the window and the menu count screens, or the screen's own title, which survives
/// reordering. A number nobody has, or a title no screen carries, is refused *with the list* rather than landing
/// somewhere unexpected.
fn screen_of_applet(
    visual: &str,
    target: &str,
    applets: &std::path::Path,
) -> Result<usize, String> {
    let Some(name) = visual.strip_prefix("applet:") else {
        return Err(format!(
            "`screen:` needs an applet on the screen, and {visual} is not one"
        ));
    };
    let text = std::fs::read_to_string(applets.join(format!("{name}.json")))
        .map_err(|problem| format!("cannot read {name}: {problem}"))?;
    let applet =
        g13_applets::parse_applet(&text).map_err(|problem| format!("{name}: {problem}"))?;
    let count = g13_applets::screen_count(&applet);
    let titles: Vec<String> = (0..count)
        .map(|screen| g13_applets::screen_title(&applet, screen).to_string())
        .collect();
    if let Ok(number) = target.trim().parse::<usize>() {
        return match number {
            0 => Err(format!("screens are counted from one: {titles:?}")),
            number if number <= count => Ok(number - 1),
            number => Err(format!(
                "{name} has {count} screens, not {number}: {titles:?}"
            )),
        };
    }
    // a title, which does not change when screens are moved about
    let wanted = target.trim();
    match titles
        .iter()
        .position(|title| title.eq_ignore_ascii_case(wanted))
    {
        Some(screen) => Ok(screen),
        None => Err(format!(
            "{name} has no screen called {wanted:?}: the screens are {titles:?}"
        )),
    }
}

/// Keep the chosen item's text beside the selection, and say which thing is chosen.
///
/// The item is only there when the chosen thing is a row of a list: a widget you can act on is its own thing,
/// and its command runs without an item.
fn note_selection(state: &mut State, walk: &[g13_applets::Selectable], items: &[String]) {
    state.screen_selected = if walk.is_empty() {
        0
    } else {
        state.screen_selected.min(walk.len() - 1)
    };
    state.screen_item = walk
        .get(state.screen_selected)
        .and_then(|thing| thing.item)
        .and_then(|at| items.get(at))
        .cloned()
        .unwrap_or_default();
}

/// The items behind a screen's list, read now.
///
/// The same resolver everything else uses: a list's source is an ordinary source, so `cmd:`, `file:`, `json:`,
/// `http:` and the built-in names all work and none of them is a second dialect.
fn list_items(source: &str, world: &g13_sources::World) -> (Vec<String>, Option<String>) {
    match g13_sources::resolve(&g13_sources::Spec::parse(source), world) {
        Ok(value) => (
            value
                .text()
                .lines()
                .map(str::to_string)
                .filter(|line| !line.trim().is_empty())
                .collect(),
            None,
        ),
        // said, not swallowed: an empty list and a list whose source will not read are different things, and
        // only one of them is the user's file
        Err(problem) => (Vec::new(), Some(format!("{source}: {problem}"))),
    }
}

/// The list behind the screen that is showing, kept up to date **off** the loop that reads the pad.
///
/// Reading it means running the list's own source, which can be a command or a fetch: measured on this machine
/// an applet's sources run from a fifth of a second up, which is most of a second of a keypress if it happens
/// on the loop. So a change of screen starts a thread, the thread's answer is drained at the top of a pass, and
/// a press that arrives before the answer is told the list is still being read rather than being made to wait.
struct ListWatch {
    /// The visual and screen the items in hand belong to.
    showing: (String, usize),
    /// The list of the screen in hand, empty until its reader answers and after one fails.
    items: Vec<String>,
    /// Why the list could not be read, when that is what happened.
    problem: Option<String>,
    /// The reader, while one is running: its answer is taken at the top of a pass.
    pending: Option<std::sync::mpsc::Receiver<(Vec<String>, Option<String>)>>,
}

impl ListWatch {
    /// A watch for no screen yet, so the first `follow` reads rather than matching what is showing.
    fn new() -> Self {
        Self {
            showing: (String::new(), usize::MAX),
            items: Vec::new(),
            problem: None,
            pending: None,
        }
    }

    /// Take whatever a thread has finished, if anything.
    fn drain(&mut self, state: &mut State) -> Option<String> {
        // nothing pending is nothing to say, which is what `?` means here
        let pending = self.pending.as_ref()?;
        let Ok((items, problem)) = pending.try_recv() else {
            return None;
        };
        self.pending = None;
        self.items = items;
        self.problem = problem.clone();
        if !self.items.is_empty() {
            state.screen_selected = state.screen_selected.min(self.items.len() - 1);
            return None;
        }
        state.screen_item.clear();
        Some(match problem {
            // a source that will not read is the user's file; a source that answers with nothing is its own
            // answer, and both are said once rather than leaving an empty list unexplained
            Some(problem) => format!("the list could not be read: {problem}"),
            None => format!("the list on {} answered with nothing", self.showing.0),
        })
    }

    /// Start reading the list if the showing screen is not the one the items in hand belong to.
    fn follow(
        &mut self,
        visual: &str,
        screen: usize,
        applet_dir: &Path,
        state: &mut State,
    ) -> bool {
        if self.showing == (visual.to_string(), screen) {
            return false;
        }
        self.showing = (visual.to_string(), screen);
        // a list belongs to the screen that declares it, so a change of screen starts at the top again
        self.items = Vec::new();
        self.problem = None;
        state.screen_selected = 0;
        state.screen_item.clear();
        self.pending = None;
        let Some(source) = visuals::list_source(visual, screen, applet_dir) else {
            return true;
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let world = applet_world(&g13_config::config_dir()).0;
            // the receiver is gone if the screen moved on, and then nobody wants these
            let _ = sender.send(list_items(&source, &world));
        });
        self.pending = Some(receiver);
        true
    }
}

/// How many screens the applet that is showing has. One for anything that is not an applet, and one for an applet
/// that will not parse - so a broken file is a single blank screen rather than a control that does nothing.
fn screens_of(visual: &str, applets: &Path) -> usize {
    let Some(name) = visual.strip_prefix("applet:") else {
        return 1;
    };
    let Ok(text) = std::fs::read_to_string(applets.join(format!("{name}.json"))) else {
        return 1;
    };
    match g13_applets::parse_applet(&text) {
        Ok(applet) => g13_applets::screen_count(&applet),
        Err(_) => 1,
    }
}

/// Whether a press is held back because the control has something it does when held.
///
/// `fresh` matters, and getting it wrong is how a tap of LR came to open the menu: a tap arrives at the dispatch
/// as a *release* turned back into its short action, and deferring that as though it were a new press created a
/// second entry stamped at the release - which `due` then fired half a second later, as a hold. The tap's own
/// action was never reached at all.
pub fn defer_to_hold(fresh: bool, has_hold: bool) -> bool {
    fresh && has_hold
}

/// Telling a tap from a hold.
///
/// The whole decision, in one place, with the clock passed in: a control that has a hold does nothing on press
/// until either it is let go (a tap, and the short action happens then) or the threshold passes while it is still
/// down (a hold, and the long action happens then). Controls with no hold are not tracked at all, which is what
/// keeps everything else as quick as it was.
///
/// The reason the short action has to wait for the release: firing it on press *and* opening the menu on a hold
/// would move a screen every time somebody held the button.
#[derive(Debug)]
pub struct Holds {
    /// How long a press must last to be a hold rather than a tap.
    threshold: Duration,
    /// The controls being timed, each from when it was pressed and whether its hold has already fired.
    open: BTreeMap<usize, (Instant, bool)>,
}

impl Holds {
    /// A hold clock with this threshold: held at least this long is a hold, anything shorter is a tap.
    pub fn new(threshold: Duration) -> Self {
        Self {
            threshold,
            open: BTreeMap::new(),
        }
    }

    /// A control was pressed. Returns true when it has a hold, so the caller knows the short action must wait.
    pub fn pressed(&mut self, bit: usize, now: Instant, has_hold: bool) -> bool {
        if !has_hold {
            return false;
        }
        self.open.insert(bit, (now, false));
        true
    }

    /// The controls held past the threshold that have not fired yet. Each is marked, so it fires once.
    pub fn due(&mut self, now: Instant) -> Vec<usize> {
        let threshold = self.threshold;
        let mut ready: Vec<usize> = Vec::new();
        for (bit, (started, fired)) in self.open.iter_mut() {
            if !*fired && now.duration_since(*started) >= threshold {
                *fired = true;
                ready.push(*bit);
            }
        }
        ready
    }

    /// A control was let go. Returns true when it was a tap, so the short action happens now; a hold that
    /// already fired does nothing more.
    pub fn released(&mut self, bit: usize) -> bool {
        match self.open.remove(&bit) {
            Some((_, fired)) => !fired,
            None => false,
        }
    }

    /// What is being held, for the screen and the state file.
    pub fn holding(&self, bit: usize) -> bool {
        self.open.contains_key(&bit)
    }

    /// Whether anything is being held at all, so an idle loop does no work.
    pub fn any(&self) -> bool {
        !self.open.is_empty()
    }
}

/// `p,k.<code>` sends a key; `mk,<profile>` switches profile; `m,<id>,<repeats>` plays a macro;
/// `x` nothing.
pub fn parse_action(action: &str) -> Action {
    let action = action.trim();
    if action == "x" {
        return Action::Nothing;
    }
    if let Some(rest) = action.strip_prefix("p,k.")
        && let Ok(code) = rest.trim().parse::<u16>()
    {
        return Action::Key(KeyCode::new(code));
    }
    if let Some(rest) = action.strip_prefix("mk,")
        && let Ok(profile) = rest.trim().parse::<u32>()
    {
        return Action::SwitchProfile(profile);
    }
    if let Some(rest) = action.strip_prefix("m,") {
        let mut parts = rest.split(',');
        let id = parts.next().and_then(|n| n.trim().parse::<u32>().ok());
        let repeats = parts
            .next()
            .and_then(|n| n.trim().parse::<u32>().ok())
            .unwrap_or(1);
        if let Some(id) = id {
            return Action::Macro(id, repeats.max(1));
        }
    }
    // Both spellings are accepted: the rest of the vocabulary separates with a comma, and a colon is what a
    // person reaches for when writing one of these by hand. Neither should be silently rejected.
    // recording: `rec` takes the lowest free macro id, `rec,3` writes over macro 3
    if action == "rec" {
        return Action::Record(None);
    }
    if action == "menu" {
        return Action::Menu;
    }
    // the screen: `sv,next`, `sv,prev`, or `sv,<visual>`, with the colon spelling accepted as with the rest
    if let Some(rest) = action
        .strip_prefix("sv,")
        .or_else(|| action.strip_prefix("sv:"))
    {
        let wanted = rest.trim();
        if wanted.is_empty() {
            return Action::Unsupported(
                "`sv,` names nothing: it is `sv,next`, `sv,prev`, or the name of a visual like sv,clock"
                    .to_string(),
            );
        }
        return Action::Screen(wanted.to_string());
    }
    if let Some(rest) = action.strip_prefix("rec,")
        && let Ok(id) = rest.trim().parse::<u32>()
    {
        return Action::Record(Some(id));
    }
    if action.starts_with("rec,") {
        return Action::Unsupported(format!(
            "`{action}` names no macro: recording is `rec` or `rec,<id>`, e.g. rec,3"
        ));
    }
    if let Some(rest) = action
        .strip_prefix("mb,")
        .or_else(|| action.strip_prefix("mb:"))
    {
        return match g13_device::mouse::Button::from_name(rest) {
            Some(button) => Action::MouseButton(button),
            None => Action::Unsupported(format!(
                "no mouse button called {}. The names are {}",
                rest.trim(),
                g13_device::mouse::Button::ALL
                    .iter()
                    .map(|button| button.name())
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
        };
    }
    if let Some(rest) = action
        .strip_prefix("gb,")
        .or_else(|| action.strip_prefix("gb:"))
    {
        return match g13_device::joystick::Button::from_name(rest) {
            Some(button) => Action::GamepadButton(button),
            None => Action::Unsupported(format!(
                "no gamepad button called {}. The names are {}",
                rest.trim(),
                g13_device::joystick::Button::ALL
                    .iter()
                    .map(|button| button.name())
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
        };
    }
    Action::Unsupported("not a binding action this build knows".to_string())
}

/// Name and action for every binding in a file's contents.
fn binding_lines(text: &str) -> Vec<(String, String)> {
    g13_config::bindings_from_text(text)
        .into_iter()
        .map(|binding| (binding.name, binding.action))
        .collect()
}

/// Where the bindings live: one file per profile, and the active profile's is the one that counts.
pub fn active_bindings_path() -> PathBuf {
    g13_config::bindings_path(g13_config::read_active_profile())
}

/// Where the running driver publishes what it is doing, so it can be read without a log.
pub fn state_path() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    Path::new(&dir).join(format!("g13-state-{user}.json"))
}

/// What the driver is doing, written as JSON by hand: the shape is fixed and a dependency is not needed for it.
pub struct State {
    /// Which profile's bindings are in force.
    pub profile: u32,
    /// How many controls the pad has, from the protocol's own list.
    pub controls: usize,
    /// How many of them this map gives something to do.
    pub applied: usize,
    /// What the map could not use, one entry per control, as the file's own words.
    pub problems: Vec<String>,
    /// The last controls the pad sent, newest last: its name, whether it went down, and when.
    pub last_keys: Vec<(String, bool, u64)>,
    /// When this driver started, in milliseconds since the epoch.
    pub started_ms: u64,
    /// The record wizard, while one is running. `None` the rest of the time.
    ///
    /// It lives here rather than on a thread of its own because it is the pad's state: the screen draws it,
    /// the values file publishes it, and the controls panel shows it. One answer, three readers.
    pub recording: Option<Wizard>,
    /// The menu, while one is open. Beside the wizard for the same reason: the pad's controls drive it, the
    /// screen draws it, and the state file publishes it.
    pub menu: Option<Menu>,
    /// Which screen of the applet is showing, and L1 is what moves it. Reset when the visual changes: a number
    /// chosen in one applet means nothing in another, and carrying it over would show a screen nobody asked for.
    pub screen: usize,
    /// Which of the showing screen's actionable things the pad is on, and the text of its item when it is a row
    /// of a list. L2/L3 move it and L4 runs its command. `screen_selected` counts the walk the drawing makes,
    /// which is why it is an index and not a widget number.
    pub screen_selected: usize,
    /// The text of the chosen item, when the chosen thing is a row of a list. Empty otherwise.
    pub screen_item: String,
}

impl State {
    /// The state of a driver that has just started, with this profile's map and what it could not use.
    pub fn new(profile: u32, bindings: &Bindings, problems: Vec<String>) -> Self {
        Self {
            profile,
            controls: CONTROLS.len(),
            applied: bindings.len(),
            problems,
            last_keys: Vec::new(),
            started_ms: now_ms(),
            recording: None,
            menu: None,
            screen: 0,
            screen_selected: 0,
            screen_item: String::new(),
        }
    }

    /// Record that a control went down or up, keeping only the last twelve.
    pub fn note(&mut self, control: &str, pressed: bool) {
        self.last_keys
            .push((control.to_string(), pressed, now_ms()));
        if self.last_keys.len() > 12 {
            self.last_keys.remove(0);
        }
    }

    /// The state as the JSON the file holds, with the version and pid a reader checks it by.
    pub fn to_json(&self) -> String {
        let problems = self
            .problems
            .iter()
            .map(|problem| format!("\"{}\"", escape(problem)))
            .collect::<Vec<_>>()
            .join(",");
        let keys = self
            .last_keys
            .iter()
            .map(|(control, pressed, at)| {
                format!(
                    "{{\"control\":\"{}\",\"pressed\":{},\"at_ms\":{}}}",
                    escape(control),
                    pressed,
                    at
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"version\":\"{}\",\"pid\":{},\"started_ms\":{},\"profile\":{},\"controls\":{},\"applied\":{},\"problems\":[{}],\"last_keys\":[{}]}}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            self.started_ms,
            self.profile,
            self.controls,
            self.applied,
            problems,
            keys
        )
    }

    /// Write it where `g13 values` looks. Failure is reported, never swallowed silently.
    pub fn publish(&self) -> std::io::Result<()> {
        // beside and renamed, and it matters most here: this is written many times a second and read by the
        // window while it is being written
        g13_files::write(&state_path(), &self.to_json()).map_err(std::io::Error::other)
    }
}

/// Backslash and quote escaped, so a control's name cannot end the JSON string it is written into.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Which controls are down, by name, for a screen that draws the pad itself.
///
/// The names come from the same map the bindings use (`describe_bit`), so `G1` here is the `G1` in a bindings file
/// and there is no second vocabulary. Only *controls*: the stick's own bits and the report's flag bits are not
/// controls and are left out, or a pad drawn from this would grow a row of things nobody can press.
///
/// Empty when nothing is pressed, never a missing key - a key that comes and goes is one a reader has to guess
/// about, which is the rule the stick's values already follow.
fn pressed_controls(report: &g13_proto::InputReport) -> String {
    (0..g13_proto::INPUT_BITS)
        .filter(|bit| report.is_set(*bit) && g13_proto::keymap::control_for_bit(*bit).is_some())
        .map(g13_proto::keymap::describe_bit)
        .collect::<Vec<_>>()
        .join(",")
}

/// The bits high in a report, as a space-separated list, for a sample line.
fn pressed_bits(report: &g13_proto::InputReport) -> String {
    (0..g13_proto::INPUT_BITS)
        .filter(|bit| report.is_set(*bit))
        .map(|bit| bit.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The wall clock in milliseconds since the epoch, or zero when it reads before it.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub mod visuals;

/// What the driver is doing, in the two numbers that move when the profile does.
///
/// One place owns both, because they drifted: the pad's own profile keys set the published profile and
/// nothing else did, so a profile changed *in the file* - `g13 profile 2`, an edit, the window - left
/// `g13 values` naming the profile the driver had before the switch, for as long as it ran. The map it had
/// actually loaded and the number it published disagreed, and only a restart put them back together.
///
/// Called from the pad's own switch, which can say it immediately, and from the periodic publish, which is
/// where a switch that arrived through the file is picked up.
fn what_is_in_force(state: &mut State, profile: u32, bindings: &Bindings) {
    state.profile = profile;
    state.applied = bindings.len();
}

/// Hand the screen the driver's own state, as much of it as the screen shows.
///
/// Called from the one place that derives the published stage, and woken rather than left to a timeout, so a
/// stage is drawn as it happens instead of at the end of an interval.
fn tell_screen(state: &State, wake: &ScreenWake) {
    wake.set(ScreenStatus {
        recording: state.recording.clone(),
        last_keys: state.last_keys.clone(),
        menu: state.menu.clone(),
        screen: state.screen,
        screen_selected: state.screen_selected,
        screen_item: state.screen_item.clone(),
    });
}

/// Show the active profile on the pad's own macro key lights.
///
/// M1, M2 and M3 are the profile keys, so they say which profile is in use; MR shows a recording. The device
/// keeps no state, so this is said at every start and after every change - otherwise the lights show whatever
/// they were left at, which is how a profile key came to look like it did nothing.
///
/// A refusal is returned rather than swallowed: a light that never comes on is indistinguishable from a driver
/// that never sends the report.
fn show_profile_light(device: &Device, profile: u32, recording: bool) -> Option<String> {
    device
        .write_m_keys(g13_proto::m_keys_mask(profile, recording))
        .err()
        .map(|error| format!("could not light the profile's key: {error}"))
}

/// Put a profile's colour on the screen, if its file names one.
///
/// The colour is part of the profile - `color=R,G,B` in the same file as the bindings - so it follows a profile
/// switch exactly as the bindings do. A file that names no colour leaves the screen as it is: the pad keeps
/// whatever it was last told, and inventing one would be worse than saying nothing. A refusal is returned so it
/// can be said out loud, because a screen that ignores a colour looks the same as a driver that never sent one.
pub fn show_profile_colour(device: &Device, path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let colour = g13_config::colour_in(&text)?;
    device
        .write_backlight(colour.red, colour.green, colour.blue)
        .err()
        .map(|error| {
            format!(
                "could not set the screen colour to {}: {error}",
                colour.text()
            )
        })
}

/// The pad's M keys select profiles unless the profile binds them.
///
/// M1, M2 and M3 are the pad's own profile keys. A profile file that says nothing about them should still have
/// them work  -  the prototype gave its M keys a default role rather than leaving them dead when a file did not
/// mention them  -  and a line in the file always wins, so this only fills in what is unbound.
///
/// A key is only given a default when there is a profile file to switch to: switching to one that does not
/// exist would replace the whole map with nothing, which is a worse outcome than a key that does nothing.
/// The defaults a profile file that says nothing still gets.
///
/// Named for the M keys because that is where it started; MR and LR are here for the same reason - each is what
/// the pad prints on the control, and a line in the file always wins.
pub fn with_defaults(bindings: &mut Bindings, path: &Path) {
    // Which set each M key switches to comes from profiles.json, and defaults to the number printed on the key.
    // This is the route a set is *assigned* to a key; a binding line is the override, for a key doing something
    // other than switching profile at all.
    let profiles = g13_config::Profiles::read();
    for control in [Control::M1, Control::M2, Control::M3] {
        let Some(bit) = bit_for_control(control) else {
            continue;
        };
        if bindings.bound(bit) {
            continue;
        }
        let Some(profile) = profiles.profile_for_key(&g13_proto::keymap::describe_bit(bit)) else {
            continue;
        };
        if !path
            .with_file_name(format!("bindings-{profile}.properties"))
            .exists()
        {
            continue;
        }
        bindings.set_switch(bit, profile);
    }
    // MR records, which is what the pad prints on it. The prototype left the button sending its own code
    // "unless the profile binds it", and that is the shape kept here: a line in the file wins.
    if let Some(bit) = bit_for_control(Control::MacroRecord)
        && !bindings.bound(bit)
    {
        bindings.set_record(bit);
    }
    // LR is the round button, bit 40, and it changes the screen; L1-L4 (bits 41-44) are also for changing
    // applets and menus. LR is the one with an obvious single job today, so it gets it; L1-L4 are left alone
    // until there are screens per applet for them to choose between, because a default that guesses would
    // take four controls away for a feature that does not exist.
    //
    // `bound` deliberately does not count a `.hold` line: `LR.hold=menu` says what a *held* LR does and nothing
    // about a tap, so a profile that only adds the hold must still get the tap's default. Counting it stopped
    // the tap from working at all, which is what the first run of this found.
    if let Some(bit) = bit_for_control(Control::LeftRound)
        && !bindings.bound(bit)
    {
        bindings.set_screen(bit, "next");
    }
}

/// The map as the driver will use it: what the file binds, plus the defaults a file that says nothing still
/// gets.
///
/// Read by the driver and by the tools that show what a control will do, so the table and the pad cannot
/// disagree about a key the file does not mention.
pub fn effective_bindings(text: &str, path: &Path) -> Bindings {
    let mut bindings = Bindings::from_text(text);
    with_defaults(&mut bindings, path);
    bindings
}

/// Read a bindings file and add the defaults. A missing file is an empty map, not an error: a driver with no
/// map is a driver that sends nothing, which is a state a person can see and fix.
pub fn load_bindings(path: &Path) -> Bindings {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    effective_bindings(&text, path)
}

/// What the loop is doing, reported as it happens rather than discovered later.
#[derive(Debug)]
pub enum Event {
    /// Something the driver has to say that is not about the screen: a key it could not send, a macro that is
    /// not there, a recording that gave up.
    ///
    /// These used to be collected into a list and never shown, which made every one of them silent - including
    /// "`sv,custom` asks for custom, which is not a visual this build can draw". A complaint nobody can read is
    /// the same as no complaint.
    Trouble {
        /// What went wrong, one entry per thing, in the driver's own words.
        problems: Vec<String>,
    },
    /// The driver has started and read its first bindings file.
    Started {
        /// How many controls the pad has.
        controls: usize,
        /// Which profile's bindings are in force.
        profile: u32,
        /// What the map could not use, one entry per control.
        problems: Vec<String>,
    },
    /// A control went down or up, and what it sends.
    Key {
        /// The control's bit in the input report.
        bit: usize,
        /// What the control is called and what it does, in words.
        description: String,
        /// True when it went down, false when it came up.
        pressed: bool,
    },
    /// A control bound to a profile key changed the profile.
    ProfileSwitched {
        /// The profile now in force, which is the one the pad's lights show.
        profile: u32,
        /// How many controls the new profile's map gives something to do.
        controls: usize,
    },
    /// A macro finished playing from a control.
    MacroPlayed {
        /// The macro's id in its profile.
        id: u32,
        /// The macro's name, as its file writes it.
        name: String,
        /// How many steps it has, so a caller can see it was not empty.
        steps: usize,
        /// How many times it was played.
        repeats: u32,
    },
    /// A recording has started: the macro it will be written to.
    RecordingStarted {
        /// The macro that will be written.
        id: u32,
    },
    /// A control has been chosen: the macro being recorded will play from it, in this profile.
    RecordingControl {
        /// The profile whose file the binding will go into.
        profile: u32,
        /// The control that will play it, as the pad prints it.
        control: String,
    },
    /// A recording finished and was written, or was not because nothing was pressed.
    MacroRecorded {
        /// The macro that was written.
        id: u32,
        /// Its name, which is the one it already had if it recorded over one.
        name: String,
        /// How many steps were taken.
        steps: usize,
        /// The profile whose file the binding was written to.
        profile: u32,
        /// The control that now plays it.
        control: String,
    },
    /// The bindings file changed and was read again.
    Reloaded {
        /// How many controls the new map gives something to do.
        controls: usize,
    },
    /// The screen was drawn, and anything about it worth saying.
    Drew {
        /// The visual that was drawn.
        visual: String,
        /// Anything worth saying about the frame, such as a source that answered with an error.
        problems: Vec<String>,
    },
}

/// Run the driver until `keep_going` says otherwise.
/// The pad is shared: this loop reads it and the screen thread writes the panel. One handle, two threads, because
/// the input loop must never wait on a 26ms frame write.
pub fn run<F>(device: Arc<Device>, mut report: F) -> Result<(), g13_device::DeviceError>
where
    F: FnMut(Event) -> bool,
{
    let path = active_bindings_path();
    let mut bindings = load_bindings(&path);
    let mut stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

    let mut profile = g13_config::read_active_profile();
    let keyboard = match keyboard::VirtualKeyboard::new() {
        Ok(keyboard) => Arc::new(Mutex::new(keyboard)),
        Err(error) => {
            report(Event::Started {
                controls: 0,
                profile: 0,
                problems: vec![format!("no virtual keyboard: {error}")],
            });
            return Ok(());
        }
    };

    let mut problems = bindings.problems();
    if let Some(complaint) = show_profile_colour(&device, &path) {
        problems.push(complaint);
    }
    // the pad's own lights: it comes up with all four out, so the active profile has to be shown here
    if let Some(complaint) = show_profile_light(&device, profile, false) {
        problems.push(complaint);
    }
    let mut state = State::new(profile, &bindings, problems.clone());
    let _ = state.publish();
    if !report(Event::Started {
        controls: bindings.len(),
        profile,
        problems,
    }) {
        return Ok(());
    }
    let mut published = Instant::now();
    // the recording in progress, if any. Only one at a time: a second `rec` while one runs stops it.
    let mut recording: Option<Recording> = None;

    // the stick's raw bytes, once each, until its encoding is established by a sweep
    // every distinct reading of the stick's sixteen bits, written once each. The file is capped, because it
    // is a measurement aid rather than a log, and removed by hand once the encoding is established.
    let stick_log_path = g13_config::config_dir().join("stick-samples.csv");
    let mut last_stick: Option<[u8; 2]> = None;
    // The stick's settings, read at startup and watched: the window writes them, and a driver that read them
    // once would ignore the mode being turned on - which reads as the window not working, again.
    let stick_path = g13_config::stick::stick_path();
    let mut stick_settings = g13_config::stick::Settings::load(&g13_config::config_dir());
    let mut stick_stamp = std::fs::metadata(&stick_path)
        .and_then(|details| details.modified())
        .ok();
    // what the stick is holding, so a direction change is one release and one press
    let mut stick_state = stick_settings.stick.clone();
    // the pointer, if mouse mode ever wants one. Started here rather than on a mode change because creating
    // a device from inside the read loop is how a driver drops a packet.
    // The pointer and the gamepad are made when a mode that needs one is chosen, which is inside the worker.
    // A device it cannot create comes back through `problem` and is said once, because a driver with no
    // pointer while mouse mode is selected behaves exactly like a driver whose pointer does nothing.
    // set whenever the pad reports anything, so the published values follow the pad and not the clock
    let mut changed = false;
    let mut values_due = Instant::now();
    // the values that change slowly and cost a great deal to gather, kept between publishes

    let mut previous = g13_proto::InputReport::default();
    // device state that is not input: whatever is set before any key is pressed
    {
        let settle = Instant::now() + Duration::from_millis(1200);
        while Instant::now() < settle {
            if let Ok(Some(packet)) = device.read_packet(Duration::from_millis(200))
                && let Some(report_now) = g13_proto::InputReport::from_packet(&packet)
            {
                previous = report_now;
            }
        }
    }

    let profile_path = g13_config::config_dir().join("active-profile");
    let mut profile_stamp = std::fs::metadata(&profile_path)
        .and_then(|m| m.modified())
        .ok();
    let mut last_check = Instant::now();

    // the screen: what to draw, how often, and what has already been said about it
    let visuals_path = g13_config::config_dir().join("visuals.json");
    let applet_dir = g13_config::config_dir().join("applets");
    let mut visuals = visuals::load_visuals(&visuals_path);
    // the list on the showing screen, read off the loop and drained at the top of each pass
    let mut list_watch = ListWatch::new();
    // what a screen's `select` had to say, for the same reason: a command is not run on the loop, so its
    // outcome has to come back to the loop's own reporting rather than being printed from a thread
    let mut select_report: Option<std::sync::mpsc::Receiver<String>> = None;
    // a macro plays on a thread of its own, which cannot reach the reporter: what it has to say is sent
    // here and said by the loop, so a library still never prints
    let mut macro_report: Option<std::sync::mpsc::Receiver<String>> = None;
    // watched, not read once: the window writes this file, and a driver that only reads it at startup
    // ignores every change made in it, which reads as "the window cannot change the screen"
    let mut visuals_stamp = std::fs::metadata(&visuals_path)
        .and_then(|m| m.modified())
        .ok();
    let mut visual = visuals.active.clone();
    // started once the visual is known
    // what the screen shows of the driver's own state, handed over as it changes
    let screen_wake = ScreenWake::default();
    let workers = Workers {
        screen: ScreenWorker::start(
            visual.clone(),
            g13_config::config_dir().join("applets"),
            g13_config::config_dir(),
            // the panel is written on the screen's thread: one frame costs about 26ms and this loop is the pad's
            // responsiveness, so it reads the pad and publishes and does nothing else
            Some(Panel::claimed(&device)),
            screen_wake.clone(),
        ),
        // the pointer and the gamepad are made inside the worker, when a mode that needs one is chosen: a
        // device it cannot create comes back through `problem` and is said once, because a driver with no
        // pointer while mouse mode is selected behaves exactly like a driver whose pointer does nothing
        stick: StickWorker::start(stick_settings.clone()),
    };
    workers.set_needs(&bindings);
    let mut since_cycle = Instant::now();
    let cycle_every = Duration::from_secs_f64(visuals.cycle_seconds.clamp(1.0, 600.0));
    // a problem is worth saying once, not sixty times a minute
    let mut told: Vec<String> = Vec::new();
    // A program that feeds an applet takes the screen while it runs: see `visuals::Follower`. Started here, and
    // asked first about the screen the file names, because a driver restarted after a game was closed must not
    // come up on that game's screen with stale numbers on it.
    let mut a_follow = visuals::Follower::new();
    if let Some(other) = visuals::start_on(
        &visuals.enabled,
        &visuals.active,
        &applet_dir,
        std::time::SystemTime::now(),
    ) {
        if visuals::write_active_visual(&visuals_path, &other).is_ok() {
            visuals.active = other.clone();
            visual = other.clone();
            workers.screen.show(&visual);
            told.push(format!(
                "{other} is showing: the screen the file named is fed by a program that is not running"
            ));
        }
    }
    // How long a hold is, from the profile's own file, re-read whenever the bindings are
    let mut holds = holds_for(&path);
    // A screen that is enabled and that this build cannot draw is skipped by `next` and `prev`. That is the right
    // behaviour and it is also invisible, so it is said once at the start: an enabled screen that will never
    // appear is something to know about, not something to work out from a rotation that steps over it.
    told.extend(visuals::undrawable_in(&visuals.enabled, &applet_dir));
    if !report(Event::Drew {
        visual: visual.clone(),
        problems: vec![format!(
            "drawing {visual} every {}s from {}",
            visuals::interval_for(&visual, &applet_dir).as_secs_f64(),
            visuals_path.display()
        )],
    }) {
        return Ok(());
    }
    loop {
        // Anything the driver had to say during the last pass is said now. These were collected into a list and
        // never shown, which made every one of them silent - including the one that says why a screen did not
        // change. A complaint nobody can read is the same as no complaint.
        if !told.is_empty() {
            let problems = std::mem::take(&mut told);
            state.problems = problems.clone();
            let _ = state.publish();
            if !report(Event::Trouble { problems }) {
                return Ok(());
            }
        }
        match device.read_packet(Duration::from_millis(200)) {
            Ok(Some(packet)) => {
                let Some(now) = g13_proto::InputReport::from_packet(&packet) else {
                    continue;
                };
                // While the stick's encoding is unestablished, every distinct reading of its sixteen bits is
                // written down. A sweep of the stick then says what the numbers mean, and this can happen
                // while the driver is running rather than instead of it.
                {
                    let raw = &packet[1..3.min(packet.len())];
                    if raw.len() == 2 && Some([raw[0], raw[1]]) != last_stick {
                        last_stick = Some([raw[0], raw[1]]);
                        // the stick sets no key bits, so without this a movement would only be published on
                        // the timer below - twenty times a second, unevenly, which is what reads as lag
                        changed = true;
                        // And the stick drives the keys its directions are bound to. It is a calculation on
                        // two axes, so this is where a direction becomes a keypress: the parts of the new
                        // direction are pressed and the parts of the old one that it does not share are
                        // released, which is what makes a diagonal keep the direction it came from.
                        let reading = (raw[0], raw[1]);
                        // the pointer follows the stick whatever else is happening, so it has the newest
                        // reading rather than one from the last time a direction changed
                        workers.stick.report(reading);
                        if let Some((released, pressed)) = apply_stick(
                            reading,
                            &mut stick_state,
                            &stick_settings,
                            &bindings,
                            &keyboard,
                            &workers.stick,
                        ) {
                            if let Some(index) = pressed {
                                state.note(
                                    &format!("stick {}", stick_settings.sectors.words(index)),
                                    true,
                                );
                            } else if let Some(index) = released {
                                state.note(
                                    &format!("stick {}", stick_settings.sectors.words(index)),
                                    false,
                                );
                            }
                        }
                        let line = format!(
                            "{},0x{:02x},0x{:02x},{}\n",
                            now_ms(),
                            raw[0],
                            raw[1],
                            pressed_bits(&now)
                        );
                        let small_enough = std::fs::metadata(&stick_log_path)
                            .map(|details| details.len() < 200_000)
                            .unwrap_or(true);
                        if small_enough
                            && let Ok(mut file) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&stick_log_path)
                        {
                            let _ = std::io::Write::write_all(&mut file, line.as_bytes());
                        }
                    }
                }
                // A recording takes its keys from the real keyboards on another thread, so they are collected
                // here between two reads of the pad - which is what lets the pad itself finish the recording.
                if let Some(Recording::Capturing {
                    events,
                    keys,
                    last,
                    started,
                    ..
                }) = recording.as_mut()
                {
                    while let Ok(event) = keys.try_recv() {
                        events.push(event);
                        *last = std::time::Instant::now();
                    }
                    let idle = last.elapsed() > RECORD_IDLE;
                    let longest = started.elapsed() > RECORD_LONGEST;
                    if idle || longest {
                        if longest && !idle {
                            // say why it ended by itself, so a long recording does not look like a dead key
                            told.push(format!(
                                "the recording reached its {}s limit",
                                RECORD_LONGEST.as_secs()
                            ));
                        }
                        if let Some(Recording::Capturing {
                            id,
                            name,
                            profile: target,
                            control,
                            events,
                            ..
                        }) = recording.take()
                        {
                            let (event, complaints) = finish_recording(
                                &g13_config::config_dir(),
                                target,
                                &control,
                                id,
                                &name,
                                &events,
                            );
                            told.extend(complaints);
                            if let Some(complaint) = show_profile_light(&device, profile, false) {
                                told.push(complaint);
                            }
                            if !report(event) {
                                return Ok(());
                            }
                        }
                    }
                }
                // and a wizard left waiting is not allowed to hold the pad's bindings off for ever either
                if let Some(Recording::ChoosingControl { started, .. }) = recording.as_ref()
                    && started.elapsed() > std::time::Duration::from_secs(CHOOSING_SECONDS)
                {
                    recording = None;
                    if let Some(complaint) = show_profile_light(&device, profile, false) {
                        told.push(complaint);
                    }
                    told.push(format!(
                        "recording cancelled: no control was chosen within {CHOOSING_SECONDS}s"
                    ));
                }

                // What the pad shows follows what the loop is doing, derived in one place after this
                // iteration's presses have been handled - so a finished recording cannot go on being
                // published as running, and a profile key that changed nothing still shows as taken.
                let showing = published_stage(&recording, profile);
                if state.recording != showing {
                    state.recording = showing;
                    let _ = state.publish();
                    tell_screen(&state, &screen_wake);
                }

                let (pressed, released) = now.changes_from(&previous);
                previous = now;
                if !pressed.is_empty() || !released.is_empty() {
                    changed = true;
                }
                // A tap on a control that has a hold becomes that control's short action here, at the release: until
                // the release it might have been a hold, and a hold must not also do the tap's work. Controls with
                // no hold are not in this at all, so nothing else is any slower than it was.
                let taps: Vec<usize> = released
                    .iter()
                    .copied()
                    .filter(|bit| holds.released(*bit))
                    .collect();
                for bit in pressed.iter().copied().chain(taps.iter().copied()) {
                    // The record button: it starts the wizard, finishes it, or gives up on it. MR takes this by
                    // default, and any control bound to `rec` does the same.
                    if let Some(target) = bindings.record_for(bit) {
                        match recording.take() {
                            None => {
                                let (id, choosing) = start_choosing(target);
                                recording = Some(choosing);
                                if let Some(complaint) = show_profile_light(&device, profile, true)
                                {
                                    told.push(complaint);
                                }
                                if !report(Event::RecordingStarted { id }) {
                                    return Ok(());
                                }
                            }
                            Some(Recording::ChoosingControl { .. }) => {
                                if let Some(complaint) = show_profile_light(&device, profile, false)
                                {
                                    told.push(complaint);
                                }
                                told.push("recording cancelled: no control was chosen".to_string());
                            }
                            Some(Recording::Capturing {
                                id,
                                name,
                                profile: target,
                                control,
                                keys: _,
                                events,
                                ..
                            }) => {
                                let (event, complaints) = finish_recording(
                                    &g13_config::config_dir(),
                                    target,
                                    &control,
                                    id,
                                    &name,
                                    &events,
                                );
                                told.extend(complaints);
                                if let Some(complaint) = show_profile_light(&device, profile, false)
                                {
                                    told.push(complaint);
                                }
                                if !report(event) {
                                    return Ok(());
                                }
                            }
                        }
                        continue;
                    }
                    // While the wizard runs the pad's bindings do not fire: the control being pressed is the
                    // answer, not a key to send. A profile key is the exception - it switches profile as it
                    // always does, because that switch *is* the answer to "which profile".
                    if let Some(Recording::ChoosingControl { id, name, .. }) = recording.as_ref() {
                        if bindings.switch_for(bit).is_some() {
                            // falls through to the ordinary handling below, which switches the profile and its
                            // light with it, and the wizard carries on waiting for a control
                        } else if g13_proto::control_for_bit(bit).is_none() {
                            // A flag or a stick bit is not a control and cannot be the answer. Bit 55 toggles
                            // between reports on its own, and taking it as the answer is how a wizard came to
                            // bind itself to a bit nobody pressed.
                            continue;
                        } else {
                            let control = g13_proto::describe_bit(bit);
                            let (id, name) = (*id, name.clone());
                            // the pad's own last-key list is a list of controls, so the control that
                            // answered goes in it and nothing else does
                            state.note(&control, true);
                            recording = Some(start_capturing(id, name, profile, control.clone()));
                            if !report(Event::RecordingControl { profile, control }) {
                                return Ok(());
                            }
                            continue;
                        }
                    }
                    if matches!(recording, Some(Recording::Capturing { .. })) {
                        // capturing: the pad is only listened to for the record button, handled above, so a
                        // press here is the person's hand resting on the pad. Key *releases* are still applied
                        // below, because a key held when the recording started must not be left down.
                        continue;
                    }
                    // What the list's reader came back with, and whether the screen has moved on to another
                    // one. Both are done here, before any press is read, so a press in this pass sees the list
                    // the screen actually has.
                    if let Some(said) = list_watch.drain(&mut state) {
                        if !report(Event::Trouble {
                            problems: vec![said],
                        }) {
                            return Ok(());
                        }
                    }
                    if let Some(from_thread) = select_report.as_ref() {
                        if let Ok(said) = from_thread.try_recv() {
                            select_report = None;
                            if !report(Event::Trouble {
                                problems: vec![said],
                            }) {
                                return Ok(());
                            }
                        }
                    }
                    if let Some(from_thread) = macro_report.as_ref() {
                        let mut said: Vec<String> = Vec::new();
                        while let Ok(one) = from_thread.try_recv() {
                            said.push(one);
                        }
                        if !said.is_empty() {
                            macro_report = None;
                            if !report(Event::Trouble { problems: said }) {
                                return Ok(());
                            }
                        }
                    }
                    if list_watch.follow(&visual, state.screen, &applet_dir, &mut state) {
                        let _ = state.publish();
                        tell_screen(&state, &screen_wake);
                    }

                    // The menu owns L1-L4 while it is open - back, previous, next, select. This is the only place
                    // a screen takes a control away from the profile's file, and only for as long as it is open:
                    // shut, every control does exactly what the file says.
                    if state.menu.is_some() {
                        if let Some(key) = menu_key(bit) {
                            state.note(&g13_proto::describe_bit(bit), true);
                            let _ = state.publish();
                            tell_screen(&state, &screen_wake);
                            let menu = match state.menu.take() {
                                Some(menu) => menu,
                                None => continue,
                            };
                            let said = match menu_move(menu, key, &applet_dir) {
                                MenuMove::Stayed(moved) => {
                                    let at = moved.index + 1;
                                    let of = moved.items.len();
                                    state.menu = Some(moved);
                                    Some(format!("menu {at}/{of}"))
                                }
                                MenuMove::Deeper(below, level) => {
                                    let count = level.items.len();
                                    let title = level.title.clone();
                                    state.menu = Some(below.open_over(level));
                                    Some(format!("{title}: {count} screens, level 2 of the menu"))
                                }
                                MenuMove::Wound(below) => match below {
                                    Some(level) => {
                                        let said = format!("back to level {}", level.level);
                                        state.menu = Some(level);
                                        Some(said)
                                    }
                                    None => Some("the menu is closed".to_string()),
                                },
                                MenuMove::Chose(target) => {
                                    if let MenuTarget::ShowAt { screen, .. } = &target {
                                        state.screen = *screen;
                                        let _ = state.publish();
                                    }
                                    // An item that runs a command does that instead of showing anything: the
                                    // menu closes, the pad stays where it is, and the run goes off the loop.
                                    if let MenuTarget::Run(command) = &target {
                                        let values = visuals::read_published_values();
                                        let to_run = g13_applets::fill_command(command, &values);
                                        let shown = to_run.clone();
                                        let (sender, receiver) = std::sync::mpsc::channel();
                                        select_report = Some(receiver);
                                        std::thread::spawn(move || {
                                            let world = applet_world(&g13_config::config_dir()).0;
                                            let spec = g13_sources::Spec::parse(&to_run);
                                            let said = match g13_sources::resolve(&spec, &world) {
                                                Ok(_) => format!("{to_run} ran"),
                                                Err(problem) => {
                                                    format!("{to_run} failed: {problem}")
                                                }
                                            };
                                            let _ = sender.send(said);
                                        });
                                        state.menu = None;
                                        tell_screen(&state, &screen_wake);
                                        if !report(Event::Trouble {
                                            problems: vec![format!("{shown} started")],
                                        }) {
                                            return Ok(());
                                        }
                                        continue;
                                    }
                                    // an item of the user's own with something under it opens them, a level
                                    // down
                                    if let MenuTarget::Child(item) = &target {
                                        match child_level(item) {
                                            Some(level) => {
                                                let over = state
                                                    .menu
                                                    .take()
                                                    .map(|menu| menu.open_over(level));
                                                state.menu = over;
                                                let said = format!(
                                                    "{} is open over {} item(s)",
                                                    item.label,
                                                    state
                                                        .menu
                                                        .as_ref()
                                                        .map(|menu| menu.items.len())
                                                        .unwrap_or(0)
                                                );
                                                tell_screen(&state, &screen_wake);
                                                if !report(Event::Trouble {
                                                    problems: vec![said],
                                                }) {
                                                    return Ok(());
                                                }
                                                continue;
                                            }
                                            None => {
                                                let said =
                                                    format!("{} has nothing under it", item.label);
                                                state.menu = None;
                                                tell_screen(&state, &screen_wake);
                                                if !report(Event::Trouble {
                                                    problems: vec![said],
                                                }) {
                                                    return Ok(());
                                                }
                                                continue;
                                            }
                                        }
                                    }
                                    tell_screen(&state, &screen_wake);
                                    let showing = match &target {
                                        MenuTarget::Show(name) => Some(name.clone()),
                                        MenuTarget::ShowAt { visual, .. } => Some(visual.clone()),
                                        _ => None,
                                    };
                                    match showing {
                                        Some(target) => show_target(
                                            &target,
                                            &mut visuals,
                                            &mut visual,
                                            &visuals_path,
                                            &mut visuals_stamp,
                                            &workers.screen,
                                        )
                                        .err()
                                        .map(|problem| problem.to_string()),
                                        None => None,
                                    }
                                }
                                MenuMove::Refused(why) => Some(why),
                            };
                            tell_screen(&state, &screen_wake);
                            if let Some(said) = said {
                                if !report(Event::Trouble {
                                    problems: vec![said],
                                }) {
                                    return Ok(());
                                }
                            }
                            continue;
                        }
                    }
                    // L1 is the applet's own control: with the menu shut it moves to the applet's next workers.screen.
                    // Only when the applet *has* more than one, and only while a screen is what is showing - a
                    // control gets first refusal here only where there is something for it to do: L1-L4 are never
                    // quietly claimed by the menu, and outside it they do whatever the profile's file says.
                    if state.menu.is_none() && g13_proto::control_for_bit(bit) == Some(Control::L1)
                    {
                        let count = screens_of(&visual, &applet_dir);
                        if count > 1 {
                            state.screen = (state.screen + 1) % count;
                            state.note(&g13_proto::describe_bit(bit), true);
                            let _ = state.publish();
                            tell_screen(&state, &screen_wake);
                            if !report(Event::Trouble {
                                problems: vec![format!(
                                    "screen {} of {count} of {visual}",
                                    state.screen + 1
                                )],
                            }) {
                                return Ok(());
                            }
                            continue;
                        }
                    }
                    // L2, L3 and L4 drive whatever the showing screen says can be acted on: previous, next,
                    // and run its command. A widget with a `command` is one of those things; a list is one per
                    // row. Where a screen has nothing of the sort they keep the profile's bindings, so the
                    // binding system doesn't quietly take them - the rule L1 follows too.
                    let walk = visuals::screen_walk(
                        &visual,
                        state.screen,
                        &applet_dir,
                        &list_watch.items,
                        // what the screen is drawing from, so the walk agrees with what is on it
                        &workers.screen.values(),
                        state.profile,
                    );
                    if !walk.is_empty() {
                        let control = g13_proto::control_for_bit(bit);
                        if matches!(
                            control,
                            Some(Control::L2) | Some(Control::L3) | Some(Control::L4)
                        ) {
                            let said = match control {
                                Some(Control::L2) | Some(Control::L3) => {
                                    state.screen_selected = step_selection(
                                        state.screen_selected,
                                        walk.len(),
                                        control == Some(Control::L2),
                                    );
                                    note_selection(&mut state, &walk, &list_watch.items);
                                    let thing = &walk[state.screen_selected];
                                    format!(
                                        "{} of {}: {}",
                                        state.screen_selected + 1,
                                        walk.len(),
                                        thing.label
                                    )
                                }
                                // L4: run the command of whatever is chosen, off the loop like a macro, with
                                // the chosen row available to it as `{screen_item}` where there is one
                                _ => {
                                    let thing = walk[state.screen_selected].clone();
                                    note_selection(&mut state, &walk, &list_watch.items);
                                    let item = state.screen_item.clone();
                                    let _ = state.publish();
                                    let values = visuals::read_published_values();
                                    // Everything this widget does, in order. `{screen_item}` is filled the way a
                                    // widget fills its own format: one resolver for a name, not a second one
                                    // hidden inside commands.
                                    let mut commands: Vec<String> = Vec::new();
                                    for one in &thing.commands {
                                        let filled = g13_applets::fill_command(one, &values);
                                        match filled.strip_prefix("screen:") {
                                            // A screen change is decided here, on the loop, because this is where
                                            // the screen is decided - a thread cannot change it any better than
                                            // this can.
                                            None => commands.push(filled),
                                            Some(target) => {
                                                match screen_of_applet(&visual, target, &applet_dir)
                                                {
                                                    Err(problem) => told.push(problem),
                                                    Ok(screen_at) => {
                                                        state.screen = screen_at;
                                                        workers.screen.show_screen(screen_at);
                                                        tell_screen(&state, &screen_wake);
                                                        let _ = state.publish();
                                                        if !report(Event::Drew {
                                                            visual: visual.clone(),
                                                            problems: vec![format!(
                                                                "now on screen {} of {}",
                                                                screen_at + 1,
                                                                visual
                                                            )],
                                                        }) {
                                                            return Ok(());
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if commands.is_empty() {
                                        // nothing left to run: the screen change was the whole of it
                                        continue;
                                    }
                                    let shown = commands.join(" then ");
                                    let (sender, receiver) = std::sync::mpsc::channel();
                                    select_report = Some(receiver);
                                    std::thread::spawn(move || {
                                        let world = applet_world(&g13_config::config_dir()).0;
                                        let mut ran: Vec<String> = Vec::new();
                                        for to_run in &commands {
                                            let spec = g13_sources::Spec::parse(to_run);
                                            ran.push(match g13_sources::resolve(&spec, &world) {
                                                Ok(_) => match item.is_empty() {
                                                    true => format!("{to_run} ran"),
                                                    false => format!("{to_run} ran for {item}"),
                                                },
                                                Err(problem) => {
                                                    format!("{to_run} failed: {problem}")
                                                }
                                            });
                                        }
                                        let _ = sender.send(ran.join("; "));
                                    });
                                    format!("{shown} started")
                                }
                            };
                            state.note(&g13_proto::describe_bit(bit), true);
                            let _ = state.publish();
                            tell_screen(&state, &screen_wake);
                            if !report(Event::Trouble {
                                problems: vec![said],
                            }) {
                                return Ok(());
                            }
                            continue;
                        }
                    }
                    // A control with something it does when held waits for the release. Until then it might be a
                    // hold, so neither thing happens yet - and a tap arriving here as its own short action is not
                    // a fresh press: deferring it again would fire the hold half a second later.
                    let fresh = pressed.contains(&bit) && !taps.contains(&bit);
                    if defer_to_hold(fresh, bindings.hold_for(bit).is_some())
                        && holds.pressed(bit, Instant::now(), true)
                    {
                        state.note(&g13_proto::describe_bit(bit), true);
                        let _ = state.publish();
                        tell_screen(&state, &screen_wake);
                        continue;
                    }
                    // and a control bound to `menu` opens it on the tap
                    if bindings.opens_menu(bit) {
                        state.note(&g13_proto::describe_bit(bit), true);
                        let _ = state.publish();
                        match open_menu(&visuals, &applet_dir, &visual) {
                            Err(problem) => told.push(problem),
                            Ok((menu, complaint)) => {
                                if let Some(complaint) = complaint {
                                    told.push(complaint);
                                }
                                let said = menu.items.len();
                                state.menu = Some(menu);
                                tell_screen(&state, &screen_wake);
                                if !report(Event::Trouble {
                                    problems: vec![format!(
                                        "the menu is open over {said} screen(s): L2 and L3 move, L4 chooses, L1 goes back"
                                    )],
                                }) {
                                    return Ok(());
                                }
                            }
                        }
                        continue;
                    }
                    // a control that switches profile does that instead of sending a key
                    if let Some(wanted) = bindings.switch_for(bit) {
                        // the key is taken as the answer whether or not it changes the profile: it is
                        // marked first, so a press on the profile already in force still shows
                        if let Some(Recording::ChoosingControl { picked, .. }) = recording.as_mut()
                        {
                            *picked = Some(g13_proto::describe_bit(bit));
                        }
                        // and it is taken as a press in its own right, whether or not it changed anything:
                        // this list is what the pad's own LAST line and the values file read
                        state.note(&g13_proto::describe_bit(bit), true);
                        let _ = state.publish();
                        tell_screen(&state, &screen_wake);
                        if wanted != profile {
                            let _ = g13_config::write_active_profile(wanted);
                            profile = wanted;
                            let switched = g13_config::bindings_path(profile);
                            bindings = load_bindings(&switched);
                            holds = holds_for(&switched);

                            // the light follows the profile, because it is a line in the profile's own file
                            if let Some(complaint) = show_profile_colour(&device, &switched) {
                                told.push(complaint);
                            }
                            // on the pad as well: the profile key lights up to say which profile is in use,
                            // and the record button stays lit if a wizard is running
                            if let Some(complaint) =
                                show_profile_light(&device, profile, recording.is_some())
                            {
                                told.push(complaint);
                            }
                            // a new map may want a device that was not needed before, so the worker is told
                            workers.set_needs(&bindings);
                            what_is_in_force(&mut state, profile, &bindings);
                            if !report(Event::ProfileSwitched {
                                profile,
                                controls: bindings.len(),
                            }) {
                                return Ok(());
                            }
                        }
                        continue;
                    }
                    // a control that changes the workers.screen. LR does this by default and the file can put it on any
                    // control; the write is the whole of it, because the loop watches this file. So a press on
                    // the pad lands the same way as a click in the window and the two cannot drift apart.
                    if let Some(wanted) = bindings.screen_for(bit).map(str::to_string) {
                        state.note(&g13_proto::describe_bit(bit), true);
                        let _ = state.publish();
                        tell_screen(&state, &screen_wake);
                        // what to show, and whether asking at all was a mistake: the decision is in
                        // `visuals::screen_target`, where it is tested without a pad
                        a_follow.user_chose(&wanted);
                        match visuals::screen_target(
                            &wanted,
                            &visuals.enabled,
                            &visuals.active,
                            &applet_dir,
                        ) {
                            Err(problem) => told.push(problem),
                            Ok(None) => {}
                            Ok(Some(target)) => {
                                // The write, the take-up, the stamp and the wake, in one place: the frame section
                                // of this pass runs *before* the watcher that would notice the file, so taking it
                                // up here is what makes a press land now rather than a pass later.
                                match show_target(
                                    &target,
                                    &mut visuals,
                                    &mut visual,
                                    &visuals_path,
                                    &mut visuals_stamp,
                                    &workers.screen,
                                ) {
                                    Err(problem) => told.push(problem),
                                    Ok(()) => {
                                        since_cycle = Instant::now();
                                        if !report(Event::Drew {
                                            visual: target.clone(),
                                            problems: vec![format!(
                                                "the screen now shows {target} (from the pad)"
                                            )],
                                        }) {
                                            return Ok(());
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    // a control that plays a macro hands it to a thread and carries on reading the pad
                    if let Some((id, repeats)) = bindings.macro_for(bit) {
                        // A macro with a graph is played as a graph. A graph that will not parse is said out
                        // loud and nothing is played: falling back to the plain sequence would be playing
                        // something other than what the file asks for.
                        let (macro_file, graph) = match g13_config::read_playable(id) {
                            Ok(Some(both)) => both,
                            Ok(None) => {
                                told.push(format!("macro {id} is not there to play"));
                                continue;
                            }
                            Err(problem) => {
                                told.push(problem);
                                continue;
                            }
                        };
                        let name = macro_file.name.clone();
                        let queue = graph
                            .as_ref()
                            .map(|graph| graph.nodes.len())
                            .unwrap_or(macro_file.steps.len());
                        if !report(Event::MacroPlayed {
                            id,
                            name,
                            steps: queue,
                            repeats,
                        }) {
                            return Ok(());
                        }
                        state.note(&g13_proto::describe_bit(bit), true);
                        let _ = state.publish();
                        tell_screen(&state, &screen_wake);
                        let keyboard = Arc::clone(&keyboard);
                        let control = g13_proto::describe_bit(bit);
                        let profile_now = profile;
                        let (sender, receiver) = std::sync::mpsc::channel();
                        macro_report = Some(receiver);
                        std::thread::spawn(move || match graph {
                            Some(graph) => {
                                // what the macro can ask about: the button that fired it, the profile, and
                                // the values the driver publishes - the same ones an applet sees
                                // the world a macro's own sources are read from: the endpoints it may call, and
                                // the published values, which is exactly what an applet is given
                                let playback = Playback {
                                    control: Some(control),
                                    profile: profile_now,
                                    values: visuals::read_published_values(),
                                    world: applet_world(&g13_config::config_dir()).0,
                                };
                                let load = |id: u32| g13_config::read_playable(id).ok().flatten();
                                let mut quiet = |_node: &str| {};
                                for _ in 0..repeats.max(1) {
                                    let mut problems: Vec<String> = Vec::new();
                                    let played = play_graph_reporting(
                                        &keyboard,
                                        &graph,
                                        &playback,
                                        &load,
                                        &mut quiet,
                                        &mut problems,
                                    );
                                    // a source that would not read is said out loud: the alternative is a
                                    // macro that took the wrong branch and nobody knowing why
                                    // a source that would not read is said out loud: the alternative is a
                                    // macro that took the wrong branch and nobody knowing why. Sent rather
                                    // than printed, because this is a library
                                    for problem in &problems {
                                        let _ = sender.send(format!(
                                            "the macro's sources did not all read: {problem}"
                                        ));
                                    }
                                    if let Err(problem) = played {
                                        // a macro that stops itself must not do so quietly
                                        let _ = sender.send(format!("macro {id}: {problem}"));
                                        break;
                                    }
                                }
                            }
                            None => {
                                let _ = play_macro(&keyboard, &macro_file, repeats);
                            }
                        });
                        continue;
                    }
                    if let Some(key) = bindings.key_for(bit) {
                        if let Ok(mut keyboard) = keyboard.lock() {
                            let _ = keyboard.press(key);
                        }
                    }
                    // A button on the pointer or the gamepad, handed to the worker that owns that device.
                    if let Some(button) = bindings.mouse_for(bit) {
                        workers.stick.act(DeviceButton::Mouse(button), true);
                    }
                    if let Some(button) = bindings.gamepad_for(bit) {
                        workers.stick.act(DeviceButton::Gamepad(button), true);
                    }
                    state.note(&g13_proto::describe_bit(bit), true);
                    if !report(Event::Key {
                        bit,
                        description: g13_proto::describe_bit(bit),
                        pressed: true,
                    }) {
                        return Ok(());
                    }
                }
                for bit in released {
                    // a control that switches profile or plays a macro sent no key when it went down, so
                    // its release is not one either and should not be reported as though it were
                    if bindings.switch_for(bit).is_some() || bindings.macro_for(bit).is_some() {
                        continue;
                    }
                    if let Some(key) = bindings.key_for(bit) {
                        if let Ok(mut keyboard) = keyboard.lock() {
                            let _ = keyboard.release(key);
                        }
                    }
                    if let Some(button) = bindings.mouse_for(bit) {
                        workers.stick.act(DeviceButton::Mouse(button), false);
                    }
                    if let Some(button) = bindings.gamepad_for(bit) {
                        workers.stick.act(DeviceButton::Gamepad(button), false);
                    }
                    if !report(Event::Key {
                        bit,
                        description: g13_proto::describe_bit(bit),
                        pressed: false,
                    }) {
                        return Ok(());
                    }
                }
            }
            Ok(None) => {}
            Err(error) => return Err(error),
        }

        // keep the published state fresh, but not on every packet
        if published.elapsed() > Duration::from_millis(250) {
            published = Instant::now();
            if let Some(problem) = workers.stick.problem() {
                told.push(problem);
            }
            what_is_in_force(&mut state, profile, &bindings);
            let _ = state.publish();
        }

        // the values the window reads, published when the pad changes rather than on the screen clock. The
        // screen changes once a second at most; the stick changes hundreds of times a second, and publishing
        // only with the screen is why the window looked slow while the driver was not.
        // The values the window reads. There is exactly one place that publishes them: two copies of this
        // block once existed, and whichever ran first consumed the ten-millisecond window, so the other one
        // published a stale copy of the stick - which is what an intermittent one-second update looks like.
        if changed || values_due.elapsed() > Duration::from_millis(10) {
            if changed {
                changed = false;
            }
            values_due = Instant::now();
            // The machine's own numbers and what is playing are gathered by the worker, which does it on
            // its own thread once a second. This loop used to gather them too, and that was the last thing
            // still able to stop it: seven playerctl processes and several /proc reads, about 200 ms, once
            // a second - which is a driver that starts fine and then hitches every second after that.
            let mut values = workers.screen.values();

            // the menu, while one is open: what is highlighted, and whether one is open at all
            match state.menu.as_ref() {
                Some(menu) => {
                    values.insert("menu_open".into(), g13_values::Value::Number(1.0));
                    values.insert(
                        "menu_item".into(),
                        g13_values::Value::Text(
                            menu.items.get(menu.index).cloned().unwrap_or_default(),
                        ),
                    );
                    values.insert(
                        "menu_level".into(),
                        g13_values::Value::Number(menu.level as f64),
                    );
                }
                None => {
                    values.insert("menu_open".into(), g13_values::Value::Number(0.0));
                    values.insert("menu_item".into(), g13_values::Value::Text(String::new()));
                    values.insert("menu_level".into(), g13_values::Value::Number(0.0));
                }
            }
            values.insert(
                "screen_visual".into(),
                g13_values::Value::Text(visual.clone()),
            );
            values.insert(
                "pad_down".into(),
                g13_values::Value::Text(pressed_controls(&previous)),
            );
            values.insert(
                "stick_raw".into(),
                g13_values::Value::Text(match last_stick {
                    Some([x, y]) => format!("0x{x:02x} 0x{y:02x}"),
                    None => String::new(),
                }),
            );
            // published every time, so a reader finds the key whether or not the stick has been touched: a
            // key that comes and goes is one a reader has to guess about
            match last_stick {
                Some([x, y]) => {
                    values.insert("stick_x".into(), g13_values::Value::Number(x as f64));
                    values.insert("stick_y".into(), g13_values::Value::Number(y as f64));
                }
                None => {
                    values.insert("stick_x".into(), g13_values::Value::Missing);
                    values.insert("stick_y".into(), g13_values::Value::Missing);
                }
            }
            let _ = visuals::publish_values(&values, &visuals::values_path());
        }

        // The screen is drawn *and written* on another thread, because drawing means running an applet's sources
        // and those can take most of a second, and because one frame costs about 26ms to put on the panel. This
        // thread does neither: it reads the pad and publishes, which is the pad's responsiveness. Nothing here
        // waits for a frame, and nothing here writes one.
        {
            let mut fresh: Vec<String> = Vec::new();
            for problem in workers.screen.problems() {
                if !told.contains(&problem) {
                    told.push(problem.clone());
                    fresh.push(problem);
                }
            }
            if !fresh.is_empty()
                && !report(Event::Drew {
                    visual: visual.clone(),
                    problems: fresh,
                })
            {
                return Ok(());
            }
        }

        // A control held past its threshold does what it does when held. Checked every pass, which is at worst
        // the pad's read timeout apart - a fifth of a second, on a threshold measured in halves of a second.
        for bit in holds.due(Instant::now()) {
            let Some(action) = bindings.hold_for(bit).map(str::to_string) else {
                continue;
            };
            match parse_action(&action) {
                Action::Menu => match open_menu(&visuals, &applet_dir, &visual) {
                    Err(problem) => told.push(problem),
                    Ok((menu, complaint)) => {
                        if let Some(complaint) = complaint {
                            told.push(complaint);
                        }
                        let said = menu.items.len();
                        state.menu = Some(menu);
                        tell_screen(&state, &screen_wake);
                        if !report(Event::Trouble {
                            problems: vec![format!(
                                "the menu is open over {said} screen(s): L2 and L3 move, L4 chooses, L1 goes back"
                            )],
                        }) {
                            return Ok(());
                        }
                    }
                },
                Action::Screen(wanted) => {
                    // a hold that changes the screen: the same decision a press makes, from the same place
                    match visuals::screen_target(
                        &wanted,
                        &visuals.enabled,
                        &visuals.active,
                        &applet_dir,
                    ) {
                        Err(problem) => told.push(problem),
                        Ok(None) => {}
                        Ok(Some(target)) => {
                            match show_target(
                                &target,
                                &mut visuals,
                                &mut visual,
                                &visuals_path,
                                &mut visuals_stamp,
                                &workers.screen,
                            ) {
                                Err(problem) => told.push(problem),
                                Ok(()) => {
                                    since_cycle = Instant::now();
                                    if !report(Event::Drew {
                                        visual: target.clone(),
                                        problems: vec![format!(
                                            "the screen now shows {target} (from a held control)"
                                        )],
                                    }) {
                                        return Ok(());
                                    }
                                }
                            }
                        }
                    }
                }
                // Everything else does what a press of it would do. The calls are the press path's own, so a hold
                // and a tap cannot drift apart: a held key reaches the keyboard, a held macro reaches the player.
                Action::Key(key) => {
                    if let Ok(mut keyboard) = keyboard.lock() {
                        let _ = keyboard.press(key);
                        let _ = keyboard.release(key);
                    }
                }
                Action::Macro(id, repeats) => match g13_config::read_playable(id).ok().flatten() {
                    // the flat form of the macro, which is what a `p,m,<id>` press plays; a graph is played by
                    // `play_graph_reporting` on the press path and reaching it from here is a separate piece
                    Some((macro_file, _)) => {
                        if let Err(problem) = play_macro(&keyboard, &macro_file, repeats) {
                            told.push(format!(
                                "holding {}: macro {id}: {problem}",
                                g13_proto::describe_bit(bit)
                            ));
                        }
                    }
                    None => told.push(format!(
                        "holding {}: there is no macro {id}",
                        g13_proto::describe_bit(bit)
                    )),
                },
                Action::MouseButton(button) => {
                    workers.stick.act(DeviceButton::Mouse(button), true);
                }
                Action::GamepadButton(button) => {
                    workers.stick.act(DeviceButton::Gamepad(button), true);
                }
                Action::SwitchProfile(profile) => {
                    let _ = g13_config::write_active_profile(profile);
                }
                Action::Record(id) => told.push(format!(
                    "holding {} would start recording {id:?}, which a hold does not do yet",
                    g13_proto::describe_bit(bit)
                )),
                Action::Unsupported(kind) => told.push(format!(
                    "holding {} asks for {kind}, which this build cannot do",
                    g13_proto::describe_bit(bit)
                )),
                Action::Nothing => {}
            }
        }

        // A program that feeds an applet takes the screen while it is running, and hands it back when it stops.
        // The write is the whole of it - the same two files a click in the window writes, `visuals.json` and
        // `active-profile` - so the pad, the window and the file cannot come to disagree.
        match a_follow.wanted(
            &visuals.enabled,
            &visual,
            &applet_dir,
            profile,
            std::time::SystemTime::now(),
        ) {
            visuals::Wanted::Nothing => {}
            visuals::Wanted::Take {
                visual: fed,
                profile: set,
            } => match visuals::screen_target(&fed, &visuals.enabled, &visual, &applet_dir) {
                Err(problem) => told.push(problem),
                Ok(None) => {}
                Ok(Some(target)) => {
                    match show_target(
                        &target,
                        &mut visuals,
                        &mut visual,
                        &visuals_path,
                        &mut visuals_stamp,
                        &workers.screen,
                    ) {
                        Err(problem) => told.push(problem),
                        Ok(()) => {
                            if let Some(name) = set {
                                match g13_config::Profiles::read_in(&g13_config::config_dir())
                                    .profile_named(&name)
                                {
                                    Some(chosen) if chosen != profile => {
                                        let _ = g13_config::write_active_profile(chosen);
                                        a_follow.took_profile(Some(chosen));
                                        told.push(format!(
                                            "{target} took the screen, and the binding set {name} with it"
                                        ));
                                    }
                                    Some(_) => {}
                                    None => told.push(format!(
                                        "{target} asks for the binding set {name:?}, which \\
                                         no set is called, so the profile is unchanged"
                                    )),
                                }
                            } else {
                                told.push(format!("{target} took the screen: it is being fed"));
                            }
                        }
                    }
                }
            },
            visuals::Wanted::Give {
                visual: back,
                profile: back_profile,
            } => {
                if let Some(back) = back {
                    match show_target(
                        &back,
                        &mut visuals,
                        &mut visual,
                        &visuals_path,
                        &mut visuals_stamp,
                        &workers.screen,
                    ) {
                        Err(problem) => told.push(problem),
                        Ok(()) => {
                            told.push(format!("{back} is showing again: the program stopped"))
                        }
                    }
                }
                if let Some(back_profile) = back_profile {
                    let _ = g13_config::write_active_profile(back_profile);
                    told.push(format!("and the profile before it: {back_profile}"));
                }
            }
        }

        // cycling: move to the next enabled visual after the configured wait
        if visuals.cycle && visuals.enabled.len() > 1 && since_cycle.elapsed() >= cycle_every {
            since_cycle = Instant::now();
            let index = visuals
                .enabled
                .iter()
                .position(|item| item == &visual)
                .unwrap_or(0);
            visual = visuals.enabled[(index + 1) % visuals.enabled.len()].clone();
            a_follow.user_chose(&visual);
            workers.screen.show(&visual);
        }

        // The stick's settings are the window's to write, so they are watched rather than read once. Turning
        // the stick off while it is holding a direction must let go of it, or the key stays down with no way
        // to release it - and the same is true when a profile change moves what a direction is bound to.
        {
            let fresh = std::fs::metadata(&stick_path)
                .and_then(|details| details.modified())
                .ok();
            if fresh != stick_stamp {
                stick_stamp = fresh;
                let was = stick_settings.mode;
                stick_settings = g13_config::stick::Settings::load(&g13_config::config_dir());
                stick_state = stick_settings.stick.clone();
                // Everything a sector could be holding, let go of: the map may have changed under a stick
                // that is being held, and a key left down with nothing holding it has no way to be released.
                for bound in bindings.all_sector_bounds() {
                    set_bound(&bound, false, &keyboard, &workers.stick);
                }
                // and the pointer is told, so a speed change takes effect and leaving mouse mode stops it
                workers.stick.adjust(stick_settings.clone());
                if was != stick_settings.mode {
                    told.push(format!("the stick is now {}", stick_settings.mode.name()));
                }
            }
        }

        // a config edit should take effect without a restart
        if last_check.elapsed() > Duration::from_secs(1) {
            last_check = Instant::now();
            let fresh = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            let fresh_profile = std::fs::metadata(&profile_path)
                .and_then(|m| m.modified())
                .ok();
            if fresh_profile != profile_stamp {
                // the profile changed underneath us: follow it, wherever it changed from
                profile_stamp = fresh_profile;
                profile = g13_config::read_active_profile();
                let switched = g13_config::bindings_path(profile);
                bindings = load_bindings(&switched);
                holds = holds_for(&switched);
                if let Some(complaint) = show_profile_colour(&device, &switched) {
                    told.push(complaint);
                }
                if let Some(complaint) = show_profile_light(&device, profile, false) {
                    told.push(complaint);
                }
                // a new map may want a device that was not needed before, so the worker is told
                workers.set_needs(&bindings);
                if !report(Event::ProfileSwitched {
                    profile,
                    controls: bindings.len(),
                }) {
                    return Ok(());
                }
            }
            if fresh != stamp {
                stamp = fresh;
                bindings = load_bindings(&path);
                // the file was edited under us, and a colour line may be one of the edits - which is how
                // `g13 colour` and the window take effect: write the file, and the driver picks it up
                if let Some(complaint) = show_profile_colour(&device, &path) {
                    told.push(complaint);
                }
                // a new map may want a device that was not needed before, so the worker is told
                workers.set_needs(&bindings);
                if !report(Event::Reloaded {
                    controls: bindings.len(),
                }) {
                    return Ok(());
                }
            }
            let fresh_visuals = std::fs::metadata(&visuals_path)
                .and_then(|m| m.modified())
                .ok();
            if fresh_visuals != visuals_stamp {
                visuals_stamp = fresh_visuals;
                visuals = visuals::load_visuals(&visuals_path);
                a_follow.user_chose(&visuals.active);
                visual = visuals.active.clone();
                workers.screen.show(&visual);
                since_cycle = Instant::now();
                if !report(Event::Drew {
                    visual: visual.clone(),
                    problems: vec![format!("the screen now shows {visual} (from visuals.json)")],
                }) {
                    return Ok(());
                }
            }
        }
    }
}

#[test]
fn the_stick_is_read_as_sectors_rather_than_as_controls() {
    // The check that keeps the two halves from drifting back into each other: the stick's binding names are
    // the stick's own, and none of them is a control on the pad. They were in both places at once, which is
    // what put them in the Bindings tab beside the buttons while the stick counted them as directions.
    for name in ["JUP", "JDOWN", "JLEFT", "JRIGHT", "J0", "J7", "J15"] {
        assert!(
            g13_config::stick::Sectors::is_sector_name(name),
            "{name} should be a sector of the stick"
        );
        assert!(
            !CONTROLS.contains(&name),
            "{name} is a sector, not a control"
        );
    }
    assert!(
        !g13_config::stick::Sectors::is_sector_name("JCLICK"),
        "the stick's click is a control, not a sector"
    );
    assert!(CONTROLS.contains(&"JCLICK"), "and it is a control");
}

#[test]
fn the_sticks_sector_names_are_read_as_sectors_not_rejected_as_controls() {
    // The sector names are read from the bindings file's own lines. Before this they arrived as "not a control
    // this project knows" and the stick did nothing at all.
    let bindings = Bindings::from_text("JUP=p,k.17\nJDOWN=p,k.31\nJLEFT=p,k.30\nJRIGHT=p,k.32\n");
    let sectors = g13_config::stick::Sectors::default();
    let bound = |index| bindings.bounds_for(&sectors, index);
    // sector 0 is up and they count clockwise: up, right, down, left are 0, 2, 4 and 6
    assert_eq!(bound(0), vec![Bound::Key(KeyCode::new(17))]);
    assert_eq!(bound(2), vec![Bound::Key(KeyCode::new(32))]);
    assert_eq!(bound(4), vec![Bound::Key(KeyCode::new(31))]);
    assert_eq!(bound(6), vec![Bound::Key(KeyCode::new(30))]);
    // and they are no longer reported as problems, which is what the doctor was printing
    assert!(
        bindings.problems().is_empty(),
        "still rejected: {:?}",
        bindings.problems()
    );
    // the four diagonals are bindable too, which is what an eight-way radial needs
    let radial = Bindings::from_text("JUPLEFT=p,k.30\nJDOWNRIGHT=p,k.32\n");
    let diagonal = |index| radial.bounds_for(&sectors, index);
    assert_eq!(
        diagonal(7),
        vec![Bound::Key(KeyCode::new(30))],
        "up and left"
    );
    assert_eq!(
        diagonal(3),
        vec![Bound::Key(KeyCode::new(32))],
        "down and right"
    );
}

#[test]
fn the_record_action_is_written_two_ways_and_refused_a_third() {
    assert_eq!(parse_action("rec"), Action::Record(None));
    assert_eq!(parse_action("rec,3"), Action::Record(Some(3)));
    assert_eq!(parse_action("rec, 7"), Action::Record(Some(7)));
    match parse_action("rec,three") {
        Action::Unsupported(reason) => assert!(reason.contains("rec,<id>"), "{reason}"),
        other => panic!("a recording that names a word should be refused: {other:?}"),
    }
}

#[test]
fn the_published_stage_follows_the_wizard_and_is_never_left_behind() {
    // The regression this exists for: a recording finished and the pad went on saying RECORDING, because the
    // path that finished it did not clear what was published. Deriving it from what the loop is doing is what
    // makes that impossible, so this asserts the whole mapping.
    assert_eq!(
        published_stage(&None, 3),
        None,
        "a wizard that is over is not published"
    );

    let choosing = Some(Recording::ChoosingControl {
        started: std::time::Instant::now(),
        id: 7,
        name: String::new(),
        picked: Some("M2".into()),
    });
    assert_eq!(
        published_stage(&choosing, 2),
        Some(Wizard {
            stage: Stage::Choosing,
            id: 7,
            profile: 2,
            control: None,
            picked: Some("M2".into()),
        }),
        "the profile key that answered is not published with the profile it chose"
    );
    // the profile is the loop's, not one remembered from when the wizard started
    assert_eq!(published_stage(&choosing, 3).unwrap().profile, 3);

    let (_, keys) = std::sync::mpsc::channel();
    let capturing = Some(Recording::Capturing {
        id: 7,
        name: String::new(),
        profile: 2,
        control: "G7".into(),
        keys,
        events: Vec::new(),
        started: std::time::Instant::now(),
        last: std::time::Instant::now(),
    });
    assert_eq!(
        published_stage(&capturing, 2),
        Some(Wizard {
            stage: Stage::Capturing,
            id: 7,
            profile: 2,
            control: Some("G7".into()),
            picked: None,
        }),
        "the control being recorded is not published"
    );
}

#[test]
fn the_screen_is_woken_the_moment_its_state_changes() {
    // On a timer alone the pad went on showing the stage that had just ended - up to a second of a stale
    // `RECORDING`, which is how finishing a recording came to look like it had not happened.
    let wake = ScreenWake::default();
    let writer = wake.clone();
    let started = std::time::Instant::now();
    let waiter = std::thread::spawn(move || {
        writer.wait(Duration::from_secs(10));
        started.elapsed()
    });
    std::thread::sleep(Duration::from_millis(50));
    wake.set(ScreenStatus {
        recording: None,
        last_keys: Vec::new(),
        menu: None,
        screen: 0,
        screen_selected: 0,
        screen_item: String::new(),
    });
    let waited = waiter.join().expect("the waiter panicked");
    assert!(
        waited < Duration::from_secs(2),
        "the screen waited {waited:?} after its state changed"
    );

    // and with nothing changing it waits its own timeout rather than hanging or spinning
    let started = std::time::Instant::now();
    wake.wait(Duration::from_millis(50));
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(45) && waited < Duration::from_secs(2),
        "an unchanged screen waited {waited:?}"
    );
    assert!(wake.get().recording.is_none());
}

#[test]
fn finishing_a_recording_writes_the_macro_and_puts_it_on_the_control() {
    // a directory of its own: two tests sharing one path delete each other's files
    let dir = std::env::temp_dir().join(format!("g13-record-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_the_once_built_in_screens(&dir);
    // a profile with other things in it, all of which must survive - the colour especially, because it is the
    // only trace of a setting that is not a binding
    std::fs::write(
        dir.join("bindings-2.properties"),
        "color=255,0,0\nG5=x\nG6=p,k.30\n",
    )
    .unwrap();
    let events = vec![
        g13_device::capture::KeyEvent {
            code: 29,
            down: true,
            after_ms: 0,
        },
        g13_device::capture::KeyEvent {
            code: 29,
            down: false,
            after_ms: 40,
        },
    ];
    let (event, complaints) = finish_recording(&dir, 2, "G7", 4, "", &events);
    assert!(complaints.is_empty(), "{complaints:?}");
    // the macro is exactly what the player reads
    let text = std::fs::read_to_string(dir.join("macro-4.properties")).unwrap();
    assert!(text.contains("sequence=kd.29,d.40,ku.29,d.100"), "{text}");
    assert!(text.contains("name=G7 in profile 2"), "{text}");
    assert!(text.contains("id=4"), "{text}");
    // the control plays it, and nothing else in that profile moved
    let bindings = std::fs::read_to_string(dir.join("bindings-2.properties")).unwrap();
    assert!(bindings.contains("G7=m,4,1"), "{bindings}");
    assert!(
        bindings.contains("color=255,0,0") && bindings.contains("G6=p,k.30"),
        "{bindings}"
    );
    match event {
        Event::MacroRecorded {
            id,
            steps,
            profile,
            control,
            ..
        } => assert_eq!((id, steps, profile, control.as_str()), (4, 4, 2, "G7")),
        other => panic!("expected a MacroRecorded, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_time_before_the_first_key_is_not_part_of_the_pattern() {
    // A recording starts when the button is pressed, and the hand then moves to the first key. That gap was
    // written as a leading `d.`, so every macro began with a pause: on hardware it read as half a second (or
    // two) between pressing the button and anything happening. The previous stack's own macros start straight
    // at a key - `sequence=kd.29,kd.56,kd.111,...` - which is the convention kept here.
    let events = vec![
        g13_device::capture::KeyEvent {
            code: 20,
            down: true,
            after_ms: 2_000,
        },
        g13_device::capture::KeyEvent {
            code: 20,
            down: false,
            after_ms: 40,
        },
    ];
    let steps = steps_from_events(&events);
    assert!(
        !matches!(steps.first(), Some(g13_config::MacroStep::Delay(_))),
        "the recording begins with a pause instead of the first key: {steps:?}"
    );
    // and a pause *between* steps is the pattern, so it is kept
    assert_eq!(
        steps,
        vec![
            g13_config::MacroStep::KeyDown(20),
            g13_config::MacroStep::Delay(40),
            g13_config::MacroStep::KeyUp(20),
            g13_config::MacroStep::Delay(100),
        ]
    );
}

#[test]
fn a_recording_with_nothing_in_it_changes_nothing() {
    // what the wizard must never do is take a working binding away because a hand slipped
    let dir = std::env::temp_dir().join(format!("g13-record-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_the_once_built_in_screens(&dir);
    std::fs::write(dir.join("bindings-1.properties"), "G7=m,2,1\n").unwrap();
    let (event, complaints) = finish_recording(&dir, 1, "G7", 3, "", &[]);
    assert!(complaints.is_empty(), "{complaints:?}");
    assert!(
        !dir.join("macro-3.properties").exists(),
        "an empty macro was written"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("bindings-1.properties")).unwrap(),
        "G7=m,2,1\n",
        "an empty recording changed a working binding"
    );
    match event {
        Event::MacroRecorded { steps, .. } => assert_eq!(steps, 0),
        other => panic!("expected a MacroRecorded, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn what_was_pressed_becomes_the_steps_a_macro_file_holds() {
    use g13_config::MacroStep;
    use g13_device::capture::KeyEvent;
    // CTRL-ALT-DEL as somebody actually types it: two modifiers down, the third key 20ms later, then the
    // releases in order - which is what the previous tool wrote, and what its player expects
    let events = [
        KeyEvent {
            code: 29,
            down: true,
            after_ms: 0,
        },
        KeyEvent {
            code: 56,
            down: true,
            after_ms: 2,
        },
        KeyEvent {
            code: 111,
            down: true,
            after_ms: 20,
        },
        KeyEvent {
            code: 111,
            down: false,
            after_ms: 20,
        },
        KeyEvent {
            code: 56,
            down: false,
            after_ms: 3,
        },
        KeyEvent {
            code: 29,
            down: false,
            after_ms: 3,
        },
    ];
    let steps = steps_from_events(&events);
    assert_eq!(
        steps,
        vec![
            MacroStep::KeyDown(29),
            MacroStep::KeyDown(56),
            MacroStep::Delay(20),
            MacroStep::KeyDown(111),
            MacroStep::Delay(20),
            MacroStep::KeyUp(111),
            MacroStep::KeyUp(56),
            MacroStep::KeyUp(29),
            // and the tail, so playing it twice does not run the two together
            MacroStep::Delay(100),
        ]
    );
    // a gap too small to have been meant is not a step of its own
    assert!(
        steps_from_events(&[KeyEvent {
            code: 30,
            down: true,
            after_ms: 2
        }])
        .len()
            == 2
    );
    // nothing pressed, nothing written
    assert!(steps_from_events(&[]).is_empty());
}

#[test]
fn the_record_button_records_unless_the_file_says_otherwise() {
    let dir = std::env::temp_dir().join("g13-agent-mr-default");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_the_once_built_in_screens(&dir);
    let path = dir.join("bindings-0.properties");
    std::fs::write(&path, "G1=p,k.30\n").unwrap();

    let bit = bit_for_control(Control::MacroRecord).unwrap();
    // nothing mentions MR, and its own label is Macro Record
    let bindings = effective_bindings("G1=p,k.30\n", &path);
    assert_eq!(bindings.record_for(bit), Some(None), "MR should record");
    // a line in the file wins
    let own = effective_bindings("MR=p,k.30\n", &path);
    assert_eq!(own.record_for(bit), None, "the file bound MR to a key");
    assert_eq!(own.key_for(bit).map(|key| key.code()), Some(30));
    // and it can be pointed at a macro of its own
    let into = effective_bindings("MR=rec,7\n", &path);
    assert_eq!(into.record_for(bit), Some(Some(7)));
}

#[test]
fn the_m_keys_select_profiles_unless_the_file_says_otherwise() {
    let dir = std::env::temp_dir().join("g13-agent-m-key-defaults");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_the_once_built_in_screens(&dir);
    for profile in 0..4 {
        std::fs::write(
            dir.join(format!("bindings-{profile}.properties")),
            "G1=p,k.30\n",
        )
        .unwrap();
    }
    let path = dir.join("bindings-2.properties");
    let text = std::fs::read_to_string(&path).unwrap();

    // nothing in the file mentions the M keys, so each one gets the profile it is printed beside
    let bindings = effective_bindings(&text, &path);
    for (control, profile) in [(Control::M1, 1u32), (Control::M2, 2), (Control::M3, 3)] {
        let bit = bit_for_control(control).unwrap();
        assert_eq!(
            bindings.switch_for(bit),
            Some(profile),
            "{control:?} should default to profile {profile}"
        );
    }
    // and nothing else is given anything
    assert!(
        !bindings.bound(bit_for_control(Control::L1).unwrap()),
        "L1 was not asked to default to anything"
    );

    // a line in the file wins over the default
    let own = effective_bindings("M1=p,k.30\n", &path);
    assert_eq!(
        own.key_for(bit_for_control(Control::M1).unwrap())
            .map(|key| key.code()),
        Some(30)
    );
    assert_eq!(own.switch_for(bit_for_control(Control::M1).unwrap()), None);
    // while the other two still take theirs
    assert_eq!(
        own.switch_for(bit_for_control(Control::M2).unwrap()),
        Some(2)
    );
}

#[test]
fn a_profile_key_is_not_defaulted_to_a_profile_that_does_not_exist() {
    // switching to a profile with no file would replace the whole map with nothing, which is worse than a key
    // that does nothing at all
    let dir = std::env::temp_dir().join("g13-agent-m-key-missing-profile");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write_the_once_built_in_screens(&dir);
    let path = dir.join("bindings-0.properties");
    std::fs::write(&path, "G1=p,k.30\n").unwrap();

    let bindings = effective_bindings("G1=p,k.30\n", &path);
    for control in [Control::M1, Control::M2, Control::M3] {
        assert_eq!(
            bindings.switch_for(bit_for_control(control).unwrap()),
            None,
            "{control:?} was defaulted with no profile file to switch to"
        );
    }
}

#[test]
fn a_diagonal_keeps_the_part_it_shares_with_the_sector_before_it() {
    // up, then up-and-right: up stays down and right joins it. Pressing up again would be a repeat, and
    // releasing it would break the chord - so the shared part is what this is about.
    let sectors = g13_config::stick::Sectors::default();
    let bindings = Bindings::from_text("JUP=p,k.17\nJRIGHT=p,k.32\n");
    let mut stick = g13_config::stick::Stick::default();
    // up, then up-and-right
    assert_eq!(stick.update((126, 11), &sectors), Some((None, Some(0))));
    let (released, pressed) = stick.update((236, 11), &sectors).unwrap();
    let gone = bindings.bounds_for(&sectors, released.unwrap());
    let here = bindings.bounds_for(&sectors, pressed.unwrap());
    assert_eq!(gone, vec![Bound::Key(KeyCode::new(17))]);
    assert_eq!(
        here,
        vec![Bound::Key(KeyCode::new(17)), Bound::Key(KeyCode::new(32))]
    );
    // the shared part is neither released nor pressed again: up was held throughout
    assert!(gone.iter().all(|bound| here.contains(bound)));
    assert_eq!(
        here.iter()
            .filter(|bound| !gone.contains(bound))
            .collect::<Vec<_>>(),
        vec![&Bound::Key(KeyCode::new(32))]
    );
}

#[test]
fn every_target_the_joystick_dropdowns_offer_drives_the_axis_it_names() {
    // The Controls tab offers one dropdown per side of the stick, each listing these. A target that drove the
    // wrong axis, or none at all, would be a dropdown entry that lies about what it does.
    //
    // The eight axes are the eight a real pad has: a pad on this machine advertises exactly ABS_X, ABS_Y,
    // ABS_Z, ABS_RX, ABS_RY, ABS_RZ, ABS_HAT0X and ABS_HAT0Y, so every entry here names an axis a game can
    // actually read.
    use g13_config::stick::Target;
    use g13_device::joystick::Axis;
    for (target, axis) in [
        (Target::LeftX, Axis::LeftX),
        (Target::LeftY, Axis::LeftY),
        (Target::RightX, Axis::RightX),
        (Target::RightY, Axis::RightY),
        (Target::LeftTrigger, Axis::LeftTrigger),
        (Target::RightTrigger, Axis::RightTrigger),
        (Target::DpadX, Axis::DpadX),
        (Target::DpadY, Axis::DpadY),
    ] {
        assert_eq!(
            axis_for_target(target),
            Some(axis),
            "{target:?} should drive the axis it is named after"
        );
        // and the two names agree, so what the dropdown says and what is driven cannot drift apart
        assert_eq!(
            target.name(),
            axis.name(),
            "the dropdown would say one thing and drive another"
        );
    }
    // nowhere is a real choice, and it drives nothing
    assert_eq!(axis_for_target(Target::None), None);
    // every one of them is offered, and nothing else is
    let offered: Vec<&str> = Target::ALL.iter().map(|t| t.name()).collect();
    assert_eq!(offered.len(), 9, "eight axes and nowhere");
    for (target, _) in [(Target::LeftX, ()), (Target::DpadY, ())] {
        assert!(offered.contains(&target.name()));
    }
    assert!(offered.contains(&"none"));
    // What each axis's range is - centred, one-way, or three positions - is asserted in `g13-device`, beside
    // the ranges themselves, rather than repeated here.
}

#[test]
fn the_ordinary_route_puts_the_stick_on_the_left_stick() {
    use g13_device::joystick::Axis;
    let route = g13_config::stick::Route::default();
    let at = |x: f64, y: f64| {
        gamepad_axes(route, x, y)
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    // pushed right: the left stick's x follows, and the two sides of one axis do not fight
    assert_eq!(at(1.0, 0.0)[&Axis::LeftX], 1.0);
    assert_eq!(at(-1.0, 0.0)[&Axis::LeftX], -1.0);
    assert_eq!(at(0.0, 1.0)[&Axis::LeftY], 1.0);
    assert_eq!(at(0.0, -1.0)[&Axis::LeftY], -1.0);
    // and at rest everything is at rest, which is what releases a stick that was being held
    assert!(at(0.0, 0.0).values().all(|value| *value == 0.0));
}

#[test]
fn a_racing_route_steers_with_the_stick_and_drives_the_triggers_with_the_other_axis() {
    // Left and right on the joystick, forwards and backwards on the controller's triggers.
    use g13_config::stick::{Route, Target};
    use g13_device::joystick::Axis;
    let route = Route {
        right: Target::LeftX,
        left: Target::LeftX,
        // on this stick a low y is up, so up is the throttle and down is the brake
        up: Target::RightTrigger,
        down: Target::LeftTrigger,
    };
    let at = |x: f64, y: f64| {
        gamepad_axes(route, x, y)
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
    };

    // steered hard right, nothing on the triggers
    let right = at(1.0, 0.0);
    assert_eq!(right[&Axis::LeftX], 1.0);
    assert_eq!(right[&Axis::RightTrigger], 0.0);
    assert_eq!(right[&Axis::LeftTrigger], 0.0);

    // stick forward: the throttle opens and the steering is untouched
    let forward = at(0.0, -1.0);
    assert_eq!(forward[&Axis::LeftX], 0.0);
    assert_eq!(forward[&Axis::RightTrigger], 1.0);
    assert_eq!(forward[&Axis::LeftTrigger], 0.0);

    // stick back: the brake, and only the brake
    let back = at(0.0, 1.0);
    assert_eq!(back[&Axis::LeftTrigger], 1.0);
    assert_eq!(back[&Axis::RightTrigger], 0.0);

    // and a trigger only ever opens: a negative reading must not send it backwards
    let left_and_forward = at(-1.0, -1.0);
    assert_eq!(left_and_forward[&Axis::LeftX], -1.0);
    assert_eq!(left_and_forward[&Axis::RightTrigger], 1.0);
    for (axis, value) in gamepad_axes(route, 0.5, 0.5) {
        if matches!(axis, Axis::LeftTrigger | Axis::RightTrigger) {
            assert!((0.0..=1.0).contains(&value), "{axis:?} was {value}");
        }
    }
}

#[test]
fn a_side_routed_nowhere_leaves_its_axis_alone() {
    use g13_config::stick::{Route, Target};
    use g13_device::joystick::Axis;
    // steering only: pushing left must do nothing at all
    let route = Route {
        right: Target::LeftX,
        left: Target::None,
        down: Target::None,
        up: Target::None,
    };
    let at = |x: f64, y: f64| {
        gamepad_axes(route, x, y)
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    assert_eq!(at(1.0, 0.0)[&Axis::LeftX], 1.0);
    assert_eq!(at(-1.0, 0.0)[&Axis::LeftX], 0.0);
    assert_eq!(at(0.0, -1.0)[&Axis::LeftY], 0.0);
}

#[test]
fn the_dpad_is_a_direction_rather_than_an_amount() {
    use g13_config::stick::{Route, Target};
    use g13_device::joystick::Axis;
    let route = Route {
        right: Target::DpadX,
        left: Target::DpadX,
        down: Target::DpadY,
        up: Target::DpadY,
    };
    let at = |x: f64, y: f64| {
        gamepad_axes(route, x, y)
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    // the value handed over is the position; the hat snaps it, which the device does
    assert_eq!(at(0.3, 0.0)[&Axis::DpadX], 0.3);
    assert_eq!(at(-0.3, 0.0)[&Axis::DpadX], -0.3);
    assert_eq!(at(0.0, 0.0)[&Axis::DpadX], 0.0);
}

#[test]
fn a_control_bound_to_a_mouse_button_becomes_that_button() {
    // G1 and G2 are wired to the first two bits the pad reports for them, so this is the whole path: a line in
    // the file, the bit the pad sets, and the button the driver would press.
    let has = |name: &str, action: &str, wanted: Option<g13_device::mouse::Button>| {
        let bindings = Bindings::from_text(&format!("{name}={action}\n"));
        let bit = g13_proto::control_from_name(name)
            .and_then(g13_proto::bit_for_control)
            .expect("a control with a bit");
        assert_eq!(
            bindings.mouse_for(bit),
            wanted,
            "{name}={action} should give {wanted:?}"
        );
        bindings
    };
    use g13_device::mouse::Button;
    let map = has("G1", "mb,left", Some(Button::Left));
    has("G2", "mb,extra", Some(Button::Extra));
    // the words people use for the third button all mean the middle one
    has("G3", "mb,wheel", Some(Button::Middle));
    has("G4", "mb,right", Some(Button::Right));
    assert!(
        map.needs_mouse(),
        "a mouse button must make a pointer be made"
    );
    assert!(!map.needs_gamepad());
}

#[test]
fn a_control_bound_to_a_gamepad_button_becomes_that_button() {
    use g13_device::joystick::Button;
    let bindings = Bindings::from_text("G1=gb,south\nG2=gb,dpad-up\nG3=gb,rt\n");
    let bit = |name: &str| {
        g13_proto::control_from_name(name)
            .and_then(g13_proto::bit_for_control)
            .expect("a control with a bit")
    };
    assert_eq!(bindings.gamepad_for(bit("G1")), Some(Button::South));
    assert_eq!(bindings.gamepad_for(bit("G2")), Some(Button::DpadUp));
    assert_eq!(bindings.gamepad_for(bit("G3")), Some(Button::Rt));
    assert!(bindings.needs_gamepad());
    assert!(!bindings.needs_mouse());
}

#[test]
fn the_letters_printed_on_a_pad_all_point_at_the_same_buttons() {
    // A game asks for BTN_SOUTH whoever made the pad, so these are spellings of one button, not four.
    use g13_device::joystick::Button;
    for (spelling, wanted) in [
        ("a", Button::South),
        ("cross", Button::South),
        ("b", Button::East),
        ("circle", Button::East),
        ("x", Button::West),
        ("square", Button::West),
        ("y", Button::North),
        ("triangle", Button::North),
        ("l1", Button::Lb),
        ("r2", Button::Rt),
        ("l3", Button::ThumbL),
    ] {
        assert_eq!(
            Button::from_name(spelling),
            Some(wanted),
            "{spelling} should be {wanted:?}"
        );
    }
}

#[test]
fn a_misspelled_button_says_what_the_names_are() {
    // A wrong name that only says "not a binding action" sends the user looking in the wrong place.
    match parse_action("gb,triggers") {
        Action::Unsupported(reason) => {
            assert!(
                reason.contains("gb,triggers") || reason.contains("triggers"),
                "{reason}"
            );
            assert!(
                reason.contains("south"),
                "the names should be listed: {reason}"
            );
        }
        other => panic!("expected a problem, got {other:?}"),
    }
    match parse_action("mb,middleish") {
        Action::Unsupported(reason) => {
            assert!(
                reason.contains("left"),
                "the names should be listed: {reason}"
            );
        }
        other => panic!("expected a problem, got {other:?}"),
    }
    // and the good ones parse
    assert_eq!(
        parse_action("mb,left"),
        Action::MouseButton(g13_device::mouse::Button::Left)
    );
    assert_eq!(
        parse_action("gb,south"),
        Action::GamepadButton(g13_device::joystick::Button::South)
    );
}

#[test]
fn binding_only_the_four_cardinals_still_gives_the_diagonals() {
    // Four directions bound: a diagonal has to be both of the keys it is made of,
    // because that is what "up and left" means to anything reading a keyboard - and without this the
    // diagonal half of an eight-way stick would press nothing at all.
    let sectors = g13_config::stick::Sectors::default();
    let bindings = Bindings::from_text("JUP=p,k.17\nJDOWN=p,k.31\nJLEFT=p,k.30\nJRIGHT=p,k.32\n");
    let bound = |index| bindings.bounds_for(&sectors, index);
    assert_eq!(
        bound(7),
        vec![Bound::Key(KeyCode::new(17)), Bound::Key(KeyCode::new(30))],
        "up and left"
    );
    // the order of a fallback pair is fixed - up, right, down, left - so the same press is always the same
    // press, and a game that watches for the first of the two sees the same one every time
    assert_eq!(
        bound(3),
        vec![Bound::Key(KeyCode::new(32)), Bound::Key(KeyCode::new(31))],
        "down and right"
    );
    // a cardinal is itself
    assert_eq!(bound(0), vec![Bound::Key(KeyCode::new(17))]);
}

#[test]
fn a_diagonal_bound_in_its_own_right_sends_only_that_key() {
    // The eight-way radial: when a direction has a binding of its own, that is what it sends rather than the
    // two cardinals. Otherwise binding a diagonal would do nothing at all - a capability the file accepts
    // and the driver never uses, which is a shape of bug this project has met before.
    let sectors = g13_config::stick::Sectors::default();
    let bindings = Bindings::from_text("JUP=p,k.17\nJLEFT=p,k.30\nJUPLEFT=p,k.11\n");
    assert_eq!(
        bindings.bounds_for(&sectors, 7),
        vec![Bound::Key(KeyCode::new(11))],
        "up and left"
    );
    // and a diagonal with no binding of its own still falls back to the pair
    let plain = Bindings::from_text("JUP=p,k.17\nJLEFT=p,k.30\n");
    assert_eq!(
        plain.bounds_for(&sectors, 7),
        vec![Bound::Key(KeyCode::new(17)), Bound::Key(KeyCode::new(30))]
    );
}

#[test]
fn a_sector_with_nothing_bound_to_it_sends_nothing() {
    let sectors = g13_config::stick::Sectors::default();
    let bindings = Bindings::from_text("JUP=p,k.17\n");
    assert!(bindings.bounds_for(&sectors, 4).is_empty(), "down");
    assert!(
        bindings.bounds_for(&sectors, 3).is_empty(),
        "down and right"
    );
    // and a bound sector beside them is unaffected
    assert_eq!(
        bindings.bounds_for(&sectors, 0),
        vec![Bound::Key(KeyCode::new(17))]
    );
}

#[test]
fn the_sector_count_is_set_by_the_user_and_the_names_follow_it() {
    // Four sectors is a d-pad, and the names a file already uses still find the directions they name.
    let four = g13_config::stick::Sectors::new(4);
    let bindings = Bindings::from_text("JUP=p,k.17\nJRIGHT=p,k.32\nJDOWN=p,k.31\nJLEFT=p,k.30\n");
    assert_eq!(
        bindings.bounds_for(&four, 0),
        vec![Bound::Key(KeyCode::new(17))]
    );
    assert_eq!(
        bindings.bounds_for(&four, 1),
        vec![Bound::Key(KeyCode::new(32))]
    );
    assert_eq!(
        bindings.bounds_for(&four, 2),
        vec![Bound::Key(KeyCode::new(31))]
    );
    assert_eq!(
        bindings.bounds_for(&four, 3),
        vec![Bound::Key(KeyCode::new(30))]
    );

    // Sixteen gives sixteen bindable sectors, numbered clockwise from up, and a sector that has nothing of
    // its own sends nothing rather than something wrong.
    let sixteen = g13_config::stick::Sectors::new(16);
    let many = Bindings::from_text("J0=p,k.17\nJ5=p,k.18\nJ15=p,k.19\n");
    assert_eq!(
        many.bounds_for(&sixteen, 0),
        vec![Bound::Key(KeyCode::new(17))]
    );
    assert_eq!(
        many.bounds_for(&sixteen, 5),
        vec![Bound::Key(KeyCode::new(18))]
    );
    assert_eq!(
        many.bounds_for(&sixteen, 15),
        vec![Bound::Key(KeyCode::new(19))]
    );
    assert!(many.bounds_for(&sixteen, 9).is_empty());
}

#[test]
fn a_sector_can_be_bound_to_a_controller_button_as_well_as_a_key() {
    // "also with joystick": the same radial, wired to a gamepad's d-pad rather than a keyboard's arrows - and
    // a mouse button for good measure, because a sector is just somewhere to put a binding.
    let sectors = g13_config::stick::Sectors::default();
    let bindings = Bindings::from_text("JUP=gb,dpad-up\nJRIGHT=gb:dpad-right\nJLEFT=mb,side\n");
    assert!(
        bindings.problems().is_empty(),
        "rejected: {:?}",
        bindings.problems()
    );
    assert_eq!(
        bindings.bounds_for(&sectors, 0),
        vec![Bound::Gamepad(g13_device::joystick::Button::DpadUp)]
    );
    assert_eq!(
        bindings.bounds_for(&sectors, 2),
        vec![Bound::Gamepad(g13_device::joystick::Button::DpadRight)]
    );
    assert_eq!(
        bindings.bounds_for(&sectors, 6),
        vec![Bound::Mouse(g13_device::mouse::Button::Side)]
    );
    // and writing one with a colon instead of a comma is understood rather than silently ignored
    let colons = Bindings::from_text("JUP=gb:dpad-up\n");
    assert!(
        colons.problems().is_empty(),
        "a colon was rejected: {:?}",
        colons.problems()
    );
    assert_eq!(
        colons.bounds_for(&sectors, 0),
        vec![Bound::Gamepad(g13_device::joystick::Button::DpadUp)]
    );

    // and a stick that drives buttons still asks for those devices to be there
    assert!(bindings.needs_gamepad(), "the gamepad must be created");
    assert!(bindings.needs_mouse(), "the pointer must be created");
    // while the ones it does not use are not asked for
    assert!(
        Bindings::from_text("JUP=p,k.17\n")
            .bounds_for(&sectors, 0)
            .len()
            == 1
    );
}

#[cfg(test)]
mod a_screen_action_names_the_screen_it_goes_to {
    use super::*;

    /// `where_at` names the folder, so each test has its own: these run in parallel, and two of them writing the
    /// same file made one read it half-written - "EOF while parsing a value", which is what the gate's three runs
    /// caught and a single run did not.
    fn an_applet_with_screens(where_at: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("g13-screen-action-{where_at}"));
        let _ = std::fs::create_dir_all(&dir);
        write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("media.json"),
            r#"{"name": "media", "sources": {}, "screens": [
                 {"title": "playing", "widgets": [{"type": "text", "x": 0, "y": 0, "format": "PLAYING",
                  "command": "screen:playlist"}]},
                 {"title": "playlist", "widgets": [{"type": "text", "x": 0, "y": 0, "format": "LIST",
                  "command": ["cmd:playerctl play {screen_item}", "screen:playing"]}]}]}"#,
        )
        .unwrap();
        (dir, "applet:media".to_string())
    }

    #[test]
    fn a_screen_is_named_by_its_number_or_by_its_title() {
        let (dir, visual) = an_applet_with_screens("names");
        // counted from one, as the window and the menu count them
        assert_eq!(screen_of_applet(&visual, "1", &dir), Ok(0));
        assert_eq!(screen_of_applet(&visual, "2", &dir), Ok(1));
        // and by its title, which is what survives moving screens about
        assert_eq!(screen_of_applet(&visual, "playlist", &dir), Ok(1));
        assert_eq!(screen_of_applet(&visual, "PLAYING", &dir), Ok(0));
        assert_eq!(screen_of_applet(&visual, " playing ", &dir), Ok(0));
        // and something nobody has is refused *with the list*, not landed on
        let too_many = screen_of_applet(&visual, "3", &dir).unwrap_err();
        assert!(
            too_many.contains("playing") && too_many.contains("playlist"),
            "{too_many}"
        );
        let nothing = screen_of_applet(&visual, "chart", &dir).unwrap_err();
        assert!(
            nothing.contains("chart") && nothing.contains("playlist"),
            "{nothing}"
        );
        // zero is not a screen, since they are counted from one
        assert!(screen_of_applet(&visual, "0", &dir).is_err());
        // and a `screen:` action on something that is not an applet says so
        assert!(screen_of_applet("clock", "1", &dir).is_err());
    }

    #[test]
    fn a_widget_may_do_two_things_and_the_walk_carries_both() {
        // A playlist: pick a track and go back to the playing screen, on one press
        let (dir, _) = an_applet_with_screens("both");
        let applet =
            g13_applets::parse_applet(&std::fs::read_to_string(dir.join("media.json")).unwrap())
                .unwrap();
        assert!(applet.unhandled.is_empty(), "{:?}", applet.unhandled);
        let walk = g13_applets::selectables(&applet, 1, &[], &BTreeMap::new(), 0);
        assert_eq!(walk.len(), 1, "the list screen has one actionable widget");
        assert_eq!(
            walk[0].commands,
            vec![
                "cmd:playerctl play {screen_item}".to_string(),
                "screen:playing".to_string()
            ],
            "both actions should travel, in order"
        );
        // and a widget with one command written plainly still carries exactly that
        let plain = g13_applets::selectables(&applet, 0, &[], &BTreeMap::new(), 0);
        assert_eq!(plain[0].commands, vec!["screen:playlist".to_string()]);
    }
}

#[cfg(test)]
mod a_slow_source_does_not_stall_the_drawing {
    use super::*;

    #[test]
    fn frames_keep_coming_while_a_source_is_still_reading() {
        // The stutter, in one test: every reading is a process, the machine's numbers are about 51ms of them and an
        // applet's own `cmd:` can be slower still. Gathered on the drawing thread that is a frame lost every time,
        // which reads as a stutter about once a second on the pad's screen and in the window's preview. The source
        // here takes a whole second, so before this the drawing thread would manage about two frames in two seconds.
        let dir = std::env::temp_dir().join("g13-slow-source");
        let _ = std::fs::create_dir_all(dir.join("applets"));
        std::fs::write(
            dir.join("applets").join("slow.json"),
            r#"{"name": "slow", "interval": 1, "sources": {"n": "cmd:sleep 1"},
                 "widgets": [{"type": "text", "x": 0, "y": 0, "format": "n {n}",
                              "scroll_width": 10, "scroll": true, "scroll_speed": 30}]}"#,
        )
        .unwrap();
        let worker = ScreenWorker::start(
            "applet:slow".to_string(),
            dir.join("applets"),
            dir.clone(),
            // no writer: this is about the drawing, not about the panel
            None,
            ScreenWake::default(),
        );
        let started = std::time::Instant::now();
        let mut last = started;
        let mut frames = 0;
        let mut worst = std::time::Duration::ZERO;
        let mut periods: Vec<f64> = Vec::new();
        while started.elapsed() < std::time::Duration::from_secs(2) {
            if worker.take().is_some() {
                let gap = last.elapsed();
                periods.push(gap.as_secs_f64() * 1000.0);
                worst = worst.max(gap);
                last = std::time::Instant::now();
                frames += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // What this is about is that no frame waited for the source, and the measure of that is the *middle* of
        // the periods, not how many arrived: three test suites run at once on this machine, so a count depends on
        // what else is running. The median does not.
        let mut sorted = periods.clone();
        sorted.sort_by(|left, right| left.partial_cmp(right).unwrap());
        let median = sorted[sorted.len() / 2];
        assert!(
            median < 200.0,
            "the drawing waited for the source: the middle frame took {median:.0}ms, where fifty is expected"
        );
        assert!(
            frames > 15,
            "the drawing stopped altogether: {frames} frames in two seconds"
        );
        assert!(
            worst < std::time::Duration::from_millis(700),
            "a frame was lost to a gather, the worst gap being {worst:?} - the source takes a whole second, so \
             this says the frame came back before it finished"
        );
    }
}

#[cfg(test)]
mod the_screen_thread_writes_and_the_input_loop_does_not {

    #[test]
    fn a_worker_with_no_panel_draws_for_a_reader_instead_of_the_pad() {
        // the window's shape, and the only one it can have: it holds no `Panel` and cannot make one, so the
        // frames it draws are read out of the worker by the pane rather than written anywhere. A preview taking
        // the panel is a type error, not a promise.
        let dir = std::env::temp_dir().join("g13-no-panel");
        let _ = std::fs::create_dir_all(&dir);
        write_the_once_built_in_screens(&dir);
        let worker = ScreenWorker::start(
            "clock".to_string(),
            dir.join("applets"),
            dir.clone(),
            None,
            ScreenWake::default(),
        );

        let started = std::time::Instant::now();
        let mut drawn = 0;
        while started.elapsed() < std::time::Duration::from_secs(3) {
            if worker.take().is_some() {
                drawn += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(
            drawn > 0,
            "a worker with no panel still has to draw: the window's pane reads these frames"
        );
    }
    use super::*;
    use std::sync::Mutex;

    /// Stands in for the pad: records what it was asked to show.
    #[derive(Default)]
    struct Recorder {
        shown: Mutex<Vec<usize>>,
    }

    impl FrameSink for Recorder {
        fn show(&self, report: &[u8]) -> Result<usize, g13_device::DeviceError> {
            self.shown.lock().unwrap().push(report.len());
            Ok(report.len())
        }
    }

    #[test]
    fn a_worker_with_a_sink_puts_its_frames_there() {
        // the driver's worker writes the panel itself, so no frame write is left for the input loop to do
        let dir = std::env::temp_dir().join("g13-sink-test");
        let _ = std::fs::create_dir_all(&dir);
        write_the_once_built_in_screens(&dir);
        let recorder = Arc::new(Recorder::default());
        let worker = ScreenWorker::start(
            "clock".to_string(),
            dir.join("applets"),
            dir.clone(),
            Some(Panel(Arc::clone(&recorder) as Arc<dyn FrameSink>)),
            ScreenWake::default(),
        );
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(3) {
            if !recorder.shown.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(worker);
        let shown = recorder.shown.lock().unwrap().clone();
        assert!(
            !shown.is_empty(),
            "the worker drew but nothing reached the sink"
        );
        assert!(
            shown.iter().all(|bytes| *bytes > 900),
            "a frame should be the whole panel, got {shown:?}"
        );
    }

    #[test]
    fn the_input_loop_never_writes_the_panel_itself() {
        // The input loop has to stay clean and as fast as possible: that loop is the pad's responsiveness. One
        // frame write is 26ms measured, so this is not a style rule: a `write_lcd`
        // inside `run` is 26ms of the pad not being read, and it must not come back by accident.
        let source = include_str!("lib.rs");
        let start = source
            .find("pub fn run<F>")
            .expect("the driver loop is here");
        let rest = &source[start + 1..];
        // up to the tests, which live in this same file: the first version of this test scanned to the next
        // `pub fn`, found its own assertion text, and failed on the driver's behalf
        let end = rest.find("\n#[cfg(test)]").unwrap_or(rest.len());
        let loop_body = &rest[..end];
        assert!(
            !loop_body.contains("write_lcd"),
            "the input loop writes the panel again: that is a 26ms stall per frame"
        );
        assert!(
            loop_body.contains("read_packet"),
            "this test is looking at the wrong function"
        );
    }
}

/// The screens that used to be built in are applet files now, so any fixture that expects them in the walk has to
/// write them. At the crate root rather than inside one test module, so every test module can call it.
///
/// It writes into **both** the directory it is given and an `applets/` inside it, because the tests disagree about
/// which one is the applets folder: some pass a config directory with `applets/` in it, others pass the applets
/// folder itself. Writing both is what lets one helper serve them all, and in a temporary directory the extra files
/// cost nothing.
#[cfg(test)]
pub(crate) fn write_the_once_built_in_screens(dir: &std::path::Path) {
    let mut places = vec![dir.to_path_buf()];
    places.push(dir.join("applets"));
    for applets in places {
        let _ = std::fs::create_dir_all(&applets);
        for name in crate::visuals::ONCE_BUILT_IN {
            let _ = std::fs::write(
                applets.join(format!("{name}.json")),
                format!(
                    r#"{{"name": "{name}", "widgets": [{{"type": "text", "x": 0, "y": 0, "format": "x"}}]}}"#
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "G1=p,k.30\nLR=p,k.1\nM1=m,7,2\nG2=x\nnonsense=p,k.4\n";

    #[test]
    fn a_passthrough_binding_becomes_a_key_on_the_measured_bit() {
        let bindings = Bindings::from_text(FILE);
        assert_eq!(bindings.key_for(16), Some(KeyCode::new(30)), "G1 is bit 16");
        assert_eq!(bindings.key_for(40), Some(KeyCode::new(1)), "LR is bit 40");
    }

    #[test]
    fn an_unknown_control_is_reported_rather_than_silently_dropped() {
        let bindings = Bindings::from_text(FILE);
        assert!(
            bindings.problems().iter().any(|p| p.contains("nonsense")),
            "a name that is not a control on this pad is reported"
        );
    }

    #[test]
    fn a_macro_binding_becomes_a_macro_on_the_measured_bit() {
        let bindings = Bindings::from_text("M1=m,7,2\n");
        assert_eq!(bindings.macro_for(45), Some((7, 2)), "M1 is bit 45");
        assert!(
            bindings.problems().is_empty(),
            "a macro is something this build does"
        );
    }

    #[test]
    fn a_profile_switch_becomes_a_switch_on_the_measured_bit() {
        let bindings = Bindings::from_text("LR=mk,1\n");
        assert_eq!(bindings.switch_for(40), Some(1), "LR is bit 40");
        assert!(
            bindings.problems().is_empty(),
            "a switch is something this build does"
        );
    }

    #[test]
    fn every_action_can_be_said_in_words() {
        // The CLI once printed `sv,next  -  not applied by this build` about an action this build applies, because
        // the words lived in two places and only one was taught the new action. This walks the vocabulary: an
        // action with no words of its own is a control that reads as broken.
        for (action, expected) in [
            ("p,k.30", "sends a"),
            ("m,200,1", "plays macro 200, 1 time(s)"),
            ("mk,2", "switches to profile 2"),
            ("sv,next", "shows the next screen"),
            ("sv,prev", "shows the previous screen"),
            ("sv,applet:docker", "shows applet:docker"),
            ("mb,left", "presses the left mouse button"),
            ("gb,south", "presses south on the gamepad"),
            ("x", "nothing"),
        ] {
            assert_eq!(describe_action(action), expected, "{action}");
        }
        assert!(describe_action("rec").contains("records a macro"));
        assert!(describe_action("rec,3").contains("macro 3"));
        // and something that cannot be done says why rather than reading as if it works
        assert!(
            describe_action("sv,").contains("cannot be done"),
            "{}",
            describe_action("sv,")
        );
    }

    #[test]
    fn asking_for_another_visual_draws_it_now_rather_than_when_the_wait_ends() {
        let dir = std::env::temp_dir().join(format!("g13-wake-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_the_once_built_in_screens(&dir);
        // An applet that would be redrawn every five seconds, so a worker left to its own timer is five seconds
        // late - the shape of the "a little slow" report, made big enough to see.
        std::fs::write(
            dir.join("applets").join("changed.json"),
            r#"{"name": "changed", "interval": 5, "sources": {}, "widgets": [
                 {"type": "text", "x": 2, "y": 2, "format": "CHANGED"}]}"#,
        )
        .unwrap();
        let wake = ScreenWake::default();
        let worker = ScreenWorker::start(
            "clock".to_string(),
            dir.join("applets"),
            dir.clone(),
            // this test is about what the worker draws into, not about the panel
            None,
            wake.clone(),
        );

        // it draws what it was started with, and this is not a race about that
        let started = Instant::now();
        let mut first = None;
        while started.elapsed() < Duration::from_secs(3) {
            if let Some(frame) = worker.take() {
                first = Some(frame);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(first.is_some(), "the worker never drew anything");

        // now ask for the other one, and time how long the frame takes
        let asked = Instant::now();
        worker.show("applet:changed");
        let mut second = None;
        while asked.elapsed() < Duration::from_secs(3) {
            if let Some(frame) = worker.take() {
                second = Some(frame);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let took = asked.elapsed();
        let frame = second.expect("the new visual was never drawn");
        let first = first.expect("the first frame");
        // the applet writes `CHANGED` at (2, 2), so there is ink there; the clock does not
        let ink = (2..30)
            .flat_map(|x| (2..9).map(move |y| (x, y)))
            .any(|(x, y)| frame.get(x, y));
        assert!(
            ink,
            "the frame that arrived is not the visual that was asked for"
        );
        assert!(first != frame, "the frame did not change at all");
        assert!(
            took < Duration::from_millis(300),
            "asking for a visual took {took:?}: the worker was not woken, it waited out the applet's own \
             interval"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A menu of labels, for the tests that are about drawing and moving rather than about targets.
    fn a_menu(labels: &[&str], index: usize) -> Menu {
        Menu {
            title: String::new(),
            items: labels.iter().map(|label| label.to_string()).collect(),
            targets: labels
                .iter()
                .map(|label| MenuTarget::Show(label.to_string()))
                .collect(),
            index,
            level: 1,
            parent: None,
        }
    }

    #[test]
    fn the_menu_lists_the_screens_that_are_enabled_and_moves_through_them() {
        let dir = std::env::temp_dir().join(format!("g13-menu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_the_once_built_in_screens(&dir);
        // an applet with a title, one without, and a name that is not a screen at all
        std::fs::write(
            dir.join("demo-stats.json"),
            r#"{"name": "demo-stats", "title": "stats", "widgets": []}"#,
        )
        .unwrap();
        std::fs::write(dir.join("gpu.json"), r#"{"name": "gpu", "widgets": []}"#).unwrap();
        let enabled = vec![
            "clock".to_string(),
            "custom".to_string(),
            "applet:demo-stats".to_string(),
            "applet:gpu".to_string(),
        ];
        let mut menu = Menu::over_screens(&enabled, &dir, "clock").expect("a menu");
        // named the way a person would say it, and a screen this build cannot draw is not offered
        assert_eq!(menu.items, vec!["clock", "stats", "gpu"]);
        assert_eq!(
            menu.targets[1],
            MenuTarget::Show("applet:demo-stats".to_string())
        );
        assert_eq!(menu.level, 1);

        // moving wraps both ways, and choosing gives the visual rather than the label
        assert_eq!(menu.chosen(), Some(&MenuTarget::Show("clock".to_string())));
        menu.next();
        assert_eq!(
            menu.chosen(),
            Some(&MenuTarget::Show("applet:demo-stats".to_string()))
        );
        menu.previous();
        menu.previous();
        assert_eq!(
            menu.chosen(),
            Some(&MenuTarget::Show("applet:gpu".to_string())),
            "previous must wrap"
        );
        menu.next();
        assert_eq!(
            menu.chosen(),
            Some(&MenuTarget::Show("clock".to_string())),
            "next must wrap"
        );

        // nothing to list is not a menu
        assert!(Menu::over_screens(&["custom".to_string()], &dir, "clock").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_screen_can_carry_a_list_and_the_command_l4_runs_on_it() {
        let dir = std::env::temp_dir().join(format!("g13-screen-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("docker.json"),
            r#"{"name": "docker", "sources": {"containers": "cmd:printf 'alpha\\nbeta\\ngamma'"},
                "screens": [
                 {"title": "one", "widgets": [{"type": "text", "x": 0, "y": 0, "format": "no list here"}]},
                 {"title": "two", "widgets": [
                    {"type": "list", "x": 0, "y": 0, "w": 20, "rows": 3, "source": "containers",
                     "command": "cmd:docker restart {screen_item}"}]}]}"#,
        )
        .unwrap();
        // a screen with no list has nothing for L2/L3/L4, so they keep the profile's bindings
        assert!(visuals::list_source("applet:docker", 0, &dir).is_none());
        // the *spec* the applet declared for that name, not the name: `containers` means nothing on its own
        assert_eq!(
            visuals::list_source("applet:docker", 1, &dir).as_deref(),
            Some("cmd:printf 'alpha\\nbeta\\ngamma'")
        );
        // and a screen of an applet with no `screens` at all is the applet itself
        std::fs::write(
            dir.join("plain.json"),
            r#"{"name": "plain", "widgets": [
                 {"type": "list", "x": 0, "y": 0, "w": 20, "source": "containers", "command": "cmd:echo hi"}]}"#,
        )
        .unwrap();
        // a name nothing declares is a mechanism in its own right, and is left to say so when it is read
        assert_eq!(
            visuals::list_source("applet:plain", 0, &dir).as_deref(),
            Some("containers")
        );
        // not an applet at all, and an applet that is not there
        assert!(visuals::list_source("clock", 0, &dir).is_none());
        assert!(visuals::list_source("applet:nope", 0, &dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_selection_wraps_and_one_thing_to_act_on_does_not_move() {
        assert_eq!(step_selection(0, 3, false), 1);
        assert_eq!(step_selection(2, 3, false), 0, "next must wrap off the end");
        assert_eq!(
            step_selection(0, 3, true),
            2,
            "previous must wrap off the start"
        );
        assert_eq!(step_selection(1, 3, true), 0);
        // one item, or none: the same item rather than a division by zero
        assert_eq!(step_selection(0, 1, false), 0);
        assert_eq!(step_selection(0, 1, true), 0);
        assert_eq!(step_selection(0, 0, true), 0);
    }

    #[test]
    fn four_buttons_are_walked_in_order_and_each_says_what_it_would_do() {
        // Four text widgets - Start/Pause, Previous, Next, Stop for an audio applet. No list
        // anywhere: the widgets that carry a command are what L2/L3 move through and what L4 runs.
        let dir = std::env::temp_dir().join(format!("g13-buttons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("audio.json"),
            r#"{"name": "audio", "interval": 2, "sources": {}, "widgets": [
                 {"type": "text", "x": 4, "y": 2, "format": "Start/Pause", "command": "cmd:playerctl play-pause"},
                 {"type": "text", "x": 4, "y": 10, "format": "Previous", "command": "cmd:playerctl previous"},
                 {"type": "text", "x": 4, "y": 18, "format": "now playing"},
                 {"type": "text", "x": 4, "y": 26, "format": "Next", "command": "cmd:playerctl next"},
                 {"type": "text", "x": 4, "y": 34, "format": "Stop", "command": "cmd:playerctl stop"}]}"#,
        )
        .unwrap();

        let walk = visuals::screen_walk("applet:audio", 0, &dir, &[], &BTreeMap::new(), 0);
        assert_eq!(
            walk.len(),
            4,
            "only the widgets with a command can be acted on"
        );
        assert_eq!(
            walk.iter()
                .map(|thing| thing.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Start/Pause", "Previous", "Next", "Stop"],
            "the walk must follow the order they are written, which is the order they are drawn"
        );

        // walking it: forward wraps off the end, back wraps off the start, and what is chosen is said
        let mut state = State::new(1, &Bindings::from_text(""), Vec::new());
        let mut seen = Vec::new();
        for _ in 0..5 {
            note_selection(&mut state, &walk, &[]);
            seen.push(walk[state.screen_selected].label.clone());
            state.screen_selected = step_selection(state.screen_selected, walk.len(), false);
        }
        assert_eq!(
            seen,
            vec!["Start/Pause", "Previous", "Next", "Stop", "Start/Pause"]
        );
        // from the first one, back is the last: said outright rather than left to wherever the loop above
        // happened to stop
        state.screen_selected = 0;
        state.screen_selected = step_selection(state.screen_selected, walk.len(), true);
        note_selection(&mut state, &walk, &[]);
        assert_eq!(walk[state.screen_selected].label, "Stop", "back must wrap");
        // a button is not a row of a list, so there is no item for a command to use
        assert!(state.screen_item.is_empty());

        // a screen where nothing carries a command has nothing for L2/L3/L4, so they keep the file's bindings
        std::fs::write(
            dir.join("readings.json"),
            r#"{"name": "readings", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "just a reading"}]}"#,
        )
        .unwrap();
        assert!(
            visuals::screen_walk("applet:readings", 0, &dir, &[], &BTreeMap::new(), 0).is_empty()
        );
        // and so does a screen this build cannot read at all
        assert!(visuals::screen_walk("clock", 0, &dir, &[], &BTreeMap::new(), 0).is_empty());
        assert!(visuals::screen_walk("applet:nope", 0, &dir, &[], &BTreeMap::new(), 0).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_list_is_read_off_the_loop_and_the_chosen_item_comes_back() {
        // the whole point of the watch: the loop never runs the list's source itself
        let dir = std::env::temp_dir().join(format!("g13-list-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("mine.json"),
            // the list carries a command, or its rows would not be things the pad can act on
            r#"{"name": "mine", "sources": {"items": "cmd:printf 'one\\ntwo\\nthree'"},
                "widgets": [{"type": "list", "x": 0, "y": 0, "w": 20, "rows": 3, "source": "items",
                             "command": "cmd:echo {screen_item}"}]}"#,
        )
        .unwrap();

        let mut watch = ListWatch::new();
        let mut state = State::new(1, &Bindings::from_text(""), Vec::new());
        assert!(
            watch.follow("applet:mine", 0, &dir, &mut state),
            "a change of screen must start a read"
        );
        assert!(
            watch.items.is_empty(),
            "the loop must not have the items yet - they are read on a thread"
        );
        // the answer arrives and is drained
        let until = Instant::now() + Duration::from_secs(5);
        let mut said = None;
        while watch.items.is_empty() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            if let Some(problem) = watch.drain(&mut state) {
                said = Some(problem);
                break;
            }
        }
        assert_eq!(
            watch.items,
            vec!["one", "two", "three"],
            "the list did not arrive: {said:?}"
        );
        // the loop pairs the drain with the walk, because which item is chosen is a fact about the walk
        let walk = visuals::screen_walk("applet:mine", 0, &dir, &watch.items, &BTreeMap::new(), 0);
        note_selection(&mut state, &walk, &watch.items);
        assert_eq!(
            state.screen_item, "one",
            "the first item is chosen to start"
        );
        assert_eq!(state.screen_selected, 0);
        assert_eq!(walk.len(), 3, "each row of the list is one thing to act on");
        assert_eq!(
            state.screen_item, "one",
            "the first item is chosen to start"
        );
        assert_eq!(state.screen_selected, 0);

        // moving the cursor keeps the item beside it, and going to a screen with no list clears both
        state.screen_selected = step_selection(state.screen_selected, watch.items.len(), false);
        state.screen_item = watch.items[state.screen_selected].clone();
        assert_eq!(state.screen_item, "two");
        assert!(watch.follow("clock", 0, &dir, &mut state));
        assert_eq!(state.screen_selected, 0, "another screen starts at the top");
        assert!(state.screen_item.is_empty());
        // following the same screen again is not a read
        assert!(!watch.follow("clock", 0, &dir, &mut state));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_screens_select_runs_with_the_chosen_item_available_to_it() {
        // the mechanism the driver relies on: the spec is resolved against a values file, and the chosen item
        // is in it, so `{screen_item}` means what the list is showing rather than a second dialect of filling in
        let dir = std::env::temp_dir().join(format!("g13-select-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_the_once_built_in_screens(&dir);
        let values_path = dir.join("values.json");
        std::fs::write(
            &values_path,
            r#"{"screen_item": "web-1", "screen_selected": 2}"#,
        )
        .unwrap();
        let wrote = dir.join("ran");
        let world = g13_sources::World::default().with_values(&values_path);
        // the two steps the driver takes: the applet's own filler turns `{screen_item}` into what the list is
        // showing, quoted because the thing it filled is a command, and the resolver runs what is left.
        // `resolve` alone leaves the braces in place, which is exactly what the first version did.
        let values = visuals::read_published_values_from(&values_path);
        let filled = g13_applets::fill_command(
            &format!("cmd:echo restarted {{screen_item}} > {}", wrote.display()),
            &values,
        );
        let spec = g13_sources::Spec::parse(&filled);
        g13_sources::resolve(&spec, &world).expect("the select ran");
        assert_eq!(
            std::fs::read_to_string(&wrote).unwrap().trim(),
            "restarted web-1",
            "the chosen item did not reach the command"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_value_that_looks_like_shell_code_is_a_value_and_not_code() {
        let dir = std::env::temp_dir().join(format!("g13-injection-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let canary = dir.join("canary");
        let wrote = dir.join("ran");
        let values_path = dir.join("values.json");
        // what a list's row is when the thing the list is showing is called this: a track's title, a
        // container's name, a line of somebody else's file
        std::fs::write(
            &values_path,
            format!(
                r#"{{"screen_item": "x; touch {}", "screen_selected": 1}}"#,
                canary.display()
            ),
        )
        .unwrap();
        let world = g13_sources::World::default().with_values(&values_path);
        let values = visuals::read_published_values_from(&values_path);
        let filled = g13_applets::fill_command(
            &format!("cmd:echo {{screen_item}} > {}", wrote.display()),
            &values,
        );
        let spec = g13_sources::Spec::parse(&filled);
        g13_sources::resolve(&spec, &world).expect("the command ran");
        assert!(!canary.exists(), "the value ran as code: {filled}");
        assert_eq!(
            std::fs::read_to_string(&wrote).unwrap().trim(),
            format!("x; touch {}", canary.display()),
            "the value did not reach the command as one argument"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_screen_target_keeps_its_spelling_and_a_command_is_quoted() {
        // quoting a `screen:` target would name a screen that does not exist, which is why the kind is
        // read from the spec before anything is substituted into it
        let mut values = g13_applets::Values::new();
        values.insert(
            "screen_item".into(),
            g13_values::Value::Text("playlist".into()),
        );
        assert_eq!(
            g13_applets::fill_command("screen:{screen_item}", &values),
            "screen:playlist"
        );
        assert_eq!(
            g13_applets::fill_command("cmd:echo {screen_item}", &values),
            "cmd:echo 'playlist'"
        );
    }
    #[test]
    fn a_user_menu_replaces_the_rotation_and_a_broken_one_leaves_it_alone() {
        let dir = std::env::temp_dir().join(format!("g13-menu-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_the_once_built_in_screens(&dir);
        let menu_file = dir.join("menu.json");
        let applets = dir.join("applets");
        // through the file the driver actually reads, rather than a struct built by hand
        std::fs::write(
            dir.join("visuals.json"),
            r#"{"active": "clock", "enabled": ["clock", "media"]}"#,
        )
        .unwrap();
        let visuals = visuals::load_visuals(&dir.join("visuals.json"));

        // no file: the rotation, as it has always been
        let (menu, complaint) = menu_for(&visuals, &applets, "clock", &menu_file).expect("a menu");
        assert!(complaint.is_none());
        assert_eq!(menu.items, vec!["clock", "media"]);

        // a hand-written file, with a level in it, a screen of an applet and a command
        std::fs::write(
            &menu_file,
            r#"{"items": [
                 {"label": "Watch", "items": [
                    {"label": "temp", "show": "applet:temps"},
                    {"label": "docker", "show": "applet:docker", "screen": 2}]},
                 {"label": "Music", "command": "cmd:playerctl play-pause"},
                 {"label": "Clock", "show": "clock"}]}"#,
        )
        .unwrap();
        let (menu, complaint) = menu_for(&visuals, &applets, "clock", &menu_file).expect("a menu");
        assert!(complaint.is_none(), "{complaint:?}");
        assert_eq!(menu.items, vec!["Watch", "Music", "Clock"]);
        assert!(matches!(menu.targets[0], MenuTarget::Child(_)));
        assert_eq!(
            menu.targets[1],
            MenuTarget::Run("cmd:playerctl play-pause".to_string())
        );
        assert_eq!(menu.targets[2], MenuTarget::Show("clock".to_string()));

        // the level under Watch is hand-written too, and its screen counts from one as the window writes it
        let MenuTarget::Child(watch) = menu.targets[0].clone() else {
            panic!("Watch must open a level");
        };
        let level = child_level(&watch).expect("the level under Watch");
        assert_eq!(level.items, vec!["temp", "docker"]);
        assert_eq!(level.title, "Watch");
        assert_eq!(
            level.targets[1],
            MenuTarget::ShowAt {
                visual: "applet:docker".to_string(),
                screen: 1
            },
            "screen 2 of the file is the second screen"
        );
        // and the keys work on it: choosing the level opens it over the list
        assert_eq!(
            menu_move(menu.clone(), MenuKey::Select, &applets),
            MenuMove::Deeper(menu.clone(), level)
        );

        // a file that will not read is said, and the rotation is what opens: the pad's own button must not go
        // with it
        std::fs::write(&menu_file, r#"{"items": [{"label": "broken"}]}"#).unwrap();
        let (menu, complaint) = menu_for(&visuals, &applets, "clock", &menu_file).expect("a menu");
        assert!(
            complaint.unwrap_or_default().contains("broken"),
            "a menu file with a do-nothing item was not reported"
        );
        assert_eq!(
            menu.items,
            vec!["clock", "media"],
            "the rotation must still open"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_menu_keys_decide_what_l1_to_l4_do_without_a_pad() {
        let dir = std::env::temp_dir().join(format!("g13-menu-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("docker.json"),
            r#"{"name": "docker", "sources": {}, "screens": [
                 {"title": "containers", "widgets": []}, {"title": "images", "widgets": []}]}"#,
        )
        .unwrap();

        let list = Menu::over_screens(
            &["clock".to_string(), "applet:docker".to_string()],
            &dir,
            "applet:docker",
        )
        .expect("a menu");
        // the first item is the applet that is showing, because that is the one with levels under it
        assert!(matches!(list.targets[0], MenuTarget::Screens(_)));

        // L4 on it opens a level rather than choosing: the list is kept under it
        let level = match menu_move(list.clone(), MenuKey::Select, &dir) {
            MenuMove::Deeper(below, level) => {
                assert_eq!(below.level, 1);
                assert_eq!(level.items, vec!["containers", "images"]);
                level
            }
            other => panic!("L4 on an applet with screens must open them, not {other:?}"),
        };
        let opened = list.clone().open_over(level);
        assert_eq!(opened.level, 2);

        // L1 comes back one level - not out of the menu
        match menu_move(opened.clone(), MenuKey::Back, &dir) {
            MenuMove::Wound(Some(back)) => assert_eq!(back.level, 1),
            other => panic!("L1 must go back one level, not {other:?}"),
        }
        // and at the first level there is nothing above, so there it closes
        assert_eq!(
            menu_move(list.clone(), MenuKey::Back, &dir),
            MenuMove::Wound(None)
        );

        // L2 and L3 move, and L4 on a screen of the applet chooses it - with which screen
        let moved = match menu_move(opened.clone(), MenuKey::Next, &dir) {
            MenuMove::Stayed(moved) => moved,
            other => panic!("L3 must move the cursor, not {other:?}"),
        };
        assert_eq!(moved.index, 1);
        assert_eq!(
            menu_move(moved, MenuKey::Select, &dir),
            MenuMove::Chose(MenuTarget::ShowAt {
                visual: "applet:docker".to_string(),
                screen: 1
            })
        );
        // a screen of the rotation is chosen by its visual, with no screen of its own to set
        let mut on_the_list = opened.back().expect("the list");
        // the list is the applet's own item first, then the enabled screens in their order
        assert_eq!(
            on_the_list.targets,
            vec![
                MenuTarget::Screens("applet:docker".to_string()),
                MenuTarget::Show("clock".to_string()),
                MenuTarget::Show("applet:docker".to_string()),
            ]
        );
        on_the_list.index = 1;
        assert_eq!(
            menu_move(on_the_list.clone(), MenuKey::Select, &dir),
            MenuMove::Chose(MenuTarget::Show("clock".to_string()))
        );
        on_the_list.index = 2;
        assert_eq!(
            menu_move(on_the_list, MenuKey::Select, &dir),
            MenuMove::Chose(MenuTarget::Show("applet:docker".to_string()))
        );

        // a file that lost its second screen between two presses is refused by name rather than opening nothing
        std::fs::write(
            dir.join("one.json"),
            r#"{"name": "one", "screens": [{"widgets": []}]}"#,
        )
        .unwrap();
        let mut level = screens_level("applet:docker", &dir).expect("a level");
        level.items = vec!["one".to_string()];
        level.targets = vec![MenuTarget::Screens("applet:one".to_string())];
        match menu_move(list.clone(), MenuKey::Select, &dir) {
            MenuMove::Deeper(..) => {}
            other => panic!("expected the good applet to open, got {other:?}"),
        }
        let mut empty = list;
        empty.items = vec!["one".to_string()];
        empty.targets = vec![MenuTarget::Screens("applet:one".to_string())];
        match menu_move(empty, MenuKey::Select, &dir) {
            MenuMove::Refused(why) => assert!(why.contains("applet:one"), "{why}"),
            other => panic!("an applet with one screen has no level to open, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_level_of_its_own_goes_over_the_one_below_and_back_returns_to_it() {
        let dir = std::env::temp_dir().join(format!("g13-menu-levels-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_the_once_built_in_screens(&dir);
        std::fs::write(
            dir.join("docker.json"),
            r#"{"name": "docker", "title": "docker", "sources": {}, "screens": [
                 {"title": "containers", "widgets": []}, {"title": "images", "widgets": []}]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("clock.json"), "{}").ok();

        // what is showing has screens of its own, so the menu offers them as a level
        let enabled = vec!["clock".to_string(), "applet:docker".to_string()];
        let menu = Menu::over_screens(&enabled, &dir, "applet:docker").expect("a menu");
        assert_eq!(menu.level, 1);
        assert_eq!(menu.items[0], "docker  2 screens", "{:?}", menu.items);
        assert_eq!(
            menu.targets[0],
            MenuTarget::Screens("applet:docker".to_string())
        );

        // that level is the applet's own screens, named by their titles
        let level = screens_level("applet:docker", &dir).expect("a level of screens");
        assert_eq!(level.items, vec!["containers", "images"]);
        assert_eq!(level.title, "docker");
        assert_eq!(
            level.targets[1],
            MenuTarget::ShowAt {
                visual: "applet:docker".to_string(),
                screen: 1
            }
        );
        // one screen is not a level: there would be nothing to move through
        std::fs::write(
            dir.join("solo.json"),
            r#"{"name": "solo", "screens": [{"widgets": []}]}"#,
        )
        .unwrap();
        assert!(screens_level("applet:solo", &dir).is_none());

        // opening it over the list keeps the list, with the cursor where it was left
        let mut list = menu.clone();
        list.next();
        assert_eq!(list.index, 1);
        let opened = list.clone().open_over(level);
        assert_eq!(opened.level, 2);
        let back = opened.back().expect("a level to go back to");
        assert_eq!(back.level, 1);
        assert_eq!(
            back.index, 1,
            "going back lost the cursor, which is the whole point of a level rather than a reset"
        );
        // and at the top there is nothing above, so back means closing
        assert!(back.back().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_menu_is_drawn_with_a_mark_on_the_one_that_would_be_chosen() {
        let menu = a_menu(&["clock", "media", "pad"], 1);
        let lines = menu_lines(&menu, 6, 26);
        assert_eq!(lines[0], "menu  2/3");
        assert_eq!(lines[1], "  clock");
        assert_eq!(lines[2], "> media", "the chosen one carries the mark");
        assert_eq!(lines[3], "  pad");
        assert_eq!(
            lines.last().unwrap(),
            "L1 close L2/3 move L4 pick",
            "at the top level there is nothing above to go back to"
        );
        assert!(
            lines.last().unwrap().chars().count() <= g13_screen::TEXT_COLUMNS,
            "the key hint does not fit the panel and would be cut on the pad: {lines:?}"
        );
        // and one level down it says back, because that is what L1 does there; this one is shorter, so it fits
        let opened_over = menu.clone().open_over(Menu {
            title: "stats".to_string(),
            items: vec!["first".to_string(), "second".to_string()],
            targets: vec![
                MenuTarget::ShowAt {
                    visual: "applet:x".to_string(),
                    screen: 0,
                },
                MenuTarget::ShowAt {
                    visual: "applet:x".to_string(),
                    screen: 1,
                },
            ],
            index: 0,
            level: 2,
            parent: None,
        });
        let lines = menu_lines(&opened_over, 6, 26);
        assert_eq!(lines[0], "stats  1/2", "a level of its own says what it is");
        assert!(
            lines.last().unwrap().starts_with("L1 back"),
            "L1 must say back when there is a level above: {lines:?}"
        );
        assert!(
            lines.last().unwrap().chars().count() <= g13_screen::TEXT_COLUMNS,
            "the key hint does not fit the panel: {lines:?}"
        );
        // exactly one mark, whatever is chosen
        for index in 0..menu.items.len() {
            let mut moved = menu.clone();
            moved.index = index;
            let marks = menu_lines(&moved, 6, 26)
                .iter()
                .filter(|line| line.starts_with('>'))
                .count();
            assert_eq!(marks, 1, "at index {index}");
        }

        // more items than rows: the window follows the mark, so the chosen one is always visible
        let many = a_menu(
            &(1..=9)
                .map(|n| format!("screen {n}"))
                .collect::<Vec<String>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<&str>>(),
            8,
        );
        let lines = menu_lines(&many, 6, 26);
        assert!(
            lines.iter().any(|line| line.starts_with("> screen 9")),
            "the chosen item scrolled out of sight: {lines:?}"
        );
        assert!(lines[0].starts_with("menu  9/9"), "{lines:?}");

        // and a long label is cut rather than wrapped into the next row
        let wide = a_menu(&[&"a".repeat(60)], 0);
        assert!(
            menu_lines(&wide, 6, 26)
                .iter()
                .all(|line| line.chars().count() <= 26)
        );

        // and it reaches the panel
        let frame = menu_frame(&menu);
        assert!(
            !frame.is_blank(0, 0, g13_screen::WIDTH, g13_screen::VISIBLE_HEIGHT),
            "the menu drew nothing"
        );
    }

    #[test]
    fn a_tap_is_not_deferred_again_when_it_comes_back_as_its_short_action() {
        // The sequence that broke on the pad: press, release, and then the loop turning that release back into the
        // control's short action. Deferring that again stamped a new start time, so `due` fired it 500ms later as
        // a hold - a tap of LR opened the menu and never moved the screen.
        let start = Instant::now();
        let long = Duration::from_millis(500);
        let mut holds = Holds::new(long);

        assert!(
            holds.pressed(40, start, true),
            "a press waits for the release"
        );
        assert!(holds.released(40), "and it was a tap");

        // the tap's own dispatch: not a fresh press, so it must not be deferred
        assert!(!defer_to_hold(false, true));
        assert!(
            holds.due(start + long).is_empty(),
            "a tap fired a hold half a second later"
        );
        // while a real press of a control with a hold still is deferred
        assert!(defer_to_hold(true, true));
        assert!(
            !defer_to_hold(true, false),
            "a control with no hold is untouched"
        );
    }

    #[test]
    fn a_tap_and_a_hold_are_told_apart() {
        let start = Instant::now();
        let long = Duration::from_millis(500);

        // a quick tap: nothing on press, and the short action when it is let go
        let mut holds = Holds::new(long);
        assert!(
            holds.pressed(40, start, true),
            "a control with a hold must wait"
        );
        assert!(holds.due(start + Duration::from_millis(100)).is_empty());
        assert!(holds.released(40), "a tap must apply the short action");
        assert!(!holds.any());

        // a hold: nothing on press, the long action at the threshold, and nothing on release
        let mut holds = Holds::new(long);
        holds.pressed(40, start, true);
        assert!(holds.due(start + Duration::from_millis(499)).is_empty());
        assert_eq!(holds.due(start + long), vec![40]);
        // and only once, however many passes go by
        assert!(holds.due(start + long + long).is_empty());
        assert!(
            !holds.released(40),
            "a hold must not also do the short thing"
        );
        assert!(!holds.any());

        // a control with no hold is not tracked at all: nothing about it is slower
        let mut holds = Holds::new(long);
        assert!(!holds.pressed(16, start, false));
        assert!(!holds.any());
        assert!(!holds.released(16));

        // two controls at once, each on its own clock
        let mut holds = Holds::new(long);
        holds.pressed(41, start, true);
        holds.pressed(42, start + Duration::from_millis(400), true);
        assert_eq!(holds.due(start + long), vec![41]);
        assert!(
            holds.holding(41),
            "a control that fired is still being held"
        );
        assert_eq!(holds.due(start + Duration::from_millis(900)), vec![42]);
        // both fired their holds, so neither applies a short action when it is let go
        assert!(!holds.released(41));
        assert!(!holds.released(42));
    }

    #[test]
    fn a_control_can_change_what_the_screen_shows() {
        // the vocabulary, both spellings, and the two words that mean a direction
        assert_eq!(parse_action("sv,next"), Action::Screen("next".to_string()));
        assert_eq!(parse_action("sv,prev"), Action::Screen("prev".to_string()));
        assert_eq!(
            parse_action("sv,applet:docker"),
            Action::Screen("applet:docker".to_string())
        );
        assert_eq!(
            parse_action("sv:clock"),
            Action::Screen("clock".to_string())
        );
        assert_eq!(
            parse_action("sv, clock "),
            Action::Screen("clock".to_string())
        );
        // and one that names nothing is reported rather than taken as "next"
        let problem = match parse_action("sv,") {
            Action::Unsupported(problem) => problem,
            other => panic!("expected a complaint, got {other:?}"),
        };
        assert!(problem.contains("sv,next"), "{problem}");
    }

    #[test]
    fn lr_changes_the_screen_unless_the_file_says_otherwise() {
        let dir = std::env::temp_dir().join(format!("g13-agent-lr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_the_once_built_in_screens(&dir);

        // a profile file that says nothing: LR gets its default job
        let path = dir.join("bindings-1.properties");
        let bindings = effective_bindings("G1=p,k.30\n", &path);
        let lr = bit_for_control(Control::LeftRound).expect("LR has a bit");
        assert_eq!(
            bindings.screen_for(lr),
            Some("next"),
            "LR has no default job"
        );

        // a `.hold` line is about the hold and not about the tap, so the tap keeps its default: this is the
        // shape that broke first, because a hold counted as "the file bound this control"
        let bindings = effective_bindings("G1=p,k.30\nLR.hold=menu\n", &path);
        assert_eq!(
            bindings.screen_for(lr),
            Some("next"),
            "a hold line stopped the tap from working"
        );
        assert_eq!(bindings.hold_for(lr), Some("menu"));

        // and a line for the tap wins, as it does for the M keys
        let bindings = effective_bindings("G1=p,k.30\nLR=m,200,1\n", &path);
        assert_eq!(bindings.screen_for(lr), None);
        assert_eq!(bindings.macro_for(lr), Some((200, 1)));

        // L1-L4 are left alone: they are for screens inside an applet, which do not exist yet, and a default
        // that guessed would take four controls away for a feature that is not there
        for control in [Control::L1, Control::L2, Control::L3, Control::L4] {
            let bit = bit_for_control(control).expect("a bit");
            assert!(
                !bindings.bound(bit),
                "{} was given a job it was not asked for",
                control.name()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn x_means_nothing_and_is_not_a_problem() {
        let bindings = Bindings::from_text("G2=x\n");
        assert!(bindings.is_empty());
        assert!(bindings.problems().is_empty());
    }

    #[test]
    fn the_map_has_one_entry_per_usable_binding() {
        let bindings = Bindings::from_text(FILE);
        assert_eq!(
            bindings.len(),
            3,
            "G1 and LR send keys, M1 plays a macro; the x sends nothing and the unknown name is not a control"
        );
    }

    #[test]
    fn a_file_with_nothing_usable_loads_as_an_empty_map() {
        assert!(Bindings::from_text("").is_empty());
        assert!(Bindings::from_text("color=255,255,255\n").is_empty());
    }

    #[test]
    fn actions_are_classified_exactly() {
        assert_eq!(parse_action("p,k.44"), Action::Key(KeyCode::new(44)));
        assert_eq!(parse_action("x"), Action::Nothing);
        assert_eq!(parse_action("m,1,2"), Action::Macro(1, 2));
        assert_eq!(
            parse_action("m,3"),
            Action::Macro(3, 1),
            "repeats default to one"
        );
        assert_eq!(
            parse_action("m,0,0"),
            Action::Macro(0, 1),
            "zero repeats still plays once"
        );
        assert_eq!(parse_action("mk,1"), Action::SwitchProfile(1));
        assert!(matches!(parse_action("wibble"), Action::Unsupported(_)));
    }
}

/// The workers as a set: nothing else holds these handles, so dropping the set is what stops them.
///
/// The test is about the *absence* of work after the drop, over more than one drawing interval - a thread that
/// is merely quiet would pass a shorter wait, and "still running with no way to stop it" is the fault this
/// struct exists to make impossible.
#[cfg(test)]
mod workers_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A sink that counts frames, so "still drawing" is a number rather than an impression.
    #[derive(Default)]
    struct Counting {
        frames: AtomicUsize,
    }

    impl FrameSink for Counting {
        fn show(&self, report: &[u8]) -> Result<usize, g13_device::DeviceError> {
            self.frames.fetch_add(1, Ordering::Relaxed);
            Ok(report.len())
        }
    }

    #[test]
    fn dropping_the_workers_stops_what_it_owns() {
        let dir = std::env::temp_dir().join("g13-workers-set");
        let _ = std::fs::create_dir_all(&dir);
        write_the_once_built_in_screens(&dir);
        let sink = Arc::new(Counting::default());

        let workers = Workers {
            screen: ScreenWorker::start(
                "clock".to_string(),
                dir.join("applets"),
                dir.clone(),
                Some(Panel(Arc::clone(&sink) as Arc<dyn FrameSink>)),
                ScreenWake::default(),
            ),
            // the settings a file that says nothing gives: no mode, so the worker makes no device
            stick: StickWorker::start(g13_config::stick::Settings::load(&dir)),
        };

        // it is running: the screen draws without anybody asking it to
        let deadline = Instant::now() + Duration::from_secs(5);
        while sink.frames.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let drawn = sink.frames.load(Ordering::Relaxed);
        assert!(
            drawn > 0,
            "the screen never drew, so there was nothing to stop"
        );

        drop(workers);

        // and after the drop nothing more: the drawing thread is gone, not merely between frames
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(
            sink.frames.load(Ordering::Relaxed),
            drawn,
            "frames were still being written after the workers were dropped"
        );
    }
}

#[cfg(test)]
mod playing_tests {
    use super::*;

    /// Playing a macro really means pressing and releasing keys through a virtual keyboard.
    ///
    /// Keycode 194 is F24: nothing reacts to it, so this can run on a real desktop without disturbing
    /// anything. On a machine where a virtual keyboard cannot be created  -  CI, a container  -  the test says
    /// so and stops rather than pretending it passed.
    #[test]
    fn a_macro_is_played_through_the_keyboard() {
        let Ok(keyboard) = g13_device::keyboard::VirtualKeyboard::new() else {
            eprintln!("no virtual keyboard here; skipping the playback test");
            return;
        };
        let keyboard = Arc::new(Mutex::new(keyboard));
        let macro_file = g13_config::Macro {
            name: "test".to_string(),
            id: 999,
            steps: vec![
                g13_config::MacroStep::KeyDown(194),
                g13_config::MacroStep::Delay(5),
                g13_config::MacroStep::KeyUp(194),
            ],
        };
        assert_eq!(play_macro(&keyboard, &macro_file, 2), Ok(()));
    }

    /// A macro comes out at the speed it went in.
    ///
    /// The delays are deadlines counted from the start, not waits added after the work of the step before. This
    /// is the guarantee a macro is held to: a macro replays at the speed it was entered, because time-sensitive
    /// patterns are mapped to macros - so it is measured rather than asserted in prose. F24 is used
    /// because nothing on a real desktop reacts to it.
    #[test]
    fn a_macro_plays_at_the_speed_it_was_recorded() {
        let Ok(keyboard) = g13_device::keyboard::VirtualKeyboard::new() else {
            eprintln!("no virtual keyboard here; skipping the playback timing test");
            return;
        };
        let keyboard = Arc::new(Mutex::new(keyboard));
        let macro_file = g13_config::Macro {
            name: "timing".to_string(),
            id: 998,
            steps: vec![
                g13_config::MacroStep::KeyDown(194),
                g13_config::MacroStep::Delay(60),
                g13_config::MacroStep::KeyUp(194),
                g13_config::MacroStep::Delay(40),
                g13_config::MacroStep::KeyDown(194),
                g13_config::MacroStep::Delay(60),
                g13_config::MacroStep::KeyUp(194),
            ],
        };
        let started = std::time::Instant::now();
        let mut marks: Vec<u128> = Vec::new();
        assert_eq!(
            play_macro_reporting(&keyboard, &macro_file, 1, |step| {
                if !matches!(step, g13_config::MacroStep::Delay(_)) {
                    marks.push(started.elapsed().as_millis());
                }
            }),
            Ok(())
        );
        // due: press 0, release 60, press 100, release 160
        assert_eq!(marks.len(), 4, "not every key was performed");
        for (index, want) in [0i128, 60, 100, 160].iter().enumerate() {
            let got = marks[index] as i128;
            assert!(
                (got - want).abs() <= 25,
                "step {index} happened at {got}ms instead of {want}ms: {marks:?}"
            );
        }
    }

    /// A pause at the very start is the trigger, not the pattern: it is dropped rather than played back.
    #[test]
    fn a_leading_pause_is_not_played_back() {
        let Ok(keyboard) = g13_device::keyboard::VirtualKeyboard::new() else {
            eprintln!("no virtual keyboard here; skipping the leading-pause test");
            return;
        };
        let keyboard = Arc::new(Mutex::new(keyboard));
        let macro_file = g13_config::Macro {
            name: "pre-roll".to_string(),
            id: 997,
            steps: vec![
                g13_config::MacroStep::Delay(2_000),
                g13_config::MacroStep::KeyDown(194),
                g13_config::MacroStep::KeyUp(194),
            ],
        };
        let started = std::time::Instant::now();
        assert_eq!(play_macro(&keyboard, &macro_file, 1), Ok(()));
        let took = started.elapsed().as_millis();
        assert!(
            took < 500,
            "a macro that waits 2000ms before its first key took {took}ms, so the pause was played back"
        );
    }

    #[test]
    fn an_empty_macro_plays_nothing_and_succeeds() {
        let Ok(keyboard) = g13_device::keyboard::VirtualKeyboard::new() else {
            return;
        };
        let keyboard = Arc::new(Mutex::new(keyboard));
        let macro_file = g13_config::Macro {
            name: "empty".to_string(),
            id: 0,
            steps: Vec::new(),
        };
        assert_eq!(play_macro(&keyboard, &macro_file, 3), Ok(()));
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn a_driver_that_followed_its_profile_publishes_the_one_it_is_using() {
        // the fault this exists for: the published profile was set by the pad's own profile keys and by
        // nothing else, so a profile switched in the file - `g13 profile 2`, an edit, the window - left
        // `g13 values` naming the profile the driver had *before* the switch, until it was restarted
        let mut state = State::new(1, &Bindings::from_text("G1=p,k.30"), Vec::new());
        assert_eq!(state.applied, 1);

        let mut followed = Bindings::default();
        followed.set_switch(16, 2);
        followed.set_screen(17, "clock");

        what_is_in_force(&mut state, 2, &followed);

        assert_eq!(state.profile, 2, "the profile the map in force belongs to");
        assert_eq!(
            state.applied, 2,
            "and how many controls it gives something to"
        );
    }

    #[test]
    fn the_published_state_is_the_shape_values_reads() {
        let bindings = Bindings::from_text("G1=a\nG2=b\nnonsense=c\n");
        let mut state = State::new(0, &bindings, bindings.problems());
        state.note("G1", true);
        state.note("G1", false);
        let json = state.to_json();

        // the fields `g13 values` looks up by name
        for field in [
            "\"version\":\"",
            "\"pid\":",
            "\"started_ms\":",
            "\"profile\":0",
            "\"applied\":",
            "\"problems\":[",
            "\"last_keys\":[",
        ] {
            assert!(json.contains(field), "the state is missing {field}: {json}");
        }
        // and the keys it holds
        assert!(json.contains("\"control\":\"G1\",\"pressed\":true"));
        assert!(json.contains("\"control\":\"G1\",\"pressed\":false"));
        // nothing unescaped can break the file open
        let nasty = Bindings::from_text("G1=a\n\"quote\"=b\n");
        let mut state = State::new(0, &nasty, vec!["a \"quoted\" problem".to_string()]);
        state.note("G\"1", true);
        let json = state.to_json();
        assert!(json.contains("a \\\"quoted\\\" problem"), "{json}");
        assert!(json.contains("G\\\"1"), "{json}");
    }

    #[test]
    fn the_recent_keys_do_not_grow_without_end() {
        let bindings = Bindings::from_text("G1=a\n");
        let mut state = State::new(0, &bindings, Vec::new());
        for index in 0..50 {
            state.note("G1", index % 2 == 0);
        }
        assert_eq!(state.last_keys.len(), 12);
    }
}

#[cfg(test)]
mod which_controls_are_down {
    use super::*;

    #[test]
    fn a_pad_drawn_from_the_values_says_only_what_somebody_can_press() {
        // G1 and G2 are bits 16 and 17, which is byte 2 of the payload. The names must be the bindings' own names,
        // because the pad widget and a bindings file have to mean the same thing by `G1`.
        let two_buttons = g13_proto::InputReport::from_payload(&[0, 0, 0b0000_0011, 0, 0, 0, 0]);
        assert_eq!(pressed_controls(&two_buttons), "G1,G2");

        // nothing pressed is an empty string, never a missing key: a key that comes and goes is one a reader has
        // to guess about, which is the rule the stick's values already follow
        let nothing = g13_proto::InputReport::default();
        assert_eq!(pressed_controls(&nothing), "");

        // the stick's own bits and the report's flags are not controls: bit 0 is the stick, bit 39 is a flag that
        // is set on this pad and means nothing anybody can press.
        let stick_and_flag =
            g13_proto::InputReport::from_payload(&[0b0000_0001, 0, 0, 0, 0b1000_0000, 0, 0]);
        assert_eq!(
            pressed_controls(&stick_and_flag),
            "",
            "a pad drawn from this would grow a row of things nobody can press"
        );
        // and a flag alongside a real control leaves the control alone
        let both = g13_proto::InputReport::from_payload(&[0, 0, 0b0000_0001, 0, 0b1000_0000, 0, 0]);
        assert_eq!(pressed_controls(&both), "G1");
    }
}

#[cfg(test)]
mod what_a_hold_can_be {
    use super::*;

    /// Write a bindings file and load it the way the driver does.
    fn loaded(text: &str) -> Vec<(String, String)> {
        let dir = std::env::temp_dir().join(format!("g13-holds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bindings-0.properties");
        std::fs::write(&path, text).unwrap();
        let bindings = load_bindings(&path);
        let _ = std::fs::remove_dir_all(&dir);
        // what the file could not be read as, which is where a hold this build cannot do is said
        bindings.unsupported().to_vec()
    }

    #[test]
    fn any_action_may_be_the_one_a_hold_does() {
        // A hold is *when* something happens, not a kind of thing, so a key or a macro can be the second
        // function of a control. This used to be refused at load with "a hold can be `menu` or a screen change".
        let unsupported = loaded(
            "G1=p,k.30\nG1.hold=m,4,1\nG2=p,k.31\nG2.hold=p,k.32\nG3=p,k.33\nG3.hold=sv,next\n",
        );
        assert!(
            unsupported.is_empty(),
            "a hold on a key, a macro and a screen change should all be allowed: {unsupported:?}"
        );
    }
}
