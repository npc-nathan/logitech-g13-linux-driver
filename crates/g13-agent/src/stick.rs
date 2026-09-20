//! The stick, routed: which axis a target means, and one worker that drives the virtual devices from it.
//!
//! The pad's stick is not a set of directions to this driver - it is a pair of absolute axes that a person can
//! move to any position - so what it does is decided by the profile's own routing (`stick:` in `stick.json`) and
//! carried out on a thread of its own, one worker per device the routing ends up needing.
//!
//! A binding for a sector and a routing target are two different things, and the names they use are kept apart on
//! purpose: the sector names are the stick's own, and none of them is a control on the pad.

use crate::{Bindings, describe_action};
use g13_device::joystick::VirtualStick;
use g13_device::keyboard::{self, KeyCode};
use g13_device::mouse::VirtualMouse;
use std::sync::Mutex;
use std::time::Duration;

/// The gamepad axis a routing target means, if it means one.
pub(crate) fn axis_for_target(
    target: g13_config::stick::Target,
) -> Option<g13_device::joystick::Axis> {
    use g13_config::stick::Target;
    use g13_device::joystick::Axis;
    match target {
        Target::None => None,
        Target::LeftX => Some(Axis::LeftX),
        Target::LeftY => Some(Axis::LeftY),
        Target::RightX => Some(Axis::RightX),
        Target::RightY => Some(Axis::RightY),
        Target::LeftTrigger => Some(Axis::LeftTrigger),
        Target::RightTrigger => Some(Axis::RightTrigger),
        Target::DpadX => Some(Axis::DpadX),
        Target::DpadY => Some(Axis::DpadY),
    }
}

/// Every gamepad axis and what it should be set to, for one stick position.
///
/// The four sides of the two axes are worked out separately, because they need not go to the same place: a
/// racing game steers with left and right and wants forward and back on the two triggers, which are axes of
/// their own and only ever move one way. Only one side of an axis can be pushed at a time, so whichever side
/// is being pushed is the one that sets the axis, and an axis nobody is pushing goes back to rest.
pub(crate) fn gamepad_axes(
    route: g13_config::stick::Route,
    here_x: f64,
    here_y: f64,
) -> Vec<(g13_device::joystick::Axis, f64)> {
    // each side: where it goes, whether it is being pushed, and what it carries
    let sides = [
        (route.right, here_x > 0.0, here_x),
        (route.left, here_x < 0.0, here_x),
        (route.down, here_y > 0.0, here_y),
        (route.up, here_y < 0.0, here_y),
    ];
    let mut wanted = Vec::new();
    for axis in g13_device::joystick::Axis::ALL {
        let mut value = 0.0;
        for (target, pushed, carried) in sides {
            if !pushed || axis_for_target(target) != Some(axis) {
                continue;
            }
            // A trigger has no middle: it is either being pulled or it is not. A stick takes the position
            // itself, sign and all, or the two sides routed to one axis would fight.
            value = match axis {
                g13_device::joystick::Axis::LeftTrigger
                | g13_device::joystick::Axis::RightTrigger => carried.abs(),
                _ => carried,
            };
        }
        wanted.push((axis, value));
    }
    wanted
}

/// The event node for a uinput device, found from where the kernel put it.
///
/// Not by name: two programs can each have a device called "g13 pointer", so matching on the name finds
/// whichever was created first - which is how a check of one device ends up reading another.
pub(crate) fn event_node_for(syspath: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(syspath).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("event") {
            return Some(format!("/dev/input/{name}"));
        }
    }
    None
}

/// A button on one of the devices the worker owns.
///
/// The devices belong to the worker's thread - one owner each - so the pad loop does not press them directly.
/// It hands the press over instead, and the worker applies it on its next tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceButton {
    /// A button on the virtual pointer.
    Mouse(g13_device::mouse::Button),
    /// A button on the virtual gamepad.
    Gamepad(g13_device::joystick::Button),
}

