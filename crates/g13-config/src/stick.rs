//! The stick: what it is reporting, and what that means as a direction.
//!
//! The stick is two analog axes, not buttons. Each byte is unsigned and each axis has its **own** minimum,
//! centre and maximum, measured on this pad: x runs 10 (left) to 236 (right) around a centre of 130, and y
//! runs 11 (up) to 228 (down) around a centre of 119. None of that is assumed - it is read from
//! `stick.json`, which is what the calibration in the window fills in.
//!
//! Nothing here touches a device. It takes a reading, the calibration and a deadzone and answers which
//! direction, if any, the stick is being held in - so it can be tested against measured points taken from
//! the pad rather than against whichever pad happens to be to hand.
//!
//! Two rules that exist to stop a resting hand chattering:
//!
//! - **A deadzone.** The stick wanders several counts at rest, and a direction that fires on every twitch is
//!   unusable.
//! - **Hysteresis.** A direction that has been entered is held until the stick moves back *past* the
//!   deadzone by a margin. Without it, a stick held near the boundary repeats a direction many times a
//!   second, and a key that repeats is worse than a key that is slow.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The radial the stick is read as: a number of sectors, each with its own binding.
///
/// The stick's angle is what matters, not how far each axis has moved, so this breaks the full turn into as
/// many sectors as asked for and reports which one the stick is in. Eight is the shape the previous stack
/// used and the shape the familiar names describe, but nothing about eight is special: four makes a d-pad,
/// sixteen makes a wheel, and a sector count that does not divide 360 evenly is still a valid set of ranges.
///
/// **Sector 0 is up and they count clockwise**, which is how a person describes a stick rather than how
/// mathematics does. The eight names the old bindings files use are kept as aliases, each pointing at the
/// sector nearest the direction it names, so a file written for the eight-sector stick keeps working whatever
/// the count is set to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sectors {
    /// How many sectors the full turn is broken into.
    pub count: u32,
    /// How far past a boundary the stick has to go before the sector changes, as a fraction of a sector.
    ///
    /// Without this, a stick held near a boundary repeats two sectors dozens of times a second, and a key
    /// that repeats is worse than a key that is slow.
    pub hysteresis: f64,
}

impl Default for Sectors {
    fn default() -> Self {
        Sectors {
            count: 8,
            hysteresis: 0.25,
        }
    }
}

/// The names the old bindings files use, in degrees clockwise from up.
const NAMED: [(&str, f64); 8] = [
    ("JUP", 0.0),
    ("JUPRIGHT", 45.0),
    ("JRIGHT", 90.0),
    ("JDOWNRIGHT", 135.0),
    ("JDOWN", 180.0),
    ("JDOWNLEFT", 225.0),
    ("JLEFT", 270.0),
    ("JUPLEFT", 315.0),
];

impl Sectors {
    /// The fewest and most sectors worth having. Two is a coin toss and 64 is finer than a hand can aim.
    pub const LEAST: u32 = 2;
    /// The most sectors worth having: 64 is already finer than a hand can aim.
    pub const MOST: u32 = 64;

    /// Sectors of a given count, clamped to sit between `LEAST` and `MOST`, with the default hysteresis.
    pub fn new(count: u32) -> Self {
        Sectors {
            count: count.clamp(Self::LEAST, Self::MOST),
            ..Default::default()
        }
    }

    /// The width of one sector in degrees.
    pub fn width(&self) -> f64 {
        360.0 / self.count as f64
    }

    /// Where a sector's centre is, in degrees clockwise from up.
    pub fn centre(&self, index: u32) -> f64 {
        index as f64 * self.width()
    }

    /// The name a sector is bound under.
    pub fn name(&self, index: u32) -> String {
        format!("J{}", index % self.count)
    }

    /// Every name that will do for a sector: its own number, then any of the eight familiar names whose
    /// direction falls in it. So with eight sectors `JUP` and `J0` are the same binding, and with four they
    /// still are.
    pub fn names(&self, index: u32) -> Vec<String> {
        let mut names = vec![self.name(index)];
        for (name, degrees) in NAMED {
            if self.sector_at(degrees) == index % self.count {
                names.push(name.to_string());
            }
        }
        names
    }

