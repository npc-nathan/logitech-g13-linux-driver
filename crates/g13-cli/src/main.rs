//! The `g13` binary.
//!
//! Eleven entry points in the previous stack become subcommands of this one.

// a test may unwrap and may fail loudly: a test that cannot panic on a fixture cannot fail
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
// the command line is the voice of the program: everything it prints is the answer to
// something a person typed, which is the one place printing is the job
#![allow(clippy::print_stdout, clippy::print_stderr)]
mod applet;
mod colour;
mod doctor;
mod gui_helper;
mod lcd;
mod macros;
mod record;
mod service;
mod setup;
mod values;

use g13_agent::{Event, active_bindings_path};
use g13_device::keyboard::{self, KeyCode};
use g13_device::{CONTINUOUS, CONTROLS, Device, unaccounted};
use g13_proto::{INPUT_BITS, InputReport, bit_for_control, control_from_name, describe_bit};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::sync::mpsc::{TryRecvError, channel};
use std::time::{Duration, Instant};

fn main() {
    // Before anything reads a config: on a machine where g13 has just been installed, this is what puts the
    // applets, fonts, themes, bindings and macros there. It does nothing at all once a config exists, so it
    // cannot overwrite work, and it means installing the package is the whole install - no second command to
    // know about. Only the fact that something was added is printed, and only when something was.
    let seeded = g13_config::seed_defaults();
    if !seeded.is_empty() {
        println!(
            "g13: {} default file(s) set up in {}",
            seeded.len(),
            g13_config::config_dir().display()
        );
    }

    // A closed pipe is how a person stops reading - `g13 applet list | head` - and Rust ignores SIGPIPE by
    // default, which turns that into a panic message printed over the output. Restoring the default ends the
    // process quietly, as every other command-line tool does.
    #[cfg(unix)]
    // SAFETY: the only global this program changes, and it changes it back to the default. No memory is
    // touched, and nothing but this process observes the result.
    #[allow(unsafe_code)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("watch") => {
            let mapping = args.iter().any(|a| a == "--map");
            // Names are optional. With none, the walk is positional: the point is to learn what the
            // controls are, so it cannot start from a list of what they are called.
            let named: Vec<String> = args
                .iter()
                .skip(1)
                .filter(|a| !a.starts_with("--") && !a.contains('='))
                .cloned()
                .collect();
            match emit_map(&args) {
                Ok(emit) => watch(mapping, &named, emit),
                Err(problem) => {
                    eprintln!("g13 watch: {problem}");
                    2
                }
            }
        }
        Some("run") => run(),
        Some("bind") => {
            let rest: Vec<String> = args.iter().skip(1).cloned().collect();
            bind(&rest)
        }
        Some("bindings") => show_bindings(),
        Some("profile") => profile(&args.iter().skip(1).cloned().collect::<Vec<_>>()),
        Some("doctor") => doctor::run(),
        Some("setup") => setup::run(),
        Some("applet") => applet::run(&args.iter().skip(1).cloned().collect::<Vec<_>>()),
        Some("gui") => gui_helper::run(),
        Some("screen") => {
            // who owns the screen: the driver, or a program that wants it. The predecessor's `g13-buttons`.
            let owner = args.iter().skip(1).find(|a| !a.starts_with('-')).cloned();
            match owner.as_deref() {
                None => {
                    println!("{}", g13_agent::visuals::screen_owner());
                    0
                }
                Some("auto") => match g13_agent::visuals::set_screen_owner(None) {
                    Ok(()) => 0,
                    Err(error) => {
                        eprintln!("could not give the screen back: {error}");
                        1
                    }
                },
                Some(owner) => match g13_agent::visuals::set_screen_owner(Some(owner)) {
                    Ok(()) => 0,
                    Err(error) => {
                        eprintln!("could not take the screen for `{owner}`: {error}");
                        1
                    }
                },
            }
        }
        Some("service") => service::run(&args.iter().skip(1).cloned().collect::<Vec<_>>()),
        Some("lcd") => lcd::run(&args.iter().skip(1).cloned().collect::<Vec<_>>()),
        // both spellings: the file says `color`, and the way it is said out loud here is `colour`
        Some("colour") | Some("color") => {
            colour::run(&args.iter().skip(1).cloned().collect::<Vec<_>>())
        }
        Some("macro") | Some("macros") => {
            macros::run(&args.iter().skip(1).cloned().collect::<Vec<_>>())
        }
        Some("values") => {
            if args.iter().any(|a| a == "--catalogue") {
                values::catalogue()
            } else {
                values::run(args.iter().any(|a| a == "--json"))
            }
        }
        Some("record") => {
            let rest: Vec<String> = args
                .iter()
                .skip(1)
                .filter(|a| !a.starts_with("--"))
                .cloned()
                .collect();
            match rest.first() {
                Some(control) => record::run(control),
                None => {
                    println!("g13 record <control>    -  press the key that control should send");
                    println!("\nfor example:  g13 record G1    then press the key you want");
                    println!("\nthe controls this build knows:");
                    for chunk in CONTROLS.chunks(8) {
                        println!("    {}", chunk.join(" "));
                    }
                    2
                }
            }
        }
        Some("version") | None => {
            println!("g13 {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some(other) => {
            eprintln!("g13: no such command: {other}");
            usage();
            2
        }
    };
    std::process::exit(code);
}

