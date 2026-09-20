//! `g13 lcd`  -  put a frame on the pad's screen.
//!
//! The driver holds the pad while it runs, so this needs the pad free: stop the service, or stop `g13 run`.
//! That is said plainly rather than left as a silent failure, because "nothing appeared" is not a diagnosis.

use g13_device::Device;
use g13_screen::{Frame, TEXT_ROWS};

/// Draws the frame the arguments name on the pad, or prints a built-in visual as text with `--visual`.
pub fn run(args: &[String]) -> i32 {
    let mut frame = Frame::new();

    // `g13 lcd --visual system` draws one of the driver's own visuals once, so a built-in layout can be
    // looked at without running the driver and without the pad
    if let Some(index) = args.iter().position(|argument| argument == "--visual") {
        let name = match args.get(index + 1) {
            Some(name) => name.clone(),
            None => {
                println!("g13 lcd --visual <name>    -  draw a built-in visual as text");
                println!("names: clock, system, pad, media");
                return 2;
            }
        };
        let state = g13_agent::State::new(
            g13_config::read_active_profile(),
            &g13_agent::Bindings::from_text(""),
            Vec::new(),
        );
        let world = g13_sources::World::default()
            .with_endpoints(&g13_config::config_dir().join("endpoints.json"));
        let moment = g13_agent::visuals::Moment {
            state: &state,
            clock: g13_sources::resolve(&g13_sources::Spec::parse("time"), &world)
                .map(|value| value.text())
                .unwrap_or_default(),
            date: g13_sources::resolve(&g13_sources::Spec::parse("date"), &world)
                .map(|value| value.text())
                .unwrap_or_default(),
            day: g13_sources::resolve(&g13_sources::Spec::parse("day"), &world)
                .map(|value| value.text())
                .unwrap_or_default(),
            profile: g13_config::read_active_profile(),
            button_mode: "auto".to_string(),
        };
        // which screen of an applet: the first unless told otherwise, so an applet with several can be drawn
        // without the pad rather than only ever showing the one it starts on
        let screen = args
            .iter()
            .position(|argument| argument == "--screen")
            .and_then(|at| args.get(at + 1))
            .and_then(|number| number.parse::<usize>().ok())
            // the flag names the screen as it is numbered in the window and the menu: 1 is the first
            .unwrap_or(1)
            .saturating_sub(1);
        let (drawn, problems) = g13_agent::visuals::render_screen(
            &name,
            screen,
            &moment,
            &world,
            &g13_config::config_dir().join("applets"),
        );
        if args.iter().any(|argument| argument == "--screen") {
            println!("{name}  -  screen {}", screen + 1);
        } else {
            println!("{name}  - ");
        }
        {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = writeln!(out, "{drawn:?}");
        }
        for problem in problems {
            println!("  {problem}");
        }
        return 0;
    }

    let line = args
        .iter()
        .position(|argument| argument == "--line")
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);

    let clear = args.iter().any(|argument| argument == "--clear");
    let words: Vec<&str> = args
        .iter()
        .filter(|argument| !argument.starts_with("--"))
        .enumerate()
        // drop the value that followed --line, so it is not drawn as text
        .filter(|(index, _)| {
            !(index > &0 && args.get(index - 1).map(String::as_str) == Some("--line"))
        })
        .map(|(_, argument)| argument.as_str())
        .collect();

    if clear || words.is_empty() {
        println!("a blank frame: nothing drawn.");
    } else {
        if line >= TEXT_ROWS {
            println!("there is no text row {line}: the panel shows {TEXT_ROWS} rows of 8 pixels.",);
            return 1;
        }
        let text = words.join(" ");
        frame.text_line(line, &text);
        println!("drawing {text:?} on text row {line}.");
    }

    let device = match Device::open() {
        Ok(device) => device,
        Err(error) => {
            println!("cannot take the pad: {error}");
            println!();
            println!(
                "the LCD is written through the same interface the driver holds, so exactly one of these"
            );
            println!("can be talking to the pad at a time. If a driver is running, stop it first:");
            println!(
                "    systemctl --user stop g13.service     (or stop `g13 run`, or g13-visuals)"
            );
            return 1;
        }
    };

    let report = frame.to_report();
    match device.write_lcd(&report) {
        Ok(written) if written == report.len() => {
            println!("sent {written} bytes. The pad should be showing it.");
            0
        }
        Ok(written) => {
            println!(
                "the device accepted only {written} of {} bytes.",
                report.len()
            );
            1
        }
        Err(error) => {
            println!("the write failed: {error}");
            1
        }
    }
}
