//! The virtual stick this program presents to the system.
//!
//! Joystick mode hands the stick's two axes to whatever is reading a gamepad, as an analogue position rather
//! than a pointer or a set of keys. An absolute axis reports *where* it is, not how far it moved, so this is a
//! third kind of device: the pointer reports changes, the keyboard reports key states, and this reports a
//! position that has to be centred again when nothing is holding it.
//!
//! Written from the kernel's uinput interface through the evdev crate, named as this project names things.

use evdev::uinput::VirtualDevice;
use evdev::{AbsInfo, AbsoluteAxisCode, EventType, InputEvent, KeyCode, UinputAbsSetup};
use std::io;

/// The name this stick appears under, which is what `evtest`, `jstest` and the desktop will show.
pub const DEVICE_NAME: &str = "g13 stick";

/// Full deflection.
///
/// The range every axis of a gamepad is expected to use, so a game that maps an axis to a movement gets the
/// whole of it. Any other range works, but only this one needs no explanation to the thing reading it.
const FULL: i32 = 32767;

/// The top of a trigger's travel. A controller's trigger reports a byte, so a byte is what this reports.
const TRIGGER_FULL: i32 = 255;

/// How far from the centre the reading may wander before it is treated as centred at all.
///
/// Reported to the kernel as the axis's flat area, so a game applying its own deadzone to our numbers does not
/// stack a second one on top of ours for a stick that is already resting.
const FLAT: i32 = 128;

/// Why the virtual stick could not be created or written to.
#[derive(Debug)]
pub enum StickError {
    /// The kernel refused a uinput call: creating the device, or emitting an event.
    Io(io::Error),
}

impl std::fmt::Display for StickError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StickError::Io(error) => write!(
                f,
                "{error}\n(a virtual gamepad needs write access to /dev/uinput)"
            ),
        }
    }
}

impl std::error::Error for StickError {}

impl From<io::Error> for StickError {
    fn from(error: io::Error) -> Self {
        StickError::Io(error)
    }
}

/// An axis a gamepad reports.
///
/// The eight a controller has between its two sticks, two triggers and hat. A pad that advertised only the
/// first two could never be routed anywhere else, and a device cannot gain an axis after it is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Axis {
    /// The left stick's horizontal axis, positive being right.
    LeftX,
    /// The left stick's vertical axis, positive being down.
    LeftY,
    /// The right stick's horizontal axis, positive being right.
    RightX,
    /// The right stick's vertical axis, positive being down.
    RightY,
    /// A trigger, which unlike a stick only moves one way.
    LeftTrigger,
    /// The right trigger, which rests at nothing and only travels one way.
    RightTrigger,
    /// The hat, which has three positions rather than a range.
    DpadX,
    /// The hat's vertical axis: -1 up, 0 centred, 1 down.
    DpadY,
}

impl Axis {
    /// Every axis, in the order they are indexed and reported. All eight are advertised when the device is
    /// created, because a device cannot gain an axis afterwards.
    pub const ALL: [Axis; 8] = [
        Axis::LeftX,
        Axis::LeftY,
        Axis::RightX,
        Axis::RightY,
        Axis::LeftTrigger,
        Axis::RightTrigger,
        Axis::DpadX,
        Axis::DpadY,
    ];