/// The command list, printed when the argument names no command this build has.
fn usage() {
    eprintln!(
        "usage: g13 <command>\n\n\
         the driver and the pad:\n  \
         run             the driver: read the pad, apply the bindings, send keys\n  \
         bind C KEY      set what a control sends, e.g. bind G1 a\n  \
         bindings        show what each control currently sends\n  \
         profile [N]     show the active profile, or switch to N\n  \
         watch           read the pad and print which bits change\n  \
         watch --emit K=V  as above, and type: e.g. --emit G1=a G2=space\n  \
         watch --map     walk the pad's controls and record the bit each one sets\n  \
         record C        press a control, then a key, mouse or gamepad button\n  \
         doctor          check the install: device, permissions, config, services
    setup           put the default applets, fonts, themes and bindings in your own config\n\n\
         the window and the screen:\n  \
         gui             the window: bindings, endpoints, applets, values, controls\n  \
         lcd [--visual V]  what is on the pad, or set the visual\n\
  screen [WHO]      who owns the screen: nothing prints it, `auto` gives it back\n\
  service start|stop|restart|status  the driver as a user service\n\n\
         applets and values:\n  \
         applet list           the applets installed\n  \
         applet preview NAME   draw it as text and print the values it resolved\n  \
         applet check NAME     sources that gave nothing, ink that would collide\n  \
         applet ...            see `g13 applet` for the rest\n  \
         values --catalogue    every value this build publishes, and what each means\n  \
         values [--json]       what the running driver is doing\n  \
         colour [R,G,B]  the screen backlight, e.g. colour 0,153,255\n\n\
         macros:\n  \
         macro list            every macro, and whether it decides anything\n  \
         macro show ID         what it does, step by step or node by node\n  \
         macro play ID [--as C]  play it now, as if control C fired it\n\n\
         version         print the version"
    );
}

/// Lines typed at the terminal, delivered on a channel so the device keeps being read while waiting.
fn typed_lines() -> std::sync::mpsc::Receiver<String> {
    let (sender, receiver) = channel::<String>();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    receiver
}

/// Claims the pad and reports what it does, walked control by control when `--map` was asked for.
fn watch(mapping: bool, named: &[String], emit: EmitMap) -> i32 {
    println!("This claims the pad, which takes it from the running driver.");
    println!("Start it again when you are done:  systemctl --user restart g13.service\n");
    let device = match Device::open() {
        Ok(device) => device,
        Err(error) => {
            eprintln!("g13 watch: {error}");
            return 1;
        }
    };

    if mapping {
        guided_mapping(&device, named)
    } else {
        plain_watch(&device, &emit)
    }
}

/// Which key each control sends, by bit.
pub type EmitMap = BTreeMap<usize, KeyCode>;

