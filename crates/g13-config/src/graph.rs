//! The macro graph: a macro with branches in it.
//!
//! A recorded macro is a flat list of steps in `macro-<id>.properties`, and that file stays exactly as it is -
//! the previous stack reads it, so its shape is not mine to change. A macro that needs to *decide* something
//! gets a file beside it instead, `macro-<id>.nodes.json`, and the player uses that when it exists.
//!
//! The shape is deliberately small: nodes with an id, a kind and a place on the editor's canvas, and edges
//! between them that name a port. A plain edge leaves by `out`; an `if` has `then` and `else`; a `repeat` has
//! `body` and `then`. That is enough for branches and loops without ports, types or a schema to learn.
//!
//! Two rules the format keeps, because both have cost this project time elsewhere:
//!
//! - **Every error names the node.** `node n4 (key): no "code"` says where to look, and the editor can point
//!   at it. A graph that half-loads is worse than one that refuses.
//! - **Positions are the editor's business.** `x` and `y` are where the box sits, nothing more; the player
//!   ignores them, so a graph that is never opened in the window still plays.

use g13_values::{Compare, Cond, parse_cond};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where a macro's graph lives, beside the steps the previous stack reads.
pub fn graph_path(id: u32) -> PathBuf {
    crate::config_dir().join(format!("macro-{id}.nodes.json"))
}

/// A macro with branches.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroGraph {
    /// The node a playback starts at.
    pub start: String,
    /// Every box, in the order the file lists them.
    pub nodes: Vec<GraphNode>,
    /// Every line between two boxes.
    pub edges: Vec<GraphEdge>,
    /// What this macro reads, in the same syntax an applet declares its sources in -
    /// `{"cpu_temp": "cmd:sensors | ..."}`. A question can then name `cpu_temp`.
    ///
    /// They are read **when the question is asked**, not when the macro was fired, so "is the cpu over 50" is
    /// about now. A source here shadows a published value of the same name, because saying it here is the more
    /// specific thing to say.
    pub sources: BTreeMap<String, String>,
}

/// One box on the canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    /// What nodes and edges call each other by; no two nodes in a graph share one.
    pub id: String,
    /// What this box does.
    pub kind: NodeKind,
    /// Where the editor draws it. The player ignores this.
    pub x: f32,
    /// As `x`: the editor's own row, which the player ignores.
    pub y: f32,
}

/// What a box does.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    /// A key. `tap` presses and releases it, `down` leaves it held for a later node, `up` releases it.
    Key {
        /// The keycode to send, in the numbering a passthrough binding uses.
        code: u16,
        /// How long a tap holds it, in milliseconds.
        hold: u32,
        /// Whether the key is tapped, left held, or released; a tap also uses `hold`.
        mode: KeyMode,
    },
    /// Do nothing for a while.
    Wait {
        /// How long to wait, in milliseconds.
        ms: u32,
    },
    /// Type a string, one character at a time.
    Type {
        /// What to type.
        text: String,
        /// The gap between characters, in milliseconds.
        per_key: u32,
    },
    /// Go one way or the other.
    If {
        /// The question to ask; the node leaves by `then` or `else`.
        when: Cond,
    },
    /// Run the body this many times, then carry on.
    Repeat {
        /// How many times the body runs before carrying on.
        times: u32,
    },
    /// Play another macro.
    Call {
        /// Which macro to play, by its id.
        macro_id: u32,
    },
    /// Do something outside the keyboard: run a command, ask an endpoint. This is the output side - a macro
    /// that tells you the cpu was too hot, or that turns something on.
    ///
    /// `spec` is a source spec, the same as an applet's (`cmd:notify-send ...`), and what it returns is
    /// discarded. A spec that fails stops the macro and says so: an action that did not happen must not look
    /// like one that did.
    Run {
        /// A source spec, in the same shape an applet declares a source in: `cmd:notify-send done`.
        spec: String,
    },
    /// Finish here.
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// What a key node does with its key.
pub enum KeyMode {
    /// Press it, wait for `hold`, and release it.
    Tap,
    /// Press it and leave it held, for a later node to release.
    Down,
    /// Release a key an earlier node left held.
    Up,
}

