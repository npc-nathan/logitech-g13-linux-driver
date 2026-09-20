//! Prove the virtual pointer can be created and that its axes reach the system.
//!
//! A virtual pointer is not something reading the code can confirm: uinput either sets it up or refuses, and
//! a relative axis that was never advertised silently reports nothing. So this creates one, moves it, moves
//! it back, and reports what the kernel made.
//!
//! The movement is net zero on purpose: the pointer really does move the cursor while this runs, so it puts
//! it back rather than leaving somebody's mouse somewhere else.
//!
//! Run with `cargo run -p g13-device --example pointer --release`.

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_device::mouse::{DEVICE_NAME, VirtualMouse};

fn main() {
    let mut pointer = match VirtualMouse::new() {
        Ok(pointer) => pointer,
        Err(error) => {
            println!("could not create the virtual pointer: {error}");
            std::process::exit(1);
        }
    };
    println!("created {DEVICE_NAME}");

    // What the kernel says it is. Read from /proc/bus/input/devices rather than by opening the event
    // nodes: opening them needs privileges this does not have, and a reader that skips what it cannot open
    // reports a device that exists as absent. That mistake has already been made once in this project.
    match std::fs::read_to_string("/proc/bus/input/devices") {
        Ok(listing) => {
            let mut named = false;
            for block in listing.split("\n\n") {
                if !block.contains(DEVICE_NAME) {
                    continue;
                }
                named = true;
                println!("the kernel lists it as:");
                for line in block.lines() {
                    let line = line.trim();
                    if line.starts_with("N:") || line.starts_with("H:") || line.starts_with("B:") {
                        // the handlers and the capability bits are what say whether it is a pointer
                        if line.starts_with("B: PROP") || line.len() > 200 {
                            continue;
                        }
                        println!("  {line}");
                    }
                }
            }
            if !named {
                println!("  it is not in /proc/bus/input/devices, so the kernel did not take it");
            }
        }
        Err(error) => println!("could not read the device list: {error}"),
    }

    // a real movement, there and back, so the axis is exercised rather than merely present
    match pointer.move_by(20, 0) {
        Ok(()) => println!("moved the cursor 20 right"),
        Err(error) => println!("could not move: {error}"),
    }
    std::thread::sleep(std::time::Duration::from_millis(120));
    match pointer.move_by(-20, 0) {
        Ok(()) => println!("moved it back, so the cursor is where it was"),
        Err(error) => println!("could not move it back: {error}"),
    }
    std::thread::sleep(std::time::Duration::from_millis(120));

    // and letting go of everything, which is what happens when the stick is turned off
    match pointer.release_all() {
        Ok(()) => println!("released everything it was holding"),
        Err(error) => println!("could not release: {error}"),
    }
}
