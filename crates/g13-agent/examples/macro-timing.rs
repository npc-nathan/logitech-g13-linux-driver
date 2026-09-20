//! Time a real playback of a macro file, and say what that file asked for.
//!
//! A temporary instrument, not a shipped tool: it exists to answer "does a macro come out at the speed it
//! went in" with numbers rather than with a guess.
//!
//! Every key is replaced with F24 (194) before playing, so this can be run on a live desktop without typing
//! anything into whatever window happens to have focus. The *delays* are untouched, and the number of keys is
//! the same, so the timing measured is the timing the real macro would have.

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
    let path = std::env::args()
        .nth(1)
        .expect("usage: macro-timing <macro-N.properties>");
    let text = std::fs::read_to_string(&path).expect("read the macro");
    let macro_file = g13_config::parse_macro(&text);
    println!(
        "{}: id {} name {:?}, {} steps",
        path,
        macro_file.id,
        macro_file.name,
        macro_file.steps.len()
    );

    // the delays as the file has them, with every key made harmless
    let steps: Vec<g13_config::MacroStep> = macro_file
        .steps
        .iter()
        .map(|step| match step {
            g13_config::MacroStep::KeyDown(_) => g13_config::MacroStep::KeyDown(194),
            g13_config::MacroStep::KeyUp(_) => g13_config::MacroStep::KeyUp(194),
            g13_config::MacroStep::Delay(ms) => g13_config::MacroStep::Delay(*ms),
        })
        .collect();
    let harmless = g13_config::Macro {
        name: macro_file.name.clone(),
        id: macro_file.id,
        steps,
    };

    // the same rule the player uses: a pause at the very start is the trigger, not the pattern
    let played_steps: Vec<g13_config::MacroStep> = match harmless.steps.first() {
        Some(g13_config::MacroStep::Delay(_)) => harmless.steps[1..].to_vec(),
        _ => harmless.steps.clone(),
    };
    let due = g13_config::step_deadlines(&played_steps);
    println!(
        "the file asks for {:.3}s in all ({} key steps, {} waits)",
        due.last().map(|d| d.as_secs_f64()).unwrap_or_default(),
        harmless
            .steps
            .iter()
            .filter(|s| !matches!(s, g13_config::MacroStep::Delay(_)))
            .count(),
        harmless
            .steps
            .iter()
            .filter(|s| matches!(s, g13_config::MacroStep::Delay(_)))
            .count()
    );

    let keyboard = g13_device::keyboard::VirtualKeyboard::new().expect("a virtual keyboard");
    let keyboard = std::sync::Arc::new(std::sync::Mutex::new(keyboard));
    let started = std::time::Instant::now();
    let mut marks: Vec<u128> = Vec::new();
    let mut want: Vec<u128> = Vec::new();
    let mut index = 0usize;
    g13_agent::play_macro_reporting(&keyboard, &harmless, 1, |_| {
        marks.push(started.elapsed().as_millis());
        want.push(due.get(index).map(|d| d.as_millis()).unwrap_or_default());
        index += 1;
    })
    .expect("played");

    let played = started.elapsed().as_millis();
    println!(
        "it took {:.3}s, so {:.1}ms of drift over {} steps",
        played as f64 / 1000.0,
        played as f64 - due.last().map(|d| d.as_millis()).unwrap_or_default() as f64,
        marks.len()
    );
    let mut worst = 0i128;
    for (got, asked) in marks.iter().zip(&want) {
        let late = *got as i128 - *asked as i128;
        if late.abs() > worst.abs() {
            worst = late;
        }
    }
    println!("worst step was {worst}ms from where the file puts it");
    for (got, asked) in marks.iter().zip(&want).take(14) {
        println!("  file {asked:>5}ms   actually {got:>5}ms");
    }
    if marks.len() > 14 {
        println!("  ... {} steps in all", marks.len());
    }

    // What one press and release costs on this machine: a schedule cannot absorb work that happens between
    // two steps with no wait at all, so this is the part of the timing that is not mine to place.
    let taps = 50;
    let mut steps = Vec::new();
    for _ in 0..taps {
        steps.push(g13_config::MacroStep::KeyDown(194));
        steps.push(g13_config::MacroStep::KeyUp(194));
    }
    let bang = g13_config::Macro {
        name: "timing".to_string(),
        id: 0,
        steps,
    };
    let started = std::time::Instant::now();
    g13_agent::play_macro(&keyboard, &bang, 1).expect("played");
    let each = started.elapsed().as_secs_f64() / (taps as f64 * 2.0);
    println!(
        "a single key event costs {:.3}ms ({taps} press/release pairs took {:.3}s)",
        each * 1000.0,
        started.elapsed().as_secs_f64()
    );
}
