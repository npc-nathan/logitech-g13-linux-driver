//! Reading an applet's JSON.
//!
//! JSON is parsed as JSON. A file whose shape is not understood is reported rather than half-read, and a
//! widget type this build does not know is listed rather than dropped.

use crate::Screen;
use crate::{
    Alert, AlertLook, Align, Animate, Applet, FOLLOW_KEY, FONT_KEY, Follow, THEME_KEY, Theme,
    Widget,
};
use serde_json::Value as Json;
use std::collections::BTreeMap;

/// Read the `follow` key: absent (or `false`) is "never takes the screen by itself", and everything else is a
/// window with a length of time in it.
fn follow_in(object: &serde_json::Map<String, Json>) -> Option<Follow> {
    match object.get(FOLLOW_KEY)? {
        Json::Bool(true) => Some(Follow {
            seconds: Follow::DEFAULT_SECONDS,
            profile: None,
        }),
        Json::Object(fields) => Some(Follow {
            seconds: fields
                .get("seconds")
                .and_then(Json::as_f64)
                .unwrap_or(Follow::DEFAULT_SECONDS),
            profile: fields
                .get("profile")
                .and_then(Json::as_str)
                .map(str::to_string),
        }),
        other => other.as_f64().map(|seconds| Follow {
            seconds,
            profile: None,
        }),
    }
}

/// The widgets in one array of an applet's file, saying which of them this build does not draw.
///
/// Used for the applet's own `widgets` and for each of its `screens`, because they are the same shape and two
/// readers of one format is how a screen comes to accept something the applet does not.
fn widgets_in(
    object: &serde_json::Map<String, Json>,
    key: &str,
    unhandled: &mut Vec<String>,
    animations: &mut BTreeMap<usize, Animate>,
    alerts: &mut BTreeMap<usize, Alert>,
    commands: &mut BTreeMap<usize, Vec<String>>,
) -> Result<Vec<Widget>, String> {
    let mut widgets = Vec::new();
    for (index, widget) in object
        .get(key)
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let kind = widget
            .get("type")
            .and_then(Json::as_str)
            .unwrap_or("with no type")
            .to_string();
        match parse_widget(widget) {
            Ok(Some(parsed)) => {
                // What this widget does when it is chosen: one action written plainly, or a list of them, because
                // "play this and go back to the playing screen" is two things and one press.
                match widget.get("command") {
                    None => {}
                    Some(Json::String(one)) => {
                        commands.insert(index, vec![one.clone()]);
                    }
                    Some(Json::Array(list)) => {
                        let mut several = Vec::new();
                        for (at, one) in list.iter().enumerate() {
                            match one.as_str() {
                                Some(one) => several.push(one.to_string()),
                                None => unhandled.push(format!(
                                    "widget {index}: command {at} is not a string, and every action in a list is one"
                                )),
                            }
                        }
                        commands.insert(index, several);
                    }
                    Some(other) => unhandled.push(format!(
                        "widget {index}: `command` is a string or a list of them, not {other}"
                    )),
                }
                // what this widget does when it moves, if it does: one word, and the theme says how
                // an alert: the question, and what to do while the answer is yes. The condition is read by the
                // macros' own parser, because there is one language for "when is this true" in this program.
                if let Some(alert) = widget.get("alert") {
                    match alert.get("when") {
                        None => unhandled.push(format!(
                            "widget {index}: an alert needs `when`: the question it asks, as a macro's condition"
                        )),
                        Some(when) => match g13_values::parse_cond(when) {
                            Ok(condition) => {
                                let look = alert
                                    .get("look")
                                    .and_then(Json::as_str)
                                    .and_then(AlertLook::from_word)
                                    .unwrap_or(AlertLook::Flash);
                                // Keys an alert used to take and no longer does are *said*, not swallowed: a
                                // file that still carries `"bitmap": "l"` looks like it draws something, and a
                                // silent key is the kind of thing that wastes an afternoon.
                                for (key, _) in alert.as_object().into_iter().flatten() {
                                    if !matches!(key.as_str(), "when" | "look") {
                                        unhandled.push(format!(
                                            "widget {index}: an alert has no `{key}`; `show` draws the widget \
                                             itself, so there is no picture to name"
                                        ));
                                    }
                                }
                                // `show` is the widget's own drawing shown only while the question is true, so
                                // there is no picture to name: several widgets in one place take turns
                                alerts.insert(index, Alert { condition, look });
                            }
                            Err(problem) => {
                                unhandled.push(format!("widget {index}: alert {problem}"));
                            }
                        },
                    }
                }
                if let Some(word) = widget.get("animate").and_then(Json::as_str) {
                    match Animate::from_word(word) {
                        Some(animate) => {
                            animations.insert(index, animate);
                        }
                        None => unhandled.push(format!(
                            "widget {index}: `animate` is one of {:?}, not {word:?}",
                            Animate::WORDS
                        )),
                    }
                }
                widgets.push(parsed);
            }
            // say which type, so "unknown" is not the whole story
            Ok(None) => unhandled.push(format!(
                "widget {index}: {kind} is not a widget type this build draws"
            )),
            Err(problem) => return Err(format!("widget {index}: {problem}")),
        }
    }
    Ok(widgets)
}