/// A line between two boxes. `port` is `out` unless the node it leaves has more than one way to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    /// The id of the node this line leaves.
    pub from: String,
    /// Which way out of that node it is: `out` for most, `then` or `else` for an `if`.
    pub port: String,
    /// The id of the node it arrives at.
    pub to: String,
}

impl GraphNode {
    /// The port this node leaves by when the walker is not choosing.
    pub fn port(&self) -> &'static str {
        match self.kind {
            NodeKind::If { .. } => "then",
            NodeKind::Repeat { .. } => "then",
            _ => "out",
        }
    }
}

impl MacroGraph {
    /// The node with this id, if the graph has one.
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// Where a node goes next.
    ///
    /// A node that leaves by `out` may have exactly one line, so a graph with two is a mistake worth naming
    /// rather than silently picking the first.
    pub fn next(&self, from: &str, port: &str) -> Result<Option<&str>, String> {
        let leaving: Vec<&GraphEdge> = self
            .edges
            .iter()
            .filter(|edge| edge.from == from && edge.port == port)
            .collect();
        match leaving.len() {
            0 => Ok(None),
            1 => {
                let to = leaving[0].to.as_str();
                if self.node(to).is_none() {
                    return Err(format!("{from} goes to {to}, which is not a node"));
                }
                Ok(Some(to))
            }
            _ => Err(format!(
                "{from} leaves by {port} {} times; a port carries one line",
                leaving.len()
            )),
        }
    }

    /// Every port that leaves this node, whatever it is called. For the editor and for saying what is wrong.
    pub fn ports_of(&self, id: &str) -> Vec<String> {
        let mut ports: Vec<String> = self
            .edges
            .iter()
            .filter(|edge| edge.from == id)
            .map(|edge| edge.port.clone())
            .collect();
        ports.sort();
        ports.dedup();
        ports
    }
}

/// Read a graph.
pub fn parse_graph(text: &str) -> Result<MacroGraph, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("not JSON: {error}"))?;

    let start = value
        .get("start")
        .and_then(|start| start.as_str())
        .unwrap_or_default()
        .to_string();

    let Some(raw_nodes) = value.get("nodes").and_then(|nodes| nodes.as_array()) else {
        return Err(
            "no \"nodes\": a graph with nothing in it is a macro that does nothing".to_string(),
        );
    };

    let mut nodes: Vec<GraphNode> = Vec::new();
    for (index, raw) in raw_nodes.iter().enumerate() {
        let id = raw
            .get("id")
            .and_then(|id| id.as_str())
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            return Err(format!("node {index} has no \"id\""));
        }
        if nodes.iter().any(|node| node.id == id) {
            return Err(format!("two nodes are both called {id}"));
        }
        let kind_text = raw
            .get("kind")
            .and_then(|kind| kind.as_str())
            .unwrap_or_default();
        let kind = parse_kind(&id, kind_text, raw)?;
        nodes.push(GraphNode {
            id,
            kind,
            x: raw.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
            y: raw.get("y").and_then(|y| y.as_f64()).unwrap_or(0.0) as f32,
        });
    }

    let mut edges: Vec<GraphEdge> = Vec::new();
    for raw in value
        .get("edges")
        .and_then(|edges| edges.as_array())
        .into_iter()
        .flatten()
    {
        let from = raw
            .get("from")
            .and_then(|from| from.as_str())
            .unwrap_or_default()
            .to_string();
        let to = raw
            .get("to")
            .and_then(|to| to.as_str())
            .unwrap_or_default()
            .to_string();
        if !nodes.iter().any(|node| node.id == from) {
            return Err(format!("an edge leaves {from}, which is not a node"));
        }
        if !nodes.iter().any(|node| node.id == to) {
            return Err(format!("an edge arrives at {to}, which is not a node"));
        }
        edges.push(GraphEdge {
            from,
            port: raw
                .get("port")
                .and_then(|port| port.as_str())
                .unwrap_or("out")
                .to_string(),
            to,
        });
    }

    let mut sources: BTreeMap<String, String> = BTreeMap::new();
    if let Some(raw) = value.get("sources") {
        let Some(object) = raw.as_object() else {
            return Err("\"sources\" is not an object of name and spec".to_string());
        };
        for (name, spec) in object {
            let Some(spec) = spec.as_str() else {
                return Err(format!("source {name} is not a string"));
            };
            if spec.trim().is_empty() {
                return Err(format!("source {name} says nothing"));
            }
            sources.insert(name.clone(), spec.to_string());
        }
    }

    if start.is_empty() {
        // one node with nothing arriving at it is the start, and a graph with several is one that has to say
        let mut arriving: BTreeMap<&str, usize> =
            nodes.iter().map(|node| (node.id.as_str(), 0)).collect();
        for edge in &edges {
            *arriving.entry(edge.to.as_str()).or_default() += 1;
        }
        let roots: Vec<&str> = nodes
            .iter()
            .filter(|node| arriving.get(node.id.as_str()).copied().unwrap_or(0) == 0)
            .map(|node| node.id.as_str())
            .collect();
        if roots.len() != 1 {
            return Err(format!(
                "no \"start\", and {} nodes have nothing arriving at them, so which is first is not known",
                roots.len()
            ));
        }
        return Ok(MacroGraph {
            start: roots[0].to_string(),
            nodes,
            edges,
            sources,
        });
    }
    if !nodes.iter().any(|node| node.id == start) {
        return Err(format!("start is {start}, which is not a node"));
    }
    Ok(MacroGraph {
        start,
        nodes,
        edges,
        sources,
    })
}