    /// Whether a name in a bindings file names a sector of the stick rather than a control on the pad.
    ///
    /// Either one of the eight familiar names, or `J` followed by the number of a sector. No control on this
    /// pad is spelled that way, so the two cannot be confused - which is what lets a sector be bound in the
    /// same file, with the same syntax, as everything else.
    pub fn is_sector_name(name: &str) -> bool {
        let name = name.trim().to_ascii_uppercase();
        if NAMED.iter().any(|(known, _)| *known == name) {
            return true;
        }
        match name.strip_prefix('J') {
            Some(digits) => !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()),
            None => false,
        }
    }

    /// Which sector a direction in degrees falls in.
    pub fn sector_at(&self, degrees: f64) -> u32 {
        let turn = degrees.rem_euclid(360.0);
        // a sector straddles its centre, so half a width is added before dividing
        (((turn + self.width() / 2.0) / self.width()).floor() as i64).rem_euclid(self.count as i64)
            as u32
    }

    /// The direction a reading is being held in, in degrees clockwise from up, or `None` inside the deadzone.
    ///
    /// `x` and `y` are the calibrated offsets, positive right and positive *down* - which is why the angle is
    /// taken against the negated y: up is the direction a person starts counting from.
    pub fn degrees(&self, x: f64, y: f64) -> Option<f64> {
        if x == 0.0 && y == 0.0 {
            return None;
        }
        Some(x.atan2(-y).to_degrees().rem_euclid(360.0))
    }

    /// Which sector a position is in, given the sector that was held before it, if any.
    ///
    /// The previous sector is kept unless the stick has moved a whole sector away from its centre, which is
    /// the hysteresis: the boundary between two sectors only counts once it has been crossed properly.
    pub fn sector(&self, x: f64, y: f64, held: Option<u32>) -> Option<u32> {
        let degrees = self.degrees(x, y)?;
        if let Some(index) = held {
            let from_centre = (degrees - self.centre(index)).abs();
            // the short way round: 350 and 10 are twenty degrees apart, not three hundred and forty
            let from_centre = from_centre.min(360.0 - from_centre);
            if from_centre <= self.width() / 2.0 + self.width() * self.hysteresis {
                return Some(index);
            }
        }
        Some(self.sector_at(degrees))
    }

    /// A sector said as a person would: the name of the direction it sits on, or its bearing when it sits
    /// between the ones that have names.
    pub fn words(&self, index: u32) -> String {
        // The direction this sector sits *on*, not any that merely falls inside it. A four-sector stick spans
        // ninety degrees a sector, so each one contains three of the eight familiar directions, and taking the
        // first of them named the right-hand sector "up and right" and the left-hand one "down and left".
        let centre = self.centre(index);
        for (name, degrees) in NAMED {
            let apart = (degrees - centre).abs();
            if apart < 0.5 || (360.0 - apart) < 0.5 {
                let words = match name {
                    "JUP" => "up",
                    "JDOWN" => "down",
                    "JLEFT" => "left",
                    "JRIGHT" => "right",
                    "JUPLEFT" => "up and left",
                    "JUPRIGHT" => "up and right",
                    "JDOWNLEFT" => "down and left",
                    _ => "down and right",
                };
                return words.to_string();
            }
        }
        // a bearing, which is the only honest way to name one of sixteen
        format!("{:.0} degrees clockwise from up", self.centre(index))
    }

    /// Which of the four cardinals a sector leans towards, as (up, right, down, left).
    ///
    /// Used when a sector has no binding of its own: the cardinals it points between are what a keyboard can
    /// actually express, and for the eight-sector stick that reproduces the old behaviour exactly - the
    /// diagonal pressed both of the cardinals beside it.
    pub fn cardinals(&self, index: u32) -> (bool, bool, bool, bool) {
        let degrees = self.centre(index).to_radians();
        // a bearing clockwise from up: the horizontal part is the sine, and the vertical part is the cosine
        // *negated*, because on this stick a low y is up
        let right = degrees.sin();
        let down = -degrees.cos();
        // a hair of slack, so a sector sitting exactly on an axis does not claim the one beside it
        let leans = |value: f64| value.abs() > 0.01;
        (
            leans(down) && down < 0.0,
            leans(right) && right > 0.0,
            leans(down) && down > 0.0,
            leans(right) && right < 0.0,
        )
    }

    /// The sector nearest each cardinal, so a fallback can find where up, right, down and left are.
    pub fn cardinal_sectors(&self) -> (u32, u32, u32, u32) {
        (
            self.sector_at(0.0),
            self.sector_at(90.0),
            self.sector_at(180.0),
            self.sector_at(270.0),
        )
    }
}

/// What one axis was measured to do, from the nine calibration points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Axis {
    /// The reading measured at the axis's low end.
    pub low: u8,
    /// The reading measured with the stick resting.
    pub centre: u8,
    /// The reading measured at the axis's high end.
    pub high: u8,
}

