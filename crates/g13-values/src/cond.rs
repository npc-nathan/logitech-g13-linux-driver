//! The condition language: what a macro, an applet's alert or a menu can ask, and whether it holds.
//!
//! One language, four readers - a macro's `if`, an applet's alert, a menu item and the window's editors - and
//! **one evaluator** (`holds`). It used to live in `g13-config` beside the macro graph, which is where a
//! condition is most often written, but the language is not the graph's: an applet's alert holds one in its own
//! file, which is why `cond_to_json` exists beside `parse_cond`.
//!
//! A condition is asked about four things and nothing else: the control that fired it, the active profile, a
//! published value, and combinations of those.

use crate::Value;

/// A question a macro can ask.
#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    /// Was this macro fired by this control? The one thing only a pad can answer.
    Control(String),
    /// Is this profile the active one?
    Profile(u32),
    /// How a published value compares now.
    Value {
        /// The value to read, by the name its publisher or this graph's `sources` gives it.
        name: String,
        /// How the reading is compared.
        op: Compare,
        /// The amount to compare against, when the question is about an amount rather than words.
        number: Option<f64>,
        /// The words to compare against, when it is about words; the two are alternatives.
        text: Option<String>,
    },
    /// Every one of these.
    All(Vec<Cond>),
    /// Any one of these.
    Any(Vec<Cond>),
    /// Not this.
    Not(Box<Cond>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Which comparison a condition asks for.
pub enum Compare {
    /// Equal. Written `==`, or `=` as a person writing one by hand would.
    Eq,
    /// Not equal: `!=`.
    Ne,
    /// Less than: `<`.
    Lt,
    /// Less than or equal: `<=`.
    Le,
    /// Greater than: `>`.
    Gt,
    /// Greater than or equal: `>=`.
    Ge,
    /// The value has this text in it somewhere: `contains`.
    Contains,
}

impl Compare {
    /// The comparison a file's own spelling names, or none when the word is not a comparison at all.
    fn from_text(text: &str) -> Option<Self> {
        Some(match text {
            "==" | "=" => Compare::Eq,
            "!=" => Compare::Ne,
            "<" => Compare::Lt,
            "<=" => Compare::Le,
            ">" => Compare::Gt,
            ">=" => Compare::Ge,
            "contains" => Compare::Contains,
            _ => return None,
        })
    }
}
/// Write a condition back out as the JSON `parse_cond` reads.
///
/// The two belong together: one language, one pair of functions. An applet's alert holds a condition in its own
/// file, so it needs the way back that a macro - whose graph is its own line format - never did.
pub fn cond_to_json(cond: &Cond) -> serde_json::Value {
    match cond {
        Cond::Control(control) => serde_json::json!({ "control": control }),
        Cond::Profile(profile) => serde_json::json!({ "profile": profile }),
        Cond::Value {
            name,
            op,
            number,
            text,
        } => {
            let mut asked = serde_json::Map::new();
            asked.insert("value".to_string(), serde_json::json!(name));
            asked.insert("op".to_string(), serde_json::json!(op_text(*op)));
            match (number, text) {
                (Some(number), _) => {
                    asked.insert("to".to_string(), serde_json::json!(number));
                }
                (None, Some(text)) => {
                    asked.insert("text".to_string(), serde_json::json!(text));
                }
                (None, None) => {}
            }
            serde_json::Value::Object(asked)
        }
        Cond::All(parts) => serde_json::json!({
            "all": parts.iter().map(cond_to_json).collect::<Vec<_>>()
        }),
        Cond::Any(parts) => serde_json::json!({
            "any": parts.iter().map(cond_to_json).collect::<Vec<_>>()
        }),
        Cond::Not(inner) => serde_json::json!({ "not": cond_to_json(inner) }),
    }
}

/// The words a comparison is written with, which `parse_cond` reads back.
pub fn op_text(op: Compare) -> &'static str {
    match op {
        Compare::Eq => "==",
        Compare::Ne => "!=",
        Compare::Lt => "<",
        Compare::Le => "<=",
        Compare::Gt => ">",
        Compare::Ge => ">=",
        Compare::Contains => "contains",
    }
}

