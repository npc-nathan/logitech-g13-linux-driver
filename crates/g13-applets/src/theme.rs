//! Themes: what an applet looks like when it is moving.
//!
//! A theme is a file, `themes/<name>.json`, and an applet or one of its screens names one. Nothing is built in:
//! a theme written and dropped in the folder works, and an applet with no theme behaves exactly as it did
//! before any of this existed.
//!
//! The panel is 160x43 in **one bit**, so a theme cannot be a palette. What it can be is motion, rhythm and
//! shape - which is how one-bit art has always worked, and exactly what the screen worker's fast draw is for.
//! Two of the effects are screen-level (a scanline sweeping, a glitch shift) and two are per-widget (a blink,
//! a typewriter reveal), so a theme says what the *rhythm* is and a widget says whether it is part of it.

use serde_json::Value;

/// What a theme asks the drawing to do. All of it is a function of the clock, so the same clock gives the same
/// picture - nothing here is random, and the drawing is redrawn twenty times a second while any of it is on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Theme {
    /// The name it was loaded from, so a problem can say which theme it is about.
    pub name: String,
    /// A one-pixel line sweeping down the screen: pixels a second. `None` is no scanline.
    pub scanline: Option<f64>,
    /// A shift of the whole picture every so many seconds, to a fault: `(every, pixels)`.
    pub glitch: Option<(f64, usize)>,
    /// The rhythm the widgets that blink share: `(on, off)` in seconds. A widget that blinks is drawn only
    /// during the on part, because in one bit a blink is ink appearing and disappearing.
    pub pulse: (f64, f64),
    /// How fast a widget that reveals itself types: characters a second.
    pub typewriter: f64,
    /// Boxes round widgets: `none`, `single` or `brackets`.
    pub frame: ThemeFrame,
}

/// How a theme boxes what it draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeFrame {
    #[default]
    /// No box round anything, which is how a screen looked before themes.
    None,
    /// A one-pixel outline round the place each widget takes.
    Single,
    /// A corner at each end of every side, the way a heads-up display boxes a reading.
    Brackets,
}

impl Theme {
    /// Read a theme out of its file's text.
    pub fn from_json(text: &str, name: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|error| format!("{error}"))?;
        let object = value
            .as_object()
            .ok_or_else(|| "a theme is an object of settings".to_string())?;
        let mut theme = Theme {
            name: name.to_string(),
            pulse: (1.0, 1.0),
            typewriter: 12.0,
            ..Theme::default()
        };
        for (key, value) in object {
            match key.as_str() {
                // the file may name itself; the folder already did, and the folder wins
                "name" | "title" => {}
                "frame" => {
                    theme.frame = match value.as_str() {
                        Some("none") | None => ThemeFrame::None,
                        Some("single") => ThemeFrame::Single,
                        Some("brackets") => ThemeFrame::Brackets,
                        Some(other) => {
                            return Err(format!(
                                "`frame` is none, single or brackets, not {other:?}"
                            ));
                        }
                    }
                }
                // `scanline: 60` is the short way, `scanline: {"speed": 60}` the long one
                "scanline" => {
                    theme.scanline = match value {
                        Value::Number(_) => value.as_f64().map(|speed| speed.clamp(1.0, 400.0)),
                        // no `speed` at all means no scanline: `{"scanline": {}}` turns a theme's own one off
                        Value::Object(settings) => settings
                            .get("speed")
                            .and_then(Value::as_f64)
                            .map(|speed| speed.clamp(1.0, 400.0)),
                        Value::Bool(false) | Value::Null => None,
                        other => {
                            return Err(format!(
                                "`scanline` is a speed in pixels a second or an object with `speed`, not {other}"
                            ));
                        }
                    }
                }
                // `glitch: {"every": 4, "shake": 1}`: every so many seconds, shift the picture this far
                "glitch" => {
                    let settings = value.as_object().ok_or_else(|| {
                        "`glitch` is an object with `every` seconds and `shake` pixels".to_string()
                    })?;
                    // `{}` - which is what the window writes when the last glitch setting is taken away - is no
                    // glitch at all, rather than a glitch on the defaults
                    if settings.is_empty() {
                        continue;
                    }
                    let every = settings
                        .get("every")
                        .and_then(Value::as_f64)
                        .unwrap_or(6.0)
                        .clamp(0.2, 600.0);
                    let shake = settings
                        .get("shake")
                        .and_then(Value::as_f64)
                        .unwrap_or(1.0)
                        .clamp(1.0, 20.0) as usize;
                    theme.glitch = Some((every, shake));
                }
                "pulse" => {
                    theme.pulse = match value {
                        // one number is the whole cycle, shared evenly
                        Value::Number(speed) => match speed.as_f64() {
                            Some(cycle) => (cycle / 2.0, cycle / 2.0),
                            None => (1.0, 1.0),
                        },
                        _ => {
                            let settings = value.as_object().ok_or_else(|| {
                                "`pulse` is a cycle in seconds or an object with `on` and `off`"
                                    .to_string()
                            })?;
                            let on = settings
                                .get("on")
                                .and_then(Value::as_f64)
                                .unwrap_or(1.0)
                                .clamp(0.05, 60.0);
                            let off = settings
                                .get("off")
                                .and_then(Value::as_f64)
                                .unwrap_or(1.0)
                                .clamp(0.05, 60.0);
                            (on, off)
                        }
                    }
                }
                "typewriter" => {
                    theme.typewriter = value
                        .as_f64()
                        .ok_or_else(|| "`typewriter` is characters a second".to_string())?
                        .clamp(0.5, 200.0);
                }
                // a theme for a widget kind this build does not draw yet: kept, and said, rather than refused
                other => {
                    return Err(format!(
                        "`{other}` is not something a theme can set in this build (it knows frame, scanline, glitch, pulse and typewriter)"
                    ));
                }
            }
        }
        Ok(theme)
    }

    /// Whether anything in this theme moves, so the screen showing it has to be drawn faster than it is read.
    pub fn moves(&self) -> bool {
        self.scanline.is_some() || self.glitch.is_some()
    }

    /// Whether a widget blinking is on at this moment. The phase comes from the clock, so two screens side by
    /// side blink together rather than each keeping its own idea of when a second started.
    pub fn lit(&self, now_seconds: f64) -> bool {
        let (on, off) = self.pulse;
        let cycle = (on + off).max(0.05);
        let at = now_seconds.max(0.0) % cycle;
        at < on
    }

    /// How far the picture is shifted at this moment, for a glitch. Nothing most of the time; this is why a
    /// theme with a glitch is drawn on its own clock rather than being nudged by a random number.
    pub fn shake(&self, now_seconds: f64) -> i64 {
        let Some((every, pixels)) = self.glitch else {
            return 0;
        };
        let at = now_seconds.max(0.0);
        // one frame in a second's worth of frames is shaken, decided by the clock: at 20 draws a second that is
        // the twentieth of a second after each `every`, which reads as a twitch rather than a wobble
        let into = at % every.max(0.2);
        if into < every / 20.0 {
            let direction = if (at / every) as i64 % 2 == 0 { 1 } else { -1 };
            return direction * pixels as i64;
        }
        0
    }
}

