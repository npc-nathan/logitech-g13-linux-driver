//! The window: what it holds, and what it does when you change something.
//!
//! Everything here is separate from the drawing so it can be tested without a display. The window itself is
//! thin: it renders this and calls `edit`/`select`/`toggle`, which write the configuration as they go.
//!
//! Two rules from the way the rest of this is built:
//!
//! - **Edits apply immediately.** There is no Apply button and no confirmation: a change writes the file, and
//!   a running driver picks it up within a second, exactly as if `g13 bind` had been used.
//! - **Nothing is hidden.** What the window cannot do is shown rather than dropped.

use g13_agent::{Bindings, visuals};
use g13_screen::Frame;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub mod tabs;
pub use tabs::{
    ANIMATE_WORDS, FieldKind, FieldValue, MenuDoes, MenuRow, Origin, Row, SectorRow, Tab,
    ValueRead, ValueRow, WIDGET_KINDS, WaitingImport, WidgetField, WidgetRow, widget_fields_for,
};
use tabs::{menu_row, number_value};

/// The names a complete calibration consists of, as `stick.json` stores them.
///
/// One list, read by the window that takes the readings and the check that says what is missing, so the two
/// cannot drift into disagreeing about what a complete calibration is.
pub const CALIBRATION_NAMES: [&str; 9] = [
    "centre",
    "up",
    "down",
    "left",
    "right",
    "up-left",
    "up-right",
    "down-left",
    "down-right",
];

/// What a theme asks for, in a line, for the list: enough to tell two of them apart without opening either.
fn describe_theme(theme: &g13_applets::Theme) -> String {
    let mut parts = Vec::new();
    if let Some(speed) = theme.scanline {
        parts.push(format!("scanline {speed:.0}"));
    }
    if let Some((every, shake)) = theme.glitch {
        parts.push(format!("glitch every {every:.0}s by {shake}"));
    }
    if theme.pulse != (1.0, 1.0) {
        parts.push(format!("pulse {:.1}/{:.1}", theme.pulse.0, theme.pulse.1));
    }
    parts.push(format!("typewriter {:.0}", theme.typewriter));
    match theme.frame {
        g13_applets::ThemeFrame::None => {}
        g13_applets::ThemeFrame::Single => parts.push("boxed".to_string()),
        g13_applets::ThemeFrame::Brackets => parts.push("brackets".to_string()),
    }
    parts.join(", ")
}

/// How often a tab is redrawn.
///
/// This is the whole of the smoothness question, and it lives somewhere it can be asserted rather than as a
/// literal inside the drawing code. The stick's reading is a live view of a stick under a hand: the driver
/// publishes it up to a hundred times a second, and a window that looks once a second shows a stutter that no
/// change to the driver can fix. Sixty frames a second makes it a direct reading. The other tabs are
/// configuration, where once a second is plenty and does not hold a core while the window sits open.
pub fn repaint_interval(tab: Tab) -> std::time::Duration {
    match tab {
        // the stick's live reading lives on the Bindings tab now, beside the map, so that tab is the one that
        // redraws at frame rate
        Tab::Bindings => std::time::Duration::from_millis(16),
        _ => std::time::Duration::from_secs(1),
    }
}

/// How often the window redraws, given what it is showing.
///
/// The tab's own rate, unless the preview beside the applet being edited is showing something that **moves**: the
/// driver draws a screen with a running text on it twenty times a second, so a window that repaints once a second
/// shows one frame in twenty and the result is jitter, which is what the preview's own judder was.
///
/// The faster of the two wins, so the Bindings tab's live stick is never slowed down by this.
pub fn repaint_interval_for(window: &Window) -> std::time::Duration {
    let tab = repaint_interval(window.tab);
    let Some(name) = window.applet_editing.as_ref() else {
        return tab;
    };
    let visual = format!("applet:{name}");
    let moving = g13_agent::visuals::fast_draw_for(
        &visual,
        window.screen_editing,
        &window.config_dir.join("applets"),
    );
    match moving {
        true => tab.min(std::time::Duration::from_millis(50)),
        false => tab,
    }
}

/// Whether a driver is running, and what to do about it.
///
/// The window only edits configuration. A driver is what reads that configuration and draws on the pad, so
/// without one nothing the window does can be seen on the pad - which looks exactly like the window being
/// broken. This is read from the state a running driver publishes, so it reports the driver and not merely a
/// process with the same name (the window itself is also called `g13`).
#[derive(Debug, Clone, PartialEq)]
pub enum Driver {
    /// A driver is publishing state, and how long ago it last did.
    Running(f64),
    /// A process is running the driver but has not published yet.
    Starting,
    /// Nothing is driving the pad.
    Stopped,
}

impl Driver {
    /// The driver's state, as the one line the status bar shows.
    pub fn sentence(&self) -> String {
        match self {
            Driver::Running(seconds) => {
                format!("a driver is drawing the pad (last updated {seconds:.0}s ago)")
            }
            Driver::Starting => "a driver is starting up".to_string(),
            Driver::Stopped => {
                "no driver is drawing the pad: nothing here can be seen on it".to_string()
            }
        }
    }
}

/// Look for a running driver: the state file it publishes, and how fresh it is.
pub fn driver_state() -> Driver {
    let path = g13_agent::state_path();
    let Ok(metadata) = std::fs::metadata(&path) else {
        return Driver::Stopped;
    };
    let Ok(modified) = metadata.modified() else {
        return Driver::Stopped;
    };
    let age = match std::time::SystemTime::now().duration_since(modified) {
        Ok(age) => age.as_secs_f64(),
        Err(_) => return Driver::Running(0.0),
    };
    // the driver rewrites this several times a second; a quarter of a minute means it is gone
    if age < 5.0 {
        Driver::Running(age)
    } else {
        Driver::Stopped
    }
}

/// Start a driver, detached, so it outlives the window.
pub fn start_driver() -> Result<String, String> {
    match std::process::Command::new("sh")
        .arg("-c")
        .arg("nohup g13 run >/dev/null 2>&1 &")
        .spawn()
    {
        Ok(_) => Ok("asked for a driver; it should be drawing within a second".to_string()),
        Err(error) => Err(format!("could not start a driver: {error}")),
    }
}

/// Stop the driver, by asking the processes whose command really is `g13 run`.
pub fn stop_driver() -> Result<String, String> {
    let mut stopped = 0;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Err("cannot read /proc to find the driver".to_string());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|text| text.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(command) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let command: Vec<&str> = command
            .split(|byte| *byte == 0)
            .filter_map(|part| std::str::from_utf8(part).ok())
            .filter(|part| !part.is_empty())
            .collect();
        // exactly `g13 run`, not the window, which is also called g13
        if command.len() == 2 && command[0].ends_with("g13") && command[1] == "run" {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .spawn();
            stopped += 1;
        }
    }
    match stopped {
        0 => Err("no driver process was found".to_string()),
        count => Ok(format!("asked {count} driver process(es) to stop")),
    }
}

/// The nine readings a stick calibration consists of.
pub const CALIBRATION_POINTS: [(&str, &str); 9] = [
    ("centre", "let the stick go, resting in the middle"),
    ("up", "hold it fully up"),
    ("down", "hold it fully down"),
    ("left", "hold it fully left"),
    ("right", "hold it fully right"),
    ("up-left", "hold it up and left"),
    ("up-right", "hold it up and right"),
    ("down-left", "hold it down and left"),
    ("down-right", "hold it down and right"),
];

/// The window: every tab's state, and what each change does to the files.
pub struct Window {
    /// Which tab is showing.
    pub tab: Tab,
    /// The configuration directory every edit here reads from and writes to.
    pub config_dir: PathBuf,
    /// Every set that has a bindings file, in the order they are numbered.
    pub profiles: Vec<u32>,
    /// The set the bindings table is showing.
    pub profile: u32,
    /// What each set is called and which M key switches to it. Read with the rest of the config and written
    /// straight back, the way every other edit in this window works.
    pub profile_sets: g13_config::Profiles,
    /// The name box for a set, so typing in it does not write on every keystroke.
    pub profile_name_draft: String,
    /// The bindings table, one row per control on the pad.
    pub rows: Vec<Row>,
    /// What the map and the screens report wrong, said in the status bar.
    pub problems: Vec<String>,
    /// Which control's action is being typed, if any.
    pub editing: Option<String>,
    /// The text in the box while editing.
    pub entry: String,
    /// The screen: which visuals exist, which are on, which is showing.
    pub enabled: Vec<String>,
    /// Which visual is showing, as `visuals.json` names it.
    pub visual: String,
    /// Whether a running driver walks through the enabled visuals by itself.
    pub cycle: bool,
    /// How long each is shown when it does.
    pub cycle_seconds: f64,
    /// The last frame the worker finished drawing, shown beside the applet being edited.
    pub preview: Frame,
    /// When that frame arrived, so a preview that has stopped can be told from one that is slow.
    pub preview_at: Instant,
    /// What the last thing done said, shown under the tab bar.
    pub status: String,
    /// Whether anything is actually driving the pad, which the window cannot do itself.
    pub driver: Driver,
    /// What the driver is reporting, read from what it publishes.
    pub live: crate::live::Live,
    /// Which calibration point is being taken, or none.
    pub taking: Option<usize>,
    /// The stick's settings: the mode, the deadzone, and the calibration points - one file, one writer.
    pub stick: g13_config::stick::Settings,
    /// The endpoints applets can read, as `endpoints.json` has them right now.
    pub endpoints: BTreeMap<String, g13_sources::Endpoint>,
    /// What stopped the file being read, if anything did. A file that cannot be parsed must not look like a
    /// machine with no endpoints, or a typo reads as "nothing to configure".
    pub endpoints_problem: Option<String>,

    /// An applet file that was read and is waiting for a decision, because its name is already here.
    pub waiting_import: Option<WaitingImport>,
    /// Which applets name which endpoint, so each row can say what depends on it.
    pub endpoint_use: Vec<(String, Vec<String>)>,
    /// Endpoints an applet names that do not exist - a real fault, and one that is otherwise only visible in
    /// the renderer's own problem list.
    pub endpoint_missing: Vec<(String, Vec<String>)>,
    /// Whose token is being shown. Everything else is masked.
    pub revealing: Option<String>,
    /// What is being typed into the Values tab's add row, and why the last attempt was refused.
    pub new_value_name: String,
    /// What the new value reads, in the spelling an applet's source uses.
    pub new_value_spec: String,
    /// Why the last value could not be added, said under the row rather than swallowed.
    pub value_problem: Option<String>,

    /// The endpoint the Endpoints tab is showing the settings of.
    pub endpoint_selected: Option<String>,

    /// The name and url being typed into the add row, before there is an endpoint to put them in.
    pub new_endpoint_name: String,
    /// The url being typed for a new endpoint.
    pub new_endpoint_url: String,
    /// The applet being designed: its name, and its file as it stands. Held as the file's own JSON rather than
    /// as a parsed applet, so every key it has survives being edited.
    pub applet_editing: Option<String>,
    /// The applet being edited, held as the file's own JSON so every key survives an edit.
    pub applet_json: serde_json::Value,
    /// The theme open for editing, and its file as JSON: what the fields show is the file's own keys.
    pub theme_editing: Option<String>,
    /// The theme being edited, held as the file's own JSON the same way.
    pub theme_json: serde_json::Value,
    /// Which of the applet's screens the designer is editing, when it has more than one.
    pub screen_editing: usize,
    /// Why the applet could not be read or written, if anything.
    pub applet_problem: Option<String>,
    /// What went wrong when a theme was made, said in the row that makes them.
    pub theme_problem: Option<String>,
    /// Which widget's alert question is open in the rows, if any.
    pub alert_editing: Option<usize>,
    /// The name being typed for a new theme.
    pub new_theme_name: String,
    /// Whether the inspector is showing its `add a resource` form, opened from a widget's source picker.
    pub adding_source: Option<usize>,
    /// The name and spec being typed into the add-a-source row.
    pub new_source_name: String,
    /// What that source reads.
    pub new_source_spec: String,
    /// The macros tab: what there is, which one is open, and that one as it is being edited. The files are
    /// written as the fields change, the way the rest of this window works.
    pub macro_rows: Vec<crate::macros::MacroRow>,
    /// Which macro is open for editing, or none.
    pub macro_editing: Option<u32>,
    /// Its name, as the properties file holds it.
    pub macro_name: String,
    /// Its flat form, which is what plays when the macro has no graph.
    pub macro_steps: Vec<g13_config::MacroStep>,
    /// Its graph, when it has one.
    pub macro_graph: Option<g13_config::MacroGraph>,
    /// Why the last change to it was refused, if one was.
    pub macro_problem: Option<String>,
    /// What is being typed into the add-a-source row, before it is added.
    pub macro_source_name: String,
    /// What that source reads.
    pub macro_source_spec: String,
    /// What the last thing done on this tab said.
    pub macro_status: String,
    /// A question being added to an `if` node: which node, and the fields as they are being typed.
    pub macro_question_for: Option<String>,
    /// The control a question is to be fired by, while it is being typed.
    pub macro_question_control: String,
    /// The value it is to be compared with, while it is being typed.
    pub macro_question_value: String,
    /// What the new macro is to be called, before it has a file to hold the name.
    pub new_macro_name: String,
    /// The menu file's own JSON, read on the Menu tab and written when it is edited. Nothing means there is
    /// no menu file, which is how the built-in rotation is chosen.
    pub menu_json: Option<serde_json::Value>,
    /// Why the menu file could not be read or written, said rather than leaving an empty list.
    pub menu_problem: Option<String>,
    /// The label being typed for a new menu item.
    pub new_menu_label: String,
    /// Which published value the Values tab is showing the detail of.
    pub value_selected: Option<String>,
    /// The bitmap being drawn in the inspector: its name, its size, and its pixels.
    ///
    /// Loaded from the applet's own file when a `bitmap` widget's row is opened, and written back on every click.
    /// Drawing one rather than spelling rows of dots and hashes into a file is the whole point of this being
    /// here.
    pub bitmap_editing: Option<String>,
    /// Every frame of the picture being drawn, and which one the pixels being clicked belong to.
    pub bitmap_frames: Vec<Vec<Vec<bool>>>,
    /// One frame is a still bitmap, which is what most of them are.
    pub bitmap_frame: usize,
    /// How long each frame of a moving bitmap is shown, in milliseconds.
    pub bitmap_ms: f64,
    /// Why the picture could not be written, said rather than leaving the file alone with no explanation.
    pub bitmap_problem: Option<String>,
    /// Which widget's fields are open. One at a time, because a widget's fields are a grid and five grids at
    /// once is the wall this is replacing.
    pub widget_editing: Option<usize>,
    /// The name being typed for a new applet. An applet's name *is* its file's name, so it has to be one.
    pub new_applet_name: String,
    /// Whether the list shows the empty slots the previous stack left behind.
    pub show_empty_macros: bool,
    /// The node whose fields are being edited, chosen by clicking it on the canvas.
    pub macro_selected: Option<String>,
    /// A line being dragged from a port: which node, and which port.
    pub macro_connecting: Option<(String, String)>,
    /// What the last playback did, put there by the thread that played it.
    pub macro_played: std::sync::Arc<std::sync::Mutex<Option<crate::macros::Played>>>,
    /// The map as loaded, kept so the stick's directions can be asked for by name: a direction is not a
    /// control with a bit, so it is not one of the rows.
    pub bindings: Bindings,
    /// A capture in progress, if one is: what was pressed arrives here.
    pub capture: Option<std::sync::mpsc::Receiver<g13_device::capture::Captured>>,
    /// Renders the preview off this thread. Drawing a visual means running its sources, which can take most
    /// of a second; doing that here would stall the window exactly as it would stall the driver.
    screen: g13_agent::ScreenWorker,
}

impl Window {
    /// What this set is called: its own name, or "Profile N".
    pub fn profile_name(&self, profile: u32) -> String {
        self.profile_sets.name(profile)
    }

    /// The M key that switches to this set, if one has been given to it.
    pub fn profile_key(&self, profile: u32) -> Option<&str> {
        self.profile_sets.key(profile)
    }

    /// Make a new set, numbered after the highest one there is, and switch to it.
    ///
    /// A set is a bindings file like any other, so this only has to make an empty one - it starts unassigned,
    /// which is the point: the M keys keep their number until a set is deliberately put on one.
    pub fn new_profile(&mut self) -> u32 {
        let next = self.profiles.iter().copied().max().unwrap_or(0) + 1;
        let path = self.config_dir.join(format!("bindings-{next}.properties"));
        if !path.exists() {
            let _ = g13_files::write(
                &path,
                "# A set of your own. Give a control an action on the right, or assign this set to an M key.\n",
            );
        }
        let _ = self.profile_sets.write_in(&self.config_dir);
        self.reload_profiles();
        self.reload();
        self.select_profile(next);
        next
    }

    /// Remove a set: its file, and its name and assignment with it.
    ///
    /// The set currently in use lands on another one rather than leaving the window looking at a file that is
    /// gone. No confirmation, for the same reason removing an endpoint has none.
    pub fn delete_profile(&mut self, profile: u32) {
        if self.profiles.len() <= 1 {
            return;
        }
        let path = self
            .config_dir
            .join(format!("bindings-{profile}.properties"));
        let _ = std::fs::remove_file(&path);
        self.profile_sets.forget(profile);
        let _ = self.profile_sets.write_in(&self.config_dir);
        self.reload_profiles();
        self.reload();
        if self.profile == profile {
            let fallback = self.profiles.first().copied().unwrap_or(0);
            self.select_profile(fallback);
        }
    }

    /// Name a set, or clear the name to go back to "Profile N".
    pub fn rename_profile(&mut self, profile: u32, name: &str) {
        self.profile_sets.set_name(profile, name);
        let _ = self.profile_sets.write_in(&self.config_dir);
        self.profile_sets = g13_config::Profiles::read_in(&self.config_dir);
    }

    /// Put a set on an M key, or take it off with `None`.
    pub fn assign_profile_key(&mut self, profile: u32, key: Option<&str>) {
        match key {
            Some(key) => self.profile_sets.set_key(profile, key),
            None => self.profile_sets.clear_key(profile),
        }
        let _ = self.profile_sets.write_in(&self.config_dir);
        self.profile_sets = g13_config::Profiles::read_in(&self.config_dir);
    }

    /// Read the configuration: which profiles exist, and the bindings of the active one.
    pub fn load(config_dir: &Path) -> Self {
        let profile_sets = g13_config::Profiles::read_in(config_dir);
        let mut profiles: Vec<u32> = (0..8)
            .filter(|index| {
                config_dir
                    .join(format!("bindings-{index}.properties"))
                    .exists()
            })
            .collect();
        profiles.sort_unstable();
        if profiles.is_empty() {
            profiles.push(g13_config::read_active_profile());
        }
        let profile = g13_config::read_active_profile_in(config_dir);
        let visuals_file = config_dir.join("visuals.json");
        let visuals = visuals::load_visuals(&visuals_file);

        let mut window = Self {
            tab: Tab::Bindings,
            config_dir: config_dir.to_path_buf(),
            profiles,
            profile,
            profile_sets,
            profile_name_draft: String::new(),
            rows: Vec::new(),
            problems: Vec::new(),
            editing: None,
            theme_problem: None,
            alert_editing: None,
            theme_editing: None,
            theme_json: serde_json::Value::Null,
            new_theme_name: String::new(),
            entry: String::new(),
            enabled: visuals.enabled.clone(),
            visual: visuals.active.clone(),
            cycle: visuals.cycle,
            cycle_seconds: visuals.cycle_seconds,
            preview: Frame::new(),
            preview_at: Instant::now() - std::time::Duration::from_secs(3600),
            status: String::new(),
            driver: driver_state(),
            live: crate::live::Live::load(),
            stick: g13_config::stick::Settings::load(config_dir),
            endpoints: BTreeMap::new(),
            endpoints_problem: None,
            waiting_import: None,
            endpoint_use: Vec::new(),
            endpoint_missing: Vec::new(),
            revealing: None,
            new_value_name: String::new(),
            new_value_spec: String::new(),
            value_problem: None,
            endpoint_selected: None,
            new_endpoint_name: String::new(),
            new_endpoint_url: String::new(),
            applet_editing: None,
            applet_json: serde_json::Value::Null,
            screen_editing: 0,
            applet_problem: None,
            new_source_name: String::new(),
            adding_source: None,
            new_source_spec: String::new(),
            macro_rows: Vec::new(),
            macro_editing: None,
            macro_name: String::new(),
            macro_steps: Vec::new(),
            macro_graph: None,
            macro_problem: None,
            macro_source_name: String::new(),
            macro_source_spec: String::new(),
            macro_status: String::new(),
            macro_question_for: None,
            macro_question_control: String::new(),
            macro_question_value: String::new(),
            new_macro_name: String::new(),
            new_applet_name: String::new(),
            widget_editing: None,
            value_selected: None,
            bitmap_editing: None,
            bitmap_frames: Vec::new(),
            bitmap_frame: 0,
            bitmap_ms: 120.0,
            bitmap_problem: None,
            menu_json: None,
            menu_problem: None,
            new_menu_label: String::new(),
            show_empty_macros: false,
            macro_selected: None,
            macro_connecting: None,
            macro_played: std::sync::Arc::new(std::sync::Mutex::new(None)),
            bindings: Bindings::default(),
            capture: None,
            taking: None,
            screen: g13_agent::ScreenWorker::start(
                visuals.active.clone(),
                config_dir.join("applets"),
                config_dir.to_path_buf(),
                // a preview draws into a pane and must never take the panel
                None,
                g13_agent::ScreenWake::default(),
            ),
        };
        window.reload();
        window.reload_endpoints();
        window.reload_macros();
        window
    }

    /// Every macro there is, for the list.
    pub fn reload_macros(&mut self) {
        self.macro_rows = crate::macros::list(&self.config_dir);
    }

    /// How many macros are empty slots, so the tab can say so rather than just hiding them.
    pub fn empty_macro_count(&self) -> usize {
        self.macro_rows.iter().filter(|row| row.empty).count()
    }