impl Axis {
    /// How far the axis is from its centre, as a fraction of the distance to the extreme in that direction.
    ///
    /// Negative is towards `low`, positive towards `high`. Clamped to -1..1 so a reading slightly beyond a
    /// recorded extreme - which happens, because the extremes are held by a hand - cannot exceed full throw.
    pub fn offset(&self, reading: u8) -> f64 {
        let span = if reading >= self.centre {
            (self.high as i32 - self.centre as i32).max(1)
        } else {
            (self.centre as i32 - self.low as i32).max(1)
        };
        let distance = reading as i32 - self.centre as i32;
        (distance as f64 / span as f64).clamp(-1.0, 1.0)
    }
}

/// The calibration as measured, and what the stick is currently doing.
#[derive(Debug, Clone, PartialEq)]
pub struct Stick {
    /// The horizontal axis, low to high.
    pub x: Axis,
    /// Note the direction: on this pad **a low y is up**, so `high` here is *down*.
    pub y: Axis,
    /// How far from the centre a reading must be before it counts as a direction, as a fraction of full throw.
    pub deadzone: f64,
    /// Where the stick was last seen, and which sector that put it in.
    pub last: Option<(u8, u8)>,
    /// The sector the stick is in, which the next reading is judged against.
    pub held: Option<u32>,
}

impl Default for Stick {
    fn default() -> Self {
        // The values measured on this pad, so the type is useful before a calibration exists - but a real
        // calibration overwrites them, because they are a property of one particular pad rather than of the
        // device as a whole.
        Stick {
            x: Axis {
                low: 10,
                centre: 130,
                high: 236,
            },
            y: Axis {
                low: 11,
                centre: 119,
                high: 228,
            },
            deadzone: 0.35,
            last: None,
            held: None,
        }
    }
}

impl Stick {
    /// Build from a calibration file's contents, falling back to the measured defaults for anything absent.
    pub fn from_points(points: &BTreeMap<String, (u8, u8)>) -> Self {
        let mut stick = Stick::default();
        let get = |name: &str| points.get(name).copied();
        if let (Some((left, _)), Some((centre, _)), Some((right, _))) =
            (get("left"), get("centre"), get("right"))
        {
            stick.x = Axis {
                low: left,
                centre,
                high: right,
            };
        }
        if let (Some((_, up)), Some((_, centre)), Some((_, down))) =
            (get("up"), get("centre"), get("down"))
        {
            stick.y = Axis {
                low: up,
                centre,
                high: down,
            };
        }
        stick
    }

    /// Which way the stick is being held, if it is being held anywhere.
    ///
    /// The deadzone decides whether it is being held at all, measured as distance from the centre: a stick
    /// pushed into a corner is as far out as one pushed along an axis, and testing each axis on its own would
    /// have said otherwise. The sector's own hysteresis then decides which way, so a hand resting near a
    /// boundary between two sectors does not alternate between them.
    ///
    /// A sector already being held lets go at *half* the deadzone, so a stick barely past the edge does not
    /// flicker in and out.
    pub fn sector(&self, reading: (u8, u8), sectors: &Sectors) -> Option<u32> {
        let x = self.x.offset(reading.0);
        let y = self.y.offset(reading.1);
        let after = if self.held.is_some() {
            self.deadzone * 0.5
        } else {
            self.deadzone
        };
        if (x * x + y * y).sqrt() < after {
            return None;
        }
        sectors.sector(x, y, self.held)
    }

    /// Take a reading, and answer what changed: the sector that must be released, and the one held now.
    ///
    /// A change is two edges - the old sector goes up and the new one comes down - and the order matters for a
    /// chord like up-then-right becoming up-and-right.
    pub fn update(
        &mut self,
        reading: (u8, u8),
        sectors: &Sectors,
    ) -> Option<(Option<u32>, Option<u32>)> {
        if self.last == Some(reading) {
            return None;
        }
        self.last = Some(reading);
        let next = self.sector(reading, sectors);
        if next == self.held {
            return None;
        }
        let released = self.held;
        self.held = next;
        Some((released, next))
    }
}

