//! Where an applet's numbers come from.
//!
//! A source is written as `<kind>:<rest>#<path>`, named by mechanism rather than by product:
//!
//! ```text
//! cmd:docker ps -q | wc -l
//! json:/home/me/hud.json#health
//! json:/home/me/hud.json#player.stats.level
//! http:weather/London?format=j1#current_condition.0.temp_C
//! imap:mail/INBOX#unread
//! ```
//!
//! This module knows the forms and the local kinds (a command, a file, an environment variable, the machine
//! itself). Networked kinds live in `net`, so nothing here can surprise you by opening a socket.

// a test may unwrap and may fail loudly: a test that cannot panic on a fixture cannot fail
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
use g13_values::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// A source as written in an applet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// Which mechanism it names: `cmd`, `file`, `json`, `http`, `env`, `regex`, or a built-in.
    pub kind: String,
    /// What that mechanism is handed: the command line, the path, or the endpoint, up to any `#`.
    pub rest: String,
    /// What follows the `#`, when the source names something inside its answer: the key to read.
    pub path: Option<String>,
    /// The source exactly as it was written, for saying it back to a person.
    pub raw: String,
}

impl Spec {
    /// Split `<kind>:<rest>#<path>`. A source with no colon names no mechanism.
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        let (head, path) = match text.rsplit_once('#') {
            Some((head, path)) => (head, Some(path.to_string())),
            None => (text, None),
        };
        // A source with no colon is a name out of the published catalogue: `"cpu": "cpu"`. `built-in:<name>`
        // says the same thing in long form. Both are the catalogue, and the catalogue is a documented list with
        // a meaning per entry, shown in the window - not a hidden fallback.
        let (kind, rest) = match head.split_once(':') {
            Some((kind, rest)) => (kind.to_string(), rest.to_string()),
            None => ("built-in".to_string(), head.to_string()),
        };
        Self {
            kind,
            rest,
            path,
            raw: text.to_string(),
        }
    }

    /// The endpoint this source reads, for the kinds that name one.
    ///
    /// An `http:` source is `http:<endpoint>/<path>#<field>`, so the endpoint is the first thing after the
    /// scheme - the rest is the path within it. A source that reads this machine (`file:`, `cmd:`, `regex:`)
    /// names no endpoint, and says so rather than inventing one.
    pub fn endpoint(&self) -> Option<&str> {
        if self.kind != "http" {
            return None;
        }
        let name = self
            .rest
            .split_once('/')
            .map(|(name, _)| name)
            .unwrap_or(self.rest.as_str())
            .trim();
        if name.is_empty() { None } else { Some(name) }
    }

    /// Whether this source can be read without a network or another program's endpoint.
    pub fn is_local(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "built-in" | "env" | "file" | "json" | "cmd" | "regex"
        )
    }
}

/// Why a source produced nothing. Kept as text because it ends up on a screen or in a log line.
pub type Problem = String;

/// An endpoint from `endpoints.json`, as the applets name them.
#[derive(Debug, Clone, Default)]
pub struct Endpoint {
    /// Where the request goes: scheme, host, and the path its fields hang off.
    pub url: String,
    /// The credential to send with it, for an endpoint that needs one.
    pub token: Option<String>,
    /// Seconds to wait before giving up: three by default, held between half a second and thirty.
    pub timeout: Option<f64>,
    /// Ask for the certificate not to be checked. Read from the file, not yet acted on.
    pub insecure: bool,
}

/// Everything a resolver may need that is not the machine itself.
///
/// `Clone` because a macro's questions are asked with a copy of the playback that has the macro's own sources
/// read into it, and the world goes with it.
#[derive(Debug, Clone)]
pub struct World {
    /// Endpoints by the name an applet writes after `http:`.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// How long a `cmd` source has before it is killed - 400 ms by default - so a slow command cannot
    /// hold a screen up.
    pub command_timeout: Duration,
    /// Where to measure the machine from: `/` normally, a fixture in a test.
    pub root: PathBuf,
    /// The file the running driver publishes its values to, for the names only it can know - the stick, the
    /// profile, what is on the pad. None in a test, or when no driver is running: those names are then Missing,
    /// which is the truth.
    pub values: Option<PathBuf>,
    /// Values the user named, from `values.json`, already resolved.
    ///
    /// They are *here* rather than in the file above because resolving one runs a command, and this world is built
    /// on the drawing side - off the input loop, which is the pad's responsiveness. Filled once per change of the
    /// file (see `named_values`), so twenty frames a second do not mean twenty resolutions.
    pub named: BTreeMap<String, Value>,
    /// The clock, read once. Empty in a world built by hand, which then reads one when a clock value is asked
    /// for - so a test or a one-shot tool is correct without knowing about this, and the driver's own path reads
    /// the clock exactly once per gather.
    pub clock: Clock,
}

impl Default for World {
    fn default() -> Self {
        Self {
            endpoints: BTreeMap::new(),
            command_timeout: Duration::from_millis(400),
            root: PathBuf::from("/"),
            values: None,
            named: BTreeMap::new(),
            clock: Clock::default(),
        }
    }
}

impl World {
    /// Where the running driver publishes its values.
    ///
    /// Set by whatever is driving: the names only it can know - the stick, the profile, what is on the pad - are
    /// read back out of that file. A test, or a run with no driver, leaves it unset and those names are Missing.
    pub fn with_values(mut self, path: &std::path::Path) -> Self {
        self.values = Some(path.to_path_buf());
        self
    }

    /// The values the user named, resolved by the caller so this stays a plain struct.
    pub fn with_named(mut self, named: BTreeMap<String, Value>) -> Self {
        self.named = named;
        self
    }

    /// Give this world a clock reading, so everything built from it shares one instant.
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// Read `endpoints.json` if it is there. A missing file is not an error: applets may use only local
    /// sources.
    pub fn with_endpoints(mut self, path: &std::path::Path) -> Self {
        if let Ok(text) = std::fs::read_to_string(path) {
            self.endpoints = parse_endpoints(&text);
        }
        self
    }
}

/// The file a source reads, when it reads one: `file:` and `json:` are files, and a `cmd:` runs, an `http:`
/// fetches.
///
/// `follow` uses this: "the file behind this applet is being written" is how an applet says "the program feeding
/// me is running", without anything having to watch processes.
pub fn file_behind(spec: &str) -> Option<std::path::PathBuf> {
    let spec = Spec::parse(spec);
    match spec.kind.as_str() {
        "file" | "json" => {
            let file = spec.rest.trim();
            (!file.is_empty()).then(|| std::path::PathBuf::from(file))
        }
        _ => None,
    }
}

/// Whether an endpoint's url is something an endpoint can be, and why not if it is not.
///
/// An endpoint is a *host* and its credentials, named so that a source can point at it - `http:name/path`. It is
/// not a source: a `cmd:`, `file:` or bare name written here would be pasted into a url and fetched, which cannot
/// work, and saying so on the tab is the difference between a wrong entry and a silently dead one.
pub fn endpoint_url_problem(url: &str) -> Option<String> {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return Some(if url.contains(':') {
            format!(
                "an endpoint is the host a source points at, not the source itself: `{url}` looks like a source \
                 spec, and those go in the applet's own sources"
            )
        } else {
            "not a url: an endpoint is written as scheme://host, e.g. https://api.github.com"
                .to_string()
        });
    };
    if rest.is_empty() {
        return Some("a url with no host after the scheme".to_string());
    }
    const NETWORK: [&str; 8] = [
        "http", "https", "mqtt", "mqtts", "ws", "wss", "imap", "imaps",
    ];
    if !NETWORK.contains(&scheme.to_ascii_lowercase().as_str()) {
        return Some(format!(
            "{scheme}:// is not something this build fetches (it knows {})",
            NETWORK.join(", ")
        ));
    }
    None
}

/// What is risky about an endpoint's credential, if anything, as a sentence for a person to read.
///
/// A note, never a refusal: the file is the endpoint owner's own, an `http:` host on a private network is a
/// real setup, and `insecure` is written down on purpose (wttr.in's certificate has been expired for hours at
/// a time, and an applet that shows nothing is the alternative). What this exists for is the one combination
/// nobody means to write: a bearer token sent somewhere it can be read on the way.
///
/// Two shapes, and neither is about the url being wrong - `endpoint_url_problem` is what says that.
pub fn endpoint_risk(endpoint: &Endpoint) -> Option<String> {
    if endpoint
        .token
        .as_deref()
        .is_none_or(|token| token.trim().is_empty())
    {
        // no credential, so there is nothing a link can give away. An `insecure` host with no token is the
        // weather endpoint this project ships, and a warning about it would be noise
        return None;
    }
    // no url to judge: a url that is not `scheme://host` is `endpoint_url_problem`'s to report
    let (scheme, _) = endpoint.url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    const PROTECTED: [&str; 4] = ["https", "wss", "imaps", "mqtts"];
    if !PROTECTED.contains(&scheme.as_str()) {
        return Some(format!(
            "a token is sent over {scheme}:// and anyone on the way to it can read the token"
        ));
    }
    if endpoint.insecure {
        return Some(
            "the certificate is not checked and a token is sent: a machine in the middle can take the token"
                .to_string(),
        );
    }
    None
}

/// Read `endpoints.json`, with the reason when it cannot be read.
///
/// A missing file is not an error - applets may use only local sources - but a file that is present and
/// **unreadable or malformed is an error**, and saying so is the whole point of this function. The resolver's
/// own loader ignores a file it cannot parse, which leaves the endpoint map empty and makes a typo in a token
/// look exactly like a machine with no endpoints configured at all.
pub fn load_endpoints(path: &std::path::Path) -> Result<BTreeMap<String, Endpoint>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))?;
    if !parsed.is_object() {
        return Err(format!(
            "{} should hold an object of named endpoints, one per name",
            path.display()
        ));
    }
    Ok(parse_endpoints(&text))
}

/// Values the user has named, from `values.json`: a name to a spec.
///
/// The spec is written in the applets' own language - `cmd:`, `file:`, `http:<endpoint>/<path>#<field>`, `json:`, or
/// a published name - so there is one language and one resolver for the whole system rather than a second one here.
/// A file that will not read is *said*, never silently empty: a `values.json` with a typo in it must not look like
/// one with nothing in it.
pub fn load_custom_values(path: &std::path::Path) -> Result<BTreeMap<String, String>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))?;
    let Some(object) = parsed.as_object() else {
        return Err(format!(
            "{} should hold an object of names to specs, one per value",
            path.display()
        ));
    };
    let mut values = BTreeMap::new();
    for (name, spec) in object {
        match spec.as_str() {
            Some(spec) => {
                values.insert(name.clone(), spec.to_string());
            }
            None => {
                return Err(format!(
                    "{}: {name} should be a spec written as a string, like \"cmd:uptime\"",
                    path.display()
                ));
            }
        }
    }
    Ok(values)
}

