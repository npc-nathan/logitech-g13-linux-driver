//! The pad's menu: levels of the user's own making, drawn on the screen and driven by L1-L4.
//!
//! It opens on the screens that are enabled, and an item of the user's own opens a level below it, so the menu is
//! a tree the configuration describes rather than a list this code decides. Every decision is a function here and
//! every side effect belongs to the caller, which is what makes the menu answerable without a hand on a pad: the
//! driver's loop applies what `menu_move` says, and the window draws what `menu_lines` returns.
//!
//! The file the menu comes from (`menu.json`, beside the applets) is the user's own words, so its labels are used
//! as written rather than reworded.

use crate::{Visuals, screens_of, visuals};
use g13_proto::Control;
use g13_screen::Frame;
use std::path::Path;

/// The menu, while one is open.
///
/// It lives in the driver's state beside the record wizard, for the same reason: the pad's controls drive it, the
/// screen draws it and the state file can publish it. One answer, three readers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    /// What this level is called, when it is a level of its own. Empty for the list of screens, which is what
    /// the menu opens on and what the title line already describes.
    pub title: String,
    /// What each item is called, as it is drawn.
    pub items: Vec<String>,
    /// What choosing each item does. Same length as `items`.
    pub targets: Vec<MenuTarget>,
    /// Which item is highlighted.
    pub index: usize,
    /// How deep: one for the list of screens, and one more when an applet's own screens are opened.
    pub level: usize,
    /// The level this one was opened from, with its cursor where it was left.
    ///
    /// A level rather than a list of levels, because that is what the rule asks for: L1 goes back one, and
    /// going back means the level underneath is exactly as it was.
    pub parent: Option<Box<Menu>>,
}

/// What choosing a menu item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuTarget {
    /// Show this screen of the rotation.
    Show(String),
    /// Open this applet's own screens as a level of the menu.
    Screens(String),
    /// Show one screen of an applet: the visual, and which screen of it.
    ShowAt {
        /// Which visual to show.
        visual: String,
        /// Which screen of it, counted from zero.
        screen: usize,
    },
    /// Open this item's own items, from `menu.json`.
    Child(g13_config::menu::MenuItem),
    /// Run this command, from `menu.json`.
    Run(String),
}

/// Which of the menu's four keys a control is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    /// Leave this level: back one when there is one above, and out when there is not.
    Back,
    /// Move the highlight up one item, wrapping at the top.
    Previous,
    /// Move the highlight down one item, wrapping at the bottom.
    Next,
    /// Choose the highlighted item.
    Select,
}

/// The menu's keys: L1 back, L2 previous, L3 next, L4 select.
///
/// Fixed rather than bindable, because they are the menu's own controls and a menu you have to configure before
/// you can use it is not a menu. They are only taken *while the menu is open*: the rest of the time each control
/// does whatever the profile's file says: outside the menu, none of these four is claimed by anything.
pub fn menu_key(bit: usize) -> Option<MenuKey> {
    let control = g13_proto::control_for_bit(bit)?;
    Some(match control {
        Control::L1 => MenuKey::Back,
        Control::L2 => MenuKey::Previous,
        Control::L3 => MenuKey::Next,
        Control::L4 => MenuKey::Select,
        _ => return None,
    })
}