/// Where one side of one axis of the stick is sent.
///
/// The stick has two axes but four sides, and they need not go to the same place: a racing game steers with
/// left and right but wants *forward* and *back* on the two analogue triggers, which are separate axes on a
/// controller and only ever move one way. So each side is routed on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// Nowhere. This side of the stick does nothing.
    None,
    /// The left stick, which is where a stick normally goes.
    LeftX,
    /// The left stick's vertical axis.
    LeftY,
    /// The right stick, for a game that reads that one - or for steering with the right hand.
    RightX,
    /// The right stick's vertical axis.
    RightY,
    /// The analogue triggers. A trigger only moves one way, so a whole side of an axis drives one.
    LeftTrigger,
    /// The right analogue trigger, which only moves one way.
    RightTrigger,
    /// The d-pad, as a direction rather than an amount: a game that wants a hat gets one.
    DpadX,
    /// The d-pad's vertical axis; with `DpadX` beside it, a hat is complete.
    DpadY,
}

impl Target {
    /// Every target, in the order the window lists them.
    pub const ALL: [Target; 9] = [
        Target::None,
        Target::LeftX,
        Target::LeftY,
        Target::RightX,
        Target::RightY,
        Target::LeftTrigger,
        Target::RightTrigger,
        Target::DpadX,
        Target::DpadY,
    ];

    /// The name this target is written under in `stick.json`.
    pub fn name(self) -> &'static str {
        match self {
            Target::None => "none",
            Target::LeftX => "left-x",
            Target::LeftY => "left-y",
            Target::RightX => "right-x",
            Target::RightY => "right-y",
            Target::LeftTrigger => "left-trigger",
            Target::RightTrigger => "right-trigger",
            Target::DpadX => "dpad-x",
            Target::DpadY => "dpad-y",
        }
    }

    /// The target a name means, synonyms included: `rt`, `gas` and `throttle` are all the right trigger.
    pub fn from_name(name: &str) -> Option<Target> {
        let name = name.trim().to_ascii_lowercase();
        Target::ALL
            .into_iter()
            .find(|target| target.name() == name)
            // the words people use for them: a gamepad's triggers are its shoulders on some pads, and the
            // d-pad has two spellings
            .or(match name.as_str() {
                "off" | "nothing" => Some(Target::None),
                "l2" | "lt" | "brake" => Some(Target::LeftTrigger),
                "r2" | "rt" | "throttle" | "gas" => Some(Target::RightTrigger),
                "dpad" | "hat" => Some(Target::DpadX),
                "stick-x" | "x" => Some(Target::LeftX),
                "stick-y" | "y" => Some(Target::LeftY),
                _ => None,
            })
    }
}

/// Where each side of each axis goes.
///
/// Named by direction rather than by sign, because "y+" is *down* on this stick and nobody should have to
/// remember that to edit the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    /// Where the stick being pushed right goes.
    pub right: Target,
    /// Where the stick being pushed left goes.
    pub left: Target,
    /// Where the stick being pushed down goes.
    pub down: Target,
    /// Where the stick being pushed up goes.
    pub up: Target,
}

impl Default for Route {
    fn default() -> Self {
        // The stick drives the left stick, which is what a stick normally does.
        Route {
            right: Target::LeftX,
            left: Target::LeftX,
            down: Target::LeftY,
            up: Target::LeftY,
        }
    }
}

impl Route {
    /// The four sides in a fixed order, for showing and for writing.
    pub fn sides(&self) -> [(&'static str, Target); 4] {
        [
            ("right", self.right),
            ("left", self.left),
            ("down", self.down),
            ("up", self.up),
        ]
    }

    /// Point one side at a target by name; a name that is not a side leaves the route as it was.
    pub fn set(&mut self, side: &str, target: Target) {
        match side {
            "right" => self.right = target,
            "left" => self.left = target,
            "down" => self.down = target,
            "up" => self.up = target,
            _ => {}
        }
    }

    /// Whether both sides of the horizontal axis go the same way and the vertical does too, which is the
    /// ordinary case and worth saying in a word.
    pub fn is_a_plain_stick(&self) -> bool {
        self.right == self.left && self.down == self.up
    }
}

/// Where the stick's calibration and settings are written.
pub fn stick_path() -> PathBuf {
    crate::config_dir().join("stick.json")
}

/// Whether the stick should be driving keys at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The stick does nothing. For a user who has not set it up, and for anyone who does not want it.
    Off,
    /// The turn is broken into sectors, and each sector does whatever it is bound to - `JUP=p,k.17` and its
    /// seven siblings are the eight-sector case of it.
    Keyboard,
    /// The stick moves the pointer: the further from the centre, the faster it travels.
    Mouse,
    /// The stick is a gamepad's analogue stick, reported as a position.
    Joystick,
}