/// The user's own values, resolved, kept until `values.json` changes.
///
/// The same shape as the media readings' cache and for the same reason: resolving one runs a command, and this is
/// asked for on the drawing side twenty times a second. The file's own modification time and length are the whole of
/// the test - reading a file this size costs microseconds; *resolving* is what costs, and that is what is skipped.
pub fn named_values(
    path: &std::path::Path,
    published: &std::path::Path,
) -> (BTreeMap<String, Value>, Vec<String>) {
    /// The file's modification time and length, then what it held: a change to either is a different file.
    type Kept = (
        std::time::SystemTime,
        u64,
        BTreeMap<String, Value>,
        Vec<String>,
    );
    static KEPT: std::sync::OnceLock<std::sync::Mutex<Kept>> = std::sync::OnceLock::new();
    let stamp = std::fs::metadata(path)
        .and_then(|data| Ok((data.modified()?, data.len())))
        .unwrap_or((std::time::SystemTime::UNIX_EPOCH, 0));
    let kept = KEPT.get_or_init(|| {
        std::sync::Mutex::new((
            std::time::SystemTime::UNIX_EPOCH,
            0,
            BTreeMap::new(),
            Vec::new(),
        ))
    });
    if let Ok(held) = kept.lock()
        && held.0 == stamp.0
        && held.1 == stamp.1
    {
        return (held.2.clone(), held.3.clone());
    }
    let (specs, mut complaints) = match load_custom_values(path) {
        Ok(specs) => (specs, Vec::new()),
        Err(problem) => (BTreeMap::new(), vec![problem]),
    };
    let world = World::default().with_values(published);
    let (resolved, more) = resolve_custom_values(&specs, &world);
    complaints.extend(more);
    if let Ok(mut held) = kept.lock() {
        *held = (stamp.0, stamp.1, resolved.clone(), complaints.clone());
    }
    (resolved, complaints)
}

/// Resolve each of the user's own values, in name order.
///
/// **A name that is already published is refused, not written over.** The built-in catalogue is what every applet
/// already reads, and a value of the user's own that quietly took one of those names would change the meaning of
/// applets the user did not touch. The refusal comes back as a complaint rather than an error, because one bad name
/// should not stop the rest from working.
///
/// The world is the same one an applet resolves against, so a spec may name an endpoint, a built-in or a file. What
/// it may *not* do is name another value the user defined - there is one pass and no order to rely on, and a silent
/// empty answer to a half-defined name is worse than saying so.
pub fn resolve_custom_values(
    specs: &BTreeMap<String, String>,
    world: &World,
) -> (BTreeMap<String, Value>, Vec<String>) {
    let mut resolved = BTreeMap::new();
    let mut complaints = Vec::new();
    for (name, spec) in specs {
        if published_names().contains(&name.as_str()) {
            complaints.push(format!(
                "{name} is a name this build already publishes, so your own {name} was not used - \
                 call it something else"
            ));
            continue;
        }
        match resolve(&Spec::parse(spec), world) {
            Ok(value) => {
                resolved.insert(name.clone(), value);
            }
            Err(problem) => complaints.push(format!("{name}: {problem}")),
        }
    }
    (resolved, complaints)
}

/// The contents of `endpoints.json`, for the endpoints given.
///
/// A token is written back exactly as it was read. This is the only place that has to be careful with it: the
/// file is where a credential lives, and a writer that dropped a field it did not recognise would silently
/// strip the credentials every networked source needs.
pub fn endpoints_to_json(endpoints: &BTreeMap<String, Endpoint>) -> String {
    let mut object = serde_json::Map::new();
    for (name, endpoint) in endpoints {
        let mut entry = serde_json::Map::new();
        entry.insert(
            "url".to_string(),
            serde_json::Value::String(endpoint.url.clone()),
        );
        if let Some(token) = &endpoint.token {
            entry.insert(
                "token".to_string(),
                serde_json::Value::String(token.clone()),
            );
        }
        if let Some(timeout) = endpoint.timeout {
            entry.insert("timeout".to_string(), serde_json::json!(timeout));
        }
        // written only when it is asked for: absent already means "check the certificate", which is the safe
        // reading of a file that does not mention it
        if endpoint.insecure {
            entry.insert("insecure".to_string(), serde_json::Value::Bool(true));
        }
        object.insert(name.clone(), serde_json::Value::Object(entry));
    }
    let mut text = serde_json::to_string_pretty(&serde_json::Value::Object(object))
        .unwrap_or_else(|_| "{}".to_string());
    text.push('\n');
    text
}

/// Write `endpoints.json`, beside and then into place, for its owner alone.
///
/// A plain write truncates first, so anything reading the file at that moment sees an empty or half-written
/// one - and what reads this file is a renderer redrawing a screen. It holds a bearer token, so it is written
/// `0600`: a file holding a credential is created with its mode rather than corrected afterwards. Both of those
/// are one job in one place - `g13-files` - and are tested there rather than here.
pub fn save_endpoints(
    path: &std::path::Path,
    endpoints: &BTreeMap<String, Endpoint>,
) -> Result<(), String> {
    g13_files::write_owner_only(path, &endpoints_to_json(endpoints))
}

/// Parse `endpoints.json`: `{"name": {"url": "...", "token": "...", "timeout": 3}}`.
///
/// Parsed as JSON rather than by hand: a hand-written reader handled the file's own layout and nothing else,
/// which is not a property worth having in a file a person edits.
pub fn parse_endpoints(text: &str) -> BTreeMap<String, Endpoint> {
    let mut endpoints = BTreeMap::new();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return endpoints;
    };
    for (name, entry) in value.as_object().into_iter().flatten() {
        let endpoint = Endpoint {
            url: entry
                .get("url")
                .and_then(|url| url.as_str())
                .unwrap_or_default()
                .to_string(),
            token: entry
                .get("token")
                .and_then(|token| token.as_str())
                .map(str::to_string),
            timeout: entry.get("timeout").and_then(|timeout| timeout.as_f64()),
            insecure: entry
                .get("insecure")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        };
        if !endpoint.url.is_empty() {
            endpoints.insert(name.clone(), endpoint);
        }
    }
    endpoints
}

/// Read an HTTP source: `http:<endpoint>/<path>#<field>`.
///
/// The endpoint is a name in `endpoints.json`, so an applet never carries a host or a token itself; that is
/// what makes the file shareable and what keeps credentials out of the applets.
fn http_get(spec: &Spec, world: &World) -> Result<Value, Problem> {
    let (name, path) = match spec.rest.split_once('/') {
        Some((name, path)) => (name, path),
        None => (spec.rest.as_str(), ""),
    };
    let Some(endpoint) = world.endpoints.get(name) else {
        return Err(format!(
            "no endpoint called {name} in endpoints.json (it has: {})",
            if world.endpoints.is_empty() {
                "none".to_string()
            } else {
                world
                    .endpoints
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
    };
    let url = format!(
        "{}/{}",
        endpoint.url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let timeout = Duration::from_secs_f64(endpoint.timeout.unwrap_or(3.0).clamp(0.5, 30.0));
    let mut config = ureq::Agent::config_builder().timeout_global(Some(timeout));
    if endpoint.insecure {
        // the endpoint asks for the certificate not to be checked. That is the endpoint owner's decision,
        // written in their own file, and it is needed in practice: wttr.in's certificate has been expired
        // for hours at a time, and the alternative is an applet that shows nothing.
        config = config.tls_config(
            ureq::tls::TlsConfig::builder()
                .disable_verification(true)
                .build(),
        );
    }
    let agent: ureq::Agent = config.build().into();

    let mut request = agent.get(&url);
    if let Some(token) = &endpoint.token {
        // the scheme the endpoints file implies: a bearer token, which is what GitHub and the rest expect
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let body = match request.call() {
        Ok(mut response) => response
            .body_mut()
            .read_to_string()
            .map_err(|error| format!("{url}: could not read the reply: {error}"))?,
        Err(error) => return Err(format!("{url}: {error}")),
    };

    match spec.path.as_deref() {
        Some(path) => Ok(match json_field(&body, path) {
            Some(found) => Value::Text(found),
            None => Value::Missing,
        }),
        None => Ok(Value::Text(body.trim().to_string())),
    }
}

/// Read a regex source: `regex:<file>#<pattern>`, taking the first capture group, or the whole match.
fn regex_file(spec: &Spec, world: &World) -> Result<Value, Problem> {
    let Some(pattern) = spec.path.as_deref() else {
        return Err("a regex source needs a pattern after #".to_string());
    };
    let file = if PathBuf::from(&spec.rest).is_absolute() {
        PathBuf::from(&spec.rest)
    } else {
        world.root.join(&spec.rest)
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(_) => return Ok(Value::Missing),
    };
    let expression = regex::Regex::new(pattern).map_err(|error| format!("bad pattern: {error}"))?;
    match expression.captures(&text) {
        Some(captures) => {
            let found = captures
                .get(1)
                .or_else(|| captures.get(0))
                .map(|capture| capture.as_str().to_string())
                .unwrap_or_default();
            Ok(Value::Text(found))
        }
        None => Ok(Value::Missing),
    }
}

/// Read a source. Local kinds only; a network kind is reported rather than attempted.
pub fn resolve(spec: &Spec, world: &World) -> Result<Value, Problem> {
    match spec.kind.as_str() {
        // a name out of the catalogue, read here if it can be, otherwise read back from the running driver
        "built-in" => {
            let name = spec.rest.trim();
            // the user's own values first. They cannot take a published name - that is refused when the file is read -
            // so the two sets never overlap and this order is about which lookup is cheaper, not about precedence.
            if let Some(value) = world.named.get(name) {
                return Ok(value.clone());
            }
            if published_names().contains(&name) {
                Ok(built_in_value(name, world))
            } else {
                // a misspelling used to be a blank on the pad and nothing anywhere else
                Err(format!(
                    "there is no value called `{name}`. This build publishes: {}{}",
                    published_names().join(", "),
                    match world.named.is_empty() {
                        true => String::new(),
                        false => format!(
                            ". Yours are: {}",
                            world.named.keys().cloned().collect::<Vec<_>>().join(", ")
                        ),
                    }
                ))
            }
        }
        "env" => match std::env::var(&spec.rest) {
            Ok(value) => Ok(Value::Text(value)),
            Err(_) => Ok(Value::Missing),
        },
        "file" => match std::fs::read_to_string(real_path(&spec.rest)) {
            Ok(text) => Ok(Value::Text(text.trim_end().to_string())),
            Err(_) => Ok(Value::Missing),
        },
        "json" => read_json(&real_path(&spec.rest), spec.path.as_deref(), world),
        "cmd" => run_command(&spec.rest, world),
        "http" => http_get(spec, world),
        "regex" => regex_file(spec, world),
        "mqtt" | "ws" | "imap" => Err(format!(
            "{} is not resolved yet: it needs a connection this build does not open",
            spec.kind
        )),
        other => Err(format!("unknown source kind {other}")),
    }
}

/// Pull a dotted path out of JSON text. Only the shapes the applets use are supported: objects and arrays,
/// walked by name and by index.
pub fn json_field(text: &str, path: &str) -> Option<String> {
    json_value(text, path).map(|value| value.text())
}

/// The same walk, keeping what the JSON says rather than flattening it to text.
///
/// A number read out of a file is a number, so an applet's `{cpu:.0f}` rounds it. Flattening it meant the whole
/// `47.265625` was printed and ran over whatever was beside it.
pub fn json_value(text: &str, path: &str) -> Option<Value> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut current = &value;
    for step in path.split('.').filter(|step| !step.is_empty()) {
        current = match step.parse::<usize>() {
            Ok(index) => current.get(index)?,
            Err(_) => current.get(step)?,
        };
    }
    Some(match current {
        serde_json::Value::Number(number) => Value::Number(number.as_f64()?),
        serde_json::Value::Null => return None,
        serde_json::Value::String(text) => Value::Text(text.clone()),
        serde_json::Value::Bool(flag) => Value::Text(flag.to_string()),
        other => Value::Text(other.to_string()),
    })
}

/// Read a file the world names as JSON and take one value out of it, or the whole file's text when there is no
/// path to take.
fn read_json(file: &str, path: Option<&str>, world: &World) -> Result<Value, Problem> {
    // a relative path is read from the world's root, which is what makes a fixture directory possible
    let file = if PathBuf::from(file).is_absolute() {
        PathBuf::from(file)
    } else {
        world.root.join(file)
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        // a game that is not running is a normal state, not a failure to shout about
        Err(_) => return Ok(Value::Missing),
    };
    match path {
        Some(path) => Ok(json_value(&text, path).unwrap_or(Value::Missing)),
        None => Ok(Value::Text(text.trim_end().to_string())),
    }
}

/// Run a command and take its standard output.
///
/// The exit status is deliberately not treated as a failure: `wc -l` on an empty list exits 0 with `0`, and
/// a command that legitimately prints nothing is not an error. What comes out is what appears.
fn run_command(command: &str, world: &World) -> Result<Value, Problem> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = match std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return Err(format!("could not run {command:?}: {error}")),
    };

    // read on another thread, so a command that prints more than a pipe holds cannot deadlock the wait
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    });

    // and a deadline, because a source that never returns would freeze the screen rather than show nothing
    let deadline = std::time::Instant::now() + world.command_timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(format!(
                        "{command:?} took longer than {}ms and was stopped",
                        world.command_timeout.as_millis()
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(format!("could not wait for {command:?}: {error}")),
        }
    }

    let text = reader.join().unwrap_or_default().trim().to_string();
    if text.is_empty() {
        Ok(Value::Missing)
    } else if let Ok(number) = text.parse::<f64>() {
        Ok(Value::Number(number))
    } else {
        Ok(Value::Text(text))
    }
}

