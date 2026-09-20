//! The macros tab: editing a macro where it lives, and playing it.
//!
//! The window could not list macros at all before this, so the editor is also the first place a macro can be
//! *seen*. Two shapes are edited here, and they are the two shapes the player reads:
//!
//! - **steps**, the flat `macro-<id>.properties` a recording produces and the previous stack can read;
//! - **a graph**, `macro-<id>.nodes.json`, for a macro that decides something. "make it decide things" turns
//!   the steps into a chain of nodes in place, so the thing that was recorded can be given an `if` without
//!   being rewritten by hand.
//!
//! Everything here writes straight to the file as it is edited, which is how the rest of this window works.
//! Two rules are kept:
//!
//! - **A file that would not read back is not written.** Every save re-parses what it is about to write, and a
//!   change that would produce something unreadable is refused with the reason.
//! - **Nothing is written that the player cannot run.** An unknown node kind keeps the file as it is rather
//!   than being replaced by something this build would run differently.

use g13_config::{GraphNode, KeyMode, MacroGraph, MacroStep, NodeKind};
use g13_values::{Compare, Cond};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One macro as the list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroRow {
    /// Its number: the `200` of `macro-200.properties`.
    pub id: u32,
    /// What the macro is called, as its properties file says.
    pub name: String,
    /// How many steps its flat form has.
    pub steps: usize,
    /// Nothing in it at all: no steps and no graph.
    ///
    /// The previous stack made a slot for every id it could use, so a real installation has hundreds of these.
    /// They are kept because a binding can point at one and because the previous stack reads them, but a list
    /// of two hundred empty slots buries the handful of macros that do something.
    pub empty: bool,
    /// How many nodes its graph has, if it has one.
    pub nodes: Option<usize>,
    /// What stopped it being read, if anything.
    pub problem: Option<String>,
}

/// A question, as the editor holds it: the four kinds a person actually writes.
#[derive(Debug, Clone, PartialEq)]
pub enum Question {
    /// Fired by a control, named as the map names it: `G7`, `LR`.
    Control(String),
    /// A profile number is the active one.
    Profile(u32),
    /// A published value compared with a number or a string.
    Value {
        /// The published value, or the macro's own source, being read.
        name: String,
        /// How it is compared.
        op: Compare,
        /// What it is compared with, when that is a number.
        number: Option<f64>,
        /// What it is compared with, when that is text.
        text: Option<String>,
    },
}

/// One line of a condition, with its own `not`.
#[derive(Debug, Clone, PartialEq)]
pub struct QuestionRow {
    /// Whether this row is negated.
    pub not: bool,
    /// What the row asks.
    pub question: Question,
}

/// A condition as the editor holds it: questions joined by all or any.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionForm {
    /// True for `all`, false for `any`.
    pub all: bool,
    /// The questions, joined by `all` or `any`.
    pub rows: Vec<QuestionRow>,
}

impl ConditionForm {
    /// The form for a condition, or `None` when it does not fit one.
    ///
    /// A tree of nested `all`/`any` is a real thing a hand-written graph can hold, and this editor does not
    /// pretend to draw one: it says so and leaves the file alone.
    pub fn from_cond(cond: &Cond) -> Option<ConditionForm> {
        match cond {
            Cond::Control(_) | Cond::Profile(_) | Cond::Value { .. } => Some(ConditionForm {
                all: true,
                rows: vec![QuestionRow {
                    not: false,
                    question: question_from(cond)?,
                }],
            }),
            Cond::Not(inner) => Some(ConditionForm {
                all: true,
                rows: vec![QuestionRow {
                    not: true,
                    question: question_from(inner)?,
                }],
            }),
            Cond::All(parts) | Cond::Any(parts) => {
                let mut rows = Vec::new();
                for part in parts {
                    match part {
                        Cond::Not(inner) => rows.push(QuestionRow {
                            not: true,
                            question: question_from(inner)?,
                        }),
                        other => rows.push(QuestionRow {
                            not: false,
                            question: question_from(other)?,
                        }),
                    }
                }
                Some(ConditionForm {
                    all: matches!(cond, Cond::All(_)),
                    rows,
                })
            }
        }
    }

    /// The condition this form stands for, as the file holds it.
    pub fn to_cond(&self) -> Cond {
        let parts: Vec<Cond> = self
            .rows
            .iter()
            .map(|row| {
                let question = cond_from(&row.question);
                if row.not {
                    Cond::Not(Box::new(question))
                } else {
                    question
                }
            })
            .collect();
        match parts.len() {
            0 => Cond::Control(String::new()),
            // one row is that row, with no `all` or `any` around it. The `unwrap_or` is the empty condition
            // the arm above gives, so nothing here can panic on a length nobody can arrange anyway
            1 => parts
                .into_iter()
                .next()
                .unwrap_or(Cond::Control(String::new())),
            _ => {
                if self.all {
                    Cond::All(parts)
                } else {
                    Cond::Any(parts)
                }
            }
        }
    }

    /// What this condition reads as, in one line.
    pub fn describe(&self) -> String {
        let join = if self.all { "and" } else { "or" };
        if self.rows.is_empty() {
            return "nothing yet".to_string();
        }
        self.rows
            .iter()
            .map(|row| {
                let not = if row.not { "not " } else { "" };
                format!("{not}{}", describe_question(&row.question))
            })
            .collect::<Vec<_>>()
            .join(&format!(" {join} "))
    }
}

/// The editor's question for a file's condition, or none for a shape the editor cannot draw.
fn question_from(cond: &Cond) -> Option<Question> {
    Some(match cond {
        Cond::Control(control) => Question::Control(control.clone()),
        Cond::Profile(profile) => Question::Profile(*profile),
        Cond::Value {
            name,
            op,
            number,
            text,
        } => Question::Value {
            name: name.clone(),
            op: *op,
            number: *number,
            text: text.clone(),
        },
        // a nested tree is not something this editor can draw, so it gives no form at all rather than a form
        // that would rewrite the file into something else
        Cond::All(_) | Cond::Any(_) | Cond::Not(_) => return None,
    })
}

/// The file's condition for what the editor's question says.
fn cond_from(question: &Question) -> Cond {
    match question {
        Question::Control(control) => Cond::Control(control.clone()),
        Question::Profile(profile) => Cond::Profile(*profile),
        Question::Value {
            name,
            op,
            number,
            text,
        } => Cond::Value {
            name: name.clone(),
            op: *op,
            number: *number,
            text: text.clone(),
        },
    }
}

