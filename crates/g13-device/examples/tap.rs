//! Throwaway helper: create a keyboard named `g13-tap-test`, wait, tap F24, exit.
//!
//! F24 because no application reacts to it, so an end-to-end test cannot disturb anything. Used to prove
//! that observing another device works, without needing a real keypress.
//!
//!     cargo run -p g13-device --example tap

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use evdev::{EventType, InputEvent, KeyCode, uinput::VirtualDevice};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut keys = evdev::AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::new(194)); // KEY_F24

    let mut keyboard = VirtualDevice::builder()?
        .name("g13-tap-test")
        .with_keys(&keys)?
        .build()?;
    println!("g13-tap-test created; it will tap every 4s for 40s");

    for round in 1..=10 {
        std::thread::sleep(Duration::from_secs(4));
        keyboard.emit(&[InputEvent::new(EventType::KEY.0, 194, 1)])?;
        keyboard.emit(&[InputEvent::new(EventType::KEY.0, 194, 0)])?;
        println!("tap {round}: keycode 194 (F24)");
    }
    Ok(())
}