/// Drives the pointer or the gamepad from the stick, on its own thread.
///
/// Both need a steady rate. The pad's loop wakes when the pad sends something or its read times out, which at
/// rest is a fifth of a second: a pointer driven from there steps visibly, and a gamepad read by a game every
/// frame would show a stick that only changes five times a second. So both are emitted here, from the newest
/// reading the loop has seen.
///
/// Only the device the chosen mode needs is created, and it is created when that mode is first selected rather
/// than at startup: a stray pointer on the desktop while somebody is using the stick as keys is litter, and a
/// gamepad nobody is using is an entry in every game's controller list for no reason.
///
/// The deadzone is applied and then the remainder is stretched back to the full range, so full deflection is
/// full deflection. Without that stretch the deadzone silently costs a fifth of the travel at the far end,
/// and a setting that says 600 pixels a second delivers 396.
pub struct StickWorker {
    /// The state the loop and the worker's thread share: neither can borrow the other's.
    shared: std::sync::Arc<std::sync::Mutex<StickState>>,
    /// Set on drop, so the worker's thread leaves its sleep and hands the devices back.
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The worker's thread, joined on drop so the devices are released before this returns.
    handle: Option<std::thread::JoinHandle<()>>,
}

/// Everything the worker's thread reads and the driver's side writes, behind one lock.
struct StickState {
    /// The routing, speed and deadzone in force, replaced whole when the window writes new ones.
    settings: g13_config::stick::Settings,
    /// The newest stick reading the loop has seen, or none while no mode is using the stick.
    latest: Option<(u8, u8)>,
    /// What is left over from the last tick, per axis, for the pointer's relative movement.
    carried_x: f64,
    /// The same leftover for the vertical axis, accumulated apart from the horizontal one.
    carried_y: f64,
    /// A device that could not be created, read once by the caller and then cleared.
    problem: Option<String>,
    /// Button presses and releases handed over by the pad loop, applied on the next tick.
    pending: Vec<(DeviceButton, bool)>,
    /// Set when a binding needs a device, so one is made even if no stick mode is using it.
    want_pointer: bool,
    /// The same for the gamepad: set when a binding needs one, whether or not the stick mode does.
    want_gamepad: bool,
    /// Where each device ended up, so a caller can find the one this made rather than one that merely has
    /// the same name.
    pointer_node: Option<String>,
    /// Where the gamepad landed: the node this worker made, not whichever device shares its name.
    gamepad_node: Option<String>,
}

