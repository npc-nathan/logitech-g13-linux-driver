//! `g13 gui`  -  open the configuration window.
//!
//! The window needs a display, so the failure to open one is reported rather than swallowed: the same
//! configuration is editable from the command line, and the message says so.

/// Opens the configuration window, and names the command-line equivalents when there is no display.
pub fn run() -> i32 {
    let config = g13_config::config_dir();
    let window = g13_gui::state::Window::load(&config);
    println!(
        "opening the g13 configuration window ({})",
        config.display()
    );
    match g13_gui::App::open(window) {
        Ok(()) => 0,
        Err(error) => {
            println!("could not open a window: {error}");
            println!();
            println!(
                "this needs a display. Everything in it is also available from the command line:"
            );
            println!("    g13 bindings        which control sends what");
            println!("    g13 bind <c> <a>    change one");
            println!("    g13 lcd --visual    see the screen");
            1
        }
    }
}
