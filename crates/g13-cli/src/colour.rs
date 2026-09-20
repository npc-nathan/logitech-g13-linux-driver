//! `g13 colour` - the screen's backlight, which lives in the active profile's own file.
//!
//! The colour is a line in `bindings-<n>.properties` (`color=R,G,B`), which is where the previous stack kept
//! it, so a profile carries its own light and switching profile switches it. Written to the file rather than
//! sent straight to the device: the driver watches that file, so a running one picks it up within a second, and
//! a change made with the driver stopped still applies at the next start.

use g13_agent::active_bindings_path;
use g13_config::Colour;

/// The backlight in the active profile: printed when no value is named, written into the file when one is.
pub fn run(args: &[String]) -> i32 {
    let path = active_bindings_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    let Some(value) = args.first() else {
        match g13_config::colour_in(&existing) {
            Some(colour) => {
                println!("the screen is {}  (from {})", colour.text(), path.display());
                println!("  set another: g13 colour 0,153,255");
            }
            None => {
                println!(
                    "{} names no colour, so the screen keeps whatever it was last told",
                    path.display()
                );
                println!("  set one: g13 colour 0,153,255");
            }
        }
        return 0;
    };

    let Some(colour) = Colour::parse(value) else {
        eprintln!(
            "g13 colour: {value} is not a colour. Give it as R,G,B with each part from 0 to 255, e.g. 0,153,255"
        );
        return 2;
    };

    let updated = g13_config::set_colour(&existing, colour);
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("g13 colour: could not create {}: {error}", parent.display());
        return 1;
    }
    match g13_files::write(&path, &updated) {
        Ok(()) => {
            println!(
                "the screen is now {}  (in {})",
                colour.text(),
                path.display()
            );
            println!("  a running driver picks it up within a second");
            0
        }
        Err(error) => {
            eprintln!("g13 colour: could not write {}: {error}", path.display());
            1
        }
    }
}
