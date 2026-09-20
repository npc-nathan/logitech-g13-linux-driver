//! What the pad is doing, read from the driver that is running.
//!
//! The window cannot open the pad for itself - a driver holds it, and only one thing can - so this reads what
//! that driver publishes. It is the difference between "the pad does nothing" and being able to watch a
//! control, or the stick, and see exactly what the driver is receiving.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// A reading of the driver's published values.
#[derive(Debug, Clone, Default)]
pub struct Live {
    /// Every value the driver published, as text.
    pub values: BTreeMap<String, String>,
    /// The ones that are numbers, so a reading can be used without parsing it again.
    pub numbers: BTreeMap<String, f64>,
    /// How long ago the driver wrote this reading.
    pub read_at: Option<Duration>,
    /// The file this was read from - what the driver writes its values to.
    pub path: PathBuf,
}

impl Live {
    /// Read what the driver publishes, or nothing at all when there is no file yet.
    pub fn load() -> Self {
        let path = g13_agent::visuals::values_path();
        let mut live = Live {
            path: path.clone(),
            ..Default::default()
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return live;
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) else {
            return live;
        };
        if let Some(object) = parsed.as_object() {
            for (key, value) in object {
                match value {
                    serde_json::Value::String(text) => {
                        live.values.insert(key.clone(), text.clone());
                    }
                    serde_json::Value::Number(number) => {
                        if let Some(number) = number.as_f64() {
                            live.numbers.insert(key.clone(), number);
                        }
                    }
                    other => {
                        live.values.insert(key.clone(), other.to_string());
                    }
                }
            }
        }
        live.read_at = std::fs::metadata(&path)
            .and_then(|details| details.modified())
            .ok()
            .and_then(|modified| std::time::SystemTime::now().duration_since(modified).ok());
        live
    }

    /// How long ago this reading was written by the driver.
    pub fn age(&self) -> Option<Duration> {
        self.read_at
    }

    /// The stick, as the two raw bytes the driver reports, and whether they look like a signed pair.
    ///
    /// The bytes are shown as they are: their meaning has not been established, and this view exists to
    /// watch them while the stick is moved, which is how it will be.
    pub fn stick(&self) -> Option<(u8, u8)> {
        let raw = self.values.get("stick_raw")?;
        let mut parts = raw.split_whitespace();
        let first = parts.next()?.trim_start_matches("0x");
        let second = parts.next()?.trim_start_matches("0x");
        Some((
            u8::from_str_radix(first, 16).ok()?,
            u8::from_str_radix(second, 16).ok()?,
        ))
    }

    /// One published value as text, or empty when the driver has not published it.
    pub fn text(&self, key: &str) -> String {
        self.values.get(key).cloned().unwrap_or_default()
    }

    /// One published value as a number, or none when it was not published or is not one.
    pub fn number(&self, key: &str) -> Option<f64> {
        self.numbers.get(key).copied()
    }

    /// The controls the driver reported pressed, most recent last.
    pub fn recent_keys(&self) -> Vec<String> {
        self.text("recent_keys")
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_read_is_not_an_error() {
        let live = Live {
            path: PathBuf::from("/nonexistent/g13-values.json"),
            ..Default::default()
        };
        assert!(live.stick().is_none());
        assert_eq!(live.text("profile"), "");
        assert!(live.age().is_none());
    }

    #[test]
    fn the_stick_comes_back_as_the_two_bytes_it_was_given() {
        let mut live = Live::default();
        live.values.insert("stick_raw".into(), "0x7b 0x34".into());
        assert_eq!(live.stick(), Some((0x7b, 0x34)));
        // an empty value is not a reading of anything
        live.values.insert("stick_raw".into(), String::new());
        assert_eq!(live.stick(), None);
    }

    #[test]
    fn the_recent_keys_split_the_way_the_driver_writes_them() {
        let mut live = Live::default();
        live.values
            .insert("recent_keys".into(), "G1- G2+ G1+".into());
        assert_eq!(live.recent_keys(), vec!["G1-", "G2+", "G1+"]);
    }
}
