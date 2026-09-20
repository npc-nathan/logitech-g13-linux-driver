//! Playing a macro graph, and the questions a macro can ask.
//!
//! The player for a flat `sequence=` list lives next to the driver; this is the one for a graph. Both keep the
//! same discipline: a wait says how long until the *next* thing, so the walk waits until each node is due
//! rather than sleeping after the work of the one before it.
//!
//! Three things here are deliberate:
//!
//! - **Questions are answered from what the playback was fired with**, never from anything global: the control
//!   that fired it, the profile that was active, and the published values. So `g13 macro play` needs no pad and
//!   gives the same answer as the pad would for the same inputs, and a test can ask the same questions the
//!   window would.
//! - **A graph that cannot finish is stopped and says so.** Loops are allowed - a `repeat`'s body goes back to
//!   the repeat - so a graph whose body never comes back round would otherwise hold the keyboard for ever.
//! - **A character this build cannot type is an error**, not an approximation: `type "£"` fails rather than
//!   typing something else into whatever is focused.

use g13_config::{KeyMode, MacroGraph, NodeKind};
use g13_device::keyboard::{KeyCode, VirtualKeyboard};
use g13_sources::{Spec, World, resolve};
use g13_values::Cond;
use g13_values::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The most nodes one playback may visit, so a loop that never comes back round stops.
const MOST_NODES: usize = 20_000;

/// What a macro knows about the moment it was fired.
///
/// This is the whole of a macro's ability to decide anything, so it is small on purpose.
#[derive(Debug, Clone, Default)]
pub struct Playback {
    /// The control that fired it, as the pad prints it (`G7`).
    pub control: Option<String>,
    /// The profile that was active, used both for `profile` questions and for writing a binding.
    pub profile: u32,
    /// The published values, for `value` questions.
    pub values: BTreeMap<String, Value>,
    /// Where a macro's own sources are read from: the endpoints file and the published values, the same
    /// world an applet is given. Default is a world with neither, which is what a test wants.
    pub world: World,
}

impl Playback {
    /// This playback with the graph's own sources read into it.
    ///
    /// Read **now**, at the question, not at the trigger: "is the cpu over 50" has to be about now, and a
    /// macro in a loop is asking about now on every pass. A source shadows a published value of the same
    /// name, because declaring it here is the more specific thing to say.
    ///
    /// A source that will not read is `Missing` and says why, rather than quietly comparing as zero.
    fn with_sources(&self, sources: &BTreeMap<String, String>, problems: &mut Vec<String>) -> Self {
        let mut asked = self.clone();
        for (name, spec) in sources {
            match resolve(&Spec::parse(spec), &self.world) {
                Ok(value) => {
                    asked.values.insert(name.clone(), value);
                }
                Err(problem) => {
                    asked.values.insert(name.clone(), Value::Missing);
                    problems.push(format!("{name} ({spec}): {problem}"));
                }
            }
        }
        asked
    }
}

/// Whether a question is true now, for a macro. The comparison itself lives beside `Cond` in `g13-config`, so an
/// applet's alert and a macro's `if` are the same code rather than two that agree.
pub fn condition_holds(condition: &Cond, playback: &Playback) -> bool {
    g13_values::holds(
        condition,
        &playback.values,
        playback.profile,
        playback.control.as_deref(),
    )
}

/// Every name a question asks about, however it is nested.
pub fn condition_names(condition: &Cond) -> Vec<&str> {
    match condition {
        Cond::Value { name, .. } => vec![name.as_str()],
        Cond::All(parts) | Cond::Any(parts) => parts.iter().flat_map(condition_names).collect(),
        Cond::Not(inner) => condition_names(inner),
        // a control and a profile are not names anything provides
        Cond::Control(_) | Cond::Profile(_) => Vec::new(),
    }
}

/// Load a macro to call, however it is stored.
pub type Loader<'a> = &'a dyn Fn(u32) -> Option<(g13_config::Macro, Option<MacroGraph>)>;