impl Menu {
    /// The menu as it opens: one row per screen that is enabled, named the way a person would say it.
    ///
    /// The applet's own title is read from its file when there is one - `demo-stats` rather than
    /// `applet:demo-stats` - because a menu is read at a glance on a small screen and the scheme is noise there.
    pub fn over_screens(enabled: &[String], applets: &Path, showing: &str) -> Option<Self> {
        let drawable = visuals::drawable_visuals(applets);
        let mut items: Vec<String> = Vec::new();
        let mut targets: Vec<MenuTarget> = Vec::new();

        // What is showing, when it has screens of its own, comes first: it is the thing the pad is on and the
        // thing L1's rule is about. One press opens its screens as a level, and back returns to this list.
        let showing_screens = screens_of(showing, applets);
        if showing_screens > 1 {
            items.push(format!(
                "{}  {} screens",
                menu_label(showing, applets),
                showing_screens
            ));
            targets.push(MenuTarget::Screens(showing.to_string()));
        }

        for name in enabled {
            if !drawable.contains(name) {
                continue;
            }
            items.push(menu_label(name, applets));
            targets.push(MenuTarget::Show(name.clone()));
        }
        if items.is_empty() {
            return None;
        }
        Some(Self {
            title: String::new(),
            items,
            targets,
            index: 0,
            level: 1,
            parent: None,
        })
    }

    /// The item that would be chosen now.
    pub fn chosen(&self) -> Option<&MenuTarget> {
        self.targets.get(self.index)
    }

    /// Back one level: the level this one was opened from, or nothing when there is nothing above.
    pub fn back(self) -> Option<Menu> {
        self.parent.map(|parent| *parent)
    }

    /// Open `child` over this level, so back returns here with its cursor where it was.
    pub fn open_over(self, mut child: Menu) -> Menu {
        child.level = self.level + 1;
        child.parent = Some(Box::new(self));
        child
    }

    /// Step the highlight on by one item, wrapping to the first after the last.
    pub fn next(&mut self) {
        if !self.items.is_empty() {
            self.index = (self.index + 1) % self.items.len();
        }
    }

    /// Step the highlight back by one item, wrapping to the last before the first.
    pub fn previous(&mut self) {
        if !self.items.is_empty() {
            self.index = (self.index + self.items.len() - 1) % self.items.len();
        }
    }
}

/// How a screen is named in the menu: the applet's own title if it has one, otherwise its name without the
/// scheme in front of it.
pub fn menu_label(name: &str, applets: &Path) -> String {
    let plain = name.strip_prefix("applet:").unwrap_or(name);
    if name.starts_with("applet:") {
        // the applet's own title, read from the file the driver is already pointed at
        if let Ok(value) = g13_applets::load_applet_json(applets, plain) {
            if let Some(title) = value.get("title").and_then(|title| title.as_str()) {
                if !title.trim().is_empty() {
                    return title.trim().to_string();
                }
            }
        }
    }
    plain.to_string()
}
/// Open the menu over the screens that are enabled.
///
/// Nothing to list is not a menu, so it says so rather than opening an empty one.
pub(crate) fn open_menu(
    visuals: &Visuals,
    applets: &Path,
    showing: &str,
) -> Result<(Menu, Option<String>), String> {
    menu_for(visuals, applets, showing, &menu_file_beside(applets))
}
/// What one press of a menu key does, decided here and carried out by the loop.
///
/// The loop's own handling of L1-L4 used to be the decision and the side effects together, in a match nested
/// three deep in the run function - which is where the tap/hold fault lived for the same reason: nothing that
/// only happens under a hand on a pad can be tested there. The menu goes in and comes back out, so "does L1 go
/// back one level or out" is a question with an answer a test can read.
#[derive(Debug, PartialEq, Eq)]
pub enum MenuMove {
    /// The menu as it is now, with the cursor moved.
    Stayed(Menu),
    /// One level deeper: the level this one was opened from is kept under it.
    Deeper(Menu, Menu),
    /// Back one level: this is the level underneath, and `None` means there was none and the menu is closed.
    Wound(Option<Menu>),
    /// The item was chosen: show or open this.
    Chose(MenuTarget),
    /// Nothing to do, with the reason said.
    Refused(String),
}

