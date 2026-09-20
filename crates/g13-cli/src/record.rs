//! `g13 record <control>`  -  set a binding by pressing the thing it should send.
//!
//! Reads the physical keyboards, pointers and gamepads (never our own virtual devices) and writes the first
//! press it sees into the active profile's bindings. A key, a mouse button and a gamepad button are all just
//! codes, and a gamepad's d-pad and triggers arrive as absolute axes rather than codes - both are understood,
//! so all of them can be recorded here; a combination is a macro, not a binding, and `g13 bind` takes
//! the rest.

use g13_device::capture::{Captured, action_or_reason, watch};

/// Records the next press the physical devices see into the named control's binding.
pub fn run(control: &str) -> i32 {
    if g13_proto::control_from_name(control).is_none() {
        println!("there is no control called {control} on this pad.");
        println!("\nthe controls this build knows, as measured from the pad:");
        for chunk in g13_device::CONTROLS.chunks(8) {
            println!("    {}", chunk.join(" "));
        }
        return 1;
    }

    println!("Press the key or button that {control} should send.");
    println!(
        "(One press. For a combination like ALT-TAB, use a macro: g13 bind {control} m,<id>,1)"
    );

    let receiver = watch();
    let (device, press) = loop {
        match receiver.recv() {
            Ok(Captured::Watching { devices, refused }) => {
                println!("\nreading from {devices} device(s).");
                for problem in refused {
                    println!("  cannot read {problem}");
                }
            }
            Ok(Captured::Nothing(reason)) => {
                println!("\n{reason}");
                return 1;
            }
            Ok(Captured::Input { device, press }) => break (device, press),
            Err(_) => {
                println!("nothing was pressed.");
                return 1;
            }
        }
    };

    let action = match action_or_reason(press) {
        Ok(action) => action,
        Err(reason) => {
            println!("\n  {press:?}, from \"{device}\"");
            println!("  {reason}");
            return 1;
        }
    };
    println!("\n  {press:?}, from \"{device}\"  ->  {action}");

    let path = g13_agent::active_bindings_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = g13_config::set_binding(&text, control, &action);
    if let Err(error) = g13_files::write(&path, &updated) {
        println!("could not write {}: {error}", path.display());
        return 1;
    }
    println!("{control} now sends {action}  (in {})", path.display());
    println!("a running driver picks it up within a second.");
    0
}