    /// The axis as it is written in a binding, like `left-x`.
    pub fn name(self) -> &'static str {
        match self {
            Axis::LeftX => "left-x",
            Axis::LeftY => "left-y",
            Axis::RightX => "right-x",
            Axis::RightY => "right-y",
            Axis::LeftTrigger => "left-trigger",
            Axis::RightTrigger => "right-trigger",
            Axis::DpadX => "dpad-x",
            Axis::DpadY => "dpad-y",
        }
    }

    /// The kernel's own code for this axis, which is what a game reading the pad sees.
    pub fn code(self) -> AbsoluteAxisCode {
        match self {
            Axis::LeftX => AbsoluteAxisCode::ABS_X,
            Axis::LeftY => AbsoluteAxisCode::ABS_Y,
            Axis::RightX => AbsoluteAxisCode::ABS_RX,
            Axis::RightY => AbsoluteAxisCode::ABS_RY,
            Axis::LeftTrigger => AbsoluteAxisCode::ABS_Z,
            Axis::RightTrigger => AbsoluteAxisCode::ABS_RZ,
            Axis::DpadX => AbsoluteAxisCode::ABS_HAT0X,
            Axis::DpadY => AbsoluteAxisCode::ABS_HAT0Y,
        }
    }

    /// How this axis is measured, which its kind decides: a stick is centred, a trigger rests at nothing and
    /// only travels one way, and a hat has three positions.
    fn kind(self) -> Kind {
        match self {
            Axis::LeftTrigger | Axis::RightTrigger => Kind::Trigger,
            Axis::DpadX | Axis::DpadY => Kind::Hat,
            _ => Kind::Stick,
        }
    }
}

/// What sort of measurement an axis carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A centred axis that travels both ways from the middle.
    Stick,
    /// An axis that rests at nothing and only travels one way.
    Trigger,
    /// A three-position axis, one of the d-pad's directions.
    Hat,
}

/// A button on the gamepad.
///
/// The standard set every gamepad reports, named as a person would name them rather than by the kernel's
/// letters: `south` is the bottom face button whatever a given pad prints on it, which is the only name that
/// means the same thing on a Playstation pad, an Xbox pad and anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Button {
    /// The bottom face button: A on an Xbox pad, cross on a Playstation pad.
    South,
    /// The right face button: B on an Xbox pad, circle on a Playstation pad.
    East,
    /// The top face button: Y on an Xbox pad, triangle on a Playstation pad.
    North,
    /// The left face button: X on an Xbox pad, square on a Playstation pad.
    West,
    /// The left shoulder button, the click above the left trigger.
    Lb,
    /// The right shoulder button, the click above the right trigger.
    Rb,
    /// The left trigger, held rather than clicked. A real pad reports it on an axis instead of a button.
    Lt,
    /// The right trigger, held rather than clicked. A real pad reports it on an axis instead of a button.
    Rt,
    /// Select, or share: the small button at the left of the middle.
    Back,
    /// Start, or options: the small button at the right of the middle.
    Start,
    /// The button in the middle that opens the platform's own menu.
    Guide,
    /// Pressing the left stick down.
    ThumbL,
    /// Pressing the right stick down.
    ThumbR,
    /// The hat pushed up, away from the user.
    DpadUp,
    /// The hat pushed down, towards the user.
    DpadDown,
    /// The hat pushed left.
    DpadLeft,
    /// The hat pushed right.
    DpadRight,
}

impl Button {
    /// Every button, in the order they are named and advertised. All of them are, whether or not a binding
    /// uses one, because a game asking for a button a pad does not have gets nothing.
    pub const ALL: [Button; 17] = [
        Button::South,
        Button::East,
        Button::North,
        Button::West,
        Button::Lb,
        Button::Rb,
        Button::Lt,
        Button::Rt,
        Button::Back,
        Button::Start,
        Button::Guide,
        Button::ThumbL,
        Button::ThumbR,
        Button::DpadUp,
        Button::DpadDown,
        Button::DpadLeft,
        Button::DpadRight,
    ];

