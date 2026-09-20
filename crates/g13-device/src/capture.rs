//! Watching the real input devices, so a binding can be set by pressing the thing.
//!
//! `g13 record` and the window both need this, and both need it to mean the same thing: press a key, a mouse
//! button or a gamepad button, and the code that arrives says which it was. So it is one place, and the
//! meaning of a code is a function of the code itself rather than of which device it came from - the kernel
//! gives keys, mouse buttons and gamepad buttons disjoint ranges, and that is enough to tell them apart.
//!
//! Our own devices are excluded by name. Without that the driver would hear what it just sent and bind a
//! control to itself.

use crate::Press;
use std::sync::mpsc;

/// What was pressed, or why nothing could be watched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// What is being read, sent as soon as the watch starts.
    ///
    /// First, so a caller can say "reading four devices" rather than leaving the user to wonder whether
    /// anything is listening at all.
    Watching {
        /// How many could be opened, so a caller can report the number it is actually reading.
        devices: usize,
        /// The ones that could not be opened, each as `name: reason`, so a device missing from the count can be
        /// named rather than guessed at.
        refused: Vec<String>,
    },
    /// A press, with the device it came from and what the kernel reported.
    ///
    /// The press rather than a bare code, because **a gamepad's d-pad and triggers are not keys**: they arrive
    /// as absolute axes, and a reader that only understood key codes could never see them. Pressing the d-pad
    /// to set a binding did nothing at all, while the face buttons worked - which was the report.
    Input {
        /// The device it came from, by the name the kernel gives it.
        device: String,
        /// What the kernel reported - a key going down, or an absolute axis arriving at a press.
        press: Press,
    },
    /// Nothing can be captured, and this is the reason rather than a silence.
    Nothing(String),
}

/// Watch the real input devices for the next press.
///
/// Returns immediately with a channel. The answer arrives when something is pressed, or at once with a reason
/// if no device can be read - which is a permission on `/dev/input`, not a missing press, and the two look
/// identical from outside unless one of them is said out loud.
pub fn watch() -> mpsc::Receiver<Captured> {
    let (sender, receiver) = mpsc::channel();

    // our own virtual devices would hear what we send
    let candidates: Vec<(String, std::path::PathBuf)> = crate::input_devices()
        .into_iter()
        .filter(|(name, _)| !name.starts_with("g13 "))
        .collect();

    // open them here first, so "nothing was captured" can never be a silent mystery
    let mut openable = Vec::new();
    let mut refused = Vec::new();
    for (name, path) in candidates {
        match crate::open_input(&path) {
            Ok(_) => openable.push((name, path)),
            Err(error) => refused.push(format!("{name}: {error}")),
        }
    }
    if openable.is_empty() {
        let reason = if refused.is_empty() {
            "no input devices were found to listen to".to_string()
        } else {
            format!(
                "none of the {} device(s) could be read, so no press can reach this. \
                 This is a permission on /dev/input rather than a missing press. The fix, once:\\n{}",
                refused.len(),
                refused.join("\\n")
            )
        };
        let _ = sender.send(Captured::Nothing(reason));
        return receiver;
    }
    let count = openable.len();
    let _ = sender.send(Captured::Watching {
        devices: count,
        refused,
    });

    for (name, path) in openable {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let Ok(mut device) = crate::open_input(&path) else {
                return;
            };
            // One press, and this thread is done: a release is the other half of a press that has already
            // been reported and a stick moving is not a button at all, and `next_press` skips both itself.
            if let Ok(press) = crate::next_press(&mut device) {
                let _ = sender.send(Captured::Input {
                    device: name,
                    press,
                });
            }
        });
    }
    drop(sender);
    receiver
}

/// One key as it went down or came up, and how long after the one before it.
///
/// The pause matters as much as the key: a macro is a sequence *and* its timing, which is what the `d.` steps
/// in a `macro-<id>.properties` file are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// The kernel's key code for the key that changed.
    pub code: u16,
    /// True when the key went down, false when it came back up.
    pub down: bool,
    /// Milliseconds since the previous event, and from the start of the recording for the first one.
    pub after_ms: u32,
}