/// Parse `--emit CONTROL=KEY ...` into that map.
///
/// The mapping is given rather than assumed: what a pad key should do is the user's decision, and until
/// the bindings work (M2) this is how that decision is expressed.
pub fn emit_map(args: &[String]) -> Result<EmitMap, String> {
    let mut map = EmitMap::new();
    for argument in args {
        // only the pairs that follow --emit are bindings
        let Some((left, right)) = argument.split_once('=') else {
            continue;
        };
        if left.starts_with("--") {
            continue;
        }
        let control = control_from_name(left)
            .ok_or_else(|| format!("no such control: {left} (try G1, LR, LMB, M1, JCLICK)"))?;
        let bit = bit_for_control(control).ok_or_else(|| format!("{left} has no bit"))?;
        let key = keyboard::parse_key(right)?;
        map.insert(bit, key);
    }
    Ok(map)
}

/// How long to wait before suggesting why nothing is arriving.
const SILENCE_HINT: Duration = Duration::from_secs(15);

/// Prints the bits that changed on each report, typing the key `--emit` bound to each.
fn plain_watch(device: &Device, emit: &EmitMap) -> i32 {
    // A keyboard is created only when there is something to send: a plain read should not add a device.
    let mut keyboard = if emit.is_empty() {
        None
    } else {
        match keyboard::VirtualKeyboard::new() {
            Ok(keyboard) => {
                println!(
                    "keyboard \"{}\" created; {} control(s) will type",
                    keyboard::DEVICE_NAME,
                    emit.len()
                );
                Some(keyboard)
            }
            Err(error) => {
                eprintln!("g13 watch: {error}");
                return 1;
            }
        }
    };

    println!("watching the pad ({} bits). ctrl-c to stop.\n", INPUT_BITS);
    let started = Instant::now();
    let mut hinted = false;
    let result = device.watch(|report, pressed, released| {
        if !hinted && started.elapsed() > SILENCE_HINT {
            hinted = true;
            println!(
                "\n  nothing yet after {:?}. If keys do nothing, the driver may not have the pad:",
                started.elapsed()
            );
            println!("    systemctl --user restart g13.service\n");
        }
        if pressed.is_empty() && released.is_empty() {
            return true;
        }
        let down: Vec<String> = pressed.iter().map(|bit| describe_bit(*bit)).collect();
        let up: Vec<String> = released.iter().map(|bit| describe_bit(*bit)).collect();
        if let Some(keyboard) = keyboard.as_mut() {
            for bit in pressed {
                if let Some(key) = emit.get(bit)
                    && let Err(error) = keyboard.press(*key)
                {
                    eprintln!("could not send {key:?}: {error}");
                }
            }
            for bit in released {
                if let Some(key) = emit.get(bit)
                    && let Err(error) = keyboard.release(*key)
                {
                    eprintln!("could not release {key:?}: {error}");
                }
            }
        }
        println!("0x{:014x}   down {down:?}   up {up:?}", report.bits());
        true
    });
    if let Err(error) = result {
        eprintln!("g13 watch: {error}");
        return 1;
    }
    0
}