/// The names a format string asks for, in the order they appear and without repeats.
///
/// `"cpu {cpu:.0f}% of {memory}"` gives `cpu`, `memory`. A `{` with no `}` is ignored, exactly as `fill` leaves
/// it alone, so a half-typed format cannot make this run off the end.
pub fn names_in_format(format: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut rest = format;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        let token = &rest[start + 1..start + end];
        let name = token.split(':').next().unwrap_or(token).trim();
        if !name.is_empty() && !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
        rest = &rest[start + end + 1..];
    }
    names
}

/// One value this build publishes, and what it means.
///
/// This is the whole catalogue, in one place, because it is the thing a person has to be able to look at: what
/// can I use, what does it do, and where does it come from. `computed` separates the two ways a value arrives -
/// read out of the machine on the spot (`cmd`-like), or reported by the running driver, which is the only thing
/// that knows what is on the pad, what the stick is doing, and which profile is active.
pub struct Published {
    /// The name an applet writes, e.g. `cpu` or `temp`.
    pub name: &'static str,
    /// One line saying what it is and where it comes from, for the catalogue.
    pub meaning: &'static str,
    /// True when it is read out of the machine here and now; false when only the running driver knows it.
    pub computed: bool,
}

/// Every value this build publishes, with its meaning. The window shows this list.
pub const PUBLISHED: [Published; 32] = [
    Published {
        name: "cpu",
        meaning: "how busy the processor is, sampled over the last second",
        computed: true,
    },
    Published {
        name: "memory",
        meaning: "how much of the memory is in use, per cent",
        computed: true,
    },
    Published {
        name: "load",
        meaning: "the one-minute load average",
        computed: true,
    },
    Published {
        name: "host",
        meaning: "this machine's name",
        computed: true,
    },
    Published {
        name: "uptime",
        meaning: "since the last boot, as 9d 17h",
        computed: true,
    },
    Published {
        name: "uptime_seconds",
        meaning: "since the last boot, in seconds, for arithmetic",
        computed: false,
    },
    Published {
        name: "day",
        meaning: "the day of the week, as Tue",
        computed: true,
    },
    Published {
        name: "date",
        meaning: "the day and the month, as 15 Sep",
        computed: true,
    },
    Published {
        name: "date_iso",
        meaning: "the date in ISO 8601, as 2026-09-18, for comparing",
        computed: true,
    },
    Published {
        name: "time",
        meaning: "the clock, as 21:14",
        computed: true,
    },
    Published {
        name: "time_seconds",
        meaning: "seconds since midnight, for arithmetic",
        computed: false,
    },
    Published {
        name: "media_title",
        meaning: "the track playing now",
        computed: true,
    },
    Published {
        name: "media_artist",
        meaning: "who is playing it",
        computed: true,
    },
    Published {
        name: "media_album",
        meaning: "the album it is from",
        computed: true,
    },
    Published {
        name: "media_status",
        meaning: "Playing, Paused or Stopped",
        computed: true,
    },
    Published {
        name: "media_position",
        meaning: "how far into the track, as 1:23",
        computed: true,
    },
    Published {
        name: "media_duration",
        meaning: "how long the track is, as 3:45",
        computed: true,
    },
    Published {
        name: "media_percent",
        meaning: "how far through the track, per cent",
        computed: true,
    },
    Published {
        name: "profile",
        meaning: "which of the profiles is active, 0 to 3",
        computed: false,
    },
    Published {
        name: "button_mode",
        meaning: "what the M keys are set to do",
        computed: false,
    },
    Published {
        name: "recording",
        meaning: "1 while a recording is being made, else 0",
        computed: false,
    },
    Published {
        name: "last_key",
        meaning: "the last control pressed on the pad",
        computed: false,
    },
    Published {
        name: "recent_keys",
        meaning: "the last few presses, newest first",
        computed: false,
    },
    Published {
        name: "screen_visual",
        meaning: "what is on the pad, as applet:docker",
        computed: false,
    },
    Published {
        name: "screen_owner",
        meaning: "which program is drawing the pad",
        computed: false,
    },
    Published {
        name: "screen_width",
        meaning: "the screen's width in pixels",
        computed: false,
    },
    Published {
        name: "screen_height",
        meaning: "the screen's height in pixels",
        computed: false,
    },
    Published {
        name: "pad_down",
        meaning: "which controls are down, by name - `G1,L2,MR`",
        computed: true,
    },
    Published {
        name: "stick_x",
        meaning: "the stick across, 0 to 255",
        computed: false,
    },
    Published {
        name: "stick_y",
        meaning: "the stick up and down, 0 to 255",
        computed: false,
    },
    Published {
        name: "stick_raw",
        meaning: "the stick's two bytes exactly as the pad sent them",
        computed: false,
    },
    Published {
        name: "sdk_client",
        meaning: "published for the previous stack's reader; this build does not fill it in yet",
        computed: false,
    },
];

/// The names read out of the machine here and now.
pub fn computed_names() -> Vec<&'static str> {
    PUBLISHED
        .iter()
        .filter(|value| value.computed)
        .map(|value| value.name)
        .collect()
}

/// Every name this build answers to, whether it is computed here or reported by the driver.
pub fn published_names() -> Vec<&'static str> {
    PUBLISHED.iter().map(|value| value.name).collect()
}

/// Every built-in name. The resolver and the gatherer both read this list, so a name cannot be added to one
/// without being added to the other - which is exactly how the media values first came out empty.
/// The names read out of the machine here and now, as a fixed list.
pub const BUILT_IN_NAMES: [&str; 16] = [
    "cpu",
    "memory",
    "load",
    "uptime",
    "uptime_seconds",
    "day",
    "date",
    "date_iso",
    "time",
    "time_seconds",
    "host",
    "media_title",
    "media_artist",
    "media_album",
    "media_status",
    "media_position",
];

/// The values that move as fast as the pad does, so anything drawing them must be given them fresh.
///
/// One list, here, because two would drift: the driver's own publish path, the screen worker and any applet
/// that names one all ask this question, and the answer has to be the same in all three. They are read out of
/// the published file, which the driver rewrites whenever the pad changes; the machine's own numbers are not
/// here because they cost a great deal to gather (about a fifth of a second) and change once a second.
pub const MOVES_WITH_THE_PAD: &[&str] = &["pad_down", "stick_x", "stick_y"];

/// Whether a value is one of those.
pub fn changes_every_frame(name: &str) -> bool {
    MOVES_WITH_THE_PAD.contains(&name)
}

