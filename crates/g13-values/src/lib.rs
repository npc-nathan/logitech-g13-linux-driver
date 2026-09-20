//! A value, and the conditions that read them.
//!
//! A source produces a [`Value`]; a macro's question, an applet's alert and a menu item's condition all ask
//! about values with the language in [`cond`]. The two belong together - a condition with nothing to read is
//! not a language - and they belong *here* rather than in the crates that used to hold them, because of what
//! holding them cost: the value type lived in `g13-sources`, so every crate that read a configuration file
//! linked `ureq`, `tungstenite` and `rumqttc` to get a type that has nothing to do with the network.
//!
//! Nothing here touches a machine, a file or a network. `serde_json` is the whole of what it depends on,
//! because two of the conditions' written forms are JSON.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::print_stdout,
        clippy::print_stderr
    )
)]

pub mod cond;

pub use cond::{Compare, Cond, cond_to_json, day_number, holds, minutes, op_text, parse_cond};

/// What a source produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A string, as the source produced it.
    Text(String),
    /// A number, before a format decides how many places of it to show.
    Number(f64),
    /// No answer: the source produced nothing, or was not there. Its text is empty.
    Missing,
}

impl Value {
    /// The value as it should appear on the screen.
    pub fn text(&self) -> String {
        match self {
            Value::Text(text) => text.clone(),
            Value::Number(number) => {
                if number.fract() == 0.0 {
                    format!("{}", *number as i64)
                } else {
                    format!("{number}")
                }
            }
            Value::Missing => String::new(),
        }
    }

    /// The value as a number, for a bar or a comparison.
    pub fn number(&self) -> Option<f64> {
        match self {
            Value::Number(number) => Some(*number),
            Value::Text(text) => text.trim().parse().ok(),
            Value::Missing => None,
        }
    }

    /// Format it the way a widget's `format` asks: `{cpu}` or `{cpu:.0f}`.
    ///
    /// The spec arrives with everything after the colon, so `{cpu:.0f}` gives `.0f`. Both that form and the
    /// .NET-style `.0` are accepted, and anything else falls back to the value itself rather than failing.
    pub fn formatted(&self, spec: &str) -> String {
        let spec = spec.trim_start_matches(':');
        // padding, which the applets use to keep numbers in a column: >3 right-aligns in three characters
        if let Some(width) = spec
            .strip_prefix('>')
            .and_then(|rest| rest.parse::<usize>().ok())
        {
            return format!("{:>width$}", self.text());
        }
        if let Some(width) = spec
            .strip_prefix('<')
            .and_then(|rest| rest.parse::<usize>().ok())
        {
            return format!("{:<width$}", self.text());
        }
        let digits = spec.trim_start_matches('.').trim_end_matches('f');
        match self {
            Value::Number(number) => match digits {
                "0" => format!("{number:.0}"),
                "1" => format!("{number:.1}"),
                "2" => format!("{number:.2}"),
                _ => self.text(),
            },
            _ => self.text(),
        }
    }
}
