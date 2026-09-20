//! Nothing that ships may carry a credential.
//!
//! `defaults/` is what an install copies onto a new machine, so a token in there is a token given away. The files that
//! are *meant* to hold one - `endpoints.json`, and a `values.json` spec - ship with the slot **empty**, because a file
//! that is absent teaches nothing and a file with a credential teaches the wrong thing.
//!
//! This runs over the tree rather than over a list, so a file added tomorrow is covered without anyone remembering.

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use std::path::{Path, PathBuf};

/// The alphabet a token is usually made of, which is how a prefix is told from prose.
fn is_token_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' || c == '_' || c == '-'
}

/// What gives a credential away, if anything does.
///
/// Written out rather than matched with a regex: three shapes are all this needs, and a test that pulls in a pattern
/// crate to check three things is a dependency that has to be kept working.
pub fn looks_like_a_credential(text: &str) -> Option<String> {
    // 1. a known prefix, which is the unmistakable one
    for prefix in ["ghp_", "gho_", "ghs_", "ghu_", "github_pat_", "sk-"] {
        if let Some(at) = text.find(prefix) {
            let tail: String = text[at + prefix.len()..].chars().take(40).collect();
            if tail.chars().filter(|c| is_token_character(*c)).count() >= 16 {
                return Some(format!("what looks like a key beginning `{prefix}`"));
            }
        }
    }
    // 2. a labelled secret with a value beside it: `"token": "something long"`
    for label in [
        "\"token\"",
        "\"password\"",
        "\"secret\"",
        "\"api_key\"",
        "\"apikey\"",
    ] {
        let mut rest = text;
        while let Some(at) = rest.find(label) {
            rest = &rest[at + label.len()..];
            let Some(colon) = rest.find(':') else { break };
            let after = rest[colon + 1..].trim_start();
            let after = after.strip_prefix('"').unwrap_or(after);
            let value: String = after.chars().take(80).collect();
            let value = value.split('"').next().unwrap_or("");
            if value.len() >= 20 && !value.eq_ignore_ascii_case("null") {
                return Some(format!("a value beside `{label}`"));
            }
            if after.len() < 2 {
                break;
            }
        }
    }
    // No third rule. A bare "this is forty characters with no spaces" test cannot tell a credential from the API
    // url in `ci.json` or the save path in `cp2077-hud.json` - and a scanner that fails the build on those is a
    // scanner that gets switched off. The two shapes above are the ones a real credential actually has.
    None
}

fn defaults() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../defaults")
}

#[test]
fn the_scanner_finds_what_it_is_for_and_leaves_what_it_is_not() {
    // without this, a scanner that finds nothing passes forever
    assert!(
        looks_like_a_credential("{\"token\": \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\"}")
            .is_some(),
        "a GitHub token went unnoticed"
    );
    assert!(
        looks_like_a_credential("{\"token\": \"averylongsecretvaluegoeshere\"}").is_some(),
        "a value beside a token label went unnoticed"
    );
    assert!(
        looks_like_a_credential("{\"api_key\": \"9f4c1e77ab23d8e05c6f4a91bd7e2308\"}").is_some(),
        "a labelled api key went unnoticed"
    );
    // and the shapes that are not credentials, including the template itself
    assert!(looks_like_a_credential("{\"token\": null}").is_none());
    assert!(looks_like_a_credential("G1=p,k.59\nLR.hold=menu\n").is_none());
    assert!(looks_like_a_credential("{\"name\": \"temps\", \"format\": \"cpu {cpu}%\"}").is_none());
    // the two that tripped the first version of this, and must not trip it again
    assert!(
        looks_like_a_credential(
            "\"state\": \"http:github/actions/runs?per_page=1#workflow_runs.0.conclusion\""
        )
        .is_none(),
        "an endpoint url is not a credential"
    );
    assert!(
        looks_like_a_credential(
            "\"ammo\": \"json:/home/somebody/Games/Heroic/Cyberpunk 2077/save/current.json#ammo\""
        )
        .is_none(),
        "a file path is not a credential"
    );
}

#[test]
fn no_file_that_ships_carries_a_credential() {
    let mut checked = 0;
    let mut offenders = Vec::new();
    let mut stack = vec![defaults()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            checked += 1;
            if let Some(what) = looks_like_a_credential(&text) {
                offenders.push(format!("{}: {what}", path.display()));
            }
        }
    }
    assert!(
        checked > 20,
        "the defaults folder is not being read: {checked} files"
    );
    assert!(
        offenders.is_empty(),
        "something that ships carries a credential:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn the_endpoint_template_shows_the_slot_and_does_not_fill_it() {
    let text =
        std::fs::read_to_string(defaults().join("endpoints.json")).expect("the template ships");
    assert!(
        text.contains("\"token\""),
        "the template should show where a credential goes: {text}"
    );
    assert!(
        !text
            .lines()
            .any(|line| line.contains("\"token\"") && !line.contains("null")),
        "the template carries a token: {text}"
    );
}
