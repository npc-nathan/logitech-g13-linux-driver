//! `g13 applet`  -  see what an applet would put on the screen.
//!
//! `list` shows what is installed, `preview` draws it as text so the layout can be read without the pad, and
//! `check` reports the things that would make it wrong: sources that gave nothing, lines that do not apply
//! to this build, and text that landed on ink.

use g13_applets::{Applet, gather, parse_applet, values_nothing_provides};
use g13_sources::World;
use std::path::PathBuf;

/// The values the user named, resolved, with anything wrong with them said on stderr.
///
/// The CLI is not a thread and not a driver, so it may speak. What it may not do is drop the complaints: a
/// `values.json` with a typo in it would otherwise look exactly like a value that does not work.
fn named_values() -> std::collections::BTreeMap<String, g13_values::Value> {
    let config = g13_config::config_dir();
    let (named, complaints) = g13_sources::named_values(
        &config.join("values.json"),
        &g13_agent::visuals::values_path(),
    );
    for problem in complaints {
        eprintln!("values.json: {problem}");
    }
    named
}

/// The applet directory, as the configuration keeps it.
pub fn applet_dir() -> PathBuf {
    g13_config::config_dir().join("applets")
}

/// What `g13 applet` was asked for: `list`, `preview <name>` or `check [<name>|--all]`.
pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("list") | None => list(),
        Some("preview") => match args.get(1) {
            Some(name) => {
                // numbered as the window, the menu and `g13 lcd` number them: screen 1 is the first
                let screen = args
                    .iter()
                    .position(|a| a == "--screen")
                    .and_then(|at| args.get(at + 1))
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(1)
                    .saturating_sub(1);
                preview(name, args.iter().any(|a| a == "--text"), screen, args)
            }
            None => {
                println!("g13 applet preview <name>    -  draw it as text");
                2
            }
        },
        Some("check") => {
            // `--all` is what the predecessor offered as well, and it is the one that matters before a release:
            // every applet checked, and an exit code that says whether any of them would misbehave on the pad.
            if args.iter().any(|a| a == "--all") {
                return check_all();
            }
            match args.get(1) {
                Some(name) => check(name),
                None => {
                    println!("g13 applet check <name>      -  report what would be wrong");
                    println!(
                        "g13 applet check --all      -  every applet, and a failing exit code if any"
                    );
                    println!("and `g13 applet list` for the names.");
                    2
                }
            }
        }
        Some(other) => {
            println!("g13 applet: {other} is not a subcommand. Try list, preview or check.");
            2
        }
    }
}

/// The applet names in the applet directory, sorted, with each applet's `.bitmaps.json` sidecar left out.
fn names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(applet_dir())
        .into_iter()
        .flatten()
        .flatten()
        // the same rule the window asks: `applets/<name>.bitmaps.json` is that applet's pictures, and a
        // listing that takes every `.json` lists an applet called `weather.bitmaps` that is not one
        .filter(|entry| g13_applets::is_applet_file(&entry.path()))
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })
        .collect();
    names.sort();
    names
}

/// One applet as the driver sees it: parsed, with its bitmaps, fonts and theme.
fn read(name: &str) -> Result<Applet, String> {
    let path = applet_dir().join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let applet = parse_applet(&text).map_err(|problem| format!("{}: {problem}", path.display()))?;
    // The applet as the driver sees it: with its pictures and its theme.
    //
    // Without this `g13 applet check` said "no bitmap called Play" about a picture that exists, and a preview drew
    // nothing where a bitmap widget is - a check that cries wolf and a preview that lies are both worse than
    // neither existing. One loader, as the driver has one.
    let (faces, _) = g13_applets::fonts_cached(&g13_agent::visuals::fonts_dir(&applet_dir()));
    let applet = g13_applets::with_font(g13_applets::with_bitmaps(applet, &applet_dir()), &faces);
    Ok(g13_applets::with_theme(
        applet,
        &g13_agent::visuals::themes_dir(&applet_dir()),
    ))
}

/// The values a preview or check resolves against, including the ones only a running driver knows.
fn world() -> World {
    World::default()
        .with_named(named_values())
        .with_endpoints(&g13_config::config_dir().join("endpoints.json"))
        // the names only the running driver knows: without this, `g13 applet check` would call `stick_x`
        // unprovided while the pad answers it perfectly well
        .with_values(&g13_agent::visuals::values_path())
}

