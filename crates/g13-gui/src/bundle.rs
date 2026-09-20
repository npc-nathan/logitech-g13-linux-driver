//! An applet, packed into one file so it can move to another machine.
//!
//! An applet is not one file. It can have pictures beside it, it can name a font and a theme, its
//! resources can name an endpoint, it can read a value of the user's own, and it can be one of the
//! screens the pad walks to. A bundle carries all of that at once, so moving an applet between
//! machines is one file rather than five files and a note about which belongs to which.
//!
//! Two things are deliberately left behind:
//!
//! - **A token.** An endpoint travels with its token slot *empty*. A bundle is a file somebody mails
//!   to somebody else or puts on a website; it is not a place for a key.
//! - **A machine's own paths.** A resource that reads a file names a path on the machine the applet
//!   came from, and no bundle can know where that file is somewhere else. The person importing it
//!   points it at their own, and the window says which resources name something that is not there.

use std::collections::BTreeMap;
use std::path::Path;

/// What a bundle says it is, so that importing one can refuse a file that is not one.
pub const KIND: &str = "g13-applet";

/// The shape of the contents. A bundle from a later version is refused rather than half read.
pub const VERSION: u64 = 1;

/// An applet and everything it needs, in one place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bundle {
    /// The applet's name, which is also the name of its files.
    pub name: String,
    /// The applet file, exactly as it was.
    pub applet: serde_json::Value,
    /// `applets/<name>.bitmaps.json`, if it has one.
    pub pictures: Option<serde_json::Value>,
    /// The fonts it names, by name, from `fonts/<name>.json`.
    pub fonts: BTreeMap<String, serde_json::Value>,
    /// The theme it wears, if it names one: `themes/<name>.json`.
    pub theme: Option<(String, serde_json::Value)>,
    /// The endpoints its resources use, with the token slot empty.
    pub endpoints: BTreeMap<String, serde_json::Value>,
    /// The values of the user's own that its resources read.
    pub values: BTreeMap<String, serde_json::Value>,
    /// Whether the pad walks to it.
    pub on_the_pad: bool,
}

/// A JSON file, or nothing. A part that will not read is left out rather than carried broken.
fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The names this applet's resources read through an endpoint: `http:weather/path#field` names
/// `weather`.
fn endpoints_used(applet: &serde_json::Value) -> Vec<String> {
    let mut found = Vec::new();
    let Some(sources) = applet.get("sources").and_then(serde_json::Value::as_object) else {
        return found;
    };
    for spec in sources.values() {
        let Some(spec) = spec.as_str() else { continue };
        let Some(rest) = spec.strip_prefix("http:") else {
            continue;
        };
        let Some((name, _)) = rest.split_once('/') else {
            continue;
        };
        if !name.is_empty() && !found.contains(&name.to_string()) {
            found.push(name.to_string());
        }
    }
    found
}

/// The names this applet's resources read that are not a mechanism: a bare word is either one of the
/// machine's own or a value the user has named, and only the second one has anything to carry.
fn bare_names(applet: &serde_json::Value) -> Vec<String> {
    let mut found = Vec::new();
    let Some(sources) = applet.get("sources").and_then(serde_json::Value::as_object) else {
        return found;
    };
    for spec in sources.values() {
        let Some(spec) = spec.as_str() else { continue };
        let bare = !spec.is_empty()
            && spec
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_');
        if bare && !found.contains(&spec.to_string()) {
            found.push(spec.to_string());
        }
    }
    found
}

