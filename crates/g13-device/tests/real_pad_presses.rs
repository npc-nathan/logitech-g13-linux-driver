//! What the recorded controller sent, replayed through the capture's own decisions.
//!
//! Not a simulation and not a pretend pad: every line of the fixture is an event the **Microsoft X-Box 360 pad
//! on this machine** emitted while its d-pad and its two triggers were pressed by hand. If the capture's idea of
//! what a d-pad press is ever drifts from what that controller sends, this test stops passing.

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_device::Press;
use g13_device::axis_means_a_press;
use g13_device::capture::action_for;
use std::collections::BTreeSet;

/// `type code value`, one event per line.
const FIXTURE: &str = include_str!("fixtures/real-pad-presses.txt");

fn events() -> Vec<(u16, u16, i32)> {
    FIXTURE
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut parts = line.split_whitespace();
            let kind: u16 = parts.next().expect("a type").parse().expect("a number");
            let code: u16 = parts.next().expect("a code").parse().expect("a number");
            let value: i32 = parts.next().expect("a value").parse().expect("a number");
            (kind, code, value)
        })
        .collect()
}

#[test]
fn the_recorded_presses_read_as_the_controller_buttons_they_are() {
    // LT, RT, then the four d-pad directions, in the order they were pressed.
    let mut actions: Vec<String> = Vec::new();
    // an axis already away from rest is being held: the values it passes on the way back are the end of one
    // press rather than a run of new ones
    let mut held: BTreeSet<u16> = BTreeSet::new();
    for (kind, code, value) in events() {
        match kind {
            1 if value == 1 => actions
                .push(action_for(Press::Key(code)).expect("a key press should always be bindable")),
            3 => {
                if axis_means_a_press(code, value) && !held.contains(&code) {
                    actions
                        .push(action_for(Press::Axis { code, value }).unwrap_or_else(|| {
                            panic!("axis {code} at {value} should be bindable")
                        }));
                    held.insert(code);
                }
                if value == 0 {
                    held.remove(&code);
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        actions,
        vec![
            "gb,lt",
            "gb,rt",
            "gb,dpad-left",
            "gb,dpad-up",
            "gb,dpad-right",
            "gb,dpad-down"
        ],
        "the recorded presses should read as exactly these six controller buttons"
    );
}

#[test]
fn the_recorded_presses_produced_no_key_event_at_all() {
    // This is the whole reason a d-pad or a trigger could not be set by pressing one: the reader took
    // `EventType::KEY` and nothing else, and the controller reports every one of these six presses as an
    // absolute axis. Not one key event exists in the capture to have been caught.
    let key_presses = events()
        .iter()
        .filter(|(kind, _, value)| *kind == 1 && *value == 1)
        .count();
    assert_eq!(
        key_presses, 0,
        "the d-pad and the triggers sent no key event, so a key-only reader saw nothing"
    );
}

#[test]
fn the_axes_the_recorded_presses_arrived_on_are_the_d_pad_and_the_triggers() {
    // named explicitly, so a change to what is read is caught against the real controller rather than against
    // our own assumptions
    let axes: BTreeSet<u16> = events()
        .iter()
        .filter(|(kind, code, value)| *kind == 3 && axis_means_a_press(*code, *value))
        .map(|(_, code, _)| *code)
        .collect();
    assert_eq!(
        axes,
        BTreeSet::from([0x02, 0x05, 0x10, 0x11]),
        "LT and RT are ABS_Z and ABS_RZ, and the d-pad is the hat, measured from the pad"
    );
}