/// A path written in a config file, with `~` and `$VAR` expanded.
///
/// An applet is written once and read for years, so `/run/user/1000/...` in one is a latent bug - 1000 is
/// whoever happened to write it. `$XDG_RUNTIME_DIR` and `~` say the same thing without a uid in it, and a path
/// is the one place where reading an environment variable is what the writer meant.
pub fn real_path(path: &str) -> String {
    let path = path.trim();
    let home = std::env::var("HOME").ok();
    let expanded = match (path, &home) {
        ("~", Some(home)) => home.clone(),
        _ => match (path.strip_prefix("~/"), &home) {
            (Some(rest), Some(home)) => format!("{home}/{rest}"),
            _ => path.to_string(),
        },
    };
    let mut out = String::new();
    let mut rest = expanded.as_str();
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let (name, tail, braced) = match after.strip_prefix('{') {
            Some(braced_part) => match braced_part.split_once('}') {
                Some((name, tail)) => (name, tail, true),
                None => {
                    out.push('$');
                    out.push_str(after);
                    return out;
                }
            },
            None => {
                let end = after
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(after.len());
                (&after[..end], &after[end..], false)
            }
        };
        if name.is_empty() {
            out.push('$');
        } else {
            match std::env::var(name) {
                Ok(value) => out.push_str(&value),
                // a name that is not set stays exactly as written, braces included, so a mistake is visible
                // rather than quietly turning into a path that cannot exist
                Err(_) => {
                    out.push('$');
                    if braced {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    } else {
                        out.push_str(name);
                    }
                }
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// This world's clock: the reading it was built with, or a fresh one if it was built without.
///
/// A world built by hand - in a test, or by a one-shot tool - has no reading, and reading one when a clock value
/// is asked for keeps that correct without every caller having to know about the clock at all. The driver's own
/// path fills it once per gather, so nothing there reads the clock twice for one screen.
fn clock_of(world: &World) -> Clock {
    if world.clock.is_empty() {
        Clock::read()
    } else {
        world.clock.clone()
    }
}

/// One of the machine's own readings, read out of the machine.
///
/// The driver gathers these for its own visuals and publishes them; an applet reaches them by reading the
/// published file (`json:$XDG_RUNTIME_DIR/g13-values.json#cpu`), which is a mechanism like any other. Nothing
/// here is available to an applet by name.
pub fn built_in_value(name: &str, world: &World) -> Value {
    match name {
        "cpu" => cpu_percent(world),
        "memory" => memory_percent(world),
        "uptime" => uptime(world),
        // One reading serves all four: they are the same instant, and reading the clock once per value can
        // return two different minutes either side of a boundary - or two different days at midnight.
        "day" => Value::Text(clock_of(world).day),
        "date" => Value::Text(clock_of(world).date),
        "date_iso" => Value::Text(clock_of(world).iso),
        "time" => Value::Text(clock_of(world).time),
        "load" => load_average(world),
        // what is playing, through playerctl, which speaks MPRIS. A machine with no player, or without
        // playerctl, gets Missing rather than an invented value.
        "media_title" => reading("media_title"),
        "media_artist" => reading("media_artist"),
        "media_album" => reading("media_album"),
        "media_status" => reading("media_status"),
        "media_url" => reading("media_url"),
        "media_position" => media_now().0,
        "media_duration" => reading("media_duration"),
        "media_percent" => media_now().1,
        // names the gatherer asks for but that need a second value, or are filled in by the agent
        // `time_seconds` and `uptime_seconds` are published by the driver as numbers, not computed here
        "uptime_seconds" | "time_seconds" => published_value(name, world),
        "host" => Value::Text(
            std::fs::read_to_string("/etc/hostname")
                .map(|name| name.trim().to_string())
                .unwrap_or_default(),
        ),
        // Everything else in the catalogue is what the running driver knows and nothing else can: the stick,
        // the profile, what is on the pad. It is read back out of the file the driver publishes, so `{stick_x}`
        // works in an applet by name rather than by spelling out a runtime path.
        other if published_names().contains(&other) => published_value(other, world),
        _ => Value::Missing,
    }
}

/// A name only the running driver knows, read out of the file it publishes.
fn published_value(name: &str, world: &World) -> Value {
    let Some(path) = &world.values else {
        return Value::Missing;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Value::Missing;
    };
    json_value(&text, name).unwrap_or(Value::Missing)
}

/// The load average, which the values file publishes as `load`.
fn load_average(world: &World) -> Value {
    match std::fs::read_to_string(world.root.join("proc/loadavg")) {
        Ok(text) => Value::Number(
            text.split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0),
        ),
        Err(_) => Value::Missing,
    }
}

/// Run a command and wait for it, but not for ever.
///
/// `Command::output()` waits as long as the child takes, and a player that has stopped answering its D-Bus
/// call never returns - so the thing doing the waiting stops too. Measured: a renderer stalled for over a
/// minute with nothing drawn, and the same stall would stop the driver drawing on the pad. Every command
/// that produces a value is run through here for that reason.
fn run_within(mut command: Command, within: Duration) -> Option<std::process::Output> {
    use std::process::Stdio;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let deadline = std::time::Instant::now() + within;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                return None;
            }
        }
        if std::time::Instant::now() >= deadline {
            // killed rather than left running: a child that never returns would otherwise outlive us
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// How long a command is given. Long enough for a player that is merely busy, short enough that a reader
/// refreshing several times a second is never held up by one that has stopped answering.
fn command_limit() -> Duration {
    // Measured: 500 ms was not enough for playerctl under load, and the value came back missing on a machine
    // that had a player. This bounds a call that has stopped answering without cutting off one that is busy.
    Duration::from_millis(1500)
}

/// Ask playerctl for one field of the playing track.
///
/// `--all-players` is used so that a player which is not the default still counts; a machine with nothing
/// playing prints nothing, which is Missing.
/// The position and the percentage to show **now**: the last reading the player gave, carried forward by this
/// machine's clock.
///
/// A bar drawn straight from MPRIS steps once a second, which is the fault this fixes. While the player is playing,
/// the position is known to be advancing at a second a second, so it is carried here and re-synced the moment the
/// player's own answer *changes*. **Deliberately not part of `media_readings`:** that cache holds what the player
/// last said, and this is what to draw, which is a question asked twenty times a second. Putting the carry inside
/// the cache was the first version's flaw - it advanced only on a cache miss, so it stepped five times a second
/// instead of sweeping.
fn media_now() -> (Value, Value) {
    let readings = media_readings();
    let playing = readings
        .get("media_status")
        .map(|status| status.text().trim().eq_ignore_ascii_case("Playing"))
        .unwrap_or(false);
    let track = readings
        .get("media_title")
        .map(|title| title.text())
        .unwrap_or_default();
    // one carry for the driver, in the process rather than in the readings cache
    static MOVING: std::sync::OnceLock<std::sync::Mutex<Option<Moving>>> =
        std::sync::OnceLock::new();
    let mut state = match MOVING.get_or_init(|| std::sync::Mutex::new(None)).lock() {
        Ok(held) => held,
        Err(_) => return (Value::Missing, Value::Missing),
    };
    let seconds = moving_seconds(
        &mut state,
        media_number(&readings, "media_seconds")
            .filter(|value| *value >= 0.0)
            .map(|micros| micros / 1_000_000.0),
        playing,
        &track,
        media_number(&readings, "media_length")
            .filter(|value| *value > 0.0)
            .map(|micros| micros / 1_000_000.0),
    );
    let length = media_number(&readings, "media_length")
        .filter(|value| *value > 0.0)
        .map(|micros| micros / 1_000_000.0);
    let position = match seconds {
        Some(seconds) => media_time_from(seconds * 1_000_000.0),
        None => Value::Missing,
    };
    let percent = match (seconds, length) {
        (Some(seconds), Some(length)) if length > 0.0 => {
            Value::Number((seconds / length * 100.0).clamp(0.0, 100.0))
        }
        _ => Value::Missing,
    };
    (position, percent)
}

/// The last position the player gave, when, and whether it was moving.
struct Moving {
    /// When the player's answer arrived, which the carry forward is measured from.
    asked: std::time::Instant,
    /// The position that answer gave, in seconds.
    seconds: f64,
    /// The same position exactly as the player gave it, so a repeated reading can be told from a new one.
    origin: f64,
    /// Whether the player said the track was playing when it gave the position.
    playing: bool,
    /// The track the position belongs to, so a new track does not inherit the last one's carry.
    track: String,
}

/// Where the track has got to, as far as it can be known.
///
/// Nothing here is invented: the player's own answer is the base, the clock only moves it forward, it never runs
/// past the length of the track, a paused player does not move, and a track nothing is known about starts from what
/// the player says rather than from the last one's position.
///
/// `origin` is the last answer *the player gave*, and it exists so that a **repeated** reading - the same number
/// arriving again, which is what the cache does twenty times a second - carries on instead of re-seeding and
/// throwing the carry away. Only a *changed* answer re-seeds.
fn moving_seconds(
    state: &mut Option<Moving>,
    real: Option<f64>,
    playing: bool,
    track: &str,
    length: Option<f64>,
) -> Option<f64> {
    let held = &mut *state;
    let same_track = held
        .as_ref()
        .map(|moving| moving.track == track)
        .unwrap_or(false);
    let same_answer = match (real, held.as_ref()) {
        (Some(real), Some(moving)) => moving.origin == real,
        _ => false,
    };
    if let (Some(real), _, false) = (real, same_track, same_answer) {
        *held = Some(Moving {
            asked: std::time::Instant::now(),
            seconds: real,
            origin: real,
            playing,
            track: track.to_string(),
        });
        return Some(real);
    }
    if !same_track {
        return None;
    }
    let moving = held.as_ref()?;
    let carried = match moving.playing {
        true => {
            let advanced = moving.seconds + moving.asked.elapsed().as_secs_f64();
            match length {
                Some(length) if length > 0.0 => advanced.min(length),
                _ => advanced,
            }
        }
        false => moving.seconds,
    };
    Some(carried)
}

/// One reading about what is playing, from a single call to `playerctl`.
///
/// **No player is ever named.** `--all-players` returns a line per player, which is what this always did, and each
/// line carries its own status - so the player that is *playing* is chosen **from the output itself**. Naming one
/// with `-p` is what broke this on 2026-09-17: with nothing playing, the choice fell to the first player listed,
/// which was a stopped `chromium`, and `playerctl` answered *"No player could handle this command"* - so every
/// media reading went empty at once and a screen full of text went blank.
///
/// **The rule that keeps it working: never narrow a source to something that might not be able to answer. Keep
/// what already worked as the fallback.**
fn reading(name: &str) -> Value {
    media_readings()
        .get(name)
        .cloned()
        .unwrap_or(Value::Missing)
}

/// Every reading about what is playing, in one call, kept briefly.
///
/// The position is read from the player's **metadata**, not from MPRIS's `Position` property: Firefox implements
/// the former and not the latter, so asking for the property returned a stubborn `0.000000` and a media applet sat
/// at `0:00` and `0%` - while the metadata answered `409000015` microseconds.
///
/// One call, because seven of them cost 51ms and stalled the drawing once a second. The fields are joined by a tab
/// with the status first, so every line says who is speaking and whether they are playing.
fn media_readings() -> BTreeMap<String, Value> {
    static KEPT: std::sync::OnceLock<
        std::sync::Mutex<(std::time::Instant, BTreeMap<String, Value>)>,
    > = std::sync::OnceLock::new();
    let kept = KEPT.get_or_init(|| {
        std::sync::Mutex::new((
            std::time::Instant::now() - std::time::Duration::from_secs(60),
            BTreeMap::new(),
        ))
    });
    if let Ok(held) = kept.lock()
        && held.0.elapsed() < std::time::Duration::from_millis(200)
    {
        return held.1.clone();
    }
    let mut values = BTreeMap::new();
    let fields = [
        ("media_status", "{{status}}"),
        ("media_title", "{{xesam:title}}"),
        ("media_artist", "{{xesam:artist}}"),
        ("media_album", "{{xesam:album}}"),
        // the track's own address, which Firefox publishes and nothing was reading
        ("media_url", "{{xesam:url}}"),
        // the raw numbers, so the bar and the clock are not a second behind
        ("media_seconds", "{{position}}"),
        ("media_length", "{{mpris:length}}"),
    ];
    let format = fields
        .iter()
        .map(|(_, field)| *field)
        .collect::<Vec<_>>()
        .join("\t");
    let mut command = Command::new("playerctl");
    command.args(["--all-players", "metadata", "--format", &format]);
    let text = match run_within(command, command_limit()) {
        Some(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => String::new(),
    };
    // one line per player: the one that is playing is the one to believe, and otherwise the first that answered -
    // which is exactly what this did before anything chose a player for it
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let chosen = chosen_line(&lines);
    if let Some(line) = chosen {
        for ((name, _), part) in fields.iter().zip(line.split('\t')) {
            let part = part.trim();
            if !part.is_empty() {
                values.insert(name.to_string(), Value::Text(part.to_string()));
            }
        }
        // the player gives microseconds, and everything below is seconds: dividing here, once, is the whole of it
        let seconds = media_number(&values, "media_seconds")
            .filter(|value| *value >= 0.0)
            .map(|micros| micros / 1_000_000.0);
        let length = media_number(&values, "media_length")
            .filter(|value| *value > 0.0)
            .map(|micros| micros / 1_000_000.0);
        if let Some(seconds) = seconds {
            values.insert(
                "media_position".to_string(),
                media_time_from(seconds * 1_000_000.0),
            );
        }
        if let Some(length) = length {
            values.insert(
                "media_duration".to_string(),
                media_time_from(length * 1_000_000.0),
            );
        }
        if let (Some(seconds), Some(length)) = (seconds, length) {
            values.insert(
                "media_percent".to_string(),
                Value::Number((seconds / length * 100.0).clamp(0.0, 100.0)),
            );
        }
    }
    if let Ok(mut held) = kept.lock() {
        *held = (std::time::Instant::now(), values.clone());
    }
    values
}

/// Which of `playerctl`'s lines to believe, out of one line per player.
///
/// The line that is **playing** first, and otherwise the first line that answered - which is what this did before
/// anything chose a player for it. The status is the first field of each line, which is why the format asks for it
/// first.
///
/// This is a function of its own, with its own test, because choosing *which* source to read is the part that can
/// be wrong while every example of reading it looks right: on 2026-09-17 a change here picked a stopped player and
/// emptied every media reading at once.
fn chosen_line<'a>(lines: &[&'a str]) -> Option<&'a str> {
    lines
        .iter()
        .find(|line| {
            line.split('\t')
                .next()
                .map(|status| status.trim().eq_ignore_ascii_case("Playing"))
                .unwrap_or(false)
        })
        .or_else(|| lines.first())
        .copied()
}

