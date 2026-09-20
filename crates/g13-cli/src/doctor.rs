//! `g13 doctor`  -  why isn't my pad working?
//!
//! Every check answers with a fact and, when something is wrong, what to do about it. Written because being
//! told "nothing was sent" with no explanation is the least useful possible outcome.

use g13_agent::Bindings;
use g13_device::{CONTROLS, input_devices};
use g13_proto::{PRODUCT_G13, VENDOR_LOGITECH};
use std::path::Path;
use std::process::Command;

/// How a check came out. Four levels rather than a boolean, because "worth knowing" is not the same as
/// "fine" and reporting it as fine is how a real problem hides.
/// `Debug` as well as the comparison: a failing test prints the level and the line it came from, which is the
/// only way a report about a machine can be argued with.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Level {
    /// The check passed.
    Ok,
    /// A fact about the current state, neither fine nor broken.
    Note,
    /// Not as it should be, and nothing is broken by it.
    Warn,
    /// Needs attention, and the reason the exit code is not zero.
    Fail,
}

/// One line of the report.
#[derive(Debug)]
struct Check {
    /// How the check came out, which decides the mark and whether it counts as a failure.
    level: Level,
    /// The thing checked, in the left column of the report.
    what: String,
    /// What was found, in the right column.
    detail: String,
    /// What to do about it, printed under the line when there is one.
    fix: Option<String>,
}

impl Check {
    /// A check that passed, with nothing to suggest.
    fn ok(what: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level: Level::Ok,
            what: what.into(),
            detail: detail.into(),
            fix: None,
        }
    }
    /// A check that failed, carrying the fix that is the whole point of reporting it.
    fn bad(what: impl Into<String>, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            level: Level::Fail,
            what: what.into(),
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
    /// Something to know, with nothing to do about it.
    fn warn(what: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            what: what.into(),
            detail: detail.into(),
            fix: None,
        }
    }
    /// Neither fine nor broken: a fact about the current state.
    fn note(what: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level: Level::Note,
            what: what.into(),
            detail: detail.into(),
            fix: None,
        }
    }
}

/// What this machine says about itself.
///
/// Gathered in one place so that every check is a decision about facts handed to it, which is what makes the
/// deciding testable: every fault this tool has had was in the deciding, not in the reading.
struct Machine {
    /// Where `g13` is on PATH, as `which_g13` words it.
    g13_on_path: String,
    /// Whether `/dev/uinput` is there.
    uinput: bool,
    /// The input devices the kernel lists, as `/proc/bus/input/devices` names them.
    devices: Vec<(String, std::path::PathBuf)>,
    /// Whether a pad is on the USB bus at all, whether or not anything has claimed it.
    pad_on_usb: bool,
    /// What systemd says about each unit this project ships, by unit name.
    units: Vec<(String, String)>,
}

/// Read the machine. The only part of this file that touches it.
fn gather() -> Machine {
    Machine {
        g13_on_path: which_g13(),
        uinput: Path::new("/dev/uinput").exists(),
        devices: input_devices(),
        pad_on_usb: usb_present(),
        units: ["g13.service", "g13-rs.service"]
            .iter()
            .map(|unit| ((*unit).to_string(), unit_state(unit)))
            .collect(),
    }
}

/// Every check about the programme and the machine it is running on.
fn machine_checks(machine: &Machine) -> Vec<Check> {
    let mut checks = Vec::new();
    checks.push(Check::ok("g13 on PATH", machine.g13_on_path.clone()));
    checks.push(if machine.uinput {
        Check::ok("virtual keyboard", "/dev/uinput exists")
    } else {
        Check::bad(
            "virtual keyboard",
            "/dev/uinput is missing",
            "the uinput module is needed: sudo modprobe uinput",
        )
    });

    // the pad exposes a device node only while a driver is holding it, so its absence means "nobody has
    // claimed it", not "not plugged in" - the USB check below is what says whether it is there at all
    match machine.devices.iter().find(|(name, _)| name == "G13") {
        Some((name, path)) => checks.push(Check::ok(
            "pad claimed",
            format!("{name} at {}", path.display()),
        )),
        None => checks.push(Check::note(
            "pad claimed",
            "no pad device node: nothing is holding the pad, which is why it is not a device right now",
        )),
    }
    let ours = machine
        .devices
        .iter()
        .any(|(name, _)| name == g13_device::keyboard::DEVICE_NAME);
    checks.push(if ours {
        Check::ok(
            "our keyboard",
            format!("{} is registered", g13_device::keyboard::DEVICE_NAME),
        )
    } else {
        Check::warn(
            "our keyboard",
            "not registered: no g13 run in progress, so nothing is being sent",
        )
    });

    // --- which driver holds the pad
    for (unit, state) in &machine.units {
        checks.push(match state.as_str() {
            "active" => Check::ok(unit.clone(), "active"),
            "inactive" => Check::warn(unit.clone(), "inactive"),
            other => Check::warn(unit.clone(), other),
        });
    }

    // --- the USB device itself, which is true whether or not a driver is running
    checks.push(match machine.pad_on_usb {
        true => Check::ok(
            "pad on USB",
            format!("{VENDOR_LOGITECH:04x}:{PRODUCT_G13:04x} found"),
        ),
        false => Check::bad(
            "pad on USB",
            format!("{VENDOR_LOGITECH:04x}:{PRODUCT_G13:04x} not found"),
            "check the cable, and `lsusb | grep -i logitech`",
        ),
    });
    checks
}