/// Read a condition out of its JSON, the shape a macro node holds.
///
/// Public because an applet's alert is a condition too, and it must be the *same* language: one parser, one
/// evaluator, never two that agree until they do not.
pub fn parse_cond(value: &serde_json::Value) -> Result<Cond, String> {
    if let Some(control) = value.get("control").and_then(|value| value.as_str()) {
        return Ok(Cond::Control(control.to_string()));
    }
    if let Some(profile) = value.get("profile").and_then(|value| value.as_u64()) {
        return Ok(Cond::Profile(profile as u32));
    }
    if let Some(value_of) = value.get("value").and_then(|value| value.as_str()) {
        let op_text = value.get("op").and_then(|op| op.as_str()).unwrap_or("==");
        let op = Compare::from_text(op_text).ok_or_else(|| {
            format!("\"op\" is {op_text}; it can be ==, !=, <, <=, >, >= or contains")
        })?;
        let number = value
            .get("to")
            .and_then(|to| to.as_f64())
            .or_else(|| value.get("number").and_then(|number| number.as_f64()));
        let text = value
            .get("text")
            .and_then(|text| text.as_str())
            .map(|text| text.to_string());
        if number.is_none() && text.is_none() {
            return Err(format!(
                "value {value_of} is compared with nothing: give \"to\" a number or \"text\" a string"
            ));
        }
        return Ok(Cond::Value {
            name: value_of.to_string(),
            op,
            number,
            text,
        });
    }
    for (key, join) in [("all", 0), ("any", 1)] {
        if let Some(list) = value.get(key).and_then(|list| list.as_array()) {
            let mut parts = Vec::new();
            for item in list {
                parts.push(parse_cond(item)?);
            }
            if parts.is_empty() {
                return Err(format!("\"{key}\" is empty, so it asks nothing"));
            }
            return Ok(if join == 0 {
                Cond::All(parts)
            } else {
                Cond::Any(parts)
            });
        }
    }
    if let Some(inner) = value.get("not") {
        return Ok(Cond::Not(Box::new(parse_cond(inner)?)));
    }
    Err(format!(
        "{value} is not a question: it needs control, profile, value, all, any or not"
    ))
}
/// An ISO 8601 date as a day number, for comparing: `2026-09-18`.
///
/// The year is required. `18 Sep` has none, so "is 31 December after 1 January" has no answer from that string -
/// which is exactly why `date_iso` exists. The conversion is the standard one (days since 1970-01-01), so the
/// ordering is a real calendar's: leap years and month lengths included, not an approximation.
///
/// Anything else - a bare month name, a date with a clock on the end, a month or day out of range - is not a date
/// this will order.
pub fn day_number(text: &str) -> Option<i64> {
    let text = text.trim();
    let mut parts = text.split('-');
    let year: i64 = parts.next()?.trim().parse().ok()?;
    let month: i64 = parts.next()?.trim().parse().ok()?;
    let day: i64 = parts.next()?.trim().parse().ok()?;
    // a fourth part means this is an instant, which is a different question from a day
    if parts.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year_adjusted = if month <= 2 { year - 1 } else { year };
    let era = if year_adjusted >= 0 {
        year_adjusted
    } else {
        year_adjusted - 399
    } / 400;
    let year_of_era = year_adjusted - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146097 + day_of_era - 719468)
}
/// A clock reading, as minutes since midnight, or nothing if it is not one.
///
/// This project publishes the clock as text - `time` is "10:54" - and a question about it is an ordering like
/// any other. Ordering the text gives nonsense: "10:54" sorts before "6:00", because '1' comes before '6'. So
/// when both sides read as a time, they are compared as times.
///
/// Accepts what a person writes: "6:00", "06:00", "19:30", and "10:54:07" (the seconds are dropped, since
/// nothing here asks a question finer than a minute).
pub fn minutes(text: &str) -> Option<u32> {
    let text = text.trim();
    let (hours, rest) = text.split_once(':')?;
    let hours: u32 = hours.trim().parse().ok()?;
    if hours > 23 {
        return None;
    }
    let rest = rest.trim();
    let minute_part = rest.split_once(':').map(|(m, _)| m).unwrap_or(rest);
    let minutes: u32 = minute_part.trim().parse().ok()?;
    if minutes > 59 {
        return None;
    }
    Some(hours * 60 + minutes)
}
/// Whether a question is true, given the four things it can be asked about.
///
/// The one implementation. A macro fills `control` with the button that fired it and an applet fills it with
/// nothing; everything else is the same question about the same values.
pub fn holds(
    condition: &Cond,
    values: &std::collections::BTreeMap<String, Value>,
    profile: u32,
    control: Option<&str>,
) -> bool {
    match condition {
        Cond::Control(wanted) => control
            .map(|control| control.eq_ignore_ascii_case(wanted))
            .unwrap_or(false),
        Cond::Profile(wanted) => profile == *wanted,
        Cond::Value {
            name,
            op,
            number,
            text,
        } => {
            let Some(value) = values.get(name) else {
                // a value nobody has published is not zero: it is unknown, and unknown is not a comparison
                return false;
            };
            match (number, text) {
                (Some(wanted), _) => {
                    let got = match value {
                        Value::Number(got) => *got,
                        // a value that is not a number cannot be more or less than one, but it can be
                        // "the same as" if the two read alike
                        other => match other.text().parse::<f64>() {
                            Ok(got) => got,
                            Err(_) => return false,
                        },
                    };
                    match op {
                        Compare::Eq => (got - wanted).abs() < f64::EPSILON,
                        Compare::Ne => (got - wanted).abs() >= f64::EPSILON,
                        Compare::Lt => got < *wanted,
                        Compare::Le => got <= *wanted,
                        Compare::Gt => got > *wanted,
                        Compare::Ge => got >= *wanted,
                        // ordering text is not a comparison this makes, and saying so would be noise
                        Compare::Contains => false,
                    }
                }
                (None, Some(wanted)) => {
                    let got = value.text();
                    // Two clock readings are ordered as times. Without this every ordering against a text value
                    // was false, so "the time is before six" could never hold however early it was.
                    if let (Some(got_at), Some(wanted_at)) = (minutes(&got), minutes(wanted)) {
                        return match op {
                            Compare::Eq => got_at == wanted_at,
                            Compare::Ne => got_at != wanted_at,
                            Compare::Lt => got_at < wanted_at,
                            Compare::Le => got_at <= wanted_at,
                            Compare::Gt => got_at > wanted_at,
                            Compare::Ge => got_at >= wanted_at,
                            Compare::Contains => got.contains(wanted.as_str()),
                        };
                    }
                    // Two ISO dates are ordered as dates, which is what `date_iso` is for.
                    if let (Some(got_at), Some(wanted_at)) = (day_number(&got), day_number(wanted))
                    {
                        return match op {
                            Compare::Eq => got_at == wanted_at,
                            Compare::Ne => got_at != wanted_at,
                            Compare::Lt => got_at < wanted_at,
                            Compare::Le => got_at <= wanted_at,
                            Compare::Gt => got_at > wanted_at,
                            Compare::Ge => got_at >= wanted_at,
                            Compare::Contains => got.contains(wanted.as_str()),
                        };
                    }
                    match op {
                        Compare::Eq => got == *wanted,
                        Compare::Ne => got != *wanted,
                        Compare::Contains => got.contains(wanted.as_str()),
                        // Ordering text that is neither a time nor a date is not a comparison this makes: "is the
                        // title after this string" has no answer worth inventing. `date` is one of these - it has
                        // no year, so ordering it could not answer a question that crosses a new year.
                        Compare::Lt | Compare::Le | Compare::Gt | Compare::Ge => false,
                    }
                }
                (None, None) => false,
            }
        }
        Cond::All(parts) => parts
            .iter()
            .all(|part| holds(part, values, profile, control)),
        Cond::Any(parts) => parts
            .iter()
            .any(|part| holds(part, values, profile, control)),
        Cond::Not(inner) => !holds(inner, values, profile, control),
    }
}
#[cfg(test)]
mod holds_tests {
    use super::*;
    use Value;
    use std::collections::BTreeMap;

