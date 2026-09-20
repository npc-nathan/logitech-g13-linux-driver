//! `g13 service start|stop|restart|status` - the driver as a user service.
//!
//! The predecessor shipped `g13-service` for this and it is one of the two parity gaps the audit found. Nothing here
//! is clever: the driver is the unit `packaging/g13-rs.service`, `Conflicts=g13.service` keeps the GPL driver and
//! this one from both holding the pad, and systemd does the work. The point is that a person who has just installed
//! this should not have to know that.

use std::process::Command;

/// The unit an install writes. `g13-rs` rather than `g13`, because the old stack's unit is `g13.service` and both
/// may be installed at once - they simply never hold the pad together.
const UNIT: &str = "g13-rs.service";

/// The driver's user unit acted on as asked, defaulting to `status` when nothing is named.
pub fn run(args: &[String]) -> i32 {
    let what = args.first().map(String::as_str).unwrap_or("status");
    match what {
        "start" | "stop" | "restart" | "status" => {
            // `systemctl --user`: this is a user service, and asking for the system one would need root and would
            // start nothing.
            let result = Command::new("systemctl")
                .args(["--user", what, UNIT])
                .status();
            match result {
                Ok(status) if status.success() => 0,
                Ok(_) => {
                    eprintln!("`systemctl --user {what} {UNIT}` did not succeed.");
                    eprintln!("If the unit is not installed, `g13 doctor` says what is missing.");
                    1
                }
                Err(error) => {
                    eprintln!("could not run systemctl: {error}");
                    eprintln!("Without systemd, run the driver directly: `g13 run`");
                    1
                }
            }
        }
        other => {
            eprintln!("g13 service <start|stop|restart|status>, not `{other}`");
            eprintln!("The driver is the user unit {UNIT}. `g13 gui` can also start and stop it.");
            2
        }
    }
}
