//! `g13 values`  -  what the running driver is doing, without reading a log.
//!
//! Reads the state file the driver publishes. The JSON has a fixed shape, so the fields are pulled out by
//! name rather than by adding a parser dependency.

use g13_agent::{now_ms, state_path};

/// `g13 values --catalogue`  -  every value this build publishes, what it means, and what it is now.
///
/// The same list the window's Values page shows, from the same place in the sources crate, so the two cannot
/// disagree about what exists. Anyone writing an applet can look here instead of reading the source.
pub fn catalogue() -> i32 {
    // written through a writer whose errors are ignored: `g13 values --catalogue | head` closes the pipe, and
    // that is a normal thing to do rather than a reason to panic
    use std::io::Write;
    let path = g13_agent::visuals::values_path();
    let published = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = std::io::stdout();
    let _ = writeln!(
        out,
        "every value this build publishes, from {}",
        path.display()
    );
    let _ = writeln!(out);
    let width = g13_sources::published_names()
        .iter()
        .map(|name| name.len())
        .max()
        .unwrap_or(0);
    for value in g13_sources::PUBLISHED {
        let now = match g13_sources::json_value(&published, value.name) {
            Some(found) => found.text(),
            None if value.computed => "(no driver running)".to_string(),
            None => "not published yet".to_string(),
        };
        let _ = writeln!(
            out,
            "  {:<width$}  {:<60}  {}",
            value.name,
            value.meaning,
            now,
            width = width
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "an applet uses one by naming it:  \"sources\": {{\"cpu\": \"cpu\"}}"
    );
    0
}

/// What the running driver is doing, read from its state file, or that file's own text with `--json`.
pub fn run(raw: bool) -> i32 {
    let path = state_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            println!("no driver state at {}", path.display());
            println!("  ({error})");
            println!(
                "\nnothing is publishing: start the driver with `g13 run`, or check `g13 doctor`."
            );
            return 1;
        }
    };

    if raw {
        println!("{text}");
        return 0;
    }

    let age_s = now_ms().saturating_sub(number(&text, "started_ms")) / 1000;
    println!("driver state, from {} ({age_s}s old)", path.display());
    println!("  pid         {}", number(&text, "pid"));
    println!("  version     {}", text_field(&text, "version"));
    println!(
        "  profile     {} (of the profiles present)",
        number(&text, "profile")
    );
    println!(
        "  bindings    {} of {} controls bound",
        number(&text, "applied"),
        number(&text, "controls")
    );

    let problems = array(&text, "problems");
    if problems.is_empty() {
        println!("  problems    none");
    } else {
        println!(
            "  problems    {} line(s) not used by this build:",
            problems.len()
        );
        for problem in problems.iter().take(8) {
            println!("                {problem}");
        }
    }

    let keys = array(&text, "last_keys");
    if keys.is_empty() {
        println!("  last keys   nothing pressed yet");
    } else {
        println!("  last keys   (oldest first)");
        for entry in keys.iter().rev().take(6).rev() {
            let control = text_field(entry, "control");
            let pressed = number(entry, "pressed") == 1;
            let at = number(entry, "at_ms");
            println!(
                "                {control:<8} {}  ({}s ago)",
                if pressed { "down" } else { "up" },
                now_ms().saturating_sub(at) / 1000
            );
        }
    }
    0
}

/// The value of a `"key": 12` field, or 0.
fn number(json: &str, key: &str) -> u64 {
    let needle = format!("\"{key}\":");
    let Some(start) = json.find(&needle) else {
        return 0;
    };
    let rest = &json[start + needle.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

/// The value of a `"key": "text"` field.
fn text_field(json: &str, key: &str) -> String {
    let needle = format!("\"{key}\":\"");
    let Some(start) = json.find(&needle) else {
        return String::new();
    };
    let rest = &json[start + needle.len()..];
    match rest.find('"') {
        Some(end) => rest[..end].to_string(),
        None => String::new(),
    }
}

/// The strings of a `"key": [ "a", "b" ]` array field.
fn array(json: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\":[");
    let Some(start) = json.find(&needle) else {
        return Vec::new();
    };
    let rest = &json[start + needle.len()..];
    let Some(end) = rest.find(']') else {
        return Vec::new();
    };
    let body = &rest[..end];
    if body.trim().is_empty() {
        return Vec::new();
    }
    // entries are either quoted strings or objects; split on the commas between top-level entries
    let mut entries = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for character in body.chars() {
        match character {
            '{' | '[' => {
                depth += 1;
                current.push(character);
            }
            '}' | ']' => {
                depth -= 1;
                current.push(character);
            }
            ',' if depth == 0 => {
                entries.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() {
        entries.push(current.trim().to_string());
    }
    entries
        .into_iter()
        .map(|entry| entry.trim_matches('"').to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"version":"0.1.0","pid":123,"started_ms":1000,"profile":1,"controls":38,"applied":36,"problems":["G29: not a control","nonsense: not a control"],"last_keys":[{"control":"G1","pressed":true,"at_ms":2000},{"control":"G1","pressed":false,"at_ms":2100}]}"#;

    #[test]
    fn fields_are_pulled_out_by_name() {
        assert_eq!(number(SAMPLE, "pid"), 123);
        assert_eq!(number(SAMPLE, "profile"), 1);
        assert_eq!(number(SAMPLE, "applied"), 36);
        assert_eq!(text_field(SAMPLE, "version"), "0.1.0");
    }

    #[test]
    fn arrays_are_split_at_the_right_level() {
        let problems = array(SAMPLE, "problems");
        assert_eq!(problems.len(), 2);
        assert_eq!(problems[0], "G29: not a control");
        let keys = array(SAMPLE, "last_keys");
        assert_eq!(keys.len(), 2);
        assert_eq!(text_field(&keys[0], "control"), "G1");
        assert_eq!(number(&keys[1], "pressed"), 0);
    }

    #[test]
    fn a_missing_field_is_not_a_panic() {
        assert_eq!(number("{}", "pid"), 0);
        assert_eq!(text_field("{}", "version"), "");
        assert!(array("{}", "problems").is_empty());
    }
}