/// One node of a graph file as its typed kind; every error names the node, so the editor can point at it.
fn parse_kind(id: &str, kind: &str, raw: &serde_json::Value) -> Result<NodeKind, String> {
    let number = |key: &str| raw.get(key).and_then(|value| value.as_u64());
    let text = |key: &str| {
        raw.get(key)
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
    };
    // every message names the node, so the editor can point at the box that is wrong
    let named = |what: String| format!("node {id} ({kind}): {what}");
    match kind {
        "key" => {
            let code = number("code").ok_or_else(|| named("no \"code\"".to_string()))?;
            if code > u16::MAX as u64 {
                return Err(named(format!("\"code\" {code} is not a key code")));
            }
            let mode = match text("mode").as_deref() {
                None | Some("tap") => KeyMode::Tap,
                Some("down") => KeyMode::Down,
                Some("up") => KeyMode::Up,
                Some(other) => {
                    return Err(named(format!(
                        "\"mode\" is {other}; it can be tap, down or up"
                    )));
                }
            };
            let hold = number("hold").unwrap_or(30);
            if hold > 60_000 {
                return Err(named(format!("\"hold\" {hold}ms is longer than a minute")));
            }
            Ok(NodeKind::Key {
                code: code as u16,
                hold: hold as u32,
                mode,
            })
        }
        "wait" => Ok(NodeKind::Wait {
            ms: number("ms").unwrap_or(100).min(600_000) as u32,
        }),
        "type" => Ok(NodeKind::Type {
            text: text("text").ok_or_else(|| named("no \"text\"".to_string()))?,
            per_key: number("per_key").unwrap_or(30).min(10_000) as u32,
        }),
        "if" => {
            let when = raw
                .get("when")
                .ok_or_else(|| named("no \"when\"".to_string()))?;
            Ok(NodeKind::If {
                when: parse_cond(when).map_err(&named)?,
            })
        }
        "repeat" => {
            let times = number("times").ok_or_else(|| named("no \"times\"".to_string()))?;
            if times == 0 {
                return Err(named(
                    "\"times\" is 0, which is a loop that never runs".to_string(),
                ));
            }
            if times > 10_000 {
                return Err(named(format!(
                    "\"times\" {times} is more than this will run"
                )));
            }
            Ok(NodeKind::Repeat {
                times: times as u32,
            })
        }
        "call" => Ok(NodeKind::Call {
            macro_id: number("macro").ok_or_else(|| named("no \"macro\"".to_string()))? as u32,
        }),
        "run" => {
            // `"spec": "cmd:..."` is the full form; `"cmd": "..."` is the shortcut for the common one, because
            // a person writing an action by hand writes the command and not the scheme
            let spec = match (text("spec"), text("cmd")) {
                (Some(spec), _) if !spec.trim().is_empty() => spec,
                (_, Some(command)) if !command.trim().is_empty() => format!("cmd:{command}"),
                _ => {
                    return Err(named(
                        "no \"spec\": give it what to do, e.g. \"cmd:notify-send done\""
                            .to_string(),
                    ));
                }
            };
            Ok(NodeKind::Run { spec })
        }
        "stop" => Ok(NodeKind::Stop),
        "" => Err(named("no \"kind\"".to_string())),
        other => Err(named(format!(
            "\"kind\" is {other}; this build runs key, wait, type, if, repeat, call, run and stop"
        ))),
    }
}