/// Every theme in a folder, by name, for a listing that shows what is there rather than what was expected.
///
/// One rule, the same shape as the applet listing: a file in this folder whose name ends `.json` is a theme. A
/// theme that will not read is listed with the reason rather than hidden, because a theme that silently does
/// nothing looks exactly like a theme that works and is subtle.
pub fn themes(dir: &std::path::Path) -> Vec<(String, Result<Theme, String>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        let read = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|text| Theme::from_json(&text, stem));
        found.push((stem.to_string(), read));
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
}

/// Load one theme by name, or say why not.
pub fn load(dir: &std::path::Path, name: &str) -> Result<Theme, String> {
    let path = dir.join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Theme::from_json(&text, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_reads_the_four_effects_and_refuses_what_it_does_not_know() {
        let theme = Theme::from_json(
            r#"{"name": "cyberpunk-2077", "frame": "brackets", "scanline": 60,
                "glitch": {"every": 3, "shake": 2}, "pulse": {"on": 0.4, "off": 0.6},
                "typewriter": 20}"#,
            "cyberpunk-2077",
        )
        .expect("a theme");
        assert_eq!(theme.name, "cyberpunk-2077");
        assert_eq!(theme.frame, ThemeFrame::Brackets);
        assert_eq!(theme.scanline, Some(60.0));
        assert_eq!(theme.glitch, Some((3.0, 2)));
        assert_eq!(theme.pulse, (0.4, 0.6));
        assert_eq!(theme.typewriter, 20.0);
        assert!(
            theme.moves(),
            "a scanline moves: the screen has to be drawn faster"
        );

        // one number for `pulse` is the whole cycle, shared evenly
        let short = Theme::from_json(r#"{"pulse": 2}"#, "short").unwrap();
        assert_eq!(short.pulse, (1.0, 1.0));
        // and no effects at all is the look everything had before themes
        let plain = Theme::from_json(r#"{}"#, "plain").unwrap();
        assert!(!plain.moves());
        assert_eq!(plain.frame, ThemeFrame::None);
        // a key this build does not know is said, not ignored: a theme that silently does half of what it says
        // is the worst thing to debug
        let wrong = Theme::from_json(r#"{"sparkle": true}"#, "wrong");
        assert!(wrong.unwrap_err().contains("sparkle"));
    }

    #[test]
    fn the_pulse_and_the_glitch_are_functions_of_the_clock() {
        // nothing here is random, so the same moment gives the same picture twice - which is what lets a
        // drawing be tested at all
        let theme = Theme::from_json(r#"{"pulse": {"on": 0.5, "off": 0.5}}"#, "p").unwrap();
        assert!(theme.lit(0.0), "the cycle starts lit");
        assert!(theme.lit(0.25));
        assert!(!theme.lit(0.6), "the off half of the cycle is off");
        assert!(theme.lit(1.1), "and it comes round");

        let glitchy = Theme::from_json(r#"{"glitch": {"every": 2, "shake": 1}}"#, "g").unwrap();
        assert_eq!(glitchy.shake(1.0), 0, "most of the time nothing is shaken");
        assert_ne!(glitchy.shake(0.01), 0, "just after each `every` it is");
        assert!(
            glitchy.shake(0.01).abs() <= 1 && glitchy.shake(2.01).abs() <= 1,
            "the shake is at most what the theme asked for"
        );
    }

    #[test]
    fn the_folder_lists_themes_by_one_rule() {
        let dir = std::env::temp_dir().join("g13-themes-listing");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("good.json"), r#"{"scanline": 40}"#).unwrap();
        std::fs::write(dir.join("broken.json"), "{not json").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();
        let listed = themes(&dir);
        let names: Vec<&str> = listed.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec!["broken", "good"],
            "one rule, and a broken one still listed"
        );
        assert!(listed[0].1.is_err(), "a theme that will not read says so");
        assert!(listed[1].1.is_ok());
        assert!(load(&dir, "good").is_ok());
        assert!(load(&dir, "nothing").unwrap_err().contains("cannot read"));
    }
}