/// Play a graph, calling `on_step` with each node's id as it is reached.
///
/// Returns how long the playback occupied, so a `call` inside another macro keeps the outer timeline right.
pub fn play_graph(
    keyboard: &Arc<Mutex<VirtualKeyboard>>,
    graph: &MacroGraph,
    playback: &Playback,
    load: &dyn Fn(u32) -> Option<(g13_config::Macro, Option<MacroGraph>)>,
    on_step: &mut dyn FnMut(&str),
) -> Result<Duration, String> {
    play_graph_reporting(keyboard, graph, playback, load, on_step, &mut Vec::new())
}

/// Play a graph, collecting anything that went wrong with its sources.
///
/// The player that says what happened: a source that will not read is reported rather than compared as zero.
pub fn play_graph_reporting(
    keyboard: &Arc<Mutex<VirtualKeyboard>>,
    graph: &MacroGraph,
    playback: &Playback,
    load: &dyn Fn(u32) -> Option<(g13_config::Macro, Option<MacroGraph>)>,
    on_step: &mut dyn FnMut(&str),
    problems: &mut Vec<String>,
) -> Result<Duration, String> {
    // Neither `load` nor `on_step` is generic, and that is not a style choice: a macro may call another, so
    // this function calls itself. With a generic callback the compiler instantiates a new copy of it at every
    // level of that recursion, which is a build error rather than a slow build.
    let on_step = &mut *on_step;
    let started = Instant::now();
    // where we are in the macro's own time: a wait moves this forward rather than sleeping after the work
    let mut at = Duration::ZERO;
    // A wait at the *end* of a macro has no next node to be due, so the clock is honoured on the way out: a
    // macro that waits 100ms at the end takes 100ms longer, and a caller that called it is told so.
    let finish = |at: Duration, started: Instant| {
        if let Some(rest) = at.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
        started.elapsed()
    };
    let mut iterations: HashMap<String, u32> = HashMap::new();
    let mut node = graph.start.clone();
    let mut previous: Option<String> = None;
    let mut visits = 0usize;

    loop {
        visits += 1;
        if visits > MOST_NODES {
            return Err(format!(
                "the macro went round {MOST_NODES} nodes without finishing, so it was stopped"
            ));
        }
        let Some(current) = graph.node(&node) else {
            return Err(format!("the macro arrived at {node}, which is not a node"));
        };
        if let Some(rest) = at.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
        on_step(&node);

        match &current.kind {
            NodeKind::Key { code, hold, mode } => match mode {
                KeyMode::Tap => {
                    send(keyboard, *code, true)?;
                    at += Duration::from_millis(*hold as u64);
                    let release_at = at;
                    if let Some(rest) = release_at.checked_sub(started.elapsed()) {
                        std::thread::sleep(rest);
                    }
                    send(keyboard, *code, false)?;
                }
                KeyMode::Down => send(keyboard, *code, true)?,
                KeyMode::Up => send(keyboard, *code, false)?,
            },
            NodeKind::Wait { ms } => at += Duration::from_millis(*ms as u64),
            NodeKind::Type { text, per_key } => {
                for character in text.chars() {
                    let (code, shift) =
                        g13_device::keyboard::char_key(character).ok_or_else(|| {
                            format!("{character:?} is a character this build cannot type")
                        })?;
                    if shift {
                        send(keyboard, SHIFT, true)?;
                    }
                    send(keyboard, code, true)?;
                    std::thread::sleep(Duration::from_millis(8));
                    send(keyboard, code, false)?;
                    if shift {
                        send(keyboard, SHIFT, false)?;
                    }
                    at += Duration::from_millis(*per_key as u64);
                }
            }
            NodeKind::If { when } => {
                // the macro's own sources are read as the question is asked
                let asked = playback.with_sources(&graph.sources, problems);
                // A question naming something nothing provides is false, which is right - but it is false for
                // ever, and a branch that never runs for a reason nobody can see is the worst kind of macro.
                // Renaming a source leaves exactly that behind, so it is said here.
                for name in condition_names(when) {
                    if !asked.values.contains_key(name) {
                        problems.push(format!(
                            "the question asks about {name}, which neither this macro's sources nor the \
                             published values provide"
                        ));
                    }
                }
                let port = if condition_holds(when, &asked) {
                    "then"
                } else {
                    "else"
                };
                match graph.next(&node, port)? {
                    Some(next) => {
                        previous = Some(node.clone());
                        node = next.to_string();
                        continue;
                    }
                    // nothing down that way is the end of the macro, not an error
                    None => return Ok(finish(at, started)),
                }
            }
            NodeKind::Repeat { times } => {
                // Counting consecutive visits: coming back round from the body carries on, arriving from
                // anywhere else starts again. A loop re-entered later should run its count again rather than
                // be treated as having already finished.
                let came_round = previous
                    .as_deref()
                    .map(|previous| {
                        graph
                            .edges
                            .iter()
                            .any(|edge| edge.from == previous && edge.to == node)
                    })
                    .unwrap_or(false);
                let count = iterations.entry(node.clone()).or_insert(0);
                if !came_round {
                    *count = 0;
                }
                *count += 1;
                // the count is of entries, so the body runs for entries 1..=times and the extra entry is the
                // one that decides it is finished
                let port = if *count <= *times { "body" } else { "then" };
                match graph.next(&node, port)? {
                    Some(next) => {
                        previous = Some(node.clone());
                        node = next.to_string();
                        continue;
                    }
                    None => return Ok(finish(at, started)),
                }
            }
            NodeKind::Run { spec } => {
                // the output side: do it here, and a failure stops the macro rather than passing for success
                resolve(&Spec::parse(spec), &playback.world).map_err(|problem| {
                    format!("the macro asked for {spec} and it did not happen: {problem}")
                })?;
            }
            NodeKind::Call { macro_id } => {
                let Some((macro_file, called_graph)) = load(*macro_id) else {
                    return Err(format!("macro {macro_id} is not there to play"));
                };
                match called_graph {
                    Some(inner) => {
                        let spent = play_graph_reporting(
                            keyboard, &inner, playback, load, on_step, problems,
                        )?;
                        // the outer macro's own clock now includes the time that took
                        at += spent;
                    }
                    None => {
                        let before = Instant::now();
                        crate::play_macro(keyboard, &macro_file, 1)?;
                        at += before.elapsed();
                    }
                }
            }
            NodeKind::Stop => return Ok(finish(at, started)),
        }

        match graph.next(&node, current.port())? {
            Some(next) => {
                previous = Some(node.clone());
                node = next.to_string();
            }
            None => return Ok(finish(at, started)),
        }
    }
}