    fn values() -> BTreeMap<String, Value> {
        let mut values = BTreeMap::new();
        values.insert("cpu".to_string(), Value::Number(87.0));
        values.insert("title".to_string(), Value::Text("now playing".to_string()));
        values
    }

    #[test]
    fn a_question_is_answered_the_same_way_a_macro_would_answer_it() {
        // the one implementation: an applet's alert and a macro's `if` come here, so this is the only place the
        // answer can be wrong
        let asked = parse_cond(&serde_json::json!({"value": "cpu", "op": ">", "to": 80})).unwrap();
        assert!(holds(&asked, &values(), 2, None));
        let too_high =
            parse_cond(&serde_json::json!({"value": "cpu", "op": ">", "to": 90})).unwrap();
        assert!(!holds(&too_high, &values(), 2, None));

        // text, and the comparison that only makes sense for text
        let text =
            parse_cond(&serde_json::json!({"value": "title", "op": "contains", "text": "playing"}))
                .unwrap();
        assert!(holds(&text, &values(), 2, None));

        // joins, and a `not`
        let both = parse_cond(&serde_json::json!({"all": [
            {"value": "cpu", "op": ">", "to": 80},
            {"value": "title", "op": "contains", "text": "playing"}
        ]}))
        .unwrap();
        assert!(holds(&both, &values(), 2, None));
        let either = parse_cond(&serde_json::json!({"any": [
            {"value": "cpu", "op": "<", "to": 10},
            {"value": "cpu", "op": ">", "to": 80}
        ]}))
        .unwrap();
        assert!(holds(&either, &values(), 2, None));
        let neither =
            parse_cond(&serde_json::json!({"not": {"value": "cpu", "op": ">", "to": 80}})).unwrap();
        assert!(!holds(&neither, &values(), 2, None));

        // a control, which an applet never fills: it is a question only a pad can answer
        let control = parse_cond(&serde_json::json!({"control": "G7"})).unwrap();
        assert!(!holds(&control, &values(), 2, None), "no pad, no answer");
        assert!(
            holds(&control, &values(), 2, Some("g7")),
            "and it never cares about case"
        );

        // the profile, which an applet does fill
        let profile = parse_cond(&serde_json::json!({"profile": 2})).unwrap();
        assert!(holds(&profile, &values(), 2, None));
        assert!(!holds(&profile, &values(), 0, None));

        // a value nobody published is unknown, not zero
        let unknown =
            parse_cond(&serde_json::json!({"value": "nothing", "op": "<", "to": 10})).unwrap();
        assert!(!holds(&unknown, &values(), 2, None));
    }
}