/// Apply one of the menu's own keys: move its cursor, go a level, or choose what is highlighted.
///
/// The decision is here and the side effects are the caller's, which is what makes the menu answerable
/// without a hand on a pad.
pub fn menu_move(menu: Menu, key: MenuKey, applets: &Path) -> MenuMove {
    match key {
        MenuKey::Previous | MenuKey::Next => {
            let mut moved = menu;
            match key {
                MenuKey::Previous => moved.previous(),
                _ => moved.next(),
            }
            MenuMove::Stayed(moved)
        }
        MenuKey::Back => MenuMove::Wound(menu.back()),
        MenuKey::Select => match menu.chosen().cloned() {
            Some(MenuTarget::Screens(visual)) => match screens_level(&visual, applets) {
                Some(level) => MenuMove::Deeper(menu, level),
                None => MenuMove::Refused(format!("{visual} has no screens of its own to list")),
            },
            // an item of the user's own with something under it opens them
            Some(MenuTarget::Child(item)) => match child_level(&item) {
                Some(level) => MenuMove::Deeper(menu, level),
                None => MenuMove::Refused(format!("{} has nothing under it", item.label)),
            },
            Some(target) => MenuMove::Chose(target),
            None => MenuMove::Refused("this level of the menu is empty".to_string()),
        },
    }
}

/// A level of the menu built from `menu.json`, and the one below it when an item opens.
///
/// The file's labels are used as they are written rather than reworded: a menu is the user's own words, and an
/// item that shows a screen is not the same thing as the screen's name.
pub fn level_from(items: &[g13_config::menu::MenuItem], title: &str, level: usize) -> Option<Menu> {
    if items.is_empty() {
        return None;
    }
    let mut labels = Vec::new();
    let mut targets = Vec::new();
    for item in items {
        labels.push(item.label.clone());
        targets.push(if item.opens() {
            MenuTarget::Child(item.clone())
        } else if let Some(command) = &item.command {
            MenuTarget::Run(command.clone())
        } else if let Some(show) = &item.show {
            match item.screen {
                // the file counts screens from 1, as the window and the pad do
                Some(screen) => MenuTarget::ShowAt {
                    visual: show.clone(),
                    screen: screen.saturating_sub(1),
                },
                None => MenuTarget::Show(show.clone()),
            }
        } else {
            // `problem()` refuses this when the file is read, so it cannot arrive here from a menu that loaded
            MenuTarget::Run(String::new())
        });
    }
    Some(Menu {
        title: title.to_string(),
        items: labels,
        targets,
        index: 0,
        level,
        parent: None,
    })
}

/// One item's own items, as a level of the menu.
pub fn child_level(item: &g13_config::menu::MenuItem) -> Option<Menu> {
    level_from(&item.items, item.label.trim(), 0)
}

/// The menu to open: the user's own when there is a file, and the rotation when there is not.
///
/// That order is the whole point of the file being optional - an installation that never writes one keeps the
/// menu it has, and writing one is how you say something the rotation cannot.
pub(crate) fn menu_for(
    visuals: &Visuals,
    applets: &Path,
    showing: &str,
    menu_file: &Path,
) -> Result<(Menu, Option<String>), String> {
    match g13_config::menu::read_menu(menu_file) {
        Ok(Some(menu)) => match level_from(&menu.items, "", 1) {
            Some(menu) => Ok((menu, None)),
            None => Err("menu.json has no items to show".to_string()),
        },
        Ok(None) => Ok((over_screens_or(visuals, applets, showing)?, None)),
        // A file that will not read is said - once, at the top of the next pass - and the rotation is used
        // instead, because a broken menu must not take the pad's own button with it.
        Err(problem) => Ok((over_screens_or(visuals, applets, showing)?, Some(problem))),
    }
}

/// The rotation as a menu, or the reason there is none.
fn over_screens_or(visuals: &Visuals, applets: &Path, showing: &str) -> Result<Menu, String> {
    Menu::over_screens(&visuals.enabled, applets, showing)
        .ok_or_else(|| "there is nothing to put in the menu: no screens are enabled".to_string())
}