/// A raw number the player gave, in microseconds.
fn media_number(values: &BTreeMap<String, Value>, name: &str) -> Option<f64> {
    values
        .get(name)
        .and_then(|value| value.text().parse::<f64>().ok())
        .filter(|number| number.is_finite())
}

/// A position given in microseconds as `M:SS`, or Missing when it is not a real, non-negative number.
fn media_time_from(micros: f64) -> Value {
    let seconds = micros / 1_000_000.0;
    match seconds.is_finite() && seconds >= 0.0 {
        true => {
            let total = seconds.round() as u64;
            Value::Text(format!("{}:{:02}", total / 60, total % 60))
        }
        false => Value::Missing,
    }
}

/// One reading of the clock, in one call.
///
/// Everything here is the *same instant*. Reading the clock five times for five values can return two different
/// minutes either side of a boundary - and two different days at midnight - so a screen showing the date and the
/// time could be showing two moments. One read, then everything derived from it.
///
/// The textual forms are ISO 8601 (`2026-09-18`), which sorts correctly as text and is what anything else reading
/// these is expecting. `seconds` is the true count since local midnight, not a rounded display string: the display
/// is `HH:MM`, and a value claiming to be seconds has to come from a reading that has them.
#[derive(Debug, Clone, Default)]
pub struct Clock {
    /// `2026-09-18`
    pub iso: String,
    /// `10:58`
    pub time: String,
    /// Seconds since local midnight, from a reading that has them.
    pub seconds: u32,
    /// `18 Sep`, for the screen.
    pub date: String,
    /// `Fri`, for the screen.
    pub day: String,
}

impl Clock {
    /// Read it, once.
    pub fn read() -> Self {
        let line = clock("+%Y-%m-%d|%H:%M|%H:%M:%S|%d %b|%a");
        let mut parts = line.split('|');
        let iso = parts.next().unwrap_or_default().trim().to_string();
        let time = parts.next().unwrap_or_default().trim().to_string();
        let with_seconds = parts.next().unwrap_or_default().trim();
        let date = parts.next().unwrap_or_default().trim().to_string();
        let day = parts.next().unwrap_or_default().trim().to_string();
        Self {
            iso,
            time,
            seconds: seconds_of(with_seconds),
            date,
            day,
        }
    }

    /// Nothing was read. A world built without a clock reads one when it is asked.
    pub fn is_empty(&self) -> bool {
        self.iso.is_empty() && self.time.is_empty()
    }
}

/// `10:58:07` as seconds since midnight.
fn seconds_of(clock: &str) -> u32 {
    let mut parts = clock.split(':');
    let hours: u32 = parts
        .next()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(0);
    let minutes: u32 = parts
        .next()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(0);
    let seconds: u32 = parts
        .next()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(0);
    hours * 3600 + minutes * 60 + seconds
}

/// The clock as `date` formats it, or an empty string when `date` does not answer in time.
fn clock(format: &str) -> String {
    let mut command = Command::new("date");
    command.arg(format);
    match run_within(command, command_limit()) {
        Some(output) => String::from_utf8_lossy(&output.stdout).trim().to_string(),
        None => String::new(),
    }
}

/// How long the machine has been up, out of `/proc/uptime`, as days and hours or hours and minutes.
fn uptime(world: &World) -> Value {
    let path = world.root.join("proc/uptime");
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let seconds: f64 = text
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0);
            let days = (seconds / 86400.0).floor();
            let hours = ((seconds % 86400.0) / 3600.0).floor();
            let minutes = ((seconds % 3600.0) / 60.0).floor();
            if days > 0.0 {
                Value::Text(format!("{}d {}h", days as u64, hours as u64))
            } else {
                Value::Text(format!("{}h {}m", hours as u64, minutes as u64))
            }
        }
        Err(_) => Value::Missing,
    }
}

/// The CPU as it is *now*, not as it has averaged since boot.
///
/// `/proc/stat` is cumulative, so a single reading divided by another single reading gives the average since
/// the machine started  -  which hardly moves, and looks like a frozen display. The busy share is the
/// difference between two readings, so the previous one is kept and each call reports the interval since it.
fn cpu_percent(world: &World) -> Value {
    use std::sync::Mutex;
    use std::sync::OnceLock;

    /// The last reading: total jiffies, idle jiffies, and when it was taken.
    struct Sample {
        total: u64,
        idle: u64,
        at: std::time::Instant,
        /// The last share worked out, kept so a reading that cannot be computed still has an answer.
        percent: f64,
    }

    static LAST: OnceLock<Mutex<Option<Sample>>> = OnceLock::new();
    let last = LAST.get_or_init(|| Mutex::new(None));

    let Some(now) = read_cpu_jiffies(world) else {
        return Value::Missing;
    };

    let previous = {
        let mut guard = match last.lock() {
            Ok(guard) => guard,
            Err(_) => return Value::Missing,
        };
        let previous = guard.take();
        *guard = Some(Sample {
            total: now.0,
            idle: now.1,
            at: std::time::Instant::now(),
            percent: previous
                .as_ref()
                .map(|sample| sample.percent)
                .unwrap_or(0.0),
        });
        previous
    };

    let Some(previous) = previous else {
        // nothing to compare against yet. Wait for the counters to move rather than sleeping a fixed time:
        // two readings inside the same jiffy differ by nothing, and dividing by that gives no answer at all.
        // A wait here costs one frame once, and without it the first reading would be the since-boot average.
        let mut second = read_cpu_jiffies(world);
        let mut waited = 0_u64;
        while waited < 500 && second.map(|(total, _)| total <= now.0).unwrap_or(true) {
            std::thread::sleep(Duration::from_millis(20));
            waited += 20;
            second = read_cpu_jiffies(world);
        }
        let Some(second) = second else {
            return Value::Missing;
        };
        let (total, idle) = (
            second.0.saturating_sub(now.0),
            second.1.saturating_sub(now.1),
        );
        if total == 0 {
            // the counters did not move, which on a running machine means something else took the sample
            return Value::Missing;
        }
        let percent = 100.0 * (total - idle) as f64 / total as f64;
        let mut guard = match last.lock() {
            Ok(guard) => guard,
            Err(_) => return Value::Missing,
        };
        *guard = Some(Sample {
            total: second.0,
            idle: second.1,
            at: std::time::Instant::now(),
            percent,
        });
        return Value::Number(percent);
    };

    let total = now.0.saturating_sub(previous.total);
    let idle = now.1.saturating_sub(previous.idle);
    let _ = previous.at;
    if total == 0 {
        // two readings close enough together that the counters did not move. The last answer is a fact about
        // this machine a moment ago; a missing value is not, and this happens whenever two callers ask at
        // once, which is exactly what a parallel test run does.
        return if previous.percent > 0.0 || previous.total > 0 {
            Value::Number(previous.percent)
        } else {
            Value::Missing
        };
    }
    let percent = 100.0 * (total - idle) as f64 / total as f64;
    if let Ok(mut guard) = last.lock() {
        *guard = Some(Sample {
            total: now.0,
            idle: now.1,
            at: std::time::Instant::now(),
            percent,
        });
    }
    Value::Number(percent)
}

/// The total and idle jiffies across all CPUs.
fn read_cpu_jiffies(world: &World) -> Option<(u64, u64)> {
    let text = std::fs::read_to_string(world.root.join("proc/stat")).ok()?;
    let line = text.lines().find(|line| line.starts_with("cpu "))?;
    let numbers: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|value| value.parse().ok())
        .collect();
    if numbers.len() < 4 {
        return None;
    }
    let idle = numbers[3] + numbers.get(4).copied().unwrap_or(0);
    Some((numbers.iter().sum(), idle))
}

/// The share of memory in use, from `/proc/meminfo`, as a percentage.
fn memory_percent(world: &World) -> Value {
    let text = match std::fs::read_to_string(world.root.join("proc/meminfo")) {
        Ok(text) => text,
        Err(_) => return Value::Missing,
    };
    let mut total = 0.0;
    let mut available = 0.0;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some("MemTotal:"), Some(value)) => total = value.parse().unwrap_or(0.0),
            (Some("MemAvailable:"), Some(value)) => available = value.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    if total == 0.0 {
        return Value::Missing;
    }
    Value::Number(100.0 * (total - available) / total)
}

