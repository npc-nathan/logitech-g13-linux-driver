//! The virtual keyboard this program presents to the system.
//!
//! Written from the kernel's uinput interface and the evdev crate's documentation for it. Nothing here
//! comes from the predecessor: this device is named as this project names things, and which key each
//! control sends is a decision the configuration makes, not a fact this module invents.

pub use evdev::KeyCode;
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent};
use std::io;

/// The name this keyboard appears under, which is what `evtest`, games and window managers will show.
pub const DEVICE_NAME: &str = "g13 keyboard";

/// Why the virtual keyboard could not be created or written to.
#[derive(Debug)]
pub enum KeyboardError {
    /// The kernel refused a uinput call: creating the device, or emitting an event.
    Io(io::Error),
}

impl std::fmt::Display for KeyboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyboardError::Io(error) => write!(
                f,
                "{error}\n(a virtual keyboard needs write access to /dev/uinput)"
            ),
        }
    }
}

impl std::error::Error for KeyboardError {}

impl From<io::Error> for KeyboardError {
    fn from(error: io::Error) -> Self {
        KeyboardError::Io(error)
    }
}

/// A virtual keyboard.
pub struct VirtualKeyboard {
    /// The uinput device itself, which owns the node and takes it away when dropped.
    device: VirtualDevice,
}

impl VirtualKeyboard {
    /// Create the virtual keyboard, advertising the whole key range a keyboard can send.
    ///
    /// The full range rather than a chosen subset: which keys are actually emitted is decided by the
    /// user's bindings, and a device that cannot report a key it is asked for would fail silently in the
    /// middle of somebody's game.
    pub fn new() -> Result<Self, KeyboardError> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in 0..=255u16 {
            keys.insert(KeyCode::new(code));
        }
        let device = VirtualDevice::builder()?
            .name(DEVICE_NAME)
            .with_keys(&keys)?
            .build()?;
        Ok(Self { device })
    }

    /// Send a key going down.
    pub fn press(&mut self, key: KeyCode) -> Result<(), KeyboardError> {
        self.device
            .emit(&[InputEvent::new(EventType::KEY.0, key.code(), 1)])?;
        Ok(())
    }

    /// Send a key coming up.
    pub fn release(&mut self, key: KeyCode) -> Result<(), KeyboardError> {
        self.device
            .emit(&[InputEvent::new(EventType::KEY.0, key.code(), 0)])?;
        Ok(())
    }

    /// Press and release in one go, for a binding that is a single tap rather than a hold.
    pub fn tap(&mut self, key: KeyCode) -> Result<(), KeyboardError> {
        self.press(key)?;
        self.release(key)
    }
}

/// Parse the right-hand side of a binding into a key.
///
/// Both forms are accepted because both exist in the wild: a number, which is what a keycode is, and a
/// name like `a` or `space`, which is what a person types. Nothing is guessed  -  an unknown name is an
/// error rather than a silent default.
pub fn parse_key(text: &str) -> Result<KeyCode, String> {
    let text = text.trim();
    if let Ok(code) = text.parse::<u16>() {
        return Ok(KeyCode::new(code));
    }
    for name in KEY_NAMES {
        if name.0.eq_ignore_ascii_case(text) {
            return Ok(KeyCode::new(name.1));
        }
    }
    Err(format!("unknown key: {text}"))
}

/// The key for a character, and whether shift makes it. `None` for a character this build will not type.
///
/// A table rather than a layout engine, and deliberately so: US and UK layouts agree on every character here.
/// Returning `None` is the point - a macro that says "type £" must be told this build cannot, rather than
/// quietly typing something else.
pub fn char_key(character: char) -> Option<(u16, bool)> {
    let (name, shifted): (&str, bool) = match character {
        'a'..='z' => (Box::leak(character.to_string().into_boxed_str()), false),
        'A'..='Z' => (
            Box::leak(character.to_ascii_lowercase().to_string().into_boxed_str()),
            true,
        ),
        '0'..='9' => (Box::leak(character.to_string().into_boxed_str()), false),
        ' ' => ("space", false),
        '\n' => ("enter", false),
        '\t' => ("tab", false),
        '-' => ("minus", false),
        '_' => ("minus", true),
        '=' => ("equal", false),
        '+' => ("equal", true),
        '[' => ("leftbrace", false),
        '{' => ("leftbrace", true),
        ']' => ("rightbrace", false),
        '}' => ("rightbrace", true),
        ';' => ("semicolon", false),
        ':' => ("semicolon", true),
        '\'' => ("apostrophe", false),
        '"' => ("apostrophe", true),
        '`' => ("grave", false),
        '~' => ("grave", true),
        '\\' => ("backslash", false),
        '|' => ("backslash", true),
        ',' => ("comma", false),
        '<' => ("comma", true),
        '.' => ("dot", false),
        '>' => ("dot", true),
        '/' => ("slash", false),
        '?' => ("slash", true),
        '!' => ("1", true),
        '@' => ("2", true),
        '#' => ("3", true),
        '$' => ("4", true),
        '%' => ("5", true),
        '^' => ("6", true),
        '&' => ("7", true),
        '*' => ("8", true),
        '(' => ("9", true),
        ')' => ("0", true),
        _ => return None,
    };
    let code = KEY_NAMES
        .iter()
        .find(|(candidate, _)| *candidate == name)?
        .1;
    Some((code, shifted))
}