/// Walk the pad's controls and record what each one reports.
///
/// The names come from the device's own legends, transcribed by whoever can see them, and are kept in
/// `g13_device::CONTROLS`  -  never from the predecessor's configuration or source.
///
/// Continuous controls are read differently. The stick streams while it moves, so a single "which bit
/// changed" answer is meaningless for it: those prompts collect the states passed through and report the
/// aggregate, which is the only way to say what a stick direction actually looks like on the wire.
fn guided_mapping(device: &Device, named: &[String]) -> i32 {
    let controls: Vec<String> = if named.is_empty() {
        CONTROLS.iter().map(|name| name.to_string()).collect()
    } else {
        named.to_vec()
    };

    println!(
        "Walking this pad's {} controls. Press each one when named, then Enter.",
        controls.len()
    );
    println!("Enter alone skips. q then Enter finishes, ctrl-c aborts.\n");

    let typed = typed_lines();

    // Settle first. This device sends an all-zero packet and only then its state, so reading the first
    // packet as the baseline leaves the state bits to be mistaken for the first control's output. The
    // baseline is the last report of a short settling window instead.
    let mut previous = InputReport::default();
    let mut baseline = 0u64;
    let settle = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < settle {
        if let Ok(Some(packet)) = device.read_packet(Duration::from_millis(300))
            && let Some(report) = InputReport::from_packet(&packet)
        {
            previous = report;
            baseline = report.bits();
        }
    }
    println!("baseline bits (device state, not input): 0x{baseline:014x}\n");

    let mut found: Vec<(String, Vec<usize>, bool)> = Vec::new();
    let mut taken: Vec<usize> = Vec::new();
    let mut position = 0usize;

    for control in &controls {
        position += 1;
        let continuous = CONTINUOUS.contains(&control.as_str());
        print!(
            "  {position:>2}/{:<2} {control:<7}{} > ",
            controls.len(),
            if continuous { " (hold it)" } else { "" }
        );
        let _ = io::stdout().flush();

        let mut changes: BTreeMap<usize, u32> = BTreeMap::new();
        let mut new_bits: Vec<usize> = Vec::new();
        // for continuous controls, the set of complete states the device passed through
        let mut states: BTreeMap<u64, u32> = BTreeMap::new();

        let answer = loop {
            match typed.try_recv() {
                Ok(line) => {
                    let drain = Instant::now() + Duration::from_millis(400);
                    while Instant::now() < drain {
                        collect(
                            device,
                            &mut previous,
                            baseline,
                            &mut changes,
                            &mut new_bits,
                            &taken,
                            Some(&mut states),
                        );
                    }
                    break line.trim().to_string();
                }
                Err(TryRecvError::Disconnected) => return 0,
                Err(TryRecvError::Empty) => {}
            }
            collect(
                device,
                &mut previous,
                baseline,
                &mut changes,
                &mut new_bits,
                &taken,
                Some(&mut states),
            );
        };

        if answer.eq_ignore_ascii_case("q") {
            println!();
            break;
        }
        if answer.eq_ignore_ascii_case("s") || (new_bits.is_empty() && states.is_empty()) {
            println!("     nothing recorded\n");
            continue;
        }

        if continuous {
            // the aggregate: the states it passed through, most frequent first
            let mut by_frequency: Vec<(u64, u32)> =
                states.iter().map(|(bits, count)| (*bits, *count)).collect();
            by_frequency.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            let varied: Vec<usize> = (0..INPUT_BITS)
                .filter(|bit| states.keys().any(|state| state >> bit & 1 == 1))
                .collect();
            println!(
                "     {} distinct states, most seen first:",
                by_frequency.len()
            );
            for (state, count) in by_frequency.iter().take(6) {
                let set: Vec<usize> = (0..INPUT_BITS)
                    .filter(|bit| state >> bit & 1 == 1)
                    .collect();
                println!("       {count:>4}x  0x{state:014x}  bits {set:?}");
            }
            if by_frequency.len() > 6 {
                println!("       ... and {} more", by_frequency.len() - 6);
            }
            println!("     bits that ever went high: {varied:?}");
            found.push((control.clone(), varied.clone(), true));
            for bit in varied {
                if !taken.contains(&bit) {
                    taken.push(bit);
                }
            }
            println!();
            continue;
        }

        for bit in &new_bits {
            let repeats = changes.get(bit).copied().unwrap_or(1);
            println!(
                "     bit {bit:>2}   {repeats} change{}",
                if repeats == 1 { "" } else { "s" }
            );
            found.push((control.clone(), vec![*bit], false));
            taken.push(*bit);
        }
        println!();
    }

    println!("== what this pad reported ==");
    for (name, bits, continuous) in &found {
        println!(
            "  {name:<8} {}{}",
            if bits.len() == 1 {
                format!("bit {}", bits[0])
            } else {
                format!("bits {bits:?}")
            },
            if *continuous { "   (continuous)" } else { "" }
        );
    }
    let seen: Vec<usize> = found.iter().flat_map(|(_, bits, _)| bits.clone()).collect();
    println!(
        "\n  {} control(s) answered; {} distinct bits",
        found.len(),
        {
            let mut unique = seen.clone();
            unique.sort_unstable();
            unique.dedup();
            unique.len()
        }
    );
    println!("  bits never seen: {:?}", unaccounted(&seen));
    0
}