#[cfg(test)]
mod cond_json_tests {
    use super::*;

    #[test]
    fn a_condition_written_out_reads_back_as_itself() {
        // one language means the way out and the way in have to agree, and the only way to know is to go round
        let written = [
            serde_json::json!({"value": "cpu", "op": ">", "to": 80}),
            serde_json::json!({"value": "title", "op": "contains", "text": "playing"}),
            serde_json::json!({"profile": 2}),
            serde_json::json!({"control": "G7"}),
            serde_json::json!({"all": [{"value": "cpu", "op": ">", "to": 80}, {"profile": 0}]}),
            serde_json::json!({"any": [{"value": "cpu", "op": "<", "to": 10}, {"value": "ram", "op": ">=", "to": 90}]}),
            serde_json::json!({"not": {"value": "cpu", "op": "!=", "to": 0}}),
        ];
        for json in written {
            let cond = parse_cond(&json).expect("a condition");
            let back = cond_to_json(&cond);
            let again = parse_cond(&back).expect("and back again");
            assert_eq!(
                cond, again,
                "did not survive being written: {json} became {back}"
            );
        }
    }
}

#[cfg(test)]
mod the_clock_is_read_as_a_time {
    use super::*;
    use Value;
    use std::collections::BTreeMap;

    fn at(clock: &str) -> BTreeMap<String, Value> {
        let mut values = BTreeMap::new();
        values.insert("time".to_string(), Value::Text(clock.to_string()));
        values
    }

