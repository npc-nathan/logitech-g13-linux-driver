//! `menu.json`  -  a menu of your own, with submenus and an action per item.
//!
//! The pad's menu is built from this file when it is there, and from the enabled screens when it is not. That
//! order matters: an installation that has no menu file keeps the menu it has, and nothing has to be written to
//! get one - this is how you say something the rotation cannot.
//!
//! Read and written as the file's own JSON rather than through a derive, for the same reason the applet editor
//! is: what the window edits has to be what the file says, keys this build does not know included.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// One item of a menu: something to look at, something to run, or something else to open.
///
/// * `show` - a screen: `clock`, `media`, `pad`, `applet:docker`.
/// * `screen` - with `show`, which screen of that applet, counted from 1 the way the window and the pad count.
/// * `command` - run it instead, as a source spec (`cmd:`, `file:`, `http:`, …).
/// * `items` - more menu under this one. An item with children opens them rather than acting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuItem {
    /// What the item is called on the pad's screen.
    pub label: String,
    /// A screen to show when it is chosen: `clock`, `media`, `pad`, `applet:docker`.
    pub show: Option<String>,
    /// Which screen of the `show`ed applet, counted from 1 as the pad counts them.
    pub screen: Option<usize>,
    /// A source spec to run instead of showing anything: `cmd:`, `file:`, `http:`.
    pub command: Option<String>,
    /// Items under this one; an item that has children opens them rather than acting.
    pub items: Vec<MenuItem>,
}

impl MenuItem {
    /// Whether this item opens a level rather than doing something.
    pub fn opens(&self) -> bool {
        !self.items.is_empty()
    }

    /// What is wrong with this item, if anything. A menu that cannot act is refused by name rather than opening
    /// a level with nothing in it or a level that does nothing.
    pub fn problem(&self) -> Option<String> {
        if self.label.trim().is_empty() {
            return Some("every menu item needs a label: it is what is on the screen".to_string());
        }
        if self.opens() {
            // a level is opened by choosing it, so acting as well would be two things from one press
            if self.show.is_some() || self.command.is_some() {
                return Some(format!(
                    "{}: an item with items under it opens them, so it cannot also show a screen or run a \
                     command",
                    self.label
                ));
            }
            for child in &self.items {
                if let Some(problem) = child.problem() {
                    return Some(format!("{} > {problem}", self.label));
                }
            }
            return None;
        }
        if self.show.is_none() && self.command.is_none() {
            return Some(format!(
                "{}: an item needs something to do - a `show` screen or a `command` to run",
                self.label
            ));
        }
        if self.show.is_some() && self.command.is_some() {
            return Some(format!(
                "{}: an item shows a screen or runs a command, not both",
                self.label
            ));
        }
        if self.screen.is_some() && self.show.is_none() {
            return Some(format!(
                "{}: `screen` says which screen of a `show`ed applet, so it needs a `show` beside it",
                self.label
            ));
        }
        None
    }

    /// Read one item out of the file's JSON.
    pub fn from_value(value: &Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "a menu item is a JSON object".to_string())?;
        let text = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|held| !held.is_empty())
                .map(str::to_string)
        };
        let mut items = Vec::new();
        for child in object
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            items.push(MenuItem::from_value(child)?);
        }
        Ok(MenuItem {
            label: object
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string(),
            show: text("show"),
            screen: object
                .get("screen")
                .and_then(Value::as_u64)
                .map(|screen| screen as usize),
            command: text("command"),
            items,
        })
    }

    /// This item as the file writes it. An empty field is left out rather than written as null, because not
    /// being there is what the reader looks for.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("label".to_string(), Value::String(self.label.clone()));
        if let Some(show) = &self.show {
            object.insert("show".to_string(), Value::String(show.clone()));
        }
        if let Some(screen) = self.screen {
            object.insert("screen".to_string(), Value::from(screen as u64));
        }
        if let Some(command) = &self.command {
            object.insert("command".to_string(), Value::String(command.clone()));
        }
        if !self.items.is_empty() {
            object.insert(
                "items".to_string(),
                Value::Array(self.items.iter().map(MenuItem::to_value).collect()),
            );
        }
        Value::Object(object)
    }
}

/// The file as a whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuFile {
    /// The top level, in the order the pad shows it.
    pub items: Vec<MenuItem>,
}

impl MenuFile {
    /// What is wrong with the file as a whole, if anything: the first problem, named by its item.
    pub fn problem(&self) -> Option<String> {
        if self.items.is_empty() {
            return Some("a menu file with no items in it is not a menu".to_string());
        }
        for item in &self.items {
            if let Some(problem) = item.problem() {
                return Some(problem);
            }
        }
        None
    }
}

impl MenuFile {
    /// Read the file out of its own JSON, so a caller that edits the JSON can still check what it would write.
    pub fn from_value(value: &Value) -> Result<Self, String> {
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| "a menu needs an `items` list".to_string())?;
        let mut menu = MenuFile::default();
        for item in items {
            menu.items.push(MenuItem::from_value(item)?);
        }
        Ok(menu)
    }

    /// This file as the JSON it is written as.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert(
            "items".to_string(),
            Value::Array(self.items.iter().map(MenuItem::to_value).collect()),
        );
        Value::Object(object)
    }
}

/// Where the menu file lives.
pub fn menu_path() -> PathBuf {
    crate::config_dir().join("menu.json")
}

