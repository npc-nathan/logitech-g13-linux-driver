//! Play a macro from the configuration and show exactly what reaches the system.
//!
//! Prints the parsed steps, then plays the macro while reading its own virtual keyboard back, so the events
//! that leave this program are visible rather than assumed. Reading a device needs privileges; without them
//! the playback still happens and only the readback is missing.
//!
//!     sudo -E cargo run -p g13-agent --example play-macro -- 1

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_agent::play_macro_reporting;
use g13_device::keyboard::{self, VirtualKeyboard};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let id: u32 = std::env::args()
        .skip(1)
        .find_map(|argument| argument.parse().ok())
        .unwrap_or(1);

    let path = g13_config::macro_path(id);
    let text = std::fs::read_to_string(&path)?;
    let macro_file = g13_config::parse_macro(&text);
    println!("{}", path.display());
    println!("  name: {}", macro_file.name);
    println!("  id:   {}", macro_file.id);
    println!("  steps: {:?}", macro_file.steps);
    if macro_file.steps.is_empty() {
        println!("  nothing to play: the sequence is empty or not in a form this reads");
    }

    let keyboard = Arc::new(Mutex::new(VirtualKeyboard::new()?));
    println!("created {}", keyboard::DEVICE_NAME);

    // read our own device back, if we may
    let reader = std::thread::spawn(|| {
        let devices = g13_device::input_devices();
        let Some((_, node)) = devices
            .iter()
            .find(|(name, _)| name == keyboard::DEVICE_NAME)
        else {
            println!("readback: our keyboard is not in the device list");
            return;
        };
        println!("readback: watching {}", node.display());
        match g13_device::open_input(node) {
            Ok(mut device) => {
                let deadline = std::time::Instant::now() + Duration::from_secs(15);
                while std::time::Instant::now() < deadline {
                    match g13_device::next_key(&mut device) {
                        Ok((code, pressed)) => {
                            println!(
                                "  readback: keycode {code} {}",
                                if pressed { "down" } else { "up" }
                            )
                        }
                        Err(error) => {
                            println!("  readback: {error}");
                            return;
                        }
                    }
                }
            }
            Err(error) => {
                println!("readback: cannot read it ({error}). Run with sudo to see the events.")
            }
        }
    });

    std::thread::sleep(Duration::from_millis(300));
    println!("playing...");
    play_macro_reporting(&keyboard, &macro_file, 1, |step| match step {
        g13_config::MacroStep::KeyDown(code) => println!("  we sent: keycode {code} down"),
        g13_config::MacroStep::KeyUp(code) => println!("  we sent: keycode {code} up"),
        g13_config::MacroStep::Delay(ms) => println!("  we sent: wait {ms}ms"),
    })
    .map_err(|e| e.to_string())?;
    println!("played (our side is the list above)");
    let _ = reader.join();
    Ok(())
}