#[cfg(test)]
mod the_list_tests {
    use super::*;

    #[test]
    fn the_pad_s_own_values_move_every_frame_and_the_machines_do_not() {
        // two things follow from this list: which values a screen may be drawn many times between readings of,
        // and which applets need the fast draw at all. Two lists would drift, so there is one.
        assert!(changes_every_frame("stick_x"));
        assert!(changes_every_frame("stick_y"));
        assert!(!changes_every_frame("cpu"));
        assert!(!changes_every_frame("media_position"));
        assert!(!changes_every_frame("time_seconds"));
        assert!(!changes_every_frame("nothing_at_all"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Two properties of a network read that the manual promises and that are not this crate's to keep: the cap
    /// on a reply and what happens to the token across a redirect are both `ureq`'s defaults. Nothing here would
    /// fail if somebody changed them - only the two sentences in the manual would quietly become untrue.
    ///
    /// The scan stops at `#[cfg(test)]`, because the words below are the very words it is looking for.
    #[test]
    fn the_reply_stays_bounded_and_a_redirect_does_not_carry_the_token() {
        let source = include_str!("lib.rs");
        let code = &source[..source.find("#[cfg(test)]").expect("the test module")];
        // the capped read: the client's own reader applies its limit, where reading a body to the end takes all
        // of it
        assert!(
            code.contains(".body_mut()") && code.contains(".read_to_string()"),
            "the reply is no longer read through the client's capped reader"
        );
        assert!(
            !code.contains("read_to_end("),
            "a reply is being read without the client's limit"
        );
        // and the token across a redirect: dropping it is the default, and nothing here overrides that
        assert!(
            !code.contains("redirect_auth_headers"),
            "the redirect's handling of the token has been overridden"
        );
        assert!(
            !code.contains("max_redirects("),
            "the redirect limit has been overridden"
        );
        // and the manual still says both, because a sentence about behaviour is the thing that goes stale
        let manual = include_str!("../../../docs/applets-and-sources.md");
        assert!(
            manual.contains("10 MB"),
            "the manual no longer says how large a reply may be"
        );
        assert!(
            manual.contains("the token is not sent on to where it"),
            "the manual no longer says what a redirect does with the token"
        );
    }
    #[test]
    fn a_position_is_read_from_the_metadata_not_the_property_firefox_does_not_implement() {
        // Firefox answers `0.000000` to `playerctl position` - MPRIS's Position property - while its metadata
        // carries the real one. Measured on this machine with Firefox playing: metadata said 409000015, the
        // property said 0. A media applet built on the property sat at 0:00 and 0% for a whole track.
        assert_eq!(media_time_from(409_000_015.0).text(), "6:49");
        assert_eq!(media_time_from(0.0).text(), "0:00");
        assert_eq!(media_time_from(125_500_000.0).text(), "2:06");
        assert_eq!(media_time_from(f64::NAN), Value::Missing);
        assert_eq!(media_time_from(-1.0), Value::Missing);
        // the length in the same units, so a percent is position over length and nothing else
        assert_eq!(media_time_from(2_125_000_000.0).text(), "35:25");
    }

    #[test]
    fn the_endpoint_a_source_names_is_read_from_the_source_itself() {
        // what the window uses to say which applets rely on which endpoint, and to notice an applet naming one
        // that does not exist
        assert_eq!(
            Spec::parse("http:weather/London?format=j1#current_condition.0.temp_C").endpoint(),
            Some("weather")
        );
        assert_eq!(
            Spec::parse("http:github/repos/x/y/issues").endpoint(),
            Some("github")
        );
        assert_eq!(
            Spec::parse("http: github /thing").endpoint(),
            Some("github")
        );
        // a source of this machine names no endpoint, and says so rather than inventing one
        assert_eq!(Spec::parse("file:/proc/loadavg").endpoint(), None);
        assert_eq!(Spec::parse("cmd:uptime").endpoint(), None);
        assert_eq!(Spec::parse("cpu").endpoint(), None);
        // and an http source with no name is not one either
        assert_eq!(Spec::parse("http:/thing").endpoint(), None);
    }

    #[test]
    fn a_token_is_called_out_when_the_link_cannot_keep_it() {
        let endpoint = |url: &str, token: Option<&str>, insecure: bool| Endpoint {
            url: url.to_string(),
            token: token.map(str::to_string),
            insecure,
            ..Endpoint::default()
        };
        // the shapes nobody means to write
        assert!(
            endpoint_risk(&endpoint("http://host", Some("a-token"), false))
                .is_some_and(|said| said.contains("anyone on the way")),
            "a token over http"
        );
        assert!(
            endpoint_risk(&endpoint("https://host", Some("a-token"), true))
                .is_some_and(|said| said.contains("certificate is not checked")),
            "a token on a link whose certificate is not checked"
        );
        // and the shapes that are fine, including the one this project ships
        assert_eq!(
            endpoint_risk(&endpoint("https://api.github.com", Some("a-token"), false)),
            None
        );
        assert_eq!(endpoint_risk(&endpoint("http://host", None, false)), None);
        assert_eq!(
            endpoint_risk(&endpoint("https://wttr.in", None, true)),
            None,
            "an insecure host with nothing to give away is not a warning"
        );
        assert_eq!(
            endpoint_risk(&endpoint("wss://host", Some("a-token"), false)),
            None,
            "wss is a checked link"
        );
        assert_eq!(
            endpoint_risk(&endpoint("https://host", Some("   "), false)),
            None,
            "an empty token is no token"
        );
        // a url that is not a url belongs to `endpoint_url_problem`, not to this
        assert_eq!(
            endpoint_risk(&endpoint("api.github.com", Some("t"), false)),
            None
        );
    }

    #[test]
    fn an_endpoint_that_is_really_a_source_is_reported() {
        // the mistake this catches: a `cmd:` line put on the endpoints tab, where it would be pasted into a url
        assert!(endpoint_url_problem("cmd:nvidia-smi --query-gpu=utilization.gpu").is_some());
        assert!(endpoint_url_problem("cmd:uptime").is_some());
        assert!(endpoint_url_problem("api.github.com").is_some());
        assert!(endpoint_url_problem("https://").is_some());
        assert!(endpoint_url_problem("ftp://host").is_some());
        // and the real ones are not
        assert_eq!(endpoint_url_problem("https://api.github.com"), None);
        assert_eq!(endpoint_url_problem("http://host:8080"), None);
        assert_eq!(endpoint_url_problem("mqtt://host"), None);
        assert_eq!(endpoint_url_problem("  https://wttr.in  "), None);
    }

    #[test]
    fn every_published_name_answers_to_its_name() {
        // The catalogue is the promise: if the page lists it, an applet can name it. A `computed` name is read
        // out of the machine; the rest need a running driver and are Missing without one - which is not the
        // same as being unknown, and an unknown name is an error with the list in it.
        let world = World::default();
        for value in PUBLISHED {
            let resolved = resolve(&Spec::parse(value.name), &world);
            assert!(
                resolved.is_ok(),
                "{} is in the catalogue but does not resolve: {resolved:?}",
                value.name
            );
            assert!(!value.meaning.is_empty(), "{} has no meaning", value.name);
        }
        // and the two lists cannot drift: computed ones are part of the catalogue
        for name in computed_names() {
            assert!(
                published_names().contains(&name),
                "{name} is computed but not published"
            );
        }
    }

    #[test]
    fn a_name_out_of_the_catalogue_is_a_source() {
        let world = World::default();
        // bare, and the long form, are the same thing
        let cpu = resolve(&Spec::parse("cpu"), &world).unwrap();
        assert!(
            cpu.number().is_some(),
            "cpu should be a number, got {cpu:?}"
        );
        assert_eq!(Spec::parse("built-in:cpu").kind, "built-in");
        assert!(
            resolve(&Spec::parse("built-in:cpu"), &world)
                .unwrap()
                .number()
                .is_some()
        );
        // and a name that is not in the catalogue is a mistake, said out loud with the list
        let problem = resolve(&Spec::parse("cpuu"), &world).unwrap_err();
        assert!(problem.contains("no value called `cpuu`"), "{problem}");
        assert!(problem.contains("memory"), "{problem}");
    }

    #[test]
    fn a_name_only_the_driver_knows_is_read_back_from_what_it_publishes() {
        // the stick is the shape of it: nothing in this process can know it, and the file is how it is answered
        let dir = std::env::temp_dir().join("g13-sources-published");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("g13-values.json");
        std::fs::write(
            &file,
            r#"{"stick_x": 130, "profile": 1, "screen_visual": "applet:docker"}"#,
        )
        .unwrap();
        let world = World::default().with_values(&file);
        assert_eq!(
            resolve(&Spec::parse("stick_x"), &world).unwrap(),
            Value::Number(130.0)
        );
        assert_eq!(
            resolve(&Spec::parse("screen_visual"), &world)
                .unwrap()
                .text(),
            "applet:docker"
        );
        // with no driver there is no file, and the name is Missing rather than invented
        let nothing = World::default();
        assert_eq!(
            resolve(&Spec::parse("stick_x"), &nothing).unwrap(),
            Value::Missing
        );
    }

    #[test]
    fn a_path_in_a_config_is_expanded() {
        // set_var is unsafe in this edition: this test is the only thing running here, and the variable is
        // read by the call below and nothing else.
        // SAFETY: no other thread reads the environment in this test.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("G13_TEST_DIR", "/run/user/4242")
        };
        assert_eq!(
            real_path("$G13_TEST_DIR/g13-values.json"),
            "/run/user/4242/g13-values.json"
        );
        assert_eq!(real_path("${G13_TEST_DIR}/x"), "/run/user/4242/x");
        assert_eq!(real_path("/plain/path"), "/plain/path");
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            real_path("~/applets/x.json"),
            format!("{home}/applets/x.json")
        );
        assert_eq!(real_path("~"), home);
        // something unset stays as written rather than turning into a directory that cannot exist
        assert_eq!(real_path("$G13_NOT_SET/x"), "$G13_NOT_SET/x");
        assert_eq!(real_path("${G13_NOT_SET}/x"), "${G13_NOT_SET}/x");
        // and a stray `$` is left alone
        assert_eq!(real_path("/a$b"), "/a$b");
    }

    #[test]
    fn the_names_a_format_asks_for_are_read_out_of_it() {
        assert_eq!(names_in_format("cpu {cpu:.0f}%"), vec!["cpu"]);
        assert_eq!(
            names_in_format("{day} {date} up: {uptime}"),
            vec!["day", "date", "uptime"]
        );
        assert_eq!(names_in_format("{cpu} and {cpu} again"), vec!["cpu"]);
        assert!(names_in_format("no names here").is_empty());
        assert!(names_in_format("{unclosed").is_empty());
        assert_eq!(names_in_format("{}"), Vec::<String>::new());
    }

    #[test]
    fn endpoints_survive_a_read_and_a_write() {
        let dir = std::env::temp_dir().join("g13-sources-endpoints-round-trip");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("endpoints.json");
        // the shape of a hand-written file, including the two fields most easily lost: the token and `insecure`
        std::fs::write(
            &path,
            r#"{
              "weather": {"url": "http://wttr.in", "timeout": 3, "insecure": true},
              "github": {"url": "https://api.github.com", "token": "a-token-that-must-survive"}
            }"#,
        )
        .unwrap();

        let read = load_endpoints(&path).expect("the file should read");
        assert_eq!(read.len(), 2);
        assert_eq!(read["weather"].url, "http://wttr.in");
        assert!(read["weather"].insecure, "the load-bearing flag");
        assert_eq!(read["weather"].timeout, Some(3.0));
        assert_eq!(
            read["github"].token.as_deref(),
            Some("a-token-that-must-survive")
        );

        // change one field, write, and read back: nothing else may move
        let mut edited = read.clone();
        edited.get_mut("weather").unwrap().timeout = Some(9.0);
        save_endpoints(&path, &edited).expect("it should write");

        let again = load_endpoints(&path).expect("it should read");
        assert_eq!(again["weather"].timeout, Some(9.0), "the change");
        assert_eq!(
            again["weather"].url, "http://wttr.in",
            "everything else kept"
        );
        assert!(again["weather"].insecure);
        assert_eq!(
            again["github"].token.as_deref(),
            Some("a-token-that-must-survive"),
            "the credential must come back exactly as it went in"
        );
        // and the write went through its own path rather than over the file directly
        assert!(
            !dir.join("endpoints.json.writing").exists(),
            "the half-written file should be gone once it is in place"
        );
    }
    #[cfg(unix)]
    #[test]
    fn the_credential_file_is_written_for_its_owner_alone() {
        use std::os::unix::fs::PermissionsExt;
        let mode_of =
            |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let dir = std::env::temp_dir().join("g13-sources-endpoints-mode");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("endpoints.json");
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "github".to_string(),
            Endpoint {
                url: "https://api.github.com".to_string(),
                token: Some("a-token".to_string()),
                ..Endpoint::default()
            },
        );