/// The menu as it is written, or nothing when the file is not there.
///
/// A file that will not read is reported rather than ignored: no menu file and a menu file that will not parse
/// look identical from the pad, and only one of them is worth looking for.
pub fn read_menu(path: &Path) -> Result<Option<MenuFile>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|problem| format!("{} is not a menu: {problem}", path.display()))?;
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{} is not a menu: it needs an `items` list", path.display()))?;
    let mut menu = MenuFile::default();
    for item in items {
        menu.items.push(MenuItem::from_value(item)?);
    }
    match menu.problem() {
        Some(problem) => Err(format!("{}: {problem}", path.display())),
        None => Ok(Some(menu)),
    }
}

/// Write a menu held as its own JSON, refusing anything that would not read back.
///
/// The window edits the file's JSON, so this is the door it comes through: the same checks run, and a menu that
/// would not read is not written at all.
pub fn write_menu_value(path: &Path, value: &Value) -> Result<(), String> {
    let menu = MenuFile::from_value(value)?;
    write_menu(path, &menu)
}

/// Write the menu, whole or not at all, and never a file that would not read back.
pub fn write_menu(path: &Path, menu: &MenuFile) -> Result<(), String> {
    if let Some(problem) = menu.problem() {
        return Err(problem);
    }
    let text = serde_json::to_string_pretty(&Value::Object({
        let mut object = Map::new();
        object.insert(
            "items".to_string(),
            Value::Array(menu.items.iter().map(MenuItem::to_value).collect()),
        );
        object
    }))
    .map_err(|problem| problem.to_string())?;
    // read it back before writing: a menu the pad cannot read is worse than no menu file at all
    serde_json::from_str::<Value>(&text)
        .map_err(|problem| format!("this would not read back: {problem}"))?;
    // written beside and renamed into place, so a reader sees all of either version and never half of one
    g13_files::write(path, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("g13-menu-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_menu_item_says_what_it_does_and_what_is_wrong_with_it() {
        let showing = MenuItem {
            label: "clock".to_string(),
            show: Some("clock".to_string()),
            ..MenuItem::default()
        };
        assert!(!showing.opens());
        assert_eq!(showing.problem(), None);

        let folder = MenuItem {
            label: "screens".to_string(),
            items: vec![showing.clone()],
            ..MenuItem::default()
        };
        assert!(folder.opens());
        assert_eq!(folder.problem(), None);

        // a labelling fault, a do-nothing item, and the shapes that are two things at once
        assert!(MenuItem::default().problem().unwrap().contains("label"));
        let nothing = MenuItem {
            label: "?".to_string(),
            ..MenuItem::default()
        };
        assert!(nothing.problem().unwrap().contains("something to do"));
        let both = MenuItem {
            label: "both".to_string(),
            show: Some("clock".to_string()),
            command: Some("cmd:true".to_string()),
            ..MenuItem::default()
        };
        assert!(both.problem().unwrap().contains("not both"));
        let screen_only = MenuItem {
            label: "which".to_string(),
            screen: Some(2),
            ..MenuItem::default()
        };
        assert!(screen_only.problem().unwrap().contains("show"));
        let folder_that_acts = MenuItem {
            label: "confused".to_string(),
            show: Some("clock".to_string()),
            items: vec![showing],
            ..MenuItem::default()
        };
        assert!(folder_that_acts.problem().unwrap().contains("opens them"));
        // a problem inside a level is named by the way there
        let deep = MenuItem {
            label: "top".to_string(),
            items: vec![MenuItem::default()],
            ..MenuItem::default()
        };
        assert!(
            deep.problem().unwrap().starts_with("top > "),
            "{:?}",
            deep.problem()
        );
    }

    #[test]
    fn a_menu_file_round_trips_and_a_bad_one_is_refused_by_name() {
        let dir = a_dir("round-trip");
        let path = dir.join("menu.json");
        // not there at all is not an error: that is how the built-in menu is chosen
        assert_eq!(read_menu(&path), Ok(None));

        let menu = MenuFile {
            items: vec![
                MenuItem {
                    label: "screens".to_string(),
                    items: vec![MenuItem {
                        label: "clock".to_string(),
                        show: Some("clock".to_string()),
                        ..MenuItem::default()
                    }],
                    ..MenuItem::default()
                },
                MenuItem {
                    label: "docker".to_string(),
                    show: Some("applet:docker".to_string()),
                    screen: Some(2),
                    ..MenuItem::default()
                },
                MenuItem {
                    label: "music".to_string(),
                    command: Some("cmd:playerctl play-pause".to_string()),
                    ..MenuItem::default()
                },
            ],
        };
        write_menu(&path, &menu).unwrap();
        assert_eq!(read_menu(&path), Ok(Some(menu)));

        // an empty menu is not written, and neither is one with a broken item
        assert!(write_menu(&path, &MenuFile::default()).is_err());
        assert!(
            write_menu(
                &path,
                &MenuFile {
                    items: vec![MenuItem::default()]
                }
            )
            .is_err()
        );
        // and the file that was already there is untouched by the attempt
        assert!(read_menu(&path).is_ok());

        // a file that is not a menu says so rather than looking empty
        std::fs::write(&path, "{not json").unwrap();
        assert!(read_menu(&path).unwrap_err().contains("not a menu"));
        std::fs::write(&path, r#"{"items": []}"#).unwrap();
        assert!(read_menu(&path).unwrap_err().contains("not a menu"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