    /// The button as it is written in a binding, like `dpad-up`.
    pub fn name(self) -> &'static str {
        match self {
            Button::South => "south",
            Button::East => "east",
            Button::North => "north",
            Button::West => "west",
            Button::Lb => "lb",
            Button::Rb => "rb",
            Button::Lt => "lt",
            Button::Rt => "rt",
            Button::Back => "back",
            Button::Start => "start",
            Button::Guide => "guide",
            Button::ThumbL => "thumb-l",
            Button::ThumbR => "thumb-r",
            Button::DpadUp => "dpad-up",
            Button::DpadDown => "dpad-down",
            Button::DpadLeft => "dpad-left",
            Button::DpadRight => "dpad-right",
        }
    }

    /// The button a name refers to, accepting the letters printed on a pad and the words people use for them.
    ///
    /// `a`, `cross` and `bottom` are one button: which one a pad prints depends on who made it, and the
    /// kernel asks for `BTN_SOUTH` whoever made it, so these are spellings rather than devices.
    pub fn from_name(name: &str) -> Option<Button> {
        let name = name.trim().to_ascii_lowercase();
        Button::ALL
            .into_iter()
            .find(|button| button.name() == name)
            // The letters printed on a pad, and the words people use for them, all pointing at the same
            // buttons. A game asks for BTN_SOUTH whoever made the pad, so these are spellings, not devices.
            .or(match name.as_str() {
                "a" | "cross" | "bottom" => Some(Button::South),
                "b" | "circle" | "right" => Some(Button::East),
                "x" | "square" | "left" => Some(Button::West),
                "y" | "triangle" | "top" => Some(Button::North),
                "l1" | "left-shoulder" => Some(Button::Lb),
                "r1" | "right-shoulder" => Some(Button::Rb),
                "l2" | "left-trigger" => Some(Button::Lt),
                "r2" | "right-trigger" => Some(Button::Rt),
                "select" | "share" => Some(Button::Back),
                "options" | "menu" => Some(Button::Start),
                "home" | "ps" | "xbox" => Some(Button::Guide),
                "l3" | "left-stick" => Some(Button::ThumbL),
                "r3" | "right-stick" => Some(Button::ThumbR),
                "up" => Some(Button::DpadUp),
                "down" => Some(Button::DpadDown),
                "dp-left" => Some(Button::DpadLeft),
                "dp-right" => Some(Button::DpadRight),
                _ => None,
            })
    }

    /// The kernel's own code for this button, which is what a game reading the pad sees.
    pub fn code(self) -> KeyCode {
        match self {
            Button::South => KeyCode::BTN_SOUTH,
            Button::East => KeyCode::BTN_EAST,
            Button::North => KeyCode::BTN_NORTH,
            Button::West => KeyCode::BTN_WEST,
            Button::Lb => KeyCode::BTN_TL,
            Button::Rb => KeyCode::BTN_TR,
            Button::Lt => KeyCode::BTN_TL2,
            Button::Rt => KeyCode::BTN_TR2,
            Button::Back => KeyCode::BTN_SELECT,
            Button::Start => KeyCode::BTN_START,
            Button::Guide => KeyCode::BTN_MODE,
            Button::ThumbL => KeyCode::BTN_THUMBL,
            Button::ThumbR => KeyCode::BTN_THUMBR,
            Button::DpadUp => KeyCode::BTN_DPAD_UP,
            Button::DpadDown => KeyCode::BTN_DPAD_DOWN,
            Button::DpadLeft => KeyCode::BTN_DPAD_LEFT,
            Button::DpadRight => KeyCode::BTN_DPAD_RIGHT,
        }
    }
}

/// A virtual gamepad with one stick.
pub struct VirtualStick {
    /// The uinput device itself, which owns the node and takes it away when dropped.
    device: VirtualDevice,
    /// Where each axis was last set, so an unchanged position is not re-sent sixty times a second. Indexed by
    /// the axis itself rather than by field, because there are eight of them now and they move independently.
    last: [i32; 8],
    /// Which buttons are down, so they can all be released when the mode is left or this stops.
    down: Vec<Button>,
}