/// The installed applets with their shapes, or a note that there are none.
fn list() -> i32 {
    let names = names();
    if names.is_empty() {
        println!("no applets in {}", applet_dir().display());
        return 0;
    }
    println!("applets in {}:", applet_dir().display());
    for name in names {
        match read(&name) {
            Ok(applet) => {
                let unhandled = if applet.unhandled.is_empty() {
                    String::new()
                } else {
                    format!("  ({} thing(s) not acted on)", applet.unhandled.len())
                };
                let count = g13_applets::screen_count(&applet);
                let screens = if count > 1 {
                    format!(", {count} screens")
                } else {
                    String::new()
                };
                println!(
                    "  {name:<16} {} widget(s), {} source(s), every {}s{screens}{unhandled}",
                    // the widgets of the screen showing first, so a one-screen applet reads as it always did
                    g13_applets::screen_widgets(&applet, 0).len(),
                    applet.sources.len(),
                    applet.interval
                );
            }
            Err(problem) => println!("  {name:<16} {problem}"),
        }
    }
    0
}

/// The frame a preview draws, given the flags.
///
/// Split out of `preview` so a flag can be tested without a terminal. `--selected` is here because it was dropped
/// silently once, when the drawing's arguments changed shape, and nothing but a clippy warning noticed.
fn preview_frame(
    applet: &g13_applets::Applet,
    screen: usize,
    values: &g13_applets::Values,
    args: &[String],
) -> (g13_screen::Frame, Vec<String>) {
    // `--selected N` draws the border the pad would put on the Nth thing this screen can act on, so the
    // highlighted look can be checked without holding the pad. Numbered from one, as a person counts.
    let selected = args
        .iter()
        .position(|a| a == "--selected")
        .and_then(|at| args.get(at + 1))
        .and_then(|n| n.parse::<usize>().ok())
        .map(|n| n.saturating_sub(1));
    let mut frame = g13_screen::Frame::new();
    let mut problems = Vec::new();
    g13_applets::draw_screen_at(
        applet,
        screen,
        g13_applets::Context {
            selected,
            // a preview is a still picture: nothing to scroll it with
            now_seconds: f64::NAN,
            // the profile in force, so a preview of an alert asking about it answers the same way the pad would
            profile: g13_config::read_active_profile(),
        },
        &mut frame,
        values,
        &mut problems,
    );
    (frame, problems)
}

/// One screen of an applet drawn as text, with the values behind it and anything that went wrong.
fn preview(name: &str, text_only: bool, screen: usize, args: &[String]) -> i32 {
    let applet = match read(name) {
        Ok(applet) => applet,
        Err(problem) => {
            println!("{problem}");
            return 1;
        }
    };
    let (values, problems) = gather(&applet, &world());
    let count = g13_applets::screen_count(&applet);
    // a number from a keyboard should not draw a blank screen: wrap it, as the driver does
    let screen = if count > 1 { screen % count } else { 0 };
    let (drawn, drawing_problems) = preview_frame(&applet, screen, &values, args);

    if count > 1 {
        println!(
            "{}  -  {} screen {}/{}: {}",
            applet.title,
            applet.name,
            screen + 1,
            count,
            g13_applets::screen_title(&applet, screen)
        );
    } else {
        println!("{}  -  {}", applet.title, applet.name);
    }
    // written through a writer whose errors are ignored: `g13 applet preview x | head` closes the pipe, and
    // that is a normal thing to do rather than a reason to panic
    {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{drawn:?}");
    }
    println!("values:");
    for (key, value) in &values {
        println!("  {key:<12} {value:?}");
    }
    if text_only {
        return 0;
    }
    if !problems.is_empty() {
        println!("sources that gave nothing:");
        for problem in &problems {
            println!("  {problem}");
        }
    }
    if !drawing_problems.is_empty() {
        println!("drawing problems:");
        for problem in &drawing_problems {
            println!("  {problem}");
        }
    }
    0
}

/// Every applet, one after another, with a count at the end.
///
/// A check that stops at the first problem hides the rest: this one goes through all of them and reports *whether
/// any* would misbehave, which is the question asked before a release or after changing something shared.
fn check_all() -> i32 {
    let names = g13_applets::applet_names(&applet_dir());
    if names.is_empty() {
        println!("no applets in {}", applet_dir().display());
        return 0;
    }
    let mut bad = 0;
    for name in &names {
        if check(name) != 0 {
            bad += 1;
        }
    }
    println!();
    match bad {
        0 => println!("{} applet(s) checked, all clean.", names.len()),
        bad => println!(
            "{} applet(s) checked, {bad} with something to say.",
            names.len()
        ),
    }
    match bad {
        0 => 0,
        _ => 1,
    }
}