/// Every check about the configuration in one directory, decided from what is in it.
///
/// The directory is a parameter rather than a lookup, so a test can hand it a directory of its own - and so
/// that nothing here can read a different one than the driver would.
fn config_checks(config: &Path) -> Vec<Check> {
    let mut checks = Vec::new();
    let profile = g13_config::read_active_profile_in(config);
    let path = g13_config::bindings_path_in(config, profile);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let bindings = Bindings::from_text(&text);
            checks.push(Check::ok(
                "bindings",
                format!("{} controls bound, in {}", bindings.len(), path.display()),
            ));
            let problems = bindings.problems();
            if !problems.is_empty() {
                let known: Vec<&String> = problems
                    .iter()
                    .filter(|problem| {
                        // `G1.hold` names G1, the same control asked a second question, so the suffix comes off
                        // before the name is looked up. It is a hold this build reads, not a stranger in the file.
                        let name = problem.split(':').next().unwrap_or("").trim();
                        let name = name.strip_suffix(".hold").unwrap_or(name);
                        // a sector of the stick is understood too: it is bound in the same file and is not a
                        // control, so a file full of them is not a file full of problems
                        CONTROLS.contains(&name) || g13_config::stick::Sectors::is_sector_name(name)
                    })
                    .collect();
                if !known.is_empty() {
                    checks.push(Check::bad(
                        "bindings applied",
                        format!(
                            "{} line(s) name a control this build cannot use",
                            known.len()
                        ),
                        "run `g13 bindings` to see each one",
                    ));
                } else {
                    checks.push(Check::warn(
                        "bindings applied",
                        format!(
                            "{} line(s) name something that is not a control on this pad",
                            problems.len()
                        ),
                    ));
                }
            } else {
                checks.push(Check::ok("bindings applied", "every line was used"));
            }
            let mut names: Vec<String> = text
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
                .filter_map(|line| {
                    line.split_once('=').map(|(name, _)| {
                        let name = name.trim();
                        name.strip_suffix(".hold").unwrap_or(name).to_string()
                    })
                })
                .collect();
            names.sort();
            names.dedup();
            let unknown: Vec<&String> = names
                .iter()
                .filter(|name| {
                    !CONTROLS.contains(&name.as_str())
                        && !g13_config::stick::Sectors::is_sector_name(name)
                        && *name != "color"
                })
                .collect();
            if !unknown.is_empty() {
                checks.push(Check::warn(
                    "names in the file",
                    format!(
                        "{} not a control on this pad: {}",
                        unknown.len(),
                        unknown
                            .iter()
                            .take(6)
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                ));
            }
        }
        Err(_) => checks.push(Check::bad(
            "bindings",
            format!("no file at {}", path.display()),
            "set one: g13 bind G1 a",
        )),
    }

    checks.push(Check::ok("active profile", format!("{profile}")));

    // --- the credentials, which nothing else reports. A token sent over a link that cannot keep it is a
    // fact about the configuration; it is a warning rather than a failure, because an `http` host on a
    // private network is a real setup and this is not the tool that decides otherwise
    let endpoints_path = config.join("endpoints.json");
    match g13_sources::load_endpoints(&endpoints_path) {
        Ok(endpoints) => {
            let risky: Vec<String> = endpoints
                .iter()
                .filter_map(|(name, endpoint)| {
                    g13_sources::endpoint_risk(endpoint).map(|risk| format!("{name}: {risk}"))
                })
                .collect();
            checks.push(match risky.is_empty() {
                true => Check::ok("endpoint tokens", "every one is sent over a checked link"),
                false => Check::warn("endpoint tokens", risky.join("; ")),
            });
        }
        // no file, or one that will not read, is not this check's business to shout about
        Err(problem) => checks.push(Check::note("endpoint tokens", problem)),
    }
    checks
}