/// A question in the words the window uses.
pub fn describe_question(question: &Question) -> String {
    match question {
        Question::Control(control) => {
            if control.is_empty() {
                "fired by ...".to_string()
            } else {
                format!("fired by {control}")
            }
        }
        Question::Profile(profile) => format!("profile {profile} is active"),
        Question::Value {
            name,
            op,
            number,
            text,
        } => {
            let op = op_words(*op);
            let against = match (number, text) {
                (_, Some(text)) => format!("{text:?}"),
                (Some(number), None) => format!("{number}"),
                (None, None) => "...".to_string(),
            };
            if name.is_empty() {
                "a value ...".to_string()
            } else {
                format!("{name} {op} {against}")
            }
        }
    }
}

/// A comparison as a person reads it: `is`, `is not`.
///
/// Not `g13_values::op_text`, which is how the *file* spells it (`==`, `!=`) - hence two names. The
/// file's spelling used to be reachable under this one too, and a call site could take either register.
pub fn op_words(op: Compare) -> &'static str {
    match op {
        Compare::Eq => "is",
        Compare::Ne => "is not",
        Compare::Lt => "<",
        Compare::Le => "<=",
        Compare::Gt => ">",
        Compare::Ge => ">=",
        Compare::Contains => "contains",
    }
}

/// The comparison a word stands for, or none when it is not one the editor offers.
pub fn op_from_text(text: &str) -> Option<Compare> {
    Some(match text {
        "is" | "==" | "=" => Compare::Eq,
        "is not" | "!=" => Compare::Ne,
        "<" => Compare::Lt,
        "<=" => Compare::Le,
        ">" => Compare::Gt,
        ">=" => Compare::Ge,
        "contains" => Compare::Contains,
        _ => return None,
    })
}

/// The ports a node can leave by, in the order the editor shows them.
///
/// A port with no line is a port the walker ends at, which is a real thing to want: `if` with nothing on
/// `else` means "otherwise, finish".
pub fn ports_of_kind(kind: &NodeKind) -> &'static [&'static str] {
    match kind {
        NodeKind::If { .. } => &["then", "else"],
        NodeKind::Repeat { .. } => &["body", "then"],
        _ => &["out"],
    }
}

/// How a node reads on the canvas.
///
/// These are the words a person would say: the box has to be worth reading on its own, because the whole point
/// of the picture is seeing the flow at a glance. A number in a box is not a flow, it is machine output.
pub fn node_words(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Key { code, hold, mode } => {
            let name = key_name(*code);
            match mode {
                // the hold is only worth saying when it is not the ordinary one
                KeyMode::Tap if *hold == 30 => format!("Press {name}"),
                KeyMode::Tap => format!("Press {name} for {hold}ms"),
                KeyMode::Down => format!("Hold {name} down"),
                KeyMode::Up => format!("Release {name}"),
            }
        }
        NodeKind::Wait { ms } => format!("Wait {ms}ms"),
        NodeKind::Type { text, per_key } => {
            if *per_key == 30 {
                format!("Type {text:?}")
            } else {
                format!("Type {text:?}, {per_key}ms apart")
            }
        }
        NodeKind::If { when } => match ConditionForm::from_cond(when) {
            Some(form) => format!("If {}", form.describe()),
            // a tree this editor does not draw is still a condition, and it is not rewritten
            None => "If (a question only the file holds)".to_string(),
        },
        NodeKind::Repeat { times } => format!("Repeat {times} times"),
        NodeKind::Call { macro_id } => format!("Play macro {macro_id}"),
        NodeKind::Run { spec } => {
            // the scheme is dropped when it is the ordinary one: "Run notify-send done" is what a person said
            match spec.strip_prefix("cmd:") {
                Some(command) => format!("Run {command}"),
                None => format!("Run {spec}"),
            }
        }
        NodeKind::Stop => "Finish".to_string(),
    }
}

/// What a port means, in words.
///
/// The file's own names are `out`, `then`, `else` and `body`, and they stay in the file. A person reading the
/// picture needs "if yes" and "when it has done".
pub fn describe_port(port: &str) -> &'static str {
    match port {
        "then" => "then (if yes, or when a repeat is done)",
        "else" => "else (if no)",
        "body" => "body (the bit a repeat does again)",
        _ => "next",
    }
}

/// The text the canvas writes on a box: what it does, and its own name underneath.
pub fn box_text(kind: &NodeKind) -> (String, String) {
    (node_words(kind), String::new())
}

/// The name of a key code, where the table has one.
///
/// The lookup is the table owner's: this used to walk `KEY_NAMES` itself and so could disagree with the
/// command line about what a code with no name is called.
pub fn key_name(code: u16) -> String {
    g13_device::keyboard::name_or_code(code)
}