/// Draws every screen of an applet and reports what would be wrong, non-zero when anything was.
fn check(name: &str) -> i32 {
    let applet = match read(name) {
        Ok(applet) => applet,
        Err(problem) => {
            println!("{problem}");
            return 1;
        }
    };
    let (values, problems) = gather(&applet, &world());
    let count = g13_applets::screen_count(&applet);
    let mut drawing_problems = Vec::new();
    // every screen is drawn, not only the first: a collision or a bad coordinate on screen 3 is exactly as wrong
    // as one on screen 1, and finding it from the pad is finding it the hard way
    for screen in 0..count {
        let mut frame = g13_screen::Frame::new();
        g13_applets::draw_screen(&applet, screen, &mut frame, &values, &mut drawing_problems);
        if count > 1 {
            // say which screen each complaint belongs to
            for problem in drawing_problems.drain(..) {
                println!(
                    "screen {} ({}): {problem}",
                    screen + 1,
                    g13_applets::screen_title(&applet, screen)
                );
            }
        }
    }

    let mut complaints = 0;
    for problem in &problems {
        println!("source gave nothing: {problem}");
        complaints += 1;
    }
    // an applet's values are the ones it declares and no others, so a name with nothing behind it is drawn as
    // itself on the pad. Report it here rather than leaving it to be seen on the pad.
    for name in values_nothing_provides(&applet, &values) {
        println!("nothing provides this value, so the pad draws {name} as itself");
        complaints += 1;
    }
    for problem in &drawing_problems {
        println!("{problem}");
        complaints += 1;
    }
    for item in &applet.unhandled {
        println!("not acted on: {item}");
    }
    if complaints == 0 {
        let screens = if count > 1 {
            format!(", {count} screens")
        } else {
            String::new()
        };
        println!(
            "{} checks out: {} widget(s){screens}, every source answered.",
            applet.name,
            g13_applets::screen_widgets(&applet, 0).len()
        );
    }
    i32::from(complaints > 0)
}

#[cfg(test)]
mod preview_flag_tests {
    use super::*;

    fn two_buttons() -> g13_applets::Applet {
        g13_applets::parse_applet(
            r#"{"name": "p", "sources": {}, "widgets": [
                 {"type": "button", "x": 4, "y": 2, "w": 40, "h": 9, "format": "ONE", "command": "cmd:true"},
                 {"type": "button", "x": 4, "y": 16, "w": 40, "h": 9, "format": "TWO", "command": "cmd:true"}]}"#,
        )
        .expect("an applet")
    }

    #[test]
    fn the_selected_flag_marks_the_thing_it_names_and_counted_from_one() {
        // This flag was dropped silently once, when the drawing's arguments changed shape, and nothing but an
        // unused-variable lint noticed. It is a preview of the pad's own border, so it has to be a test.
        let applet = two_buttons();
        let values = g13_applets::Values::new();
        let flag = |n: &str| vec!["--selected".to_string(), n.to_string()];

        let (plain, _) = preview_frame(&applet, 0, &values, &[]);
        let (first, _) = preview_frame(&applet, 0, &values, &flag("1"));
        let (second, _) = preview_frame(&applet, 0, &values, &flag("2"));

        assert_ne!(plain, first, "`--selected 1` should change the picture");
        assert_ne!(
            first, second,
            "and `--selected 2` should mark a different thing"
        );

        // What the pad marks a button with is *filling* it, its words punched out - not an outline, because the
        // button already has one. So the probe is a pixel inside each button, above and beside the words.
        assert!(first.get(5, 4), "the first button should be filled in");
        assert!(!first.get(5, 18), "and the second left alone");
        assert!(second.get(5, 18), "`--selected 2` fills the second button");
        assert!(!second.get(5, 4), "and leaves the first");

        // a number nobody has is the same as asking for nothing, rather than a border on the wrong thing
        let (far, _) = preview_frame(&applet, 0, &values, &flag("9"));
        assert_eq!(far, plain, "a selection past the end marks nothing");
        // and text where a number belongs marks nothing either
        let (odd, _) = preview_frame(&applet, 0, &values, &flag("two"));
        assert_eq!(odd, plain);
    }
}
