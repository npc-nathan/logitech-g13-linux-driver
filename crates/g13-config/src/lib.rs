//! Reading and writing `~/.config/g13/`.
//!
//! The formats are this project's own documented formats, and reading them is how an existing
//! installation keeps working - the compatibility promise this project is built on.
//!
//! **This crate is not a source of knowledge about the hardware.** The names in these files are the
//! previous driver's vocabulary; they are read to keep a user's configuration working, never to decide
//! what controls the pad has. What the device has is discovered from the device.

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
pub mod graph;
pub mod menu;
pub mod stick;

pub use graph::{
    GraphEdge, GraphNode, KeyMode, MacroGraph, NodeKind, graph_path, graph_to_json, is_graph_file,
    parse_graph, read_graph, read_playable, write_graph,
};

use std::path::PathBuf;

/// What each binding set is called, and which M key switches to it.
///
/// The M keys have always auto-bound to the set with the same number - M1 to 1, M2 to 2, M3 to 3. That stays as
/// the fallback, and this file is how it is changed: a name for the set, and the key it answers to. The point of
/// the file is that assigning a set to a key is not a binding - a binding line is the *override*, for a key doing
/// something other than what it does by default.
///
/// Nothing here is required. A missing file means every set keeps its number and reads as "Profile N".
#[derive(Debug, Default, Clone)]
pub struct Profiles {
    /// What each binding set is called, by number; a set missing from here reads as "Profile N".
    names: std::collections::BTreeMap<u32, String>,
    /// The M key each set answers to, when the file assigns one rather than the number on the key.
    keys: std::collections::BTreeMap<u32, String>,
}

impl Profiles {
    /// The path of `profiles.json` in the config directory the driver reads.
    pub fn path() -> std::path::PathBuf {
        Self::path_in(&config_dir())
    }

    /// The same file, in a config directory given rather than assumed. The window works on its own directory, so
    /// naming and assigning have to be able to write there rather than to whatever the process was started with.
    pub fn path_in(config: &std::path::Path) -> std::path::PathBuf {
        config.join("profiles.json")
    }

    /// Read them from a given config directory.
    pub fn read_in(config: &std::path::Path) -> Self {
        match std::fs::read_to_string(Self::path_in(config)) {
            Ok(text) => Self::from_json(&text),
            Err(_) => Self::default(),
        }
    }

    /// Write them into a given config directory.
    pub fn write_in(&self, config: &std::path::Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(&self.as_json())?;
        // beside and renamed: a driver reading this file while it is being replaced sees all of one version
        g13_files::write(&Self::path_in(config), &format!("{text}\n"))
            .map_err(std::io::Error::other)
    }

    /// Read them, or an empty set if there is no file or it cannot be read.
    pub fn read() -> Self {
        let Ok(text) = std::fs::read_to_string(Self::path()) else {
            return Self::default();
        };
        Self::from_json(&text)
    }

    /// The same read, from text, so the format is testable without a config directory.
    pub fn from_json(text: &str) -> Self {
        let mut profiles = Profiles::default();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            return profiles;
        };
        let Some(map) = value.as_object() else {
            return profiles;
        };
        for (number, entry) in map {
            let Ok(profile) = number.parse::<u32>() else {
                continue;
            };
            let Some(object) = entry.as_object() else {
                continue;
            };
            if let Some(name) = object.get("name").and_then(|v| v.as_str())
                && !name.trim().is_empty()
            {
                profiles.names.insert(profile, name.trim().to_string());
            }
            if let Some(key) = object.get("key").and_then(|v| v.as_str()) {
                let key = key.trim().to_ascii_uppercase();
                if matches!(key.as_str(), "M1" | "M2" | "M3") {
                    profiles.keys.insert(profile, key);
                }
            }
        }
        profiles
    }

    /// The set a name belongs to, so something outside the window can name a set instead of numbering it - an
    /// applet's `follow` says `"profile": "Cyberpunk"`.
    pub fn profile_named(&self, name: &str) -> Option<u32> {
        self.names
            .iter()
            .find(|(_, called)| called.as_str() == name)
            .map(|(profile, _)| *profile)
    }

    /// What a set is called. "Profile N" when the file does not name it.
    pub fn name(&self, profile: u32) -> String {
        self.names
            .get(&profile)
            .cloned()
            .unwrap_or_else(|| format!("Profile {profile}"))
    }

    /// Whether this set has been given a name of its own.
    pub fn is_named(&self, profile: u32) -> bool {
        self.names.contains_key(&profile)
    }

    /// The M key that switches to this set, if the file says so.
    pub fn key(&self, profile: u32) -> Option<&str> {
        self.keys.get(&profile).map(|k| k.as_str())
    }

    /// Which set an M key switches to.
    ///
    /// The file if it says, and otherwise the number on the key: M1 to 1, M2 to 2, M3 to 3. That fallback is what
    /// the prototype did and what a set seeded by number assumes.
    pub fn profile_for_key(&self, key: &str) -> Option<u32> {
        let key = key.trim().to_ascii_uppercase();
        if let Some((profile, _)) = self.keys.iter().find(|(_, k)| **k == key) {
            return Some(*profile);
        }
        match key.as_str() {
            "M1" => Some(1),
            "M2" => Some(2),
            "M3" => Some(3),
            _ => None,
        }
    }

    /// Give a set a name. An empty or blank name goes back to being unnamed.
    pub fn set_name(&mut self, profile: u32, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.names.remove(&profile);
        } else {
            self.names.insert(profile, name.to_string());
        }
    }

    /// Put a set on an M key, taking it off whatever set had that key before.
    ///
    /// One key switches to one set: leaving two sets answering to M1 would make which one you get depend on the
    /// order a map happens to be read in.
    pub fn set_key(&mut self, profile: u32, key: &str) {
        let key = key.trim().to_ascii_uppercase();
        if !matches!(key.as_str(), "M1" | "M2" | "M3") {
            return;
        }
        self.keys.retain(|_, existing| *existing != key);
        self.keys.insert(profile, key);
    }

    /// Take a set off its M key, which puts the key back to switching to whatever is numbered the same.
    pub fn clear_key(&mut self, profile: u32) {
        self.keys.remove(&profile);
    }

    /// Forget a set entirely, for when its file is gone.
    pub fn forget(&mut self, profile: u32) {
        self.names.remove(&profile);
        self.keys.remove(&profile);
    }

    /// Write the whole lot back, so the GUI can add, rename, assign and remove.
    pub fn write(&self) -> std::io::Result<()> {
        self.write_in(&config_dir())
    }

    /// The file's own shape, so writing and testing do not each build it.
    fn as_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        let mut numbers: Vec<u32> = self.names.keys().chain(self.keys.keys()).copied().collect();
        numbers.sort_unstable();
        numbers.dedup();
        for profile in numbers {
            let mut entry = serde_json::Map::new();
            if let Some(name) = self.names.get(&profile) {
                entry.insert("name".into(), serde_json::Value::String(name.clone()));
            }
            if let Some(key) = self.keys.get(&profile) {
                entry.insert("key".into(), serde_json::Value::String(key.clone()));
            }
            map.insert(profile.to_string(), serde_json::Value::Object(entry));
        }
        serde_json::Value::Object(map)
    }
}