/// Left shift, for the characters that need it.
const SHIFT: u16 = 42;

/// Press or release one key on the shared virtual keyboard, handing back the lock or device failure as text.
fn send(keyboard: &Arc<Mutex<VirtualKeyboard>>, code: u16, down: bool) -> Result<(), String> {
    let mut keyboard = keyboard
        .lock()
        .map_err(|_| "the keyboard lock is poisoned".to_string())?;
    let code = KeyCode::new(code);
    let sent = if down {
        keyboard.press(code)
    } else {
        keyboard.release(code)
    };
    sent.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use g13_config::parse_graph;

    fn a_keyboard() -> Option<Arc<Mutex<VirtualKeyboard>>> {
        VirtualKeyboard::new()
            .ok()
            .map(|keyboard| Arc::new(Mutex::new(keyboard)))
    }

    fn values(pairs: &[(&str, g13_values::Value)]) -> BTreeMap<String, g13_values::Value> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn a_macro_can_ask_which_control_fired_it() {
        // The question only a pad can answer, and the reason one macro can sit on several buttons.
        let fired_by_g7 = Playback {
            control: Some("G7".to_string()),
            profile: 2,
            ..Playback::default()
        };
        assert!(condition_holds(&Cond::Control("G7".into()), &fired_by_g7));
        assert!(condition_holds(&Cond::Control("g7".into()), &fired_by_g7));
        assert!(!condition_holds(&Cond::Control("G8".into()), &fired_by_g7));
        // a macro played from the terminal has no control, so the question is false rather than a guess
        assert!(!condition_holds(
            &Cond::Control("G7".into()),
            &Playback::default()
        ));

        assert!(condition_holds(&Cond::Profile(2), &fired_by_g7));
        assert!(!condition_holds(&Cond::Profile(3), &fired_by_g7));
    }

    #[test]
    fn a_macro_can_ask_about_a_published_value() {
        let playback = Playback {
            values: values(&[
                ("cpu", Value::Number(63.5)),
                ("media_status", Value::Text("Playing".to_string())),
            ]),
            ..Playback::default()
        };
        let ask = |json: &str| {
            let graph = parse_graph(&format!(
                r#"{{"nodes":[{{"id":"q","kind":"if","when":{json}}}]}}"#
            ))
            .expect("a question");
            match &graph.node("q").expect("q").kind {
                NodeKind::If { when } => condition_holds(when, &playback),
                other => panic!("expected an if, got {other:?}"),
            }
        };
        assert!(ask(r#"{"value": "cpu", "op": ">", "to": 50}"#));
        assert!(!ask(r#"{"value": "cpu", "op": "<", "to": 50}"#));
        assert!(ask(r#"{"value": "cpu", "op": ">=", "to": 63.5}"#));
        assert!(ask(
            r#"{"value": "media_status", "op": "==", "text": "Playing"}"#
        ));
        assert!(ask(
            r#"{"value": "media_status", "op": "contains", "text": "lay"}"#
        ));
        assert!(!ask(
            r#"{"value": "media_status", "op": "==", "text": "Paused"}"#
        ));
        assert!(ask(
            r#"{"all": [{"profile": 0}, {"value": "cpu", "op": ">", "to": 10}]}"#
        ));
        assert!(ask(
            r#"{"any": [{"profile": 3}, {"value": "cpu", "op": ">", "to": 10}]}"#
        ));
        assert!(ask(r#"{"not": {"profile": 3}}"#));
        // a value nobody has published is unknown, and unknown is not a comparison
        assert!(!ask(r#"{"value": "gpu", "op": ">", "to": 0}"#));
        // ordering text is not a comparison this makes, and false is the honest answer
        assert!(!ask(r#"{"value": "media_status", "op": ">", "to": 0}"#));
    }

    #[test]
    fn a_branch_follows_the_control_that_fired_it() {
        let Some(keyboard) = a_keyboard() else {
            eprintln!("no virtual keyboard here; skipping the branch test");
            return;
        };
        // one macro on two buttons: the same graph, two answers
        let graph = parse_graph(
            r#"{"start": "ask",
                "nodes": [
                    {"id": "ask", "kind": "if", "when": {"control": "G7"}},
                    {"id": "left", "kind": "wait", "ms": 0},
                    {"id": "right", "kind": "wait", "ms": 0}
                ],
                "edges": [
                    {"from": "ask", "port": "then", "to": "left"},
                    {"from": "ask", "port": "else", "to": "right"}
                ]}"#,
        )
        .expect("a branching graph");
        let mut walked: Vec<String> = Vec::new();
        let fired_by_g7 = Playback {
            control: Some("G7".to_string()),
            ..Playback::default()
        };
        play_graph(&keyboard, &graph, &fired_by_g7, &|_| None, &mut |node| {
            walked.push(node.to_string())
        })
        .expect("played");
        assert_eq!(walked, vec!["ask".to_string(), "left".to_string()]);

        walked.clear();
        let fired_by_g8 = Playback {
            control: Some("G8".to_string()),
            ..Playback::default()
        };
        play_graph(&keyboard, &graph, &fired_by_g8, &|_| None, &mut |node| {
            walked.push(node.to_string())
        })
        .expect("played");
        assert_eq!(walked, vec!["ask".to_string(), "right".to_string()]);
    }

    #[test]
    fn a_repeat_runs_its_body_the_number_of_times_it_says() {
        let Some(keyboard) = a_keyboard() else {
            eprintln!("no virtual keyboard here; skipping the repeat test");
            return;
        };
        // three times round: the body comes back to the repeat, which is how a loop is drawn
        let graph = parse_graph(
            r#"{"start": "again",
                "nodes": [
                    {"id": "again", "kind": "repeat", "times": 3},
                    {"id": "body", "kind": "wait", "ms": 0},
                    {"id": "after", "kind": "wait", "ms": 0}
                ],
                "edges": [
                    {"from": "again", "port": "body", "to": "body"},
                    {"from": "body", "to": "again"},
                    {"from": "again", "port": "then", "to": "after"}
                ]}"#,
        )
        .expect("a loop");
        let mut walked: Vec<String> = Vec::new();
        play_graph(
            &keyboard,
            &graph,
            &Playback::default(),
            &|_| None,
            &mut |node| walked.push(node.to_string()),
        )
        .expect("played");
        let bodies = walked.iter().filter(|node| *node == "body").count();
        assert_eq!(bodies, 3, "the body ran {bodies} times: {walked:?}");
        assert_eq!(walked.last().map(String::as_str), Some("after"));
    }

    #[test]
    fn a_graph_that_never_finishes_is_stopped_and_says_so() {
        let Some(keyboard) = a_keyboard() else {
            eprintln!("no virtual keyboard here; skipping the runaway test");
            return;
        };
        // two nodes pointing at each other with nothing to end it: the walker's own cap is the only way out
        let graph = parse_graph(
            r#"{"start": "a",
                "nodes": [
                    {"id": "a", "kind": "wait", "ms": 0},
                    {"id": "b", "kind": "wait", "ms": 0}
                ],
                "edges": [{"from": "a", "to": "b"}, {"from": "b", "to": "a"}]}"#,
        )
        .expect("a loop with no end");
        let problem = play_graph(
            &keyboard,
            &graph,
            &Playback::default(),
            &|_| None,
            &mut |_| {},
        )
        .expect_err("a graph that never finishes");
        assert!(problem.contains("without finishing"), "{problem}");
    }

    #[test]
    fn a_character_this_build_cannot_type_is_refused_rather_than_approximated() {
        let Some(keyboard) = a_keyboard() else {
            eprintln!("no virtual keyboard here; skipping the typing test");
            return;
        };
        let graph =
            parse_graph(r#"{"nodes":[{"id": "t", "kind": "type", "text": "ok £", "per_key": 0}]}"#)
                .expect("a graph that types");
        let problem = play_graph(
            &keyboard,
            &graph,
            &Playback::default(),
            &|_| None,
            &mut |_| {},
        )
        .expect_err("a character this build cannot type");
        assert!(problem.contains("cannot type"), "{problem}");
    }

    #[test]
    fn a_call_plays_the_macro_it_names() {
        let Some(keyboard) = a_keyboard() else {
            eprintln!("no virtual keyboard here; skipping the call test");
            return;
        };
        let graph = parse_graph(
            r#"{"start": "here",
                "nodes": [
                    {"id": "here", "kind": "call", "macro": 7},
                    {"id": "after", "kind": "wait", "ms": 0}
                ],
                "edges": [{"from": "here", "to": "after"}]}"#,
        )
        .expect("a graph that calls");
        let called = parse_graph(
            r#"{"start": "inner",
                "nodes": [
                    {"id": "inner", "kind": "wait", "ms": 0},
                    {"id": "deep", "kind": "wait", "ms": 0}
                ],
                "edges": [{"from": "inner", "to": "deep"}]}"#,
        )
        .expect("the called graph");
        let mut walked: Vec<String> = Vec::new();
        play_graph(
            &keyboard,
            &graph,
            &Playback::default(),
            &|id| {
                (id == 7).then(|| {
                    (
                        g13_config::Macro {
                            name: "called".to_string(),
                            id: 7,
                            steps: Vec::new(),
                        },
                        Some(called.clone()),
                    )
                })
            },
            &mut |node| walked.push(node.to_string()),
        )
        .expect("played");
        // the called macro's own nodes are walked, and then the caller carries on
        assert_eq!(
            walked,
            vec![
                "here".to_string(),
                "inner".to_string(),
                "deep".to_string(),
                "after".to_string()
            ]
        );

        // and a macro that is not there is a named failure rather than a silence
        let problem = play_graph(
            &keyboard,
            &graph,
            &Playback::default(),
            &|_| None,
            &mut |_| {},
        )
        .expect_err("a call to nothing");
        assert!(problem.contains("macro 7 is not there"), "{problem}");
    }

    /// A directory of its own, so two tests never write the same path.
    fn a_scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("g13-playing-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    #[test]
    fn a_macro_reads_its_own_sources_when_it_asks_and_not_when_it_was_pressed() {
        let Some(keyboard) = a_keyboard() else {
            eprintln!("no virtual keyboard here; skipping the source test");
            return;
        };
        let dir = a_scratch_dir("fresh-source");
        let heat = dir.join("heat");
        std::fs::write(&heat, "80").expect("a hot reading");

        // the macro waits, and while it waits the machine cools down
        let graph = parse_graph(&format!(
            r#"{{"start": "wait",
                "sources": {{"heat": "cmd:cat {}"}},
                "nodes": [
                    {{"id": "wait", "kind": "wait", "ms": 150}},
                    {{"id": "ask", "kind": "if", "when": {{"value": "heat", "op": ">", "to": 50}}}},
                    {{"id": "hot", "kind": "wait", "ms": 0}},
                    {{"id": "cool", "kind": "wait", "ms": 0}}
                ],
                "edges": [
                    {{"from": "wait", "to": "ask"}},
                    {{"from": "ask", "port": "then", "to": "hot"}},
                    {{"from": "ask", "port": "else", "to": "cool"}}
                ]}}"#,
            heat.display()
        ))
        .expect("a graph that reads the machine");

        // The machine cools down *during* the play, and the write is caused by the macro reaching its first
        // node rather than by a sleep. It was a thread that slept 60ms while the question came at 150ms, which
        // a loaded machine loses - and a lost race showed up as "the macro answered with the reading from before
        // it started", i.e. as the promise this test exists to prove looking broken. A test for an ordering
        // must force the ordering.
        let changer = heat.clone();
        let mut wrote_at: Option<String> = None;
        let mut walked: Vec<String> = Vec::new();
        let mut problems: Vec<String> = Vec::new();
        play_graph_reporting(
            &keyboard,
            &graph,
            &Playback::default(),
            &|_| None,
            &mut |node| {
                if wrote_at.is_none() {
                    std::fs::write(&changer, "10").expect("the cooler reading");
                    wrote_at = Some(node.to_string());
                }
                walked.push(node.to_string());
            },
            &mut problems,
        )
        .expect("played");
        assert_eq!(
            wrote_at.as_deref(),
            Some("wait"),
            "the cooling must happen after the macro has started and before it asks"
        );

        // it read 10, not the 80 that was there when it was pressed: a question is about now
        assert_eq!(
            walked,
            vec!["wait", "ask", "cool"],
            "the macro answered with the reading from before it started"
        );
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_source_that_will_not_read_is_reported_and_is_not_a_number() {
        let Some(keyboard) = a_keyboard() else {
            eprintln!("no virtual keyboard here; skipping the broken source test");
            return;
        };
        // a command that outlasts the timeout: the reading is unknown, which is not zero
        let graph = parse_graph(
            r#"{"start": "ask",
                "sources": {"heat": "cmd:sleep 5"},
                "nodes": [
                    {"id": "ask", "kind": "if", "when": {"value": "heat", "op": "<", "to": 500}},
                    {"id": "cool", "kind": "wait", "ms": 0},
                    {"id": "hot", "kind": "wait", "ms": 0}
                ],
                "edges": [
                    {"from": "ask", "port": "then", "to": "cool"},
                    {"from": "ask", "port": "else", "to": "hot"}
                ]}"#,
        )
        .expect("a graph that cannot read");

        let mut walked: Vec<String> = Vec::new();
        let mut problems: Vec<String> = Vec::new();
        play_graph_reporting(
            &keyboard,
            &graph,
            &Playback::default(),
            &|_| None,
            &mut |node| walked.push(node.to_string()),
            &mut problems,
        )
        .expect("played");
        // "less than 500" is not true of a value nobody could read
        assert_eq!(walked, vec!["ask", "hot"], "{walked:?}");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("heat"), "{problems:?}");
        assert!(problems[0].contains("cmd:sleep 5"), "{problems:?}");
    }

    #[test]
    fn an_output_node_does_something_and_a_failure_stops_the_macro() {
        let Some(keyboard) = a_keyboard() else {
            eprintln!("no virtual keyboard here; skipping the output test");
            return;
        };
        let dir = a_scratch_dir("run-node");
        let marker = dir.join("it-happened");

        let graph = parse_graph(&format!(
            r#"{{"start": "do", "nodes": [
                {{"id": "do", "kind": "run", "spec": "cmd:touch {}"}},
                {{"id": "after", "kind": "wait", "ms": 0}}
            ], "edges": [{{"from": "do", "to": "after"}}]}}"#,
            marker.display()
        ))
        .expect("a graph that runs a command");
        let mut walked: Vec<String> = Vec::new();
        play_graph(
            &keyboard,
            &graph,
            &Playback::default(),
            &|_| None,
            &mut |node| walked.push(node.to_string()),
        )
        .expect("played");
        // the file is there: the action happened, and that is the only proof that counts
        assert!(marker.exists(), "the command never ran");
        assert_eq!(walked, vec!["do", "after"]);

        // and a command that cannot run stops the macro and says which one
        let broken = parse_graph(
            r#"{"start": "do", "nodes": [{"id": "do", "kind": "run", "spec": "cmd:sleep 5"}]}"#,
        )
        .expect("a graph with a command that will not finish");
        let problem = play_graph(
            &keyboard,
            &broken,
            &Playback::default(),
            &|_| None,
            &mut |_| {},
        )
        .expect_err("a command that did not happen");
        assert!(problem.contains("did not happen"), "{problem}");
        assert!(problem.contains("cmd:sleep 5"), "{problem}");
    }

    #[test]
    fn a_question_about_a_name_nothing_provides_says_so() {
        let Some(keyboard) = a_keyboard() else {
            eprintln!("no virtual keyboard here; skipping the unknown name test");
            return;
        };
        // the shape renaming a source leaves behind: the question still names the old name
        let graph = parse_graph(
            r#"{"start": "ask",
                "sources": {"heat": "cmd:true"},
                "nodes": [
                    {"id": "ask", "kind": "if",
                     "when": {"all": [{"value": "cpu_temp", "op": ">", "to": 70}, {"profile": 1}]}},
                    {"id": "yes", "kind": "wait", "ms": 0},
                    {"id": "no", "kind": "wait", "ms": 0}
                ],
                "edges": [
                    {"from": "ask", "port": "then", "to": "yes"},
                    {"from": "ask", "port": "else", "to": "no"}
                ]}"#,
        )
        .expect("a graph asking about a name nobody provides");
        let mut problems: Vec<String> = Vec::new();
        play_graph_reporting(
            &keyboard,
            &graph,
            &Playback::default(),
            &|_| None,
            &mut |_| {},
            &mut problems,
        )
        .expect("played");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("cpu_temp"), "{problems:?}");
        assert!(
            problems[0].contains("nothing") || problems[0].contains("provide"),
            "{problems:?}"
        );
    }

    #[test]
    fn a_graph_plays_to_the_schedule_its_waits_describe() {
        let Some(keyboard) = a_keyboard() else {
            eprintln!("no virtual keyboard here; skipping the graph timing test");
            return;
        };
        // two waits of 60ms: the whole thing takes about 120ms and the second node lands at about 60
        let graph = parse_graph(
            r#"{"start": "one",
                "nodes": [
                    {"id": "one", "kind": "wait", "ms": 60},
                    {"id": "two", "kind": "wait", "ms": 60}
                ],
                "edges": [{"from": "one", "to": "two"}]}"#,
        )
        .expect("a graph with waits");
        let started = Instant::now();
        let mut marks: Vec<u128> = Vec::new();
        let spent = play_graph(
            &keyboard,
            &graph,
            &Playback::default(),
            &|_| None,
            &mut |_| marks.push(started.elapsed().as_millis()),
        )
        .expect("played");
        assert_eq!(marks.len(), 2);
        assert!(
            (marks[1] as i128 - 60).abs() <= 25,
            "the second node was reached at {}ms, not about 60ms",
            marks[1]
        );
        assert!(
            spent.as_millis() >= 120 - 1,
            "the graph said it took {:?} but its waits add up to 120ms",
            spent
        );
    }
}
