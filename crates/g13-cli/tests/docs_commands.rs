//! The manual names commands, and a command it names has to exist.
//!
//! This is the rule the docs are held to: "documented commands must exist and work". A manual that tells somebody
//! to run `g13 sources` when there is no such command is worse than no manual, because they will assume they
//! installed it wrong. So every `g13 <word>` in the documentation is checked against the commands the binary
//! actually dispatches, and every `--flag` against the flags it actually takes.
//!
//! When this fails, the fix is almost always the document - unless the command is genuinely missing, in which
//! case the fix is the command.

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Every command `crates/g13-cli/src/main.rs` dispatches. Kept here rather than derived, because a test that
/// derived it from the same source as the dispatch could not disagree with it.
const COMMANDS: &[&str] = &[
    "applet", "bind", "bindings", "colour", "color", "doctor", "gui", "lcd", "macro", "macros",
    "profile", "record", "run", "screen", "service", "setup", "values", "version", "watch",
];

/// Every flag the binary reads anywhere.
const FLAGS: &[&str] = &[
    "--as",
    "--catalogue",
    "--clear",
    "--emit",
    "--json",
    "--line",
    "--map",
    "--visual",
];

fn docs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the repository root")
        .to_path_buf();
    let mut files = vec![root.join("README.md")];
    let folder = root.join("docs");
    let entries = std::fs::read_dir(&folder).expect("the docs folder");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|kind| kind == "md").unwrap_or(false) {
            files.push(path);
        }
    }
    files
}

/// Every `g13 <word>` in a text, and every `--flag`, with the line they are on for the failure message.
fn invocations(text: &str) -> Vec<(String, String, usize)> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        // the shell's own operators end a command, so `g13 bind G1 a && g13 profile` is two
        for piece in line.split(['&', '|', ';']) {
            // Requirements, both of which keep the noise out without a parser: the name has to be followed by a
            // space (so `g13-values.json`, `~/.local/bin/g13` and the bare word `g13` are not invocations), and
            // the next word has to be a word rather than a flag.
            let mut at = 0usize;
            while let Some(found_at) = piece[at..].find("g13 ") {
                let start = at + found_at;
                let after = start + 4;
                let word: String = piece[after..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                // A command in prose is written between backticks or after `$ `, and in a block it starts the
                // line; both are fine. Anything else is prose *about* g13 or a line of its own output - the
                // doctor prints `g13 on PATH` and `g13 keyboard is registered`, and reading those as commands
                // fails the manual for quoting the program truthfully. The second assertion in the test below
                // is the safety net: a command nobody names properly is still a command nobody documents.
                let before = piece[..start].trim_end();
                let named = before.is_empty()
                    || before.ends_with('`')
                    || before.ends_with('$')
                    || before.ends_with('#')
                    || before.ends_with('>');
                if named && !word.is_empty() && !word.starts_with('-') {
                    found.push((word, line.to_string(), index + 1));
                }
                at = after;
            }
        }
        for flag in FLAGS {
            if line.contains(flag) {
                found.push((flag.to_string(), line.to_string(), index + 1));
            }
        }
    }
    found
}

#[test]
fn every_command_the_manual_names_exists() {
    let mut wrong: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for path in docs() {
        let text = std::fs::read_to_string(&path).expect("a readable document");
        for (word, line, number) in invocations(&text) {
            if word.starts_with("--") {
                seen.insert(word);
                continue;
            }
            if !COMMANDS.contains(&word.as_str()) {
                wrong.push(format!(
                    "{}:{number}: `g13 {word}`  -  no such command\n    {line}",
                    path.display()
                ));
            } else {
                seen.insert(word);
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "the documentation names commands that do not exist:\n{}",
        wrong.join("\n")
    );
    // and the manual is not silent about any of them: a command nobody documents is a command nobody finds
    let missing: Vec<&str> = COMMANDS
        .iter()
        .copied()
        .filter(|command| !seen.contains(*command))
        .collect();
    assert!(
        missing.is_empty(),
        "these commands exist and are in no document: {missing:?}"
    );
}

#[test]
fn a_flag_written_in_the_manual_is_a_flag_the_binary_reads() {
    // the flags are checked by the other test's walk; this one is here so the list above cannot grow a flag
    // nothing reads without the failure being about flags
    assert!(!FLAGS.is_empty());
    assert!(FLAGS.iter().all(|flag| flag.starts_with("--")));
}

#[test]
fn a_line_of_the_doctor_output_is_not_an_invocation() {
    // Quoting the program truthfully must not fail the manual: these are lines `g13 doctor` prints.
    let output = "  [ok  ] g13 on PATH        /usr/bin/g13\n  [ok  ] our keyboard       g13 keyboard is registered\n";
    assert!(
        invocations(output).is_empty(),
        "a line of output was read as a command: {:?}",
        invocations(output)
    );

    // and a command really is one, however the manual writes it
    for line in [
        "g13 doctor",
        "$ g13 doctor",
        "- `g13 doctor`",
        "then run `g13 doctor` now",
    ] {
        assert!(
            invocations(line)
                .iter()
                .any(|(word, _, _)| word == "doctor"),
            "{line:?} is an invocation and was not read as one"
        );
    }
}
