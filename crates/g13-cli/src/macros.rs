//! `g13 macro`  -  what macros there are, what they do, and playing one.
//!
//! Macros lived only on the pad until now: the window could not list them and the terminal could not play
//! one, so the only way to test a macro was to bind it to a control and press that. That is a poor loop for
//! anything time-sensitive, and no loop at all for a macro that has never been bound.

use g13_agent::{Playback, play_graph_reporting, play_macro};
use g13_config::MacroStep;

/// What `g13 macro` was asked for: `list`, `show <id>` or `play <id> [--as <control>]`.
pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("list") | None => list(),
        Some("show") => match args.get(1) {
            Some(id) => show(id),
            None => {
                println!("g13 macro show <id>    -  what that macro does, step by step");
                2
            }
        },
        Some("play") => {
            let id = match args.get(1) {
                Some(id) => id,
                None => {
                    println!("g13 macro play <id> [--as <control>]    -  play it now");
                    return 2;
                }
            };
            let control = args
                .iter()
                .position(|arg| arg == "--as")
                .and_then(|at| args.get(at + 1))
                .cloned();
            play(id, control)
        }
        Some(other) => {
            eprintln!("g13 macro: no such thing: {other}");
            eprintln!("  g13 macro list");
            eprintln!("  g13 macro show <id>");
            eprintln!("  g13 macro play <id> [--as <control>]");
            2
        }
    }
}