impl StickWorker {
    /// Start driving the pointer or the gamepad from the stick, on a thread of its own.
    pub fn start(settings: g13_config::stick::Settings) -> Self {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(StickState {
            settings,
            latest: None,
            carried_x: 0.0,
            carried_y: 0.0,
            problem: None,
            pending: Vec::new(),
            want_pointer: false,
            want_gamepad: false,
            pointer_node: None,
            gamepad_node: None,
        }));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let theirs = std::sync::Arc::clone(&shared);
        let halt = std::sync::Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            use g13_config::stick::Mode;
            use std::sync::atomic::Ordering;
            let mut pointer: Option<VirtualMouse> = None;
            let mut gamepad: Option<VirtualStick> = None;
            // sixty ticks a second, which is what a game expects to read a stick at
            let tick = Duration::from_millis(16);
            while !halt.load(Ordering::Relaxed) {
                std::thread::sleep(tick);
                let Ok(mut state) = theirs.lock() else {
                    break;
                };
                let mode = state.settings.mode;

                // A device is made when a stick mode needs it, or when a binding needs it: a control bound to
                // a mouse button must work whether or not the stick is in mouse mode.
                let need_pointer = mode == Mode::Mouse || state.want_pointer;
                let need_gamepad = mode == Mode::Joystick || state.want_gamepad;
                if need_pointer && pointer.is_none() {
                    match VirtualMouse::new() {
                        Ok(mut made) => {
                            state.pointer_node = made.syspath().as_deref().and_then(event_node_for);
                            pointer = Some(made);
                        }
                        Err(error) => {
                            state.problem = Some(format!("no pointer could be created: {error}"))
                        }
                    }
                }
                if need_gamepad && gamepad.is_none() {
                    match VirtualStick::new() {
                        Ok(mut made) => {
                            state.gamepad_node = made.syspath().as_deref().and_then(event_node_for);
                            gamepad = Some(made);
                        }
                        Err(error) => {
                            state.problem = Some(format!("no gamepad could be created: {error}"))
                        }
                    }
                }

                // Where the stick is, as -1.0 to 1.0 either side of the centre, with the deadzone taken out
                // and the rest stretched back over the full range.
                let deadzone = state.settings.stick.deadzone;
                let usable = (1.0 - deadzone).max(0.01);
                let stay = |offset: f64| {
                    if offset.abs() < deadzone {
                        0.0
                    } else {
                        (offset.abs() - deadzone) / usable * offset.signum()
                    }
                };
                let (here_x, here_y) = match state.latest {
                    Some((x, y)) => (
                        stay(state.settings.stick.x.offset(x)),
                        stay(state.settings.stick.y.offset(y)),
                    ),
                    None => (0.0, 0.0),
                };

                let mut move_pointer = None;
                let mut set_gamepad = None;
                match mode {
                    Mode::Mouse => {
                        let per_tick = state.settings.speed / 60.0;
                        state.carried_x += here_x * per_tick;
                        state.carried_y += here_y * per_tick;
                        let dx = state.carried_x.trunc() as i32;
                        let dy = state.carried_y.trunc() as i32;
                        state.carried_x -= dx as f64;
                        state.carried_y -= dy as f64;
                        move_pointer = Some((dx, dy));
                    }
                    Mode::Joystick => {
                        set_gamepad = Some(gamepad_axes(state.settings.route, here_x, here_y));
                    }
                    _ => {
                        // Leaving either mode must put things back: a gamepad left at full deflection keeps a
                        // game turning with nothing touching the stick, and a trigger left open holds the
                        // throttle down, with no way to lift it.
                        set_gamepad = Some(
                            g13_device::joystick::Axis::ALL
                                .iter()
                                .map(|axis| (*axis, 0.0))
                                .collect(),
                        );
                    }
                }
                let handed: Vec<(DeviceButton, bool)> = std::mem::take(&mut state.pending);
                drop(state);

                for (button, pressed) in handed {
                    match button {
                        DeviceButton::Mouse(button) => {
                            if let Some(pointer) = pointer.as_mut() {
                                let _ = if pressed {
                                    pointer.press(button)
                                } else {
                                    pointer.release(button)
                                };
                            }
                        }
                        DeviceButton::Gamepad(button) => {
                            if let Some(gamepad) = gamepad.as_mut() {
                                let _ = if pressed {
                                    gamepad.press(button)
                                } else {
                                    gamepad.release(button)
                                };
                            }
                        }
                    }
                }

                if let (Some((dx, dy)), Some(pointer)) = (move_pointer, pointer.as_mut()) {
                    let _ = pointer.move_by(dx, dy);
                }
                if let (Some(wanted), Some(gamepad)) = (set_gamepad, gamepad.as_mut()) {
                    for (axis, value) in wanted {
                        let _ = gamepad.set_axis(axis, value);
                    }
                }
            }
            if let Some(pointer) = pointer.as_mut() {
                let _ = pointer.release_all();
            }
            if let Some(gamepad) = gamepad.as_mut() {
                let _ = gamepad.centre();
            }
        });
        Self {
            shared,
            stop,
            handle: Some(handle),
        }
    }

    /// Tell it where the stick is now. The loop calls this as readings arrive.
    pub fn report(&self, reading: (u8, u8)) {
        if let Ok(mut state) = self.shared.lock() {
            state.latest = Some(reading);
        }
    }

    /// Take changed settings, when the window writes them.
    pub fn adjust(&self, settings: g13_config::stick::Settings) {
        if let Ok(mut state) = self.shared.lock() {
            state.settings = settings;
            if !matches!(
                state.settings.mode,
                g13_config::stick::Mode::Mouse | g13_config::stick::Mode::Joystick
            ) {
                state.latest = None;
            }
        }
    }

    /// Press or release a button on one of the worker's devices.
    pub fn act(&self, button: DeviceButton, pressed: bool) {
        if let Ok(mut state) = self.shared.lock() {
            state.pending.push((button, pressed));
        }
    }

    /// Say that a binding needs a device, so one is made even when no stick mode is using it.
    pub fn set_needs(&self, pointer: bool, gamepad: bool) {
        if let Ok(mut state) = self.shared.lock() {
            state.want_pointer |= pointer;
            state.want_gamepad |= gamepad;
        }
    }

    /// Where this worker's devices are: the pointer and the gamepad, if they have been made.
    pub fn device_nodes(&self) -> (Option<String>, Option<String>) {
        match self.shared.lock() {
            Ok(state) => (state.pointer_node.clone(), state.gamepad_node.clone()),
            Err(_) => (None, None),
        }
    }

    /// A device that could not be made, taken so it is said once. `None` if there is nothing to say.
    pub fn problem(&self) -> Option<String> {
        self.shared
            .lock()
            .ok()
            .and_then(|mut state| state.problem.take())
    }
}