/// Watch the real keyboards, reporting every key press and release with its timing, until the caller stops
/// listening.
///
/// `watch` answers one question - what was pressed - because a binding needs one thing. A macro needs the whole
/// sequence, both halves of every press and how long each was held, so this reports all of it. Only keyboard
/// keys: the macro format holds key codes, and a mouse button or a stick direction is not a key.
///
/// The caller stops it by dropping the receiver, which is what a recording ending looks like.
pub fn record_keys() -> mpsc::Receiver<KeyEvent> {
    let (sender, receiver) = mpsc::channel();

    let candidates: Vec<(String, std::path::PathBuf)> = crate::input_devices()
        .into_iter()
        .filter(|(name, _)| !name.starts_with("g13 "))
        .collect();

    for (_, path) in candidates {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let Ok(mut device) = crate::open_input(&path) else {
                return;
            };
            let mut last = std::time::Instant::now();
            loop {
                let Ok((code, down)) = crate::next_key(&mut device) else {
                    return;
                };
                let now = std::time::Instant::now();
                let after_ms = now.duration_since(last).as_millis().min(u32::MAX as u128) as u32;
                last = now;
                // a keyboard key is below the button range, and a button is not a key a macro can hold
                if code >= 0x100 {
                    continue;
                }
                if sender
                    .send(KeyEvent {
                        code,
                        down,
                        after_ms,
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
    }
    drop(sender);
    receiver
}

/// The binding action a key code means.
///
/// The kernel gives keyboard keys, mouse buttons and gamepad buttons disjoint ranges, so the code alone says
/// which kind of thing was pressed. `None` for a code that is none of them - a media key, say - rather than a
/// guess at what it might have been.
pub fn action_for_key(code: u16) -> Option<String> {
    let wanted = evdev::KeyCode::new(code);
    // a keyboard key is below the button range: the kernel puts KEY_* at 0..0x100 and BTN_* above it
    if code < 0x100 {
        return Some(format!("p,k.{code}"));
    }
    if let Some(button) = crate::mouse::Button::ALL
        .into_iter()
        .find(|button| button.code() == wanted)
    {
        return Some(format!("mb,{}", button.name()));
    }
    if let Some(button) = crate::joystick::Button::ALL
        .into_iter()
        .find(|button| button.code() == wanted)
    {
        return Some(format!("gb,{}", button.name()));
    }
    None
}

/// The binding action an absolute axis at this value means.
///
/// This is how a real pad reports its d-pad and its two triggers: a hat direction is `ABS_HAT0X`/`ABS_HAT0Y` at
/// -1 or 1, and a trigger is `ABS_Z`/`ABS_RZ` travelling up from nothing. They are the same buttons the
/// `gb,` names describe, so pressing one binds it exactly as pressing a face button does - and before this,
/// pressing a d-pad direction or a trigger to set a binding did nothing at all, because the reader was waiting
/// for a key event that a real pad never sends.
pub fn action_for_axis(code: u16, value: i32) -> Option<String> {
    use evdev::AbsoluteAxisCode as Abs;
    let action = match (Abs(code), value.signum()) {
        (Abs::ABS_HAT0X, -1) => "gb,dpad-left",
        (Abs::ABS_HAT0X, 1) => "gb,dpad-right",
        (Abs::ABS_HAT0Y, -1) => "gb,dpad-up",
        (Abs::ABS_HAT0Y, 1) => "gb,dpad-down",
        // the triggers travel one way, so any travel at all is the trigger being held
        (Abs::ABS_Z, 1) => "gb,lt",
        (Abs::ABS_RZ, 1) => "gb,rt",
        _ => return None,
    };
    Some(action.to_string())
}

/// The binding action a press means, from either kind of event.
pub fn action_for(press: Press) -> Option<String> {
    match press {
        Press::Key(code) => action_for_key(code),
        Press::Axis { code, value } => action_for_axis(code, value),
    }
}

/// The action a press means, or a sentence saying why it cannot be bound.
pub fn action_or_reason(press: Press) -> Result<String, String> {
    action_for(press).ok_or_else(|| match press {
        Press::Key(code) => format!(
            "keycode {code} is not a key, a mouse button or a gamepad button this build knows, so there is nothing to bind it to"
        ),
        Press::Axis { code, value } => format!(
            "axis {code} at {value} is not a d-pad direction or a trigger, so there is nothing to bind it to - move a stick and nothing should be bound"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keyboard_key_becomes_a_passthrough() {
        // 30 is KEY_A on every PC keyboard, 1 is Escape, 57 the space bar
        assert_eq!(action_for_key(30).as_deref(), Some("p,k.30"));
        assert_eq!(action_for_key(1).as_deref(), Some("p,k.1"));
        assert_eq!(action_for_key(57).as_deref(), Some("p,k.57"));
    }

    #[test]
    fn a_mouse_button_becomes_a_mouse_button() {
        // the kernel's own numbers for them, which is what a real mouse sends
        assert_eq!(action_for_key(0x110).as_deref(), Some("mb,left"));
        assert_eq!(action_for_key(0x111).as_deref(), Some("mb,right"));
        assert_eq!(action_for_key(0x112).as_deref(), Some("mb,middle"));
        assert_eq!(action_for_key(0x113).as_deref(), Some("mb,side"));
        assert_eq!(action_for_key(0x114).as_deref(), Some("mb,extra"));
        assert_eq!(action_for_key(0x117).as_deref(), Some("mb,task"));
    }

    #[test]
    fn a_gamepad_button_becomes_a_gamepad_button() {
        assert_eq!(action_for_key(0x130).as_deref(), Some("gb,south"));
        assert_eq!(action_for_key(0x131).as_deref(), Some("gb,east"));
        assert_eq!(action_for_key(0x133).as_deref(), Some("gb,north"));
        assert_eq!(action_for_key(0x134).as_deref(), Some("gb,west"));
        assert_eq!(action_for_key(0x136).as_deref(), Some("gb,lb"));
        assert_eq!(action_for_key(0x139).as_deref(), Some("gb,rt"));
        assert_eq!(action_for_key(0x220).as_deref(), Some("gb,dpad-up"));
        assert_eq!(action_for_key(0x223).as_deref(), Some("gb,dpad-right"));
    }

    #[test]
    fn a_code_that_is_none_of_them_says_so_rather_than_guessing() {
        // a media key: a real code, and not something this build can bind
        assert_eq!(action_for_key(0x163), None);
        // the reason names the code in the number a reader would see in the file
        match action_or_reason(Press::Key(0x163)) {
            Err(reason) => assert!(reason.contains("355"), "{reason}"),
            Ok(action) => panic!("expected a reason, got {action}"),
        }
    }

    #[test]
    fn pressing_a_d_pad_direction_or_a_trigger_becomes_a_controller_button() {
        // The report this exists for: everything on the controller could be set by pressing it except the
        // d-pad and the triggers. They arrive as absolute axes, not key events, so the reader never saw them.
        assert_eq!(action_for_axis(0x11, -1).as_deref(), Some("gb,dpad-up"));
        assert_eq!(action_for_axis(0x11, 1).as_deref(), Some("gb,dpad-down"));
        assert_eq!(action_for_axis(0x10, -1).as_deref(), Some("gb,dpad-left"));
        assert_eq!(action_for_axis(0x10, 1).as_deref(), Some("gb,dpad-right"));
        // a trigger travels up from nothing, so any travel is the trigger
        assert_eq!(action_for_axis(0x02, 1).as_deref(), Some("gb,lt"));
        assert_eq!(action_for_axis(0x02, 255).as_deref(), Some("gb,lt"));
        assert_eq!(action_for_axis(0x05, 200).as_deref(), Some("gb,rt"));
        // and the same through the one entry point both kinds of press go through
        assert_eq!(
            action_for(Press::Axis {
                code: 0x11,
                value: -1
            })
            .as_deref(),
            Some("gb,dpad-up")
        );
        assert_eq!(action_for(Press::Key(0x130)).as_deref(), Some("gb,south"));
    }

    #[test]
    fn a_stick_moving_and_a_hat_returning_to_the_middle_bind_nothing() {
        // a hat returning to the centre is the *end* of a press, and a stick is not a button at all: binding
        // either would put a binding on a control the user never pressed
        assert_eq!(action_for_axis(0x11, 0), None);
        assert_eq!(action_for_axis(0x10, 0), None);
        assert_eq!(action_for_axis(0x02, 0), None);
        assert_eq!(action_for_axis(0x00, -20000), None, "the left stick's x");
        assert_eq!(action_for_axis(0x03, 15000), None, "the right stick's x");
        // and the sentence says which axis it was, so an unexpected one is diagnosable
        match action_or_reason(Press::Axis {
            code: 0x00,
            value: -20000,
        }) {
            Err(reason) => assert!(reason.contains("not a d-pad direction"), "{reason}"),
            Ok(action) => panic!("expected a reason, got {action}"),
        }
    }

    #[test]
    fn the_ranges_do_not_overlap_so_a_code_means_one_thing() {
        // if a code were in two sets, which action it produced would depend on lookup order
        for code in 0x100..0x300u16 {
            let mouse = crate::mouse::Button::ALL
                .into_iter()
                .any(|button| button.code() == evdev::KeyCode::new(code));
            let gamepad = crate::joystick::Button::ALL
                .into_iter()
                .any(|button| button.code() == evdev::KeyCode::new(code));
            assert!(
                !(mouse && gamepad),
                "keycode {code} is both a mouse and a gamepad button"
            );
        }
    }
}