/// Read a macro's graph, if it has one.
pub fn read_graph(id: u32) -> Result<Option<MacroGraph>, String> {
    let path = graph_path(id);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse_graph(&text).map(Some)
}

/// Where a graph is written.
pub fn write_graph(id: u32, graph: &MacroGraph) -> Result<PathBuf, String> {
    let path = graph_path(id);
    let text = graph_to_json(graph);
    // beside and renamed, and the directory made, both by `g13-files`
    g13_files::write(&path, &text)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(path)
}

/// A graph as the file holds it.
///
/// Written by hand rather than by a derive, so the keys are in a fixed order and a diff of two graphs is
/// readable: this file is meant to be looked at.
pub fn graph_to_json(graph: &MacroGraph) -> String {
    let mut out = String::from("{\n  \"start\": ");
    out.push_str(&json_string(&graph.start));
    if !graph.sources.is_empty() {
        // written only when there are some, so a graph that reads nothing keeps the file it had
        out.push_str(",\n  \"sources\": {\n");
        for (index, (name, spec)) in graph.sources.iter().enumerate() {
            out.push_str(&format!("    {}: {}", json_string(name), json_string(spec)));
            if index + 1 < graph.sources.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("  }");
    }
    out.push_str(",\n  \"nodes\": [\n");
    for (index, node) in graph.nodes.iter().enumerate() {
        out.push_str("    {");
        out.push_str(&format!("\"id\": {}, ", json_string(&node.id)));
        out.push_str(&node_kind_json(&node.kind));
        out.push_str(&format!(", \"x\": {}, \"y\": {}", node.x, node.y));
        out.push('}');
        if index + 1 < graph.nodes.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ],\n  \"edges\": [\n");
    for (index, edge) in graph.edges.iter().enumerate() {
        out.push_str("    {");
        out.push_str(&format!("\"from\": {}, ", json_string(&edge.from)));
        if edge.port != "out" {
            out.push_str(&format!("\"port\": {}, ", json_string(&edge.port)));
        }
        out.push_str(&format!("\"to\": {}", json_string(&edge.to)));
        out.push('}');
        if index + 1 < graph.edges.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ]\n}\n");
    out
}

/// One node's kind and its own keys as the body of a node in a written graph file.
fn node_kind_json(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Key { code, hold, mode } => format!(
            "\"kind\": \"key\", \"code\": {code}, \"hold\": {hold}, \"mode\": {}",
            match mode {
                KeyMode::Tap => "\"tap\"",
                KeyMode::Down => "\"down\"",
                KeyMode::Up => "\"up\"",
            }
        ),
        NodeKind::Wait { ms } => format!("\"kind\": \"wait\", \"ms\": {ms}"),
        NodeKind::Type { text, per_key } => format!(
            "\"kind\": \"type\", \"text\": {}, \"per_key\": {per_key}",
            json_string(text)
        ),
        NodeKind::If { when } => format!("\"kind\": \"if\", \"when\": {}", cond_json(when)),
        NodeKind::Repeat { times } => format!("\"kind\": \"repeat\", \"times\": {times}"),
        NodeKind::Call { macro_id } => format!("\"kind\": \"call\", \"macro\": {macro_id}"),
        NodeKind::Run { spec } => {
            format!("\"kind\": \"run\", \"spec\": {}", json_string(spec))
        }
        NodeKind::Stop => "\"kind\": \"stop\"".to_string(),
    }
}

