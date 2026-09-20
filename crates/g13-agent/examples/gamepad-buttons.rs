//! Prove which controller buttons the driver's gamepad actually emits, without the pad.
//!
//! Presses each button through the same `StickWorker` the driver uses, so what reaches the kernel here is what
//! reaches it when a G key bound to `gb,<button>` is held. Run it with something privileged reading the node
//! it prints:
//!
//!   G13_CONFIG_DIR=/tmp/g13-buttons \
//!     cargo run -p g13-agent --example gamepad-buttons --release
//!
//! The question it answers is the one a report of "the d-pad and the triggers do nothing" leaves open: are the
//! events dropped here, or do they arrive and something downstream ignores them.

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
fn main() {
    let dir = g13_config::config_dir();
    let settings = g13_config::stick::Settings::load(&dir);
    println!("config: {}", dir.display());
    println!("mode:   {}", settings.mode.name());

    let worker = g13_agent::StickWorker::start(settings);
    // as though a bindings file had a `gb,` line in it
    worker.set_needs(false, true);
    std::thread::sleep(std::time::Duration::from_millis(300));
    if let Some(problem) = worker.problem() {
        println!("PROBLEM: {problem}");
    }
    let (pointer, gamepad) = worker.device_nodes();
    println!("pointer node: {}", pointer.as_deref().unwrap_or("NONE"));
    println!("gamepad node: {}", gamepad.as_deref().unwrap_or("NONE"));
    println!("READY");
    std::thread::sleep(std::time::Duration::from_millis(1200));

    use g13_device::joystick::Button;
    for button in Button::ALL {
        println!("  {}  code {}", button.name(), button.code().code());
        worker.act(g13_agent::DeviceButton::Gamepad(button), true);
        std::thread::sleep(std::time::Duration::from_millis(250));
        worker.act(g13_agent::DeviceButton::Gamepad(button), false);
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    drop(worker);
    println!("stopped the worker");
}
