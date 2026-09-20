//! Round-trip every applet in a directory and report whether writing one back would change anything.
//!
//!   cargo run -p g13-applets --example applet-round-trip --release -- ~/.config/g13/applets
//!
//! The designer edits an applet's file and writes it back, so it has to be able to say what that would do
//! first. Applets are hand-written and hold `cmd:` strings with quotes, braces and awk inside them, and
//! the thing worth being sure of is that none of it comes back different. Read-only: nothing is written.

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
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: applet-round-trip <applets directory>");
        std::process::exit(2);
    });
    let dir = std::path::PathBuf::from(dir);
    let names = g13_applets::applet_names(&dir);
    if names.is_empty() {
        println!("no applets in {}", dir.display());
        std::process::exit(1);
    }

    let mut changed = Vec::new();
    for name in &names {
        let before = match g13_applets::load_applet_json(&dir, name) {
            Ok(value) => value,
            Err(problem) => {
                println!("  {name:<14} could not be read: {problem}");
                changed.push(name.clone());
                continue;
            }
        };
        let text = match g13_applets::applet_json_text(&before) {
            Ok(text) => text,
            Err(problem) => {
                println!("  {name:<14} could not be written: {problem}");
                changed.push(name.clone());
                continue;
            }
        };
        // what the designer would write, read back and compared as data
        let after: serde_json::Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                println!("  {name:<14} wrote something that is not JSON: {error}");
                changed.push(name.clone());
                continue;
            }
        };
        let sources = before
            .get("sources")
            .and_then(|s| s.as_object())
            .map(|o| o.len())
            .unwrap_or(0);
        let widgets = before
            .get("widgets")
            .and_then(|w| w.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if after == before {
            println!(
                "  {name:<14} {widgets} widget(s), {sources} source(s)   nothing would change"
            );
        } else {
            println!("  {name:<14} would change");
            changed.push(name.clone());
        }
    }

    println!();
    if changed.is_empty() {
        println!(
            "writing any of these {} applets back would change nothing: every key, every source string and every number would come back the same.",
            names.len()
        );
    } else {
        println!("writing these back would change something: {changed:?}");
        std::process::exit(1);
    }
}
