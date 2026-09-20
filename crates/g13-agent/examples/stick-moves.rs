//! Prove the stick worker drives its device, without the pad.
//!
//! `StickWorker` is what the driver uses, so this exercises the same code. Which device it drives comes
//! from the mode in the config: `mouse` makes a pointer, `joystick` makes a gamepad. It reports a stick reading
//! to the worker as though the pad had reported one, so the timings line up with whatever is watching the device.
//!
//! Run it with the event node being read by something privileged at the same time - the node for a uinput
//! pointer is root-owned and this cannot open it:
//!
//!   G13_CONFIG_DIR=/tmp/g13-stick-test cargo run -p g13-agent --example stick-moves --release
//!
//! The config directory must hold a `stick.json` with the mode to test and the calibration points, or the
//! worker will read a mode of off and do nothing, which is what it is supposed to do in that case.

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
    println!("speed:  {:.0} pixels a second", settings.speed);
    println!(
        "x axis: low {} centre {} high {}",
        settings.stick.x.low, settings.stick.x.centre, settings.stick.x.high
    );
    if settings.mode == g13_config::stick::Mode::Off {
        println!("off, so the worker will correctly do nothing");
    }

    let worker = g13_agent::StickWorker::start(settings.clone());

    // The bindings decide what the stick's sectors do, and whether a pointer or a gamepad is needed at all - the
    // same two calls the driver makes before it starts reading. Without them a sector bound to a controller
    // button would have no controller to press it on, and this would quietly prove nothing.
    let bindings_text =
        std::fs::read_to_string(g13_config::bindings_path(g13_config::read_active_profile()))
            .unwrap_or_default();
    let bindings = g13_agent::Bindings::from_text(&bindings_text);
    println!("bindings: {} usable", bindings.len());
    for problem in bindings.problems() {
        println!("  not applied: {problem}");
    }
    worker.set_needs(bindings.needs_mouse(), bindings.needs_gamepad());
    println!(
        "needs:  mouse {} gamepad {}",
        bindings.needs_mouse(),
        bindings.needs_gamepad()
    );
    println!("started the worker");
    // A device that could not be created is the one thing this can fail at, and a worker that cannot make one
    // looks exactly like a worker that made one and has nothing to do.
    std::thread::sleep(std::time::Duration::from_millis(200));
    match worker.problem() {
        Some(problem) => println!("PROBLEM: {problem}"),
        None => println!("no problem reported (it is still starting, so this may be early)"),
    }

    // Where this worker's devices are, so whatever is watching reads these and not a device of the same name
    // belonging to something else. Printed before the readings, with a pause, so a watcher can start first.
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
    std::thread::sleep(std::time::Duration::from_millis(1200));

    // Alternating on purpose: an absolute axis only reports when it changes, so a position held still is one
    // event at the start and one at the end - and a check that starts watching late would see neither. Moving
    // it back and forth means any window catches several transitions whichever way round the timings fall.
    //
    // Both axes, and each in turn, so a split route can be seen: the two ends of one axis going to two
    // different places would look like nothing happening at all if only one end were ever pushed.
    let middle = (settings.stick.x.centre, settings.stick.y.centre);
    let readings = [
        (
            (settings.stick.x.high, settings.stick.y.centre),
            "fully right",
        ),
        (middle, "centred"),
        ((settings.stick.x.centre, settings.stick.y.low), "fully up"),
        (middle, "centred"),
        (
            (settings.stick.x.centre, settings.stick.y.high),
            "fully down",
        ),
        (middle, "centred"),
        // and left, so all four sides of both axes are exercised: a route that sends only one side of an axis
        // somewhere would look like nothing happening if that side were never pushed
        (
            (settings.stick.x.low, settings.stick.y.centre),
            "fully left",
        ),
        (middle, "centred"),
    ];
    // The stick itself, driven the way the driver drives it: the same function, so what happens here is what
    // happens when the pad reports the same bytes.
    let mut stick = settings.stick.clone();
    let keyboard = std::sync::Arc::new(std::sync::Mutex::new(
        g13_device::keyboard::VirtualKeyboard::new().expect("a keyboard"),
    ));
    for (reading, said) in readings {
        println!("  {said}  {reading:?}");
        for _ in 0..8 {
            if let Some((released, pressed)) = g13_agent::apply_stick(
                reading, &mut stick, &settings, &bindings, &keyboard, &worker,
            ) {
                println!(
                    "      sector {} -> {}",
                    released.map_or("-".to_string(), |index| settings.sectors.name(index)),
                    pressed.map_or("-".to_string(), |index| settings.sectors.name(index))
                );
            }
            worker.report(reading);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    drop(worker);
    println!("stopped the worker");
}
