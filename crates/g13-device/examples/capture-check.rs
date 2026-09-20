//! Prove the capture reads a real press and works out what it was - for a key and for a mouse button.
//!
//! The listener deliberately ignores devices named `g13 ...`, because those are the ones this program creates
//! and hearing them would mean binding a control to itself. So this makes a mouse and a keyboard with other
//! names, waits for them to appear, starts a watch, and presses one thing.
//!
//! The mouse press is the one that matters: `mb,left` in a config file is a button on a pointer, and the only
//! way to know the whole path works is to press one and see what comes out. A synthetic mouse can be pressed
//! without clicking anywhere on the desktop.
//!
//! Run without privileges:
//!
//!   cargo run -p g13-device --example capture-check --release

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode};
use g13_device::capture::{Captured, action_or_reason, watch};
use std::time::Duration;

/// Start a watch, wait for it to open its devices, press the thing, and report what came back.
fn pressed(name: &str, mut press: impl FnMut()) {
    println!("\n=== {name} ===");
    let receiver = watch();
    // let the watch open the devices before anything is sent, or the press happens before it is listening
    std::thread::sleep(Duration::from_millis(700));
    press();
    loop {
        match receiver.recv_timeout(Duration::from_secs(3)) {
            Ok(Captured::Watching { devices, refused }) => {
                println!("watching {devices} device(s), {} unreadable", refused.len());
            }
            Ok(Captured::Input { device, press }) => {
                println!("caught {press:?} from \"{device}\"");
                match action_or_reason(press) {
                    Ok(action) => println!("  which is the binding action {action}"),
                    Err(reason) => println!("  which cannot be bound: {reason}"),
                }
                return;
            }
            Ok(Captured::Nothing(reason)) => {
                println!("nothing could be watched: {reason}");
                return;
            }
            Err(error) => {
                println!("nothing was captured: {error}");
                return;
            }
        }
    }
}

fn main() {
    // --- a mouse, whose buttons are what a binding written `mb,...` sends ---
    let mut buttons = AttributeSet::<KeyCode>::new();
    for button in g13_device::mouse::Button::ALL {
        buttons.insert(button.code());
    }
    let mut axes = AttributeSet::<RelativeAxisCode>::new();
    axes.insert(RelativeAxisCode::REL_X);
    axes.insert(RelativeAxisCode::REL_Y);
    let mut mouse = VirtualDevice::builder()
        .expect("a builder")
        .name("capture check mouse")
        .with_keys(&buttons)
        .expect("buttons")
        .with_relative_axes(&axes)
        .expect("axes")
        .build()
        .expect("a mouse");
    println!("made a mouse called \"capture check mouse\"");
    std::thread::sleep(Duration::from_millis(700));
    pressed(
        "a mouse button the desktop does nothing with (BTN_SIDE)",
        || {
            // BTN_SIDE, not BTN_LEFT: a left click lands wherever the cursor happens to be
            mouse
                .emit(&[InputEvent::new(
                    EventType::KEY.0,
                    g13_device::mouse::Button::Side.code().code(),
                    1,
                )])
                .expect("a button press");
        },
    );

    // --- and a keyboard, for a key ---
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in 0..=255u16 {
        keys.insert(KeyCode::new(code));
    }
    let mut keyboard = VirtualDevice::builder()
        .expect("a builder")
        .name("capture check keyboard")
        .with_keys(&keys)
        .expect("keys")
        .build()
        .expect("a keyboard");
    println!("\nmade a keyboard called \"capture check keyboard\"");
    std::thread::sleep(Duration::from_millis(700));
    pressed("a key nothing reacts to (F24)", || {
        keyboard
            .emit(&[InputEvent::new(
                EventType::KEY.0,
                KeyCode::KEY_F24.code(),
                1,
            )])
            .expect("a key press");
    });
}