/// Where the defaults live in each shape g13 ships in, most likely first.
///
/// A package manager owns `/usr/share/g13/defaults`; a tarball or a checkout keeps them beside the binary. This
/// knows both so that nothing else has to.
pub fn defaults_dir() -> Option<std::path::PathBuf> {
    let mut candidates = vec![
        std::path::PathBuf::from("/usr/share/g13/defaults"),
        std::path::PathBuf::from("/usr/local/share/g13/defaults"),
    ];
    if let Ok(exe) = std::env::current_exe() {
        let mut up = exe.parent();
        for _ in 0..4 {
            let Some(here) = up else { break };
            candidates.push(here.join("defaults"));
            candidates.push(here.join("share/g13/defaults"));
            up = here.parent();
        }
    }
    candidates.into_iter().find(|dir| dir.is_dir())
}

/// Put the defaults in a config that has none, and never touch one that has.
///
/// This is what makes installing the package the whole install: a driver that has never been run finds its own
/// applets, fonts, themes, bindings and macros waiting for it. It only acts on an empty config, so it costs one
/// directory read on every later start and cannot overwrite anything anybody has edited.
///
/// Returns what it added, so a caller with somewhere to print can say so.
pub fn seed_defaults() -> Vec<std::path::PathBuf> {
    let config = config_dir();
    let Ok(entries) = std::fs::read_dir(&config) else {
        // no config at all yet: that is exactly the case this is for
        return copy_defaults(&config);
    };
    let has_anything = entries.flatten().any(|entry| {
        entry.file_name() != "active-profile"
            && !entry.file_name().to_string_lossy().starts_with('.')
    });
    if has_anything {
        return Vec::new();
    }
    copy_defaults(&config)
}

/// The parts of a config that live in a folder of their own.
const FOLDERS: [&str; 3] = ["applets", "fonts", "themes"];
/// Files that sit at the top of a config rather than in a folder. Taken from where the driver reads them - a
/// bindings file is `bindings-0.properties` beside the applets folder, not inside one - because a defaults tree
/// laid out differently from the reader is a machine with no bindings.
const FLAT: [&str; 7] = [
    "bindings-0.properties",
    "bindings-1.properties",
    "bindings-2.properties",
    "bindings-3.properties",
    "endpoints.json",
    "profiles.json",
    "visuals.json",
];

/// Put the shipped defaults into a config, and say which files arrived. What is already there is left alone.
fn copy_defaults(config: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Some(defaults) = defaults_dir() else {
        return Vec::new();
    };
    let mut added = Vec::new();
    for folder in FOLDERS {
        let source = defaults.join(folder);
        if !source.is_dir() {
            continue;
        }
        let target = config.join(folder);
        if std::fs::create_dir_all(&target).is_err() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&source) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            let destination = target.join(name);
            if destination.exists() {
                continue;
            }
            if std::fs::copy(&path, &destination).is_ok() {
                added.push(destination);
            }
        }
    }
    // everything the defaults hold at the top level: the bindings, the macros, the endpoint template, the rotation
    let Ok(entries) = std::fs::read_dir(&defaults) else {
        return added;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        let name = name.to_string_lossy().to_string();
        let wanted = FLAT.contains(&name.as_str()) || name.starts_with("macro-");
        if !wanted {
            continue;
        }
        let destination = config.join(&name);
        if destination.exists() {
            continue;
        }
        if std::fs::copy(&path, &destination).is_ok() {
            added.push(destination);
        }
    }
    added
}