impl Mode {
    /// The name this mode is written under in `stick.json`.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Keyboard => "keyboard",
            Mode::Mouse => "mouse",
            Mode::Joystick => "joystick",
        }
    }

    /// The mode a name means, synonyms included: `pointer` is mouse and `gamepad` is joystick.
    pub fn from_name(name: &str) -> Option<Mode> {
        match name.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Some(Mode::Off),
            "keyboard" | "keys" => Some(Mode::Keyboard),
            "mouse" | "pointer" => Some(Mode::Mouse),
            "joystick" | "gamepad" | "stick" => Some(Mode::Joystick),
            _ => None,
        }
    }

    /// Every mode, in the order the window lists them.
    pub const ALL: [Mode; 4] = [Mode::Off, Mode::Keyboard, Mode::Mouse, Mode::Joystick];
}

/// The stick's settings, alongside the calibration in the same file.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// What the stick drives, if anything.
    pub mode: Mode,
    /// How fast the pointer travels at full deflection, in pixels a second. Only mouse mode reads it.
    pub speed: f64,
    /// Where each side of each axis goes. Only joystick mode reads it.
    pub route: Route,
    /// How the turn is broken up. Only the sector modes read it.
    pub sectors: Sectors,
    /// The calibration the recorded points describe, or the built-in defaults when there are none.
    pub stick: Stick,
    /// The calibration as it was measured, kept so writing a setting cannot lose it.
    ///
    /// One file, one writer: the window fills these in and the driver reads them, and a second writer to this
    /// file would silently drop whichever part it did not know about.
    pub points: BTreeMap<String, (u8, u8)>,
    /// The file this was read from, which is where `save` writes it back.
    pub path: PathBuf,
}