/// One condition as the JSON object a graph file writes, nested for `all`, `any` and `not`.
fn cond_json(cond: &Cond) -> String {
    match cond {
        Cond::Control(control) => format!("{{\"control\": {}}}", json_string(control)),
        Cond::Profile(profile) => format!("{{\"profile\": {profile}}}"),
        Cond::Value {
            name,
            op,
            number,
            text,
        } => {
            let op = match op {
                Compare::Eq => "==",
                Compare::Ne => "!=",
                Compare::Lt => "<",
                Compare::Le => "<=",
                Compare::Gt => ">",
                Compare::Ge => ">=",
                Compare::Contains => "contains",
            };
            match (number, text) {
                (_, Some(text)) => format!(
                    "{{\"value\": {}, \"op\": {}, \"text\": {}}}",
                    json_string(name),
                    json_string(op),
                    json_string(text)
                ),
                (Some(number), None) => format!(
                    "{{\"value\": {}, \"op\": {}, \"to\": {number}}}",
                    json_string(name),
                    json_string(op)
                ),
                (None, None) => format!("{{\"value\": {}}}", json_string(name)),
            }
        }
        Cond::All(parts) => format!(
            "{{\"all\": [{}]}}",
            parts.iter().map(cond_json).collect::<Vec<_>>().join(", ")
        ),
        Cond::Any(parts) => format!(
            "{{\"any\": [{}]}}",
            parts.iter().map(cond_json).collect::<Vec<_>>().join(", ")
        ),
        Cond::Not(inner) => format!("{{\"not\": {}}}", cond_json(inner)),
    }
}

/// Text as a quoted JSON string, escaped by hand so nothing else is pulled in for it.
fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            character if (character as u32) < 0x20 => out.push(' '),
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

/// Read a macro's steps, whether it has a graph or a plain sequence.
///
/// The graph wins when there is one, because that is the file the editor writes.
pub fn read_playable(id: u32) -> Result<Option<(crate::Macro, Option<MacroGraph>)>, String> {
    let steps = std::fs::read_to_string(crate::macro_path(id)).unwrap_or_default();
    let macro_file = crate::parse_macro(&steps);
    let graph = read_graph(id)?;
    if macro_file.steps.is_empty() && graph.is_none() {
        return Ok(None);
    }
    Ok(Some((macro_file, graph)))
}