/// Read an applet from its file text.
pub fn parse_applet(text: &str) -> Result<Applet, String> {
    let root: Json =
        serde_json::from_str(text).map_err(|error| format!("not valid JSON: {error}"))?;
    let object = root
        .as_object()
        .ok_or_else(|| "an applet must be a JSON object".to_string())?;

    let name = object
        .get("name")
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| "an applet needs a name".to_string())?;

    let mut sources = BTreeMap::new();
    if let Some(map) = object.get("sources").and_then(Json::as_object) {
        for (key, value) in map {
            match value.as_str() {
                Some(source) => {
                    sources.insert(key.clone(), source.to_string());
                }
                None => {
                    return Err(format!("source {key} must be written as text"));
                }
            }
        }
    }

    let mut unhandled = Vec::new();
    let mut animations: Vec<BTreeMap<usize, Animate>> = vec![BTreeMap::new()];
    let mut alerts: Vec<BTreeMap<usize, Alert>> = vec![BTreeMap::new()];
    let mut commands: Vec<BTreeMap<usize, Vec<String>>> = vec![BTreeMap::new()];
    let widgets = widgets_in(
        object,
        "widgets",
        &mut unhandled,
        &mut animations[0],
        &mut alerts[0],
        &mut commands[0],
    )?;

    // A screen is the same shape one level down: a title and its own widgets. The sources stay the applet's,
    // because a screen is a view of the same readings rather than a different set of them.
    let mut screens: Vec<Screen> = Vec::new();
    for (index, screen) in object
        .get("screens")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(screen) = screen.as_object() else {
            return Err(format!("screen {index} must be a JSON object"));
        };
        let title = screen
            .get("title")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let mut complaints = Vec::new();
        let mut its = BTreeMap::new();
        let mut its_alerts = BTreeMap::new();
        let mut its_commands = BTreeMap::new();
        let widgets = widgets_in(
            screen,
            "widgets",
            &mut complaints,
            &mut its,
            &mut its_alerts,
            &mut its_commands,
        )?;
        alerts.push(its_alerts);
        commands.push(its_commands);
        animations.push(its);
        for complaint in complaints {
            // say which screen, or "widget 2" is a search through the whole file
            unhandled.push(format!("screen {index}: {complaint}"));
        }
        screens.push(Screen { title, widgets });
    }
    if !screens.is_empty() && !widgets.is_empty() {
        // Both is ambiguous, and guessing would be the editor deciding what the file means
        unhandled.push(
            "this applet has both `screens` and top-level `widgets`: the screens are what is drawn, and the \
             top-level widgets are ignored"
                .to_string(),
        );
    }

    // any key this build has never heard of
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "name"
                | "title"
                | "interval"
                | "border"
                | "sources"
                | "widgets"
                | "screens"
                | "theme"
                | "font"
                | "follow"
        ) {
            unhandled.push(format!("{key} is not a key this build knows"));
        }
    }

    Ok(Applet {
        follow: follow_in(object),
        title: object
            .get("title")
            .and_then(Json::as_str)
            .unwrap_or(&name)
            .to_string(),
        interval: object.get("interval").and_then(Json::as_f64).unwrap_or(1.0),
        border: object
            .get("border")
            .and_then(Json::as_bool)
            .unwrap_or(false),
        name,
        sources,
        widgets,
        screens,
        bitmaps: BTreeMap::new(),
        // read here, loaded from the folder by `with_theme`: this file has the text, not its neighbours
        theme_name: object
            .get(THEME_KEY)
            .and_then(Json::as_str)
            .map(str::to_string),
        theme: Theme::default(),
        // read here, loaded from the folder by `with_font`, for the same reason as the theme above
        font_name: object
            .get(FONT_KEY)
            .and_then(Json::as_str)
            .map(str::to_string),
        font: None,
        // every other font in the folder, put on by `with_font`: the parser has the file's text, not the folder
        fallbacks: Vec::new(),
        animations,
        alerts,
        commands,
        unhandled,
    })
}