/// The range each of the low two bytes spans across the observed states.
///
/// Measured: the stick's activity covers bits 0..=15 rather than one bit per direction, so two bytes are
/// involved. **What those bytes mean is not established**  -  whether they are positions, or a bitfield
/// that includes the device's flag bits, is exactly what using the stick will settle. Reporting the
/// ranges says more than a list of bit numbers without claiming an encoding that has not been shown.
pub fn axis_range(states: &BTreeMap<u64, u32>) -> (u8, u8, u8, u8) {
    let mut x_low = u8::MAX;
    let mut x_high = u8::MIN;
    let mut y_low = u8::MAX;
    let mut y_high = u8::MIN;
    for bits in states.keys() {
        let x = *bits as u8;
        let y = (*bits >> 8) as u8;
        x_low = x_low.min(x);
        x_high = x_high.max(x);
        y_low = y_low.min(y);
        y_high = y_high.max(y);
    }
    if states.is_empty() {
        return (0, 0, 0, 0);
    }
    (x_low, x_high, y_low, y_high)
}

/// Read one packet, if there is one, and fold it into the running picture.
fn collect(
    device: &Device,
    previous: &mut InputReport,
    baseline: u64,
    changes: &mut BTreeMap<usize, u32>,
    new_bits: &mut Vec<usize>,
    taken: &[usize],
    states: Option<&mut BTreeMap<u64, u32>>,
) {
    let Ok(Some(packet)) = device.read_packet(Duration::from_millis(120)) else {
        return;
    };
    let Some(report) = InputReport::from_packet(&packet) else {
        return;
    };
    let (pressed, released) = report.changes_from(previous);
    for bit in pressed.iter().chain(released.iter()) {
        *changes.entry(*bit).or_insert(0) += 1;
    }
    for bit in &pressed {
        // bits present in the baseline are device state; bits another control owns are not new
        if baseline >> bit & 1 == 0 && !taken.contains(bit) && !new_bits.contains(bit) {
            new_bits.push(*bit);
        }
    }
    if let Some(states) = states {
        *states.entry(report.bits()).or_insert(0) += 1;
    }
    *previous = report;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_low_two_bytes_are_reported_as_ranges() {
        let mut states = BTreeMap::new();
        // byte0 = 0x80, byte1 = 0x34  (bits 0..=7 and 8..=15)
        states.insert(0x0000_0000_0000_3480u64, 1);
        // byte0 = 0x7b, byte1 = 0x35
        states.insert(0x0000_0000_0000_357bu64, 1);
        assert_eq!(axis_range(&states), (0x7b, 0x80, 0x34, 0x35));
    }

    #[test]
    fn an_empty_aggregate_is_not_a_panic() {
        assert_eq!(axis_range(&BTreeMap::new()), (0, 0, 0, 0));
    }
}

#[cfg(test)]
mod emit_tests {
    use super::*;

    #[test]
    fn a_binding_resolves_a_control_name_to_its_measured_bit() {
        let map = emit_map(&["G1=a".to_string(), "LR=escape".to_string()]).unwrap();
        assert_eq!(map.get(&16), Some(&KeyCode::new(30)), "G1 is bit 16");
        assert_eq!(map.get(&40), Some(&KeyCode::new(1)), "LR is bit 40");
    }

    #[test]
    fn a_keycode_may_be_given_as_a_number() {
        let map = emit_map(&["M1=44".to_string()]).unwrap();
        assert_eq!(map.get(&45), Some(&KeyCode::new(44)), "M1 is bit 45");
    }

    #[test]
    fn an_unknown_control_or_key_is_an_error_not_a_default() {
        assert!(emit_map(&["NOPE=a".to_string()]).is_err());
        assert!(emit_map(&["G1=wibble".to_string()]).is_err());
    }

    #[test]
    fn arguments_that_are_not_bindings_are_ignored() {
        assert!(
            emit_map(&["--map".to_string(), "G1".to_string()])
                .unwrap()
                .is_empty()
        );
    }
}

