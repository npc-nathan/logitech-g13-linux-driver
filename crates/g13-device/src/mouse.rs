//! The virtual pointer this program presents to the system.
//!
//! Mouse mode turns the stick into relative motion: the further from the centre it is held, the faster the
//! pointer travels, and letting go stops it. That is a different kind of device from the virtual keyboard -
//! a pointer reports *changes* in position rather than key states - so it is a second device rather than a
//! second use of the first.
//!
//! Written from the kernel's uinput interface through the evdev crate, named as this project names things.

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode};
use std::io;

/// The name this pointer appears under, which is what `evtest` and the desktop will show.
pub const DEVICE_NAME: &str = "g13 pointer";

/// Why the virtual pointer could not be created or written to.
#[derive(Debug)]
pub enum MouseError {
    /// The kernel refused a uinput call: creating the device, or emitting an event.
    Io(io::Error),
}

impl std::fmt::Display for MouseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MouseError::Io(error) => write!(
                f,
                "{error}\n(a virtual pointer needs write access to /dev/uinput)"
            ),
        }
    }
}

impl std::error::Error for MouseError {}

impl From<io::Error> for MouseError {
    fn from(error: io::Error) -> Self {
        MouseError::Io(error)
    }
}

/// A button on the pointer.
///
/// The eight buttons a mouse can report under Linux, in the order the kernel defines them. Named rather than
/// numbered because a number in a config file means nothing: which of `mb,3` and `mb,right` is the right-hand
/// button is not something a user should have to remember.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Button {
    /// The primary button, under the right hand's index finger.
    Left,
    /// Pressing the wheel.
    Middle,
    /// The secondary button, under the right hand's middle finger.
    Right,
    /// The first of the two thumb buttons, which the kernel numbers before `extra`.
    Side,
    /// The second of the two thumb buttons.
    Extra,
    /// Browser forward, where a mouse carries that pair of buttons.
    Forward,
    /// Browser back, beside `forward`.
    Back,
    /// The button that opens the window list or the overview.
    Task,
}

impl Button {
    /// Every button, in the order the kernel numbers them.
    pub const ALL: [Button; 8] = [
        Button::Left,
        Button::Middle,
        Button::Right,
        Button::Side,
        Button::Extra,
        Button::Forward,
        Button::Back,
        Button::Task,
    ];

    /// The button as it is written in a binding, like `left`.
    pub fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Middle => "middle",
            Button::Right => "right",
            Button::Side => "side",
            Button::Extra => "extra",
            Button::Forward => "forward",
            Button::Back => "back",
            Button::Task => "task",
        }
    }

    /// The button a name refers to, accepting the other words people use for them.
    ///
    /// `middle`, `wheel` and `centre` are one button, so a config does not have to use the spelling this
    /// project picked. Nothing is guessed: a name that is none of them is an absence, not a default.
    pub fn from_name(name: &str) -> Option<Button> {
        let name = name.trim().to_ascii_lowercase();
        Button::ALL
            .into_iter()
            .find(|button| button.name() == name)
            // the two names people actually use for the third button
            .or(match name.as_str() {
                "middle" | "wheel" | "centre" | "center" => Some(Button::Middle),
                "side1" => Some(Button::Side),
                "side2" => Some(Button::Extra),
                _ => None,
            })
    }

    /// The kernel's own code for this button, which is what a program reading the pointer sees.
    pub fn code(self) -> KeyCode {
        match self {
            Button::Left => KeyCode::BTN_LEFT,
            Button::Middle => KeyCode::BTN_MIDDLE,
            Button::Right => KeyCode::BTN_RIGHT,
            Button::Side => KeyCode::BTN_SIDE,
            Button::Extra => KeyCode::BTN_EXTRA,
            Button::Forward => KeyCode::BTN_FORWARD,
            Button::Back => KeyCode::BTN_BACK,
            Button::Task => KeyCode::BTN_TASK,
        }
    }
}

/// A virtual pointer.
pub struct VirtualMouse {
    /// The uinput device itself, which owns the node and takes it away when dropped.
    device: VirtualDevice,
    /// Which buttons are down, so they can all be let go of when the stick is turned off or this stops.
    /// A button left down with nothing holding it is a stuck button, and the user has no way to release it.
    down: Vec<Button>,
}

impl VirtualMouse {
    /// Create the virtual pointer.
    ///
    /// Every button a mouse can report is advertised whether or not anything is bound to it. A pointer that
    /// reports only motion is still a pointer, but a game or an application asking it for a button it does
    /// not have gets nothing at all - and a device cannot gain a button after it has been created.
    pub fn new() -> Result<Self, MouseError> {
        let mut axes = AttributeSet::<RelativeAxisCode>::new();
        axes.insert(RelativeAxisCode::REL_X);
        axes.insert(RelativeAxisCode::REL_Y);

        let mut buttons = AttributeSet::<KeyCode>::new();
        for button in Button::ALL {
            buttons.insert(button.code());
        }

        let device = VirtualDevice::builder()?
            .name(DEVICE_NAME)
            .with_relative_axes(&axes)?
            .with_keys(&buttons)?
            .build()?;
        Ok(Self {
            device,
            down: Vec::new(),
        })
    }

    /// Move the pointer by a relative amount.
    ///
    /// Both axes go in one batch, which `emit` terminates with a single `SYN_REPORT` - a consumer that sees
    /// the x movement without the y would draw a diagonal as two separate jumps.
    pub fn move_by(&mut self, x: i32, y: i32) -> Result<(), MouseError> {
        if x == 0 && y == 0 {
            return Ok(());
        }
        self.device.emit(&[
            InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, x),
            InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, y),
        ])?;
        Ok(())
    }

    /// Hold one button down, remembering it so it can be let go of later.
    pub fn press(&mut self, button: Button) -> Result<(), MouseError> {
        if !self.down.contains(&button) {
            self.down.push(button);
        }
        self.device
            .emit(&[InputEvent::new(EventType::KEY.0, button.code().code(), 1)])?;
        Ok(())
    }

    /// Let one button up.
    pub fn release(&mut self, button: Button) -> Result<(), MouseError> {
        self.down.retain(|held| *held != button);
        self.device
            .emit(&[InputEvent::new(EventType::KEY.0, button.code().code(), 0)])?;
        Ok(())
    }

    /// Let go of every button this is holding.
    pub fn release_all(&mut self) -> Result<(), MouseError> {
        for button in std::mem::take(&mut self.down) {
            self.device
                .emit(&[InputEvent::new(EventType::KEY.0, button.code().code(), 0)])?;
        }
        Ok(())
    }

    /// Where the kernel put this device.
    ///
    /// A uinput device's name cannot be used to find it: a driver and a program testing that driver can both
    /// have a device called the same thing, and matching on the name finds whichever was created first. This
    /// path is unique to this device, and the event node is the `event*` entry inside it.
    pub fn syspath(&mut self) -> Option<std::path::PathBuf> {
        self.device.get_syspath().ok()
    }

    /// Which buttons are currently down, for a caller that wants to report or restore them.
    pub fn down(&self) -> &[Button] {
        &self.down
    }
}
