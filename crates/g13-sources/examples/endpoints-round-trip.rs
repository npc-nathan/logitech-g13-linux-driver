//! Round-trip an `endpoints.json` and report whether writing it back would change anything.
//!
//!   cargo run -p g13-sources --example endpoints-round-trip --release -- ~/.config/g13/endpoints.json
//!
//! `endpoints.json` is where a credential lives, so the editor that writes it has to be able to say what it
//! would do to the file before it does it. This reads the file, writes what the window would write, and
//! compares the two as data - values included, token compared only by length and never printed.

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_sources::{endpoints_to_json, load_endpoints};
use std::collections::BTreeMap;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: endpoints-round-trip <path to endpoints.json>");
        std::process::exit(2);
    });
    let path = std::path::PathBuf::from(path);

    let before = match load_endpoints(&path) {
        Ok(endpoints) => endpoints,
        Err(problem) => {
            println!("could not be read: {problem}");
            std::process::exit(1);
        }
    };
    println!("{} endpoints in {}", before.len(), path.display());
    for (name, endpoint) in &before {
        println!(
            "  {name:<12} {}   token {}   timeout {}   insecure {}",
            endpoint.url,
            match &endpoint.token {
                // the length only: this prints to a terminal that may be watched
                Some(token) => format!("yes, {} characters", token.chars().count()),
                None => "no".to_string(),
            },
            match endpoint.timeout {
                Some(seconds) => format!("{seconds}s"),
                None => "default".to_string(),
            },
            endpoint.insecure
        );
    }

    // what the window would write, then read back
    let written = endpoints_to_json(&before);
    let after: BTreeMap<String, g13_sources::Endpoint> = g13_sources::parse_endpoints(&written);

    let mut differences = Vec::new();
    for (name, was) in &before {
        match after.get(name) {
            None => differences.push(format!("{name} would be lost")),
            Some(is) => {
                if was.url != is.url {
                    differences.push(format!("{name}: url would become {}", is.url));
                }
                if was.token != is.token {
                    differences.push(format!("{name}: the token would change"));
                }
                if was.timeout != is.timeout {
                    differences.push(format!("{name}: timeout would change"));
                }
                if was.insecure != is.insecure {
                    differences.push(format!("{name}: insecure would change"));
                }
            }
        }
    }
    for name in after.keys() {
        if !before.contains_key(name) {
            differences.push(format!("{name} would appear from nowhere"));
        }
    }

    if differences.is_empty() {
        println!(
            "\nwriting it back would change nothing: same names, same urls, same credentials."
        );
    } else {
        println!("\nwriting it back would change something:");
        for difference in differences {
            println!("  {difference}");
        }
        std::process::exit(1);
    }
}