/// Print the report, and say whether anything needs attention.
fn report(checks: &[Check]) -> i32 {
    let mut failed = 0;
    for check in checks {
        let mark = match check.level {
            Level::Ok => "ok  ",
            Level::Note => "note",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        if check.level == Level::Fail {
            failed += 1;
        }
        println!("  [{mark}] {:<18} {}", check.what, check.detail);
        if let Some(fix) = &check.fix {
            println!("         {:<18} → {fix}", "");
        }
    }

    println!();
    if failed == 0 {
        println!(
            "Nothing wrong. If keys still do nothing, run the driver in a terminal and watch it:"
        );
        println!("    systemctl --user stop g13.service && g13 run");
        0
    } else {
        println!("{failed} thing(s) need attention, above.");
        1
    }
}

/// The report as a whole: every check on this machine and its configuration, and 1 when any failed.
pub fn run() -> i32 {
    // the program's name, not the crate's: `g13-cli` is this file's address in the workspace and nothing a
    // person running `g13 doctor` can do anything with
    println!("g13 doctor  -  {}\n", env!("CARGO_PKG_VERSION"));
    let mut checks = machine_checks(&gather());
    checks.extend(config_checks(&g13_config::config_dir()));
    report(&checks)
}

/// Where `g13` is on PATH, or the symlink that would put it there.
fn which_g13() -> String {
    match Command::new("sh").arg("-c").arg("command -v g13").output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => {
            let here = std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "g13".to_string());
            format!("not on PATH (running as {here})  -  link it: ln -s {here} ~/.local/bin/g13")
        }
    }
}

/// What systemd says the unit is doing, or `unknown` where there is no systemd to ask.
fn unit_state(unit: &str) -> String {
    match Command::new("systemctl")
        .args(["--user", "is-active", unit])
        .output()
    {
        Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_string(),
        Err(_) => "unknown".to_string(),
    }
}