        save_endpoints(&path, &endpoints).expect("it should write");
        assert_eq!(
            mode_of(&path),
            0o600,
            "a file with a token in it is the owner's alone"
        );

        // and a save over a file that was readable by everybody makes it the owner's alone again, because
        // the mode of the file that is renamed into place is the one that ends up there
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(mode_of(&path), 0o644, "the state this is here to correct");
        save_endpoints(&path, &endpoints).expect("it should write again");
        assert_eq!(
            mode_of(&path),
            0o600,
            "and a save corrects it rather than keeping it"
        );

        // the token itself is unchanged by any of this
        let read = load_endpoints(&path).expect("it should read");
        assert_eq!(read["github"].token.as_deref(), Some("a-token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_cannot_be_parsed_is_reported_rather_than_read_as_empty() {
        // the resolver ignores a file it cannot parse, which is right for a renderer and wrong for an editor:
        // an empty map looks exactly like a machine with no endpoints, so a typo would look like nothing to do
        let dir = std::env::temp_dir().join("g13-sources-endpoints-broken");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("endpoints.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let problem = load_endpoints(&path).expect_err("a broken file must be reported");
        assert!(problem.contains("not valid JSON"), "{problem}");
        assert!(
            problem.contains("endpoints.json"),
            "it should name the file: {problem}"
        );

        // an array where an object belongs is reported too
        std::fs::write(&path, "[1, 2, 3]").unwrap();
        assert!(load_endpoints(&path).is_err());

        // and a file that is simply not there is an empty set rather than a problem
        let missing = dir.join("not-here.json");
        assert!(load_endpoints(&missing).expect("absent is fine").is_empty());

        // an empty file is fine as well
        std::fs::write(&path, "").unwrap();
        assert!(load_endpoints(&path).unwrap().is_empty());
    }

    fn fixture_world() -> World {
        World {
            named: BTreeMap::new(),
            endpoints: BTreeMap::new(),
            command_timeout: Duration::from_millis(400),
            root: PathBuf::from("tests/fixtures/root"),
            values: None,
            // a world built by hand has no reading, and one is taken when a clock value is asked for
            clock: Clock::default(),
        }
    }

    #[test]
    fn a_source_splits_into_kind_rest_and_path() {
        let spec = Spec::parse("json:/tmp/hud.json#player.stats.level");
        assert_eq!(spec.kind, "json");
        assert_eq!(spec.rest, "/tmp/hud.json");
        assert_eq!(spec.path.as_deref(), Some("player.stats.level"));

        let spec = Spec::parse("cmd:docker ps -q | wc -l");
        assert_eq!(spec.kind, "cmd");
        assert_eq!(spec.rest, "docker ps -q | wc -l");
        assert_eq!(spec.path, None);

        // a bare name is a value out of the published catalogue, and the long form says the same thing
        let spec = Spec::parse("cpu");
        assert_eq!(spec.kind, "built-in");
        assert_eq!(spec.rest, "cpu");
        assert!(spec.is_local());
    }

    #[test]
    fn a_number_read_out_of_json_is_still_a_number() {
        let text = r#"{"cpu": 47.265625, "level": 12, "name": "V", "ok": true}"#;
        assert_eq!(json_value(text, "cpu"), Some(Value::Number(47.265625)));
        assert_eq!(json_value(text, "level"), Some(Value::Number(12.0)));
        assert_eq!(json_value(text, "name"), Some(Value::Text("V".into())));
        // a whole number shows as one, and a fraction keeps its digits
        assert_eq!(json_value(text, "level").unwrap().text(), "12");
        assert_eq!(json_value(text, "cpu").unwrap().text(), "47.265625");
        // and the format spec now rounds it, which is what a row wants
        assert_eq!(json_value(text, "cpu").unwrap().formatted(":.0f"), "47");
        // the string form is unchanged for anything that wants text
        assert_eq!(json_field(text, "cpu").as_deref(), Some("47.265625"));
        assert_eq!(json_field(text, "ok").as_deref(), Some("true"));
    }

    #[test]
    fn a_path_with_dots_and_indices_finds_the_value() {
        let text = r#"{"current_condition":[{"temp_C":"24","FeelsLikeC":"23"}],"player":{"level":12},"ok":true,"nothing":null}"#;
        assert_eq!(
            json_field(text, "current_condition.0.temp_C").as_deref(),
            Some("24")
        );
        assert_eq!(json_field(text, "player.level").as_deref(), Some("12"));
        assert_eq!(json_field(text, "ok").as_deref(), Some("true"));
        assert_eq!(json_field(text, "nothing"), None);
        assert_eq!(json_field(text, "absent.path"), None);
        // and on text that is not JSON at all
        assert_eq!(json_field("not json", "a"), None);
    }

    #[test]
    fn commands_are_read_by_what_they_print() {
        let world = fixture_world();
        let spec = Spec::parse("cmd:printf 42");
        assert_eq!(resolve(&spec, &world).unwrap(), Value::Number(42.0));

        // a command that prints nothing is missing, not zero
        let spec = Spec::parse("cmd:true");
        assert_eq!(resolve(&spec, &world).unwrap(), Value::Missing);

        // a command that fails but still printed something keeps what it printed
        let spec = Spec::parse("cmd:echo 7; false");
        assert_eq!(resolve(&spec, &world).unwrap(), Value::Number(7.0));
    }

    #[test]
    fn the_cpu_reading_is_an_interval_not_an_average_since_boot() {
        let world = World::default();
        let first = read_cpu_jiffies(&world).expect("this machine has /proc/stat");
        std::thread::sleep(std::time::Duration::from_millis(60));
        let second = read_cpu_jiffies(&world).expect("and again");
        // the counters only ever go up, which is what makes a difference meaningful
        assert!(second.0 >= first.0, "total jiffies went backwards");
        assert!(second.1 >= first.1, "idle jiffies went backwards");
        // the idle share of the whole machine is not the idle share of the last sixtieth of a second, and
        // conflating them is what produced a number that never moved
        let since_boot = 100.0 * (second.0 - second.1) as f64 / second.0.max(1) as f64;
        let interval = 100.0 * (second.0 - first.0 - (second.1 - first.1)) as f64
            / (second.0 - first.0).max(1) as f64;
        assert!((0.0..=100.0).contains(&since_boot));
        assert!((0.0..=100.0).contains(&interval));
        assert!(
            (interval - since_boot).abs() < 100.0,
            "both are percentages"
        );
    }

    #[test]
    fn a_built_in_comes_from_the_machine() {
        let world = World::default();
        let cpu = built_in_value("cpu", &world);
        assert!(
            cpu.number().is_some(),
            "cpu should be a number, got {cpu:?}"
        );
        let memory = built_in_value("memory", &world);
        let share = memory.number().expect("memory should be a number");
        assert!((0.0..=100.0).contains(&share), "memory was {share}");
        assert!(!built_in_value("uptime", &world).text().is_empty());
        assert!(!built_in_value("day", &world).text().is_empty());
    }

    #[test]
    fn a_reading_the_machine_does_not_have_is_missing_rather_than_a_panic() {
        assert_eq!(
            built_in_value("nonsense", &World::default()),
            Value::Missing
        );
    }

    #[test]
    fn a_missing_file_is_missing_not_an_error() {
        let world = fixture_world();
        let spec = Spec::parse("json:/nonexistent/hud.json#health");
        assert_eq!(resolve(&spec, &world).unwrap(), Value::Missing);
        let spec = Spec::parse("file:/nonexistent/thing");
        assert_eq!(resolve(&spec, &world).unwrap(), Value::Missing);
    }

    #[test]
    fn a_network_kind_is_refused_here_rather_than_attempted() {
        let world = fixture_world();
        for source in ["imap:mail/INBOX#unread", "mqtt:t/x#v", "ws:wss://x#y"] {
            let problem = resolve(&Spec::parse(source), &world).unwrap_err();
            assert!(
                problem.contains("not resolved yet"),
                "{source} gave {problem}"
            );
        }
        // an http source with no matching endpoint says which endpoints exist rather than trying anyway
        let problem = resolve(&Spec::parse("http:absent/thing#field"), &world).unwrap_err();
        assert!(problem.contains("no endpoint called absent"), "{problem}");
        assert!(!Spec::parse("http:a/b#c").is_local());
    }

    #[test]
    fn endpoints_are_read_the_way_the_file_writes_them() {
        let text = r#"{
  "weather": {"url": "https://wttr.in/", "timeout": 3},
  "mail": {"url": "imaps://imap.example.com", "token": "secret"}
}"#;
        let endpoints = parse_endpoints(text);
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints["weather"].url, "https://wttr.in/");
        assert_eq!(endpoints["weather"].timeout, Some(3.0));
        assert_eq!(endpoints["mail"].token.as_deref(), Some("secret"));
    }

