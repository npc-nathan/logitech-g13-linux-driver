//! Watch for the next press and say what binding it would set. Nothing else.
//!
//!   cargo run -p g13-device --example watch-once --release
//!
//! `capture-check` presses its own synthetic devices first so it can test itself; this does not, so it can be
//! pointed at a pretend controller, or a real one, and answer the one question: what did that press become?

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_device::capture::{Captured, action_or_reason, watch};
use std::time::Duration;

fn main() {
    let receiver = watch();
    loop {
        match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(Captured::Watching { devices, refused }) => {
                println!("watching {devices} device(s), {} unreadable", refused.len());
                for reason in refused {
                    println!("  unreadable: {reason}");
                }
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
                std::process::exit(1);
            }
            Err(error) => {
                println!("nothing was captured: {error}");
                std::process::exit(1);
            }
        }
    }
}