/// One widget from an applet file as its typed shape, or none when the type is not one this build draws.
fn parse_widget(widget: &Json) -> Result<Option<Widget>, String> {
    let kind = widget
        .get("type")
        .and_then(Json::as_str)
        .ok_or_else(|| "a widget needs a type".to_string())?;
    let number = |key: &str| widget.get(key).and_then(Json::as_f64);
    let count = |key: &str| number(key).unwrap_or(0.0).max(0.0) as usize;
    let text = |key: &str| {
        widget
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string()
    };

    Ok(Some(
        (match kind {
            "text" => {
                let align = Align::parse(&text("align"));
                // a widget with no x is anchored by its alignment: right-aligned text with no x ends at the last
                // column inside the border, which is how the HUD applet writes its level readout. The border
                // column itself is left alone, or the text lands on it and four pixels go missing.
                Widget::Text {
                    x: widget
                        .get("x")
                        .and_then(Json::as_f64)
                        .map(|value| value.max(0.0) as usize)
                        .unwrap_or(match align {
                            Align::Left => 0,
                            Align::Centre => g13_screen::WIDTH / 2,
                            Align::Right => g13_screen::WIDTH - 1,
                        }),
                    y: count("y"),
                    align,
                    format: text("format"),
                    scroll_width: widget
                        .get("scroll_width")
                        .and_then(Json::as_f64)
                        .map(|w| w.max(0.0) as usize),
                    // the previous stack's names for the same thing, now acted on: `scroll` runs the text through
                    // `scroll_width` instead of cutting it, `scroll_speed` is pixels a second
                    scroll: widget
                        .get("scroll")
                        .and_then(Json::as_bool)
                        .unwrap_or(false),
                    // pixels a second, not characters: stepping a whole character at a time is the juddering a
                    // character-quantised scroll shows, so the speed is read in pixels
                    scroll_speed: number("scroll_speed").unwrap_or(30.0).clamp(1.0, 200.0),
                    // one place sets it: the match's result is given its command below
                    command: None,
                }
            }
            "bar" => Widget::Bar {
                x: count("x"),
                y: count("y"),
                w: count("w"),
                h: count("h"),
                source: text("source"),
                max: number("max").unwrap_or(100.0),
                command: None,
            },
            "line" => Widget::Line {
                x: count("x"),
                y: count("y"),
                w: count("w"),
                command: None,
            },
            "segments" => Widget::Segments {
                x: count("x"),
                y: count("y"),
                w: count("w"),
                h: count("h"),
                count: count("count").max(1),
                source: text("source"),
                max: number("max").unwrap_or(100.0),
                command: None,
            },
            "bitmap" => Widget::Bitmap {
                x: count("x"),
                y: count("y"),
                name: text("name"),
                command: None,
            },
            "button" => Widget::Button {
                x: count("x"),
                y: count("y"),
                w: count("w"),
                h: count("h"),
                format: text("format"),
                command: None,
            },
            "list" => {
                // `rows` is how many of them fit on the screen, which is the applet's business rather than the
                // reader's; four is what fits the panel under a title and above nothing else
                let rows = count("rows");
                Widget::List {
                    x: count("x"),
                    y: count("y"),
                    w: count("w"),
                    rows: if rows == 0 { 4 } else { rows },
                    source: text("source"),
                    command: None,
                }
            }
            "brackets" => Widget::Brackets {
                x: count("x"),
                y: count("y"),
                h: count("h"),
                len: count("len"),
                thick: count("thick").max(1),
                command: None,
            },
            "arrow" => Widget::Arrow {
                x: count("x"),
                y: count("y"),
                radius: count("r").max(1),
                source: text("source"),
                command: None,
            },
            _ => return Ok(None),
        })
        // one place, so a new widget kind cannot be the one that forgets it
        .with_command(command_of(widget)),
    ))
}