impl VirtualStick {
    /// Create the virtual stick.
    ///
    /// Two absolute axes and the standard button set. The buttons are advertised so that a pad control bound
    /// to one can actually press it - a device cannot gain a button after it has been created, so a gamepad
    /// that had none could never be told to press one.
    pub fn new() -> Result<Self, StickError> {
        let mut buttons = evdev::AttributeSet::<KeyCode>::new();
        for button in Button::ALL {
            buttons.insert(button.code());
        }
        let mut builder = VirtualDevice::builder()?.name(DEVICE_NAME);
        for axis in Axis::ALL {
            let info = match axis.kind() {
                Kind::Stick => AbsInfo::new(0, -FULL, FULL, 0, FLAT, 0),
                // a trigger rests at nothing and travels to the top of a byte, which is what a controller's
                // trigger reports
                Kind::Trigger => AbsInfo::new(0, 0, TRIGGER_FULL, 0, 0, 0),
                // a hat has three positions and no range: -1, 0 or 1
                Kind::Hat => AbsInfo::new(0, -1, 1, 0, 0, 0),
            };
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(axis.code(), info))?;
        }
        let device = builder.with_keys(&buttons)?.build()?;
        Ok(Self {
            device,
            last: [0; 8],
            down: Vec::new(),
        })
    }

    /// Set one axis, given as -1.0 to 1.0.
    ///
    /// What a value means depends on the axis: a stick is centred and travels both ways, a trigger rests at
    /// nothing and only travels up, and a hat snaps to one of three positions because that is all it has.
    ///
    /// Only the axes that moved are sent. An absolute axis re-sent unchanged sixty times a second is sixty
    /// events a second of nothing, and a game reading them sees a stick constantly moving to where it already
    /// is.
    pub fn set_axis(&mut self, axis: Axis, value: f64) -> Result<(), StickError> {
        let wanted = match axis.kind() {
            Kind::Stick => to_axis(value),
            Kind::Trigger => (value.clamp(0.0, 1.0) * TRIGGER_FULL as f64).round() as i32,
            // a hat has no middle ground: a direction or nothing
            Kind::Hat => {
                if value > 0.5 {
                    1
                } else if value < -0.5 {
                    -1
                } else {
                    0
                }
            }
        };
        let slot = axis as usize;
        if self.last[slot] == wanted {
            return Ok(());
        }
        self.last[slot] = wanted;
        self.device.emit(&[InputEvent::new(
            EventType::ABSOLUTE.0,
            axis.code().0,
            wanted,
        )])?;
        Ok(())
    }

    /// Put the left stick at a position, positive being right and down.
    pub fn set_position(&mut self, x: f64, y: f64) -> Result<(), StickError> {
        self.set_axis(Axis::LeftX, x)?;
        self.set_axis(Axis::LeftY, y)
    }

    /// Put every axis back to rest.
    ///
    /// Called when the mode is left, and when the driver stops. An absolute axis left at full deflection stays
    /// there for whatever is reading it - a game would keep turning with nothing touching the stick, and a
    /// trigger left open would hold the throttle down with no way to lift it.
    pub fn centre(&mut self) -> Result<(), StickError> {
        for axis in Axis::ALL {
            self.set_axis(axis, 0.0)?;
        }
        Ok(())
    }

    /// Where one axis is set to, for reporting.
    pub fn position(&self, axis: Axis) -> i32 {
        self.last[axis as usize]
    }

    /// Hold one button down.
    ///
    /// The d-pad and the two triggers are also reported on the axes a real pad uses for them, because a game
    /// reads the hat and `ABS_Z`/`ABS_RZ` and never sees the matching buttons.
    pub fn press(&mut self, button: Button) -> Result<(), StickError> {
        if !self.down.contains(&button) {
            self.down.push(button);
        }
        self.device
            .emit(&[InputEvent::new(EventType::KEY.0, button.code().code(), 1)])?;
        self.report_on_axes()
    }

    /// Let one button up, and put its axis back to rest where it has one.
    pub fn release(&mut self, button: Button) -> Result<(), StickError> {
        self.down.retain(|held| *held != button);
        self.device
            .emit(&[InputEvent::new(EventType::KEY.0, button.code().code(), 0)])?;
        self.report_on_axes()
    }

    /// Let go of every button, for when the mode is left or the driver stops. A button left down with nothing
    /// holding it has no way to be released, and the game holds it for ever.
    pub fn release_all(&mut self) -> Result<(), StickError> {
        for button in std::mem::take(&mut self.down) {
            self.device
                .emit(&[InputEvent::new(EventType::KEY.0, button.code().code(), 0)])?;
        }
        self.report_on_axes()
    }

    /// Report the d-pad and the two triggers on the axes a real pad uses for them, as well as on the buttons.
    ///
    /// **A real pad does not report these as buttons.** Measured from the Xbox 360 pad on this machine, from
    /// `/sys/class/input/event*/device/capabilities/key`: it advertises no `BTN_DPAD_*` and no `BTN_TL2` or
    /// `BTN_TR2` at all, and reports its d-pad on the hat (`ABS_HAT0X`/`ABS_HAT0Y`) and its two triggers on
    /// `ABS_Z`/`ABS_RZ`. A game written against real hardware reads the hat and the axes, so a binding that
    /// only pressed the button did nothing in it - which is the report word for word: every controller button
    /// worked *except* the d-pad and the two triggers.
    ///
    /// Both are kept. The button costs a reader that ignores it nothing, and dropping it would break anything
    /// that does read `BTN_DPAD_*` - the events already reached the kernel for those.
    ///
    /// The hat is worked out from every d-pad button that is down rather than sent per press, so holding a
    /// diagonal gives the diagonal and holding nothing gives nothing. `set_axis` skips an unchanged value, so
    /// this is one event when something moved and none when nothing did.
    fn report_on_axes(&mut self) -> Result<(), StickError> {
        let (left, right) = (
            self.down.contains(&Button::DpadLeft),
            self.down.contains(&Button::DpadRight),
        );
        let (up, down) = (
            self.down.contains(&Button::DpadUp),
            self.down.contains(&Button::DpadDown),
        );
        let (x, y) = hat_from(up, down, left, right);
        self.set_axis(Axis::DpadX, x as f64)?;
        self.set_axis(Axis::DpadY, y as f64)?;
        // a trigger only travels one way, so it is full or nothing
        let (lt, rt) = (
            self.down.contains(&Button::Lt),
            self.down.contains(&Button::Rt),
        );
        self.set_axis(Axis::LeftTrigger, if lt { 1.0 } else { 0.0 })?;
        self.set_axis(Axis::RightTrigger, if rt { 1.0 } else { 0.0 })
    }

    /// Where the kernel put this device.
    ///
    /// A uinput device's name cannot be used to find it: a driver and a program testing that driver can both
    /// have a device called the same thing, and matching on the name finds whichever was created first. This
    /// path is unique to this device, and the event node is the `event*` entry inside it.
    pub fn syspath(&mut self) -> Option<std::path::PathBuf> {
        self.device.get_syspath().ok()
    }

    /// Which buttons are down, for a caller that wants to report them.
    pub fn down(&self) -> &[Button] {
        &self.down
    }
}