impl Settings {
    /// The settings for a config directory given rather than assumed, with the built-in defaults for
    /// anything the file does not say.
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join("stick.json");
        let mut settings = Settings {
            mode: Mode::Off,
            speed: 600.0,
            route: Route::default(),
            sectors: Sectors::default(),
            stick: Stick::default(),
            points: BTreeMap::new(),
            path: path.clone(),
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return settings;
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) else {
            return settings;
        };
        if let Some(name) = parsed.get("mode").and_then(|mode| mode.as_str()) {
            if let Some(mode) = Mode::from_name(name) {
                settings.mode = mode;
            }
        }
        let mut points = BTreeMap::new();
        if let Some(object) = parsed.get("points").and_then(|points| points.as_object()) {
            for (name, value) in object {
                let pair = value.as_array().and_then(|pair| {
                    Some((pair.first()?.as_u64()? as u8, pair.get(1)?.as_u64()? as u8))
                });
                if let Some(pair) = pair {
                    points.insert(name.clone(), pair);
                }
            }
        }
        settings.stick = Stick::from_points(&points);
        settings.points = points;
        if let Some(deadzone) = parsed
            .get("deadzone")
            .and_then(|deadzone| deadzone.as_f64())
        {
            settings.stick.deadzone = deadzone.clamp(0.05, 0.9);
        }
        if let Some(speed) = parsed.get("speed").and_then(|speed| speed.as_f64()) {
            settings.speed = speed.clamp(20.0, 5000.0);
        }
        if let Some(count) = parsed.get("sectors").and_then(|count| count.as_u64()) {
            settings.sectors = Sectors::new(count as u32);
        }
        if let Some(route) = parsed.get("route").and_then(|route| route.as_object()) {
            for (side, target) in route {
                if let Some(target) = target.as_str().and_then(Target::from_name) {
                    settings.route.set(side, target);
                }
            }
        }
        settings
    }

    /// Record one calibration point, as the window does when a reading is taken.
    pub fn set_point(&mut self, name: &str, reading: (u8, u8)) {
        self.points.insert(name.to_string(), reading);
        self.stick = Stick::from_points(&self.points);
    }

    /// The points that have not been taken yet.
    pub fn missing(&self, wanted: &[&str]) -> Vec<String> {
        wanted
            .iter()
            .filter(|name| !self.points.contains_key(**name))
            .map(|name| (*name).to_string())
            .collect()
    }

    /// Write the whole file: the note, the mode, the deadzone and the points.
    pub fn save(&self) -> Result<(), String> {
        let mut out = String::from("{\n");
        out.push_str("  \"note\": \"what the stick reported at each point, taken while the driver was running\",\n");
        out.push_str(&format!("  \"mode\": \"{}\",\n", self.mode.name()));
        out.push_str(&format!("  \"deadzone\": {:.2},\n", self.stick.deadzone));
        out.push_str(&format!("  \"speed\": {:.0},\n", self.speed));
        out.push_str(&format!("  \"sectors\": {},\n", self.sectors.count));
        out.push_str("  \"route\": {\n");
        for (index, (side, target)) in self.route.sides().iter().enumerate() {
            let comma = if index == 3 { "" } else { "," };
            out.push_str(&format!("    \"{side}\": \"{}\"{comma}\n", target.name()));
        }
        out.push_str("  },\n");
        out.push_str("  \"points\": {\n");
        let names: Vec<&String> = self.points.keys().collect();
        for (index, name) in names.iter().enumerate() {
            let (x, y) = self.points[*name];
            let comma = if index + 1 == names.len() { "" } else { "," };
            out.push_str(&format!("    \"{name}\": [{x}, {y}]{comma}\n"));
        }
        out.push_str("  }\n}\n");
        g13_files::write(&self.path, &out)
            .map_err(|error| format!("{}: {error}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eight() -> Sectors {
        Sectors::default()
    }

    #[test]
    fn a_resting_stick_is_in_no_sector() {
        let stick = Stick::default();
        // the centre as measured on the pad, and the counts of jitter around it that it actually reports
        for reading in [(130, 119), (128, 121), (133, 117), (131, 119), (129, 120)] {
            assert_eq!(
                stick.sector(reading, &eight()),
                None,
                "{reading:?} read as a direction"
            );
        }
    }

    #[test]
    fn every_point_held_reads_as_the_direction_it_was_held_in() {
        // The nine calibration points measured on the pad, which are the specification for the eight-sector
        // case. Sector 0 is up and they count clockwise, so up is 0, up-and-right is 1, right is 2, and so on
        // round.
        let stick = Stick::default();
        let expected = [
            ((126, 11), 0, "up"),
            ((217, 57), 1, "up and right"),
            ((236, 123), 2, "right"),
            ((210, 199), 3, "down and right"),
            ((126, 228), 4, "down"),
            ((49, 202), 5, "down and left"),
            ((10, 130), 6, "left"),
            ((47, 40), 7, "up and left"),
        ];
        for (reading, wanted, words) in expected {
            assert_eq!(
                stick.sector(reading, &eight()),
                Some(wanted),
                "{reading:?} should be the sector for {words}"
            );
            assert_eq!(eight().words(wanted), words);
        }
    }

    #[test]
    fn four_sectors_are_the_four_directions_and_not_the_ones_between_them() {
        // A sector is named after the direction it sits *on*. With four sectors each one spans ninety degrees
        // and therefore contains three of the eight familiar directions, so naming it after the first of those
        // gave "up", "up and right", "down and right", "down and left" - the eight's names offset by a quarter
        // turn, rather than up, right, down and left.
        let four = Sectors::new(4);
        let said: Vec<String> = (0..4).map(|index| four.words(index)).collect();
        assert_eq!(said, vec!["up", "right", "down", "left"]);

        // eight still reads as the eight, each on its own centre
        let eight = Sectors::new(8);
        let said: Vec<String> = (0..8).map(|index| eight.words(index)).collect();
        assert_eq!(
            said,
            vec![
                "up",
                "up and right",
                "right",
                "down and right",
                "down",
                "down and left",
                "left",
                "up and left"
            ]
        );

        // and sixteen, where only every other sector sits on a named direction: the rest say their bearing
        let sixteen = Sectors::new(16);
        assert_eq!(sixteen.words(0), "up");
        assert_eq!(sixteen.words(2), "up and right");
        assert_eq!(sixteen.words(4), "right");
        assert_eq!(sixteen.words(1), "22 degrees clockwise from up");
    }

    #[test]
    fn the_familiar_names_still_point_at_the_sectors_they_always_did() {
        // The familiar names in a bindings file are JUP, JDOWN, JLEFT and JRIGHT. Whatever the sector count,
        // those names have to keep meaning the direction they name.
        let eight = eight();
        assert!(eight.names(0).contains(&"JUP".to_string()));
        assert!(eight.names(0).contains(&"J0".to_string()));
        assert!(eight.names(2).contains(&"JRIGHT".to_string()));
        assert!(eight.names(4).contains(&"JDOWN".to_string()));
        assert!(eight.names(6).contains(&"JLEFT".to_string()));
        assert!(eight.names(7).contains(&"JUPLEFT".to_string()));
        assert!(eight.names(1).contains(&"JUPRIGHT".to_string()));
    }

    #[test]
    fn four_sectors_is_a_d_pad_and_sixteen_is_a_wheel() {
        // The whole point of the generalisation: the count is a setting, and the names follow it.
        let four = Sectors::new(4);
        assert_eq!(four.count, 4);
        assert_eq!(four.sector_at(0.0), 0, "up");
        assert_eq!(four.sector_at(90.0), 1, "right");
        assert_eq!(four.sector_at(180.0), 2, "down");
        assert_eq!(four.sector_at(270.0), 3, "left");
        // and the familiar names still work at four, pointing at the quarter-turn they name
        assert!(four.names(0).contains(&"JUP".to_string()));
        assert!(four.names(1).contains(&"JRIGHT".to_string()));
        assert!(four.names(2).contains(&"JDOWN".to_string()));
        assert!(four.names(3).contains(&"JLEFT".to_string()));

        let sixteen = Sectors::new(16);
        assert_eq!(sixteen.sector_at(0.0), 0);
        assert_eq!(sixteen.sector_at(90.0), 4);
        assert_eq!(sixteen.sector_at(180.0), 8);
        assert_eq!(sixteen.sector_at(270.0), 12);
        // and the name of the sector nearest up-and-right is still the name for up-and-right
        assert!(sixteen.names(2).contains(&"JUPRIGHT".to_string()));
    }

    #[test]
    fn the_count_is_kept_within_reason() {
        assert_eq!(Sectors::new(0).count, Sectors::LEAST);
        assert_eq!(Sectors::new(1).count, Sectors::LEAST);
        assert_eq!(Sectors::new(1000).count, Sectors::MOST);
    }

    #[test]
    fn a_reading_past_a_recorded_extreme_does_not_exceed_full_throw() {
        let stick = Stick::default();
        assert!(stick.x.offset(255) <= 1.0);
        assert!(stick.x.offset(0) >= -1.0);
        assert_eq!(stick.sector((255, 119), &eight()), Some(2), "right");
    }

    #[test]
    fn a_calibration_from_the_file_overrides_the_defaults() {
        let mut points = BTreeMap::new();
        points.insert("centre".to_string(), (128, 128));
        points.insert("left".to_string(), (20, 128));
        points.insert("right".to_string(), (240, 128));
        points.insert("up".to_string(), (128, 20));
        points.insert("down".to_string(), (128, 240));
        let stick = Stick::from_points(&points);
        assert_eq!(stick.x.centre, 128);
        assert_eq!(stick.y.low, 20);
        // and the reading that is "up" on that pad is up here too
        assert_eq!(stick.sector((128, 20), &eight()), Some(0));
    }

    #[test]
    fn a_held_sector_is_given_up_only_after_the_stick_comes_back_past_half_the_deadzone() {
        let mut stick = Stick::default();
        let sectors = eight();
        assert_eq!(
            stick.update((236, 119), &sectors),
            Some((None, Some(2))),
            "right"
        );
        // drift back to a little outside the deadzone: still right, and no edge reported
        assert_eq!(stick.update((200, 119), &sectors), None);
        assert_eq!(stick.held, Some(2));
        // and back to the middle releases it, once
        assert_eq!(stick.update((130, 119), &sectors), Some((Some(2), None)));
        assert_eq!(stick.update((131, 119), &sectors), None);
    }

    #[test]
    fn a_sector_near_a_boundary_does_not_alternate_between_the_two_either_side() {
        // Without hysteresis this repeats two sectors dozens of times a second, which is the chatter that made
        // the stick unusable before a deadzone was added.
        let sectors = eight();
        let mut stick = Stick::default();
        // held in the middle of "up"
        assert_eq!(stick.update((130, 11), &sectors), Some((None, Some(0))));
        // then sitting right on the boundary between up and up-and-right: 22.5 degrees
        let boundary = (168, 60);
        let mut changes = 0;
        for _ in 0..10 {
            if stick.update(boundary, &sectors).is_some() {
                changes += 1;
            }
        }
        assert!(
            changes <= 1,
            "it changed sector {changes} times at a boundary"
        );
    }

    #[test]
    fn one_sector_change_releases_the_old_before_holding_the_new() {
        let mut stick = Stick::default();
        let sectors = eight();
        assert_eq!(stick.update((130, 11), &sectors), Some((None, Some(0))));
        // up becomes up-and-right
        assert_eq!(stick.update((236, 11), &sectors), Some((Some(0), Some(1))));
    }

    #[test]
    fn a_repeated_reading_reports_nothing() {
        let mut stick = Stick::default();
        let sectors = eight();
        assert!(stick.update((236, 119), &sectors).is_some());
        // the same bytes again: the driver publishes on every report, so this must be silent
        assert_eq!(stick.update((236, 119), &sectors), None);
    }

    #[test]
    fn a_diagonal_leans_on_the_two_cardinals_beside_it() {
        // What a keyboard can express: the sector has no binding of its own, so the cardinals it points
        // between are pressed instead.
        let eight = eight();
        // up-and-right leans on up and right, and on neither of the other two
        assert_eq!(eight.cardinals(1), (true, true, false, false));
        // up leans on up alone
        assert_eq!(eight.cardinals(0), (true, false, false, false));
        // and the cardinals of the eight-sector stick are where they always were
        assert_eq!(eight.cardinal_sectors(), (0, 2, 4, 6));
    }

    #[test]
    fn a_sector_without_a_familiar_name_is_named_by_its_bearing() {
        let sixteen = Sectors::new(16);
        // 22.5 degrees is between up and up-and-right, and has no word of its own
        let words = sixteen.words(1);
        assert!(
            words.contains("degrees clockwise from up"),
            "a sector with no name should be given as a bearing, got {words}"
        );
        // while the ones sitting on a familiar direction keep their word
        assert_eq!(sixteen.words(0), "up");
        assert_eq!(sixteen.words(4), "right");
    }

    #[test]
    fn settings_round_trip_and_keep_the_points() {
        let dir = std::env::temp_dir().join("g13-stick-settings-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("stick.json");
        std::fs::write(
            &path,
            r#"{"note":"x","points":{"centre":[130,119],"up":[126,11],"down":[126,228],
                "left":[10,130],"right":[236,123]}}"#,
        )
        .unwrap();

        let mut settings = Settings::load(&dir);
        assert_eq!(settings.mode, Mode::Off);
        assert_eq!(
            settings.sectors.count, 8,
            "eight sectors unless asked otherwise"
        );
        settings.mode = Mode::Keyboard;
        settings.stick.deadzone = 0.4;
        settings.sectors = Sectors::new(16);
        settings.save().unwrap();

        let again = Settings::load(&dir);
        assert_eq!(again.mode, Mode::Keyboard);
        assert!((again.stick.deadzone - 0.4).abs() < 1e-9);
        assert_eq!(again.sectors.count, 16);
        // the calibration survives a settings write, which is the thing that would silently lose it
        assert_eq!(again.stick.x.centre, 130);
        assert_eq!(again.stick.y.low, 11);

        // the mouse mode and its speed survive too, and the points still do
        let mut mouse = Settings::load(&dir);
        mouse.mode = Mode::Mouse;
        mouse.speed = 1200.0;
        mouse.save().unwrap();
        let read_back = Settings::load(&dir);
        assert_eq!(read_back.mode, Mode::Mouse);
        assert!((read_back.speed - 1200.0).abs() < 1e-9);
        assert_eq!(read_back.points.len(), 5);

        // and a point taken after that survives a second write
        let mut third = read_back;
        third.set_point("up-left", (47, 40));
        third.mode = Mode::Off;
        third.save().unwrap();
        let fourth = Settings::load(&dir);
        assert_eq!(fourth.points.get("up-left"), Some(&(47, 40)));
        assert_eq!(fourth.mode, Mode::Off);
        assert_eq!(fourth.points.len(), 6);
        assert!(
            fourth
                .missing(&["centre", "up", "down", "left", "right", "up-left"])
                .is_empty()
        );
        assert_eq!(
            fourth.missing(&["centre", "up-left", "down-right"]),
            vec!["down-right".to_string()]
        );
    }

    #[test]
    fn the_route_is_written_and_read_back() {
        let dir = std::env::temp_dir().join("g13-stick-route-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("stick.json");
        std::fs::write(&path, r#"{"note":"x","points":{}}"#).unwrap();

        let mut settings = Settings::load(&dir);
        assert!(settings.route.is_a_plain_stick(), "a stick by default");
        settings.route.set("up", Target::RightTrigger);
        settings.route.set("down", Target::LeftTrigger);
        settings.save().unwrap();

        let again = Settings::load(&dir);
        assert_eq!(again.route.up, Target::RightTrigger);
        assert_eq!(again.route.down, Target::LeftTrigger);
        assert_eq!(again.route.right, Target::LeftX, "the rest are untouched");
        assert!(!again.route.is_a_plain_stick());
    }
}