/// Whether a file is a graph, for anything listing a directory.
pub fn is_graph_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(".nodes.json"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A_GRAPH: &str = r#"{
        "start": "n1",
        "nodes": [
            {"id": "n1", "kind": "key", "code": 20, "hold": 30, "mode": "tap", "x": 40, "y": 40},
            {"id": "n2", "kind": "wait", "ms": 120, "x": 40, "y": 100},
            {"id": "n3", "kind": "if", "when": {"control": "G7"}, "x": 40, "y": 160},
            {"id": "n4", "kind": "type", "text": "hello", "per_key": 25, "x": 40, "y": 220},
            {"id": "n5", "kind": "repeat", "times": 3, "x": 40, "y": 280},
            {"id": "n6", "kind": "call", "macro": 5, "x": 40, "y": 340},
            {"id": "n7", "kind": "stop", "x": 40, "y": 400}
        ],
        "edges": [
            {"from": "n1", "to": "n2"},
            {"from": "n2", "to": "n3"},
            {"from": "n3", "port": "then", "to": "n4"},
            {"from": "n3", "port": "else", "to": "n7"},
            {"from": "n4", "to": "n5"},
            {"from": "n5", "port": "body", "to": "n1"},
            {"from": "n5", "port": "then", "to": "n6"},
            {"from": "n6", "to": "n7"}
        ]
    }"#;

    #[test]
    fn a_graph_reads_and_writes_back_the_same() {
        let graph = parse_graph(A_GRAPH).expect("the graph in the format's own example");
        assert_eq!(graph.start, "n1");
        assert_eq!(graph.nodes.len(), 7);
        assert_eq!(graph.edges.len(), 8);
        assert_eq!(
            graph.node("n1").map(|node| node.kind.clone()),
            Some(NodeKind::Key {
                code: 20,
                hold: 30,
                mode: KeyMode::Tap
            })
        );
        // written and read again, it is the same graph: the file is meant to be read by a person
        let again = parse_graph(&graph_to_json(&graph)).expect("what it wrote");
        assert_eq!(again, graph, "a graph does not survive a round trip");
    }

    #[test]
    fn a_port_carries_one_line_and_only_one() {
        let graph = parse_graph(A_GRAPH).unwrap();
        assert_eq!(graph.next("n1", "out"), Ok(Some("n2")));
        assert_eq!(graph.next("n3", "then"), Ok(Some("n4")));
        assert_eq!(graph.next("n3", "else"), Ok(Some("n7")));
        // a node with nothing leaving by that port ends there rather than failing
        assert_eq!(graph.next("n7", "out"), Ok(None));
        assert_eq!(
            graph.ports_of("n5"),
            vec!["body".to_string(), "then".to_string()]
        );

        // two lines out of one port is a mistake, and it is named rather than the first being picked
        let doubled = parse_graph(
            r#"{"nodes":[{"id":"a","kind":"stop"},{"id":"b","kind":"stop"},{"id":"c","kind":"stop"}],
                "edges":[{"from":"a","to":"b"},{"from":"a","to":"c"}]}"#,
        )
        .unwrap();
        let problem = doubled.next("a", "out").expect_err("two lines out");
        assert!(problem.contains("a leaves by out 2 times"), "{problem}");
    }

    #[test]
    fn a_graph_can_declare_what_it_reads_and_an_action_to_take() {
        // the same syntax an applet uses, so a person has one way of writing a source and not two
        let graph = parse_graph(
            r#"{"start": "ask",
                "sources": {"cpu_temp": "cmd:sensors", "office": "http:http://127.0.0.1:8080/t#temp"},
                "nodes": [
                    {"id": "ask", "kind": "if", "when": {"value": "cpu_temp", "op": ">", "to": 70}},
                    {"id": "tell", "kind": "run", "spec": "cmd:notify-send hot"},
                    {"id": "quiet", "kind": "run", "cmd": "notify-send fine"}
                ],
                "edges": [
                    {"from": "ask", "port": "then", "to": "tell"},
                    {"from": "ask", "port": "else", "to": "quiet"}
                ]}"#,
        )
        .expect("a graph that reads and acts");
        assert_eq!(
            graph.sources.get("cpu_temp").map(String::as_str),
            Some("cmd:sensors")
        );
        assert_eq!(graph.sources.len(), 2);
        assert_eq!(
            graph.node("tell").map(|node| node.kind.clone()),
            Some(NodeKind::Run {
                spec: "cmd:notify-send hot".to_string()
            })
        );
        // `"cmd": "..."` is the shortcut, and it means the same thing
        assert_eq!(
            graph.node("quiet").map(|node| node.kind.clone()),
            Some(NodeKind::Run {
                spec: "cmd:notify-send fine".to_string()
            })
        );

        // and it comes back out of the file as it went in
        let text = graph_to_json(&graph);
        assert!(text.contains("\"sources\""), "{text}");
        let again = parse_graph(&text).expect("its own output");
        assert_eq!(again, graph);

        // a run with nothing to do is refused rather than written as a no-op
        let empty = parse_graph(r#"{"nodes":[{"id":"a","kind":"run","spec":"  "}]}"#).unwrap_err();
        assert!(empty.contains("no \"spec\""), "{empty}");
        // a source that says nothing is refused too
        let blank =
            parse_graph(r#"{"sources":{"a":""},"nodes":[{"id":"a","kind":"stop"}]}"#).unwrap_err();
        assert!(blank.contains("says nothing"), "{blank}");
        // and a graph with no sources writes none, so a graph that reads nothing keeps the file it had
        let plain = parse_graph(r#"{"nodes":[{"id":"a","kind":"stop"}]}"#).expect("a plain graph");
        assert!(
            !graph_to_json(&plain).contains("sources"),
            "{}",
            graph_to_json(&plain)
        );
    }

    #[test]
    fn every_problem_names_the_node_it_is_about() {
        // a node the player cannot run is refused with its own name, so the editor can point at the box
        let no_code = parse_graph(r#"{"nodes":[{"id":"n9","kind":"key"}]}"#).unwrap_err();
        assert_eq!(no_code, "node n9 (key): no \"code\"");

        let unknown = parse_graph(r#"{"nodes":[{"id":"n9","kind":"flash"}]}"#).unwrap_err();
        assert!(unknown.contains("n9 (flash)"), "{unknown}");
        assert!(
            unknown.contains("key, wait, type, if, repeat, call, run and stop"),
            "{unknown}"
        );

        let twice =
            parse_graph(r#"{"nodes":[{"id":"n9","kind":"stop"},{"id":"n9","kind":"stop"}]}"#)
                .unwrap_err();
        assert!(twice.contains("two nodes are both called n9"), "{twice}");

        let nowhere = parse_graph(
            r#"{"nodes":[{"id":"n9","kind":"stop"}],"edges":[{"from":"n9","to":"n8"}]}"#,
        )
        .unwrap_err();
        assert!(nowhere.contains("arrives at n8"), "{nowhere}");

        let no_kind = parse_graph(r#"{"nodes":[{"id":"n9"}]}"#).unwrap_err();
        assert!(no_kind.contains("no \"kind\""), "{no_kind}");

        let empty_loop =
            parse_graph(r#"{"nodes":[{"id":"n9","kind":"repeat","times":0}]}"#).unwrap_err();
        assert!(empty_loop.contains("never runs"), "{empty_loop}");

        let pointless =
            parse_graph(r#"{"nodes":[{"id":"n9","kind":"if","when":{"all":[]}}]}"#).unwrap_err();
        assert!(pointless.contains("asks nothing"), "{pointless}");

        let bad_op = parse_graph(
            r#"{"nodes":[{"id":"n9","kind":"if","when":{"value":"cpu","op":"~","to":5}}]}"#,
        )
        .unwrap_err();
        assert!(bad_op.contains("op"), "{bad_op}");

        // and a condition with nothing to compare against is not a question
        let no_target =
            parse_graph(r#"{"nodes":[{"id":"n9","kind":"if","when":{"value":"cpu","op":">"}}]}"#)
                .unwrap_err();
        assert!(no_target.contains("compared with nothing"), "{no_target}");
    }

    #[test]
    fn a_start_is_worked_out_when_it_is_not_said() {
        // one node with nothing arriving at it is the start
        let graph = parse_graph(
            r#"{"nodes":[{"id":"a","kind":"stop"},{"id":"b","kind":"stop"}],
                "edges":[{"from":"a","to":"b"}]}"#,
        )
        .unwrap();
        assert_eq!(graph.start, "a");

        // two of them and which is first is not known, which is worth saying rather than guessing
        let problem =
            parse_graph(r#"{"nodes":[{"id":"a","kind":"stop"},{"id":"b","kind":"stop"}]}"#)
                .unwrap_err();
        assert!(
            problem.contains("2 nodes have nothing arriving"),
            "{problem}"
        );
    }

    #[test]
    fn the_questions_a_macro_can_ask_read_back() {
        let all = parse_graph(
            r#"{"nodes":[{"id":"a","kind":"if","when":{"all":[
                    {"profile": 3},
                    {"value": "cpu", "op": ">", "to": 50},
                    {"not": {"control": "G7"}}
               ]}}]}"#,
        )
        .unwrap();
        let Some(NodeKind::If { when }) = all.node("a").map(|node| node.kind.clone()) else {
            panic!("no if node");
        };
        assert_eq!(
            when,
            Cond::All(vec![
                Cond::Profile(3),
                Cond::Value {
                    name: "cpu".to_string(),
                    op: Compare::Gt,
                    number: Some(50.0),
                    text: None
                },
                Cond::Not(Box::new(Cond::Control("G7".to_string()))),
            ])
        );
        // and it survives being written
        let again = parse_graph(&graph_to_json(&all)).unwrap();
        assert_eq!(again, all);

        let text = parse_graph(
            r#"{"nodes":[{"id":"a","kind":"if","when":{"value":"media_status","op":"contains","text":"Play"}}]}"#,
        )
        .unwrap();
        let Some(NodeKind::If { when }) = text.node("a").map(|node| node.kind.clone()) else {
            panic!("no if node");
        };
        assert!(matches!(
            when,
            Cond::Value {
                op: Compare::Contains,
                ..
            }
        ));
    }
}
