//! Walk the screen rotation exactly as the driver does, and print each step.
//!
//! A diagnostic for the question "why did the button stop here": it reads a configuration directory, takes the
//! enabled list and the names it can draw, and prints what `sv,next` and `sv,prev` would give from every screen
//! in the rotation - including the ones it steps over.
//!
//!     cargo run -p g13-agent --example screen-walk -- ~/.config/g13
//!
//! It touches nothing: no pad, no driver, no writes.

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
    let dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(g13_config::config_dir);
    let visuals = g13_agent::visuals::load_visuals(&dir.join("visuals.json"));
    let applets = dir.join("applets");

    println!("configuration: {}", dir.display());
    println!("showing now:   {}", visuals.active);
    println!("cycling:       {}", visuals.cycle);
    println!();
    println!("by number, for binding L1-L4 to particular screens:");
    for (index, name) in visuals.enabled.iter().enumerate() {
        println!("    sv,{:<3} -> {name}", index + 1);
    }
    println!();
    println!("what this build can draw:");
    for name in g13_agent::visuals::drawable_visuals(&applets) {
        println!("    {name}");
    }
    println!();
    println!("the rotation ({} screens):", visuals.enabled.len());
    for name in &visuals.enabled {
        let drawable = g13_agent::visuals::drawable_visuals(&applets).contains(name);
        println!(
            "    {name}{}",
            if drawable {
                ""
            } else {
                "   <- not a screen this build can draw, so it is skipped"
            }
        );
    }
    println!();
    for name in &visuals.enabled {
        let next = g13_agent::visuals::screen_target("next", &visuals.enabled, name, &applets);
        let prev = g13_agent::visuals::screen_target("prev", &visuals.enabled, name, &applets);
        let say = |what: Result<Option<String>, String>| match what {
            Ok(Some(target)) => target,
            Ok(None) => "(nothing: it stays where it is)".to_string(),
            Err(problem) => format!("(refused: {problem})"),
        };
        println!(
            "  from {name:<22} next -> {:<24} prev -> {}",
            say(next),
            say(prev)
        );
    }
}