/// Whether the pad's vendor and product ids are on the USB bus, claimed by anything or not.
fn usb_present() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/bus/usb/devices") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let vendor = std::fs::read_to_string(entry.path().join("idVendor")).unwrap_or_default();
        let product = std::fs::read_to_string(entry.path().join("idProduct")).unwrap_or_default();
        vendor.trim().eq_ignore_ascii_case("046d") && product.trim().eq_ignore_ascii_case("c21c")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("g13-doctor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn find<'a>(checks: &'a [Check], what: &str) -> &'a Check {
        checks
            .iter()
            .find(|check| check.what == what)
            .unwrap_or_else(|| {
                panic!(
                    "no check called {what}: {:?}",
                    checks.iter().map(|check| &check.what).collect::<Vec<_>>()
                )
            })
    }

    fn named(checks: &[Check], what: &str) -> bool {
        checks.iter().any(|check| check.what == what)
    }

    /// A machine where everything is fine, so each test breaks one thing.
    fn a_machine() -> Machine {
        Machine {
            g13_on_path: "/usr/bin/g13".to_string(),
            uinput: true,
            devices: Vec::new(),
            pad_on_usb: true,
            units: vec![("g13.service".to_string(), "active".to_string())],
        }
    }

    #[test]
    fn our_own_keyboard_is_not_mistaken_for_the_pad() {
        // the fault this is here for: our own `g13 keyboard` was listed among the devices, matched the "is the
        // pad here" test, and the report said the pad was claimed while nothing was holding it
        let mut machine = a_machine();
        machine.devices = vec![(
            g13_device::keyboard::DEVICE_NAME.to_string(),
            "/dev/input/event9".into(),
        )];
        let checks = machine_checks(&machine);
        assert_eq!(find(&checks, "our keyboard").level, Level::Ok);
        assert_eq!(
            find(&checks, "pad claimed").level,
            Level::Note,
            "nothing is holding the pad, so it is a note and not a claim"
        );
    }

    #[test]
    fn a_pad_that_is_held_says_where_it_is() {
        let mut machine = a_machine();
        machine.devices = vec![("G13".to_string(), "/dev/input/event7".into())];
        let checks = machine_checks(&machine);
        let claimed = find(&checks, "pad claimed");
        assert_eq!(claimed.level, Level::Ok);
        assert!(claimed.detail.contains("event7"), "{}", claimed.detail);
    }

    #[test]
    fn a_pad_that_is_not_on_the_cable_is_a_failure_that_says_what_to_check() {
        let mut machine = a_machine();
        machine.pad_on_usb = false;
        let checks = machine_checks(&machine);
        let usb = find(&checks, "pad on USB");
        assert_eq!(usb.level, Level::Fail);
        assert!(
            usb.fix.as_deref().unwrap_or_default().contains("cable"),
            "a failure without the thing to check is half a report: {usb:?}"
        );
    }

    #[test]
    fn a_driver_that_is_not_running_is_a_warning_and_not_a_failure() {
        let mut machine = a_machine();
        machine.units = vec![("g13.service".to_string(), "inactive".to_string())];
        let checks = machine_checks(&machine);
        assert_eq!(find(&checks, "g13.service").level, Level::Warn);
        assert_eq!(find(&checks, "our keyboard").level, Level::Warn);
    }

    #[test]
    fn an_unknown_control_is_named_as_a_warning_and_a_known_one_is_not() {
        let dir = dir("unknown-name");
        std::fs::write(
            dir.join("bindings-0.properties"),
            "G1=p,k.30\nNOPE=p,k.17\n",
        )
        .unwrap();
        let checks = config_checks(&dir);
        let unknown = find(&checks, "names in the file");
        assert_eq!(unknown.level, Level::Warn);
        assert!(unknown.detail.contains("NOPE"), "{}", unknown.detail);
        // and a file of names this build knows has no such line at all
        std::fs::write(dir.join("bindings-0.properties"), "G1=p,k.30\nJUP=p,k.17\n").unwrap();
        assert!(!named(&config_checks(&dir), "names in the file"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hold_line_is_not_a_stranger_in_the_file() {
        // `G1.hold` is G1 asked a second question, and a file of holds is not a file of problems: this line used
        // to be counted as a control this build cannot use
        let dir = dir("hold-line");
        std::fs::write(
            dir.join("bindings-0.properties"),
            "G1=p,k.30\nG1.hold=m,4,1\nLR.hold=menu\n",
        )
        .unwrap();
        let checks = config_checks(&dir);
        assert!(!named(&checks, "names in the file"));
        assert_eq!(find(&checks, "bindings applied").level, Level::Ok);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_token_over_http_is_reported_and_the_shipped_weather_host_is_not() {
        let dir = dir("tokens");
        std::fs::write(
            dir.join("endpoints.json"),
            r#"{"intranet": {"url": "http://10.0.0.2:8080", "token": "a-token"},
                "weather": {"url": "https://wttr.in", "insecure": true}}"#,
        )
        .unwrap();
        let checks = config_checks(&dir);
        let tokens = find(&checks, "endpoint tokens");
        assert_eq!(tokens.level, Level::Warn);
        assert!(tokens.detail.contains("intranet"), "{}", tokens.detail);
        assert!(
            !tokens.detail.contains("weather"),
            "an insecure host with nothing to give away is not a warning: {}",
            tokens.detail
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_check_is_a_decision_about_the_directory_it_was_given() {
        // the fault this is here for, in the same shape as `read_published_values_from`: a helper split out of
        // another kept reading the configured path, so every caller passed a directory that was ignored
        let dir = dir("the-parameter");
        std::fs::write(dir.join("active-profile"), "2\n").unwrap();
        std::fs::write(dir.join("bindings-2.properties"), "G1=p,k.30\n").unwrap();
        // a different profile's file, with something in it that would be reported if it were the one read
        std::fs::write(dir.join("bindings-0.properties"), "NOPE=p,k.17\n").unwrap();
        let checks = config_checks(&dir);
        assert_eq!(find(&checks, "active profile").detail, "2");
        let bindings = find(&checks, "bindings");
        assert!(
            bindings.detail.contains("bindings-2.properties"),
            "the wrong profile's file was read: {}",
            bindings.detail
        );
        assert!(
            bindings.detail.contains(&dir.display().to_string()),
            "the configured directory was read rather than the one given: {}",
            bindings.detail
        );
        assert!(!named(&checks, "names in the file"), "bindings-0 was read");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_bindings_file_is_a_failure_with_the_command_that_fixes_it() {
        let dir = dir("nothing");
        let checks = config_checks(&dir);
        let bindings = find(&checks, "bindings");
        assert_eq!(bindings.level, Level::Fail);
        assert!(
            bindings
                .fix
                .as_deref()
                .unwrap_or_default()
                .contains("g13 bind"),
            "{bindings:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