    /// Move a node on the canvas. The file is written when the drag ends, not on every frame of it.
    pub fn move_macro_node(&mut self, id: &str, by: egui::Vec2, save: bool) {
        if let Some(graph) = self.macro_graph.as_mut() {
            if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) {
                node.x = (node.x + by.x).max(0.0);
                node.y = (node.y + by.y).max(0.0);
            }
        }
        if save {
            self.save_graph();
        }
    }

    /// Open a macro for editing, reading its files as they are now.
    pub fn open_macro(&mut self, id: u32) {
        match crate::macros::open(&self.config_dir, id) {
            Ok(opening) => {
                self.macro_editing = Some(id);
                self.macro_name = opening.name;
                self.macro_steps = opening.steps;
                self.macro_graph = opening.graph;
                self.macro_problem = None;
                self.macro_status.clear();
                self.macro_question_for = None;
            }
            Err(problem) => self.macro_problem = Some(problem),
        }
    }

    /// Stop editing the open macro and clear what was read for it.
    pub fn close_macro(&mut self) {
        self.macro_editing = None;
        self.macro_steps.clear();
        self.macro_graph = None;
        self.macro_problem = None;
        self.macro_status.clear();
    }

    /// Write the open macro back: its steps always, and its graph when it has one.
    pub fn save_macro(&mut self) {
        let Some(id) = self.macro_editing else {
            return;
        };
        if let Err(problem) =
            crate::macros::save_steps(&self.config_dir, id, &self.macro_name, &self.macro_steps)
        {
            self.macro_problem = Some(problem);
            return;
        }
        if let Some(graph) = self.macro_graph.clone() {
            if let Err(problem) = crate::macros::save_graph(&self.config_dir, id, &graph) {
                self.macro_problem = Some(problem);
                return;
            }
        }
        self.macro_problem = None;
        self.reload_macros();
    }

    /// Make a new macro, write it straight away, and open it.
    ///
    /// It starts empty rather than with a step in it: an empty macro plays nothing, which is the honest
    /// starting point for one that has not been given anything yet.
    pub fn new_macro(&mut self) {
        let mut id = 1u32;
        while self.macro_rows.iter().any(|row| row.id == id) {
            id += 1;
        }
        let name = if self.new_macro_name.trim().is_empty() {
            format!("macro {id}")
        } else {
            self.new_macro_name.trim().to_string()
        };
        match crate::macros::save_steps(&self.config_dir, id, &name, &[]) {
            Ok(_) => {
                self.new_macro_name.clear();
                self.reload_macros();
                self.open_macro(id);
            }
            Err(problem) => self.macro_problem = Some(problem),
        }
    }

    /// A macro's name, as typed.
    pub fn set_macro_name(&mut self, name: &str) {
        self.macro_name = name.to_string();
        self.save_macro();
    }

    /// Add a step of a kind at the end, which is where a hand-made step usually goes.
    pub fn add_macro_step(&mut self, kind: &str) {
        let step = match kind {
            "press" => g13_config::MacroStep::KeyDown(30),
            "release" => g13_config::MacroStep::KeyUp(30),
            _ => g13_config::MacroStep::Delay(50),
        };
        self.macro_steps.push(step);
        self.save_macro();
    }

    /// Take a step out by its place in the list, and write the macro back.
    pub fn remove_macro_step(&mut self, index: usize) {
        if index < self.macro_steps.len() {
            self.macro_steps.remove(index);
            self.save_macro();
        }
    }

    /// Move a step one place, swapping so nothing else about it changes.
    pub fn move_macro_step(&mut self, index: usize, by: i64) {
        let Some(target) = index.checked_add_signed(by as isize) else {
            return;
        };
        if target < self.macro_steps.len() {
            self.macro_steps.swap(index, target);
            self.save_macro();
        }
    }

    /// Change what a step does, keeping the rest of it.
    pub fn set_macro_step_kind(&mut self, index: usize, kind: &str) {
        let Some(step) = self.macro_steps.get(index).cloned() else {
            return;
        };
        let code = match step {
            g13_config::MacroStep::KeyDown(code) | g13_config::MacroStep::KeyUp(code) => code,
            g13_config::MacroStep::Delay(_) => 30,
        };
        let ms = match step {
            g13_config::MacroStep::Delay(ms) => ms,
            _ => 50,
        };
        self.macro_steps[index] = match kind {
            "press" => g13_config::MacroStep::KeyDown(code),
            "release" => g13_config::MacroStep::KeyUp(code),
            _ => g13_config::MacroStep::Delay(ms),
        };
        self.save_macro();
    }

    /// Set the key a press or release step uses, by name.
    pub fn set_macro_step_key(&mut self, index: usize, name: &str) -> Option<String> {
        let code = match g13_device::keyboard::parse_key(name) {
            Ok(code) => code.code(),
            Err(problem) => return Some(problem),
        };
        match self.macro_steps.get_mut(index) {
            Some(g13_config::MacroStep::KeyDown(existing))
            | Some(g13_config::MacroStep::KeyUp(existing)) => *existing = code,
            _ => return None,
        }
        self.save_macro();
        None
    }

    /// Set how long a wait step waits.
    pub fn set_macro_step_delay(&mut self, index: usize, ms: u32) {
        if let Some(g13_config::MacroStep::Delay(existing)) = self.macro_steps.get_mut(index) {
            *existing = ms;
            self.save_macro();
        }
    }

    /// Give this macro a graph: its steps become a chain of nodes it can then be given an `if` in.
    pub fn make_macro_decide(&mut self) {
        let Some(id) = self.macro_editing else {
            return;
        };
        let graph = crate::macros::steps_as_chain(id, &self.macro_steps, &self.macro_name);
        if let Err(problem) = crate::macros::save_graph(&self.config_dir, id, &graph) {
            self.macro_problem = Some(problem);
            return;
        }
        self.macro_graph = Some(graph);
        self.macro_problem = None;
        self.reload_macros();
    }

    /// Take the graph away, leaving the steps the previous stack reads.
    pub fn drop_macro_graph(&mut self) {
        let Some(id) = self.macro_editing else {
            return;
        };
        match crate::macros::drop_graph(&self.config_dir, id) {
            Ok(_) => {
                self.macro_graph = None;
                self.macro_problem = None;
                self.reload_macros();
            }
            Err(problem) => self.macro_problem = Some(problem),
        }
    }

    /// Write the macro being edited's graph to its file, keeping any problem for the window to show.
    fn save_graph(&mut self) {
        let Some(id) = self.macro_editing else {
            return;
        };
        let Some(graph) = self.macro_graph.clone() else {
            return;
        };
        match crate::macros::save_graph(&self.config_dir, id, &graph) {
            Ok(_) => self.macro_problem = None,
            Err(problem) => self.macro_problem = Some(problem),
        }
    }

    /// Add a source to the macro being edited.
    pub fn add_macro_source(&mut self, name: &str, spec: &str) {
        let Some(graph) = self.macro_graph.as_mut() else {
            return;
        };
        match crate::macros::add_source(graph, name, spec) {
            Ok(()) => {
                self.save_graph();
                self.macro_source_name.clear();
                self.macro_source_spec.clear();
            }
            Err(problem) => self.macro_problem = Some(problem),
        }
    }

    /// Rename a source and change what it reads, refusing what the file could not hold.
    pub fn set_macro_source(&mut self, was: &str, name: &str, spec: &str) {
        if let Some(graph) = self.macro_graph.as_mut() {
            match crate::macros::set_source(graph, was, name, spec) {
                Ok(()) => self.save_graph(),
                Err(problem) => self.macro_problem = Some(problem),
            }
        }
    }

    /// Take a source away; a question that named it is left for the player to report.
    pub fn remove_macro_source(&mut self, name: &str) {
        if let Some(graph) = self.macro_graph.as_mut() {
            crate::macros::remove_source(graph, name);
            self.save_graph();
        }
    }

    /// Add a node of a kind, joined to the last one so the walk reaches it.
    pub fn add_macro_node(&mut self, kind: &str) {
        let Some(graph) = self.macro_graph.as_mut() else {
            return;
        };
        match crate::macros::new_node(graph, kind) {
            Ok(node) => {
                let from = graph.nodes.last().map(|node| node.id.clone());
                graph.nodes.push(node.clone());
                if graph.start.is_empty() {
                    graph.start = node.id.clone();
                }
                // a new node is joined to the one before it, because a node nothing reaches never runs
                if let Some(from) = from {
                    crate::macros::set_port(graph, &from, "out", Some(&node.id));
                }
                self.save_graph();
            }
            Err(problem) => self.macro_problem = Some(problem),
        }
    }

    /// Take a node out of the graph, and write the graph back.
    pub fn remove_macro_node(&mut self, id: &str) {
        if let Some(graph) = self.macro_graph.as_mut() {
            crate::macros::remove_node(graph, id);
            self.save_graph();
        }
    }

    /// Join a port to a node, or to nothing so the macro ends down that way.
    pub fn set_macro_port(&mut self, from: &str, port: &str, to: Option<&str>) {
        if let Some(graph) = self.macro_graph.as_mut() {
            crate::macros::set_port(graph, from, port, to);
            self.save_graph();
        }
    }

    /// Set one of a node's numbered fields: a hold, a wait, a count.
    pub fn set_macro_node_number(&mut self, id: &str, field: &str, value: f64) {
        if let Some(graph) = self.macro_graph.as_mut() {
            if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) {
                crate::macros::set_node_number(&mut node.kind, field, value);
            }
            self.save_graph();
        }
    }

    /// Set a node's key by name, saying so if the name means nothing.
    pub fn set_macro_node_key(&mut self, id: &str, name: &str) -> Option<String> {
        let code = match g13_device::keyboard::parse_key(name) {
            Ok(code) => code.code(),
            Err(problem) => return Some(problem),
        };
        if let Some(graph) = self.macro_graph.as_mut() {
            if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) {
                if let g13_config::NodeKind::Key { code: existing, .. } = &mut node.kind {
                    *existing = code;
                }
            }
            self.save_graph();
        }
        None
    }

    /// Set whether a `key` node taps, holds down or releases.
    pub fn set_macro_node_mode(&mut self, id: &str, mode: g13_config::KeyMode) {
        if let Some(graph) = self.macro_graph.as_mut() {
            if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) {
                if let g13_config::NodeKind::Key { mode: existing, .. } = &mut node.kind {
                    *existing = mode;
                }
            }
            self.save_graph();
        }
    }

    /// Set a `type` node's text, or the gaps between its characters.
    pub fn set_macro_node_text(&mut self, id: &str, text: Option<&str>) {
        if let Some(graph) = self.macro_graph.as_mut() {
            if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) {
                if let g13_config::NodeKind::Type { text: existing, .. } = &mut node.kind {
                    if let Some(text) = text {
                        *existing = text.to_string();
                    }
                }
            }
            self.save_graph();
        }
    }

    /// Set what a `run` node does, refusing one that says nothing.
    ///
    /// A run with no command would save as a box that does nothing while looking like it does something, so it
    /// is refused with a reason - the same rule as a source with no spec.
    pub fn set_macro_node_spec(&mut self, id: &str, spec: &str) -> Option<String> {
        if spec.trim().is_empty() {
            return Some("a run box needs something to do, e.g. cmd:notify-send done".to_string());
        }
        if let Some(graph) = self.macro_graph.as_mut() {
            if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) {
                if let g13_config::NodeKind::Run { spec: existing } = &mut node.kind {
                    *existing = spec.trim().to_string();
                }
            }
            self.save_graph();
        }
        None
    }

    /// Replace an `if` node's condition with the form the editor built.
    pub fn set_macro_node_condition(&mut self, id: &str, condition: g13_values::Cond) {
        if let Some(graph) = self.macro_graph.as_mut() {
            if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) {
                if let g13_config::NodeKind::If { when } = &mut node.kind {
                    *when = condition;
                }
            }
            self.save_graph();
        }
    }

    /// Play the open macro, as if the pad had fired it.
    pub fn play_macro(&mut self) {
        let Some(id) = self.macro_editing else {
            return;
        };
        self.macro_status = "playing...".to_string();
        if let Ok(mut slot) = self.macro_played.lock() {
            *slot = None;
        }
        crate::macros::play_in_background(
            &self.config_dir,
            id,
            None,
            std::sync::Arc::clone(&self.macro_played),
        );
    }

    /// What the playback did, once it has finished.
    pub fn macro_playback(&self) -> Option<crate::macros::Played> {
        self.macro_played.lock().ok().and_then(|slot| slot.clone())
    }

    /// Re-scan which binding sets exist.
    ///
    /// Separate from `reload`, which re-reads the *file* for the set in hand. Adding or deleting a set changes
    /// which files there are, so the list has to be built again - without this the new set is written to disk and
    /// never appears in the row of buttons, so there is nothing to select and nothing looks lit.
    pub fn reload_profiles(&mut self) {
        let config_dir = self.config_dir.clone();
        let mut profiles: Vec<u32> = (0..8)
            .filter(|index| {
                config_dir
                    .join(format!("bindings-{index}.properties"))
                    .exists()
            })
            .collect();
        // and any numbered file beyond the first eight, because a set made here is not limited to eight
        let mut extra: Vec<u32> = std::fs::read_dir(&config_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let number = name
                            .strip_prefix("bindings-")?
                            .strip_suffix(".properties")?
                            .parse::<u32>()
                            .ok()?;
                        Some(number)
                    })
                    .collect()
            })
            .unwrap_or_default();
        extra.retain(|number| !profiles.contains(number));
        profiles.append(&mut extra);
        profiles.sort_unstable();
        profiles.dedup();
        if profiles.is_empty() {
            profiles.push(g13_config::read_active_profile());
        }
        self.profiles = profiles;
        self.profile_sets = g13_config::Profiles::read_in(&self.config_dir);
    }

    /// Re-read the bindings file for the profile in hand.
    pub fn reload(&mut self) {
        let path = self.bindings_file();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let bindings = Bindings::from_text(&text);
        self.problems = bindings.problems();
        // the map as the driver will use it: the file's bindings plus the defaults an unmentioned key gets, so
        // the effect column below says what the pad will actually do
        self.bindings = g13_agent::effective_bindings(&text, &self.bindings_file());

        self.rows = g13_device::CONTROLS
            .iter()
            .map(|control| {
                let action = g13_config::binding_for(&text, control).unwrap_or_default();
                // and_then, because a name may have no control behind it and a control may have no bit:
                // mapping one Option into the other gives an Option of an Option, which is not a bit
                let bit =
                    g13_proto::control_from_name(control).and_then(g13_proto::bit_for_control);
                let effect = if action.is_empty() {
                    // the file names nothing, but the driver may still act: an M key has a default role, and
                    // a table that says "nothing" while the pad switches profile is worse than no table
                    let by_default = bit.and_then(|bit| {
                        self.bindings
                            .switch_for(bit)
                            .map(|profile| format!("switches to profile {profile}"))
                            .or_else(|| {
                                self.bindings.record_for(bit).map(|target| match target {
                                    Some(id) => format!("records a macro into macro {id}"),
                                    None => "records a macro".to_string(),
                                })
                            })
                    });
                    match by_default {
                        Some(effect) => {
                            format!("{effect}  (default: this file names no {control})")
                        }
                        None => "nothing".to_string(),
                    }
                } else {
                    describe(&action)
                };
                // what it does when held, if it has a hold: read from the map the driver uses, so the row and
                // the pad cannot disagree about it
                let held = bit
                    .and_then(|bit| bindings.hold_for(bit))
                    .map(g13_agent::describe_action);
                Row {
                    control: control.to_string(),
                    bit,
                    action,
                    effect,
                    held,
                }
            })
            .collect();
    }

    /// Switch which profile the table is showing, and follow it as the active one.
    pub fn select_profile(&mut self, profile: u32) {
        self.profile = profile;
        if let Err(error) = g13_config::write_active_profile_in(&self.config_dir, profile) {
            self.status = format!("could not make profile {profile} active: {error}");
        } else {
            self.status = format!("profile {profile} is now active");
        }
        self.reload();
    }

    /// Start editing a control's action.
    /// Start editing what a name sends. The name is a control on the pad or a sector of the stick.
    pub fn begin_edit(&mut self, name: &str) {
        self.editing = Some(name.to_string());
        // whatever the file already says for this name, so a sector opens with its own line rather than an
        // empty box - and the rows are not consulted, because a sector has no row of its own in them
        let text = std::fs::read_to_string(self.bindings_file()).unwrap_or_default();
        self.entry = g13_config::binding_for(&text, name).unwrap_or_default();
    }

    /// Bind what is being edited to a macro, by choosing it.
    ///
    /// The same write as `set`, through the same function: a macro is a thing the window already knows the name
    /// of, so it is picked from a list rather than typed as `m,4,1`.
    pub fn bind_macro(&mut self, id: u32) {
        self.entry = format!("m,{id}");
        self.commit_edit();
    }

    /// Bind what is being edited to nothing at all, through the same write.
    pub fn bind_nothing(&mut self) {
        self.entry = "x".to_string();
        self.commit_edit();
    }

    /// What the thing being edited is bound to right now, said the way the row says it.
    pub fn binding_being_edited(&self) -> String {
        let Some(name) = self.editing.as_deref() else {
            return String::new();
        };
        let text = std::fs::read_to_string(self.bindings_file()).unwrap_or_default();
        self.bound_label(&g13_config::binding_for(&text, name).unwrap_or_default())
    }

    /// What a control is bound to, said the way a person would read it back.
    ///
    /// `p,k.16` is the file's syntax; on the row it is `q`, because which key a code is is a property of the code.
    /// A macro is its own name rather than its id, and anything else is the action as it stands.
    pub fn bound_label(&self, action: &str) -> String {
        let action = action.trim();
        if action.is_empty() {
            return "(unbound)".to_string();
        }
        if let Some(code) = action
            .strip_prefix("p,k.")
            .and_then(|number| number.parse::<u16>().ok())
        {
            return match g13_device::keyboard::name_for(code) {
                Some(name) => name.to_string(),
                None => format!("keycode {code}"),
            };
        }
        if let Some(id) = action
            .strip_prefix("m,")
            .and_then(|rest| rest.split(',').next())
            .and_then(|number| number.parse::<u32>().ok())
        {
            let named = self
                .macro_rows
                .iter()
                .find(|row| row.id == id)
                .map(|row| row.name.clone())
                .unwrap_or_default();
            return match named.is_empty() {
                true => format!("macro {id}"),
                false => named,
            };
        }
        match action {
            "x" => "nothing".to_string(),
            other => other.to_string(),
        }
    }

    /// Listen for the next press, so a control can be bound by doing it rather than by typing its name.
    ///
    /// `mb,left` is the file's syntax, not something a person should have to know. A key, a mouse button and a
    /// gamepad button are all just codes, and which action a code means is a property of the code.
    pub fn start_capture(&mut self) {
        if self.capture.is_some() {
            return;
        }
        self.capture = Some(g13_device::capture::watch());
        let control = self.editing.clone().unwrap_or_default();
        self.status = format!("listening: press what {control} should send");
    }

    /// Take whatever was pressed, if anything has been.
    ///
    /// Applied straight away and the box closed, because the press *is* the decision: a step to confirm it
    /// would be a button nobody wants to press twice.
    pub fn poll_capture(&mut self) {
        let Some(receiver) = &self.capture else {
            return;
        };
        match receiver.try_recv() {
            Ok(g13_device::capture::Captured::Input { device, press }) => {
                self.capture = None;
                let action = match g13_device::capture::action_or_reason(press) {
                    Ok(action) => action,
                    Err(reason) => {
                        self.status = reason;
                        return;
                    }
                };
                let Some(control) = self.editing.clone() else {
                    self.status = format!("{action} was pressed, but no control is being set");
                    return;
                };
                self.entry = action.clone();
                match self.apply(&control, &action) {
                    Ok(()) => {
                        self.editing = None;
                        self.status =
                            format!("{control} now sends {action}  (caught from \"{device}\")");
                    }
                    Err(problem) => self.status = problem,
                }
                self.reload();
            }
            Ok(g13_device::capture::Captured::Nothing(reason)) => {
                self.capture = None;
                self.status = reason;
            }
            Ok(g13_device::capture::Captured::Watching { devices, .. }) => {
                self.status = format!("listening on {devices} device(s) - press something");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.capture = None;
                self.status = "the listening stopped without anything being pressed".to_string();
            }
        }
    }

    /// Commit what is in the box, straight to the file.
    pub fn commit_edit(&mut self) {
        let Some(control) = self.editing.clone() else {
            return;
        };
        let action = self.entry.trim().to_string();
        match self.apply(&control, &action) {
            Ok(()) => {
                self.status = if action.is_empty() {
                    format!(
                        "{control} now sends nothing  (profile {}, {})",
                        self.profile,
                        self.bindings_file().display()
                    )
                } else {
                    format!(
                        "{control} now sends {action}  (profile {}, {})",
                        self.profile,
                        self.bindings_file().display()
                    )
                };
            }
            Err(problem) => self.status = problem,
        }
        self.editing = None;
        self.reload();
    }

    /// Stop listening for a press, and leave the binding as it was.
    pub fn cancel_edit(&mut self) {
        self.capture = None;
        self.editing = None;
    }

    /// The bindings file for the profile the table is showing.
    ///
    /// Built from this window's own configuration directory rather than from a global lookup, so a window
    /// pointed at one directory cannot write into another.
    pub fn bindings_file(&self) -> PathBuf {
        g13_config::bindings_path_in(&self.config_dir, self.profile)
    }

    /// The colour this profile asks for, and whether it asks for one at all.
    ///
    /// `None` is not black: it is a file with no `color=` line, and the screen then keeps whatever it was last
    /// told. The window says which of the two it is rather than showing a swatch that means nothing.
    pub fn binding_colour(&self) -> Option<g13_config::Colour> {
        let text = std::fs::read_to_string(self.bindings_file()).unwrap_or_default();
        g13_config::colour_in(&text)
    }

    /// Set this profile's colour, keeping every other line of the file as it was.
    pub fn set_binding_colour(&mut self, colour: g13_config::Colour) -> Result<(), String> {
        let path = self.bindings_file();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let updated = g13_config::set_colour(&text, colour);
        g13_files::write(&path, &updated)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        // read it back rather than assume: a value already there is not a failure, and reporting one as a
        // failure is how the bindings table once looked unwritable
        match self.binding_colour() {
            Some(now) if now == colour => Ok(()),
            Some(now) => Err(format!("the file says {} now", now.text())),
            None => Err(format!("no colour in {} after writing", path.display())),
        }
    }

    /// Write one binding for the profile the table is showing, keeping every other line as it was.
    ///
    /// The file is decided by `self.profile`, not by whichever profile happens to be active: writing to the
    /// active one while showing another is how an edit lands somewhere the table is not looking, and then
    /// appears to revert when the table is re-read.
    pub fn apply(&self, control: &str, action: &str) -> Result<(), String> {
        let path = self.bindings_file();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let written = normalise(action);
        let updated = g13_config::set_binding(&text, control, &written);
        g13_files::write(&path, &updated)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        // say what the file holds now, rather than inferring from a comparison whether it worked: a value
        // that was already there is not a failure, and reporting it as one is how JUP looked unwritable
        let now = std::fs::read_to_string(&path).unwrap_or_default();
        match g13_config::binding_for(&now, control) {
            Some(found) if found == written => Ok(()),
            Some(found) => Err(format!("{control} is {found} in {}", path.display())),
            None => Err(format!(
                "{control} is not in {} after writing",
                path.display()
            )),
        }
    }

    /// Choose which visual the driver is showing, and write it.
    pub fn select_visual(&mut self, visual: &str) {
        self.visual = visual.to_string();
        if let Some(position) = self.enabled.iter().position(|item| item == visual) {
            let _ = position;
        } else {
            self.enabled.push(visual.to_string());
        }
        self.screen.show(visual);
        match self.save_visuals() {
            Ok(()) => self.status = format!("the screen now shows {visual}"),
            Err(problem) => self.status = problem,
        }
    }

    /// The name the walk knows an applet by: `clock` for the four that used to be built in, `applet:weather` for
    /// the rest. One place says which, so the tick here and the walk cannot disagree.
    pub fn visual_name_for(&self, applet: &str) -> Option<String> {
        self.available_visuals()
            .into_iter()
            .find(|visual| visual == applet || visual.strip_prefix("applet:") == Some(applet))
    }

    /// Turn a visual on or off in the cycle.
    pub fn toggle_enabled(&mut self, visual: &str) {
        match self.enabled.iter().position(|item| item == visual) {
            Some(position) => {
                self.enabled.remove(position);
            }
            None => self.enabled.push(visual.to_string()),
        }
        match self.save_visuals() {
            Ok(()) => {
                self.status = format!(
                    "{visual} {}",
                    if self.enabled.contains(&visual.to_string()) {
                        "enabled"
                    } else {
                        "disabled"
                    }
                )
            }
            Err(problem) => self.status = problem,
        }
    }

    /// Write `visuals.json`: which visuals are on, which is showing, and whether they cycle.
    pub fn save_visuals(&self) -> Result<(), String> {
        let path = self.config_dir.join("visuals.json");
        let text = format!(
            "{{\n  \"enabled\": [{}],\n  \"active\": \"{}\",\n  \"cycle\": {},\n  \"cycle_seconds\": {}\n}}\n",
            self.enabled
                .iter()
                .map(|item| format!("\"{item}\""))
                .collect::<Vec<_>>()
                .join(", "),
            self.visual,
            self.cycle,
            self.cycle_seconds
        );
        g13_files::write(&path, &text)
            .map_err(|error| format!("could not write {}: {error}", path.display()))
    }

    /// Read what the driver is publishing. Cheap, so it happens on every repaint.
    pub fn refresh_live(&mut self) {
        self.live = crate::live::Live::load();
    }

    /// Take whatever the renderer has finished, if anything.
    ///
    /// No rendering happens here: the worker draws on the visual's own interval and this only collects it,
    /// so a slow source cannot hold up the window however often it repaints.
    pub fn refresh_preview(&mut self) {
        if let Some(frame) = self.screen.take() {
            self.preview = frame;
            self.problems = self.screen.problems();
        }
    }

    /// Where the fonts live: beside the applets, as the themes are.
    pub fn fonts_dir(&self) -> PathBuf {
        g13_agent::visuals::fonts_dir(&self.config_dir.join("applets"))
    }

    /// Every font in the folder, by name, for the dropdown.
    ///
    /// Read through the same cache the driver uses, because a dropdown asks this on every frame the tab is open and
    /// parsing every glyph of every font sixty times a second would be absurd.
    pub fn fonts(&self) -> Vec<String> {
        g13_applets::fonts_cached(&self.fonts_dir())
            .0
            .keys()
            .cloned()
            .collect()
    }

    /// Every theme, with what it asks for, for the list. A theme that will not read is listed with the reason:
    /// a theme that silently does nothing looks exactly like a theme that works and is subtle.
    pub fn theme_rows(&self) -> Vec<(String, Result<String, String>)> {
        g13_applets::theme::themes(&self.themes_dir())
            .into_iter()
            .map(|(name, read)| {
                let summary = match &read {
                    Ok(theme) => describe_theme(theme),
                    Err(problem) => format!("will not read: {problem}"),
                };
                (name, Ok(summary))
            })
            .collect()
    }

    /// Open a theme for editing: its file as JSON, so the fields shown are the file's own keys.
    pub fn open_theme(&mut self, name: &str) {
        self.theme_editing = Some(name.to_string());
        self.theme_problem = None;
        let path = self.themes_dir().join(format!("{name}.json"));
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(value) => self.theme_json = value,
                Err(problem) => {
                    // a theme that will not read is not written over: that is the evidence of what is wrong
                    self.theme_json = serde_json::Value::Null;
                    self.theme_problem = Some(format!("{name}.json will not read: {problem}"));
                }
            },
            Err(error) => {
                self.theme_json = serde_json::Value::Null;
                self.theme_problem = Some(format!("cannot read {}: {error}", path.display()));
            }
        }
    }

    /// One of the open theme's numbers.
    pub fn theme_number(&self, key: &str) -> f64 {
        self.theme_json
            .get(key)
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0)
    }

    /// One of the open theme's settings, as a word.
    pub fn theme_word(&self, key: &str) -> String {
        self.theme_json
            .get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    }

    /// The same, with the number the format uses when the key is not there at all.
    pub fn theme_inner_number_or(&self, key: &str, inner: &str, otherwise: f64) -> f64 {
        match self.theme_inner_number(key, inner) {
            0.0 => otherwise,
            held => held,
        }
    }

    /// A number nested one deep, for `glitch.every` and the like.
    pub fn theme_inner_number(&self, key: &str, inner: &str) -> f64 {
        self.theme_json
            .get(key)
            .and_then(|value| value.get(inner))
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0)
    }

    /// Set one of the open theme's settings, and write the file at once. No confirm buttons: a theme is a file
    /// the driver re-reads as it draws, so a change that is written is a change that is on the pad.
    pub fn set_theme(&mut self, key: &str, value: serde_json::Value) {
        let Some(object) = self.theme_json.as_object_mut() else {
            self.theme_problem =
                Some("this theme will not read, so nothing was written".to_string());
            return;
        };
        if value.is_null() {
            object.remove(key);
        } else {
            object.insert(key.to_string(), value);
        }
        self.write_theme();
    }

    /// The same, one level down, for `glitch`: the key holds an object of its own.
    pub fn set_theme_inner(&mut self, key: &str, inner: &str, value: serde_json::Value) {
        let Some(object) = self.theme_json.as_object_mut() else {
            self.theme_problem =
                Some("this theme will not read, so nothing was written".to_string());
            return;
        };
        let entry = object
            .entry(key.to_string())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(settings) = entry.as_object_mut() {
            if value.is_null() {
                settings.remove(inner);
            } else {
                settings.insert(inner.to_string(), value);
            }
        }
        self.write_theme();
    }

    /// Write the open theme's file, saying why if it cannot be.
    fn write_theme(&mut self) {
        let Some(name) = self.theme_editing.clone() else {
            return;
        };
        let path = self.themes_dir().join(format!("{name}.json"));
        let text = match serde_json::to_string_pretty(&self.theme_json) {
            Ok(text) => format!("{text}\n"),
            Err(problem) => {
                self.theme_problem = Some(format!("this theme cannot be written: {problem}"));
                return;
            }
        };
        // read back before believing it: a theme the driver cannot load is worse than no change at all
        match g13_applets::Theme::from_json(&text, &name) {
            Ok(_) => match g13_files::write(&path, &text) {
                Ok(()) => self.theme_problem = None,
                Err(error) => {
                    self.theme_problem = Some(format!("cannot write {}: {error}", path.display()))
                }
            },
            Err(problem) => {
                self.theme_problem = Some(format!("that would not read back: {problem}"))
            }
        }
    }

    /// The themes in the folder, by name, the ones that read.
    ///
    /// A theme that will not read is not offered: choosing it would write a name that does nothing. The complaint
    /// is already on the applet that names one, which is where a person will see it.
    pub fn themes(&self) -> Vec<String> {
        g13_applets::theme::themes(&self.themes_dir())
            .into_iter()
            .filter_map(|(name, read)| match read {
                Ok(_) => Some(name),
                Err(_) => None,
            })
            .collect()
    }

    /// Where the themes live: beside the applets, which is where the driver looks.
    pub fn themes_dir(&self) -> PathBuf {
        g13_agent::visuals::themes_dir(&self.config_dir.join("applets"))
    }

    /// Write a theme, so one can be made without leaving the window.
    ///
    /// The file starts with every effect it can have, at a setting you can see, rather than empty: an empty theme
    /// and no theme look exactly the same, and this one should show what a theme is for. The name is the file's
    /// name, and the same rules as a new applet apply - no path separators, and a name already taken is refused
    /// rather than written over.
    pub fn new_theme(&mut self, name: &str) {
        let name = name.trim();
        self.theme_problem = None;
        if name.is_empty() {
            self.theme_problem =
                Some("a theme needs a name: it is also its file's name".to_string());
            return;
        }
        if let Some(bad) = name
            .chars()
            .find(|c| !(c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_'))
        {
            self.theme_problem = Some(format!(
                "a theme's name cannot have {bad:?} in it: letters, digits, spaces, - and _ only"
            ));
            return;
        }
        let dir = self.themes_dir();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.theme_problem = Some(format!("cannot make {}: {error}", dir.display()));
            return;
        }
        let path = dir.join(format!("{name}.json"));
        if path.exists() {
            self.theme_problem = Some(format!("there is already a theme called {name}"));
            return;
        }
        let text = r#"{
  "frame": "none",
  "scanline": 40,
  "glitch": {"every": 6, "shake": 1},
  "pulse": {"on": 0.5, "off": 0.5},
  "typewriter": 12
}"#;
        match g13_files::write(&path, text) {
            Ok(()) => {
                // applied straight away: the window has no confirm buttons, and a theme nobody can see is a
                // theme that looks like it did nothing
                self.set_applet_text("theme", name);
            }
            Err(error) => {
                self.theme_problem = Some(format!("cannot write {}: {error}", path.display()))
            }
        }
    }

    /// Where the endpoints live.
    pub fn endpoints_path(&self) -> PathBuf {
        self.config_dir.join("endpoints.json")
    }

    /// Read `endpoints.json` again, and work out which applets need what.
    ///
    /// A file that cannot be read is kept as a problem rather than as an empty table: an empty table is what a
    /// machine with no endpoints looks like, so a typo would be indistinguishable from nothing to do.
    pub fn reload_endpoints(&mut self) {
        match g13_sources::load_endpoints(&self.endpoints_path()) {
            Ok(endpoints) => {
                self.endpoints = endpoints;
                self.endpoints_problem = None;
            }
            Err(problem) => {
                self.endpoints.clear();
                self.endpoints_problem = Some(problem);
            }
        }
        self.recount_endpoint_use();
    }

    /// Which applets name which endpoint, and which names no endpoint has.
    ///
    /// Read from the applets themselves rather than kept as a list: the applet files are the truth, and a
    /// hand-kept list would be wrong the moment an applet was edited.
    fn recount_endpoint_use(&mut self) {
        let mut used: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut missing: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Ok(entries) = std::fs::read_dir(self.config_dir.join("applets")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !g13_applets::is_applet_file(&path) {
                    continue;
                }
                let Some(applet_name) = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                else {
                    continue;
                };
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(applet) = g13_applets::parse_applet(&text) else {
                    continue;
                };
                for spec_text in applet.sources.values() {
                    // bound to a name of its own: the endpoint is a borrow from the parsed spec
                    let spec = g13_sources::Spec::parse(spec_text);
                    let Some(endpoint) = spec.endpoint() else {
                        continue;
                    };
                    let into = if self.endpoints.contains_key(endpoint) {
                        &mut used
                    } else {
                        &mut missing
                    };
                    let list = into.entry(endpoint.to_string()).or_default();
                    if !list.contains(&applet_name) {
                        list.push(applet_name.clone());
                    }
                }
            }
        }
        for list in used.values_mut().chain(missing.values_mut()) {
            list.sort();
        }
        self.endpoint_use = used.into_iter().collect();
        self.endpoint_missing = missing.into_iter().collect();
    }

    /// Write the endpoint table as it now stands.
    ///
    /// Called the moment a field changes - there is no Apply button anywhere in this window, and a running
    /// driver or a redrawing screen should pick the change up rather than wait to be told.
    pub fn commit_endpoints(&mut self) {
        match g13_sources::save_endpoints(&self.endpoints_path(), &self.endpoints) {
            Ok(()) => {
                self.endpoints_problem = None;
                self.status = format!("endpoints written to {}", self.endpoints_path().display());
            }
            Err(problem) => {
                self.status = problem.clone();
                self.endpoints_problem = Some(problem);
            }
        }
        self.recount_endpoint_use();
    }

    /// Add an endpoint, named and pointed at a url. Refuses a name that is already taken rather than
    /// overwriting one, because the name is what every applet refers to it by.
    pub fn add_endpoint(&mut self, name: &str, url: &str) {
        let name = name.trim();
        let url = url.trim();
        if name.is_empty() || url.is_empty() {
            self.status = "an endpoint needs a name and a url".to_string();
            return;
        }
        if self.endpoints.contains_key(name) {
            self.status = format!("there is already an endpoint called {name}");
            return;
        }
        self.endpoints.insert(
            name.to_string(),
            g13_sources::Endpoint {
                url: url.to_string(),
                ..Default::default()
            },
        );
        self.commit_endpoints();
    }

    /// Remove an endpoint. Anything naming it will say so afterwards, in the list of names nothing answers to.
    /// Delete an applet, and the pictures that belong to it.
    ///
    /// A `<name>.bitmaps.json` is a sidecar, not an applet - it is that applet's own pictures and nothing else reads
    /// them - so deleting the applet deletes them too rather than leaving orphans in the folder. Like an endpoint's
    /// `remove`, there is no confirmation: everything in this window writes as it is changed, and a confirm button
    /// here would be the only one in the program.
    pub fn remove_applet(&mut self, name: &str) {
        let _ = std::fs::remove_file(self.applets_dir().join(format!("{name}.json")));
        let _ = std::fs::remove_file(self.applets_dir().join(format!("{name}.bitmaps.json")));
        if self.applet_editing.as_deref() == Some(name) {
            self.close_applet();
        }
    }

    /// Read an applet in from a file.
    ///
    /// A name that is free lands straight away. A name that is already here is **not** written over
    /// and not silently refused either: what is different is listed on the window and you choose
    /// between replacing it, keeping yours, or bringing it in as a copy under the next free name.
    pub fn import_applet(&mut self) {
        let chosen = rfd::FileDialog::new()
            .set_title("import an applet")
            .add_filter("applet files", &["json"])
            .pick_file();
        let Some(path) = chosen else {
            self.status = "import cancelled".to_string();
            return;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(problem) => {
                self.status = format!("cannot read {}: {problem}", path.display());
                return;
            }
        };
        let bundle = match crate::bundle::parse(&text) {
            Ok(bundle) => bundle,
            Err(problem) => {
                self.status = format!("{}: {problem}", path.display());
                return;
            }
        };
        if crate::bundle::name_is_free(&self.config_dir, &bundle.name) {
            let name = bundle.name.clone();
            self.land_applet(bundle, &name);
            return;
        }
        let lines = crate::bundle::compare(&self.config_dir, &bundle);
        let copy_name = crate::bundle::a_free_name(&self.config_dir, &bundle.name);
        self.status = format!(
            "{} is already here - {} difference(s) to look at",
            bundle.name,
            lines.len()
        );
        self.waiting_import = Some(WaitingImport {
            bundle,
            lines,
            copy_name,
        });
    }

    /// Put a bundle in under this name, and say what was written.
    pub fn land_applet(&mut self, bundle: crate::bundle::Bundle, name: &str) {
        match crate::bundle::apply(&self.config_dir, &bundle, name) {
            Ok(wrote) => {
                let mut wrote = wrote;
                if bundle.on_the_pad {
                    let visual = format!("applet:{name}");
                    if !self.enabled.contains(&visual) {
                        self.enabled.push(visual.clone());
                        match self.save_visuals() {
                            Ok(()) => wrote.push(format!("{visual} on the pad")),
                            Err(problem) => wrote.push(problem),
                        }
                    }
                }
                self.reload_endpoints();
                self.status = format!("imported {name}: {}", wrote.join(", "));
            }
            Err(problem) => self.status = format!("cannot import {name}: {problem}"),
        }
    }

    /// Save this applet as one file: itself, its pictures, its font, its theme, the endpoints its
    /// resources use with the token slot empty, and whether the pad walks to it.
    ///
    /// The dialog is the blocking kind, so the window stands still while it is open - which is what
    /// a file dialog does on every other program too.
    pub fn export_applet(&mut self, name: &str) {
        let bundle = match crate::bundle::build(&self.config_dir, name) {
            Ok(bundle) => bundle,
            Err(problem) => {
                self.status = format!("cannot export {name}: {problem}");
                return;
            }
        };
        let chosen = rfd::FileDialog::new()
            .set_title(format!("export {name}"))
            .set_file_name(format!("{name}.g13applet.json"))
            .save_file();
        let Some(path) = chosen else {
            self.status = format!("export of {name} cancelled");
            return;
        };
        self.status = match g13_files::write(&path, &crate::bundle::to_json(&bundle)) {
            Ok(()) => format!(
                "exported {name} to {} - {} picture(s), {} font(s), {} endpoint(s)",
                path.display(),
                bundle.pictures.is_some() as u8,
                bundle.fonts.len(),
                bundle.endpoints.len()
            ),
            Err(error) => format!("cannot write {}: {error}", path.display()),
        };
    }

    /// Take an endpoint away and write the file back; an applet naming it is reported, not changed.
    pub fn remove_endpoint(&mut self, name: &str) {
        self.endpoints.remove(name);
        if self.revealing.as_deref() == Some(name) {
            self.revealing = None;
        }
        self.commit_endpoints();
    }

    /// Where the values the user named live.
    pub fn values_file(&self) -> PathBuf {
        self.config_dir.join("values.json")
    }

    /// The values the user named, read straight off disk: a name to a spec.
    ///
    /// Read rather than cached, because the window is not the driver and this file is tiny; the *resolving* is what
    /// costs, and the window never resolves one.
    pub fn custom_values(&self) -> BTreeMap<String, String> {
        g13_sources::load_custom_values(&self.values_file()).unwrap_or_default()
    }

    /// Set one, or take it out again with an empty spec. Written at once, like everything else here.
    pub fn set_custom_value(&mut self, name: &str, spec: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let mut values = self.custom_values();
        if spec.trim().is_empty() {
            values.remove(name);
        } else {
            values.insert(name.to_string(), spec.trim().to_string());
        }
        self.write_custom_values(&values);
    }

    /// Add one, refusing a name already taken rather than writing over it.
    ///
    /// Two refusals, both of them this project's own rules: a name this build already publishes (every applet reads
    /// those, and taking one would change the meaning of applets the user did not touch), and a name the user
    /// already has. A refusal is said, never done quietly.
    pub fn add_custom_value(&mut self, name: &str, spec: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a value needs a name".to_string());
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "`{name}` cannot be a name: letters, digits, `_` and `-` only"
            ));
        }
        if g13_sources::published_names().contains(&name) {
            return Err(format!(
                "`{name}` is a name this build already publishes - call yours something else"
            ));
        }
        if self.custom_values().contains_key(name) {
            return Err(format!("there is already a value called `{name}`"));
        }
        let mut values = self.custom_values();
        values.insert(name.to_string(), spec.trim().to_string());
        self.write_custom_values(&values);
        Ok(())
    }

    /// Write the user's own named values into `values.json`, which is where an applet reads them from.
    fn write_custom_values(&mut self, values: &BTreeMap<String, String>) {
        let text = serde_json::to_string_pretty(values).unwrap_or_default();
        let _ = g13_files::write(&self.values_file(), &format!("{text}\n"));
    }

    /// Every value the Values page shows: the user's own named values first, then everything this build publishes.
    ///
    /// The list is `PUBLISHED` in the sources crate - one catalogue, read here and by the resolver - so what the
    /// page offers is exactly what an applet can use. "used by" is read out of the applet files, so a value
    /// nothing reads says so instead of looking important.
    pub fn all_value_rows(&self) -> Vec<ValueRow> {
        let mut rows: Vec<ValueRow> = self
            .custom_values()
            .into_iter()
            .map(|(name, spec)| ValueRow {
                used_by: self.used_by(&name),
                name,
                meaning: spec,
                computed: false,
                now: String::new(),
                mine: true,
            })
            .collect();
        rows.extend(self.value_rows());
        rows
    }

    /// The applets that name this value, for the Values page's "used by" column.
    fn used_by(&self, name: &str) -> Vec<String> {
        self.applet_uses().remove(name).unwrap_or_default()
    }

    /// Which applets name which value, read out of the applet files themselves.
    ///
    /// Every name an applet mentions, published by this build or named by the user. The scan asks one question -
    /// what does this applet ask for - and filtering it by the catalogue is why a value the user named showed as
    /// read by nobody, which is the one thing the "used by" column is for.
    fn applet_uses(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        let mut used: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for applet_name in g13_applets::applet_names(&self.applets_dir()) {
            let path = self.applets_dir().join(format!("{applet_name}.json"));
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(applet) = g13_applets::parse_applet(&text) else {
                continue;
            };
            let mut wanted: Vec<String> = applet.sources.keys().cloned().collect();
            // and what its own sources *point at*: an applet reads a user's value by writing it with no colon,
            // `"spot": "where"`, so a name that only ever appears on the right of a source is still one in use.
            for spec in applet.sources.values() {
                let spec = spec.trim();
                if !spec.is_empty() && !spec.contains(':') && !wanted.contains(&spec.to_string()) {
                    wanted.push(spec.to_string());
                }
            }
            for widget in &applet.widgets {
                let names = match widget {
                    g13_applets::Widget::Text { format, .. } => {
                        g13_sources::names_in_format(format)
                    }
                    g13_applets::Widget::Bar { source, .. }
                    | g13_applets::Widget::Segments { source, .. }
                    | g13_applets::Widget::Arrow { source, .. } => vec![source.clone()],
                    _ => Vec::new(),
                };
                for name in names {
                    if !wanted.contains(&name) {
                        wanted.push(name);
                    }
                }
            }
            for name in wanted {
                let list = used.entry(name).or_default();
                if !list.contains(&applet.name) {
                    list.push(applet.name.clone());
                }
            }
        }
        used
    }

    /// Every value this build publishes, ready to be shown.
    pub fn value_rows(&self) -> Vec<ValueRow> {
        let used = self.applet_uses();
        g13_sources::PUBLISHED
            .iter()
            .map(|value| ValueRow {
                name: value.name.to_string(),
                meaning: value.meaning.to_string(),
                computed: value.computed,
                mine: false,
                now: {
                    let raw = self
                        .live
                        .values
                        .get(value.name)
                        .cloned()
                        .or_else(|| {
                            self.live
                                .numbers
                                .get(value.name)
                                .map(|number| number.to_string())
                        })
                        .unwrap_or_default();
                    // a page is read by a person: `51.768488745980704` is noise in a column meant to explain.
                    // The exact value is in the file for anything that wants it.
                    match raw.parse::<f64>() {
                        Ok(number) => format!("{number:.2}"),
                        Err(_) => raw,
                    }
                },
                used_by: used.get(value.name).cloned().unwrap_or_default(),
            })
            .collect()
    }

    /// The array of widgets the designer is editing: the applet's own, or one of its screens'.
    ///
    /// One place, because the designer addresses the file's JSON and every field it writes has to land in the
    /// same array it read. An applet with no `screens` is edited exactly as it was before they existed.
    fn widget_array(&self) -> Option<&Vec<serde_json::Value>> {
        if let Some(screens) = self.applet_json.get("screens").and_then(|s| s.as_array()) {
            return screens
                .get(self.screen_editing)
                .and_then(|screen| screen.get("widgets"))
                .and_then(|widgets| widgets.as_array());
        }
        self.applet_json.get("widgets").and_then(|w| w.as_array())
    }

    /// The array `widget_array` reads, to write into: the same widgets, in the same place.
    fn widget_array_mut(&mut self) -> Option<&mut Vec<serde_json::Value>> {
        let screen = self.screen_editing;
        if self
            .applet_json
            .get("screens")
            .and_then(|s| s.as_array())
            .is_some()
        {
            return self
                .applet_json
                .get_mut("screens")
                .and_then(|s| s.as_array_mut())
                .and_then(|screens| screens.get_mut(screen))
                .and_then(|screen| screen.get_mut("widgets"))
                .and_then(|widgets| widgets.as_array_mut());
        }
        self.applet_json
            .get_mut("widgets")
            .and_then(|w| w.as_array_mut())
    }

    /// How many screens the applet being designed has. One when it has none, which is the shape it was born in.
    pub fn screen_count(&self) -> usize {
        match self.applet_json.get("screens").and_then(|s| s.as_array()) {
            Some(screens) if !screens.is_empty() => screens.len(),
            _ => 1,
        }
    }

    /// The title the file gives a screen, exactly as it is there. Empty when there is none.
    pub fn screen_title(&self, index: usize) -> String {
        self.applet_json
            .get("screens")
            .and_then(|s| s.as_array())
            .and_then(|screens| screens.get(index))
            .and_then(|screen| screen.get("title"))
            .and_then(|title| title.as_str())
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// What a screen is called in the designer: its title, or which one it is when it has none.
    pub fn screen_label(&self, index: usize) -> String {
        let title = self.screen_title(index);
        if title.is_empty() {
            format!("screen {}", index + 1)
        } else {
            title
        }
    }

    /// Turn a one-screen applet into a two-screen one, keeping what it has.
    ///
    /// The widgets move into the first screen rather than being copied, so nothing is drawn twice and nothing is
    /// lost: this is the one edit that changes a file's shape, and it says so where it is used.
    pub fn add_screen(&mut self) {
        let mut screens = match self.applet_json.get("screens").and_then(|s| s.as_array()) {
            Some(screens) if !screens.is_empty() => screens.clone(),
            _ => {
                let existing = self
                    .applet_json
                    .get("widgets")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
                self.applet_json
                    .as_object_mut()
                    .map(|object| object.remove("widgets"));
                vec![serde_json::json!({"title": "screen 1", "widgets": existing})]
            }
        };
        let next = screens.len() + 1;
        screens.push(serde_json::json!({"title": format!("screen {next}"), "widgets": []}));
        if let Some(object) = self.applet_json.as_object_mut() {
            object.insert("screens".to_string(), serde_json::Value::Array(screens));
        }
        self.screen_editing = next - 1;
        self.commit_applet();
        self.screen.show_screen(self.screen_editing);
    }

    /// Take a screen away. The last one cannot go: an applet with no screens is a shape this build does not draw.
    pub fn remove_screen(&mut self) {
        if self.screen_count() < 2 {
            return;
        }
        let Some(screens) = self
            .applet_json
            .get_mut("screens")
            .and_then(|s| s.as_array_mut())
        else {
            return;
        };
        let at = self.screen_editing.min(screens.len() - 1);
        screens.remove(at);
        if screens.len() == 1 {
            // back to the shape the file had before screens existed, so a one-screen applet is one screen again
            let only = screens.remove(0);
            let widgets = only
                .get("widgets")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
            if let Some(object) = self.applet_json.as_object_mut() {
                object.remove("screens");
                object.insert("widgets".to_string(), widgets);
            }
        }
        self.screen_editing = self
            .screen_editing
            .min(self.screen_count().saturating_sub(1));
        self.commit_applet();
    }

    /// Show one of the applet's screens in the designer, and in the preview beside it.
    ///
    /// The preview is a screen worker like the driver's, so it has to be told the screen the same way the driver
    /// tells it - by the status it draws from. Setting the number and drawing that visual alone left the preview
    /// on screen 1 for every applet, which made every screen but the first impossible to check by eye.
    pub fn select_screen(&mut self, index: usize) {
        self.screen_editing = index;
        self.screen.show_screen(index);
        self.status = format!("editing screen {} of this applet", index + 1);
    }

    /// Name one of the applet's screens.
    pub fn set_screen_title(&mut self, index: usize, title: &str) {
        let Some(screen) = self
            .applet_json
            .get_mut("screens")
            .and_then(|s| s.as_array_mut())
            .and_then(|screens| screens.get_mut(index))
            .and_then(|screen| screen.as_object_mut())
        else {
            return;
        };
        let title = title.trim();
        if title.is_empty() {
            screen.remove("title");
        } else {
            screen.insert(
                "title".to_string(),
                serde_json::Value::String(title.to_string()),
            );
        }
        self.commit_applet();
    }

    /// The applet's widgets, each with the fields its kind has.
    pub fn applet_widgets(&self) -> Vec<WidgetRow> {
        let sources = self.applet_sources();
        let Some(widgets) = self.widget_array() else {
            return Vec::new();
        };
        widgets
            .iter()
            .enumerate()
            .map(|(index, widget)| {
                let kind = widget
                    .get("type")
                    .and_then(|kind| kind.as_str())
                    .unwrap_or_default()
                    .to_string();
                let mut wanted = Vec::new();
                if let Some(format) = widget.get("format").and_then(|format| format.as_str()) {
                    wanted.extend(g13_sources::names_in_format(format));
                }
                // a bar, a line, an arrow and a segment each read one value by name
                if let Some(source) = widget.get("source").and_then(|source| source.as_str()) {
                    let source = source.trim();
                    if !source.is_empty() && !wanted.iter().any(|name| name == source) {
                        wanted.push(source.to_string());
                    }
                }
                let reads = wanted
                    .into_iter()
                    .map(|name| {
                        let origin = if sources.iter().any(|(known, _)| known == &name) {
                            Origin::Declared
                        } else {
                            Origin::Missing
                        };
                        ValueRead { name, origin }
                    })
                    .collect();
                let keys = widget_fields_for(&kind);
                let fields = keys
                    .iter()
                    .map(|(key, holds)| {
                        let value = match holds {
                            FieldKind::Number => FieldValue::Number(
                                widget.get(*key).and_then(|v| v.as_f64()).unwrap_or(0.0),
                            ),
                            FieldKind::OptionalNumber => FieldValue::OptionalNumber(
                                widget.get(*key).and_then(|v| v.as_f64()),
                            ),
                            FieldKind::Tick => FieldValue::Tick(
                                widget.get(*key).and_then(|v| v.as_bool()).unwrap_or(false),
                            ),
                            FieldKind::Text => FieldValue::Text(
                                widget
                                    .get(*key)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                            ),
                            FieldKind::OptionalText => FieldValue::OptionalText(
                                widget
                                    .get(*key)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                            ),
                            FieldKind::Animate => FieldValue::Choice(
                                widget
                                    .get(*key)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                &ANIMATE_WORDS,
                            ),
                            FieldKind::Align => FieldValue::Choice(
                                widget
                                    .get(*key)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("left")
                                    .to_string(),
                                &["left", "centre", "right"],
                            ),
                        };
                        WidgetField { key, value }
                    })
                    .collect();
                WidgetRow {
                    index,
                    kind,
                    fields,
                    reads,
                    unknown: keys.is_empty(),
                }
            })
            .collect()
    }

    /// Load the picture a `bitmap` widget names: the applet's own first, then the shared set.
    ///
    /// An 8 by 8 blank when neither has it, so a name that does not exist yet is something to draw rather than
    /// something to go and write a file for.
    pub fn refresh_bitmap(&mut self, name: &str) {
        if self.bitmap_editing.as_deref() == Some(name) {
            return;
        }
        let (found, _) = g13_applets::read_bitmaps(&self.applets_dir(), &self.applet_name());
        self.bitmap_frames = match found.get(name) {
            Some(bitmap) => {
                self.bitmap_ms = bitmap.ms;
                bitmap.frames.clone()
            }
            // an 8 by 8 blank when neither file has it, so a name that does not exist yet is something to draw
            // rather than something to go and write a file for
            None => {
                self.bitmap_ms = 120.0;
                vec![vec![vec![false; 8]; 8]]
            }
        };
        self.bitmap_editing = Some(name.to_string());
        self.bitmap_frame = 0;
        self.bitmap_problem = None;
    }

    /// Which applet is being edited, or nothing.
    pub fn applet_name(&self) -> String {
        self.applet_editing.clone().unwrap_or_default()
    }

    /// Light or clear one pixel, and write it. The click *is* the edit; there is no save.
    pub fn set_bitmap_pixel(&mut self, x: usize, y: usize, lit: bool) {
        let Some(frame) = self.bitmap_frames.get_mut(self.bitmap_frame) else {
            return;
        };
        let h = frame.len();
        let w = frame.first().map(Vec::len).unwrap_or(0);
        if x >= w || y >= h {
            return;
        }
        frame[y][x] = lit;
        self.commit_bitmap();
    }

    /// Which frame the pixels being clicked belong to.
    pub fn set_bitmap_frame(&mut self, at: usize) {
        if self.bitmap_frames.is_empty() {
            return;
        }
        self.bitmap_frame = at % self.bitmap_frames.len();
    }

    /// Another frame, a copy of this one, because a sprite is usually a change to the one before it.
    pub fn add_bitmap_frame(&mut self) {
        let Some(frame) = self.bitmap_frames.get(self.bitmap_frame).cloned() else {
            return;
        };
        let at = self.bitmap_frame + 1;
        self.bitmap_frames.insert(at, frame);
        self.bitmap_frame = at;
        self.commit_bitmap();
    }

    /// Take this frame away. A picture needs one frame, so the last one cannot go.
    pub fn remove_bitmap_frame(&mut self) {
        if self.bitmap_frames.len() <= 1 {
            return;
        }
        self.bitmap_frames.remove(self.bitmap_frame);
        if self.bitmap_frame >= self.bitmap_frames.len() {
            self.bitmap_frame = self.bitmap_frames.len() - 1;
        }
        self.commit_bitmap();
    }

    /// How long each frame is held, in milliseconds.
    pub fn set_bitmap_ms(&mut self, ms: f64) {
        self.bitmap_ms = ms.clamp(20.0, 10_000.0);
        self.commit_bitmap();
    }

    /// Change the size of the picture, keeping the pixels that fit inside the new one.
    pub fn set_bitmap_size(&mut self, w: usize, h: usize) {
        let w = w.clamp(1, 32);
        let h = h.clamp(1, 32);
        // every frame, so an animation stays one size and does not jump about as it turns over
        self.bitmap_frames = self
            .bitmap_frames
            .iter()
            .map(|frame| {
                let mut resized = vec![vec![false; w]; h];
                for (y, row) in frame.iter().enumerate().take(h) {
                    for (x, lit) in row.iter().enumerate().take(w) {
                        resized[y][x] = *lit;
                    }
                }
                resized
            })
            .collect();
        self.commit_bitmap();
    }

    /// Write the picture into the applet's **own** bitmaps file, keeping every other picture in it.
    ///
    /// The shared file is never written: it belongs to every applet at once, and an editor that wrote to it
    /// would change what other applets draw without being asked.
    pub fn commit_bitmap(&mut self) {
        let Some(name) = self.bitmap_editing.clone() else {
            return;
        };
        if name.trim().is_empty() {
            self.bitmap_problem =
                Some("a bitmap needs a name: it is what a widget asks for".to_string());
            return;
        }
        let h = self.bitmap_frames.first().map(Vec::len).unwrap_or(0);
        let w = self
            .bitmap_frames
            .first()
            .and_then(|frame| frame.first().map(Vec::len))
            .unwrap_or(0);
        let (mut all, _) = g13_applets::read_bitmaps(&self.applets_dir(), &self.applet_name());
        all.insert(
            name.clone(),
            g13_applets::Bitmap {
                w,
                h,
                ms: self.bitmap_ms,
                frames: self.bitmap_frames.clone(),
            },
        );
        match g13_applets::write_bitmaps(&self.applets_dir(), &self.applet_name(), &all) {
            Ok(()) => {
                self.bitmap_problem = None;
                self.status = format!(
                    "{name} written to {}",
                    g13_applets::bitmaps_path(&self.applets_dir(), &self.applet_name()).display()
                );
            }
            Err(problem) => self.bitmap_problem = Some(problem),
        }
    }

    /// Where the menu file is.
    pub fn menu_path(&self) -> std::path::PathBuf {
        self.config_dir.join("menu.json")
    }

    /// Read the menu file. Called as the Menu tab is drawn, so an edit made outside the window shows up.
    ///
    /// A file that will not read is reported and left alone: an editor that wrote over a menu it could not parse
    /// would lose whatever was wrong with it, which is the one thing worth seeing.
    pub fn refresh_menu(&mut self) {
        match g13_config::menu::read_menu(&self.menu_path()) {
            Ok(Some(menu)) => {
                self.menu_json = Some(menu.to_value());
                self.menu_problem = None;
            }
            // no file is not a problem: the pad's menu is the rotation until one is written
            Ok(None) => {
                self.menu_json = Some(serde_json::json!({"items": []}));
                self.menu_problem = None;
            }
            Err(problem) => {
                // Nothing is held and nothing can be written while the file is unreadable. Holding the menu from
                // before would let the next edit write *that* over a file whose own contents are the one thing
                // worth seeing - and the test for it caught exactly that.
                self.menu_json = None;
                self.menu_problem = Some(problem);
            }
        }
    }

    /// One item of the file's JSON, by where it is.
    fn menu_item(&self, at: (usize, Option<usize>)) -> Option<&serde_json::Value> {
        let top = self
            .menu_json
            .as_ref()?
            .get("items")?
            .as_array()?
            .get(at.0)?;
        match at.1 {
            None => Some(top),
            Some(child) => top.get("items")?.as_array()?.get(child),
        }
    }

    /// The item `menu_item` reads, to edit in place: a top-level item, or one of its children.
    fn menu_item_mut(&mut self, at: (usize, Option<usize>)) -> Option<&mut serde_json::Value> {
        let top = self
            .menu_json
            .as_mut()?
            .get_mut("items")?
            .as_array_mut()?
            .get_mut(at.0)?;
        match at.1 {
            None => Some(top),
            Some(child) => top.get_mut("items")?.as_array_mut()?.get_mut(child),
        }
    }

    /// The menu as rows: each top-level item, then its own items under it.
    pub fn menu_rows(&self) -> Vec<MenuRow> {
        let Some(items) = self
            .menu_json
            .as_ref()
            .and_then(|json| json.get("items"))
            .and_then(|items| items.as_array())
        else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (top, item) in items.iter().enumerate() {
            rows.push(menu_row(item, (top, None)));
            for (child, under) in item
                .get("items")
                .and_then(|items| items.as_array())
                .into_iter()
                .flatten()
                .enumerate()
            {
                rows.push(menu_row(under, (top, Some(child))));
            }
        }
        rows
    }

    /// Put one field of one item, and write the file.
    fn put_menu_field(&mut self, at: (usize, Option<usize>), key: &str, value: serde_json::Value) {
        let Some(item) = self.menu_item_mut(at) else {
            return;
        };
        let Some(object) = item.as_object_mut() else {
            return;
        };
        if value.is_null() {
            object.remove(key);
        } else {
            object.insert(key.to_string(), value);
        }
        self.commit_menu();
    }

    /// What an item is called.
    pub fn set_menu_label(&mut self, at: (usize, Option<usize>), label: &str) {
        let label = label.trim();
        self.put_menu_field(at, "label", serde_json::Value::String(label.to_string()));
    }

    /// Change what an item does. The keys for the other two go, so an item cannot be two things at once.
    pub fn set_menu_does(&mut self, at: (usize, Option<usize>), does: MenuDoes) {
        let had = match self.menu_item(at) {
            Some(item) => (
                item.get("show")
                    .and_then(|show| show.as_str())
                    .unwrap_or_default()
                    .to_string(),
                item.get("command")
                    .and_then(|command| command.as_str())
                    .unwrap_or_default()
                    .to_string(),
            ),
            None => (String::new(), String::new()),
        };
        match does {
            MenuDoes::List => {
                self.put_menu_field(at, "show", serde_json::Value::Null);
                self.put_menu_field(at, "screen", serde_json::Value::Null);
                self.put_menu_field(at, "command", serde_json::Value::Null);
                // a level with nothing in it cannot be opened, so choosing this makes it one item to start
                if self
                    .menu_item(at)
                    .and_then(|item| item.get("items"))
                    .and_then(|items| items.as_array())
                    .map(|items| items.is_empty())
                    .unwrap_or(true)
                {
                    self.put_menu_field(
                        at,
                        "items",
                        serde_json::json!([{"label": "new item", "show": "clock"}]),
                    );
                }
            }
            MenuDoes::Screen => {
                self.put_menu_field(at, "command", serde_json::Value::Null);
                // keep whatever screen it already showed, and start on `clock` rather than on nothing
                let show = if had.0.is_empty() {
                    "clock".to_string()
                } else {
                    had.0
                };
                self.put_menu_field(at, "show", serde_json::Value::String(show));
            }
            MenuDoes::Command => {
                self.put_menu_field(at, "show", serde_json::Value::Null);
                self.put_menu_field(at, "screen", serde_json::Value::Null);
                self.put_menu_field(at, "command", serde_json::Value::String(had.1));
            }
        }
    }

    /// The screen an item shows, or the command it runs.
    pub fn set_menu_value(&mut self, at: (usize, Option<usize>), value: &str) {
        let value = value.trim();
        let key = match self.menu_item(at).map(|item| item.get("show").is_some()) {
            Some(true) => "show",
            _ => "command",
        };
        self.put_menu_field(at, key, serde_json::Value::String(value.to_string()));
    }

    /// Which screen of a shown applet, counted from one. Nothing means its first.
    pub fn set_menu_screen(&mut self, at: (usize, Option<usize>), screen: Option<usize>) {
        match screen {
            Some(screen) => self.put_menu_field(at, "screen", serde_json::json!(screen)),
            None => self.put_menu_field(at, "screen", serde_json::Value::Null),
        }
    }

    /// Add an item at the top level. It shows the clock to start, so it works before it is edited.
    pub fn add_menu_item(&mut self) {
        let label = match self.new_menu_label.trim() {
            "" => "new item".to_string(),
            typed => typed.to_string(),
        };
        let Some(items) = self
            .menu_json
            .as_mut()
            .and_then(|json| json.get_mut("items"))
            .and_then(|items| items.as_array_mut())
        else {
            return;
        };
        items.push(serde_json::json!({"label": label, "show": "clock"}));
        self.new_menu_label.clear();
        self.commit_menu();
    }

    /// Add an item under one of the top-level ones.
    pub fn add_menu_child(&mut self, top: usize) {
        let Some(item) = self.menu_item_mut((top, None)) else {
            return;
        };
        let Some(object) = item.as_object_mut() else {
            return;
        };
        let under = object
            .entry("items".to_string())
            .or_insert_with(|| serde_json::json!([]));
        if let Some(list) = under.as_array_mut() {
            list.push(serde_json::json!({"label": "new item", "show": "clock"}));
        };
        // a level cannot also act, so choosing to add one to an item that acts makes it a level
        object.remove("show");
        object.remove("screen");
        object.remove("command");
        self.commit_menu();
    }

    /// Take an item, or one of an item's own items, out of the menu.
    pub fn remove_menu_item(&mut self, at: (usize, Option<usize>)) {
        let Some(items) = self
            .menu_json
            .as_mut()
            .and_then(|json| json.get_mut("items"))
            .and_then(|items| items.as_array_mut())
        else {
            return;
        };
        match at.1 {
            None => {
                if at.0 < items.len() {
                    items.remove(at.0);
                }
            }
            Some(child) => {
                if let Some(under) = items
                    .get_mut(at.0)
                    .and_then(|item| item.get_mut("items"))
                    .and_then(|items| items.as_array_mut())
                {
                    if child < under.len() {
                        under.remove(child);
                    }
                }
            }
        }
        self.commit_menu();
    }

    /// Move an item one place up or down among its neighbours.
    pub fn move_menu_item(&mut self, at: (usize, Option<usize>), up: bool) {
        let Some(items) = self
            .menu_json
            .as_mut()
            .and_then(|json| json.get_mut("items"))
            .and_then(|items| items.as_array_mut())
        else {
            return;
        };
        let list = match at.1 {
            None => items,
            Some(_) => match items
                .get_mut(at.0)
                .and_then(|item| item.get_mut("items"))
                .and_then(|items| items.as_array_mut())
            {
                Some(list) => list,
                None => return,
            },
        };
        let index = at.1.unwrap_or(at.0);
        if index >= list.len() || list.len() < 2 {
            return;
        }
        let to = if up {
            index.saturating_sub(1)
        } else {
            (index + 1).min(list.len() - 1)
        };
        if to != index {
            list.swap(index, to);
        }
        self.commit_menu();
    }

    /// Write the menu. A menu that would not read back is refused and the file is left alone.
    pub fn commit_menu(&mut self) {
        let Some(json) = self.menu_json.clone() else {
            return;
        };
        let path = self.menu_path();
        match g13_config::menu::write_menu_value(&path, &json) {
            Ok(()) => {
                self.menu_problem = None;
                self.status = format!("menu written to {}", path.display());
            }
            Err(problem) => self.menu_problem = Some(problem),
        }
    }

    /// Put one field of one widget, and write the applet.
    ///
    /// A field set to nothing is removed rather than written as null: for the fields that may be left out,
    /// leaving one out is what the reader looks for, and `null` is not the same thing to a person reading it.
    fn put_widget_field(&mut self, index: usize, key: &str, value: serde_json::Value) {
        // through the accessor: which array this is depends on whether the applet has screens
        let written = self
            .widget_array_mut()
            .and_then(|widgets| widgets.get_mut(index))
            .and_then(|widget| widget.as_object_mut());
        if let Some(widget) = written {
            if value.is_null() {
                widget.remove(key);
            } else {
                widget.insert(key.to_string(), value);
            }
        }
        self.commit_applet();
    }

    /// Set one of a widget's numbered fields.
    pub fn set_widget_number(&mut self, index: usize, key: &str, value: f64) {
        self.put_widget_field(index, key, number_value(value));
    }

    /// Set a number the file may leave out, where none takes the key away rather than writing it empty.
    pub fn set_widget_optional_number(&mut self, index: usize, key: &str, value: Option<f64>) {
        self.put_widget_field(
            index,
            key,
            value.map(number_value).unwrap_or(serde_json::Value::Null),
        );
    }

    /// reader treats "absent" and "no" the same but a person reading the file should see what was chosen.
    /// The question a new alert starts from: the first thing this applet reads, compared with something.
    ///
    /// A **value** question, never a control one: a control question is always false in an applet, because a
    /// control is the one thing only a pad can answer, so starting from one is starting from something that cannot
    /// work. It lives here rather than in the drawing so it can be tested.
    pub fn new_alert_question(&self) -> crate::macros::ConditionForm {
        let names = self.applet_source_names();
        let name = names
            .keys()
            .next()
            .cloned()
            .or_else(|| crate::macros::readable_names(None).into_iter().next())
            .unwrap_or_default();
        crate::macros::ConditionForm {
            all: true,
            rows: vec![crate::macros::QuestionRow {
                not: false,
                question: crate::macros::Question::Value {
                    name,
                    op: g13_values::Compare::Gt,
                    number: Some(80.0),
                    text: None,
                },
            }],
        }
    }

    /// Every picture this applet can draw, by name: its own file first, then the shared one.
    ///
    /// The same read the bitmap inspector does, so a name offered here is a name that will draw.
    pub fn bitmap_names(&self) -> Vec<String> {
        let (found, _) = g13_applets::read_bitmaps(&self.applets_dir(), &self.applet_name());
        found.keys().cloned().collect()
    }

    /// The titles of the applet's own screens, for offering a `screen:` action by name.
    ///
    /// A single-screen applet has none: there is nowhere to go, and offering a button that cannot work is worse
    /// than offering none.
    pub fn applet_screen_titles(&self) -> Vec<String> {
        self.applet_json
            .get("screens")
            .and_then(|screens| screens.as_array())
            .into_iter()
            .flatten()
            .filter_map(|screen| screen.get("title").and_then(|title| title.as_str()))
            .map(str::to_string)
            .collect()
    }

    /// The applet's own sources, as the names a question about it may ask for.
    pub fn applet_source_names(&self) -> std::collections::BTreeMap<String, String> {
        self.applet_json
            .get("sources")
            .and_then(|sources| sources.as_object())
            .into_iter()
            .flatten()
            .filter_map(|(name, spec)| Some((name.clone(), spec.as_str()?.to_string())))
            .collect()
    }

    /// The alert on a widget, as the window holds it: which look, and the question in the editor's form.
    ///
    /// The question is read out of the file's own JSON with the same parser the driver uses, and given to the same
    /// editor the macros tab draws - so an alert is edited with the rows a macro's `if` is, which is the point of
    /// one condition language.
    pub fn widget_alert_look(&self, index: usize) -> String {
        self.widget_alert(index)
            .and_then(|alert| alert.get("look"))
            .and_then(|look| look.as_str())
            .unwrap_or_default()
            .to_string()
    }

    /// The bitmap an alert shows when its question is true, or empty when it names none.
    pub fn widget_alert_bitmap(&self, index: usize) -> String {
        self.widget_alert(index)
            .and_then(|alert| alert.get("bitmap"))
            .and_then(|bitmap| bitmap.as_str())
            .unwrap_or_default()
            .to_string()
    }

    /// The question, in the editor's form. `None` when there is no alert or the question will not read, which the
    /// tab says rather than showing an empty one.
    pub fn widget_alert_question(&self, index: usize) -> Option<crate::macros::ConditionForm> {
        let when = self.widget_alert(index)?.get("when")?;
        crate::macros::ConditionForm::from_cond(&g13_values::parse_cond(when).ok()?)
    }

    /// The alert itself, out of the widget's JSON.
    fn widget_alert(&self, index: usize) -> Option<&serde_json::Value> {
        let at = if self.applet_json.get("screens").is_some() {
            // which array the widgets are in is asked in one place, never worked out twice
            None
        } else {
            Some(index)
        }?;
        self.applet_json
            .get("widgets")?
            .as_array()?
            .get(at)?
            .get("alert")
    }

    /// Set what an alert does. `none` takes the whole alert away rather than leaving an object with nothing in it,
    /// so a widget that is not reacting says nothing at all in the file.
    pub fn set_widget_alert_look(&mut self, index: usize, look: &str) {
        match look.is_empty() {
            true => {
                self.put_widget_alert(index, None);
            }
            false => {
                if let Some(alert) = self.put_widget_alert(index, Some(serde_json::json!({}))) {
                    alert.insert("look".to_string(), serde_json::json!(look));
                }
            }
        }
        // written as it is made, like every other field in this window: there is no Apply button anywhere
        self.commit_applet();
    }

    /// Set the question, from the form the editor edits.
    pub fn set_widget_alert_question(&mut self, index: usize, form: &crate::macros::ConditionForm) {
        let when = g13_values::cond_to_json(&form.to_cond());
        if let Some(alert) = self.put_widget_alert(index, Some(serde_json::json!({}))) {
            alert.insert("when".to_string(), when);
        }
        self.commit_applet();
    }

    /// The picture a `look: show` draws.
    pub fn set_widget_alert_bitmap(&mut self, index: usize, name: &str) {
        if let Some(alert) = self.put_widget_alert(index, Some(serde_json::json!({}))) {
            match name.is_empty() {
                true => {
                    alert.remove("bitmap");
                }
                false => {
                    alert.insert("bitmap".to_string(), serde_json::json!(name));
                }
            }
        }
        self.commit_applet();
    }

    /// Put an alert object on a widget, or take it away, and hand back the object to write into.
    fn put_widget_alert(
        &mut self,
        index: usize,
        alert: Option<serde_json::Value>,
    ) -> Option<&mut serde_json::Map<String, serde_json::Value>> {
        let widget = self.widget_array_mut()?.get_mut(index)?.as_object_mut()?;
        match alert {
            None => {
                widget.remove("alert");
                None
            }
            Some(value) => {
                // an alert that is already there is kept: changing the look must not throw the question away,
                // which is what inserting over it did
                if !widget.contains_key("alert") {
                    widget.insert("alert".to_string(), value);
                }
                widget.get_mut("alert")?.as_object_mut()
            }
        }
    }

    /// Set one of the applet's own text fields, or take the key away when there is nothing to say.
    pub fn set_applet_optional_text(&mut self, key: &str, value: Option<&str>) {
        match value {
            Some(value) => self.set_applet_text(key, value),
            None => {
                if let Some(object) = self.applet_json.as_object_mut() {
                    object.remove(key);
                }
            }
        }
    }

    /// Tick a widget's yes-or-no field. Written as a real `false` when cleared, because for these the file's
    pub fn set_widget_flag(&mut self, index: usize, key: &str, value: bool) {
        self.put_widget_field(index, key, serde_json::Value::Bool(value));
    }

    /// Set one of a widget's text fields.
    pub fn set_widget_text(&mut self, index: usize, key: &str, value: &str) {
        self.put_widget_field(index, key, serde_json::Value::String(value.to_string()));
    }

    /// Put a field the file may leave out, where `None` takes the key away rather than writing it empty.
    pub fn set_widget_optional_text(&mut self, index: usize, key: &str, value: Option<&str>) {
        let written = match value {
            Some(text) => serde_json::Value::String(text.to_string()),
            // the same path a cleared number takes: the key goes, because not being there is the instruction
            None => serde_json::Value::Null,
        };
        self.put_widget_field(index, key, written);
    }

    /// Move a widget one place up or down the list.
    ///
    /// Order is what draws: widgets are drawn in the order they are listed, so a later one covers an earlier
    /// one. Nothing else about the widget changes - the entries swap places rather than being taken out and
    /// put back, so every key of both survives exactly as it was.
    pub fn move_widget(&mut self, index: usize, up: bool) {
        let Some(widgets) = self.widget_array_mut() else {
            return;
        };
        let last = widgets.len().saturating_sub(1);
        if index > last {
            return;
        }
        // at the ends there is nowhere to go, and saying nothing is right: the button is what says whether it
        // can be pressed
        let to = if up {
            if index == 0 {
                return;
            }
            index - 1
        } else {
            if index >= last {
                return;
            }
            index + 1
        };
        widgets.swap(index, to);
        self.commit_applet();
    }

    /// Remove a widget. Everything after it moves up, which is why the index is taken rather than held.
    pub fn remove_widget(&mut self, index: usize) {
        // the open fields belong to a widget that is about to move: shut them rather than leave a grid editing
        // whichever widget slid into that place
        self.widget_editing = None;
        if let Some(widgets) = self.widget_array_mut() {
            if index < widgets.len() {
                widgets.remove(index);
            }
        }
        self.commit_applet();
    }

    /// Add a widget of a kind, placed where the next free line is so it lands somewhere visible rather than on
    /// top of what is already there.
    pub fn add_widget(&mut self, kind: &str) {
        let fields = widget_fields_for(kind);
        if fields.is_empty() {
            self.status = format!("{kind} is not a widget this build draws");
            return;
        }
        let existing = self.applet_widgets().len();
        let y = (8 + existing * 10) % g13_screen::VISIBLE_HEIGHT.max(1);
        let mut widget = serde_json::Map::new();
        widget.insert("type".to_string(), serde_json::json!(kind));
        for (key, holds) in fields {
            // a new widget is born with a command field too, so the pad can act on it without the file being
            // edited by hand first; an empty one is left out when it is written
            if key == "command" {
                continue;
            }
            let value = match (key, holds) {
                ("y", _) => number_value(y as f64),
                ("x", FieldKind::OptionalNumber) => continue,
                ("x", _) => number_value(3.0),
                ("w", _) => number_value(100.0),
                ("h", _) => number_value(6.0),
                ("count", _) => number_value(10.0),
                ("len", _) => number_value(40.0),
                ("thick", _) => number_value(2.0),
                ("r", _) => number_value(12.0),
                ("max", _) => number_value(100.0),
                ("format", _) => serde_json::json!("{name}"),
                ("align", _) => serde_json::json!("left"),
                _ => continue,
            };
            widget.insert((*key).to_string(), value);
        }
        if let Some(widgets) = self.widget_array_mut() {
            widgets.push(serde_json::Value::Object(widget));
        }
        self.commit_applet();
    }

    /// Where the applets live.
    pub fn applets_dir(&self) -> PathBuf {
        self.config_dir.join("applets")
    }

    /// Every applet that exists, which is what the designer offers to edit.
    pub fn applet_names(&self) -> Vec<String> {
        g13_applets::applet_names(&self.applets_dir())
    }

    /// Open an applet for editing, and point the preview at it.
    ///
    /// The preview follows the applet being designed rather than whatever the pad is showing, because the point
    /// of a designer is seeing the screen as you change it. Only the window's own renderer is retargeted; the
    /// driver draws on the pad from its own.
    pub fn open_applet(&mut self, name: &str) {
        match g13_applets::load_applet_json(&self.applets_dir(), name) {
            Ok(value) => {
                self.applet_json = value;
                // always start on the applet's first screen: a number left over from another applet means
                // nothing here
                self.screen_editing = 0;
                self.widget_editing = None;
                self.screen.show_screen(0);
                self.applet_editing = Some(name.to_string());
                self.applet_problem = None;
                self.screen.show(&format!("applet:{name}"));
                self.status = format!("editing {name}");
            }
            Err(problem) => {
                self.status = problem.clone();
                self.applet_problem = Some(problem);
            }
        }
    }

    /// Stop editing, and put the preview back on the visual the driver is showing.
    pub fn close_applet(&mut self) {
        self.applet_editing = None;
        self.applet_json = serde_json::Value::Null;
        self.screen_editing = 0;
        self.applet_problem = None;
        let visual = self.visual.clone();
        self.screen.show(&visual);
    }

    /// One of the applet's own fields, as text, for showing.
    pub fn applet_field(&self, key: &str) -> String {
        match self.applet_json.get(key) {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        }
    }

    /// One of the applet's own numbers, for a control that wants a number.
    pub fn applet_number(&self, key: &str) -> f64 {
        self.applet_json
            .get(key)
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0)
    }

    /// One of the applet's own flags.
    pub fn applet_flag(&self, key: &str) -> bool {
        self.applet_json
            .get(key)
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    /// Set one of the applet's own fields, and write the applet back to its file.
    fn put_applet_field(&mut self, key: &str, value: serde_json::Value) {
        if let Some(object) = self.applet_json.as_object_mut() {
            object.insert(key.to_string(), value);
        }
        self.commit_applet();
    }

    /// Set one of the applet's fields as text.
    pub fn set_applet_text(&mut self, key: &str, text: &str) {
        self.put_applet_field(key, serde_json::Value::String(text.to_string()));
    }

    /// Set one of the applet's fields as a number.
    ///
    /// A whole number is written as a whole number: `"interval": 2` is what the file said and what a person
    /// types, and `2.0` is both uglier and a change nobody asked for.
    pub fn set_applet_number(&mut self, key: &str, value: f64) {
        self.put_applet_field(key, number_value(value));
    }

    /// Set one of the applet's flags.
    pub fn set_applet_flag(&mut self, key: &str, value: bool) {
        self.put_applet_field(key, serde_json::Value::Bool(value));
    }

    /// How long this applet keeps the screen after the program feeding it stops, or 0 when it does not follow.
    ///
    /// See `g13_applets::Follow`: while the file behind one of this applet's own sources is being written, the
    /// applet is what is on the screen, and it hands it back when the writing stops.
    pub fn applet_follow_seconds(&self) -> f64 {
        match self.applet_json.get(g13_applets::FOLLOW_KEY) {
            Some(serde_json::Value::Bool(true)) => g13_applets::Follow::DEFAULT_SECONDS,
            Some(serde_json::Value::Number(number)) => number
                .as_f64()
                .unwrap_or(g13_applets::Follow::DEFAULT_SECONDS),
            Some(serde_json::Value::Object(fields)) => fields
                .get("seconds")
                .and_then(|value| value.as_f64())
                .unwrap_or(g13_applets::Follow::DEFAULT_SECONDS),
            _ => 0.0,
        }
    }

    /// The binding set this applet asks for while it is being fed, if it names one.
    pub fn applet_follow_profile(&self) -> Option<String> {
        match self.applet_json.get(g13_applets::FOLLOW_KEY) {
            Some(serde_json::Value::Object(fields)) => fields
                .get("profile")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            _ => None,
        }
    }

    /// Write what this applet follows with.
    ///
    /// Zero seconds takes the key away rather than writing `"follow": false`: an applet that does not follow has no
    /// business carrying a line that says nothing, and a file that says nothing is a file anybody can read.
    pub fn set_applet_follow(&mut self, seconds: f64, profile: Option<String>) {
        let key = g13_applets::FOLLOW_KEY;
        if seconds <= 0.0 {
            if let Some(object) = self.applet_json.as_object_mut() {
                object.remove(key);
            }
        } else {
            let mut fields = serde_json::Map::new();
            fields.insert("seconds".to_string(), number_value(seconds));
            if let Some(name) = profile.filter(|name| !name.is_empty()) {
                fields.insert("profile".to_string(), serde_json::Value::String(name));
            }
            if let Some(object) = self.applet_json.as_object_mut() {
                object.insert(key.to_string(), serde_json::Value::Object(fields));
            }
        }
        self.commit_applet();
    }

    /// The applet's sources: the name a widget's `{name}` refers to, and what reads it.
    pub fn applet_sources(&self) -> Vec<(String, String)> {
        self.applet_json
            .get("sources")
            .and_then(|sources| sources.as_object())
            .map(|object| {
                object
                    .iter()
                    .map(|(name, spec)| {
                        (name.clone(), spec.as_str().unwrap_or_default().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Which of the applet's sources a widget actually uses, so an unused one is visible.
    ///
    /// Worked out from the applet as this build reads it, because "unused" means unused by what will be drawn.
    pub fn applet_sources_used(&self) -> Vec<String> {
        let Ok(applet) = g13_applets::parse_applet(
            &g13_applets::applet_json_text(&self.applet_json).unwrap_or_default(),
        ) else {
            return Vec::new();
        };
        // the format's own rule, so the panel and the pad cannot disagree about what is read: every screen's
        // widgets, and a list widget's source as well as a bar's
        g13_applets::names_read(&applet)
    }

    /// Change what one of the applet's sources reads.
    pub fn set_applet_source(&mut self, name: &str, spec: &str) {
        if let Some(sources) = self
            .applet_json
            .get_mut("sources")
            .and_then(|sources| sources.as_object_mut())
        {
            sources.insert(
                name.to_string(),
                serde_json::Value::String(spec.to_string()),
            );
        }
        self.commit_applet();
    }

    /// Add a source to the applet. A name already used is refused rather than replaced, because every widget
    /// that names it would silently start reading something else.
    /// Make an applet: a file with a name, written straight away, then opened for editing.
    ///
    /// Its name is its file's name, so a name that would not be a sane file name is refused by name rather than
    /// written somewhere unexpected - and an applet that already exists is never written over.
    pub fn create_applet(&mut self) {
        let name = self.new_applet_name.trim().to_string();
        if name.is_empty() {
            self.applet_problem =
                Some("an applet needs a name: that name is what its file is called".to_string());
            return;
        }
        let refused = name
            .chars()
            .find(|character| {
                !(character.is_ascii_alphanumeric()
                    || *character == ' '
                    || *character == '-'
                    || *character == '_')
            })
            .map(|character| character.to_string());
        if let Some(character) = refused {
            self.applet_problem = Some(format!(
                "{name} has a {character:?} in it, and an applet's name is its file's name: letters, digits, \
                 spaces, - and _ only"
            ));
            return;
        }
        let path = g13_applets::applet_path(&self.applets_dir(), &name);
        if path.exists() {
            self.applet_problem = Some(format!(
                "there is already an applet called {name}: pick another name, or open that one. Nothing was \
                 written - this never writes over a file"
            ));
            return;
        }
        // interval 2 so a new applet redraws by itself, and no widgets: it is a blank screen to build on
        let fresh = serde_json::json!({
            "name": name,
            "title": name,
            "interval": 2,
            "sources": {},
            "widgets": [],
        });
        match g13_applets::save_applet_json(&self.applets_dir(), &name, &fresh) {
            Ok(()) => {
                self.new_applet_name.clear();
                self.open_applet(&name);
                self.status = format!(
                    "{name} made at {}. Tick it on the Screen tab to put it in the rotation.",
                    path.display()
                );
            }
            Err(problem) => self.applet_problem = Some(problem),
        }
    }

    /// Declare a source and point the widget that asked at it.
    ///
    /// Which field takes the name depends on the widget: one that reads a single value has `source`, while a
    /// text widget reads it inside its `format` as `{name}`. Either way the applet declares it, so the pad never
    /// draws a name that resolves to nothing.
    pub fn add_source_for_widget(&mut self, index: usize, name: &str, spec: &str) {
        self.add_applet_source(name, spec);
        let name = name.trim();
        // it may have been refused - no spec, or the name already taken - and the status says which, so the
        // widget is left alone rather than pointed at a name nothing declares
        if name.is_empty() || self.applet_sources().iter().all(|(known, _)| known != name) {
            return;
        }
        let Some(row) = self
            .applet_widgets()
            .into_iter()
            .find(|row| row.index == index)
        else {
            return;
        };
        if row.fields.iter().any(|field| field.key == "source") {
            self.set_widget_text(index, "source", name);
            return;
        }
        if let Some(held) = row.fields.iter().find(|field| field.key == "format") {
            let held = held.value.text();
            self.set_widget_text(index, "format", &format!("{held}{{{name}}}"));
        }
    }

    /// Add a source to the applet being edited, refusing one it already has by that name.
    pub fn add_applet_source(&mut self, name: &str, spec: &str) {
        let name = name.trim();
        let spec = spec.trim();
        if name.is_empty() || spec.is_empty() {
            self.status = "a source needs a name and something to read".to_string();
            return;
        }
        if self.applet_sources().iter().any(|(known, _)| known == name) {
            self.status = format!("this applet already has a source called {name}");
            return;
        }
        if !self.applet_json.is_object() {
            self.applet_json = serde_json::json!({});
        }
        // said rather than panicked: a file whose `sources` is not an object is a file a person edited, and
        // the window's job is to say so and leave it alone
        let Some(object) = self.applet_json.as_object_mut() else {
            self.status =
                "this applet is not an object, so a source cannot be added to it".to_string();
            return;
        };
        let Some(sources) = object
            .entry("sources")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
        else {
            self.status =
                "this applet's `sources` is not an object, so nothing was added to it".to_string();
            return;
        };
        sources.insert(
            name.to_string(),
            serde_json::Value::String(spec.to_string()),
        );
        self.commit_applet();
    }

    /// Remove a source. A widget still naming it will report that rather than draw a blank silently.
    pub fn remove_applet_source(&mut self, name: &str) {
        if let Some(sources) = self
            .applet_json
            .get_mut("sources")
            .and_then(|sources| sources.as_object_mut())
        {
            sources.remove(name);
        }
        self.commit_applet();
    }

    /// Write the applet as it now stands.
    ///
    /// Written the moment a field changes, as everywhere else in this window. An applet that cannot be read
    /// back is not written at all: a file that does not parse would leave the pad's screen blank, which looks
    /// exactly like an applet that draws nothing.
    pub fn commit_applet(&mut self) {
        let Some(name) = self.applet_editing.clone() else {
            return;
        };
        match g13_applets::save_applet_json(&self.applets_dir(), &name, &self.applet_json) {
            Ok(()) => {
                self.applet_problem = None;
                self.status = format!(
                    "{name} written to {}",
                    g13_applets::applet_path(&self.applets_dir(), &name).display()
                );
            }
            Err(problem) => {
                self.status = problem.clone();
                self.applet_problem = Some(problem);
            }
        }
    }

    /// Turn the stick on or off, in one of its modes.
    pub fn set_stick_mode(&mut self, mode: g13_config::stick::Mode) {
        self.stick.mode = mode;
        match self.stick.save() {
            Ok(()) => {
                self.status = match mode {
                    g13_config::stick::Mode::Off => "the stick does nothing".to_string(),
                    g13_config::stick::Mode::Keyboard => {
                        let bound = self.bindings.bound_sectors(&self.stick.sectors);
                        if bound.is_empty() {
                            format!(
                                "the stick sends nothing: none of its {} sectors is bound",
                                self.stick.sectors.count
                            )
                        } else {
                            format!(
                                "the stick now drives {} of its {} sectors",
                                bound.len(),
                                self.stick.sectors.count
                            )
                        }
                    }
                    g13_config::stick::Mode::Mouse => {
                        format!(
                            "the stick moves the pointer at {:.0} pixels a second",
                            self.stick.speed
                        )
                    }
                    g13_config::stick::Mode::Joystick => {
                        "the stick is a gamepad's analogue stick".to_string()
                    }
                }
            }
            Err(problem) => self.status = problem,
        }
    }

    /// Send one side of one axis somewhere else, and write it.
    pub fn set_route(&mut self, side: &str, target: g13_config::stick::Target) {
        self.stick.route.set(side, target);
        match self.stick.save() {
            Ok(()) => {
                self.status = if self.stick.route.is_a_plain_stick() {
                    format!("{side} now goes to {}", target.name())
                } else {
                    format!(
                        "{side} now goes to {} - the stick is split between targets now",
                        target.name()
                    )
                };
            }
            Err(problem) => self.status = problem,
        }
    }

    /// How fast the stick moves the pointer at full deflection.
    pub fn set_speed(&mut self, speed: f64) {
        self.stick.speed = speed.clamp(20.0, 5000.0);
        if let Err(problem) = self.stick.save() {
            self.status = problem;
        }
    }

    /// How far the stick must move before it counts as a direction.
    pub fn set_deadzone(&mut self, deadzone: f64) {
        self.stick.stick.deadzone = deadzone.clamp(0.05, 0.9);
        if let Err(problem) = self.stick.save() {
            self.status = problem;
        }
    }

    /// Every sector of the stick, in order, as the bindings file has them.
    ///
    /// A sector with no line of its own reports what it will fall back to, because that is what will happen -
    /// and says so, rather than looking like a binding somebody wrote.
    pub fn sector_rows(&self) -> Vec<SectorRow> {
        let text = std::fs::read_to_string(self.bindings_file()).unwrap_or_default();
        let sectors = self.stick.sectors;
        (0..sectors.count)
            .map(|index| {
                // the name the file already uses for this sector, so a file written by hand keeps its wording
                let own = sectors
                    .names(index)
                    .into_iter()
                    .find(|name| g13_config::binding_for(&text, name).is_some());
                let inherited = own.is_none();
                let name = own.unwrap_or_else(|| sectors.name(index));
                let action = g13_config::binding_for(&text, &name).unwrap_or_default();
                let effect = match self.bindings.bounds_for(&sectors, index).first() {
                    Some(bound) => bound.name(),
                    None => "nothing".to_string(),
                };
                SectorRow {
                    index,
                    name,
                    action,
                    effect,
                    inherited,
                }
            })
            .collect()
    }

    /// Which sector the stick is being held in, for showing.
    pub fn stick_sector(&self) -> Option<u32> {
        let (x, y) = self.live.stick()?;
        // the same calculation the driver uses, so what this shows is what the driver would do
        self.stick.stick.sector((x, y), &self.stick.sectors)
    }

    /// How many sectors the turn is broken into. Four is a d-pad, eight is what the old stack did, and more
    /// is a wheel - a setting to choose, applied the moment it is changed.
    pub fn set_sector_count(&mut self, count: u32) {
        self.stick.sectors = g13_config::stick::Sectors::new(count);
        match self.stick.save() {
            Ok(()) => {
                self.status = format!(
                    "the stick is now read as {} sectors",
                    self.stick.sectors.count
                );
            }
            Err(problem) => self.status = problem,
        }
    }

    /// Record what the stick is reporting right now as one of the calibration points.
    pub fn take_calibration_point(&mut self) {
        let Some(index) = self.taking else {
            return;
        };
        let Some((name, _)) = CALIBRATION_POINTS.get(index) else {
            self.taking = None;
            return;
        };
        let Some(stick) = self.live.stick() else {
            self.status = "the stick is not being reported: is a driver running?".to_string();
            return;
        };
        // Written through the stick settings, which own this file. Two writers would each drop the other's
        // half of it, and the calibration is the half that is expensive to lose.
        self.stick.set_point(name, stick);
        match self.stick.save() {
            Ok(()) => {
                let remaining = self.stick.missing(&CALIBRATION_NAMES);
                self.status = if remaining.is_empty() {
                    format!("{name} recorded at {}: every point is taken", show(stick))
                } else {
                    format!(
                        "{name} recorded at {}: {} to go",
                        show(stick),
                        remaining.len()
                    )
                };
            }
            Err(problem) => self.status = problem,
        }
        // move to the next point that has not been taken, or stop
        self.taking = CALIBRATION_POINTS
            .iter()
            .position(|(name, _)| !self.stick.points.contains_key(*name));
    }

    /// Every visual that can be chosen: the built-ins and the applets that exist.
    pub fn available_visuals(&self) -> Vec<String> {
        let mut names = vec![
            "clock".to_string(),
            "system".to_string(),
            "pad".to_string(),
            "media".to_string(),
        ];
        if let Ok(entries) = std::fs::read_dir(self.config_dir.join("applets")) {
            let mut applets: Vec<String> = entries
                .flatten()
                // its pictures are not a screen: without this the pad is offered `weather.bitmaps` to walk
                // to, which draws nothing and is not an applet at all
                .filter(|entry| g13_applets::is_applet_file(&entry.path()))
                .filter_map(|entry| {
                    entry
                        .path()
                        .file_stem()
                        .map(|stem| format!("applet:{}", stem.to_string_lossy()))
                })
                .collect();
            applets.sort();
            names.extend(applets);
        }
        names
    }
}

/// A stick reading, written the way the samples file writes it.
pub fn show(pair: (u8, u8)) -> String {
    format!("0x{:02x} 0x{:02x}", pair.0, pair.1)
}

/// Turn what a person types into what the file holds.
///
/// A key name on its own means that key, the way `g13 bind` reads it; anything that is already an action is
/// left exactly as written.
pub fn normalise(action: &str) -> String {
    let action = action.trim();
    if action.is_empty() {
        return "x".to_string();
    }
    if g13_config::is_action(action) {
        return action.to_string();
    }
    match g13_device::keyboard::parse_key(action) {
        Ok(key) => format!("p,k.{}", key.code()),
        // not an action and not a key name: written as it is, and the driver will say it does not know it
        Err(_) => action.to_string(),
    }
}

/// Say what an action will do, in words, so the table means something without knowing the syntax.
///
/// The words themselves are the agent's, shared with `g13 bindings`: two copies is how the CLI came to say an
/// action was not applied by this build when it was.
fn describe(action: &str) -> String {
    g13_agent::describe_action(action)
}

/// The controls, so a caller need not reach into the device crate.
pub fn controls() -> Vec<(String, Option<usize>)> {
    g13_device::CONTROLS
        .iter()
        .map(|control| {
            (
                control.to_string(),
                g13_proto::control_from_name(control).and_then(g13_proto::bit_for_control),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_config_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The frame the preview has settled on: taken twice, and the same both times.
    ///
    /// Two, not one, because the worker finishes the pass it is in when it is woken: the first frame after a
    /// change can still be the one it had already started drawing for the previous visual, and a test that
    /// compared against that would be measuring a race rather than the thing it is about.
    fn settled_frame(window: &Window) -> g13_screen::Frame {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut last: Option<g13_screen::Frame> = None;
        loop {
            if std::time::Instant::now() > until {
                return last.expect("the preview never drew a frame");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            match window.screen.take() {
                Some(frame) => {
                    if last.as_ref() == Some(&frame) {
                        return frame;
                    }
                    last = Some(frame);
                }
                None => continue,
            }
        }
    }

    #[test]
    fn the_preview_draws_the_screen_the_designer_is_on() {
        // The preview has to show the screen the designer is on, or no screen but the first can be checked by eye.
        // The preview is a screen worker, so it is told the screen the same way the driver tells it: through the
        // status it draws from.
        let dir = an_applet_dir("g13-screens-preview");
        std::fs::write(
            dir.join("applets/two.json"),
            r#"{"name": "two", "interval": 1, "sources": {}, "screens": [
                 {"title": "first", "widgets": [{"type": "text", "x": 2, "y": 2, "format": "FIRST"}]},
                 {"title": "second", "widgets": [{"type": "text", "x": 2, "y": 2, "format": "SECOND"}]}]}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("two");
        let on_first = settled_frame(&window);

        window.select_screen(1);
        assert_eq!(
            window.screen.status().screen,
            1,
            "the worker was told the wrong screen"
        );
        let on_second = settled_frame(&window);
        assert_ne!(
            on_first, on_second,
            "the preview drew screen 1 again after screen 2 was picked"
        );

        // and back again, which must be the frame it showed for screen 1
        window.select_screen(0);
        assert_eq!(
            window.screen.status().screen,
            0,
            "the worker was told the wrong screen"
        );
        let back = settled_frame(&window);
        assert_eq!(back, on_first, "going back drew something else");
    }

    #[test]
    fn choosing_one_of_the_machines_names_declares_it_so_the_widget_can_read_it() {
        // Picking one of the machine's names used to fail with "nothing provides this" while the built-in media
        // applet worked, because a widget's source is a name from the applet's own vocabulary: `media_title` reads
        // only once the applet declares it. The picker offered the catalogue's names and declared nothing, leaving
        // that to hand, so this is the declaring, in the two steps the picker now takes.
        let dir = an_applet_dir("g13-declare-a-name");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("applets/audio.json"),
            r#"{"name": "audio", "interval": 2, "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "{media_title}"}]}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("audio");

        // before: the name is read and nothing provides it
        let read = &window.applet_widgets()[0].reads[0];
        assert!(matches!(read.origin, Origin::Missing));

        // the picker's two steps: declare it in the applet's sources, then put it in the field
        window.add_applet_source("media_title", "media_title");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/audio.json")).unwrap())
                .unwrap();
        assert_eq!(written["sources"]["media_title"], "media_title");
        assert_eq!(window.applet_widgets()[0].reads[0].origin, Origin::Declared);
        assert!(
            window
                .applet_sources_used()
                .contains(&"media_title".to_string())
        );
    }

    #[test]
    fn a_bitmap_is_drawn_in_the_window_and_written_to_the_applets_own_file_only() {
        // Rows of dots and hashes are not something to type into a file. The picture is drawn by clicking cells
        // now, and it goes into the applet's own file - the shared one belongs to every applet, so an editor
        // writing to it would change what other applets draw without being asked.
        let dir = an_applet_dir("g13-bitmap-editor");
        // a directory of its own, cleaned first: the last run's written bitmaps would otherwise be the ones this
        // run starts from, and the test would pass alone and fail in company
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("applets/audio.json"),
            r#"{"name": "audio", "interval": 2, "sources": {}, "widgets": [
                 {"type": "bitmap", "x": 0, "y": 0, "name": "play"}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("bitmaps.json"),
            "{\"play\": {\"rows\": [\"#.\"]}, \"stop\": {\"rows\": [\"##\"]}}",
        )
        .unwrap();
        let shared_before = std::fs::read_to_string(dir.join("bitmaps.json")).unwrap();

        let mut window = Window::load(&dir);
        window.open_applet("audio");
        window.widget_editing = Some(0);

        // the name exists in the shared set, so that is what is loaded to start from
        window.refresh_bitmap("play");
        // `#.` is two pixels: one lit and one not, so the picture is 2 wide
        assert_eq!(
            window.bitmap_frames[0][0].len(),
            2,
            "the shared play is 2 wide"
        );
        assert_eq!(window.bitmap_frames.len(), 1, "a still bitmap is one frame");
        assert!(window.bitmap_frames[0][0][0]);

        // and drawing on it writes the applet's own file: bigger, with the pixels clicked
        window.set_bitmap_size(2, 2);
        window.set_bitmap_pixel(1, 0, true);
        window.set_bitmap_pixel(0, 1, true);
        let written = std::fs::read_to_string(dir.join("applets/audio.bitmaps.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(value["play"]["w"], 2);
        assert_eq!(value["play"]["h"], 2);
        // `#.` with the second pixel clicked is `##`, and the row that started blank with its first clicked is
        // `#.` - a cleared pixel is a dot, which is how the file spells "not lit"
        assert_eq!(value["play"]["rows"][0], "##");
        assert_eq!(value["play"]["rows"][1], "#.");
        // and it reads back as a bitmap, which is what the driver will do
        let back = g13_applets::read_bitmaps(&dir.join("applets"), "audio").0;
        assert_eq!(back["play"].w, 2);
        assert!(
            back["play"].frames[0][1][0],
            "the clicked pixel came back dark"
        );

        // **the shared file was not touched**: it is every applet's, not this one's
        assert_eq!(
            std::fs::read_to_string(dir.join("bitmaps.json")).unwrap(),
            shared_before,
            "the shared bitmaps file was written by an applet's editor"
        );

        // and another picture in the applet's own file survives an edit to this one
        window.set_bitmap_size(3, 1);
        let value: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("applets/audio.bitmaps.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(value["play"]["w"], 3);
    }

    #[test]
    fn a_source_used_on_a_screen_is_marked_as_used() {
        let dir = an_applet_dir("g13-sources-used");
        std::fs::write(
            dir.join("applets/stats.json"),
            r#"{"name": "stats", "interval": 2, "sources": {"cpu": "cpu", "spare": "cmd:uptime"},
                "screens": [{"title": "one", "widgets": [{"type": "text", "x": 0, "y": 0, "format": "{cpu}C"}]}]}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("stats");
        let used = window.applet_sources_used();
        assert!(
            used.contains(&"cpu".to_string()),
            "a source used by a widget on a screen is not marked as used: {used:?}"
        );
        assert!(
            !used.contains(&"spare".to_string()),
            "a source nothing reads is marked as used: {used:?}"
        );
    }

    #[test]
    fn adding_a_resource_from_a_widget_writes_the_source_and_takes_it_into_the_field() {
        // What the inspector's `add a resource` does, in the two steps it takes: the applet gains the source, and
        // the field that asked for it holds its name. A name typed into a widget that nothing declares is a name
        // drawn on the pad as itself.
        let dir = an_applet_dir("g13-add-resource");
        std::fs::write(
            dir.join("applets/temps.json"),
            r#"{"name": "temps", "interval": 2, "sources": {},
                "widgets": [{"type": "bar", "x": 0, "y": 0, "w": 40, "h": 5, "source": "", "max": 100}]}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("temps");
        window.widget_editing = Some(0);

        window.add_applet_source("gputemp", "cmd:cat /sys/class/hwmon/hwmon1/temp1_input");
        window.set_widget_text(0, "source", "gputemp");

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/temps.json")).unwrap())
                .unwrap();
        assert_eq!(
            written["sources"]["gputemp"],
            "cmd:cat /sys/class/hwmon/hwmon1/temp1_input"
        );
        assert_eq!(written["widgets"][0]["source"], "gputemp");
        // and the widget's reads row now says it is declared here, rather than nothing providing it
        let row = &window.applet_widgets()[0];
        assert!(
            row.reads
                .iter()
                .any(|read| read.name == "gputemp" && matches!(read.origin, Origin::Declared)),
            "the new source is not seen as declared: {:?}",
            row.reads
        );
    }

    #[test]
    fn the_menu_of_your_own_is_built_in_the_window_and_never_written_over_when_it_is_broken() {
        let dir = an_applet_dir("g13-menu-editor");
        let mut window = Window::load(&dir);

        // no file is not a problem: the pad's menu is the rotation until one is written
        window.refresh_menu();
        assert!(window.menu_problem.is_none(), "{:?}", window.menu_problem);
        assert!(window.menu_rows().is_empty());
        assert!(!window.menu_path().exists());

        // an item, which works before it is edited because it shows the clock
        window.new_menu_label = "Watch".to_string();
        window.add_menu_item();
        assert_eq!(window.menu_rows().len(), 1);
        assert_eq!(window.menu_rows()[0].label, "Watch");
        assert_eq!(window.menu_rows()[0].does, MenuDoes::Screen);
        assert!(window.menu_path().exists(), "the menu file was not written");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(window.menu_path()).unwrap()).unwrap();
        assert_eq!(written["items"][0]["label"], "Watch");
        assert_eq!(written["items"][0]["show"], "clock");
        // and the driver reads it back as the menu
        let menu = g13_config::menu::read_menu(&window.menu_path())
            .unwrap()
            .unwrap();
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0].problem(), None);

        // a level of its own, with an item under it
        window.add_menu_child(0);
        let rows = window.menu_rows();
        assert_eq!(rows.len(), 2, "the item under it is a row too");
        assert_eq!(rows[0].does, MenuDoes::List);
        assert_eq!(rows[1].at, (0, Some(0)));
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(window.menu_path()).unwrap()).unwrap();
        assert!(
            written["items"][0].get("show").is_none(),
            "a level cannot also act"
        );
        assert_eq!(written["items"][0]["items"][0]["show"], "clock");

        // a second item, moved, and one taken away
        window.new_menu_label = "Music".to_string();
        window.add_menu_item();
        // the rows are: Watch, the item under Watch, then Music - so the new one is addressed by what it is
        // called rather than by where it happens to sit
        let row = |window: &Window, label: &str| {
            window
                .menu_rows()
                .into_iter()
                .find(|row| row.label == label)
                .unwrap_or_else(|| panic!("{label} is not in the menu"))
        };
        assert_eq!(
            row(&window, "Music").at,
            (1, None),
            "Music is the second top-level item"
        );
        window.set_menu_does((1, None), MenuDoes::Command);
        window.set_menu_value((1, None), "cmd:playerctl play-pause");
        assert_eq!(row(&window, "Music").does, MenuDoes::Command);
        assert_eq!(row(&window, "Music").value, "cmd:playerctl play-pause");

        // moved up the top-level list, and then taken away
        window.move_menu_item((1, None), true);
        assert_eq!(
            window.menu_rows()[0].label,
            "Music",
            "the move did not land: {:?}",
            window
                .menu_rows()
                .iter()
                .map(|row| &row.label)
                .collect::<Vec<_>>()
        );
        window.remove_menu_item((0, None));
        assert_eq!(window.menu_rows()[0].label, "Watch");
        assert!(window.menu_rows().iter().all(|row| row.label != "Music"));

        // a file that will not read is reported, and nothing is written over it
        std::fs::write(window.menu_path(), r#"{"items": [{"label": "broken"}]}"#).unwrap();
        window.refresh_menu();
        assert!(
            window
                .menu_problem
                .clone()
                .unwrap_or_default()
                .contains("broken"),
            "a broken menu file was not reported: {:?}",
            window.menu_problem
        );
        let untouched = std::fs::read_to_string(window.menu_path()).unwrap();
        assert!(
            untouched.contains("broken"),
            "the file was changed: {untouched}"
        );
        // and a change made in that state is refused rather than written over it
        window.set_menu_does((0, None), MenuDoes::Command);
        assert!(
            window
                .menu_problem
                .clone()
                .unwrap_or_default()
                .contains("broken")
        );
        assert_eq!(
            std::fs::read_to_string(window.menu_path()).unwrap(),
            untouched,
            "the file was written over while it was unreadable"
        );
    }

    #[test]
    fn a_new_applet_is_written_once_and_never_over_a_file_that_is_there() {
        let dir = an_applet_dir("g13-new-applet");
        let mut window = Window::load(&dir);
        assert!(window.applet_names().is_empty());
        // what the window is complaining about, cloned so the window is still usable afterwards
        let said = |window: &Window| window.applet_problem.clone().unwrap_or_default();

        // an empty name is refused by name, because a name is what a file is called
        window.create_applet();
        assert!(
            said(&window).contains("needs a name"),
            "an empty name was accepted"
        );

        window.new_applet_name = "my audio".to_string();
        window.create_applet();
        assert_eq!(window.applet_names(), vec!["my audio"]);
        // it is opened, so the next thing typed goes into it
        assert_eq!(window.applet_editing.as_deref(), Some("my audio"));
        assert_eq!(window.screen_count(), 1);
        // what was written reads back as a real applet
        let written: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("applets/my audio.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(written["name"], "my audio");
        assert_eq!(written["interval"], 2);
        assert!(written["widgets"].as_array().unwrap().is_empty());
        assert_eq!(
            g13_applets::parse_applet(&written.to_string())
                .unwrap()
                .name,
            "my audio"
        );

        // and the existing file is not written over: say so and leave it alone
        std::fs::write(
            dir.join("applets/my audio.json"),
            r#"{"name": "my audio", "interval": 9}"#,
        )
        .unwrap();
        window.new_applet_name = "my audio".to_string();
        window.create_applet();
        assert!(
            said(&window).contains("already"),
            "an existing applet was written over"
        );
        let untouched = std::fs::read_to_string(dir.join("applets/my audio.json")).unwrap();
        assert!(untouched.contains("9"), "the file was changed: {untouched}");

        // a name that is not a sane file name is refused with the character that is wrong
        window.new_applet_name = "a/b".to_string();
        window.create_applet();
        assert!(
            said(&window).contains('/'),
            "a separator in the name was accepted"
        );
        assert_eq!(window.applet_names().len(), 1, "nothing extra was written");
    }

    #[test]
    fn every_kind_offers_the_command_the_pad_runs() {
        // It was a box under the screen's name and belonged to nothing. The command is a field of the widget
        // itself, so any widget can be one of the things L2/L3 move through.
        let dir = an_applet_dir("g13-command-field");
        std::fs::write(
            dir.join("applets/one.json"),
            r#"{"name": "one", "sources": {}, "widgets": [
                 {"type": "text", "x": 0, "y": 0, "format": "Start/Pause"}]}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("one");

        for kind in g13_applets::WIDGET_KINDS {
            assert!(
                widget_fields_for(kind)
                    .iter()
                    .any(|(key, _)| *key == "command"),
                "{kind} cannot be given a command"
            );
        }
        let row = &window.applet_widgets()[0];
        assert!(
            row.fields.iter().any(|field| field.key == "command"),
            "the fields the window draws do not include command: {:?}",
            row.fields.iter().map(|f| f.key).collect::<Vec<_>>()
        );

        window.set_widget_optional_text(0, "command", Some("cmd:playerctl play-pause"));
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/one.json")).unwrap())
                .unwrap();
        assert_eq!(written["widgets"][0]["command"], "cmd:playerctl play-pause");

        // emptied means the key goes, because no command and a command that runs nothing are different
        window.set_widget_optional_text(0, "command", None);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/one.json")).unwrap())
                .unwrap();
        assert!(written["widgets"][0].get("command").is_none());
    }
    #[test]
    fn a_second_screen_keeps_what_the_applet_had_and_takes_the_edits() {
        let dir = an_applet_dir("g13-screens-gui");
        let path = dir.join("applets").join("two.json");
        std::fs::write(
            &path,
            r#"{"name": "two", "interval": 2, "sources": {}, "widgets": [
                 {"type": "text", "x": 1, "y": 1, "format": "FIRST"}]}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        window.open_applet("two");
        assert_eq!(window.screen_count(), 1, "it starts as one screen");
        assert_eq!(window.applet_widgets().len(), 1);

        window.add_screen();
        assert_eq!(window.screen_count(), 2);
        // the widgets moved into screen 1 rather than being copied
        assert_eq!(window.screen_editing, 1, "it moves to the new screen");
        assert!(
            window.applet_widgets().is_empty(),
            "the new screen starts empty"
        );

        // what is written to the file: two screens, the first holding what was there
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(written.get("widgets").is_none(), "the old shape is gone");
        assert_eq!(written["screens"].as_array().unwrap().len(), 2);
        assert_eq!(
            written["screens"][0]["widgets"].as_array().unwrap().len(),
            1
        );

        // an edit on screen 2 lands on screen 2, not on the applet's own array
        window.add_widget("text");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written["screens"][1]["widgets"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            written["screens"][0]["widgets"].as_array().unwrap().len(),
            1
        );

        // naming one writes the name
        window.set_screen_title(1, "images");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["screens"][1]["title"], "images");

        // and taking one away leaves the other as the applet's own widgets again
        window.remove_screen();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            written.get("screens").is_none(),
            "one screen is a plain applet"
        );
        assert_eq!(written["widgets"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn adding_and_removing_an_endpoint_writes_the_file() {
        let dir = a_config_dir("g13-gui-endpoints-add");
        std::fs::write(
            dir.join("endpoints.json"),
            r#"{"weather": {"url": "http://wttr.in", "insecure": true}}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        assert_eq!(window.endpoints.len(), 1);
        assert!(window.endpoints["weather"].insecure, "read from the file");

        window.add_endpoint("github", "https://api.github.com");
        let written = std::fs::read_to_string(dir.join("endpoints.json")).unwrap();
        assert!(written.contains("github"), "{written}");
        // and the one already there is untouched, including the flag that matters
        assert!(written.contains("wttr.in"), "{written}");
        assert!(written.contains("insecure"), "{written}");
        assert_eq!(window.endpoints.len(), 2);

        window.remove_endpoint("weather");
        let written = std::fs::read_to_string(dir.join("endpoints.json")).unwrap();
        assert!(
            !written.contains("wttr.in"),
            "the removal should be written: {written}"
        );
        assert!(written.contains("github"));
        // and reading it again agrees with what the window holds
        let again = Window::load(&dir);
        assert_eq!(again.endpoints.len(), 1);
        assert!(again.endpoints.contains_key("github"));
    }

    #[test]
    fn a_duplicate_endpoint_name_is_refused_rather_than_overwritten() {
        // the name is what every applet refers to it by, so replacing one silently would break every applet
        // using it - and lose whatever credential it held
        let dir = a_config_dir("g13-gui-endpoints-duplicate");
        std::fs::write(
            dir.join("endpoints.json"),
            r#"{"github": {"url": "https://api.github.com", "token": "keep-me"}}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        window.add_endpoint("github", "https://somewhere.else");
        assert!(
            window.status.contains("already an endpoint called github"),
            "it should say why: {}",
            window.status
        );
        assert_eq!(window.endpoints["github"].url, "https://api.github.com");
        assert_eq!(window.endpoints["github"].token.as_deref(), Some("keep-me"));
    }

    #[test]
    fn a_token_is_written_but_never_put_on_the_status_line() {
        // the status line is the one part of this window that is plain text on a screen and in a log, and the
        // file is the only place a credential belongs
        let dir = a_config_dir("g13-gui-endpoints-token");
        std::fs::write(dir.join("endpoints.json"), "{}").unwrap();
        let mut window = Window::load(&dir);
        window.add_endpoint("github", "https://api.github.com");
        let secret = "ghp_a_token_that_must_not_be_echoed";
        window.endpoints.get_mut("github").unwrap().token = Some(secret.to_string());
        window.commit_endpoints();

        assert!(
            std::fs::read_to_string(dir.join("endpoints.json"))
                .unwrap()
                .contains(secret),
            "the token must reach the file"
        );
        assert!(
            !window.status.contains(secret),
            "the token reached the status line: {}",
            window.status
        );
        assert!(
            !window.status.contains("ghp_"),
            "even part of it: {}",
            window.status
        );
        // and reloading still masks it rather than exposing it
        assert_eq!(window.revealing, None);
    }

    fn an_applet_dir(name: &str) -> PathBuf {
        let dir = a_config_dir(name);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        dir
    }

    const AN_APPLET: &str = r#"{
      "name": "x",
      "title": "X",
      "interval": 2,
      "border": true,
      "colour": "255,0,0",
      "follow": "a key this build does not act on",
      "sources": {"cpu": "cpu", "ram": "cmd:uptime"},
      "widgets": [
        {"type": "text", "x": 3, "y": 12, "format": "cpu {cpu}C"},
        {"type": "bar", "x": 3, "y": 30, "w": 100, "h": 6, "source": "ram", "max": 100}
      ]
    }"#;

    #[test]
    fn editing_an_applet_keeps_every_key_it_already_had() {
        // Applet files are hand-written and hold keys this build recognises but does not act on. A designer that
        // read one into a struct and wrote it back would drop them, which would be editing the file into
        // something else - so the designer edits the file's own JSON.
        let dir = an_applet_dir("g13-gui-applet-keys");
        std::fs::write(dir.join("applets/x.json"), AN_APPLET).unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("x");
        window.set_applet_text("title", "TEMPS");

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert_eq!(written["title"], "TEMPS", "the change");
        assert_eq!(written["colour"], "255,0,0", "a key this build ignores");
        assert_eq!(written["follow"], "a key this build does not act on");
        assert_eq!(written["interval"], 2);
        assert_eq!(written["sources"]["cpu"], "cpu");
        assert_eq!(written["sources"]["ram"], "cmd:uptime");
        assert_eq!(written["widgets"][0]["format"], "cpu {cpu}C");
        assert_eq!(written["widgets"][1]["max"], 100);
        // and a number stays a number rather than becoming a string
        window.set_applet_number("interval", 5.0);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert_eq!(written["interval"], 5);
        assert!(written["interval"].is_number(), "{written}");
    }

    const WIDGETS: &str = r#"{
      "name": "x", "title": "X", "interval": 2,
      "sources": {"cpu": "cpu", "ram": "cmd:uptime"},
      "widgets": [
        {"type": "text", "x": 3, "y": 12, "align": "left", "format": "cpu {cpu}C"},
        {"type": "bar", "x": 3, "y": 30, "w": 100, "h": 6, "source": "ram", "max": 100},
        {"type": "arrow", "x": 60, "y": 20, "r": 12, "source": "cpu"},
        {"type": "something-new", "x": 1, "y": 2, "whatever": "kept"}
      ]
    }"#;

    fn window_with_widgets(name: &str) -> (PathBuf, Window) {
        let dir = an_applet_dir(name);
        std::fs::write(dir.join("applets/x.json"), WIDGETS).unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("x");
        (dir, window)
    }

    #[test]
    fn each_kind_of_widget_is_read_under_the_keys_the_reader_uses() {
        // An arrow's radius is `r`, not `radius`, and a text widget's x may be left out. These are read from the
        // reader rather than guessed, and this is the check that they stay that way.
        let (_dir, window) = window_with_widgets("g13-gui-widget-fields");
        let rows = window.applet_widgets();
        assert_eq!(rows.len(), 4);

        let text = &rows[0];
        assert_eq!(text.kind, "text");
        let keys: Vec<&str> = text.fields.iter().map(|field| field.key).collect();
        // `command` is last on every kind, because any widget can be one the pad acts on
        assert_eq!(
            keys,
            vec![
                "x",
                "y",
                "align",
                "format",
                "scroll_width",
                // the previous stack's two, acted on now: run the text rather than cut it, and how fast
                "scroll",
                "scroll_speed",
                "command",
                // every kind this build draws may be told what to do when it moves
                "animate",
            ]
        );
        assert!(matches!(text.fields[3].value, FieldValue::Text(ref held) if held == "cpu {cpu}C"));

        let bar = &rows[1];
        assert_eq!(bar.kind, "bar");
        assert!(matches!(bar.fields[5].value, FieldValue::Number(held) if held == 100.0));

        let arrow = &rows[2];
        assert_eq!(arrow.kind, "arrow");
        let keys: Vec<&str> = arrow.fields.iter().map(|field| field.key).collect();
        assert_eq!(
            keys,
            vec!["x", "y", "r", "source", "command", "animate"],
            "an arrow's radius is `r`"
        );
        assert!(matches!(arrow.fields[2].value, FieldValue::Number(held) if held == 12.0));
    }

    #[test]
    fn editing_one_widget_field_keeps_every_other_key_of_that_widget_and_of_the_others() {
        let (dir, mut window) = window_with_widgets("g13-gui-widget-edit");
        window.set_widget_number(1, "w", 64.0);

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert_eq!(written["widgets"][1]["w"], 64);
        assert_eq!(written["widgets"][1]["h"], 6, "the rest of the widget");
        assert_eq!(written["widgets"][1]["source"], "ram");
        assert_eq!(written["widgets"][1]["max"], 100);
        assert_eq!(
            written["widgets"][0]["format"], "cpu {cpu}C",
            "another widget entirely"
        );
        assert_eq!(written["sources"]["cpu"], "cpu", "and the sources");
        // a whole number is written as a whole number
        assert!(written["widgets"][1]["w"].is_i64(), "{written}");
    }

    #[test]
    fn a_field_that_may_be_left_out_can_be_taken_out_and_put_back() {
        // A text widget with no x is anchored by its alignment, which is how the HUD applet writes its level
        // readout - so leaving it out has to be expressible rather than becoming a zero.
        let (dir, mut window) = window_with_widgets("g13-gui-widget-optional");
        let rows = window.applet_widgets();
        assert!(matches!(
            rows[0].fields[4].value,
            FieldValue::OptionalNumber(None)
        ));

        window.set_widget_optional_number(0, "scroll_width", Some(80.0));
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert_eq!(written["widgets"][0]["scroll_width"], 80);

        window.set_widget_optional_number(0, "x", None);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert!(
            written["widgets"][0].get("x").is_none(),
            "taking a field out should remove it, not write a null: {written}"
        );
        // and the applet still reads back
        assert!(g13_applets::parse_applet(&written.to_string()).is_ok());
    }

    #[test]
    fn a_preview_showing_something_that_moves_is_redrawn_at_the_rate_the_driver_draws_at() {
        // The preview of a running text is jittery. The driver draws a screen with a running text on it twenty
        // times a second; a window that repaints once a second is dropping nineteen frames, which is the jitter.
        // The Bindings tab's frame rate is never taken away by this.
        let (dir, mut window) = window_with_widgets("g13-gui-preview-rate");
        // the fixture's text widget, given a width and told to run
        window.tab = Tab::Applets;
        window.applet_editing = Some("x".to_string());
        assert_eq!(
            repaint_interval_for(&window),
            std::time::Duration::from_secs(1),
            "a preview of a still applet should stay cheap"
        );
        window.set_widget_optional_number(0, "scroll_width", Some(22.0));
        window.set_widget_flag(0, "scroll", true);
        assert_eq!(
            repaint_interval_for(&window),
            std::time::Duration::from_millis(50),
            "a preview with a running text on it must be redrawn at the driver's rate"
        );
        // and the tab that watches a hand on the stick keeps its own frame rate
        window.tab = Tab::Bindings;
        assert_eq!(
            repaint_interval_for(&window),
            std::time::Duration::from_millis(16)
        );
        // no applet being edited: whatever the tab says
        window.applet_editing = None;
        assert_eq!(
            repaint_interval_for(&window),
            std::time::Duration::from_millis(16)
        );
        drop(dir);
    }

    #[test]
    fn a_theme_edited_in_the_window_is_written_at_once_and_reads_back() {
        // the driver re-reads a theme as it draws, so what matters is the file: an edit that is not written is an
        // edit that is not on the pad
        let (dir, mut window) = window_with_widgets("g13-gui-theme-edit");
        window.new_theme("cyber");
        let path = g13_agent::visuals::themes_dir(&dir.join("applets")).join("cyber.json");
        let written = |path: &std::path::Path| std::fs::read_to_string(path).unwrap();

        window.open_theme("cyber");
        assert_eq!(window.theme_editing.as_deref(), Some("cyber"));
        assert_eq!(window.theme_problem, None);
        let start = g13_applets::Theme::from_json(&written(&path), "cyber").unwrap();
        assert_eq!(
            start.scanline,
            Some(40.0),
            "the starting theme has a scanline to see"
        );

        // changing a number writes the file and the theme reads back with it
        window.set_theme("scanline", serde_json::json!(90.0));
        let after = g13_applets::Theme::from_json(&written(&path), "cyber").unwrap();
        assert_eq!(after.scanline, Some(90.0));

        // taking it to zero removes the key rather than writing a zero
        window.set_theme("scanline", serde_json::Value::Null);
        let file: serde_json::Value = serde_json::from_str(&written(&path)).unwrap();
        assert!(
            file.get("scanline").is_none(),
            "absent is what off means: {file}"
        );
        assert_eq!(
            g13_applets::Theme::from_json(&written(&path), "cyber")
                .unwrap()
                .scanline,
            None
        );
        // it still moves, because the theme it started from also has a glitch; clear that and nothing does
        window.set_theme("glitch", serde_json::Value::Null);
        assert!(
            !g13_applets::Theme::from_json(&written(&path), "cyber")
                .unwrap()
                .moves()
        );

        // an inner setting, and its object still parses
        window.set_theme_inner("glitch", "every", serde_json::json!(3.0));
        let after = g13_applets::Theme::from_json(&written(&path), "cyber").unwrap();
        assert_eq!(after.glitch, Some((3.0, 1)));
        // and taking the last one away leaves no glitch, not one on the defaults
        window.set_theme_inner("glitch", "every", serde_json::Value::Null);
        window.set_theme_inner("glitch", "shake", serde_json::Value::Null);
        let after = g13_applets::Theme::from_json(&written(&path), "cyber").unwrap();
        assert_eq!(after.glitch, None, "an empty glitch is no glitch");

        // the list says what a theme asks for, which is how two of them are told apart without opening either
        window.set_theme("scanline", serde_json::json!(50.0));
        let rows = window.theme_rows();
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0]
                .1
                .clone()
                .unwrap_or_default()
                .contains("scanline 50")
        );
    }

    #[test]
    fn a_question_survives_being_written_and_read_back() {
        // A reload used to break the question. This walks the whole round trip the window does - write it, reload
        // the applet the way opening it again does, and read the question back - because the fault is somewhere
        // in that path and guessing which step would be a guess.
        let (dir, mut window) = window_with_widgets("g13-gui-alert-reload");
        window.open_applet("x");
        window.set_widget_alert_look(0, "flash");
        let asked = crate::macros::ConditionForm {
            all: true,
            rows: vec![crate::macros::QuestionRow {
                not: false,
                question: crate::macros::Question::Value {
                    name: "cpu".to_string(),
                    op: g13_values::Compare::Gt,
                    number: Some(80.0),
                    text: None,
                },
            }],
        };
        window.set_widget_alert_question(0, &asked);
        assert_eq!(
            window.widget_alert_question(0).map(|form| form.describe()),
            Some(asked.describe()),
            "it should read back without reloading too"
        );

        // now the same thing the window does when the applet is opened again
        let mut again = Window::load(&dir);
        again.open_applet("x");
        let back = again
            .widget_alert_question(0)
            .expect("the question should still be there after a reload");
        assert_eq!(
            back.describe(),
            asked.describe(),
            "the question did not survive: {}",
            std::fs::read_to_string(dir.join("applets/x.json")).unwrap()
        );

        // and the default a new alert starts from is a question that can actually be true: a control question is
        // always false in an applet, so offering one is offering something that cannot work
        let fresh = again.new_alert_question();
        assert!(
            !matches!(
                fresh.rows.first().map(|row| &row.question),
                Some(crate::macros::Question::Control(_))
            ),
            "a new alert should not start as a control question: {fresh:?}"
        );
    }

    #[test]
    fn an_alert_set_in_the_window_is_one_the_driver_parses_and_draws() {
        // This is the row that sets an alert, and what matters is not that JSON lands in the file but that the
        // applet the driver loads sees the alert, with the look and the question chosen.
        let (dir, mut window) = window_with_widgets("g13-gui-alert");
        window.open_applet("x");
        assert_eq!(
            window.widget_alert_look(0),
            "",
            "a widget starts with no alert"
        );
        assert!(window.widget_alert_question(0).is_none());

        window.set_widget_alert_look(0, "flash");
        let names = window.applet_source_names();
        let first = names
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "cpu".to_string());
        window.set_widget_alert_question(
            0,
            &crate::macros::ConditionForm {
                all: true,
                rows: vec![crate::macros::QuestionRow {
                    not: false,
                    question: crate::macros::Question::Value {
                        name: first.clone(),
                        op: g13_values::Compare::Gt,
                        number: Some(80.0),
                        text: None,
                    },
                }],
            },
        );

        // what the driver loads is the judge, not the text
        let written = std::fs::read_to_string(dir.join("applets/x.json")).unwrap();
        assert!(
            written.contains("\"alert\""),
            "the row should have written an alert at all: {written}"
        );
        let applet = g13_applets::parse_applet(&written).expect("the file still reads");
        // the fixture's fourth widget is deliberately a kind this build does not draw, so the check is about the
        // widget the alert was put on rather than about the whole file
        assert!(
            !applet
                .unhandled
                .iter()
                .any(|complaint| complaint.starts_with("widget 0")),
            "the driver should have nothing to say about this widget: {:?}",
            applet.unhandled
        );
        let alert = applet.alert(0, 0).expect("the driver should see an alert");
        assert_eq!(alert.look, g13_applets::AlertLook::Flash);
        assert_eq!(
            alert.condition,
            g13_values::parse_cond(&serde_json::json!({
                "value": first, "op": ">", "to": 80
            }))
            .unwrap(),
            "the question should be the one that was written"
        );
        assert!(applet.moves(0), "an alert can flash, so the screen moves");
        // and the window can read its own question back, which is what the row shows
        assert!(window.widget_alert_question(0).is_some());
        let question_before = window
            .widget_alert_question(0)
            .expect("a question")
            .describe();

        // changing the look keeps the question: it used to throw it away, which nobody notices until they have
        // written a long one
        window.set_widget_alert_look(0, "border");
        assert_eq!(
            window.widget_alert_question(0).map(|form| form.describe()),
            Some(question_before),
            "the question should survive the look changing"
        );

        // `show` takes nothing else: the widget is drawn only while its question is true, and several widgets
        // in one place take turns
        window.set_widget_alert_look(0, "show");
        let written = std::fs::read_to_string(dir.join("applets/x.json")).unwrap();
        let applet = g13_applets::parse_applet(&written).unwrap();
        assert_eq!(
            applet.alert(0, 0).map(|alert| alert.look),
            Some(g13_applets::AlertLook::Show)
        );

        // and taking the look back to none takes the whole alert out, rather than leaving an empty object behind
        window.set_widget_alert_look(0, "");
        let written = std::fs::read_to_string(dir.join("applets/x.json")).unwrap();
        assert!(
            !written.contains("\"alert\""),
            "no alert should be left in the file: {written}"
        );
        assert_eq!(window.widget_alert_look(0), "");
    }

    #[test]
    fn a_theme_made_in_the_window_reads_back_and_is_applied_at_once() {
        // This window's rule: auto-apply, no confirm buttons. So making a theme names it on the applet in
        // the same breath, and the file it writes is one the driver can load.
        let (dir, mut window) = window_with_widgets("g13-gui-new-theme");
        assert!(window.themes().is_empty(), "no themes to start with");
        window.new_theme("cyberpunk-2077");
        assert_eq!(window.theme_problem, None);
        assert_eq!(
            window.applet_field("theme"),
            "cyberpunk-2077",
            "the applet should be wearing the theme that was just made"
        );
        assert_eq!(window.themes(), vec!["cyberpunk-2077".to_string()]);
        let written = std::fs::read_to_string(
            g13_agent::visuals::themes_dir(&dir.join("applets")).join("cyberpunk-2077.json"),
        )
        .unwrap();
        let theme = g13_applets::Theme::from_json(&written, "cyberpunk-2077")
            .expect("a theme the driver can read");
        assert!(theme.moves(), "a new theme should show what a theme is for");
        // and the applet that names it is one the driver would draw as moving
        let applet = g13_applets::with_theme(
            g13_applets::parse_applet(&window.applet_json.to_string()).unwrap(),
            &window.themes_dir(),
        );
        assert!(applet.moves(0));

        // a name already taken is refused rather than written over
        let before = written.clone();
        window.new_theme("cyberpunk-2077");
        assert!(
            window
                .theme_problem
                .clone()
                .unwrap_or_default()
                .contains("already a theme")
        );
        assert_eq!(
            std::fs::read_to_string(
                g13_agent::visuals::themes_dir(&dir.join("applets")).join("cyberpunk-2077.json")
            )
            .unwrap(),
            before
        );
        // and a name that is not a file name is refused with the character in it
        window.new_theme("a/b");
        assert!(
            window
                .theme_problem
                .clone()
                .unwrap_or_default()
                .contains('/')
        );
    }

    #[test]
    fn the_scroll_box_writes_a_scroll_the_driver_would_actually_draw() {
        // Ticking the box has to make both the window and the driver agree. The tick box uses this call, and
        // what matters is not that a key lands in the file but that the applet that comes back says it moves -
        // the driver's draw rate asks that question and nothing else.
        let (dir, mut window) = window_with_widgets("g13-gui-scroll-tick");
        window.set_widget_flag(0, "scroll", true);
        window.set_widget_optional_number(0, "scroll_width", Some(22.0));
        window.set_widget_number(0, "scroll_speed", 30.0);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert_eq!(
            written["widgets"][0]["scroll"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(written["widgets"][0]["scroll_width"], 22.0);
        let applet = g13_applets::parse_applet(&written.to_string()).unwrap();
        assert!(
            applet.widgets.iter().any(g13_applets::Widget::scrolls),
            "the window wrote a scroll the driver would not draw as one: {written}"
        );
        // unticking says no in the file rather than taking the key away, so what was chosen stays visible
        window.set_widget_flag(0, "scroll", false);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert_eq!(
            written["widgets"][0]["scroll"],
            serde_json::Value::Bool(false)
        );
        let applet = g13_applets::parse_applet(&written.to_string()).unwrap();
        assert!(!applet.widgets.iter().any(g13_applets::Widget::scrolls));
    }

    #[test]
    fn a_widget_type_this_build_does_not_draw_is_kept_exactly_as_it_is() {
        let (dir, mut window) = window_with_widgets("g13-gui-widget-unknown");
        let rows = window.applet_widgets();
        assert!(rows[3].unknown, "an unknown type has no fields to edit");
        assert!(rows[3].fields.is_empty());

        // editing another widget must not touch it
        window.set_widget_text(0, "format", "changed");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert_eq!(written["widgets"][3]["type"], "something-new");
        assert_eq!(written["widgets"][3]["whatever"], "kept");
        assert_eq!(written["widgets"][3]["x"], 1);
    }

    #[test]
    fn every_value_a_widget_reads_says_where_it_comes_from() {
        let dir = an_applet_dir("g13-gui-reads-origins");
        std::fs::write(
            dir.join("applets/mine.json"),
            r#"{"name":"mine","title":"MINE","interval":1,"border":true,
                "sources":{"mine":"cmd:uptime"},
                "widgets":[
                    {"type":"text","x":3,"y":2,"format":"cpu {cpu} of {mine} and {nothing}"},
                    {"type":"bar","x":3,"y":12,"w":10,"h":5,"source":"gpu","max":100}]}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("mine");
        let rows = window.applet_widgets();
        let named = |row: &WidgetRow| -> Vec<(String, Origin)> {
            row.reads
                .iter()
                .map(|read| (read.name.clone(), read.origin))
                .collect()
        };
        // `cpu` is a built-in name, but this applet does not declare it, so nothing provides it: an applet's
        // values are the ones it declares and no others, and saying otherwise was the old behaviour
        assert_eq!(
            named(&rows[0]),
            vec![
                ("cpu".to_string(), Origin::Missing),
                ("mine".to_string(), Origin::Declared),
                ("nothing".to_string(), Origin::Missing),
            ]
        );
        // a bar reads its source by name, and nothing provides this one
        assert_eq!(named(&rows[1]), vec![("gpu".to_string(), Origin::Missing)]);

        // the shape of `temps`: the applet declares its own `cpu`, which is also a built-in, and its own wins
        std::fs::write(
            dir.join("applets/temps.json"),
            r#"{"name":"temps","title":"TEMPS","interval":2,"border":true,
                "sources":{"cpu":"cmd:cat /sys/class/hwmon/hwmon0/temp1_input"},
                "widgets":[{"type":"text","x":3,"y":2,"format":"{cpu}C"}]}"#,
        )
        .unwrap();
        window.open_applet("temps");
        let rows = window.applet_widgets();
        assert_eq!(named(&rows[0]), vec![("cpu".to_string(), Origin::Declared)]);

        // and the shape of an applet that declares everything it uses
        std::fs::write(
            dir.join("applets/stats.json"),
            r#"{"name":"stats","title":"STATS","interval":1,"border":true,
                "sources":{"cpu":"cmd:echo 11","memory":"cmd:echo 42"},
                "widgets":[{"type":"text","x":3,"y":2,"format":"{cpu:.0f}% {memory:.0f}%"}]}"#,
        )
        .unwrap();
        window.open_applet("stats");
        let rows = window.applet_widgets();
        assert_eq!(
            named(&rows[0]),
            vec![
                ("cpu".to_string(), Origin::Declared),
                ("memory".to_string(), Origin::Declared),
            ]
        );
    }

    #[test]
    fn moving_a_widget_moves_it_and_leaves_every_key_of_it_alone() {
        let (dir, mut window) = window_with_widgets("g13-gui-widget-move");
        let kinds = |window: &Window| -> Vec<String> {
            window
                .applet_widgets()
                .iter()
                .map(|row| row.kind.clone())
                .collect()
        };
        assert_eq!(
            kinds(&window),
            vec!["text", "bar", "arrow", "something-new"]
        );

        // the bar moves up in front of the text, and every field it has comes with it
        window.move_widget(1, true);
        assert_eq!(
            kinds(&window),
            vec!["bar", "text", "arrow", "something-new"]
        );
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert_eq!(
            written["widgets"][0]["source"], "ram",
            "the bar's own fields came with it"
        );
        assert_eq!(written["widgets"][0]["max"], 100);
        assert_eq!(
            written["widgets"][1]["format"], "cpu {cpu}C",
            "and the text kept its format"
        );

        // and back down
        window.move_widget(0, false);
        assert_eq!(
            kinds(&window),
            vec!["text", "bar", "arrow", "something-new"]
        );
    }

    #[test]
    fn the_first_widget_cannot_move_up_and_the_last_cannot_move_down() {
        // the buttons are what says whether there is anywhere to go, so the ends do nothing rather than wrapping
        // round, which would move a widget to the other end of the screen
        let (_dir, mut window) = window_with_widgets("g13-gui-widget-move-ends");
        let kinds = |window: &Window| -> Vec<String> {
            window
                .applet_widgets()
                .iter()
                .map(|row| row.kind.clone())
                .collect()
        };
        let before = kinds(&window);
        window.move_widget(0, true);
        assert_eq!(kinds(&window), before);
        window.move_widget(3, false);
        assert_eq!(kinds(&window), before);
        // and an index past the end is not a panic
        window.move_widget(99, true);
        assert_eq!(kinds(&window), before);
    }

    #[test]
    fn removing_a_widget_removes_that_one_and_leaves_the_rest_in_order() {
        let (dir, mut window) = window_with_widgets("g13-gui-widget-remove");
        window.remove_widget(1);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        let kinds: Vec<&str> = written["widgets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|widget| widget["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, vec!["text", "arrow", "something-new"]);
    }

    #[test]
    fn adding_a_widget_makes_one_that_reads_back_and_lands_somewhere_visible() {
        let (dir, mut window) = window_with_widgets("g13-gui-widget-add");
        // The file starts with four widgets, one of them a kind this build does not draw, so the reader sees
        // three of them while the file holds four.
        for (position, kind) in WIDGET_KINDS.iter().enumerate() {
            window.add_widget(kind);
            let text = std::fs::read_to_string(dir.join("applets/x.json")).unwrap();
            let written: serde_json::Value = serde_json::from_str(&text).unwrap();
            let widgets = written["widgets"].as_array().unwrap();
            assert_eq!(widgets.len(), 4 + position + 1, "{kind} was not added");
            let last = widgets.last().unwrap();
            assert_eq!(last["type"], *kind, "the wrong kind was added");
            // placed inside the screen rather than off it
            let y = last["y"].as_f64().unwrap();
            assert!(
                y < g13_screen::VISIBLE_HEIGHT as f64,
                "{kind} was placed at y {y}, off a screen that many pixels tall"
            );
            // and the whole applet still reads back, with the new widget among the ones it draws
            let applet = g13_applets::parse_applet(&text)
                .unwrap_or_else(|problem| panic!("{kind} did not read back: {problem}"));
            assert_eq!(
                applet.widgets.len(),
                3 + position + 1,
                "{kind} should be a widget the reader draws"
            );
        }
        // and a kind this build does not draw is refused rather than written as a broken widget
        window.add_widget("nonsense");
        assert!(
            window.status.contains("not a widget this build draws"),
            "{}",
            window.status
        );
    }

    #[test]
    fn an_applet_that_would_not_read_back_is_left_alone() {
        // A file that does not parse leaves the pad's screen blank, and blank looks exactly like an applet that
        // draws nothing - so nothing is written until what is written can be read again.
        let dir = an_applet_dir("g13-gui-applet-unwritable");
        std::fs::write(dir.join("applets/x.json"), AN_APPLET).unwrap();
        let before = std::fs::read_to_string(dir.join("applets/x.json")).unwrap();

        let mut window = Window::load(&dir);
        window.open_applet("x");
        // a source written as a number rather than as text: a real failure the reader names
        window.applet_json["sources"]["cpu"] = serde_json::json!(42);
        window.commit_applet();

        assert!(
            window.applet_problem.is_some(),
            "the reason should be kept: {}",
            window.status
        );
        assert!(
            window
                .applet_problem
                .as_deref()
                .unwrap_or_default()
                .contains("cpu"),
            "the reason should name what is wrong: {:?}",
            window.applet_problem
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("applets/x.json")).unwrap(),
            before,
            "the file must not have been touched"
        );
    }

    #[test]
    fn a_source_is_added_removed_and_duplicates_refused() {
        let dir = an_applet_dir("g13-gui-applet-sources");
        std::fs::write(dir.join("applets/x.json"), AN_APPLET).unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("x");
        assert_eq!(window.applet_sources().len(), 2);

        window.add_applet_source("disk", "cmd:df -h /");
        assert_eq!(window.applet_sources().len(), 3);
        assert!(
            window
                .applet_sources()
                .iter()
                .any(|(name, spec)| name == "disk" && spec == "cmd:df -h /")
        );

        // a name already used is refused rather than replaced: every widget naming it would start reading
        // something else, silently
        window.add_applet_source("cpu", "cmd:rm -rf");
        assert!(
            window.status.contains("already has a source called cpu"),
            "{}",
            window.status
        );
        assert!(
            window
                .applet_sources()
                .iter()
                .any(|(name, spec)| name == "cpu" && spec == "cpu")
        );

        window.remove_applet_source("ram");
        assert_eq!(window.applet_sources().len(), 2);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert!(written["sources"].get("ram").is_none(), "{written}");
        assert!(written["sources"].get("disk").is_some());
    }

    #[test]
    fn a_source_nothing_reads_is_told_apart_from_one_a_widget_uses() {
        // cmds and fetches cost something every redraw, so a source that draws nothing is worth seeing
        let dir = an_applet_dir("g13-gui-applet-used");
        std::fs::write(dir.join("applets/x.json"), AN_APPLET).unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("x");
        let used = window.applet_sources_used();
        assert!(used.contains(&"cpu".to_string()), "{used:?}");
        assert!(
            used.contains(&"ram".to_string()),
            "a bar names its source: {used:?}"
        );

        // and one nothing refers to is not in the list
        window.add_applet_source("spare", "cmd:uptime");
        let used = window.applet_sources_used();
        assert!(!used.contains(&"spare".to_string()), "{used:?}");
    }

    #[test]
    fn a_widget_type_this_build_cannot_draw_is_still_written_and_reported() {
        // Not the same as a broken file: `unhandled` exists so a key this build does not act on is reported
        // rather than dropped, and refusing to save would leave the file uneditable for a reason nothing states.
        let dir = an_applet_dir("g13-gui-applet-unknown-widget");
        std::fs::write(dir.join("applets/x.json"), AN_APPLET).unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("x");
        window.applet_json["widgets"][0]["type"] = serde_json::json!("something-new");
        window.commit_applet();
        assert!(
            window.applet_problem.is_none(),
            "{:?}",
            window.applet_problem
        );
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/x.json")).unwrap())
                .unwrap();
        assert_eq!(written["widgets"][0]["type"], "something-new");
    }

    #[test]
    fn a_broken_applet_file_is_reported_rather_than_shown_as_an_empty_one() {
        let dir = an_applet_dir("g13-gui-applet-broken-file");
        std::fs::write(dir.join("applets/x.json"), "{ not json").unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("x");
        assert!(window.applet_problem.is_some(), "{}", window.status);
        assert!(
            window.status.contains("not valid JSON"),
            "it should say why: {}",
            window.status
        );
        assert!(
            window.applet_editing.is_none(),
            "nothing should be opened for editing"
        );
    }

    #[test]
    fn the_window_reads_the_bindings_of_the_active_profile() {
        let window = Window::load(&g13_config::config_dir());
        assert!(!window.rows.is_empty(), "there are controls to show");
        assert!(window.profiles.contains(&window.profile) || window.profiles.is_empty());
        // every row names a control and says what its action does
        for row in &window.rows {
            assert!(!row.control.is_empty());
            assert!(!row.effect.is_empty(), "{} has no description", row.control);
        }
    }

    #[test]
    fn the_actions_are_described_in_words() {
        assert_eq!(describe("p,k.30"), "sends a");
        assert_eq!(describe("m,1,2"), "plays macro 1, 2 time(s)");
        assert_eq!(describe("mk,3"), "switches to profile 3");
        assert_eq!(describe("x"), "nothing");
        assert!(
            describe("nonsense").contains("not a binding action this build knows"),
            "{}",
            describe("nonsense")
        );
    }

    #[test]
    fn the_visuals_offered_are_the_built_ins_and_the_applets_that_exist() {
        // A fixture, not this machine. It read the real configuration directory once, so it passed only where
        // applets happened to be installed - and its first run in CI, on a clean machine, is what said so.
        let dir = std::env::temp_dir().join("g13-visuals-offered");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(dir.join("applets/weather.json"), "{}").unwrap();
        std::fs::write(dir.join("applets/gpu.json"), "{}").unwrap();
        // an applet's pictures are not a screen of their own
        std::fs::write(dir.join("applets/weather.bitmaps.json"), "{}").unwrap();

        let window = Window::load(&dir);
        let offered = window.available_visuals();

        for built_in in ["clock", "system", "pad", "media"] {
            assert!(
                offered.contains(&built_in.to_string()),
                "{built_in} is missing"
            );
        }
        // and the applet files in that directory, by the name the driver uses
        for applet in ["applet:gpu", "applet:weather"] {
            assert!(
                offered.contains(&applet.to_string()),
                "{applet} is not offered: {offered:?}"
            );
        }
        assert!(
            !offered.iter().any(|name| name.contains("bitmaps")),
            "a bitmaps sidecar was offered as a screen: {offered:?}"
        );
    }

    #[test]
    fn the_tab_with_the_sticks_reading_on_it_is_redrawn_sixty_times_a_second() {
        // A regression test with a reason: this was once a second, in a line whose own comment said so, and
        // five fixes went into the driver while the window drew the stick at that rate. Sixty frames a
        // second is what makes the stick's reading a direct reading of the pad rather than a slideshow.
        assert_eq!(
            repaint_interval(Tab::Bindings),
            std::time::Duration::from_millis(16)
        );
        // and the configuration tabs stay cheap
        assert_eq!(
            repaint_interval(Tab::Applets),
            std::time::Duration::from_secs(1)
        );
        assert_eq!(
            repaint_interval(Tab::Menu),
            std::time::Duration::from_secs(1)
        );
    }

    #[test]
    fn a_preview_is_drawn_at_all() {
        let mut window = Window::load(&g13_config::config_dir());
        window.visual = "system".to_string();
        window.screen.show("system");
        // the drawing is on the worker thread, which renders on the visual's own interval, so this waits for
        // it rather than expecting it to be there the instant it is asked for
        // patient on purpose: this runs beside the rest of the suite, and every window in it has a renderer
        // of its own gathering the machine's numbers. What is asserted is that a frame arrives, not how soon.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            window.refresh_preview();
            if !window
                .preview
                .is_blank(0, 0, g13_screen::WIDTH, g13_screen::VISIBLE_HEIGHT)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the system visual drew nothing in a minute");
    }

    #[test]
    fn visuals_are_written_back_in_the_shape_the_driver_reads() {
        let dir = a_config_dir("g13-gui-visuals");
        let mut window = Window::load(&dir);
        window.enabled = vec!["system".to_string(), "applet:gpu".to_string()];
        window.visual = "applet:gpu".to_string();
        window.cycle = true;
        window.cycle_seconds = 7.0;
        window.save_visuals().expect("should write");

        let loaded = visuals::load_visuals(&dir.join("visuals.json"));
        assert_eq!(loaded.active, "applet:gpu");
        assert_eq!(
            loaded.enabled,
            vec!["system".to_string(), "applet:gpu".to_string()]
        );
        assert!(loaded.cycle);
        assert_eq!(loaded.cycle_seconds, 7.0);
    }

    #[test]
    fn editing_a_binding_writes_it_and_keeps_the_rest_of_the_file() {
        let dir = a_config_dir("g13-gui-bindings");
        let path = dir.join("bindings-0.properties");
        std::fs::write(&path, "# a comment\nG1=p,k.30\ncolor=1,2,3\n").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let updated = g13_config::set_binding(&text, "G2", "m,4,2");
        assert!(updated.contains("# a comment"), "the comment was lost");
        assert!(updated.contains("color=1,2,3"), "the setting was lost");
        assert!(
            updated.contains("G1=p,k.30"),
            "the existing binding was lost"
        );
        assert!(updated.contains("G2=m,4,2"));
    }

    #[test]
    fn the_bindings_table_says_when_a_key_acts_by_default() {
        // a table that shows the file and calls a defaulted key "nothing" is a table that disagrees with the pad
        let dir = a_config_dir("g13-gui-m-key-defaults");
        for profile in 0..4 {
            std::fs::write(
                dir.join(format!("bindings-{profile}.properties")),
                "G1=p,k.30\n",
            )
            .unwrap();
        }
        let window = Window::load(&dir);
        let row = |control: &str| {
            window
                .rows
                .iter()
                .find(|row| row.control == control)
                .cloned()
                .expect("the control is in the table")
        };
        let m1 = row("M1");
        assert!(m1.action.is_empty(), "the file names no M1: {}", m1.action);
        assert!(
            m1.effect.contains("switches to profile 1"),
            "the effect does not say what the pad will do: {}",
            m1.effect
        );
        assert!(
            m1.effect.contains("default"),
            "it does not say the action comes from a default: {}",
            m1.effect
        );
        // a control with no default and no line still says nothing
        assert_eq!(row("L1").effect, "nothing");
    }

    #[test]
    fn a_line_in_the_file_beats_the_default() {
        let dir = a_config_dir("g13-gui-m-key-bound");
        for profile in 0..4 {
            std::fs::write(
                dir.join(format!("bindings-{profile}.properties")),
                "M1=p,k.30\n",
            )
            .unwrap();
        }
        let window = Window::load(&dir);
        let m1 = window
            .rows
            .iter()
            .find(|row| row.control == "M1")
            .expect("M1 is in the table");
        assert_eq!(m1.action, "p,k.30", "the file's own line is not shown");
        assert!(
            !m1.effect.contains("default"),
            "a bound key is not a default: {}",
            m1.effect
        );
    }

    #[test]
    fn the_screen_colour_belongs_to_the_profile_the_table_shows() {
        let dir = a_config_dir("g13-gui-colour");
        std::fs::write(
            dir.join("bindings-0.properties"),
            "G1=p,k.30\ncolor=0,153,255\n",
        )
        .unwrap();
        std::fs::write(dir.join("bindings-2.properties"), "G1=p,k.31\n").unwrap();
        std::fs::write(dir.join("active-profile"), "2\n").unwrap();

        let mut window = Window::load(&dir);
        // profile 0 names one, and it is the one the table would show
        window.profile = 0;
        assert_eq!(
            window.binding_colour(),
            Some(g13_config::Colour {
                red: 0,
                green: 153,
                blue: 255
            })
        );
        // a profile that names none is not black: there is nothing to apply, and the screen keeps its light
        window.profile = 2;
        assert_eq!(window.binding_colour(), None);

        // setting one writes to the profile being shown and leaves the other alone
        window
            .set_binding_colour(g13_config::Colour {
                red: 255,
                green: 0,
                blue: 0,
            })
            .expect("should write");
        let two = std::fs::read_to_string(dir.join("bindings-2.properties")).unwrap();
        assert!(two.contains("color=255,0,0"), "{two}");
        assert!(two.contains("G1=p,k.31"), "the binding was lost: {two}");
        let zero = std::fs::read_to_string(dir.join("bindings-0.properties")).unwrap();
        assert!(zero.contains("color=0,153,255"), "{zero}");
        // and it reads back, which is what "written" means
        assert_eq!(
            window.binding_colour(),
            Some(g13_config::Colour {
                red: 255,
                green: 0,
                blue: 0
            })
        );
    }

    #[test]
    fn an_edit_is_written_to_the_profile_the_table_shows() {
        // a directory of its own, so the real configuration is not involved and no environment is touched
        let dir = a_config_dir("g13-gui-apply");
        std::fs::write(dir.join("bindings-0.properties"), "G1=p,k.30\n").unwrap();
        std::fs::write(dir.join("bindings-2.properties"), "G1=p,k.31\n").unwrap();
        std::fs::write(dir.join("active-profile"), "2\n").unwrap();

        let mut window = Window::load(&dir);
        // show profile 0 while profile 2 is the active one: the file written must follow the table
        window.profile = 0;
        window.apply("G1", "p,k.44").expect("should write");
        assert!(
            std::fs::read_to_string(dir.join("bindings-0.properties"))
                .unwrap()
                .contains("G1=p,k.44"),
            "the edit did not reach the profile being shown"
        );
        assert!(
            std::fs::read_to_string(dir.join("bindings-2.properties"))
                .unwrap()
                .contains("G1=p,k.31"),
            "the edit reached a profile that was not being shown"
        );
        // and it survives being re-read, which is what "does not hold" meant
        window.reload();
        let row = window
            .rows
            .iter()
            .find(|row| row.control == "G1")
            .expect("G1 is a control");
        assert_eq!(row.action, "p,k.44", "the edit did not survive a reload");
    }

    #[test]
    fn what_a_person_types_becomes_what_the_file_holds() {
        // a key name on its own means that key
        assert_eq!(normalise("a"), "p,k.30");
        assert_eq!(normalise("  w  "), "p,k.17");
        // an action is left exactly as written
        assert_eq!(normalise("p,k.30"), "p,k.30");
        assert_eq!(normalise("m,1,2"), "m,1,2");
        assert_eq!(normalise("mk,3"), "mk,3");
        assert_eq!(normalise("x"), "x");
        // empty means nothing, which is an action rather than an empty line
        assert_eq!(normalise(""), "x");
        assert_eq!(normalise("   "), "x");
    }

    #[test]
    fn the_driver_state_says_which_it_is() {
        assert!(Driver::Stopped.sentence().contains("no driver is drawing"));
        assert!(
            Driver::Running(0.4)
                .sentence()
                .contains("a driver is drawing")
        );
        assert!(Driver::Starting.sentence().contains("starting"));
    }

    #[test]
    fn a_slow_source_does_not_hold_the_window() {
        // A source that takes a whole second, which is what the slow ones on this machine look like. Note
        // that cmd: sources are cut off after 400 ms, so this one never finishes - which is the point here:
        // the window must not wait for it either way. What it draws is asserted separately, below.
        let dir = a_config_dir("g13-gui-slow");
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("applets/slow.json"),
            r#"{"name":"slow","interval":1,"sources":{"a":"cmd:sleep 2"},
                "widgets":[{"type":"text","x":0,"y":0,"format":"{a}"}]}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        window.screen.show("applet:slow");

        // everything the drawing thread does: asking for a visual, taking a frame, reading the live values.
        // This is the assertion that matters and it is not a matter of timing: if the rendering were on this
        // thread, twenty of these would take a hundred seconds.
        let started = std::time::Instant::now();
        for _ in 0..20 {
            window.screen.show("applet:slow");
            window.refresh_preview();
            window.refresh_live();
        }
        let took = started.elapsed();
        assert!(
            took < std::time::Duration::from_millis(500),
            "twenty repaints waited {took:?} on a source that never returns"
        );
    }

    #[test]
    fn the_window_draws_what_the_worker_renders() {
        // a source that finishes, so this checks the frame arriving rather than the waiting
        let dir = a_config_dir("g13-gui-draws");
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("applets/quick.json"),
            r#"{"name":"quick","interval":1,"sources":{"a":"cmd:printf 5"},
                "widgets":[{"type":"text","x":0,"y":0,"format":"{a}"}]}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        window.screen.show("applet:quick");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            window.refresh_preview();
            if !window
                .preview
                .is_blank(0, 0, g13_screen::WIDTH, g13_screen::VISIBLE_HEIGHT)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the applet never drew in a minute");
    }

    #[test]
    fn toggling_a_visual_off_then_on_returns_it() {
        let dir = a_config_dir("g13-gui-toggle");
        let mut window = Window::load(&dir);
        window.enabled = vec!["system".to_string()];
        window.visual = "system".to_string();
        window.toggle_enabled("system");
        assert!(!window.enabled.contains(&"system".to_string()));
        window.toggle_enabled("system");
        assert!(window.enabled.contains(&"system".to_string()));
    }
}

#[cfg(test)]
mod deleting_an_applet {
    use super::*;

    #[test]
    fn deletes_the_file_and_its_pictures_and_leaves_the_others_alone() {
        let dir = std::env::temp_dir().join(format!("g13-gui-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        for name in ["keep", "delete-me"] {
            std::fs::write(
                dir.join("applets").join(format!("{name}.json")),
                format!(r#"{{"name": "{name}", "widgets": []}}"#),
            )
            .unwrap();
        }
        // a sidecar is that applet's own pictures, not an applet: it goes with the applet
        let pictures = format!(
            r#"{{"Play": {{"w": 8, "h": 8, "rows": ["{}"]}}}}"#,
            "#".repeat(8)
        );
        std::fs::write(dir.join("applets/delete-me.bitmaps.json"), &pictures).unwrap();
        std::fs::write(dir.join("applets/keep.bitmaps.json"), &pictures).unwrap();

        let mut window = Window::load(&dir);
        window.open_applet("delete-me");
        assert_eq!(window.applet_editing.as_deref(), Some("delete-me"));
        window.remove_applet("delete-me");

        assert!(
            !dir.join("applets/delete-me.json").exists(),
            "the applet file is still there"
        );
        assert!(
            !dir.join("applets/delete-me.bitmaps.json").exists(),
            "its pictures were left behind as orphans"
        );
        assert!(
            dir.join("applets/keep.json").exists()
                && dir.join("applets/keep.bitmaps.json").exists(),
            "another applet was caught by the delete"
        );
        assert!(
            window.applet_editing.is_none(),
            "the window is still editing what it deleted"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod the_tick_and_the_walk_agree_on_the_name {
    use super::*;

    #[test]
    fn a_screen_that_was_built_in_keeps_its_bare_name_and_an_applet_is_prefixed() {
        // The tick box writes into `enabled`, which `visuals.json` holds and the walk reads. If this named the same
        // applet differently from the walk, the tick would silently do nothing - or tick the wrong screen.
        let dir = std::env::temp_dir().join(format!("g13-gui-tick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        for name in ["clock", "weather"] {
            std::fs::write(
                dir.join("applets").join(format!("{name}.json")),
                format!(r#"{{"name": "{name}", "widgets": []}}"#),
            )
            .unwrap();
        }
        let window = Window::load(&dir);
        assert_eq!(window.visual_name_for("clock").as_deref(), Some("clock"));
        assert_eq!(
            window.visual_name_for("weather").as_deref(),
            Some("applet:weather")
        );
        assert_eq!(
            window.visual_name_for("nothing-here"),
            None,
            "an applet that does not exist must not be tickable"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod values_he_names_himself {
    use super::*;

    #[test]
    fn adding_one_refuses_a_published_name_and_a_user_name_that_is_already_there() {
        let dir = std::env::temp_dir().join(format!("g13-gui-values-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        // an applet that reads the named value, which is the point of the whole thing
        std::fs::write(
            dir.join("applets/where.json"),
            r#"{"name": "where", "sources": {"spot": "where"}, "widgets": [{"type": "text", "x": 0, "y": 0, "format": "{spot}"}]}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        window
            .add_custom_value("where", "cmd:echo upstairs")
            .expect("it is free");
        assert_eq!(
            window.custom_values().get("where").map(String::as_str),
            Some("cmd:echo upstairs")
        );

        // a name every applet already reads is not available to a user value
        let refused = window
            .add_custom_value("cpu", "cmd:echo 1")
            .expect_err("cpu is published");
        assert!(refused.contains("already publishes"), "{refused}");
        // and neither is a user value already there
        let again = window
            .add_custom_value("where", "cmd:echo nowhere")
            .expect_err("it is taken");
        assert!(again.contains("already a value"), "{again}");
        // a name that cannot be a name is refused with the reason
        let odd = window
            .add_custom_value("two words", "cmd:echo 1")
            .expect_err("not a name");
        assert!(odd.contains("cannot be a name"), "{odd}");

        // it shows in the table as the user's, and says which applets read it - read out of the applet files
        let rows = window.all_value_rows();
        let mine = rows.iter().find(|row| row.name == "where").expect("a row");
        assert!(mine.mine, "a value the user named is not marked as theirs");
        assert_eq!(mine.used_by, vec!["where".to_string()]);
        assert!(rows.iter().any(|row| row.name == "cpu" && !row.mine));

        // taking it out again is an empty spec
        window.set_custom_value("where", "");
        assert!(!window.custom_values().contains_key("where"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod making_and_removing_sets {
    use super::*;

    fn a_config(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("g13-sets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bindings-0.properties"), "G1=p,k.59\n").unwrap();
        dir
    }

    /// The fault this is here for: a new set was written to disk and never appeared in the row of buttons, so
    /// there was nothing to select and nothing looked lit.
    #[test]
    fn a_new_set_appears_in_the_list_and_is_selected() {
        let dir = a_config("new");
        let mut window = Window::load(&dir);
        let before = window.profiles.len();

        let made = window.new_profile();

        assert_eq!(
            window.profiles.len(),
            before + 1,
            "the new set is not in the list: {:?} - window dir {}, config dir {}",
            window.profiles,
            window.config_dir.display(),
            g13_config::config_dir().display()
        );
        assert!(
            window.profiles.contains(&made),
            "the new set is not listed: {:?}",
            window.profiles
        );
        assert_eq!(
            window.profile, made,
            "the window should be on the set it just made"
        );
        assert!(
            dir.join(format!("bindings-{made}.properties")).exists(),
            "the set has no file behind it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_set_takes_it_out_of_the_list_and_leaves_the_window_somewhere_real() {
        let dir = a_config("delete");
        let mut window = Window::load(&dir);
        let made = window.new_profile();
        assert!(window.profiles.contains(&made));

        window.delete_profile(made);

        assert!(
            !window.profiles.contains(&made),
            "a deleted set is still listed"
        );
        assert!(!dir.join(format!("bindings-{made}.properties")).exists());
        assert!(
            window.profiles.contains(&window.profile),
            "the window is on a set that is not there"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_last_set_cannot_be_deleted() {
        let dir = a_config("last");
        let mut window = Window::load(&dir);
        assert_eq!(window.profiles.len(), 1);
        window.delete_profile(window.profile);
        assert_eq!(
            window.profiles.len(),
            1,
            "the only set was deleted, leaving nothing to edit"
        );
        assert!(dir.join("bindings-0.properties").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn naming_and_assigning_write_through_to_the_file_the_auto_bind_reads() {
        let dir = a_config("assign");
        let mut window = Window::load(&dir);
        window.rename_profile(0, "Cyberpunk");
        window.assign_profile_key(0, Some("M2"));

        let profiles = g13_config::Profiles::read_in(&dir);
        assert_eq!(profiles.name(0), "Cyberpunk");
        assert_eq!(
            profiles.profile_for_key("M2"),
            Some(0),
            "the assignment did not reach the file"
        );

        // and clicking the same key again takes it off, putting the number back
        window.assign_profile_key(0, None);
        let profiles = g13_config::Profiles::read_in(&dir);
        assert_eq!(profiles.profile_for_key("M2"), Some(2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod editing_the_menu {
    use super::*;

    fn a_config(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("g13-menu-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bindings-0.properties"), "G1=p,k.59\n").unwrap();
        dir
    }

    /// The whole point of the tab: type an item, press add, and it is in the file.
    #[test]
    fn an_item_added_in_the_window_reaches_the_file() {
        let dir = a_config("add");
        let mut window = Window::load(&dir);
        window.refresh_menu();

        window.new_menu_label = "Temperatures".to_string();
        window.add_menu_item();
        window.commit_menu();

        let path = dir.join("menu.json");
        assert!(path.exists(), "the menu was never written");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("Temperatures"),
            "the item is not in the file: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// And it survives the tab being redrawn, which re-reads the file on every draw.
    #[test]
    fn what_was_added_is_still_there_after_a_reread() {
        let dir = a_config("reread");
        let mut window = Window::load(&dir);
        window.refresh_menu();
        window.new_menu_label = "Weather".to_string();
        window.add_menu_item();
        window.commit_menu();

        // this is what the tab does on every draw
        window.refresh_menu();
        let rows = window.menu_rows();
        assert_eq!(rows.len(), 1, "the item vanished on the next draw");
        assert_eq!(rows[0].label, "Weather");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A menu file that will not parse is reported, and the editor is held rather than writing over it.
    #[test]
    fn a_menu_that_will_not_read_is_said_rather_than_overwritten() {
        let dir = a_config("broken");
        std::fs::write(dir.join("menu.json"), "{ this is not json").unwrap();
        let mut window = Window::load(&dir);
        window.refresh_menu();
        assert!(
            window.menu_problem.is_some(),
            "a broken menu was not reported"
        );
        assert!(
            window.menu_json.is_none(),
            "nothing should be editable over an unreadable file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod adding_a_resource_from_a_text_widget {
    use super::*;

    /// A config directory holding an applets folder, which is all these need.
    fn a_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        dir
    }

    /// A text widget has no `source` field - it reads names inside its `format` - so that is where the name has
    /// to go, and the applet has to declare it either way.
    #[test]
    fn the_format_takes_the_name_and_the_applet_declares_it() {
        let dir = a_dir("g13-add-for-format");
        let applet = serde_json::json!({
            "name": "where",
            "interval": 2,
            "sources": {},
            "widgets": [
                {"type": "text", "x": 3, "y": 3, "format": "at "},
                {"type": "bar", "x": 0, "y": 20, "w": 40, "h": 5, "source": "", "max": 100}
            ]
        });
        std::fs::write(dir.join("applets/where.json"), applet.to_string()).unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("where");

        let spec = "http:weather/?format=j1#nearest_area.0.areaName.0.value";
        window.add_source_for_widget(0, "place", spec);

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/where.json")).unwrap())
                .unwrap();
        assert_eq!(
            written["sources"]["place"], spec,
            "the source was not declared"
        );
        assert_eq!(
            written["widgets"][0]["format"], "at {place}",
            "the name is not in the format"
        );
        assert!(
            written["widgets"][0].get("source").is_none(),
            "a text widget was given a source field it does not have"
        );

        // and a widget that does read one value takes it in `source`, as before
        window.add_source_for_widget(1, "level", "cmd:echo 1");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/where.json")).unwrap())
                .unwrap();
        assert_eq!(written["widgets"][1]["source"], "level");
        assert_eq!(written["sources"]["level"], "cmd:echo 1");
    }

    /// A source with nothing to read is refused, and the widget is left alone rather than pointed at a name
    /// the applet does not declare.
    #[test]
    fn a_source_with_no_spec_leaves_the_widget_as_it_was() {
        let dir = a_dir("g13-add-empty");
        let applet = serde_json::json!({
            "name": "where", "interval": 2, "sources": {},
            "widgets": [{"type": "text", "x": 3, "y": 3, "format": "at "}]
        });
        std::fs::write(dir.join("applets/where.json"), applet.to_string()).unwrap();
        let mut window = Window::load(&dir);
        window.open_applet("where");

        window.add_source_for_widget(0, "place", "");

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/where.json")).unwrap())
                .unwrap();
        assert_eq!(
            written["widgets"][0]["format"], "at ",
            "the format was changed anyway"
        );
        assert!(written["sources"].as_object().unwrap().is_empty());
    }
}

#[cfg(test)]
mod a_sidecar_is_not_an_applet {
    use super::*;

    /// `applets/audio.bitmaps.json` is `audio`'s pictures. A listing that takes every `.json` offers the pad a
    /// screen called `audio.bitmaps`, which draws nothing: it was listed here, and by the command line's own
    /// `applet list`, until both asked `is_applet_file`.
    #[test]
    fn its_pictures_are_not_offered_as_a_screen() {
        let dir = std::env::temp_dir().join(format!("g13-sidecar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("applets/audio.json"),
            r#"{"name":"audio","widgets":[]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("applets/audio.bitmaps.json"),
            serde_json::json!({"heart": {"rows": ["#.#"]}}).to_string(),
        )
        .unwrap();

        let window = Window::load(&dir);
        let visuals = window.available_visuals();

        assert!(
            visuals.contains(&"applet:audio".to_string()),
            "the applet itself is not offered: {visuals:?}"
        );
        assert!(
            !visuals.iter().any(|visual| visual.contains("bitmaps")),
            "its pictures were offered as a screen: {visuals:?}"
        );
    }
}

#[cfg(test)]
mod what_an_applet_follows {
    use super::*;

    /// The row in the window writes the same thing the parser reads, and what it writes is not complained about.
    /// A window that wrote a shape its own driver called unhandled would be worse than no row at all.
    #[test]
    fn the_row_writes_what_the_driver_reads() {
        let dir = std::env::temp_dir().join(format!("g13-follow-row-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("applets/game.json"),
            r#"{"name":"game","widgets":[]}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        window.open_applet("game");
        assert_eq!(
            window.applet_follow_seconds(),
            0.0,
            "nothing set, nothing followed"
        );
        assert_eq!(window.applet_follow_profile(), None);

        window.set_applet_follow(20.0, Some("Cyberpunk".to_string()));

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/game.json")).unwrap())
                .unwrap();
        assert_eq!(written["follow"]["seconds"], 20);
        assert_eq!(written["follow"]["profile"], "Cyberpunk");

        // and the driver's own parser agrees, without complaining about the key
        let applet = g13_applets::parse_applet(&written.to_string()).unwrap();
        let follow = applet
            .follow
            .expect("the parser did not read what the row wrote");
        assert_eq!(follow.seconds, 20.0);
        assert_eq!(follow.profile.as_deref(), Some("Cyberpunk"));
        assert!(
            applet.unhandled.is_empty(),
            "the driver complains about what the window just wrote: {:?}",
            applet.unhandled
        );

        // the row reads back what it wrote, so reopening the applet shows the settings rather than blank boxes
        assert_eq!(window.applet_follow_seconds(), 20.0);
        assert_eq!(window.applet_follow_profile().as_deref(), Some("Cyberpunk"));

        // and turning it off takes the key away rather than leaving a line that says nothing
        window.set_applet_follow(0.0, None);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("applets/game.json")).unwrap())
                .unwrap();
        assert!(
            written.get("follow").is_none(),
            "switching it off left something behind: {written}"
        );
    }
}