/// The config directory: `$G13_CONFIG_DIR`, then `$XDG_CONFIG_HOME/g13`, then `~/.config/g13`.
///
/// `G13_CONFIG_DIR` is what lets a tool work on somebody else's installation - the window on its own
/// directory, `g13 doctor`, a test - instead of the user's. Failing to prefer the human over root reads
/// an empty configuration, which looks like a broken install.
pub fn config_dir() -> PathBuf {
    if let Some(explicit) = std::env::var_os("G13_CONFIG_DIR") {
        return PathBuf::from(explicit);
    }
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(base).join("g13");
    }
    if let Some(user) = std::env::var_os("SUDO_USER")
        && let Some(home) = home_of(&user)
    {
        return home.join(".config/g13");
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".config/g13"),
        None => PathBuf::from(".config/g13"),
    }
}

/// A user's home directory, from the password database.
fn home_of(user: &std::ffi::OsStr) -> Option<PathBuf> {
    let text = std::fs::read_to_string("/etc/passwd").ok()?;
    let user = user.to_string_lossy();
    text.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() > 5 && fields[0] == user {
            Some(PathBuf::from(fields[5]))
        } else {
            None
        }
    })
}

/// One line of a bindings file: a control's name and what it is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The control's own name, as the pad and the bindings file spell it: `G20`, `LR`, `M1`.
    pub name: String,
    /// What the control sends, in the file's syntax - `p,k.<code>`, `m,<macro>,<repeats>`, `mk,<index>`
    /// or `x` for nothing.
    pub action: String,
}

/// Parse a bindings file into its bindings, ignoring comments, blanks and settings.
///
/// Read as data, for the promise that an existing configuration keeps working. What the names in it mean
/// physically is a separate question answered by observation, never by assuming this file describes the
/// hardware.
pub fn bindings_from_text(text: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, action)) = line.split_once('=') else {
            continue;
        };
        let (name, action) = (name.trim(), action.trim());
        if name.is_empty() || action.is_empty() {
            continue;
        }
        // A binding's value is an action: `p,k.<code>`, `m,<macro>,<repeats>`, `mk,<index>` or `x`.
        // Settings such as `color=255,255,255` share the file but not that shape, so they are recognised
        // by the action type rather than by punctuation  -  `x` sends nothing and has no comma at all.
        if !is_action(action) {
            continue;
        }
        bindings.push(Binding {
            name: name.to_string(),
            action: action.to_string(),
        });
    }
    bindings
}

/// Whether a value is one of the action types this file's format defines.
pub fn is_action(value: &str) -> bool {
    let value = value.trim();
    matches!(value, "x")
        || value.starts_with("p,k.")
        || value.starts_with("m,")
        || value.starts_with("mk,")
        // the two device-button actions, either way round: the vocabulary separates with a comma, and a
        // colon is what a person reaches for when writing one by hand. This is also what tells a binding
        // apart from a settings line sharing the same file, so a shape missing from here is a line dropped
        // in silence - which is how `gb:` once vanished without a word.
        || value.starts_with("mb,")
        || value.starts_with("mb:")
        || value.starts_with("gb,")
        || value.starts_with("gb:")
        // and recording, which is the second shape to be found missing from this list by a binding that did
        // nothing rather than by a test: a line whose value is not recognised here never reaches any parser
        || value == "rec"
        || value.starts_with("rec,")
        // and the screen: `sv,next`, `sv,prev`, or `sv,<visual>` to show one by name. These controls change what
        // the pad is showing - LR (the round button) and L1-L4 are also used for changing applet and menus - so
        // the vocabulary has to carry them.
        || value.starts_with("sv,")
        || value.starts_with("sv:")
        // and the menu, which is what a held LR opens: `LR.hold=menu`
        || value == "menu"
}

/// How long a control has to be held to count as held, from the profile's own file.
///
/// A settings line, beside the actions, where `color=` already lives: same file, same shape, and a person editing
/// their bindings can see every knob the driver reads. Defaults to 500ms, and is clamped to something a hand can
/// tell apart from a tap.
pub fn long_press_ms_in(text: &str) -> u64 {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("long_press_ms") {
            if let Ok(ms) = value.trim().parse::<u64>() {
                return ms.clamp(150, 3_000);
            }
        }
    }
    500
}

/// The name of a control in a binding line, and whether the line is about holding it.
///
/// `LR=sv,next` is what LR does; `LR.hold=menu` is what a held LR does. One line each, because they are two
/// things a control does and a value with two meanings in it would be a parser of its own.
pub fn control_and_hold(name: &str) -> (&str, bool) {
    match name.rsplit_once('.') {
        Some((control, modifier)) if modifier.eq_ignore_ascii_case("hold") => (control, true),
        _ => (name, false),
    }
}