/// What a hat reads, given which of its four directions are being held.
///
/// A hat counts -1 for up and left and 1 for down and right, and it has one value per axis, so opposite
/// directions held together cancel - which is what a real pad's hat does when a game asks it for one number.
pub fn hat_from(up: bool, down: bool, left: bool, right: bool) -> (i32, i32) {
    let x = match (left, right) {
        (true, false) => -1,
        (false, true) => 1,
        _ => 0,
    };
    let y = match (up, down) {
        (true, false) => -1,
        (false, true) => 1,
        _ => 0,
    };
    (x, y)
}

/// One axis, from a -1.0 to 1.0 position to the range a gamepad uses.
fn to_axis(position: f64) -> i32 {
    let scaled = position.clamp(-1.0, 1.0) * FULL as f64;
    (scaled.round() as i32).clamp(-FULL, FULL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hat_counts_up_and_left_as_negative() {
        assert_eq!(hat_from(true, false, false, false), (0, -1), "up");
        assert_eq!(hat_from(false, true, false, false), (0, 1), "down");
        assert_eq!(hat_from(false, false, true, false), (-1, 0), "left");
        assert_eq!(hat_from(false, false, false, true), (1, 0), "right");
        assert_eq!(hat_from(false, false, false, false), (0, 0), "nothing held");
    }

    #[test]
    fn a_hat_has_one_value_per_axis_so_a_diagonal_is_two_of_them() {
        // which is how a real pad reports a diagonal, and why holding up and left is not a tie
        assert_eq!(hat_from(true, false, true, false), (-1, -1), "up and left");
        assert_eq!(hat_from(false, true, false, true), (1, 1), "down and right");
        // and opposite directions on one axis cancel, because the hat cannot say both
        assert_eq!(hat_from(true, true, false, false), (0, 0));
        assert_eq!(hat_from(false, false, true, true), (0, 0));
    }

    #[test]
    fn a_real_pad_reports_the_d_pad_and_the_triggers_on_axes_not_buttons() {
        // The measurement this whole path exists for, pinned so it cannot be forgotten: the Xbox 360 pad on
        // this machine advertises no BTN_DPAD_* and no BTN_TL2/BTN_TR2, and puts the d-pad on the hat and the
        // triggers on ABS_Z/ABS_RZ. If that ever stops being what this device presents, a game stops seeing
        // the d-pad and the triggers.
        assert_eq!(Button::DpadUp.code(), KeyCode::BTN_DPAD_UP);
        assert_eq!(Axis::DpadX.code(), AbsoluteAxisCode::ABS_HAT0X);
        assert_eq!(Axis::DpadY.code(), AbsoluteAxisCode::ABS_HAT0Y);
        assert_eq!(Axis::LeftTrigger.code(), AbsoluteAxisCode::ABS_Z);
        assert_eq!(Axis::RightTrigger.code(), AbsoluteAxisCode::ABS_RZ);
        // and the two the real pad does not have are the ones bound to these names
        assert_eq!(Button::Lt.code(), KeyCode::BTN_TL2);
        assert_eq!(Button::Rt.code(), KeyCode::BTN_TR2);
    }

    #[test]
    fn a_full_deflection_reaches_the_ends_of_the_range() {
        assert_eq!(to_axis(1.0), FULL);
        assert_eq!(to_axis(-1.0), -FULL);
        assert_eq!(to_axis(0.0), 0);
        // and beyond it does not exceed them, because a reading past a recorded extreme does happen
        assert_eq!(to_axis(4.0), FULL);
        assert_eq!(to_axis(-4.0), -FULL);
    }

    #[test]
    fn an_axis_keeps_its_sign_the_way_a_gamepad_reads_it() {
        // right and down are positive, which is what a game expects from ABS_X and ABS_Y
        assert!(to_axis(0.5) > 0);
        assert!(to_axis(-0.5) < 0);
    }

    #[test]
    fn the_axis_ranges_are_the_ones_a_controller_uses() {
        // the kernel's own limits for each kind, so a game reading the range is told the truth
        assert_eq!(
            AbsInfo::new(0, -FULL, FULL, 0, FLAT, 0).minimum(),
            -32767,
            "a stick should be centred"
        );
        assert_eq!(TRIGGER_FULL, 255, "a trigger should travel a byte");
        assert_eq!(Axis::LeftTrigger.kind(), Kind::Trigger);
        assert_eq!(Axis::DpadX.kind(), Kind::Hat);
        assert_eq!(Axis::RightX.kind(), Kind::Stick);
    }

    #[test]
    fn every_axis_has_its_own_name_and_code() {
        let mut names: Vec<&str> = Axis::ALL.iter().map(|axis| axis.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 8, "two axes share a name");
        let mut codes: Vec<u16> = Axis::ALL.iter().map(|axis| axis.code().0).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), 8, "two axes share a code");
    }
}
