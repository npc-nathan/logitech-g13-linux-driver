//! Prove a control bound to a mouse or gamepad button actually presses it.
//!
//! The devices belong to the worker, so a press reaches them through the worker's queue: this hands over a
//! press and a release for each device exactly as the pad loop does, and the event nodes are read from
//! outside while it runs.
//!
//! `mb,task` and `gb,south` are used rather than `mb,left`: this really does press a button on the pointer,
//! and a left click lands wherever the cursor happens to be. A task button is one almost nothing listens for,
//! and a gamepad button does nothing when no game is running.
//!
//! Run with the config directory holding a `stick.json`:
//!
//!   G13_CONFIG_DIR=/tmp/g13-stick-test cargo run -p g13-agent --example stick-buttons --release

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_agent::DeviceButton;

fn main() {
    let dir = g13_config::config_dir();
    let settings = g13_config::stick::Settings::load(&dir);
    let worker = g13_agent::StickWorker::start(settings);
    // both devices wanted, as they would be if a binding used them
    worker.set_needs(true, true);
    println!("asked for both devices");
    std::thread::sleep(std::time::Duration::from_millis(400));
    if let Some(problem) = worker.problem() {
        println!("PROBLEM: {problem}");
    }

    // Where this worker's devices are, so whatever is watching reads these and not a device of the same name
    // belonging to something else. Printed before the presses, with a pause, so a watcher can start first.
    let (pointer_node, gamepad_node) = worker.device_nodes();
    println!(
        "pointer node: {}",
        pointer_node.as_deref().unwrap_or("NONE")
    );
    println!(
        "gamepad node: {}",
        gamepad_node.as_deref().unwrap_or("NONE")
    );
    println!("READY");
    std::thread::sleep(std::time::Duration::from_millis(1500));

    let step = std::time::Duration::from_millis(500);
    for (name, button) in [
        (
            "mouse task",
            DeviceButton::Mouse(g13_device::mouse::Button::Task),
        ),
        (
            "mouse extra",
            DeviceButton::Mouse(g13_device::mouse::Button::Extra),
        ),
        (
            "gamepad south",
            DeviceButton::Gamepad(g13_device::joystick::Button::South),
        ),
        (
            "gamepad dpad-up",
            DeviceButton::Gamepad(g13_device::joystick::Button::DpadUp),
        ),
    ] {
        println!("pressing {name}");
        worker.act(button, true);
        std::thread::sleep(step);
        worker.act(button, false);
        std::thread::sleep(step);
    }

    println!("done");
    std::thread::sleep(std::time::Duration::from_millis(200));
}