    fn asks(op: Compare, against: &str) -> Cond {
        Cond::Value {
            name: "time".to_string(),
            op,
            number: None,
            text: Some(against.to_string()),
        }
    }

    #[test]
    fn a_clock_reading_is_parsed_however_it_is_written() {
        assert_eq!(minutes("6:00"), Some(360));
        assert_eq!(minutes("06:00"), Some(360));
        assert_eq!(minutes("19:30"), Some(1170));
        assert_eq!(minutes("10:54:07"), Some(654), "the seconds are dropped");
        assert_eq!(minutes(" 8:05 "), Some(485), "spaces around it are fine");
        // and what is not a clock reading is not one
        assert_eq!(minutes("25:00"), None);
        assert_eq!(minutes("10:70"), None);
        assert_eq!(minutes("Clear"), None);
        assert_eq!(minutes(""), None);
    }

    /// The fault: every ordering against a text value was false, so this could never hold however early it was.
    #[test]
    fn before_six_in_the_morning_holds_in_the_morning() {
        assert!(holds(&asks(Compare::Le, "6:00"), &at("05:30"), 0, None));
        assert!(
            holds(&asks(Compare::Le, "6:00"), &at("6:00"), 0, None),
            "the boundary is in"
        );
        assert!(!holds(&asks(Compare::Le, "6:00"), &at("10:54"), 0, None));
        // and the one naive text ordering gets wrong: "10:54" sorts before "6:00" as text
        assert!(!holds(&asks(Compare::Le, "6:00"), &at("10:54"), 0, None));
    }

    #[test]
    fn after_seven_in_the_evening_holds_in_the_evening() {
        assert!(holds(&asks(Compare::Gt, "19:00"), &at("22:30"), 0, None));
        assert!(holds(&asks(Compare::Gt, "19:00"), &at("19:01"), 0, None));
        assert!(
            !holds(&asks(Compare::Gt, "19:00"), &at("19:00"), 0, None),
            "not after, at"
        );
        assert!(!holds(&asks(Compare::Gt, "19:00"), &at("10:54"), 0, None));
    }

    /// The window a person actually wants at night, which is one or the other - written as `any of`.
    #[test]
    fn a_night_window_is_any_of_the_two_ends() {
        let night = Cond::Any(vec![asks(Compare::Le, "6:00"), asks(Compare::Gt, "19:00")]);
        assert!(holds(&night, &at("05:30"), 0, None), "early morning is in");
        assert!(holds(&night, &at("22:30"), 0, None), "late evening is in");
        assert!(!holds(&night, &at("12:00"), 0, None), "midday is out");
    }

    /// And the same two conditions with `all of`, which no clock reading can satisfy. Kept as a test so the
    /// shape of the mistake is written down rather than discovered again.
    #[test]
    fn the_same_two_ends_with_all_of_can_never_hold() {
        let impossible = Cond::All(vec![asks(Compare::Le, "6:00"), asks(Compare::Gt, "19:00")]);
        for clock in [
            "00:00", "05:30", "06:00", "12:00", "19:00", "19:01", "23:59",
        ] {
            assert!(
                !holds(&impossible, &at(clock), 0, None),
                "{clock} satisfied both ends at once, which is not a thing"
            );
        }
    }

    #[test]
    fn equality_ignores_how_the_clock_was_written() {
        assert!(holds(&asks(Compare::Eq, "6:00"), &at("06:00"), 0, None));
        assert!(holds(&asks(Compare::Ne, "6:00"), &at("07:00"), 0, None));
    }