/// Every macro in the configuration directory.
fn list() -> i32 {
    let dir = g13_config::config_dir();
    let mut ids: Vec<u32> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("cannot read {}", dir.display());
        return 1;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = name
            .strip_prefix("macro-")
            .and_then(|rest| rest.strip_suffix(".properties"))
            .and_then(|id| id.parse::<u32>().ok())
        {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    if ids.is_empty() {
        println!("no macros in {}", dir.display());
        return 0;
    }
    println!("{} macro(s) in {}", ids.len(), dir.display());
    for id in ids {
        let Ok(Some((macro_file, graph))) = g13_config::read_playable(id) else {
            println!("  {id:>4}  (a file that cannot be read)");
            continue;
        };
        let name = if macro_file.name.is_empty() {
            "(no name)".to_string()
        } else {
            macro_file.name.clone()
        };
        let shape = match &graph {
            Some(graph) => format!(
                "{} node(s), {} line(s)  -  it decides things",
                graph.nodes.len(),
                graph.edges.len()
            ),
            None => format!("{} step(s)", macro_file.steps.len()),
        };
        println!("  {id:>4}  {name:<28} {shape}");
    }
    0
}

/// What a macro does, as a person would read it.
fn show(id: &str) -> i32 {
    let Ok(id) = id.parse::<u32>() else {
        eprintln!("g13 macro show: {id} is not a macro id");
        return 2;
    };
    let (macro_file, graph) = match g13_config::read_playable(id) {
        Ok(Some(both)) => both,
        Ok(None) => {
            eprintln!("no macro {id}: {}", g13_config::macro_path(id).display());
            return 1;
        }
        Err(problem) => {
            eprintln!("macro {id}: {problem}");
            return 1;
        }
    };
    let name = if macro_file.name.is_empty() {
        "(no name)".to_string()
    } else {
        macro_file.name.clone()
    };
    println!("macro {id} \"{name}\"");
    if macro_file.steps.is_empty() {
        println!("  no steps in {}", g13_config::macro_path(id).display());
    } else {
        println!("  steps, in {}", g13_config::macro_path(id).display());
        for (index, step) in macro_file.steps.iter().enumerate() {
            println!("    {:>3}. {}", index + 1, describe_step(step));
        }
    }
    match graph {
        None => {
            println!("  no graph: nothing to decide, so it is always those steps in that order")
        }
        Some(graph) => {
            println!(
                "  graph, in {}  -  starts at {}",
                g13_config::graph_path(id).display(),
                graph.start
            );
            if graph.sources.is_empty() {
                println!("    reads nothing of its own");
            } else {
                println!("    reads:");
                for (name, spec) in &graph.sources {
                    println!("      {name} = {spec}");
                }
            }
            for node in &graph.nodes {
                let leaving = graph.ports_of(&node.id);
                let goes = if leaving.is_empty() {
                    "ends here".to_string()
                } else {
                    leaving
                        .iter()
                        .filter_map(|port| {
                            graph
                                .next(&node.id, port)
                                .ok()
                                .flatten()
                                .map(|to| format!("{port} -> {to}"))
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                println!("    {}: {} [{}]", node.id, describe_node(&node.kind), goes);
            }
        }
    }
    0
}

/// Play a macro now, however it is stored.
fn play(id: &str, control: Option<String>) -> i32 {
    let Ok(id) = id.parse::<u32>() else {
        eprintln!("g13 macro play: {id} is not a macro id");
        return 2;
    };
    let (macro_file, graph) = match g13_config::read_playable(id) {
        Ok(Some(both)) => both,
        Ok(None) => {
            eprintln!("no macro {id}: {}", g13_config::macro_path(id).display());
            return 1;
        }
        Err(problem) => {
            eprintln!("macro {id}: {problem}");
            return 1;
        }
    };
    let keyboard = match g13_device::keyboard::VirtualKeyboard::new() {
        Ok(keyboard) => std::sync::Arc::new(std::sync::Mutex::new(keyboard)),
        Err(error) => {
            eprintln!("cannot make a keyboard to type with: {error}");
            return 1;
        }
    };
    let config_dir = g13_config::config_dir();
    let playback = Playback {
        control,
        profile: g13_config::read_active_profile(),
        values: g13_agent::visuals::read_published_values(),
        // the same world the driver gives a macro, so `macro play` runs the same command the pad would
        world: g13_sources::World::default()
            .with_endpoints(&config_dir.join("endpoints.json"))
            .with_values(&g13_agent::visuals::values_path()),
    };
    let mut problems: Vec<String> = Vec::new();
    let outcome = match &graph {
        Some(graph) => {
            let load = |id: u32| g13_config::read_playable(id).ok().flatten();
            let mut on_step = |node: &str| println!("  {node}");
            play_graph_reporting(
                &keyboard,
                graph,
                &playback,
                &load,
                &mut on_step,
                &mut problems,
            )
            .map(|_| ())
        }
        None => play_macro(&keyboard, &macro_file, 1),
    };
    // a source that would not read is said out loud: a macro that took the wrong branch quietly is worse
    // than one that says why
    for problem in &problems {
        eprintln!("macro {id}: a source did not read: {problem}");
    }
    match outcome {
        Ok(()) if problems.is_empty() => 0,
        Ok(()) => 1,
        Err(problem) => {
            eprintln!("macro {id}: {problem}");
            1
        }
    }
}

/// One step of a step-list macro, as the numbered list prints it.
fn describe_step(step: &MacroStep) -> String {
    match step {
        MacroStep::KeyDown(code) => format!(
            "press   {code}  ({})",
            g13_device::keyboard::name_or_code(*code)
        ),
        MacroStep::KeyUp(code) => format!(
            "release {code}  ({})",
            g13_device::keyboard::name_or_code(*code)
        ),
        MacroStep::Delay(ms) => format!("wait    {ms}ms"),
    }
}

/// One node of a macro graph, with its kind spelled out.
fn describe_node(kind: &g13_config::NodeKind) -> String {
    match kind {
        g13_config::NodeKind::Key { code, hold, mode } => {
            let what = match mode {
                g13_config::KeyMode::Tap => "tap",
                g13_config::KeyMode::Down => "hold down",
                g13_config::KeyMode::Up => "release",
            };
            match mode {
                g13_config::KeyMode::Tap => {
                    format!(
                        "{what} {} ({}) for {hold}ms",
                        code,
                        g13_device::keyboard::name_or_code(*code)
                    )
                }
                _ => format!(
                    "{what} {} ({})",
                    code,
                    g13_device::keyboard::name_or_code(*code)
                ),
            }
        }
        g13_config::NodeKind::Wait { ms } => format!("wait {ms}ms"),
        g13_config::NodeKind::Type { text, per_key } => {
            format!("type {text:?} {per_key}ms apart")
        }
        g13_config::NodeKind::If { when } => format!("if {}", describe_cond(when)),
        g13_config::NodeKind::Repeat { times } => format!("repeat {times} times"),
        g13_config::NodeKind::Call { macro_id } => format!("play macro {macro_id}"),
        g13_config::NodeKind::Run { spec } => format!("run {spec}"),
        g13_config::NodeKind::Stop => "stop".to_string(),
    }
}

/// A graph condition in the words it tests, so an `if` reads as what it checks.
fn describe_cond(condition: &g13_values::Cond) -> String {
    match condition {
        g13_values::Cond::Control(control) => format!("it was fired by {control}"),
        g13_values::Cond::Profile(profile) => format!("profile {profile} is active"),
        g13_values::Cond::Value {
            name,
            op,
            number,
            text,
        } => {
            let op = match op {
                g13_values::Compare::Eq => "==",
                g13_values::Compare::Ne => "!=",
                g13_values::Compare::Lt => "<",
                g13_values::Compare::Le => "<=",
                g13_values::Compare::Gt => ">",
                g13_values::Compare::Ge => ">=",
                g13_values::Compare::Contains => "contains",
            };
            match (number, text) {
                (_, Some(text)) => format!("{name} {op} {text:?}"),
                (Some(number), None) => format!("{name} {op} {number}"),
                (None, None) => name.to_string(),
            }
        }
        g13_values::Cond::All(parts) => format!("all of: {}", describe_all(parts)),
        g13_values::Cond::Any(parts) => format!("any of: {}", describe_all(parts)),
        g13_values::Cond::Not(inner) => format!("not ({})", describe_cond(inner)),
    }
}

/// Several conditions in one line, joined so a list of them still reads as a sentence.
fn describe_all(parts: &[g13_values::Cond]) -> String {
    parts
        .iter()
        .map(describe_cond)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use g13_config::{KeyMode, NodeKind};
    use g13_values::{Compare, Cond};

    /// One of every node kind. A kind added to the format without being added here is a kind this test stops
    /// covering, which is the moment to add it to both registers rather than a release later.
    fn every_kind() -> Vec<NodeKind> {
        vec![
            NodeKind::Key {
                code: 16,
                hold: 30,
                mode: KeyMode::Tap,
            },
            NodeKind::Wait { ms: 100 },
            NodeKind::Type {
                text: "hi".to_string(),
                per_key: 30,
            },
            NodeKind::If {
                when: Cond::Control("G1".to_string()),
            },
            NodeKind::Repeat { times: 3 },
            NodeKind::Call { macro_id: 4 },
            NodeKind::Run {
                spec: "cmd:uptime".to_string(),
            },
            NodeKind::Stop,
        ]
    }

    /// Both registers, and the same facts in each: a node kind that one of them forgets, or one that loses the
    /// number it is about, is what this exists to catch.
    #[test]
    fn every_kind_reads_in_both_registers_and_carries_the_same_facts() {
        for kind in every_kind() {
            let technical = describe_node(&kind);
            let prose = g13_gui::macros::node_words(&kind);
            assert!(
                !technical.trim().is_empty(),
                "{kind:?} says nothing in the terminal's words"
            );
            assert!(
                !prose.trim().is_empty(),
                "{kind:?} says nothing in the window's words"
            );
            match &kind {
                NodeKind::Key { code, .. } => {
                    let name = g13_device::keyboard::name_or_code(*code);
                    assert!(
                        technical.contains(&name),
                        "the terminal's words for {kind:?} lost the key: {technical}"
                    );
                    assert!(
                        prose.contains(&name),
                        "the window's words for {kind:?} lost the key: {prose}"
                    );
                }
                NodeKind::Wait { ms } => {
                    let said = ms.to_string();
                    assert!(technical.contains(&said) && prose.contains(&said));
                }
                NodeKind::Repeat { times } => {
                    let said = times.to_string();
                    assert!(technical.contains(&said) && prose.contains(&said));
                }
                NodeKind::Call { macro_id } => {
                    let said = macro_id.to_string();
                    assert!(technical.contains(&said) && prose.contains(&said));
                }
                _ => {}
            }
        }
    }

    /// Every comparison in both registers: one is the file's spelling and one is a person's, and neither may
    /// be missing a case.
    #[test]
    fn every_comparison_has_a_word_in_each_register() {
        let all = [
            Compare::Eq,
            Compare::Ne,
            Compare::Lt,
            Compare::Le,
            Compare::Gt,
            Compare::Ge,
            Compare::Contains,
        ];
        for op in all {
            assert!(
                !g13_values::op_text(op).trim().is_empty(),
                "{op:?} has no word for the file"
            );
            assert!(
                !g13_gui::macros::op_words(op).trim().is_empty(),
                "{op:?} has no word for the window"
            );
        }
    }
}