/// The name a keycode has, so a binding can be read back as the key it is: `16` is `q`.
///
/// `None` when this build has no name for the code, which is where it is printed as a number rather than guessed at.
pub fn name_for(code: u16) -> Option<&'static str> {
    KEY_NAMES
        .iter()
        .find(|(_, known)| *known == code)
        .map(|(name, _)| *name)
}

/// The names people actually use, with the codes the kernel gives them.
///
/// A short deliberate list rather than the whole alphabet of kernel names: these are the ones worth
/// typing by hand, and a binding can always use the number for anything else.
/// The name of a key code, or the code itself where this build has no name for it.
///
/// The fallback is the doc above `name_for` put into a function: a code with no name is printed as its number
/// rather than guessed at. One place says that, because two surfaces that each chose their own fallback ended
/// up saying `no name` in one and `key 17` in the other for the same key.
pub fn name_or_code(code: u16) -> String {
    name_for(code)
        .map(str::to_string)
        .unwrap_or_else(|| format!("key {code}"))
}

/// The names a key can be written by, as `(name, code)` pairs: every name `parse_key` accepts and `name_for`
/// can return.
///
/// A short deliberate list rather than the whole alphabet of kernel names - the ones worth typing by hand -
/// and a binding can always use the number for anything else.
pub const KEY_NAMES: &[(&str, u16)] = &[
    ("escape", 1),
    ("1", 2),
    ("2", 3),
    ("3", 4),
    ("4", 5),
    ("5", 6),
    ("6", 7),
    ("7", 8),
    ("8", 9),
    ("9", 10),
    ("0", 11),
    ("minus", 12),
    ("equal", 13),
    ("backspace", 14),
    ("tab", 15),
    ("q", 16),
    ("w", 17),
    ("e", 18),
    ("r", 19),
    ("t", 20),
    ("y", 21),
    ("u", 22),
    ("i", 23),
    ("o", 24),
    ("p", 25),
    ("leftbrace", 26),
    ("rightbrace", 27),
    ("enter", 28),
    ("leftctrl", 29),
    ("a", 30),
    ("s", 31),
    ("d", 32),
    ("f", 33),
    ("g", 34),
    ("h", 35),
    ("j", 36),
    ("k", 37),
    ("l", 38),
    ("semicolon", 39),
    ("apostrophe", 40),
    ("grave", 41),
    ("leftshift", 42),
    ("backslash", 43),
    ("z", 44),
    ("x", 45),
    ("c", 46),
    ("v", 47),
    ("b", 48),
    ("n", 49),
    ("m", 50),
    ("comma", 51),
    ("dot", 52),
    ("slash", 53),
    ("rightshift", 54),
    ("leftalt", 56),
    ("space", 57),
    ("capslock", 58),
    ("f1", 59),
    ("f2", 60),
    ("f3", 61),
    ("f4", 62),
    ("f5", 63),
    ("f6", 64),
    ("f7", 65),
    ("f8", 66),
    ("f9", 67),
    ("f10", 68),
    ("f11", 87),
    ("f12", 88),
    ("up", 103),
    ("left", 105),
    ("right", 106),
    ("down", 108),
    ("home", 102),
    ("end", 107),
    ("pageup", 104),
    ("pagedown", 109),
    ("insert", 110),
    ("delete", 111),
    ("leftmeta", 125),
    ("rightctrl", 97),
    ("rightalt", 100),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keycode_is_accepted_as_a_number() {
        assert_eq!(parse_key("44").unwrap().code(), 44);
        assert_eq!(parse_key(" 57 ").unwrap().code(), 57);
    }

    #[test]
    fn a_key_name_is_accepted_case_insensitively() {
        assert_eq!(parse_key("a").unwrap().code(), 30);
        assert_eq!(parse_key("SPACE").unwrap().code(), 57);
        assert_eq!(parse_key("LeftShift").unwrap().code(), 42);
    }

    #[test]
    fn a_keycode_can_be_named() {
        assert_eq!(name_for(16), Some("q"), "16 is q on this keymap");
        assert_eq!(name_for(57), Some("space"));
        assert_eq!(name_for(60000), None, "a code with no name is not invented");
    }

    #[test]
    fn an_unknown_name_is_an_error_rather_than_a_default() {
        assert!(parse_key("wibble").is_err());
        assert!(parse_key("").is_err());
    }

    #[test]
    fn the_common_keys_are_all_present() {
        for name in [
            "a", "z", "space", "enter", "escape", "f1", "f12", "up", "down",
        ] {
            assert!(KEY_NAMES.iter().any(|(n, _)| *n == name), "{name} missing");
        }
    }
    #[test]
    fn a_character_knows_its_key_and_whether_shift_makes_it() {
        assert_eq!(char_key('a'), Some((30, false)));
        assert_eq!(char_key('A'), Some((30, true)));
        assert_eq!(char_key('1'), Some((2, false)));
        assert_eq!(char_key('!'), Some((2, true)));
        assert_eq!(char_key(' '), Some((57, false)));
        // the punctuation a macro is most likely to want
        assert_eq!(char_key('/'), Some((53, false)));
        assert_eq!(char_key('?'), Some((53, true)));
        assert_eq!(char_key(':'), Some((39, true)));
        // and a character this build will not type says so instead of typing something else
        assert_eq!(char_key('£'), None);
        assert_eq!(char_key('中'), None);
    }
}