/// What a widget does when the pad's L4 is pressed on it. Absent means it is not something you can act on.
fn command_of(widget: &Json) -> Option<String> {
    widget
        .get("command")
        .and_then(Json::as_str)
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_is_not_json_is_reported_as_such() {
        assert!(
            parse_applet("{not json")
                .unwrap_err()
                .contains("not valid JSON")
        );
        assert!(parse_applet("42").unwrap_err().contains("JSON object"));
        assert!(parse_applet("{}").unwrap_err().contains("needs a name"));
    }

    #[test]
    fn every_kind_the_window_offers_is_a_kind_this_reader_reads() {
        // The window's add button offers `WIDGET_KINDS`, so a kind missing from that list is a widget nobody
        // can make - which is exactly how `list` arrived: the fields table knew it, the offer did not.
        for kind in crate::WIDGET_KINDS {
            let text = format!(
                r#"{{"name": "x", "widgets": [{{"type": "{kind}", "source": "s", "format": "f"}}]}}"#
            );
            let applet = parse_applet(&text).expect(kind);
            assert_eq!(applet.widgets.len(), 1, "{kind} was not read as a widget");
            assert!(
                !applet
                    .unhandled
                    .iter()
                    .any(|note| note.contains("not a widget type")),
                "{kind} is offered but not read: {:?}",
                applet.unhandled
            );
        }
    }

    #[test]
    fn follow_is_read_in_the_shapes_the_file_can_write_it() {
        // absent, and false: the applet is only ever shown because somebody chose it
        assert!(parse_applet(r#"{"name":"x"}"#).unwrap().follow.is_none());
        assert!(
            parse_applet(r#"{"name":"x","follow":false}"#)
                .unwrap()
                .follow
                .is_none()
        );

        // true: the default window, no binding set
        let follow = parse_applet(r#"{"name":"x","follow":true}"#)
            .unwrap()
            .follow
            .expect("true is a window");
        assert_eq!(follow.seconds, crate::Follow::DEFAULT_SECONDS);
        assert_eq!(follow.profile, None);

        // a bare number, and the long form with a binding set named
        let follow = parse_applet(r#"{"name":"x","follow":5}"#)
            .unwrap()
            .follow
            .expect("a number is a window");
        assert_eq!(follow.seconds, 5.0);
        let follow = parse_applet(r#"{"name":"x","follow":{"seconds":30,"profile":"Cyberpunk"}}"#)
            .unwrap()
            .follow
            .expect("an object is a window");
        assert_eq!(follow.seconds, 30.0);
        assert_eq!(follow.profile.as_deref(), Some("Cyberpunk"));

        // and the long form without a number is the default window
        let follow = parse_applet(r#"{"name":"x","follow":{"profile":"Cyberpunk"}}"#)
            .unwrap()
            .follow
            .unwrap();
        assert_eq!(follow.seconds, crate::Follow::DEFAULT_SECONDS);
    }

    #[test]
    fn defaults_are_the_ones_the_applets_rely_on() {
        let applet = parse_applet(r#"{"name":"bare"}"#).unwrap();
        assert_eq!(applet.title, "bare");
        assert_eq!(applet.interval, 1.0);
        assert!(!applet.border);
        assert!(applet.widgets.is_empty());
        assert!(applet.sources.is_empty());
    }

    #[test]
    fn keys_that_are_read_but_not_acted_on_are_listed() {
        // `follow` was in this test, and in `unhandled`, until the driver acted on it: an applet fed by a running
        // program takes the screen, so the key has a meaning now and the file is not complained about
        let applet = parse_applet(r#"{"name":"x","mystery":1}"#).unwrap();
        assert!(
            !applet.unhandled.iter().any(|item| item.contains("follow")),
            "follow is acted on and should not be complained about: {:?}",
            applet.unhandled
        );
        assert!(
            applet.unhandled.iter().any(|item| item.contains("mystery")),
            "{:?}",
            applet.unhandled
        );
    }
}