/// The driver. This is the thing that replaces the prototype: it keeps running, reads the pad, and sends
/// the key each control is bound to.
fn run() -> i32 {
    println!("g13 run  -  reading the pad and sending the bound keys.");

    let device = match Device::open() {
        Ok(device) => device,
        Err(error) => {
            eprintln!("g13 run: {error}");
            // The one failure worth explaining: it is almost always the udev rule not being in place, and the
            // raw message says only "Access denied", which points at nothing a person can act on.
            let text = error.to_string();
            if text.contains("ccess denied") || text.contains("ermission") {
                eprintln!();
                eprintln!("The pad is there but this user cannot open it. That is the udev rule:");
                eprintln!("    sudo cp packaging/70-g13-record.rules /usr/lib/udev/rules.d/");
                eprintln!("    sudo udevadm control --reload-rules && sudo udevadm trigger");
                eprintln!(
                    "On an install from the package it is already in /usr/lib/udev/rules.d, so if that"
                );
                eprintln!(
                    "is there the rule is simply not applying to /dev/hidraw - log out and back in."
                );
            }
            return 1;
        }
    };

    let mut last_key = Instant::now();
    let device = std::sync::Arc::new(device);
    let result = g13_agent::run(std::sync::Arc::clone(&device), |event| {
        match event {
            Event::Started {
                controls,
                profile,
                problems,
            } => {
                println!(
                    "profile {profile}: {controls} control(s) bound to something this build does"
                );
                for problem in problems {
                    println!("  not applied  -  {problem}");
                }
                if controls == 0 {
                    println!();
                    println!("Nothing is bound, so nothing will be sent. Set a binding with:");
                    println!("    g13 bind G1 a");
                }
            }
            Event::ProfileSwitched { profile, controls } => {
                println!("profile {profile}: {controls} control(s) bound");
            }
            Event::MacroPlayed {
                id,
                name,
                steps,
                repeats,
            } => {
                println!("playing macro {id} \"{name}\" ({steps} step(s) x{repeats})");
            }
            Event::RecordingStarted { id } => {
                println!("recording: macro {id} will be written");
                println!(
                    "  press M1, M2 or M3 to pick the profile, or press the control straight away"
                );
                println!("  to use the profile that is active now");
            }
            Event::RecordingControl { profile, control } => {
                println!("  {control} in profile {profile} will play it");
                println!("  now press the keys, then MR again to finish");
                println!("  (it finishes itself after 5s of no keys, and gives up after 60s)");
            }
            Event::MacroRecorded {
                id,
                name,
                steps,
                profile,
                control,
            } => {
                if steps == 0 {
                    println!(
                        "nothing was pressed, so no macro was written and nothing was changed"
                    );
                } else {
                    println!(
                        "wrote macro {id} \"{name}\" ({steps} step(s)) and bound {control} in profile {profile} to it"
                    );
                }
            }
            Event::Key {
                bit,
                description,
                pressed,
            } => {
                // a line per press, rate limited so a flood cannot drown the terminal
                if last_key.elapsed() > Duration::from_millis(40) {
                    last_key = Instant::now();
                    println!(
                        "  {description} ({bit}) {}",
                        if pressed { "down" } else { "up" }
                    );
                }
            }
            Event::Drew { visual, problems } => {
                for problem in problems {
                    println!("  screen: {problem}");
                }
                let _ = visual;
            }
            Event::Trouble { problems } => {
                // things that went wrong while it was running: a key it could not send, a macro that is not
                // there, a screen it was asked for and cannot draw
                for problem in problems {
                    println!("  {problem}");
                }
            }
            Event::Reloaded { controls } => {
                println!("bindings reloaded: {controls} control(s) send a key")
            }
        }
        true
    });
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("g13 run: {error}");
            1
        }
    }
}