    #[test]
    fn formatting_follows_the_spec_the_applets_write() {
        assert_eq!(Value::Number(42.0).formatted(""), "42");
        assert_eq!(Value::Number(42.4).formatted(""), "42.4");
        assert_eq!(Value::Number(42.4).formatted(":0f"), "42");
        assert_eq!(Value::Number(42.456).formatted(":1f"), "42.5");
        assert_eq!(Value::Text("24".into()).formatted(":0f"), "24");
        assert_eq!(Value::Missing.formatted(":0f"), "");
    }
}

#[cfg(test)]
mod which_player_to_believe {
    use super::*;

    #[test]
    fn the_playing_player_is_believed_and_a_paused_one_is_only_a_fallback() {
        // measured on this machine, exactly: two players, one of them stopped, nothing playing - the case that broke it
        let stopped_and_paused = ["Stopped\ttitle from chromium", "Paused\ttitle from firefox"];
        assert_eq!(
            chosen_line(&stopped_and_paused),
            Some("Stopped\ttitle from chromium"),
            "with nothing playing the first line is the fallback, which is what it always did"
        );
        // and the same two, with one playing: the reading one wins, whichever order they come in
        let paused_and_playing = ["Paused\tfrom firefox", "Playing\tfrom chromium"];
        assert_eq!(
            chosen_line(&paused_and_playing),
            Some("Playing\tfrom chromium"),
            "the player that is playing is the one to believe"
        );
        let playing_first = ["Playing\tfrom firefox", "Paused\tfrom chromium"];
        assert_eq!(chosen_line(&playing_first), Some("Playing\tfrom firefox"));
        // nothing at all, and a line with no status, must not panic or invent a player
        assert_eq!(chosen_line(&[]), None);
        assert_eq!(chosen_line(&[""]), Some(""));
    }
}

#[cfg(test)]
mod the_position_that_moves_between_readings {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_playing_position_is_carried_forward_and_a_paused_one_is_not() {
        // Its own carry, not the driver's: this file holds a global one for the process, and another test running
        // in parallel against it reseeded this one's state - which is exactly the two-tests-one-fixture trap this
        // project has been caught by before.
        let mut state: Option<Moving> = None;
        // A bar drawn straight from MPRIS steps once a second. While the player is playing the position is known to
        // be advancing, so it is carried by this machine's clock between the readings the player gives.
        assert_eq!(
            moving_seconds(&mut state, Some(10.0), true, "a track", Some(200.0)),
            Some(10.0),
            "the player's own answer is the truth to start from"
        );
        std::thread::sleep(Duration::from_millis(120));
        // and the *same* answer arriving again - which is what the cache does, twenty times a second - must not
        // re-seed and lose the carry. This is the flaw the first version had.
        let carried =
            moving_seconds(&mut state, Some(10.0), true, "a track", Some(200.0)).expect("carried");
        assert!(
            (10.05..11.0).contains(&carried),
            "a playing track should have moved on by about the time that passed, not to {carried}"
        );
        // the player speaking again is believed, even when it disagrees with the carry
        assert_eq!(
            moving_seconds(&mut state, Some(4.0), true, "a track", Some(200.0)),
            Some(4.0)
        );
        // paused, it stays where it was, however long is left
        assert_eq!(
            moving_seconds(&mut state, Some(30.0), false, "a track", Some(200.0)),
            Some(30.0)
        );
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(
            moving_seconds(&mut state, Some(30.0), false, "a track", Some(200.0)),
            Some(30.0),
            "a paused track does not move"
        );
        // and it never runs past the end of the track
        assert_eq!(
            moving_seconds(&mut state, Some(199.9), true, "a track", Some(200.0)),
            Some(199.9)
        );
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(
            moving_seconds(&mut state, Some(199.9), true, "a track", Some(200.0)),
            Some(200.0)
        );
        // a length nobody gave means no ceiling, rather than a position of nothing
        assert!(moving_seconds(&mut state, Some(5.0), true, "a third track", None).is_some());
        // and a track nothing is known about is not carried on from the last one's position
        assert_eq!(
            moving_seconds(&mut state, None, true, "a fourth track", Some(200.0)),
            None
        );
    }
}

#[cfg(test)]
mod the_numbers_on_the_pad {
    use super::*;

    #[test]
    fn a_position_is_minutes_and_seconds_and_never_microseconds() {
        // A real track, at the moment it was reported: the player gives microseconds, and the pad shows 39:18 / 46:30.
        // Passing microseconds to `media_time_from` as though they were seconds multiplied them by a million, which
        // is what put 39300000:00 on the panel.
        assert_eq!(media_time_from(2_358_000_000.0).text(), "39:18");
        assert_eq!(media_time_from(2_790_000_000.0).text(), "46:30");
        // a hair over an hour, and the old case the metadata fix was written for: 409000015 microseconds is 6:49
        assert_eq!(media_time_from(3_600_000_000.0).text(), "60:00");
        assert_eq!(media_time_from(409_000_015.0).text(), "6:49");
    }
}

#[cfg(test)]
mod values_of_his_own {
    use super::*;

    fn write(dir: &std::path::Path, text: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("values.json");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_value_the_user_names_resolves_and_a_published_name_is_refused() {
        let dir = std::env::temp_dir().join(format!("g13-values-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = write(
            &dir,
            r#"{"where": "cmd:echo upstairs", "cpu": "cmd:echo 99", "broken": "http:"}"#,
        );
        let specs = load_custom_values(&path).expect("it reads");
        assert_eq!(
            specs.get("where").map(String::as_str),
            Some("cmd:echo upstairs")
        );

        let (resolved, complaints) = resolve_custom_values(&specs, &World::default());
        assert_eq!(
            resolved.get("where"),
            Some(&Value::Text("upstairs".to_string())),
            "a value the user named did not resolve"
        );
        // a published name is not available to a user value: every applet already reads `cpu`, and taking it would
        // change the meaning of applets the user did not touch
        assert!(
            !resolved.contains_key("cpu"),
            "a published name was written over"
        );
        assert!(
            complaints
                .iter()
                .any(|c| c.contains("cpu") && c.contains("already publishes")),
            "the refusal was not said: {complaints:?}"
        );
        // and a spec that will not resolve is said, not silently missing
        assert!(
            complaints.iter().any(|c| c.starts_with("broken:")),
            "a broken spec was swallowed: {complaints:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_will_not_read_is_said_rather_than_looking_empty() {
        let dir = std::env::temp_dir().join(format!("g13-values-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = write(&dir, r#"{"where": ["not", "a", "string"]}"#);
        let problem = load_custom_values(&path).expect_err("a list is not a spec");
        assert!(problem.contains("where"), "{problem}");
        // no file at all is nothing to do, not a fault
        assert_eq!(
            load_custom_values(&dir.join("absent.json")).expect("nothing to read"),
            BTreeMap::new()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod his_values_are_kept_until_the_file_changes {
    use super::*;

    #[test]
    fn resolved_once_and_again_when_the_file_changes() {
        let dir = std::env::temp_dir().join(format!("g13-named-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let values = dir.join("values.json");
        let published = dir.join("g13-values.json");
        std::fs::write(&published, r#"{"uptime": "1 day"}"#).unwrap();
        std::fs::write(&values, r#"{"where": "cmd:echo upstairs"}"#).unwrap();

        let (first, complaints) = named_values(&values, &published);
        assert!(complaints.is_empty(), "{complaints:?}");
        assert_eq!(first.get("where"), Some(&Value::Text("upstairs".into())));

        // and an applet naming it gets it, which is the whole point: no `sources` entry of its own
        let world = World::default().with_named(first.clone());
        assert_eq!(
            resolve(&Spec::parse("where"), &world).expect("the value the user named"),
            Value::Text("upstairs".into())
        );
        // a name nobody has still says what there is, the user's included
        let problem = resolve(&Spec::parse("nowhere"), &world).expect_err("no such value");
        assert!(
            problem.contains("nowhere") && problem.contains("Yours are: where"),
            "{problem}"
        );

        // the cache is keyed on the file, so a change is picked up rather than served stale
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&values, r#"{"where": "cmd:echo downstairs"}"#).unwrap();
        let (second, _) = named_values(&values, &published);
        assert_eq!(
            second.get("where"),
            Some(&Value::Text("downstairs".into())),
            "a changed values.json was served from the cache"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod one_reading_of_the_clock {
    use super::*;

    /// The reading has seconds in it, so a value claiming to be seconds is not limited to whole minutes.
    #[test]
    fn seconds_are_seconds_not_rounded_minutes() {
        assert_eq!(seconds_of("10:58:07"), 10 * 3600 + 58 * 60 + 7);
        assert_eq!(seconds_of("00:00:00"), 0);
        assert_eq!(seconds_of("23:59:59"), 86399);
        // and what a display string can express is only ever a whole minute, which is what changed
        assert_eq!(seconds_of("10:58:00"), 39480);
        assert_eq!(seconds_of("10:58:00") % 60, 0);
    }

    #[test]
    fn one_read_gives_every_form_of_the_same_instant() {
        let clock = Clock::read();
        assert!(!clock.is_empty(), "the clock read nothing");
        let parts: Vec<&str> = clock.iso.split('-').collect();
        assert_eq!(parts.len(), 3, "not an ISO date: {}", clock.iso);
        assert_eq!(
            parts[0].len(),
            4,
            "the year is not four digits: {}",
            clock.iso
        );
        assert_eq!(
            parts[1].len(),
            2,
            "the month is not zero-padded: {}",
            clock.iso
        );
        assert_eq!(
            parts[2].len(),
            2,
            "the day is not zero-padded: {}",
            clock.iso
        );
        assert_eq!(clock.time.len(), 5, "not HH:MM: {}", clock.time);
        assert!(
            clock.seconds < 86_400,
            "not a time of day: {}",
            clock.seconds
        );
        assert!(!clock.date.is_empty() && !clock.day.is_empty());
        // the day and the time are the same instant, read together
        let hours: u32 = clock.time[..2].parse().unwrap();
        assert!(
            clock.seconds >= hours * 3600,
            "the seconds are behind the time"
        );
        assert!(
            clock.seconds < (hours + 1) * 3600,
            "the seconds are ahead of the time"
        );
    }

    /// A world built by hand has no reading, and asking for a clock value still works.
    #[test]
    fn a_world_without_a_clock_reads_one_when_asked() {
        let world = World::default();
        assert!(world.clock.is_empty());
        let today = built_in_value("date_iso", &world);
        let text = today.text();
        assert_eq!(text.split('-').count(), 3, "not an ISO date: {text}");
    }
}