/// Every macro in the configuration directory, newest id last.
pub fn list(config_dir: &std::path::Path) -> Vec<MacroRow> {
    let mut rows: Vec<MacroRow> = Vec::new();
    let Ok(entries) = std::fs::read_dir(config_dir) else {
        return rows;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name
            .strip_prefix("macro-")
            .and_then(|rest| rest.strip_suffix(".properties"))
            .and_then(|id| id.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let macro_file = g13_config::parse_macro(&text);
        let graph = read_graph_in(config_dir, id);
        let (nodes, problem) = match graph {
            Ok(graph) => (graph.map(|graph| graph.nodes.len()), None),
            Err(problem) => (None, Some(problem)),
        };
        rows.push(MacroRow {
            id,
            name: macro_file.name,
            steps: macro_file.steps.len(),
            empty: macro_file.steps.is_empty() && nodes.is_none(),
            nodes,
            problem,
        });
    }
    rows.sort_by_key(|row| row.id);
    rows
}

/// What a macro's own files hold, as the editor edits them.
#[derive(Debug, Clone, PartialEq)]
pub struct Opening {
    /// Which macro this is: the id `g13 macro show ID` takes, and the number its file is named for.
    pub id: u32,
    /// What the macro is called.
    pub name: String,
    /// Its flat form, which is what plays when it has no graph.
    pub steps: Vec<MacroStep>,
    /// Its nodes and edges, when it has been made to decide things.
    pub graph: Option<MacroGraph>,
}

/// Read a macro's graph from the directory the window is working in.
///
/// `g13_config::read_graph` reads the *configured* directory, which is right for the driver and wrong for a
/// window pointed somewhere else - a test's own folder, or a second configuration.
pub fn read_graph_in(dir: &std::path::Path, id: u32) -> Result<Option<MacroGraph>, String> {
    let path = dir.join(format!("macro-{id}.nodes.json"));
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    g13_config::parse_graph(&text).map(Some)
}

/// Read a macro for editing.
pub fn open(config_dir: &std::path::Path, id: u32) -> Result<Opening, String> {
    let path = config_dir.join(format!("macro-{id}.properties"));
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let macro_file = g13_config::parse_macro(&text);
    let graph = read_graph_in(config_dir, id)?;
    Ok(Opening {
        id,
        name: macro_file.name,
        steps: macro_file.steps,
        graph,
    })
}

/// Write a macro's steps, refusing anything that would not read back.
pub fn save_steps(
    config_dir: &std::path::Path,
    id: u32,
    name: &str,
    steps: &[MacroStep],
) -> Result<PathBuf, String> {
    let macro_file = g13_config::Macro {
        name: name.to_string(),
        id,
        steps: steps.to_vec(),
    };
    let text = g13_config::macro_to_text(&macro_file);
    // read back before writing: a file the player cannot read is worse than no change at all
    let read_back = g13_config::parse_macro(&text);
    if read_back.steps != steps {
        return Err("the steps did not read back the same, so nothing was written".to_string());
    }
    let path = config_dir.join(format!("macro-{id}.properties"));
    g13_files::write(&path, &text)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(path)
}

/// Write a macro's graph, refusing anything the player would refuse.
pub fn save_graph(
    config_dir: &std::path::Path,
    id: u32,
    graph: &MacroGraph,
) -> Result<PathBuf, String> {
    let text = g13_config::graph_to_json(graph);
    // the parser is the player's own, so a graph that saves is a graph that runs
    g13_config::parse_graph(&text)?;
    let path = config_dir.join(format!("macro-{id}.nodes.json"));
    g13_files::write(&path, &text)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(path)
}

/// Delete a macro's graph, leaving the steps the previous stack can read.
pub fn drop_graph(config_dir: &std::path::Path, id: u32) -> Result<Option<PathBuf>, String> {
    let path = config_dir.join(format!("macro-{id}.nodes.json"));
    if !path.exists() {
        return Ok(None);
    }
    std::fs::remove_file(&path)
        .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
    Ok(Some(path))
}

/// Where a graph's boxes sit, and how big they are.
///
/// The canvas is drawn from this and the drop is decided by this, so "which box did that land on" is the same
/// answer in the picture and in the test - a drag that joins the wrong box is a bug a picture cannot show.
pub const CANVAS_BOX: egui::Vec2 = egui::vec2(132.0, 46.0);
/// Empty space kept around the boxes, so one on the edge can still be read and dragged.
pub const CANVAS_MARGIN: f32 = 24.0;
/// Room above the first box for the thing that says the macro has been fired.
pub const TRIGGER_ROOM: f32 = 44.0;

/// What the flow starts from, in words: the box above the first node.
pub const TRIGGER_TEXT: &str = "when this macro runs";

/// How wide a box has to be to hold what it says: a clipped label is worse than a wide box.
pub fn box_size(node: &GraphNode) -> egui::Vec2 {
    let label = node_words(&node.kind);
    let width = (label.chars().count() as f32 * 6.6 + 26.0).max(CANVAS_BOX.x);
    egui::vec2(width, CANVAS_BOX.y)
}

/// The box for each node, positioned relative to the canvas' own origin.
pub fn canvas_layout(graph: &MacroGraph) -> Vec<(String, egui::Rect)> {
    graph
        .nodes
        .iter()
        .map(|node| {
            let at = egui::pos2(node.x, node.y) + egui::vec2(CANVAS_MARGIN, CANVAS_MARGIN);
            (
                node.id.clone(),
                egui::Rect::from_min_size(at, box_size(node)),
            )
        })
        .collect()
}

/// The node whose box contains a point, if there is one.
pub fn node_at(graph: &MacroGraph, point: egui::Pos2) -> Option<String> {
    canvas_layout(graph)
        .into_iter()
        .find(|(_, rect)| rect.contains(point))
        .map(|(id, _)| id)
}

/// How big the canvas has to be to hold every box.
pub fn canvas_size(graph: &MacroGraph, at_least: egui::Vec2) -> egui::Vec2 {
    let mut width: f32 = 0.0;
    let mut height: f32 = 0.0;
    for (_, rect) in canvas_layout(graph) {
        width = width.max(rect.right());
        height = height.max(rect.bottom());
    }
    egui::vec2(
        at_least.x.max(width + CANVAS_MARGIN),
        at_least.y.max(height + CANVAS_MARGIN),
    )
}

/// Where a port's dot is, on the right edge of its box.
///
/// The ways out are spread down the edge in the order the editor lists them, which is the order the walker
/// tries them, so the picture and the behaviour agree.
pub fn port_dot(graph: &MacroGraph, node: &GraphNode, port: &str) -> egui::Pos2 {
    let at = egui::pos2(node.x, node.y) + egui::vec2(CANVAS_MARGIN, CANVAS_MARGIN);
    let rect = egui::Rect::from_min_size(at, CANVAS_BOX);
    let ports = ports_of_kind(&node.kind);
    let index = ports
        .iter()
        .position(|candidate| *candidate == port)
        .unwrap_or(0);
    let count = ports.len().max(1);
    let step = CANVAS_BOX.y / (count as f32 + 1.0);
    let _ = graph;
    egui::pos2(rect.right(), rect.top() + step * (index as f32 + 1.0))
}

/// Where a box's way in is: the middle of its left edge.
pub fn input_dot(node: &GraphNode) -> egui::Pos2 {
    let at = egui::pos2(node.x, node.y) + egui::vec2(CANVAS_MARGIN, CANVAS_MARGIN + TRIGGER_ROOM);
    let rect = egui::Rect::from_min_size(at, box_size(node));
    egui::pos2(rect.left(), rect.center().y)
}

/// A node of a kind with no graph around it, for a test's own graph.
///
/// Test-only, and that is the point: it refuses an unknown kind by panicking, which is fine in a test and is
/// not fine in the window. The window builds nodes through `new_node`, which returns the reason.
#[cfg(test)]
pub fn new_node_in(id: &str, kind: &str) -> GraphNode {
    let graph = MacroGraph {
        start: String::new(),
        nodes: Vec::new(),
        edges: Vec::new(),
        sources: BTreeMap::new(),
    };
    match new_node(&graph, kind) {
        Ok(mut node) => {
            node.id = id.to_string();
            node
        }
        Err(problem) => panic!("{kind} is not a kind: {problem}"),
    }
}

/// The steps of a macro as a chain of nodes: what "make it decide things" writes.
///
/// It has to behave exactly as the recording did, and it has to *read* as the flow it is:
///
/// - a key pressed and released with nothing between becomes **one press** ("Press t for 111ms"), because
///   "Hold t down / Wait 111ms / Release t" is the file's spelling of it and three boxes for one press is not a
///   flow;
/// - a chord is left as holds and releases, because that is what a chord is (`Hold ctrl down`, `Hold alt down`,
///   `Press delete`, …);
/// - a wait stays a wait.
pub fn steps_as_chain(id: u32, steps: &[MacroStep], name: &str) -> MacroGraph {
    let _ = (id, name);
    // what the flow does, before it is given positions and ids
    let mut kinds: Vec<NodeKind> = Vec::new();
    let mut at = 0usize;
    while at < steps.len() {
        let kind = match (steps.get(at), steps.get(at + 1), steps.get(at + 2)) {
            (
                Some(MacroStep::KeyDown(down)),
                Some(MacroStep::Delay(ms)),
                Some(MacroStep::KeyUp(up)),
            ) if down == up => {
                at += 3;
                NodeKind::Key {
                    code: *down,
                    hold: *ms,
                    mode: KeyMode::Tap,
                }
            }
            (Some(MacroStep::KeyDown(code)), _, _) => {
                at += 1;
                NodeKind::Key {
                    code: *code,
                    hold: 30,
                    mode: KeyMode::Down,
                }
            }
            (Some(MacroStep::KeyUp(code)), _, _) => {
                at += 1;
                NodeKind::Key {
                    code: *code,
                    hold: 30,
                    mode: KeyMode::Up,
                }
            }
            (Some(MacroStep::Delay(ms)), _, _) => {
                at += 1;
                NodeKind::Wait { ms: *ms }
            }
            (None, _, _) => break,
        };
        kinds.push(kind);
    }

    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut edges: Vec<g13_config::GraphEdge> = Vec::new();
    for (index, kind) in kinds.into_iter().enumerate() {
        let node_id = format!("n{}", index + 1);
        nodes.push(GraphNode {
            id: node_id.clone(),
            kind,
            // laid out down the canvas: the editor lets them be moved, this is only a start
            x: 60.0,
            y: 30.0 + (index as f32) * 70.0,
        });
        if index > 0 {
            edges.push(g13_config::GraphEdge {
                from: format!("n{index}"),
                port: "out".to_string(),
                to: node_id,
            });
        }
    }
    let start = nodes
        .first()
        .map(|node| node.id.clone())
        .unwrap_or_default();
    MacroGraph {
        start,
        nodes,
        edges,
        sources: BTreeMap::new(),
    }
}

/// A fresh node of a kind, placed at the end of the canvas.
pub fn new_node(graph: &MacroGraph, kind: &str) -> Result<GraphNode, String> {
    // ids are the file's own, so they are small and ordered rather than random
    let mut next = graph.nodes.len() + 1;
    while graph.nodes.iter().any(|node| node.id == format!("n{next}")) {
        next += 1;
    }
    let kind = match kind {
        "key" => NodeKind::Key {
            code: 30,
            hold: 30,
            mode: KeyMode::Tap,
        },
        "wait" => NodeKind::Wait { ms: 100 },
        "type" => NodeKind::Type {
            text: String::new(),
            per_key: 30,
        },
        "if" => NodeKind::If {
            when: Cond::Control(String::new()),
        },
        "repeat" => NodeKind::Repeat { times: 2 },
        "call" => NodeKind::Call { macro_id: 1 },
        // a run box starts with the scheme and nothing else: it is refused until it says what to do, which
        // is better than a box that looks like it does something
        "run" => NodeKind::Run {
            spec: "cmd:".to_string(),
        },
        "stop" => NodeKind::Stop,
        other => return Err(format!("{other} is not a node this build runs")),
    };
    Ok(GraphNode {
        id: format!("n{next}"),
        kind,
        x: 60.0,
        y: 30.0 + (graph.nodes.len() as f32) * 70.0,
    })
}

/// A node that leaves by this port, or nothing.
pub fn port_target(graph: &MacroGraph, from: &str, port: &str) -> Option<String> {
    graph
        .edges
        .iter()
        .find(|edge| edge.from == from && edge.port == port)
        .map(|edge| edge.to.clone())
}

/// Point a port at a node, or at nothing.
pub fn set_port(graph: &mut MacroGraph, from: &str, port: &str, to: Option<&str>) {
    graph
        .edges
        .retain(|edge| !(edge.from == from && edge.port == port));
    if let Some(to) = to {
        graph.edges.push(g13_config::GraphEdge {
            from: from.to_string(),
            port: port.to_string(),
            to: to.to_string(),
        });
    }
    // an edge that points at a node the file no longer has is a graph the player refuses, so they go together
    let known: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
    graph.edges.retain(|edge| known.contains(&edge.to));
}

/// Take a node out, and every line that reached it.
pub fn remove_node(graph: &mut MacroGraph, id: &str) {
    graph.nodes.retain(|node| node.id != id);
    graph.edges.retain(|edge| edge.from != id && edge.to != id);
    if graph.start == id {
        graph.start = graph
            .nodes
            .first()
            .map(|node| node.id.clone())
            .unwrap_or_default();
    }
}

/// The value of a field as the editor holds it, for the fields that are numbers.
pub fn node_number(kind: &NodeKind, field: &str) -> Option<f64> {
    Some(match (kind, field) {
        (NodeKind::Key { code, .. }, "code") => *code as f64,
        (NodeKind::Key { hold, .. }, "hold") => *hold as f64,
        (NodeKind::Wait { ms }, "ms") => *ms as f64,
        (NodeKind::Type { per_key, .. }, "per_key") => *per_key as f64,
        (NodeKind::Repeat { times }, "times") => *times as f64,
        (NodeKind::Call { macro_id }, "macro") => *macro_id as f64,
        _ => return None,
    })
}

/// Set a numeric field, clamped to something the format accepts.
pub fn set_node_number(kind: &mut NodeKind, field: &str, value: f64) {
    let rounded = value.round();
    match (kind, field) {
        (NodeKind::Key { code, .. }, "code") => *code = rounded.clamp(0.0, 65_535.0) as u16,
        (NodeKind::Key { hold, .. }, "hold") => *hold = rounded.clamp(0.0, 60_000.0) as u32,
        (NodeKind::Wait { ms }, "ms") => *ms = rounded.clamp(0.0, 600_000.0) as u32,
        (NodeKind::Type { per_key, .. }, "per_key") => {
            *per_key = rounded.clamp(0.0, 10_000.0) as u32
        }
        (NodeKind::Repeat { times }, "times") => *times = rounded.clamp(1.0, 10_000.0) as u32,
        (NodeKind::Call { macro_id }, "macro") => {
            *macro_id = rounded.clamp(0.0, u32::MAX as f64) as u32
        }
        _ => {}
    }
}

/// The published values, for the condition editor's list of names.
pub fn value_names() -> Vec<String> {
    let mut names: Vec<String> = g13_sources::PUBLISHED
        .iter()
        .map(|value| value.name.to_string())
        .collect();
    names.sort();
    names
}

/// What a playback did, for the line under the Play button.
#[derive(Debug, Clone, PartialEq)]
pub struct Played {
    /// The node ids the walk went through, in the order it went.
    pub walked: Vec<String>,
    /// What stopped it, when something did.
    pub problem: Option<String>,
}

/// Play a macro now, graph or steps.
///
/// Runs on its own thread in the window: a macro can take seconds, and a window that stops drawing while it
/// plays is a window that looks broken.
pub fn play_in_background(
    config_dir: &std::path::Path,
    id: u32,
    control: Option<String>,
    done: std::sync::Arc<std::sync::Mutex<Option<Played>>>,
) {
    let config_dir = config_dir.to_path_buf();
    std::thread::spawn(move || {
        let outcome = play(&config_dir, id, control);
        if let Ok(mut slot) = done.lock() {
            *slot = Some(outcome);
        }
    });
}

/// Play one macro for real - a virtual keyboard, this directory's world - and say what was walked.
fn play(config_dir: &std::path::Path, id: u32, control: Option<String>) -> Played {
    let opening = match open(config_dir, id) {
        Ok(opening) => opening,
        Err(problem) => {
            return Played {
                walked: Vec::new(),
                problem: Some(problem),
            };
        }
    };
    let keyboard = match g13_device::keyboard::VirtualKeyboard::new() {
        Ok(keyboard) => std::sync::Arc::new(std::sync::Mutex::new(keyboard)),
        Err(error) => {
            return Played {
                walked: Vec::new(),
                problem: Some(format!("cannot make a keyboard to type with: {error}")),
            };
        }
    };
    let playback = g13_agent::Playback {
        // the same world the driver gives a macro: this directory's endpoints, and the published values
        world: g13_sources::World::default()
            .with_endpoints(&config_dir.join("endpoints.json"))
            .with_values(&g13_agent::visuals::values_path()),
        control,
        profile: g13_config::read_active_profile(),
        values: g13_agent::visuals::read_published_values(),
    };
    let mut walked: Vec<String> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let problem = match &opening.graph {
        Some(graph) => {
            let load = |id: u32| g13_config::read_playable(id).ok().flatten();
            // asked for as they are needed, and what would not read is said here rather than only in a log
            g13_agent::play_graph_reporting(
                &keyboard,
                graph,
                &playback,
                &load,
                &mut |node| walked.push(node.to_string()),
                &mut problems,
            )
            .err()
        }
        None => g13_agent::play_macro(&keyboard, &opening.as_macro(), 1).err(),
    };
    Played {
        walked,
        problem: problem.or_else(|| (!problems.is_empty()).then(|| problems.join("; "))),
    }
}

/// The names of the macros that can be called, for the `call` node's list.
pub fn callable(config_dir: &std::path::Path, exclude: u32) -> BTreeMap<u32, String> {
    list(config_dir)
        .into_iter()
        .filter(|row| row.id != exclude)
        .map(|row| {
            let name = if row.name.is_empty() {
                "(no name)".to_string()
            } else {
                row.name
            };
            (row.id, name)
        })
        .collect()
}

/// Add a source the macro's questions can read.
///
/// The name is what a question says, so an empty one is refused: a source nothing can name is a source that
/// cannot be asked about.
pub fn add_source(graph: &mut MacroGraph, name: &str, spec: &str) -> Result<(), String> {
    let name = name.trim();
    let spec = spec.trim();
    if name.is_empty() {
        return Err("a source needs a name: a question asks about it by name".to_string());
    }
    if spec.is_empty() {
        return Err(format!("{name} needs something to read, e.g. cmd:sensors"));
    }
    if graph.sources.contains_key(name) {
        return Err(format!("{name} is already one of this macro's sources"));
    }
    graph.sources.insert(name.to_string(), spec.to_string());
    Ok(())
}

/// Change a source, renaming it if the name changed.
///
/// A question that still names the old name is left alone: rewriting somebody's question because they renamed
/// a source would be the editor deciding what the macro means. The player says the name is unknown instead.
pub fn set_source(graph: &mut MacroGraph, was: &str, name: &str, spec: &str) -> Result<(), String> {
    let name = name.trim();
    let spec = spec.trim();
    if name.is_empty() {
        return Err("a source needs a name: a question asks about it by name".to_string());
    }
    if spec.is_empty() {
        return Err(format!("{name} needs something to read, e.g. cmd:sensors"));
    }
    if name != was && graph.sources.contains_key(name) {
        return Err(format!("{name} is already one of this macro's sources"));
    }
    graph.sources.remove(was);
    graph.sources.insert(name.to_string(), spec.to_string());
    Ok(())
}

/// Take a source away. Questions that named it are left as they are, and the player says the name is unknown.
pub fn remove_source(graph: &mut MacroGraph, name: &str) {
    graph.sources.remove(name);
}

/// The names a question may ask about, and which of them the thing asking provides itself.
///
/// A macro hands over its graph's sources; an applet hands over the sources it declares. One editor can then be
/// given either, which is why an alert is edited with the same rows a macro's `if` is.
#[derive(Debug, Clone, Default)]
pub struct Names {
    /// The first name on offer, so a new question can start on something real rather than a blank.
    pub first: String,
    /// Every name on offer, the ones it provides itself first.
    pub all: Vec<String>,
    /// The names the thing asking declares itself, so a picker can say which are its own.
    pub mine: std::collections::BTreeSet<String>,
}

impl Names {
    /// For a macro: its own sources first, then everything published.
    pub fn for_macro(graph: &MacroGraph) -> Self {
        let all = readable_names(Some(graph));
        let first = all.first().cloned().unwrap_or_default();
        Names {
            first,
            all,
            mine: graph.sources.keys().cloned().collect(),
        }
    }

    /// For an applet: what it declares first, then everything published.
    pub fn for_applet(sources: &std::collections::BTreeMap<String, String>) -> Self {
        // the applet's own names come first, which is what the line above has always claimed and what a question
        // about this applet is most likely to be about
        let mut all: Vec<String> = sources.keys().cloned().collect();
        for name in readable_names(None) {
            if !all.contains(&name) {
                all.push(name);
            }
        }
        let first = all.first().cloned().unwrap_or_default();
        Names {
            first,
            all,
            mine: sources.keys().cloned().collect(),
        }
    }
}

/// Everything a macro's questions can read: its own sources and the published values.
///
/// A source shadows a published value of the same name, because declaring it on the macro is the more specific
/// thing to say - and the list shows the sources first, so what wins is what is at the top.
pub fn readable_names(graph: Option<&MacroGraph>) -> Vec<String> {
    let mut names: Vec<String> = graph
        .map(|graph| graph.sources.keys().cloned().collect())
        .unwrap_or_default();
    for name in value_names() {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

impl Opening {
    /// The macro as a player wants it.
    pub fn as_macro(&self) -> g13_config::Macro {
        g13_config::Macro {
            name: self.name.clone(),
            id: self.id,
            steps: self.steps.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_dir(name: &str) -> std::path::PathBuf {
        // a directory of its own: two tests sharing one path delete each other's files
        let dir = std::env::temp_dir().join(format!("g13-macros-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_macro_is_written_and_read_back_where_it_lives() {
        let dir = a_dir("save");
        let steps = vec![
            MacroStep::KeyDown(29),
            MacroStep::Delay(20),
            MacroStep::KeyUp(29),
        ];
        save_steps(&dir, 5, "ALT-TAB of my own", &steps).expect("saved");
        let opened = open(&dir, 5).expect("opened");
        assert_eq!(opened.name, "ALT-TAB of my own");
        assert_eq!(opened.steps, steps);
        assert!(opened.graph.is_none());

        // and the list says what it is
        let rows = list(&dir);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 5);
        assert_eq!(rows[0].steps, 3);
        assert_eq!(rows[0].nodes, None);
        assert_eq!(rows[0].problem, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn making_a_macro_decide_things_keeps_the_steps_it_had() {
        let dir = a_dir("chain");
        let steps = vec![
            MacroStep::KeyDown(29),
            MacroStep::Delay(20),
            MacroStep::KeyUp(29),
            MacroStep::Delay(100),
        ];
        save_steps(&dir, 3, "chained", &steps).expect("saved");
        let graph = steps_as_chain(3, &steps, "chained");
        save_graph(&dir, 3, &graph).expect("saved the graph");

        // the press and its pause are one box, and the wait after it is another, joined in that order - so
        // the macro does exactly what it did before it had a graph
        assert_eq!(graph.nodes.len(), 2, "{:?}", graph.nodes);
        assert_eq!(graph.start, "n1");
        assert_eq!(port_target(&graph, "n1", "out").as_deref(), Some("n2"));
        assert_eq!(port_target(&graph, "n2", "out"), None);
        assert_eq!(
            graph.node("n1").map(|node| node.kind.clone()),
            Some(NodeKind::Key {
                code: 29,
                hold: 20,
                mode: KeyMode::Tap
            })
        );
        assert_eq!(
            graph.node("n2").map(|node| node.kind.clone()),
            Some(NodeKind::Wait { ms: 100 })
        );

        // and the files now say it decides things, without the steps having been rewritten
        let rows = list(&dir);
        assert_eq!(rows[0].nodes, Some(2));
        assert_eq!(rows[0].steps, 4);
        assert_eq!(open(&dir, 3).expect("opened").steps, steps);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recorded_press_is_one_box_and_a_chord_stays_a_chord() {
        // "Hold ctrl down / Wait 20ms / Release ctrl" is the file's spelling of one press; the flow should say
        // the press, because three boxes for one press is not a flow anybody can read
        let single = vec![
            MacroStep::KeyDown(29),
            MacroStep::Delay(20),
            MacroStep::KeyUp(29),
        ];
        let graph = steps_as_chain(1, &single, "one press");
        assert_eq!(graph.nodes.len(), 1, "{:?}", graph.nodes);
        assert_eq!(node_words(&graph.nodes[0].kind), "Press leftctrl for 20ms");

        // a chord is holds and releases, which is what a chord is
        let chord = vec![
            MacroStep::KeyDown(29),
            MacroStep::KeyDown(56),
            MacroStep::KeyDown(111),
            MacroStep::Delay(20),
            MacroStep::KeyUp(111),
            MacroStep::KeyUp(56),
            MacroStep::KeyUp(29),
            MacroStep::Delay(100),
        ];
        let graph = steps_as_chain(0, &chord, "CTRL-ALT-DEL");
        let said: Vec<String> = graph
            .nodes
            .iter()
            .map(|node| node_words(&node.kind))
            .collect();
        assert_eq!(
            said,
            vec![
                "Hold leftctrl down",
                "Hold leftalt down",
                "Press delete for 20ms",
                "Release leftalt",
                "Release leftctrl",
                "Wait 100ms",
            ],
            "the chord does not read as a chord"
        );
        // and every box is joined to the next one, with nothing left unjoined
        assert_eq!(graph.edges.len(), graph.nodes.len() - 1);

        // typing keeps every press, because the rhythm is the macro
        let typed = vec![
            MacroStep::KeyDown(20),
            MacroStep::Delay(111),
            MacroStep::KeyDown(18),
            MacroStep::Delay(25),
            MacroStep::KeyUp(20),
            MacroStep::Delay(85),
            MacroStep::KeyUp(18),
        ];
        let graph = steps_as_chain(2, &typed, "test");
        assert_eq!(graph.nodes.len(), typed.len(), "a press was folded away");
    }

    #[test]
    fn a_macro_can_declare_what_it_reads_and_an_output_to_call() {
        let mut graph = MacroGraph {
            start: "ask".to_string(),
            nodes: vec![crate::macros::new_node_in("ask", "if")],
            edges: Vec::new(),
            sources: BTreeMap::new(),
        };
        // the name is what a question says, so an empty one is refused rather than saved
        let problem = add_source(&mut graph, "  ", "cmd:sensors").unwrap_err();
        assert!(problem.contains("needs a name"), "{problem}");
        // and a source with nothing to read would be a question that is always false
        let problem = add_source(&mut graph, "heat", "   ").unwrap_err();
        assert!(problem.contains("needs something to read"), "{problem}");
        add_source(&mut graph, "heat", "cmd:sensors").expect("a source");
        let problem = add_source(&mut graph, "heat", "file:/tmp/x").unwrap_err();
        assert!(problem.contains("already"), "{problem}");

        // renaming keeps the spec, and a question that still names the old name is left alone
        set_source(&mut graph, "heat", "cpu_temp", "cmd:sensors").expect("renamed");
        assert!(!graph.sources.contains_key("heat"));
        assert_eq!(
            graph.sources.get("cpu_temp").map(String::as_str),
            Some("cmd:sensors")
        );
        remove_source(&mut graph, "cpu_temp");
        assert!(graph.sources.is_empty());

        // a run box is an output, and it says what it does
        let run = crate::macros::new_node_in("do", "run");
        assert_eq!(node_words(&run.kind), "Run ");
        let written = g13_config::parse_graph(&g13_config::graph_to_json(&MacroGraph {
            start: "do".to_string(),
            nodes: vec![g13_config::GraphNode {
                id: "do".to_string(),
                kind: g13_config::NodeKind::Run {
                    spec: "cmd:notify-send done".to_string(),
                },
                x: 0.0,
                y: 0.0,
            }],
            edges: Vec::new(),
            sources: BTreeMap::new(),
        }))
        .expect("a graph with a run box");
        // the scheme is dropped when it is the ordinary one, because that is what a person wrote
        assert_eq!(
            node_words(&written.node("do").expect("do").kind),
            "Run notify-send done"
        );
    }

    #[test]
    fn the_names_a_question_can_ask_about_are_the_macros_own_ones_first() {
        let mut graph = MacroGraph {
            start: "ask".to_string(),
            nodes: vec![crate::macros::new_node_in("ask", "if")],
            edges: Vec::new(),
            sources: BTreeMap::new(),
        };
        add_source(&mut graph, "cpu_temp", "cmd:sensors").expect("a source");
        let names = readable_names(Some(&graph));
        assert_eq!(names.first().map(String::as_str), Some("cpu_temp"));
        // and the published values are there too, because a question can ask about those
        assert!(names.iter().any(|name| name == "cpu"), "{names:?}");
        // a source shadows a published value of the same name, and is listed once
        add_source(&mut graph, "cpu", "cmd:sensors").expect("a source that shadows");
        let names = readable_names(Some(&graph));
        assert_eq!(
            names.iter().filter(|name| *name == "cpu").count(),
            1,
            "{names:?}"
        );
        assert!(
            names.first().map(String::as_str) == Some("cpu"),
            "{names:?}"
        );
    }

    #[test]
    fn a_port_points_at_one_node_or_at_nothing() {
        let mut graph = MacroGraph {
            start: "a".to_string(),
            nodes: ["a", "b", "c"]
                .iter()
                .map(|id| GraphNode {
                    id: (*id).to_string(),
                    kind: NodeKind::Wait { ms: 1 },
                    x: 0.0,
                    y: 0.0,
                })
                .collect(),
            edges: Vec::new(),
            sources: BTreeMap::new(),
        };
        set_port(&mut graph, "a", "out", Some("b"));
        assert_eq!(port_target(&graph, "a", "out").as_deref(), Some("b"));
        // pointing it somewhere else replaces the line rather than adding a second one
        set_port(&mut graph, "a", "out", Some("c"));
        assert_eq!(port_target(&graph, "a", "out").as_deref(), Some("c"));
        assert_eq!(graph.edges.len(), 1);
        // and (nothing) takes it away
        set_port(&mut graph, "a", "out", None);
        assert_eq!(port_target(&graph, "a", "out"), None);
        assert!(graph.edges.is_empty());

        // taking a node away takes the lines that reached it, because the player refuses a graph that
        // points at a node that is not there
        set_port(&mut graph, "a", "out", Some("b"));
        set_port(&mut graph, "b", "out", Some("c"));
        remove_node(&mut graph, "b");
        assert_eq!(graph.nodes.len(), 2);
        assert!(
            graph.edges.is_empty(),
            "a line survived the node it reached"
        );
        assert_eq!(graph.start, "a");

        // and the start moves if the node it named is the one that went
        let mut second = MacroGraph {
            start: "b".to_string(),
            nodes: graph.nodes.clone(),
            edges: Vec::new(),
            sources: BTreeMap::new(),
        };
        remove_node(&mut second, "b");
        assert_eq!(second.start, "a");
    }

    #[test]
    fn a_graph_that_would_not_run_is_not_written() {
        let dir = a_dir("refuse");
        // two nodes with the same id: the player refuses it, so the editor must refuse to write it
        let broken = MacroGraph {
            sources: BTreeMap::new(),
            start: "n1".to_string(),
            nodes: vec![
                GraphNode {
                    id: "n1".to_string(),
                    kind: NodeKind::Stop,
                    x: 0.0,
                    y: 0.0,
                },
                GraphNode {
                    id: "n1".to_string(),
                    kind: NodeKind::Stop,
                    x: 0.0,
                    y: 0.0,
                },
            ],
            edges: Vec::new(),
        };
        let problem = save_graph(&dir, 1, &broken).expect_err("a graph the player refuses");
        assert!(
            problem.contains("two nodes are both called n1"),
            "{problem}"
        );
        assert!(
            !dir.join("macro-1.nodes.json").exists(),
            "a file that would not run was written"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_condition_reads_into_a_form_and_back_again() {
        // the simple questions
        let control = Cond::Control("G7".to_string());
        let form = ConditionForm::from_cond(&control).expect("a form");
        assert_eq!(form.rows.len(), 1);
        assert_eq!(form.to_cond(), control);
        assert_eq!(form.describe(), "fired by G7");

        // a not, on one question
        let not = Cond::Not(Box::new(Cond::Profile(2)));
        let form = ConditionForm::from_cond(&not).expect("a form");
        assert!(form.rows[0].not);
        assert_eq!(form.to_cond(), not);

        // all of two, one of them a value
        let both = Cond::All(vec![
            Cond::Profile(2),
            Cond::Value {
                name: "cpu".to_string(),
                op: Compare::Gt,
                number: Some(50.0),
                text: None,
            },
        ]);
        let form = ConditionForm::from_cond(&both).expect("a form");
        assert!(form.all);
        assert_eq!(form.rows.len(), 2);
        assert_eq!(form.to_cond(), both);
        assert_eq!(form.describe(), "profile 2 is active and cpu > 50");

        // a flat any of two simple questions has a form
        let flat_any = Cond::Any(vec![Cond::Profile(1), Cond::Control("G7".to_string())]);
        let form = ConditionForm::from_cond(&flat_any).expect("a form");
        assert!(!form.all);
        assert_eq!(form.to_cond(), flat_any);

        // **a tree this editor cannot draw gives no form at all.** Handing one back would mean saving it as
        // whatever the form could express, which rewrote `All([Profile(1)])` inside an `Any` into "fired by
        // nothing" - a condition that is always false, produced silently by opening a file.
        let nested = Cond::Any(vec![Cond::All(vec![Cond::Profile(1)]), Cond::Profile(2)]);
        assert!(
            ConditionForm::from_cond(&nested).is_none(),
            "a nested tree was given a form"
        );
        let deeply = Cond::Not(Box::new(Cond::All(vec![Cond::Profile(1)])));
        assert!(ConditionForm::from_cond(&deeply).is_none());
    }

    #[test]
    fn a_drop_lands_on_the_box_the_pointer_is_over() {
        // The canvas is drawn from this and the drop is decided by this, so a drag that joins the wrong box
        // would be a bug the picture cannot show.
        let graph = MacroGraph {
            sources: BTreeMap::new(),
            start: "n1".to_string(),
            nodes: vec![
                GraphNode {
                    id: "n1".to_string(),
                    kind: NodeKind::Wait { ms: 1 },
                    x: 0.0,
                    y: 0.0,
                },
                GraphNode {
                    id: "n2".to_string(),
                    kind: NodeKind::Wait { ms: 1 },
                    x: 0.0,
                    y: 80.0,
                },
            ],
            edges: Vec::new(),
        };
        // inside the first box
        let first = egui::pos2(CANVAS_MARGIN + 10.0, CANVAS_MARGIN + 10.0);
        assert_eq!(node_at(&graph, first).as_deref(), Some("n1"));
        // inside the second
        let second = egui::pos2(CANVAS_MARGIN + 10.0, CANVAS_MARGIN + 90.0);
        assert_eq!(node_at(&graph, second).as_deref(), Some("n2"));
        // between them, and outside both: nothing, which is how a drag is taken back
        let between = egui::pos2(CANVAS_MARGIN + 10.0, CANVAS_MARGIN + 60.0);
        assert_eq!(node_at(&graph, between), None);
        assert_eq!(node_at(&graph, egui::pos2(2000.0, 2000.0)), None);

        // the dots are where the drawing puts them: a port's on the right edge, the way in on the left
        let node = graph.node("n1").expect("n1");
        assert_eq!(
            port_dot(&graph, node, "out").x,
            CANVAS_MARGIN + CANVAS_BOX.x
        );
        assert_eq!(input_dot(node).x, CANVAS_MARGIN);

        // and the canvas is big enough for whatever the boxes are at
        let small = canvas_size(&graph, egui::vec2(200.0, 100.0));
        let tall = canvas_size(&graph, egui::vec2(0.0, 0.0));
        assert!(small.x >= 200.0 && small.y >= 100.0);
        assert!(
            tall.y >= CANVAS_MARGIN + 80.0 + CANVAS_BOX.y,
            "a box below the first is outside the canvas: {tall:?}"
        );
    }

    #[test]
    fn dragging_a_box_moves_it_and_remembers_it() {
        let dir = a_dir("drag");
        let graph = MacroGraph {
            sources: BTreeMap::new(),
            start: "n1".to_string(),
            nodes: vec![GraphNode {
                id: "n1".to_string(),
                kind: NodeKind::Wait { ms: 1 },
                x: 10.0,
                y: 10.0,
            }],
            edges: Vec::new(),
        };
        save_graph(&dir, 9, &graph).expect("saved");
        let before = open(&dir, 9).expect("opened");
        assert_eq!(before.graph.expect("a graph").nodes[0].x, 10.0);

        let mut moved = graph.clone();
        moved.nodes[0].x = 70.0;
        moved.nodes[0].y = 45.0;
        save_graph(&dir, 9, &moved).expect("saved the moved one");
        let after = open(&dir, 9).expect("reopened").graph.expect("a graph");
        assert_eq!((after.nodes[0].x, after.nodes[0].y), (70.0, 45.0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_node_gets_an_id_nothing_else_has() {
        let mut graph = MacroGraph {
            sources: BTreeMap::new(),
            start: String::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
        };
        let first = new_node(&graph, "wait").expect("a node");
        assert_eq!(first.id, "n1");
        graph.nodes.push(first);
        let second = new_node(&graph, "if").expect("a node");
        assert_eq!(second.id, "n2");
        assert!(matches!(
            second.kind,
            NodeKind::If {
                when: Cond::Control(_)
            }
        ));
        // and a kind this build does not run is refused rather than guessed at
        assert!(new_node(&graph, "flash").is_err());
    }
}

#[cfg(test)]
mod a_question_starts_on_something_real {
    use super::*;

    /// The fault: an added question was always a "fired by", so a second one had no value and nothing to compare
    /// against, while the first had both. Both now start the same way.
    #[test]
    fn a_new_question_knows_what_to_ask_about() {
        let mut sources = std::collections::BTreeMap::new();
        sources.insert("cpu".to_string(), "cpu".to_string());
        sources.insert("memory".to_string(), "memory".to_string());
        let names = Names::for_applet(&sources);

        // the applet's own names come first, so a question about this applet starts on one of them
        assert_eq!(
            names.first, "cpu",
            "a new question would start on nothing: {:?}",
            names.all
        );
        assert!(
            names
                .all
                .starts_with(&["cpu".to_string(), "memory".to_string()])
        );
    }

    /// And with nothing declared it still offers everything published, rather than a blank.
    #[test]
    fn with_nothing_declared_it_still_offers_the_published_names() {
        let empty = std::collections::BTreeMap::new();
        let names = Names::for_applet(&empty);
        assert!(!names.first.is_empty(), "nothing to start a question on");
        assert!(names.all.contains(&names.first));
    }

    /// A value question carries what it asks, how it compares and what against - the three things the second
    /// question was missing.
    #[test]
    fn a_value_question_carries_all_three_parts() {
        let form = ConditionForm {
            all: true,
            rows: vec![QuestionRow {
                not: false,
                question: Question::Value {
                    name: "cpu".to_string(),
                    op: g13_values::Compare::Gt,
                    number: Some(80.0),
                    text: None,
                },
            }],
        };
        let described = form.describe();
        assert!(
            described.contains("cpu"),
            "the value is not named: {described}"
        );
        assert!(described.contains('>'), "no comparison: {described}");
        assert!(
            described.contains("80"),
            "nothing to compare against: {described}"
        );
    }
}