/// Set what a control sends, in the active profile's bindings file.
fn bind(rest: &[String]) -> i32 {
    if rest.len() != 2 {
        eprintln!("usage: g13 bind <control> <action>");
        eprintln!();
        eprintln!("  a key             bind G1 a          (or a keycode: bind G1 44)");
        eprintln!("  a macro           bind G1 m,1,1      (macro 1, once)");
        eprintln!("  a profile switch  bind LR mk,1       (the control that becomes profile 1)");
        eprintln!(
            "  a mouse button    bind G1 mb,left    (left middle right side extra forward back task)"
        );
        eprintln!(
            "  a gamepad button  bind G1 gb,south   (south east north west lb rb lt rt back start"
        );
        eprintln!(
            "                                        guide thumb-l thumb-r dpad-up dpad-down"
        );
        eprintln!("                                        dpad-left dpad-right)");
        eprintln!("  nothing           bind G1 x");
        return 2;
    }
    // A sector of the stick is a binding name but not a control on the pad: the pad prints JUP and its seven
    // siblings, and the stick itself is two axes rather than a bit. Without this the command rejected the
    // very names it printed in its own list of controls.
    // `LR.hold` is LR with a modifier on it: the pad never printed a name with a dot in it
    let (base, hold) = g13_config::control_and_hold(rest[0].trim());
    let wanted_name = base.trim().to_ascii_uppercase();
    let name = if g13_config::stick::Sectors::is_sector_name(&wanted_name) {
        wanted_name
    } else {
        let Some(control) = control_from_name(&wanted_name) else {
            eprintln!(
                "g13 bind: {} is not a control on this pad. Controls are the names printed on it:",
                rest[0]
            );
            eprintln!("  {}", CONTROLS.join(" "));
            eprintln!();
            eprintln!(
                "and the stick is bound by sector: J0 upwards and clockwise, or the names the pad prints"
            );
            eprintln!("  JUP JDOWN JLEFT JRIGHT, and the four between them");
            return 2;
        };
        if hold {
            format!("{}.hold", control.name())
        } else {
            control.name().to_string()
        }
    };
    // The argument is what the bindings file would say. A key may be given as a name or a code for
    // convenience; anything else is taken as the action itself, so a macro or a profile switch is written
    // in the file's own syntax rather than translated into something else.
    let wanted = rest[1].trim().to_string();
    let action = if g13_config::is_action(&wanted) {
        wanted
    } else {
        match keyboard::parse_key(&wanted) {
            Ok(key) => format!("p,k.{}", key.code()),
            Err(problem) => {
                eprintln!("g13 bind: {problem}");
                eprintln!(
                    "(an action is a key, or m,<macro>,<repeats>, mk,<profile>, mb,<mouse button>, gb,<gamepad button>, sv,<next|prev|a number|a visual>, rec, or x)"
                );
                return 2;
            }
        }
    };

    let path = active_bindings_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = g13_config::set_binding(&existing, &name, &action);
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("g13 bind: could not create {}: {error}", parent.display());
        return 1;
    }
    match std::fs::write(&path, updated) {
        Ok(()) => {
            println!("{name} now sends {action}  (in {})", path.display());
            0
        }
        Err(error) => {
            eprintln!("g13 bind: could not write {}: {error}", path.display());
            1
        }
    }
}