impl Drop for StickWorker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// What one report from the pad does to the stick: work out the sector, press and release whatever the two
/// sectors say, and answer what changed.
///
/// The driver calls this for every report it reads. It is a function rather than a block inside the driver's
/// loop so the same code can be driven without the pad: the part worth proving is which bindings go down, and
/// that should not be reachable only by taking hold of the stick.
///
/// A reading that does not change the sector does nothing at all.
pub fn apply_stick(
    reading: (u8, u8),
    stick: &mut g13_config::stick::Stick,
    settings: &g13_config::stick::Settings,
    bindings: &Bindings,
    keyboard: &std::sync::Arc<Mutex<keyboard::VirtualKeyboard>>,
    devices: &StickWorker,
) -> Option<(Option<u32>, Option<u32>)> {
    let (released, pressed) = stick.update(reading, &settings.sectors)?;
    if settings.mode == g13_config::stick::Mode::Keyboard {
        // what the old sector was holding and what the new one holds, so the shared part of two sectors is
        // neither released nor pressed again
        let gone = released
            .map(|index| bindings.bounds_for(&settings.sectors, index))
            .unwrap_or_default();
        let here = pressed
            .map(|index| bindings.bounds_for(&settings.sectors, index))
            .unwrap_or_default();
        for bound in gone.iter().filter(|bound| !here.contains(bound)) {
            set_bound(bound, false, keyboard, devices);
        }
        for bound in here.iter().filter(|bound| !gone.contains(bound)) {
            set_bound(bound, true, keyboard, devices);
        }
    }
    Some((released, pressed))
}

/// Press or release whatever one of the stick's sectors drives.
///
/// A key goes to the keyboard, a mouse button or a gamepad button to the worker that owns those devices. The
/// three are the same thing here - something held while the stick is held - which is why a sector can be bound
/// to any of them.
pub(crate) fn set_bound(
    bound: &Bound,
    pressed: bool,
    keyboard: &std::sync::Arc<Mutex<keyboard::VirtualKeyboard>>,
    devices: &StickWorker,
) {
    match bound {
        Bound::Key(key) => {
            if let Ok(mut keyboard) = keyboard.lock() {
                let _ = if pressed {
                    keyboard.press(*key)
                } else {
                    keyboard.release(*key)
                };
            }
        }
        Bound::Mouse(button) => devices.act(DeviceButton::Mouse(*button), pressed),
        Bound::Gamepad(button) => devices.act(DeviceButton::Gamepad(*button), pressed),
        Bound::Nothing => {}
    }
}

/// What one sector of the stick does while the stick is held in it.
///
/// A key, a mouse button or a gamepad button, because a sector is just as good a place to put a controller's
/// d-pad as a keyboard's arrow keys - which is what makes the radial usable with the gamepad as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// A key on the virtual keyboard.
    Key(KeyCode),
    /// A button on the virtual pointer.
    Mouse(g13_device::mouse::Button),
    /// A button on the virtual gamepad.
    Gamepad(g13_device::joystick::Button),
    /// Bound to nothing on purpose.
    Nothing,
}

impl Bound {
    /// What this binding is, in words, for showing.
    pub fn name(&self) -> String {
        match self {
            // the same words the CLI and the window use: one place says what an action is, so the row and the
            // effect beside it cannot read as two different things
            Bound::Key(key) => describe_action(&format!("p,k.{}", key.code())),
            Bound::Mouse(button) => format!("mouse {}", button.name()),
            Bound::Gamepad(button) => format!("gamepad {}", button.name()),
            Bound::Nothing => "nothing".to_string(),
        }
    }
}