/// The fonts an applet names: `"font": "runes"`, and `"fallbacks": ["greek", "cyrillic"]`.
fn fonts_named(applet: &serde_json::Value) -> Vec<String> {
    let mut found = Vec::new();
    for key in ["font", "fallbacks"] {
        match applet.get(key) {
            Some(serde_json::Value::String(name)) => {
                if !found.contains(name) {
                    found.push(name.clone());
                }
            }
            Some(serde_json::Value::Array(names)) => {
                for name in names {
                    if let Some(name) = name.as_str() {
                        if !found.contains(&name.to_string()) {
                            found.push(name.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    found
}

/// Pack `<config>/applets/<name>.json` and everything it needs into one bundle.
pub fn build(config: &Path, name: &str) -> Result<Bundle, String> {
    let applets = config.join("applets");
    let path = applets.join(format!("{name}.json"));
    let applet = read_json(&path)
        .ok_or_else(|| format!("{} is not there, or is not JSON", path.display()))?;

    let pictures = read_json(&applets.join(format!("{name}.bitmaps.json")));

    let mut fonts = BTreeMap::new();
    for font in fonts_named(&applet) {
        if let Some(body) = read_json(&config.join("fonts").join(format!("{font}.json"))) {
            fonts.insert(font, body);
        }
    }

    let theme = applet
        .get("theme")
        .and_then(serde_json::Value::as_str)
        .and_then(|theme| {
            read_json(&config.join("themes").join(format!("{theme}.json")))
                .map(|body| (theme.to_string(), body))
        });

    // An endpoint goes with the applet, and its token slot goes empty: the key is the importing
    // machine's own business, and it fills it in on the Endpoints tab.
    let declared = read_json(&config.join("endpoints.json")).unwrap_or(serde_json::json!({}));
    let mut endpoints = BTreeMap::new();
    for endpoint in endpoints_used(&applet) {
        let mut body = declared
            .get(&endpoint)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(fields) = body.as_object_mut() {
            fields.insert(
                "token".to_string(),
                serde_json::Value::String(String::new()),
            );
        }
        endpoints.insert(endpoint, body);
    }

    let own = read_json(&config.join("values.json")).unwrap_or(serde_json::json!({}));
    let mut values = BTreeMap::new();
    for name in bare_names(&applet) {
        if let Some(spec) = own.get(&name) {
            values.insert(name, spec.clone());
        }
    }

    let on_the_pad = read_json(&config.join("visuals.json"))
        .and_then(|visuals| visuals.get("enabled").cloned())
        .and_then(|enabled| enabled.as_array().cloned())
        .map(|enabled| {
            enabled
                .iter()
                .any(|item| item.as_str() == Some(format!("applet:{name}").as_str()))
        })
        .unwrap_or(false);

    Ok(Bundle {
        name: name.to_string(),
        applet,
        pictures,
        fonts,
        theme,
        endpoints,
        values,
        on_the_pad,
    })
}

/// The bundle as the file it is written to. Parts an applet does not have are left out rather than
/// written empty, so a plain applet is a small file and a diff of two bundles shows what changed.
pub fn to_json(bundle: &Bundle) -> String {
    let mut object = serde_json::Map::new();
    object.insert("kind".to_string(), serde_json::json!(KIND));
    object.insert("version".to_string(), serde_json::json!(VERSION));
    object.insert("name".to_string(), serde_json::json!(bundle.name));
    object.insert("applet".to_string(), bundle.applet.clone());
    if let Some(pictures) = &bundle.pictures {
        object.insert("pictures".to_string(), pictures.clone());
    }
    if !bundle.fonts.is_empty() {
        object.insert("fonts".to_string(), serde_json::json!(bundle.fonts));
    }
    if let Some((name, body)) = &bundle.theme {
        let mut theme = serde_json::Map::new();
        theme.insert(name.clone(), body.clone());
        object.insert("theme".to_string(), serde_json::Value::Object(theme));
    }
    if !bundle.endpoints.is_empty() {
        object.insert("endpoints".to_string(), serde_json::json!(bundle.endpoints));
    }
    if !bundle.values.is_empty() {
        object.insert("values".to_string(), serde_json::json!(bundle.values));
    }
    object.insert(
        "on_the_pad".to_string(),
        serde_json::json!(bundle.on_the_pad),
    );
    serde_json::to_string_pretty(&serde_json::Value::Object(object)).unwrap_or_default() + "\n"
}

/// A field as a person would read it: strings in quotes, anything else as its JSON.
fn show(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => format!("{text:?}"),
        serde_json::Value::Null => "nothing".to_string(),
        other => other.to_string(),
    }
}

/// How many widgets an applet draws, however it is laid out: one flat screen or several.
fn widget_count(applet: &serde_json::Value) -> usize {
    if let Some(widgets) = applet.get("widgets").and_then(serde_json::Value::as_array) {
        return widgets.len();
    }
    applet
        .get("screens")
        .and_then(serde_json::Value::as_array)
        .map(|screens| {
            screens
                .iter()
                .map(|screen| {
                    screen
                        .get("widgets")
                        .and_then(serde_json::Value::as_array)
                        .map(|widgets| widgets.len())
                        .unwrap_or(0)
                })
                .sum()
        })
        .unwrap_or(0)
}

/// The names in a part, for saying what an import would add or change.
fn names(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_object)
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

/// What importing this bundle would change about the applet you already have: one line per
/// difference, as (what, yours, its).
///
/// This exists because a name you already have is not silently replaced and not silently refused
/// either - you get to see what is different and then say.
pub fn compare(config: &Path, bundle: &Bundle) -> Vec<(String, String, String)> {
    let mut lines: Vec<(String, String, String)> = Vec::new();
    let mine = read_json(&config.join("applets").join(format!("{}.json", bundle.name)));
    let theirs = &bundle.applet;

    match &mine {
        None => lines.push((
            "applet".to_string(),
            "you do not have one called this".to_string(),
            "new".to_string(),
        )),
        Some(mine) => {
            for key in ["title", "interval", "theme", "font", "border"] {
                let (a, b) = (mine.get(key), theirs.get(key));
                let (a, b) = (
                    a.map(show).unwrap_or_else(|| "not set".to_string()),
                    b.map(show).unwrap_or_else(|| "not set".to_string()),
                );
                if a != b {
                    lines.push((key.to_string(), a, b));
                }
            }
            let (a, b) = (widget_count(mine), widget_count(theirs));
            if a != b {
                lines.push(("widgets".to_string(), a.to_string(), b.to_string()));
            }
            let (a, b) = (
                mine.get("screens")
                    .and_then(serde_json::Value::as_array)
                    .map(|s| s.len())
                    .unwrap_or(0),
                theirs
                    .get("screens")
                    .and_then(serde_json::Value::as_array)
                    .map(|s| s.len())
                    .unwrap_or(0),
            );
            if a != b {
                lines.push(("screens".to_string(), a.to_string(), b.to_string()));
            }

            // Sources, by name: what it would add, what it would take away, what it would change.
            let mine_sources = mine.get("sources").and_then(|s| s.as_object());
            let theirs_sources = theirs.get("sources").and_then(|s| s.as_object());
            let mut said = Vec::new();
            for (name, spec) in theirs_sources.into_iter().flatten() {
                match mine_sources.and_then(|sources| sources.get(name)) {
                    None => said.push(format!("+{name}")),
                    Some(have) if have != spec => said.push(format!("~{name}")),
                    Some(_) => {}
                }
            }
            for name in mine_sources.into_iter().flatten().map(|(name, _)| name) {
                if theirs_sources
                    .map(|s| !s.contains_key(name))
                    .unwrap_or(false)
                {
                    said.push(format!("-{name}"));
                }
            }
            if !said.is_empty() {
                said.sort();
                lines.push((
                    "sources".to_string(),
                    format!("{} in yours", mine_sources.map(|s| s.len()).unwrap_or(0)),
                    said.join(" "),
                ));
            }
        }
    }

    // The pictures, by name: how many it would add, change or take away.
    let have = read_json(
        &config
            .join("applets")
            .join(format!("{}.bitmaps.json", bundle.name)),
    );
    let (mut added, mut changed, mut gone) = (0, 0, 0);
    for name in names(bundle.pictures.as_ref()) {
        match have.as_ref().and_then(|p| p.get(&name)) {
            None => added += 1,
            Some(picture) => {
                if Some(picture) != bundle.pictures.as_ref().and_then(|p| p.get(&name)) {
                    changed += 1;
                }
            }
        }
    }
    for name in names(have.as_ref()) {
        if !names(bundle.pictures.as_ref()).contains(&name) {
            gone += 1;
        }
    }
    if added + changed + gone > 0 {
        lines.push((
            "pictures".to_string(),
            format!("{} in yours", names(have.as_ref()).len()),
            format!("+{added} ~{changed} -{gone}"),
        ));
    }

    // The fonts and the theme: whether you would get one or keep your own.
    for font in bundle.fonts.keys() {
        if !config.join("fonts").join(format!("{font}.json")).exists() {
            lines.push(("font".to_string(), "not here".to_string(), font.clone()));
        }
    }
    if let Some((theme, _)) = &bundle.theme {
        if !config.join("themes").join(format!("{theme}.json")).exists() {
            lines.push(("theme".to_string(), "not here".to_string(), theme.clone()));
        }
    }

    // The endpoints and the values, only the ones this machine does not have.
    let endpoints = g13_sources::load_endpoints(&config.join("endpoints.json")).unwrap_or_default();
    for endpoint in bundle.endpoints.keys() {
        if !endpoints.contains_key(endpoint) {
            lines.push((
                "endpoint".to_string(),
                "not here".to_string(),
                format!("{endpoint} (its token slot travels empty)"),
            ));
        }
    }
    let own = read_json(&config.join("values.json"))
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    for value in bundle.values.keys() {
        if !own.contains_key(value) {
            lines.push((
                "value of yours".to_string(),
                "not here".to_string(),
                value.clone(),
            ));
        }
    }

    let on_the_pad = mine.as_ref().map(|_| ()).is_some();
    let enabled = read_json(&config.join("visuals.json"))
        .and_then(|visuals| visuals.get("enabled").cloned())
        .and_then(|enabled| enabled.as_array().cloned())
        .unwrap_or_default();
    let walks = enabled
        .iter()
        .any(|item| item.as_str() == Some(format!("applet:{}", bundle.name).as_str()));
    if !on_the_pad || walks != bundle.on_the_pad {
        lines.push((
            "on the pad".to_string(),
            if walks { "yes" } else { "no" }.to_string(),
            if bundle.on_the_pad { "yes" } else { "no" }.to_string(),
        ));
    }

    lines
}

/// Whether an applet of this name is already here.
pub fn name_is_free(config: &Path, name: &str) -> bool {
    !config.join("applets").join(format!("{name}.json")).exists()
}

/// The first free name after this one: `weather-2`, `weather-3`, and so on.
pub fn a_free_name(config: &Path, name: &str) -> String {
    let mut number = 2;
    loop {
        let candidate = format!("{name}-{number}");
        if name_is_free(config, &candidate) {
            return candidate;
        }
        number += 1;
    }
}

/// Read a bundle back out of the file it was written to.
pub fn parse(text: &str) -> Result<Bundle, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|problem| format!("not JSON: {problem}"))?;
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if kind != KIND {
        return Err(format!(
            "not an applet file: it says its kind is {kind:?}, and an applet file says {KIND:?}"
        ));
    }
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if version > VERSION {
        return Err(format!(
            "made by a newer version of this window (it says {version}, this build reads {VERSION})"
        ));
    }
    let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if name.is_empty() {
        return Err("it does not say which applet it is".to_string());
    }
    let applet = value
        .get("applet")
        .filter(|applet| applet.is_object())
        .cloned()
        .ok_or_else(|| "it has no applet in it".to_string())?;

    let part = |key: &str| -> BTreeMap<String, serde_json::Value> {
        value
            .get(key)
            .and_then(serde_json::Value::as_object)
            .map(|object| object.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    };

    Ok(Bundle {
        name,
        applet,
        pictures: value.get("pictures").filter(|p| p.is_object()).cloned(),
        fonts: part("fonts"),
        theme: value
            .get("theme")
            .and_then(serde_json::Value::as_object)
            .and_then(|theme| theme.iter().next())
            .map(|(name, body)| (name.clone(), body.clone())),
        endpoints: part("endpoints"),
        values: part("values"),
        on_the_pad: value
            .get("on_the_pad")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

/// Write a value out as pretty JSON, making the folder it goes in first.
fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|problem| format!("cannot make {}: {problem}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(value).unwrap_or_default() + "\n";
    g13_files::write(path, &text)
        .map_err(|problem| format!("cannot write {}: {problem}", path.display()))
}

/// Write a bundle into a config under `name`, and say what was written.
///
/// Anything you already have wins. A font, a theme, an endpoint's token and a value of your own are
/// never overwritten by an import - what is here is yours, and the file being imported cannot know
/// better. The applet itself is written, because importing an applet is the whole point; the caller
/// decides whether that is a replacement or a copy under another name.
pub fn apply(config: &Path, bundle: &Bundle, name: &str) -> Result<Vec<String>, String> {
    let applets = config.join("applets");
    let mut wrote = Vec::new();

    let mut applet = bundle.applet.clone();
    if let Some(fields) = applet.as_object_mut() {
        if fields.contains_key("name") {
            fields.insert("name".to_string(), serde_json::json!(name));
        }
    }
    write_json(&applets.join(format!("{name}.json")), &applet)?;
    wrote.push(format!("applet {name}"));

    if let Some(pictures) = &bundle.pictures {
        write_json(&applets.join(format!("{name}.bitmaps.json")), pictures)?;
        wrote.push(format!("{} picture(s)", names(Some(pictures)).len()));
    }

    for (font, body) in &bundle.fonts {
        let path = config.join("fonts").join(format!("{font}.json"));
        if path.exists() {
            wrote.push(format!("font {font} kept, you already have one"));
        } else {
            write_json(&path, body)?;
            wrote.push(format!("font {font}"));
        }
    }

    if let Some((theme, body)) = &bundle.theme {
        let path = config.join("themes").join(format!("{theme}.json"));
        if path.exists() {
            wrote.push(format!("theme {theme} kept, you already have one"));
        } else {
            write_json(&path, body)?;
            wrote.push(format!("theme {theme}"));
        }
    }

    if !bundle.endpoints.is_empty() {
        let path = config.join("endpoints.json");
        match g13_sources::load_endpoints(&path) {
            Err(problem) => wrote.push(format!(
                "endpoints left alone, {path:?} will not read ({problem})"
            )),
            Ok(mut endpoints) => {
                let mut added = Vec::new();
                let mut without_url: Vec<String> = Vec::new();
                for (endpoint, body) in &bundle.endpoints {
                    if endpoints.contains_key(endpoint) {
                        continue;
                    }
                    // Through the same parser the driver reads the file with, so a bundle cannot
                    // bring an endpoint that means something subtly different here. The token slot
                    // is empty in the file and stays empty: this machine fills in its own.
                    let mut one = serde_json::Map::new();
                    one.insert(endpoint.clone(), body.clone());
                    let parsed =
                        g13_sources::parse_endpoints(&serde_json::Value::Object(one).to_string());
                    match parsed.into_iter().next() {
                        Some((_, endpoint_body)) if !endpoint_body.url.is_empty() => {
                            endpoints.insert(endpoint.clone(), endpoint_body);
                            added.push(endpoint.clone());
                        }
                        _ => without_url.push(endpoint.clone()),
                    }
                }
                if !added.is_empty() {
                    g13_files::write(&path, &g13_sources::endpoints_to_json(&endpoints))
                        .map_err(|problem| format!("cannot write {}: {problem}", path.display()))?;
                    wrote.push(format!("endpoint(s) {}", added.join(", ")));
                }
                if !without_url.is_empty() {
                    wrote.push(format!(
                        "endpoint(s) {} brought no url, so they were left out",
                        without_url.join(", ")
                    ));
                }
            }
        }
    }

    if !bundle.values.is_empty() {
        let path = config.join("values.json");
        let mut own = read_json(&path)
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        let mut added = Vec::new();
        for (value, spec) in &bundle.values {
            if !own.contains_key(value) {
                own.insert(value.clone(), spec.clone());
                added.push(value.clone());
            }
        }
        if !added.is_empty() {
            write_json(&path, &serde_json::Value::Object(own))?;
            wrote.push(format!("value(s) of yours {}", added.join(", ")));
        }
    }

    Ok(wrote)
}

#[cfg(test)]
mod an_applet_that_leaves_home {
    use super::*;
    use std::path::PathBuf;

    fn a_config(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        dir
    }

    fn write(config: &Path, part: &str, body: &str) {
        let path = config.join(part);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// An applet using every part there is: pictures of its own, a font, a theme, an endpoint and a
    /// value the user named.
    fn everything(config: &Path) -> &'static str {
        write(
            config,
            "applets/game.json",
            r#"{"name":"game","title":"GAME","theme":"cyberpunk-2077","font":"runes",
                "sources":{"health":"http:stats/x#health","place":"where"},
                "widgets":[{"type":"text","x":3,"y":2,"format":"{health} {place}"}]}"#,
        );
        write(
            config,
            "applets/game.bitmaps.json",
            &serde_json::json!({"heart": {"rows": ["#.#", ".#."]}}).to_string(),
        );
        write(config, "fonts/runes.json", r#"{"name":"runes","size":8}"#);
        write(
            config,
            "themes/cyberpunk-2077.json",
            &serde_json::json!({"name": "cyberpunk-2077", "ink": "#00f0ff"}).to_string(),
        );
        write(
            config,
            "endpoints.json",
            r#"{"stats":{"url":"https://example.invalid","token":"ghp_notarealtokenatall"},
                "unused":{"url":"https://elsewhere.invalid","token":"also-secret"}}"#,
        );
        write(config, "values.json", r#"{"where":"http:weather/x#place"}"#);
        write(
            config,
            "visuals.json",
            r#"{"enabled":["clock","applet:game"],"active":"clock"}"#,
        );
        "game"
    }

    #[test]
    fn everything_it_needs_comes_with_it() {
        let config = a_config("g13-bundle-everything");
        let name = everything(&config);
        let bundle = build(&config, name).unwrap();

        assert_eq!(bundle.name, "game");
        assert!(bundle.pictures.is_some(), "its pictures did not come");
        assert_eq!(bundle.fonts.len(), 1, "the font it names did not come");
        assert!(bundle.fonts.contains_key("runes"));
        assert_eq!(
            bundle.theme.as_ref().map(|(name, _)| name.as_str()),
            Some("cyberpunk-2077"),
            "the theme it wears did not come"
        );
        assert_eq!(
            bundle.endpoints.len(),
            1,
            "it should carry only what it uses"
        );
        assert!(bundle.endpoints.contains_key("stats"));
        assert_eq!(bundle.values.len(), 1, "the value it reads did not come");
        assert!(bundle.values.contains_key("where"));
        assert!(
            bundle.on_the_pad,
            "the pad walks to it and the bundle says not"
        );
    }

    #[test]
    fn a_token_never_travels() {
        let config = a_config("g13-bundle-token");
        let name = everything(&config);
        let written = to_json(&build(&config, name).unwrap());

        assert!(
            !written.contains("ghp_"),
            "a token was written into the bundle"
        );
        assert!(
            !written.contains("also-secret"),
            "an endpoint it does not even use was written into the bundle"
        );
        assert!(
            written.contains("\"token\": \"\""),
            "the token slot should be there and empty, so the importing machine knows to fill it in"
        );
    }

    #[test]
    fn a_plain_applet_is_a_small_file() {
        let config = a_config("g13-bundle-plain");
        write(
            &config,
            "applets/plain.json",
            r#"{"name":"plain","widgets":[{"type":"text","x":0,"y":0,"format":"hi"}]}"#,
        );
        let bundle = build(&config, "plain").unwrap();
        assert!(bundle.pictures.is_none());
        assert!(bundle.fonts.is_empty());
        assert!(bundle.theme.is_none());
        assert!(bundle.endpoints.is_empty());
        assert!(bundle.values.is_empty());
        assert!(!bundle.on_the_pad);

        let written = to_json(&bundle);
        for absent in ["pictures", "fonts", "theme", "endpoints", "values"] {
            assert!(
                !written.contains(&format!("\"{absent}\"")),
                "{absent} is written empty instead of left out"
            );
        }
    }

    #[test]
    fn the_file_says_what_it_is() {
        let config = a_config("g13-bundle-kind");
        write(
            &config,
            "applets/plain.json",
            r#"{"name":"plain","widgets":[]}"#,
        );
        let written = to_json(&build(&config, "plain").unwrap());
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed["kind"], KIND);
        assert_eq!(parsed["version"], VERSION);
        assert_eq!(parsed["name"], "plain");
        assert!(parsed["applet"].is_object(), "the applet itself is missing");
    }

    #[test]
    fn an_applet_that_is_not_there_is_said_rather_than_guessed_at() {
        let config = a_config("g13-bundle-missing");
        let problem = build(&config, "nowhere").unwrap_err();
        assert!(
            problem.contains("nowhere"),
            "the message does not name it: {problem}"
        );
    }
}

#[cfg(test)]
mod carrying_an_applet_to_another_machine {
    use super::*;
    use std::path::PathBuf;

    fn a_config(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        dir
    }

    fn write(config: &Path, part: &str, body: &str) {
        let path = config.join(part);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// A machine with the applet on it: pictures, a font, a theme, an endpoint with a token in it.
    fn with_the_applet(name: &str) -> PathBuf {
        let config = a_config(name);
        write(
            &config,
            "applets/game.json",
            r#"{"name":"game","title":"GAME","theme":"cyberpunk-2077","font":"runes",
                "sources":{"health":"http:stats/x#health","place":"where"},
                "widgets":[{"type":"text","x":3,"y":2,"format":"{health} {place}"}]}"#,
        );
        write(
            &config,
            "applets/game.bitmaps.json",
            &serde_json::json!({"heart": {"rows": ["#.#", ".#."]}}).to_string(),
        );
        write(&config, "fonts/runes.json", r#"{"name":"runes","size":8}"#);
        write(
            &config,
            "themes/cyberpunk-2077.json",
            &serde_json::json!({"name": "cyberpunk-2077", "ink": "#00f0ff"}).to_string(),
        );
        write(
            &config,
            "endpoints.json",
            r#"{"stats":{"url":"https://from.example","token":"ghp_from_the_other_machine"}}"#,
        );
        write(
            &config,
            "values.json",
            r#"{"where":"http:weather/x#place"}"#,
        );
        write(
            &config,
            "visuals.json",
            r#"{"enabled":["clock","applet:game"],"active":"clock"}"#,
        );
        config
    }

    #[test]
    fn the_bundle_survives_the_round_trip_through_the_file() {
        let from = with_the_applet("g13-round-trip-from");
        let bundle = build(&from, "game").unwrap();
        let read_back = parse(&to_json(&bundle)).unwrap();
        assert_eq!(
            read_back, bundle,
            "the file does not read back as what was written"
        );
    }

    #[test]
    fn a_whole_applet_lands_on_another_machine() {
        let from = with_the_applet("g13-carry-from");
        let to = a_config("g13-carry-to");
        let bundle = build(&from, "game").unwrap();
        let wrote = apply(&to, &bundle, "game").unwrap();

        assert!(
            to.join("applets/game.json").exists(),
            "the applet did not land"
        );
        assert!(
            to.join("applets/game.bitmaps.json").exists(),
            "its pictures did not land"
        );
        assert!(
            to.join("fonts/runes.json").exists(),
            "the font it names did not land"
        );
        assert!(
            to.join("themes/cyberpunk-2077.json").exists(),
            "the theme did not land"
        );
        assert!(
            to.join("values.json").exists(),
            "the value it reads did not land"
        );
        assert!(
            wrote.iter().any(|line| line.contains("endpoint")),
            "an endpoint it uses was not written: {wrote:?}"
        );

        // and the endpoint it brought is here with an empty token slot, not somebody else's key
        let endpoints = g13_sources::load_endpoints(&to.join("endpoints.json")).unwrap();
        let stats = endpoints.get("stats").expect("the endpoint did not land");
        assert_eq!(stats.url, "https://from.example");
        assert!(
            stats.token.as_deref().unwrap_or("").is_empty(),
            "a token travelled with the applet"
        );
    }

    #[test]
    fn an_endpoint_you_already_have_keeps_your_token() {
        let from = with_the_applet("g13-keep-token-from");
        let to = a_config("g13-keep-token-to");
        std::fs::write(
            to.join("endpoints.json"),
            r#"{"stats":{"url":"https://mine.example","token":"my-own-token"}}"#,
        )
        .unwrap();

        apply(&to, &build(&from, "game").unwrap(), "game").unwrap();

        let endpoints = g13_sources::load_endpoints(&to.join("endpoints.json")).unwrap();
        let stats = endpoints.get("stats").expect("mine was thrown away");
        assert_eq!(
            stats.token.as_deref(),
            Some("my-own-token"),
            "my token was overwritten"
        );
        assert_eq!(
            stats.url, "https://mine.example",
            "my endpoint was overwritten"
        );
    }

    #[test]
    fn a_font_you_already_have_is_not_written_over() {
        let from = with_the_applet("g13-keep-font-from");
        let to = a_config("g13-keep-font-to");
        write(
            &to,
            "fonts/runes.json",
            r#"{"name":"runes","size":12,"mine":true}"#,
        );

        let wrote = apply(&to, &build(&from, "game").unwrap(), "game").unwrap();

        let kept = std::fs::read_to_string(to.join("fonts/runes.json")).unwrap();
        assert!(kept.contains("mine"), "my font was written over: {kept}");
        assert!(
            wrote.iter().any(|line| line.contains("font runes kept")),
            "the import did not say it kept my font: {wrote:?}"
        );
    }

    #[test]
    fn a_name_you_already_have_is_listed_rather_than_written_over() {
        let from = with_the_applet("g13-collision-from");
        let to = with_the_applet("g13-collision-to");
        // make the one on this machine different, so there is something to compare
        write(
            &to,
            "applets/game.json",
            r#"{"name":"game","title":"MINE","sources":{"health":"http:stats/x#health"},
                "widgets":[{"type":"text","x":0,"y":0,"format":"mine"},
                           {"type":"text","x":0,"y":9,"format":"and this one"}]}"#,
        );
        let before = std::fs::read_to_string(to.join("applets/game.json")).unwrap();

        let lines = compare(&to, &build(&from, "game").unwrap());

        assert_eq!(
            std::fs::read_to_string(to.join("applets/game.json")).unwrap(),
            before,
            "comparing wrote something"
        );
        let said: Vec<String> = lines.iter().map(|(what, _, _)| what.clone()).collect();
        for expected in ["title", "theme", "widgets", "sources"] {
            assert!(
                said.contains(&expected.to_string()),
                "{expected} is not listed: {said:?}"
            );
        }
        let sources = lines.iter().find(|(what, _, _)| what == "sources").unwrap();
        assert!(
            sources.2.contains("+place"),
            "a source it would add is not shown: {sources:?}"
        );
    }

    #[test]
    fn a_copy_takes_the_next_free_name() {
        let from = with_the_applet("g13-copy-from");
        let to = with_the_applet("g13-copy-to");
        assert!(!name_is_free(&to, "game"));
        assert!(name_is_free(&to, "nowhere"));
        assert_eq!(a_free_name(&to, "game"), "game-2");

        std::fs::write(to.join("applets/game-2.json"), "{}").unwrap();
        assert_eq!(a_free_name(&to, "game"), "game-3");

        // and importing as the copy puts it beside the one that was here, under its own name
        let bundle = build(&from, "game").unwrap();
        apply(&to, &bundle, "game-2").unwrap();
        let copied: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(to.join("applets/game-2.json")).unwrap())
                .unwrap();
        assert_eq!(
            copied["name"], "game-2",
            "the copy still calls itself what it was called"
        );
        assert!(
            to.join("applets/game.json").exists(),
            "the one that was here was removed"
        );
    }

    #[test]
    fn a_file_that_is_not_an_applet_is_refused() {
        let plain = r#"{"hello":"world"}"#;
        let problem = parse(plain).unwrap_err();
        assert!(problem.contains("not an applet file"), "{problem}");

        let newer = r#"{"kind":"g13-applet","version":99,"name":"x","applet":{}}"#;
        let problem = parse(newer).unwrap_err();
        assert!(problem.contains("newer version"), "{problem}");

        let no_applet = r#"{"kind":"g13-applet","version":1,"name":"x"}"#;
        let problem = parse(no_applet).unwrap_err();
        assert!(problem.contains("no applet"), "{problem}");

        let unnamed = r#"{"kind":"g13-applet","version":1,"applet":{}}"#;
        let problem = parse(unnamed).unwrap_err();
        assert!(problem.contains("which applet"), "{problem}");
    }
}