/// Show what the active bindings file says each control sends.
fn show_bindings() -> i32 {
    let path = active_bindings_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("no bindings at {}: {error}", path.display());
            eprintln!("set one with:  g13 bind G1 a");
            return 1;
        }
    };
    // the map as the driver uses it: a control the file says nothing about may still do something, and a tool
    // that shows the file rather than the driver is how a key looks dead while the pad is doing something
    let from_file = g13_agent::Bindings::from_text(&text);
    let bindings = g13_agent::effective_bindings(&text, &path);
    println!("{}", path.display());
    for control in CONTROLS {
        let Some(control_value) = control_from_name(control) else {
            continue;
        };
        let Some(bit) = bit_for_control(control_value) else {
            continue;
        };
        // what the build can do is checked first; only then does the raw line get shown
        let by_default = !from_file.bound(bit);
        let note = if by_default {
            format!("  (default: {} names no {control})", path.display())
        } else {
            String::new()
        };
        // What a hold does belongs on the row of the control it belongs to, whichever action the tap has.
        let held = match bindings.hold_for(bit) {
            Some(action) => format!(" (held: {})", g13_agent::describe_action(action)),
            None => String::new(),
        };
        // the hold is added to the rows that show a tap action; the two branches below are *for* a control whose
        // only job is the hold, and saying it twice reads as two lines for one thing
        let note_without_hold = note.clone();
        let note = if held.is_empty() {
            note
        } else {
            format!("{note}{held}")
        };
        if let Some(key) = bindings.key_for(bit) {
            // the key it is, not the number: the words come from the one place that says what an action is, so this
            // and the window's row cannot disagree about the same binding
            println!(
                "  {control:<8} bit {bit:<3} {}{note}",
                g13_agent::describe_action(&format!("p,k.{}", key.code()))
            );
        } else if let Some(profile) = bindings.switch_for(bit) {
            println!("  {control:<8} bit {bit:<3} switches to profile {profile}{note}");
        } else if let Some((id, repeats)) = bindings.macro_for(bit) {
            println!("  {control:<8} bit {bit:<3} plays macro {id} x{repeats}{note}");
        } else if let Some(wanted) = bindings.screen_for(bit) {
            println!(
                "  {control:<8} bit {bit:<3} {}{note}",
                g13_agent::describe_action(&format!("sv,{wanted}"))
            );
        } else if bindings.opens_menu(bit) {
            println!("  {control:<8} bit {bit:<3} opens the menu{note_without_hold}");
        } else if let Some(held) = bindings.hold_for(bit) {
            // a hold with nothing on the tap: said plainly, because a control that does nothing until you hold
            // it looks broken otherwise
            println!(
                "  {control:<8} bit {bit:<3} held: {}{note_without_hold}",
                g13_agent::describe_action(held)
            );
        } else if let Some(target) = bindings.record_for(bit) {
            match target {
                Some(id) => {
                    println!("  {control:<8} bit {bit:<3} records a macro into macro {id}{note}")
                }
                None => println!("  {control:<8} bit {bit:<3} records a macro{note}"),
            }
        } else {
            match g13_config::binding_for(&text, control) {
                Some(action) if action == "x" => {
                    println!("  {control:<8} bit {bit:<3} sends nothing (x)")
                }
                Some(action) => println!(
                    "  {control:<8} bit {bit:<3} {action}   -  {}",
                    g13_agent::describe_action(&action)
                ),
                // nothing the driver can do with it - but is there a line in the file? A line the build does
                // not understand is a mistake to report: calling it "unbound" says the file is fine, and the
                // person is looking at the line that says otherwise
                // nothing the driver can do with it - but is there a line in the file? A line the build does
                // not understand is a mistake to report: calling it "unbound" says the file is fine, and the
                // person is looking at the line that says otherwise. The held line counts too: a hold this build
                // cannot do is dropped exactly as silently as an action it does not know.
                None => match (
                    g13_config::raw_binding_for(&text, control),
                    g13_config::raw_binding_for(&text, &format!("{control}.hold")),
                ) {
                    (Some(written), _) => println!(
                        "  {control:<8} bit {bit:<3} the file says `{written}`, which is not an action this \
                         build knows  -  nothing is sent"
                    ),
                    (None, Some(written)) => println!(
                        "  {control:<8} bit {bit:<3} the file says `{written}` when held, and that cannot be \
                         held yet  -  nothing happens"
                    ),
                    (None, None) => println!("  {control:<8} bit {bit:<3}  -  unbound"),
                },
            }
        }
    }
    // The stick is bound in the same file and by sector rather than by bit, so it is listed here too - what
    // each sector will actually do, including the ones falling back to the cardinals beside them.
    let settings = g13_config::stick::Settings::load(&g13_config::config_dir());
    println!(
        "\nthe stick is read as {} sectors, mode {}",
        settings.sectors.count,
        settings.mode.name()
    );
    let bound = bindings.bound_sectors(&settings.sectors);
    if bound.is_empty() {
        println!("  nothing is bound, so the stick sends nothing");
    } else {
        for (index, what) in &bound {
            println!(
                "  {:<6} sector {:>2} of {}  {}",
                settings.sectors.name(*index),
                index,
                settings.sectors.count,
                what.name()
            );
        }
    }
    let problems = bindings.problems();
    if !problems.is_empty() {
        println!("\nnot applied:");
        for problem in problems {
            println!("  {problem}");
        }
    }
    0
}

/// Show or switch the active profile.
fn profile(rest: &[String]) -> i32 {
    let wanted = rest.iter().find(|a| !a.starts_with("--"));
    match wanted.and_then(|n| n.trim().parse::<u32>().ok()) {
        Some(profile) => match g13_config::write_active_profile(profile) {
            Ok(()) => {
                let path = g13_config::bindings_path(profile);
                let exists = path.exists();
                println!("profile {profile} is now active  ({})", path.display());
                if !exists {
                    println!("note: that file does not exist yet; the driver would send nothing");
                }
                0
            }
            Err(error) => {
                eprintln!("g13 profile: could not write active-profile: {error}");
                1
            }
        },
        None if rest.is_empty() => {
            let active = g13_config::read_active_profile();
            println!(
                "active profile: {active}  ({})",
                g13_config::bindings_path(active).display()
            );
            0
        }
        None => {
            eprintln!("usage: g13 profile [N]");
            2
        }
    }
}