/// The keycode a passthrough binding sends, if that is what it is.
///
/// `p,k.<code>` is a key, `m,...` a macro, `mk,...` the M key, `x` nothing. Only the first has a keycode to
/// observe, which is what makes it useful for establishing which name is which physical control.
pub fn passthrough_keycode(action: &str) -> Option<u16> {
    let rest = action.trim().strip_prefix("p,k.")?;
    rest.trim().parse::<u16>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delay_moves_the_clock_instead_of_taking_time_of_its_own() {
        // The whole point of the schedule: a wait says how long to wait *before the next thing*, so playing to
        // these deadlines is what keeps a macro at the speed it was recorded rather than that plus the cost of
        // sending every key.
        let steps = vec![
            MacroStep::KeyDown(20),
            MacroStep::Delay(50),
            MacroStep::KeyUp(20),
            MacroStep::Delay(30),
            MacroStep::KeyDown(21),
        ];
        let due = step_deadlines(&steps);
        let ms: Vec<u128> = due.iter().map(|d| d.as_millis()).collect();
        assert_eq!(ms, vec![0, 0, 50, 50, 80]);

        // keys with no wait between them are due at the same instant, which is "as fast as they can go"
        assert_eq!(
            step_deadlines(&[MacroStep::KeyDown(20), MacroStep::KeyDown(21)]),
            vec![std::time::Duration::ZERO, std::time::Duration::ZERO]
        );
        assert!(step_deadlines(&[]).is_empty());
    }

    const REAL_LINES: &str = "\
G20=p,k.20
G0=p,k.30
color=1
G5=mk,1
";

    #[test]
    fn bindings_are_read_as_data_and_settings_are_not_bindings() {
        let bindings = bindings_from_text(REAL_LINES);
        assert_eq!(
            bindings.len(),
            3,
            "G20, G0 and G5 are bindings; color is a setting"
        );
        assert_eq!(bindings[0].name, "G20");
        assert_eq!(bindings[0].action, "p,k.20");
    }

    #[test]
    fn a_passthrough_keycode_is_read_and_other_actions_are_null() {
        assert_eq!(passthrough_keycode("p,k.44"), Some(44));
        assert_eq!(
            passthrough_keycode("m,7,1"),
            None,
            "a macro has no single keycode"
        );
        assert_eq!(passthrough_keycode("mk,1"), None);
        assert_eq!(passthrough_keycode("x"), None);
        assert_eq!(passthrough_keycode("p,k.wibble"), None);
    }

    #[test]
    fn comments_and_blanks_are_ignored() {
        assert!(bindings_from_text("# G1=p,k.1\n\n\n").is_empty());
    }

    #[test]
    fn a_binding_names_a_control_and_says_what_it_sends() {
        // the map itself, read and written without interpreting the names: they are the pad's own labels
        // the file's own action syntax: p,k.<code> passthrough, m,.. macro, mk,.. M key, x nothing
        let written = "G1=p,k.30\nLR=p,k.1\nM1=m,7,2\nG2=mk,1\nG3=x\n";
        let bindings = bindings_from_text(written);
        assert_eq!(bindings.len(), 5);
        assert_eq!(bindings[0].name, "G1");
        assert_eq!(passthrough_keycode(&bindings[0].action), Some(30));
        assert_eq!(passthrough_keycode(&bindings[1].action), Some(1));
        assert_eq!(
            passthrough_keycode(&bindings[2].action),
            None,
            "a macro is not a keycode"
        );
        assert_eq!(
            passthrough_keycode(&bindings[3].action),
            None,
            "the M key is not a keycode"
        );
        assert_eq!(
            passthrough_keycode(&bindings[4].action),
            None,
            "nothing is not a keycode"
        );

        // a line whose value has no action type is a setting, not a binding
        assert_eq!(bindings_from_text("color=255,255,255\n").len(), 0);
    }
}

#[cfg(test)]
mod config_dir_tests {
    use super::*;

    #[test]
    fn a_home_can_be_looked_up_from_the_password_database() {
        // root always exists, so this checks the lookup without depending on the current user
        let home = home_of(std::ffi::OsStr::new("root"));
        assert_eq!(home, Some(PathBuf::from("/root")));
    }

    #[test]
    fn a_user_who_does_not_exist_has_no_home() {
        assert_eq!(home_of(std::ffi::OsStr::new("nobody-xyzzy")), None);
    }
}

/// The colour a profile asks the screen's backlight to be.
///
/// It lives in that profile's bindings file as `color=R,G,B`, which is where the previous stack kept it, so a
/// profile carries its own colour and switching profile switches the light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    /// How much red, 0 to 255.
    pub red: u8,
    /// How much green, 0 to 255.
    pub green: u8,
    /// How much blue, 0 to 255.
    pub blue: u8,
}

impl Colour {
    /// Read `R,G,B`. A value outside a byte is refused rather than clamped: a file saying `300,0,0` is wrong,
    /// and putting 255 there would be a different colour that looks deliberate.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().split(',');
        let red = parts.next()?.trim().parse::<u8>().ok()?;
        let green = parts.next()?.trim().parse::<u8>().ok()?;
        let blue = parts.next()?.trim().parse::<u8>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self { red, green, blue })
    }

    /// As the file writes it.
    pub fn text(self) -> String {
        format!("{},{},{}", self.red, self.green, self.blue)
    }
}

/// The colour a bindings file names, if it names one.
pub fn colour_in(text: &str) -> Option<Colour> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && line.contains('='))
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| name.trim() == "color")
        .and_then(|(_, value)| Colour::parse(value))
}