/// Where the menu file is, given where the applets are: beside them, in the configuration folder.
fn menu_file_beside(applet_dir: &Path) -> std::path::PathBuf {
    applet_dir.parent().unwrap_or(applet_dir).join("menu.json")
}

/// One applet's own screens, as a level of the menu.
///
/// The applet's file is read here rather than remembered from when the level below was built, so a screen added
/// in the window between two presses of L4 is there when it is looked for.
pub fn screens_level(visual: &str, applets: &Path) -> Option<Menu> {
    let name = visual.strip_prefix("applet:")?;
    let value = g13_applets::load_applet_json(applets, name).ok()?;
    let screens = value.get("screens").and_then(|s| s.as_array())?;
    if screens.len() < 2 {
        return None;
    }
    let mut items = Vec::new();
    let mut targets = Vec::new();
    for (at, screen) in screens.iter().enumerate() {
        let title = screen
            .get("title")
            .and_then(|title| title.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        items.push(if title.is_empty() {
            format!("screen {}", at + 1)
        } else {
            title
        });
        targets.push(MenuTarget::ShowAt {
            visual: visual.to_string(),
            screen: at,
        });
    }
    Some(Menu {
        title: menu_label(visual, applets),
        items,
        targets,
        index: 0,
        level: 2,
        parent: None,
    })
}

/// The menu as lines of text: the title, the items with one marked, and the keys.
///
/// The lines are the whole of the drawing's thinking - which items are in the window when there are more than
/// fit, where the mark goes, what is cut off at the edge - so a test can read them without a pad, a frame or a
/// font. `menu_frame` below only paints them.
pub fn menu_lines(menu: &Menu, rows: usize, columns: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let count = menu.items.len();
    // a level of its own says what it is; the list of screens is what the menu is by default
    if menu.title.is_empty() {
        lines.push(format!("menu  {}/{}", menu.index + 1, count));
    } else {
        lines.push(format!("{}  {}/{}", menu.title, menu.index + 1, count));
    }

    // room for the title and the key hint; the rest is items
    let room = rows.saturating_sub(2).max(1);
    let first = if menu.index < room {
        0
    } else {
        menu.index + 1 - room
    };
    for (at, item) in menu.items.iter().enumerate().skip(first).take(room) {
        let mark = if at == menu.index { '>' } else { ' ' };
        lines.push(format!("{mark} {item}"));
    }
    // It says what L1 will really do - at the top level there is nothing above it to go back to - and it fits
    // the panel's own 26 columns. `L2/L3` rather than `L2/3` is what makes the longer word fit: the previous
    // spelling ran to 27 and the pad showed `L4 pic`, which nobody had seen because it is only wrong on the pad.
    lines.push(
        match menu.parent.is_some() {
            true => "L1 back L2/3 move L4 pick",
            false => "L1 close L2/3 move L4 pick",
        }
        .to_string(),
    );
    lines
        .into_iter()
        .map(|line| {
            // the panel is narrow: a long label is cut, not wrapped into the row below it
            line.chars().take(columns).collect()
        })
        .collect()
}

/// The menu drawn on the panel.
pub fn menu_frame(menu: &Menu) -> Frame {
    let mut frame = Frame::new();
    let rows = (g13_screen::VISIBLE_HEIGHT / 8).max(3);
    for (row, line) in menu_lines(menu, rows, g13_screen::TEXT_COLUMNS)
        .iter()
        .enumerate()
    {
        frame.text_line(row, line);
    }
    frame
}

/// Where L2 or L3 moves the selection to, wrapping at both ends.
///
/// One thing to act on cannot be moved between: pressing either key lands on the same one rather than dividing
/// by zero, which is the difference between a control that does nothing and a driver that stops.
pub(crate) fn step_selection(cursor: usize, len: usize, back: bool) -> usize {
    if len < 2 {
        return 0;
    }
    if back {
        (cursor + len - 1) % len
    } else {
        (cursor + 1) % len
    }
}