    /// Text that is not a time keeps the behaviour it had: equality and contains, and no invented ordering.
    #[test]
    fn text_that_is_not_a_clock_reading_is_left_alone() {
        let mut values = BTreeMap::new();
        values.insert("title".to_string(), Value::Text("Clear Skies".to_string()));
        let contains = Cond::Value {
            name: "title".to_string(),
            op: Compare::Contains,
            number: None,
            text: Some("Clear".to_string()),
        };
        assert!(holds(&contains, &values, 0, None));
        let after = Cond::Value {
            name: "title".to_string(),
            op: Compare::Gt,
            number: None,
            text: Some("abc".to_string()),
        };
        assert!(
            !holds(&after, &values, 0, None),
            "ordering words is not a comparison"
        );
    }
}

#[cfg(test)]
mod a_date_is_read_as_a_date {
    use super::*;
    use Value;
    use std::collections::BTreeMap;

    fn holding(name: &str, text: &str) -> BTreeMap<String, Value> {
        let mut values = BTreeMap::new();
        values.insert(name.to_string(), Value::Text(text.to_string()));
        values
    }

    fn asks(name: &str, op: Compare, against: &str) -> Cond {
        Cond::Value {
            name: name.to_string(),
            op,
            number: None,
            text: Some(against.to_string()),
        }
    }

    #[test]
    fn an_iso_date_is_parsed_and_ordered() {
        assert!(day_number("2026-09-18").is_some());
        assert!(day_number("2026-09-18") > day_number("2026-09-01"));
        assert!(
            day_number("2027-01-01") > day_number("2026-12-31"),
            "a new year is a day later"
        );
        assert!(day_number("2026-03-01") > day_number("2026-02-28"));
        assert!(
            day_number("2024-03-01") > day_number("2024-02-29"),
            "a leap day is a real day"
        );
        // a month or day out of range, a bare month, an instant: none of those is a date
        assert_eq!(day_number("2026-13-01"), None);
        assert_eq!(day_number("2026-09-32"), None);
        assert_eq!(day_number("18 Sep"), None);
        assert_eq!(day_number("2026-09-18T10:58"), None);
    }

    #[test]
    fn a_date_condition_holds_the_way_it_reads() {
        let values = holding("date_iso", "2026-09-18");
        assert!(holds(
            &asks("date_iso", Compare::Ge, "2026-09-01"),
            &values,
            0,
            None
        ));
        assert!(!holds(
            &asks("date_iso", Compare::Ge, "2026-10-01"),
            &values,
            0,
            None
        ));
        assert!(holds(
            &asks("date_iso", Compare::Eq, "2026-09-18"),
            &values,
            0,
            None
        ));
        assert!(holds(
            &asks("date_iso", Compare::Lt, "2026-12-31"),
            &values,
            0,
            None
        ));
        // and the case the display form cannot answer: is today after the end of last year
        let across = holding("date_iso", "2026-01-02");
        assert!(holds(
            &asks("date_iso", Compare::Gt, "2025-12-31"),
            &across,
            0,
            None
        ));
    }

    /// `date` is display text with no year, and ordering it could not answer a question that crosses a new year.
    /// Kept as a test so the reason is written down, and so it does not quietly start being orderable.
    #[test]
    fn the_screen_form_of_a_date_is_not_ordered() {
        let values = holding("date", "18 Sep");
        assert!(!holds(
            &asks("date", Compare::Gt, "17 Sep"),
            &values,
            0,
            None
        ));
        assert!(!holds(
            &asks("date", Compare::Le, "31 Dec"),
            &values,
            0,
            None
        ));
        // what it does do is read, which is what it is for
        assert!(holds(
            &asks("date", Compare::Contains, "Sep"),
            &values,
            0,
            None
        ));
        assert!(holds(
            &asks("date", Compare::Eq, "18 Sep"),
            &values,
            0,
            None
        ));
    }
}