/// Put a colour in a bindings file, keeping every other line exactly as it is.
///
/// The same shape as `set_binding`, and for the same reason: the file is the user's, and reading it into
/// something and writing it back would drop whatever this build does not know about.
pub fn set_colour(text: &str, colour: Colour) -> String {
    let mut lines: Vec<String> = text.lines().map(|line| line.to_string()).collect();
    let mut replaced = false;
    for line in lines.iter_mut() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || !trimmed.contains('=') {
            continue;
        }
        if let Some((name, _)) = trimmed.split_once('=')
            && name.trim() == "color"
        {
            *line = format!("color={}", colour.text());
            replaced = true;
        }
    }
    if !replaced {
        lines.insert(0, format!("color={}", colour.text()));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// A copy of `text` with `control` bound to `action`, replacing the line it already had or adding one.
///
/// Every other line survives exactly as written - comments, settings, and controls this build does not
/// understand - because the file is the user's, and reading it into a structure and writing it back
/// would drop whatever the structure has no field for.
pub fn set_binding(text: &str, control: &str, action: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(|line| line.to_string()).collect();
    let mut replaced = false;
    for line in lines.iter_mut() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || !trimmed.contains('=') {
            continue;
        }
        if let Some((name, _)) = trimmed.split_once('=')
            && name.trim() == control
        {
            *line = format!("{control}={action}");
            replaced = true;
        }
    }
    if !replaced {
        if !lines.is_empty() && !lines.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
            // keep it tidy: bindings after bindings
        }
        lines.push(format!("{control}={action}"));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Read a control's binding out of a file's contents.
pub fn binding_for(text: &str, control: &str) -> Option<String> {
    bindings_from_text(text)
        .into_iter()
        .find(|binding| binding.name == control)
        .map(|binding| binding.action)
}

/// What the file says for a control, whether or not it is an action.
///
/// `binding_for` above answers "what will this control do", which is nothing for a line whose value is not an
/// action - and "nothing" is indistinguishable from the control having no line at all. Both are true statements
/// about the driver and neither is true about the *file*, so a tool that shows the file needs this: a line
/// written for a control that the build does not understand is a mistake to report, not a control to call
/// unbound.
pub fn raw_binding_for(text: &str, control: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case(control) {
            return Some(value.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod set_binding_tests {
    use super::*;

    const EXISTING: &str = "# a comment\nG1=p,k.30\ncolor=255,255,255\nLR=p,k.1\n";

    #[test]
    fn an_existing_binding_is_replaced_in_place() {
        let updated = set_binding(EXISTING, "G1", "p,k.31");
        assert!(updated.contains("G1=p,k.31"));
        assert!(!updated.contains("G1=p,k.30"));
        assert!(updated.starts_with("# a comment"), "the comment survives");
        assert!(updated.contains("color=255,255,255"), "a setting survives");
    }

    #[test]
    fn a_new_binding_is_appended_and_nothing_else_moves() {
        let updated = set_binding(EXISTING, "M1", "p,k.45");
        assert!(updated.contains("M1=p,k.45"));
        assert!(updated.contains("G1=p,k.30"));
        assert!(updated.contains("color=255,255,255"));
        assert!(updated.contains("LR=p,k.1"));
    }

    #[test]
    fn setting_the_same_binding_twice_does_not_duplicate_it() {
        let once = set_binding(EXISTING, "G1", "p,k.31");
        let twice = set_binding(&once, "G1", "p,k.32");
        assert_eq!(twice.matches("G1=").count(), 1);
        assert!(twice.contains("G1=p,k.32"));
    }

    #[test]
    fn a_binding_can_be_read_back() {
        assert_eq!(binding_for(EXISTING, "G1"), Some("p,k.30".to_string()));
        assert_eq!(binding_for(EXISTING, "nope"), None);
    }
}

/// Make a profile active, in a config directory given rather than assumed.
pub fn write_active_profile_in(config: &std::path::Path, profile: u32) -> std::io::Result<()> {
    std::fs::write(config.join("active-profile"), format!("{profile}\n"))
}

/// Which profile is active, as the file records it, in the config directory given.
pub fn read_active_profile_in(config: &std::path::Path) -> u32 {
    std::fs::read_to_string(config.join("active-profile"))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

/// Which profile is active in the config directory the driver uses; 0 when no file records one.
pub fn read_active_profile() -> u32 {
    std::fs::read_to_string(config_dir().join("active-profile"))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

/// Record which profile is active.
pub fn write_active_profile(profile: u32) -> std::io::Result<()> {
    let directory = config_dir();
    std::fs::create_dir_all(&directory)?;
    std::fs::write(directory.join("active-profile"), format!("{profile}\n"))
}

/// The path of a profile's bindings file, in a config directory given rather than assumed.
///
/// The parameterised form exists so that a tool which works on somebody else's directory - `g13 doctor`, or a
/// test - names the file the same way the driver does instead of spelling the pattern a second time.
pub fn bindings_path_in(config: &std::path::Path, profile: u32) -> std::path::PathBuf {
    config.join(format!("bindings-{profile}.properties"))
}

/// The path of a profile's bindings file.
pub fn bindings_path(profile: u32) -> std::path::PathBuf {
    bindings_path_in(&config_dir(), profile)
}

/// The path of a macro's file.
pub fn macro_path(id: u32) -> std::path::PathBuf {
    config_dir().join(format!("macro-{id}.properties"))
}

/// When each step is due, counted from the start of the macro.
///
/// A delay says how long to wait before the *next* thing, so it moves the clock rather than taking time of
/// its own. Playing to these deadlines is what makes a macro come out at the speed it went in, rather than at
/// that speed plus however long each key took to send.
pub fn step_deadlines(steps: &[MacroStep]) -> Vec<std::time::Duration> {
    let mut at = std::time::Duration::ZERO;
    let mut due = Vec::with_capacity(steps.len());
    for step in steps {
        due.push(at);
        if let MacroStep::Delay(ms) = step {
            at += std::time::Duration::from_millis(*ms as u64);
        }
    }
    due
}

/// One step of a macro: a key going down, a key coming up, or a pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroStep {
    /// A key going down, by the keycode a passthrough binding names.
    KeyDown(u16),
    /// The same key coming back up.
    KeyUp(u16),
    /// A pause, in milliseconds, before whatever comes next.
    Delay(u32),
}

/// A macro as the file describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Macro {
    /// What the macro is called, which is the `name=` line of its file.
    pub name: String,
    /// Which macro it is: the number in `macro-<id>.properties`, and in `m,<id>,<repeats>`.
    pub id: u32,
    /// The keys and pauses, in the order they play.
    pub steps: Vec<MacroStep>,
}

/// Parse a macro file.
///
/// The format is read from the files the previous tool wrote: a `sequence` of `kd.<code>` (key down),
/// `ku.<code>` (key up) and `d.<milliseconds>` (pause), with a `name` and an `id`. Read as data  -  this is
/// the user's own macro, and its shape is all that is taken from it.
pub fn parse_macro(text: &str) -> Macro {
    let mut macro_file = Macro::default();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "name" => macro_file.name = value.trim().to_string(),
            "id" => macro_file.id = value.trim().parse().unwrap_or(0),
            "sequence" => macro_file.steps = parse_sequence(value),
            _ => {}
        }
    }
    macro_file
}

/// The steps of a `sequence` value.
pub fn parse_sequence(value: &str) -> Vec<MacroStep> {
    let mut steps = Vec::new();
    for item in value.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let Some((kind, number)) = item.split_once('.') else {
            continue;
        };
        match (kind, number.trim().parse::<u32>()) {
            ("kd", Ok(code)) => steps.push(MacroStep::KeyDown(code as u16)),
            ("ku", Ok(code)) => steps.push(MacroStep::KeyUp(code as u16)),
            ("d", Ok(ms)) => steps.push(MacroStep::Delay(ms)),
            _ => {}
        }
    }
    steps
}

/// The `sequence` value for a set of steps.
pub fn sequence_text(steps: &[MacroStep]) -> String {
    steps
        .iter()
        .map(|step| match step {
            MacroStep::KeyDown(code) => format!("kd.{code}"),
            MacroStep::KeyUp(code) => format!("ku.{code}"),
            MacroStep::Delay(ms) => format!("d.{ms}"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// A macro written back in the shape the files use.
///
/// The same three keys the previous tool wrote, so a macro made here opens in it and a macro made there plays
/// here. The `#` line says where it came from, in place of the timestamp comment it used to carry.
pub fn macro_to_text(macro_file: &Macro) -> String {
    format!(
        "# written by g13\nsequence={}\nname={}\nid={}\n",
        sequence_text(&macro_file.steps),
        macro_file.name,
        macro_file.id
    )
}

/// The lowest macro id with no file of its own, which is where a recording goes when nothing names one.
///
/// **From 1, not 0.** `macro-0.properties` is a real macro - CTRL-ALT-DEL - and a recorder that took the
/// lowest id of all would overwrite it by default. Being wrong about that restarts the machine.
pub fn next_free_macro_id() -> u32 {
    (1..1000).find(|id| !macro_path(*id).exists()).unwrap_or(0)
}

#[test]
fn a_macro_written_here_reads_back_the_same() {
    let macro_file = Macro {
        name: "CTRL-ALT-DEL".to_string(),
        id: 0,
        steps: vec![
            MacroStep::KeyDown(29),
            MacroStep::KeyDown(56),
            MacroStep::KeyDown(111),
            MacroStep::Delay(20),
            MacroStep::KeyUp(111),
            MacroStep::KeyUp(56),
            MacroStep::KeyUp(29),
            MacroStep::Delay(100),
        ],
    };
    let written = macro_to_text(&macro_file);
    assert!(written.ends_with('\n'), "{written:?}");
    // the same three keys the previous tool wrote
    assert!(written.contains("sequence=kd.29,kd.56,kd.111,d.20,ku.111,ku.56,ku.29,d.100"));
    assert!(written.contains("name=CTRL-ALT-DEL"));
    assert!(written.contains("id=0"));
    // and it reads back as what went in, which is the whole point
    assert_eq!(parse_macro(&written), macro_file);
}

#[test]
fn a_control_can_have_one_thing_it_does_when_held() {
    // the spelling: a `.hold` line beside the ordinary one
    assert_eq!(control_and_hold("LR"), ("LR", false));
    assert_eq!(control_and_hold("LR.hold"), ("LR", true));
    assert_eq!(control_and_hold("LR.HOLD"), ("LR", true));
    // a dot that is not the modifier is part of the name, whatever it turns out to mean
    assert_eq!(control_and_hold("LR.something"), ("LR.something", false));
    assert_eq!(control_and_hold("G1"), ("G1", false));

    // and the threshold comes from the file, defaulting to something a hand can feel
    assert_eq!(long_press_ms_in("G1=p,k.30\n"), 500);
    assert_eq!(long_press_ms_in("long_press_ms=800\nG1=p,k.30\n"), 800);
    assert_eq!(long_press_ms_in("LONG_PRESS_MS = 300\n"), 300);
    // nonsense and extremes fall back rather than making a control unusable
    assert_eq!(long_press_ms_in("long_press_ms=soon\n"), 500);
    assert_eq!(long_press_ms_in("long_press_ms=0\n"), 150);
    assert_eq!(long_press_ms_in("long_press_ms=99999\n"), 3_000);
    // a commented-out one is not a setting
    assert_eq!(long_press_ms_in("# long_press_ms=800\n"), 500);
    // `menu` is an action, so a line carrying it reaches a parser instead of being dropped
    assert!(is_action("menu"));
}

#[test]
fn the_file_can_be_read_for_what_it_says_rather_than_for_what_it_does() {
    // The difference this exists for: a line written for a control that the build cannot use is invisible to
    // `binding_for`, which makes it look exactly like a control with no line at all.
    let text = "# G9=p,k.30 is commented out\nG1=p,k.30\nG2=flash,3\ncolor=255,255,255\n";
    assert_eq!(binding_for(text, "G2"), None, "flash,3 is not an action");
    assert_eq!(raw_binding_for(text, "G2").as_deref(), Some("flash,3"));
    assert_eq!(raw_binding_for(text, "G1").as_deref(), Some("p,k.30"));
    // a commented-out line is not a line
    assert_eq!(raw_binding_for(text, "G9"), None);
    // and a settings line is not a binding for a control of that name
    assert_eq!(
        raw_binding_for(text, "color").as_deref(),
        Some("255,255,255")
    );
    assert_eq!(raw_binding_for(text, "G3"), None);
    // the pad writes its own names in capitals; a file written by hand need not
    assert_eq!(raw_binding_for(text, "g1").as_deref(), Some("p,k.30"));
}

#[test]
fn a_recording_never_lands_on_macro_zero_by_default() {
    // id 0 is CTRL-ALT-DEL in the macro directory: a recorder that took the lowest id would overwrite it
    let id = next_free_macro_id();
    assert_ne!(id, 0, "a recording must not be given macro 0");
    assert!(!macro_path(id).exists(), "id {id} already has a file");
}

#[test]
fn every_action_shape_is_recognised_as_one() {
    // a shape missing from here is a binding dropped without a word, so this is the list of shapes and
    // not a sample of them
    for action in [
        "x",
        "p,k.30",
        "m,1,2",
        "mk,0",
        "mb,left",
        "mb:left",
        "gb,south",
        "gb:south",
        "rec",
        "rec,7",
        "sv,next",
        "sv,prev",
        "sv,applet:docker",
        "sv:next",
    ] {
        assert!(
            is_action(action),
            "{action} is an action and is not recognised"
        );
    }
    // and settings that share the file are not actions
    for setting in ["255,255,255", "ALT-TAB", "true", ""] {
        assert!(!is_action(setting), "{setting} is not an action");
    }
    // a wrongly written one is still recognised as an action: it has to reach a parser to be reported, and a
    // line dropped here is a line nobody hears about
    assert!(
        is_action("rec,"),
        "a wrongly written action must still be seen"
    );
}
#[cfg(test)]
mod macro_tests {
    use super::*;

    const REAL: &str = "#Wed Sep 02 19:00:14 BST 2026\nsequence=kd.56,kd.15,d.20,ku.15,ku.56,d.100\nname=ALT-TAB\nid=1\n";

    #[test]
    fn a_real_macro_file_parses() {
        let macro_file = parse_macro(REAL);
        assert_eq!(macro_file.name, "ALT-TAB");
        assert_eq!(macro_file.id, 1);
        assert_eq!(
            macro_file.steps,
            vec![
                MacroStep::KeyDown(56),
                MacroStep::KeyDown(15),
                MacroStep::Delay(20),
                MacroStep::KeyUp(15),
                MacroStep::KeyUp(56),
                MacroStep::Delay(100),
            ]
        );
    }

    #[test]
    fn rubbish_in_a_sequence_is_skipped_rather_than_misread() {
        assert_eq!(
            parse_sequence("kd.56,wibble,d.,zz.4,kd.1"),
            vec![MacroStep::KeyDown(56), MacroStep::KeyDown(1)]
        );
    }

    #[test]
    fn an_empty_sequence_is_an_empty_macro() {
        let macro_file = parse_macro("sequence=\nname=x\nid=9\n");
        assert!(macro_file.steps.is_empty());
        assert_eq!(macro_file.id, 9);
    }
}

#[cfg(test)]
mod malformed_line_tests {
    use super::*;

    #[test]
    fn a_binding_whose_value_is_not_an_action_can_still_be_replaced() {
        // the previous stack wrote some values as bare key names, and `JUP=w` is one of them in the file
        let text = "G1=p,k.30
JUP=w
color=255,0,0
";
        let updated = set_binding(text, "JUP", "p,k.44");
        assert!(
            updated.contains("JUP=p,k.44"),
            "the line was not replaced: {updated}"
        );
        assert!(updated.contains("G1=p,k.30"), "another binding was lost");
        assert!(updated.contains("color=255,0,0"), "a setting was lost");
    }
}

#[cfg(test)]
mod colour_tests {
    use super::*;

    const FILE: &str = "G1=p,k.30\ncolor=255,0,0\nG2=x\n";

    #[test]
    fn the_colour_is_read_out_of_the_profile_it_belongs_to() {
        assert_eq!(
            colour_in(FILE),
            Some(Colour {
                red: 255,
                green: 0,
                blue: 0
            })
        );
        assert_eq!(
            colour_in("color=0,153,255\n"),
            Some(Colour {
                red: 0,
                green: 153,
                blue: 255
            })
        );
        // a file with no colour names none, which is not the same as black
        assert_eq!(colour_in("G1=p,k.30\n"), None);
        // a commented-out one is not a setting
        assert_eq!(colour_in("#color=255,0,0\n"), None);
        // and a wrong one is wrong rather than quietly something else
        assert_eq!(colour_in("color=300,0,0\n"), None);
        assert_eq!(colour_in("color=1,2\n"), None);
        assert_eq!(colour_in("color=1,2,3,4\n"), None);
    }

    #[test]
    fn setting_a_colour_keeps_every_other_line() {
        let updated = set_colour(
            FILE,
            Colour {
                red: 0,
                green: 153,
                blue: 255,
            },
        );
        assert_eq!(updated, "G1=p,k.30\ncolor=0,153,255\nG2=x\n");
        // and a file with none gains one, without losing what it had
        let added = set_colour(
            "G1=p,k.30\n",
            Colour {
                red: 1,
                green: 2,
                blue: 3,
            },
        );
        assert_eq!(added, "color=1,2,3\nG1=p,k.30\n");
        // setting it twice is the same as setting it once
        assert_eq!(
            set_colour(
                &updated,
                Colour {
                    red: 0,
                    green: 153,
                    blue: 255
                }
            ),
            updated
        );
    }

    #[test]
    fn binding_a_control_does_not_lose_the_colour() {
        // the colour is a line in a file that `g13 bind` rewrites a line of, and that is the whole risk
        let updated = set_binding(FILE, "G1", "p,k.31");
        assert!(updated.contains("color=255,0,0"), "{updated}");
        assert!(updated.contains("G1=p,k.31"), "{updated}");
    }
}

#[cfg(test)]
mod the_sets_a_user_makes {
    use super::*;

    #[test]
    fn a_set_keeps_its_number_until_it_is_given_a_key() {
        let profiles = Profiles::from_json(
            r#"{"1": {"name": "Testing"}, "2": {"name": "Cyberpunk", "key": "M1"}}"#,
        );
        assert_eq!(profiles.name(1), "Testing");
        assert_eq!(profiles.name(2), "Cyberpunk");
        assert_eq!(
            profiles.name(3),
            "Profile 3",
            "an unnamed set still reads as something"
        );
        // 2 was given M1, so M1 goes there...
        assert_eq!(profiles.profile_for_key("M1"), Some(2));
        // ...and every other key keeps the number printed on it
        assert_eq!(profiles.profile_for_key("M2"), Some(2));
        assert_eq!(profiles.profile_for_key("M3"), Some(3));
    }

    #[test]
    fn one_key_switches_to_one_set() {
        let mut profiles = Profiles::from_json(r#"{"1": {"key": "M1"}}"#);
        profiles.set_key(2, "M1");
        assert_eq!(
            profiles.profile_for_key("M1"),
            Some(2),
            "the newer assignment wins"
        );
        assert_eq!(profiles.key(1), None, "and the old one lost the key");
        // taking it off puts the key back to its own number
        profiles.clear_key(2);
        assert_eq!(profiles.profile_for_key("M1"), Some(1));
    }

    #[test]
    fn a_nameless_set_and_a_missing_file_are_both_fine() {
        let empty = Profiles::from_json("");
        assert_eq!(empty.name(7), "Profile 7");
        assert!(!empty.is_named(7));
        assert_eq!(empty.profile_for_key("M1"), Some(1));
        // a name that is only spaces is not a name
        let mut profiles = Profiles::from_json(r#"{"1": {"name": "  "}}"#);
        assert!(!profiles.is_named(1));
        profiles.set_name(1, "  Testing  ");
        assert_eq!(profiles.name(1), "Testing");
        profiles.set_name(1, "");
        assert!(
            !profiles.is_named(1),
            "clearing the name goes back to the number"
        );
    }

    #[test]
    fn it_round_trips_through_the_file_format() {
        let mut profiles = Profiles::default();
        profiles.set_name(0, "Default");
        profiles.set_name(2, "Cyberpunk");
        profiles.set_key(2, "M1");
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "0": {"name": "Default"},
            "2": {"name": "Cyberpunk", "key": "M1"},
        }))
        .unwrap();
        let read_back = Profiles::from_json(&text);
        assert_eq!(read_back.name(0), "Default");
        assert_eq!(read_back.name(2), "Cyberpunk");
        assert_eq!(read_back.profile_for_key("M1"), Some(2));
    }
}
