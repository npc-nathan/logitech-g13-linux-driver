//! The configuration window.
//!
//! Everything the window does is in `state`, which is testable without a display; this file only draws it.
//! Edits write the configuration as they are made  -  there is no Apply button  -  and a running driver picks
//! them up within a second. Nothing the window cannot do is hidden: it says so where it would be.
//!
//! The drawing uses whatever the egui in the workspace provides. In 0.36 that is `App::ui` (given a `Ui`, not
//! a `Context`) and `egui::Panel::top|bottom`, which also take a `Ui`; the panels are not free-standing
//! contexts, and assuming otherwise does not compile.

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
pub mod bundle;
pub mod live;
pub mod macros;
pub mod state;

use state::{FieldValue, MenuDoes, Origin, Tab, WIDGET_KINDS, WidgetRow, Window};

/// The window, as the windowing system sees it.
pub struct App {
    /// The editor's state, which the windowing system calls each frame to draw and act on.
    window: Window,
}

impl App {
    /// Wrap a window's state so the windowing system can run it.
    pub fn new(window: Window) -> Self {
        Self { window }
    }

    /// Open the window and run it until it is closed. Returns whatever the windowing system said.
    pub fn open(window: Window) -> eframe::Result {
        eframe::run_native(
            "g13",
            eframe::NativeOptions {
                // Two columns need room to be two columns. Below this the window can still be dragged smaller,
                // and then the cells scroll rather than overlapping - but it does not open that way.
                viewport: egui::ViewportBuilder::default()
                    .with_inner_size([1100.0, 720.0])
                    .with_min_inner_size([520.0, 360.0]),
                ..Default::default()
            },
            Box::new(|_creation| Ok(Box::new(App::new(window)))),
        )
    }
}

impl App {
    /// Everything the window draws, given somewhere to draw it.
    ///
    /// Extracted so a test can draw the *whole* window: a tab that is compiled but never reached from the tab
    /// bar is a tab that does not exist, and this window has had exactly that bug before.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        // While a capture is running the window listens at frame rate. A press noticed a second later reads as
        // the button not having worked, which is worse than not offering the button.
        let interval = match self.window.capture.is_some() {
            true => std::time::Duration::from_millis(16),
            false => state::repaint_interval_for(&self.window),
        };
        ui.ctx().request_repaint_after(interval);
        // and a press is taken before anything is drawn, so the table shows it in this frame
        self.window.poll_capture();
        // the live reading every repaint: it is a file read, under a millisecond.
        self.window.refresh_live();
        // Taking a finished frame is a lock and a swap, so it happens every repaint; the drawing itself is
        // on the worker thread, which means this call never waits for a source however slow it is.
        self.window.refresh_preview();

        egui::Panel::top("tabs").show(ui, |ui| {
            // wrapped, so a narrow window puts the profile and the driver on the next line instead of drawing
            // them over the last tab
            ui.horizontal_wrapped(|ui| {
                // Tab order: the map first, then what the pad does with it, then the things that are drawn on it,
                // then what they are built from.
                for tab in [
                    Tab::Bindings,
                    Tab::Macros,
                    Tab::Menu,
                    Tab::Applets,
                    Tab::Values,
                    Tab::Sources,
                    Tab::Themes,
                ] {
                    let selected = self.window.tab == tab;
                    if ui.selectable_label(selected, tab.title()).clicked() {
                        self.window.tab = tab;
                    }
                }
                ui.separator();
                ui.label(format!(
                    "profile {} of {:?}",
                    self.window.profile, self.window.profiles
                ));
                if ui.button("reload").clicked() {
                    self.window.reload();
                }
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            // the driver first: this window only edits configuration, and if nothing is drawing the pad then
            // nothing it does can be seen there. Saying so is the difference between a bug and a mystery.
            ui.horizontal(|ui| {
                let (colour, sentence) = match &self.window.driver {
                    state::Driver::Running(_) => {
                        (egui::Color32::LIGHT_GREEN, self.window.driver.sentence())
                    }
                    state::Driver::Starting => {
                        (egui::Color32::YELLOW, self.window.driver.sentence())
                    }
                    state::Driver::Stopped => {
                        (egui::Color32::LIGHT_RED, self.window.driver.sentence())
                    }
                };
                ui.colored_label(colour, sentence);
                ui.separator();
                if ui.button("start driver").clicked() {
                    self.window.status = match state::start_driver() {
                        Ok(what) => what,
                        Err(problem) => problem,
                    };
                }
                if ui.button("stop driver").clicked() {
                    self.window.status = match state::stop_driver() {
                        Ok(what) => what,
                        Err(problem) => problem,
                    };
                }
            });
            if self.window.problems.is_empty() {
                ui.label("nothing else to report");
            } else {
                for problem in &self.window.problems {
                    ui.colored_label(egui::Color32::YELLOW, problem);
                }
            }
            if !self.window.status.is_empty() {
                ui.label(&self.window.status);
            }
        });

        egui::CentralPanel::default().show(ui, |ui| match self.window.tab {
            Tab::Bindings => bindings(ui, &mut self.window),
            Tab::Menu => menu_tab(ui, &mut self.window),
            Tab::Sources => sources(ui, &mut self.window),
            Tab::Applets => applets(ui, &mut self.window),
            Tab::Themes => themes(ui, &mut self.window),
            Tab::Values => values(ui, &mut self.window),
            Tab::Macros => macros(ui, &mut self.window),
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

/// The macros: what there is, what it does, and editing it.
///
/// The graph is drawn as a picture you can drag rather than as rows of fields: a macro that decides something
/// is a shape, and the shape is the thing being edited. The fields for the box you have clicked are beside it.
fn macros(ui: &mut egui::Ui, window: &mut Window) {
    ui.strong("macros");
    ui.label(
        "one card each. a macro is a file in the configuration folder; this edits it where it is.",
    );
    ui.add_space(4.0);
    // Cards rather than a row of chips: the previous stack left a slot for every id it could use, and two hundred
    // chips in a line bury the handful that do something.
    //
    // **One label per card, carrying the id and the name together** - `200 mine [4 nodes]`. The window's render
    // tests search the drawn text for exactly that string, which is how a first version of this that split the id
    // and the name into two labels was caught: the text the tests look for stopped existing.
    egui::ScrollArea::vertical()
        .max_height(170.0)
        .id_salt("macro-cards")
        .show(ui, |ui| {
            let empty = window.empty_macro_count();
            for row in window.macro_rows.clone() {
                if row.empty && !window.show_empty_macros {
                    continue;
                }
                let chosen = window.macro_editing == Some(row.id);
                let mut what = format!("{} ", row.id);
                if row.name.is_empty() {
                    if row.empty {
                        what.push_str("(empty)");
                    } else {
                        what.push_str("(no name)");
                    }
                } else {
                    what.push_str(&row.name);
                }
                if let Some(nodes) = row.nodes {
                    what.push_str(&format!(" [{nodes} nodes]"));
                }
                let frame = egui::Frame::group(ui.style()).fill(if chosen {
                    ui.visuals().selection.bg_fill.gamma_multiply(0.30)
                } else {
                    egui::Color32::TRANSPARENT
                });
                let card = frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.selectable_label(chosen, what)
                });
                // the whole card is the button, not just the words in it - and the words stay one label, because
                // that is what is on the screen and what a test can look for
                let clicked = ui.interact(
                    card.response.rect,
                    ui.make_persistent_id(("macro-card", row.id)),
                    egui::Sense::click(),
                );
                if card.inner.clicked() || clicked.clicked() {
                    window.open_macro(row.id);
                }
                ui.add_space(2.0);
            }
            if empty > 0 {
                let label = format!("show the {empty} empty one(s)");
                let mut shown = window.show_empty_macros;
                if ui.checkbox(&mut shown, label).changed() {
                    window.show_empty_macros = shown;
                }
            }
        });
    if window.macro_editing.is_some() && ui.button("stop editing").clicked() {
        window.close_macro();
    }
    ui.horizontal(|ui| {
        ui.label("new:");
        let mut name = window.new_macro_name.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut name)
                    .desired_width(160.0)
                    .hint_text("what it is for"),
            )
            .changed()
        {
            window.new_macro_name = name;
        }
        if ui.button("make it").clicked() {
            window.new_macro();
        }
        ui.label(" -  written straight away, so it can be recorded into or built by hand");
    });
    ui.separator();

    let Some(id) = window.macro_editing else {
        ui.label("Pick one above to edit it, or make a new one. A macro is a file in the configuration folder;");
        ui.label("this edits it where it is. The right-hand side is the macro as a picture: drag the boxes,");
        ui.label("drag from a dot to join one to another, and click a box to edit what it does.");
        return;
    };

    if let Some(problem) = window.macro_problem.clone() {
        ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), problem);
        ui.add_space(6.0);
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.columns(2, |panes| {
            // the left pane: the steps, which is what plays and what the previous stack reads
            {
                let ui = &mut panes[0];
                ui.horizontal(|ui| {
                    ui.label(format!("macro-{id}.properties"));
                    ui.label("name:");
                    let mut name = window.macro_name.clone();
                    if ui
                        .add(egui::TextEdit::singleline(&mut name).desired_width(200.0))
                        .changed()
                    {
                        window.set_macro_name(&name);
                    }
                });
                ui.add_space(4.0);
                // which of the two panes is the macro depends on whether it has a graph, and saying which is
                // the difference between two views of one thing and two things
                if window.macro_graph.is_some() {
                    ui.label(
                        "Steps. The flat form of the same macro: it is what anything older reads, and the \
                         graph on the right is what plays.",
                    );
                } else {
                    ui.label("Steps. This macro is these steps in this order, so this is what plays:");
                }
                ui.add_space(4.0);
                let steps = window.macro_steps.clone();
                for (index, step) in steps.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let last = steps.len() - 1;
                        if ui
                            .add_enabled(index > 0, egui::Button::new("^"))
                            .on_hover_text("move up")
                            .clicked()
                        {
                            window.move_macro_step(index, -1);
                        }
                        if ui
                            .add_enabled(index < last, egui::Button::new("v"))
                            .on_hover_text("move down")
                            .clicked()
                        {
                            window.move_macro_step(index, 1);
                        }
                        if ui.button("remove").clicked() {
                            window.remove_macro_step(index);
                            return;
                        }
                        ui.label(format!("{}.", index + 1));
                        let was = match step {
                            g13_config::MacroStep::KeyDown(_) => "press",
                            g13_config::MacroStep::KeyUp(_) => "release",
                            g13_config::MacroStep::Delay(_) => "wait",
                        };
                        let mut kind = was;
                        egui::ComboBox::from_id_salt(format!("step-kind-{index}"))
                            .selected_text(kind)
                            .width(90.0)
                            .show_ui(ui, |ui| {
                                for option in ["press", "release", "wait"] {
                                    ui.selectable_value(&mut kind, option, option);
                                }
                            });
                        if kind != was {
                            window.set_macro_step_kind(index, kind);
                        }
                        match step {
                            g13_config::MacroStep::KeyDown(code)
                            | g13_config::MacroStep::KeyUp(code) => {
                                let mut name = crate::macros::key_name(*code);
                                if ui
                                    .add(egui::TextEdit::singleline(&mut name).desired_width(90.0))
                                    .changed()
                                {
                                    if let Some(problem) = window.set_macro_step_key(index, &name) {
                                        window.macro_problem = Some(problem);
                                    }
                                }
                            }
                            g13_config::MacroStep::Delay(ms) => {
                                let mut ms = *ms;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut ms).range(0..=600_000).suffix(" ms"),
                                    )
                                    .changed()
                                {
                                    window.set_macro_step_delay(index, ms);
                                }
                            }
                        }
                    });
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("add:");
                    for kind in ["press", "release", "wait"] {
                        if ui.button(kind).clicked() {
                            window.add_macro_step(kind);
                        }
                    }
                });
            }

            // the right pane: the macro as a picture
            {
                let ui = &mut panes[1];
                match window.macro_graph.clone() {
                    None => {
                        ui.label("This macro is a list of steps, so it always does them in that order.");
                        ui.add_space(4.0);
                        if ui.button("make it decide things").clicked() {
                            window.make_macro_decide();
                        }
                        ui.label("That draws the steps as boxes you can join to each other, so it behaves the");
                        ui.label("same and can then be given an if, a repeat, a type or a call.");
                    }
                    Some(graph) => {
                        let mut deleting = false;
                        ui.horizontal(|ui| {
                            ui.label(format!("macro-{id}.nodes.json"));
                            if ui.button("delete the graph").clicked() {
                                deleting = true;
                            }
                        });
                        if deleting {
                            // the graph is gone, so there is nothing to draw this frame: a `return` inside the
                            // column closure would only leave the closure, which is what it looked like it did
                            window.drop_macro_graph();
                            return;
                        }
                        ui.add_space(4.0);
                        canvas(ui, window, &graph, id);
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label("add:");
                            for kind in
                                ["key", "wait", "type", "if", "repeat", "call", "run", "stop"]
                            {
                                if ui.button(kind).clicked() {
                                    window.add_macro_node(kind);
                                }
                            }
                        });
            ui.add_space(6.0);
                        match window.macro_selected.clone() {
                            Some(selected) if graph.node(&selected).is_some() => {
                                node_fields(ui, window, &selected, &graph);
                            }
                            _ => {
                                ui.label("Click a box to edit what it does.");
                            }
                        }
                        // the macro's own sources, whether or not a box is selected: they belong to the
                        // macro, and a question anywhere can name them
                        macro_sources(ui, window, &graph);
                    }
                }
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("play it now").clicked() {
                window.play_macro();
            }
            ui.label(" -  it types for real, so it goes wherever the keyboard goes");
        });
        if let Some(played) = window.macro_playback() {
            if let Some(problem) = played.problem {
                ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), problem);
            } else if played.walked.is_empty() {
                ui.label("it played");
            } else {
                ui.label(format!("it went: {}", played.walked.join(" -> ")));
            }
        } else if !window.macro_status.is_empty() {
            ui.label(window.macro_status.clone());
        }
    });
}

/// The graph as a picture: boxes you drag, dots you drag lines from, and lines you can take away.
///
/// Drawn by hand rather than as a widget because a node's *place* is part of what is being edited: the file
/// holds x and y, the player ignores them, and this is where they mean something.
fn canvas(ui: &mut egui::Ui, window: &mut Window, graph: &g13_config::MacroGraph, id: u32) {
    // The positions come from the model, which is also what decides where a drop lands. One layout, so the
    // picture and the drop cannot disagree about which box is which.
    let size =
        crate::macros::canvas_size(graph, egui::vec2(ui.available_width().max(360.0), 320.0));
    let (response, painter) = ui.allocate_painter(size, egui::Sense::click_and_drag());
    let origin = response.rect.min;
    let layout: Vec<(String, egui::Rect)> = crate::macros::canvas_layout(graph)
        .into_iter()
        .map(|(id, rect)| (id, rect.translate(origin.to_vec2())))
        .collect();
    let rect_of = |id: &str| {
        layout
            .iter()
            .find(|(other, _)| other == id)
            .map(|(_, rect)| *rect)
    };

    painter.rect_filled(response.rect, 4.0, egui::Color32::from_gray(24));

    let dot = 5.0;
    let at = |id: &str, port: &str| -> Option<egui::Pos2> {
        let node = graph.node(id)?;
        Some(crate::macros::port_dot(graph, node, port) + origin.to_vec2())
    };
    let way_in = |id: &str| -> Option<egui::Pos2> {
        let node = graph.node(id)?;
        Some(crate::macros::input_dot(node) + origin.to_vec2())
    };

    // the lines, under the boxes
    for edge in &graph.edges {
        let (Some(stem), Some(tip)) = (at(&edge.from, &edge.port), way_in(&edge.to)) else {
            continue;
        };
        let bend = ((tip.x - stem.x).abs() * 0.5).max(30.0);
        painter.add(egui::Shape::CubicBezier(
            egui::epaint::CubicBezierShape::from_points_stroke(
                [
                    stem,
                    stem + egui::vec2(bend, 0.0),
                    tip - egui::vec2(bend, 0.0),
                    tip,
                ],
                false,
                egui::Color32::TRANSPARENT,
                egui::Stroke::new(1.6, egui::Color32::from_gray(110)),
            ),
        ));
    }

    // a line being dragged from a port follows the pointer
    if let Some((from, port)) = window.macro_connecting.clone() {
        if let (Some(stem), Some(pointer)) = (
            at(&from, &port),
            ui.input(|input| input.pointer.hover_pos()),
        ) {
            painter.line_segment(
                [stem, pointer],
                egui::Stroke::new(1.6, egui::Color32::from_rgb(0x8f, 0xd4, 0xff)),
            );
        }
    }

    // where the flow starts: a small chip above the first box, so the picture reads from somewhere
    if let Some(first) = rect_of(&graph.start) {
        let chip = egui::Rect::from_min_size(
            egui::pos2(first.left(), first.top() - 30.0),
            egui::vec2(
                crate::macros::TRIGGER_TEXT.chars().count() as f32 * 6.2 + 30.0,
                22.0,
            ),
        );
        painter.rect(
            chip,
            11.0,
            egui::Color32::from_gray(38),
            egui::Stroke::new(1.0, egui::Color32::from_gray(100)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            chip.left_center() + egui::vec2(9.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!("\u{25b6} {}", crate::macros::TRIGGER_TEXT),
            egui::FontId::proportional(12.0),
            egui::Color32::from_gray(200),
        );
        painter.line_segment(
            [
                egui::pos2(chip.center().x, chip.bottom()),
                egui::pos2(first.left() + 14.0, first.top()),
            ],
            egui::Stroke::new(1.6, egui::Color32::from_gray(110)),
        );
    }

    let mut moved: Option<(String, egui::Vec2)> = None;
    let mut dropped: Option<String> = None;
    for node in &graph.nodes {
        let Some(rect) = rect_of(&node.id) else {
            continue;
        };
        let selected = window.macro_selected.as_deref() == Some(node.id.as_str());
        let response = ui.interact(
            rect,
            ui.id().with(("node", &node.id, id)),
            egui::Sense::click_and_drag(),
        );
        painter.rect(
            rect,
            6.0,
            if selected {
                egui::Color32::from_gray(58)
            } else {
                egui::Color32::from_gray(42)
            },
            egui::Stroke::new(
                if selected { 2.0 } else { 1.0 },
                if selected {
                    egui::Color32::from_rgb(0x8f, 0xd4, 0xff)
                } else {
                    egui::Color32::from_gray(90)
                },
            ),
            egui::StrokeKind::Inside,
        );
        // what the box does leads; its own name is small in the corner, because the name is for the dropdowns
        painter.text(
            rect.left_top() + egui::vec2(9.0, 9.0),
            egui::Align2::LEFT_TOP,
            crate::macros::node_words(&node.kind),
            egui::FontId::proportional(13.0),
            egui::Color32::from_gray(235),
        );
        painter.text(
            rect.right_bottom() - egui::vec2(7.0, 7.0),
            egui::Align2::RIGHT_BOTTOM,
            &node.id,
            egui::FontId::monospace(10.0),
            egui::Color32::from_gray(120),
        );

        if response.clicked() {
            window.macro_selected = Some(node.id.clone());
        }
        if response.dragged() {
            moved = Some((node.id.clone(), response.drag_delta()));
        }
        if response.drag_stopped() {
            // the place is remembered when the drag ends, not on every frame of it
            dropped = Some(node.id.clone());
        }

        if let Some(place) = way_in(&node.id) {
            painter.circle_filled(place, dot, egui::Color32::from_gray(120));
        }
        for port in crate::macros::ports_of_kind(&node.kind) {
            let Some(place) = at(&node.id, port) else {
                continue;
            };
            let hit = egui::Rect::from_center_size(place, egui::vec2(dot * 3.0, dot * 3.0));
            let port_response = ui.interact(
                hit,
                ui.id().with(("port", &node.id, *port, id)),
                egui::Sense::drag(),
            );
            let connected = crate::macros::port_target(graph, &node.id, port).is_some();
            painter.circle_filled(
                place,
                dot,
                if connected {
                    egui::Color32::from_rgb(0x8f, 0xd4, 0xff)
                } else {
                    egui::Color32::from_gray(120)
                },
            );
            port_response.clone().on_hover_text(format!(
                "{port}: drag from here onto a box to join them, or click to unjoin"
            ));
            if port_response.clicked() {
                window.set_macro_port(&node.id, port, None);
            }
            if port_response.drag_started() {
                window.macro_connecting = Some((node.id.clone(), (*port).to_string()));
            }
            if port_response.drag_stopped() {
                // the drop decides, and the model decides the drop
                let target = ui
                    .input(|input| input.pointer.interact_pos())
                    .and_then(|pointer| crate::macros::node_at(graph, pointer - origin.to_vec2()))
                    .filter(|target| target != &node.id);
                window.set_macro_port(&node.id, port, target.as_deref());
                window.macro_connecting = None;
            }
        }
    }

    if let Some((node, by)) = moved {
        window.move_macro_node(&node, by, false);
    }
    if let Some(node) = dropped {
        window.move_macro_node(&node, egui::vec2(0.0, 0.0), true);
    }
    if ui.input(|input| input.pointer.any_released()) && window.macro_connecting.is_some() {
        window.macro_connecting = None;
    }

    ui.add_space(4.0);
    ui.label("Read it from the top: \u{25b6} is when the macro runs, and each box is what happens next. The line");
    ui.label("out of a box's right-hand dot goes to the box it is joined to; \"(nothing)\" there means the macro");
    ui.label("finishes down that way. A box with two dots on the right chooses: \"then\" is the way on when an if");
    ui.label("is true (and where a repeat goes once it has done its times), \"else\" is the way on when an if is");
    ui.label("false, and \"body\" is the bit a repeat does again. Drag a box to move it, drag from a dot onto");
    ui.label("another box to join them, click a dot to take a join away, and click a box to edit what it does.");
    ui.label("A question about a value can name anything under \"what this macro reads\" below - those are read");
    ui.label("when the question is asked, so \"is the cpu over 70\" is about now - or a published value. A");
    ui.label("\"Run\" box does something instead of typing: it runs the command you give it.");
}

/// What the macro reads: its own sources, in the same syntax an applet declares them in.
///
/// A macro that asks "is the cpu over 70" has to say where the cpu reading comes from, and this is where it
/// says it. The same spelling as an applet's sources, so there is one way of writing a source in this project.
fn macro_sources(ui: &mut egui::Ui, window: &mut Window, graph: &g13_config::MacroGraph) {
    ui.separator();
    ui.label("what this macro reads:");
    ui.label(
        "A question can name any of these. Written like an applet's source: `cmd:sensors`, \
         `http:<endpoint>/<path>#<field>`, `file:/proc/...`. Endpoints come from the Endpoints tab.",
    );
    ui.add_space(4.0);

    // the rows are the file's own entries, in order, so what the file says is what is shown
    let rows: Vec<(String, String)> = graph
        .sources
        .iter()
        .map(|(name, spec)| (name.clone(), spec.clone()))
        .collect();
    for (name, spec) in rows {
        ui.horizontal(|ui| {
            let mut said = name.clone();
            let mut value = spec.clone();
            let renamed = ui
                .add(egui::TextEdit::singleline(&mut said).desired_width(110.0))
                .changed();
            ui.label("reads");
            let edited = ui
                .add(
                    egui::TextEdit::singleline(&mut value)
                        .desired_width(300.0)
                        .hint_text("cmd:sensors"),
                )
                .changed();
            if renamed || edited {
                window.set_macro_source(&name, &said, &value);
            }
            if ui.button("remove").clicked() {
                window.remove_macro_source(&name);
            }
        });
    }
    if graph.sources.is_empty() {
        ui.label("(nothing: a question can still ask about the published values)");
    }

    ui.horizontal(|ui| {
        ui.label("add:");
        ui.add(
            egui::TextEdit::singleline(&mut window.macro_source_name)
                .desired_width(110.0)
                .hint_text("cpu_temp"),
        );
        ui.label("reads");
        ui.add(
            egui::TextEdit::singleline(&mut window.macro_source_spec)
                .desired_width(300.0)
                .hint_text("cmd:sensors | head -1"),
        );
        if ui.button("add this source").clicked() {
            let name = window.macro_source_name.clone();
            let spec = window.macro_source_spec.clone();
            window.add_macro_source(&name, &spec);
        }
    });
}

/// The fields of the box that has been clicked.
fn node_fields(ui: &mut egui::Ui, window: &mut Window, id: &str, graph: &g13_config::MacroGraph) {
    let Some(node) = graph.node(id) else {
        return;
    };
    let calls: Vec<(u32, String)> = window
        .macro_rows
        .iter()
        .filter(|row| row.id != window.macro_editing.unwrap_or(u32::MAX))
        .map(|row| (row.id, row.name.clone()))
        .collect();
    let ids: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();

    ui.separator();
    ui.horizontal(|ui| {
        ui.monospace(id);
        ui.label(match &node.kind {
            g13_config::NodeKind::Key { .. } => "a key",
            g13_config::NodeKind::Wait { .. } => "a wait",
            g13_config::NodeKind::Type { .. } => "typing",
            g13_config::NodeKind::If { .. } => "a question",
            g13_config::NodeKind::Repeat { .. } => "a loop",
            g13_config::NodeKind::Call { .. } => "another macro",
            g13_config::NodeKind::Run { .. } => "something outside the keyboard",
            g13_config::NodeKind::Stop => "the end",
        });
        if ui.button("remove this box").clicked() {
            window.remove_macro_node(id);
            window.macro_selected = None;
        }
    });

    match &node.kind {
        g13_config::NodeKind::Key { code, hold, mode } => {
            ui.horizontal(|ui| {
                ui.label("key:");
                let mut name = crate::macros::key_name(*code);
                if ui
                    .add(egui::TextEdit::singleline(&mut name).desired_width(90.0))
                    .changed()
                {
                    if let Some(problem) = window.set_macro_node_key(id, &name) {
                        window.macro_problem = Some(problem);
                    }
                }
                ui.label("hold it");
                let mut hold = *hold;
                if ui
                    .add(
                        egui::DragValue::new(&mut hold)
                            .range(0..=60_000)
                            .suffix(" ms"),
                    )
                    .changed()
                {
                    window.set_macro_node_number(id, "hold", hold as f64);
                }
                let original = *mode;
                let mut mode = *mode;
                egui::ComboBox::from_id_salt(format!("mode-{id}"))
                    .selected_text(mode_name(mode))
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for option in [
                            g13_config::KeyMode::Tap,
                            g13_config::KeyMode::Down,
                            g13_config::KeyMode::Up,
                        ] {
                            ui.selectable_value(&mut mode, option, mode_name(option));
                        }
                    });
                if mode != original {
                    window.set_macro_node_mode(id, mode);
                }
            });
        }
        g13_config::NodeKind::Wait { ms } => {
            ui.horizontal(|ui| {
                ui.label("wait");
                let mut ms = *ms;
                if ui
                    .add(
                        egui::DragValue::new(&mut ms)
                            .range(0..=600_000)
                            .suffix(" ms"),
                    )
                    .changed()
                {
                    window.set_macro_node_number(id, "ms", ms as f64);
                }
            });
        }
        g13_config::NodeKind::Type { text, per_key } => {
            ui.horizontal(|ui| {
                ui.label("type:");
                let mut text = text.clone();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut text)
                            .desired_width(200.0)
                            .hint_text("what to type"),
                    )
                    .changed()
                {
                    window.set_macro_node_text(id, Some(&text));
                }
                let mut per_key = *per_key;
                if ui
                    .add(
                        egui::DragValue::new(&mut per_key)
                            .range(0..=10_000)
                            .suffix(" ms apart"),
                    )
                    .changed()
                {
                    window.set_macro_node_number(id, "per_key", per_key as f64);
                }
            });
            ui.label("A character this build cannot type stops the macro and says which one.");
        }
        g13_config::NodeKind::Repeat { times } => {
            ui.horizontal(|ui| {
                ui.label("run the body");
                let mut times = *times;
                if ui
                    .add(
                        egui::DragValue::new(&mut times)
                            .range(1..=10_000)
                            .suffix(" times"),
                    )
                    .changed()
                {
                    window.set_macro_node_number(id, "times", times as f64);
                }
                ui.label(" -  drag from its body dot back to the box that starts the loop");
            });
        }
        g13_config::NodeKind::Call { macro_id } => {
            ui.horizontal(|ui| {
                ui.label("play macro");
                let mut chosen = *macro_id;
                egui::ComboBox::from_id_salt(format!("call-{id}"))
                    .selected_text(
                        calls
                            .iter()
                            .find(|(other, _)| other == macro_id)
                            .map(|(other, name)| format!("{other} {name}"))
                            .unwrap_or_else(|| format!("{macro_id}")),
                    )
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for (other, name) in &calls {
                            ui.selectable_value(&mut chosen, *other, format!("{other} {name}"));
                        }
                    });
                if chosen != *macro_id {
                    window.set_macro_node_number(id, "macro", chosen as f64);
                }
            });
        }
        g13_config::NodeKind::Run { spec } => {
            ui.horizontal(|ui| {
                ui.label("do this:");
                let mut said = spec.clone();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut said)
                            .desired_width(320.0)
                            .hint_text("cmd:notify-send done"),
                    )
                    .changed()
                {
                    if let Some(problem) = window.set_macro_node_spec(id, &said) {
                        window.macro_problem = Some(problem);
                    }
                }
            });
            ui.label(
                "Written like an applet's source: `cmd:` runs a command, and an endpoint name asks it \
                 something. What it returns is thrown away - this is the doing side.",
            );
        }
        g13_config::NodeKind::Stop => {
            ui.label("The macro finishes here.");
        }
        g13_config::NodeKind::If { when } => match crate::macros::ConditionForm::from_cond(when) {
            Some(form) => {
                // what this question may ask about: this macro's own sources first, then everything published
                let names = crate::macros::Names::for_macro(graph);
                if let Some(edited) = condition_rows(ui, id, &form, &names) {
                    window.set_macro_node_condition(id, edited.to_cond());
                }
            }
            None => {
                ui.label(
                    "This question is one only the file holds (questions inside questions), so it is shown \
                     and not rewritten.",
                );
            }
        },
    }

    // the ways out, as a list as well as dots: a dot is quicker, a list is certain
    for port in crate::macros::ports_of_kind(&node.kind) {
        ui.horizontal(|ui| {
            ui.label(format!(
                "{port} ({}) ->",
                crate::macros::describe_port(port)
            ));
            let current = crate::macros::port_target(graph, id, port);
            let mut chosen = current.clone();
            egui::ComboBox::from_id_salt(format!("{id}-{port}"))
                .selected_text(chosen.clone().unwrap_or_else(|| "(nothing)".to_string()))
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, "(nothing)");
                    for other in &ids {
                        if other == id {
                            continue;
                        }
                        ui.selectable_value(&mut chosen, Some(other.clone()), other);
                    }
                });
            if chosen != current {
                window.set_macro_port(id, port, chosen.as_deref());
            }
            if current.is_none() {
                ui.label("(nothing means the macro ends down that way)");
            }
        });
    }
}

/// The words the window uses for a key mode.
fn mode_name(mode: g13_config::KeyMode) -> &'static str {
    match mode {
        g13_config::KeyMode::Tap => "tap",
        g13_config::KeyMode::Down => "hold down",
        g13_config::KeyMode::Up => "release",
    }
}

/// One `if` node's question: the rows, the join, and adding another.
fn condition_rows(
    ui: &mut egui::Ui,
    node: &str,
    form: &crate::macros::ConditionForm,
    names: &crate::macros::Names,
) -> Option<crate::macros::ConditionForm> {
    let mut edited = form.clone();
    let mut changed = false;
    for (index, row) in form.rows.iter().enumerate() {
        ui.horizontal(|ui| {
            // What the row asks, chosen first. Without this a row was whatever it was created as and stayed
            // that way, so a second question could only ever be a "fired by" and had no value, no comparison and
            // nothing to compare against - which is what made one question work and two not.
            let kind = match &row.question {
                crate::macros::Question::Control(_) => "fired by",
                crate::macros::Question::Profile(_) => "profile",
                crate::macros::Question::Value { .. } => "value",
            };
            let mut chosen_kind = kind.to_string();
            egui::ComboBox::from_id_salt(format!("kind-{node}-{index}"))
                .selected_text(kind)
                .width(80.0)
                .show_ui(ui, |ui| {
                    for option in ["value", "fired by", "profile"] {
                        ui.selectable_value(&mut chosen_kind, option.to_string(), option);
                    }
                });
            if chosen_kind != kind {
                edited.rows[index].question = match chosen_kind.as_str() {
                    "fired by" => crate::macros::Question::Control(String::new()),
                    "profile" => crate::macros::Question::Profile(0),
                    _ => crate::macros::Question::Value {
                        name: names.first.clone(),
                        op: g13_values::Compare::Gt,
                        number: Some(80.0),
                        text: None,
                    },
                };
                changed = true;
            }
            let mut not = row.not;
            if ui.checkbox(&mut not, "not").changed() {
                edited.rows[index].not = not;
                changed = true;
            }
            match &row.question {
                crate::macros::Question::Control(control) => {
                    ui.label("fired by");
                    let mut control = control.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut control)
                                .desired_width(70.0)
                                .hint_text("G7"),
                        )
                        .changed()
                    {
                        edited.rows[index].question = crate::macros::Question::Control(control);
                        changed = true;
                    }
                }
                crate::macros::Question::Profile(profile) => {
                    ui.label("profile");
                    let mut profile = *profile;
                    if ui
                        .add(egui::DragValue::new(&mut profile).range(0..=8))
                        .changed()
                    {
                        edited.rows[index].question = crate::macros::Question::Profile(profile);
                        changed = true;
                    }
                    ui.label("is active");
                }
                crate::macros::Question::Value {
                    name,
                    op,
                    number,
                    text,
                } => {
                    ui.label("value");
                    let mut name = name.clone();
                    let mut chosen: Option<String> = None;
                    egui::ComboBox::from_id_salt(format!("value-{node}-{index}"))
                        .selected_text(if name.is_empty() {
                            "pick one".to_string()
                        } else {
                            name.clone()
                        })
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            // the macro's own sources first: those are what it declared, and what a source
                            // shadows a published value of the same name is worth seeing before picking
                            for candidate in &names.all {
                                let mine = names.mine.contains(candidate);
                                ui.selectable_value(
                                    &mut chosen,
                                    Some(candidate.clone()),
                                    if mine {
                                        format!("{candidate} (declared here)")
                                    } else {
                                        candidate.clone()
                                    },
                                );
                            }
                        });
                    if let Some(chosen) = chosen {
                        name = chosen;
                        edited.rows[index].question = crate::macros::Question::Value {
                            name: name.clone(),
                            op: *op,
                            number: *number,
                            text: text.clone(),
                        };
                        changed = true;
                    }
                    let original_op = *op;
                    let mut op = *op;
                    egui::ComboBox::from_id_salt(format!("op-{node}-{index}"))
                        .selected_text(crate::macros::op_words(op))
                        .width(90.0)
                        .show_ui(ui, |ui| {
                            for option in [
                                g13_values::Compare::Eq,
                                g13_values::Compare::Ne,
                                g13_values::Compare::Lt,
                                g13_values::Compare::Le,
                                g13_values::Compare::Gt,
                                g13_values::Compare::Ge,
                                g13_values::Compare::Contains,
                            ] {
                                ui.selectable_value(
                                    &mut op,
                                    option,
                                    crate::macros::op_words(option),
                                );
                            }
                        });
                    if op != original_op {
                        edited.rows[index].question = crate::macros::Question::Value {
                            name: name.clone(),
                            op,
                            number: *number,
                            text: text.clone(),
                        };
                        changed = true;
                    }
                    let mut against = match (number, text) {
                        (_, Some(text)) => text.clone(),
                        (Some(number), None) => number.to_string(),
                        (None, None) => String::new(),
                    };
                    if ui
                        .add(egui::TextEdit::singleline(&mut against).desired_width(90.0))
                        .changed()
                    {
                        // a number when it reads as one, a string when it does not: the comparison is the same
                        // question either way, and this is the only place that decides
                        let (number, text) = match against.trim().parse::<f64>() {
                            Ok(number) => (Some(number), None),
                            Err(_) => (None, Some(against.clone())),
                        };
                        edited.rows[index].question = crate::macros::Question::Value {
                            name: name.clone(),
                            // `op` here is the local above, not the one from the pattern
                            op,
                            number,
                            text,
                        };
                        changed = true;
                    }
                }
            }
            if ui.button("remove").clicked() {
                edited.rows.remove(index);
                changed = true;
            }
        });
    }
    ui.horizontal(|ui| {
        if form.rows.len() > 1 {
            let mut all = form.all;
            egui::ComboBox::from_id_salt(format!("join-{node}"))
                .selected_text(if all { "all of" } else { "any of" })
                .width(80.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut all, true, "all of");
                    ui.selectable_value(&mut all, false, "any of");
                });
            if all != form.all {
                edited.all = all;
                changed = true;
            }
        }
        if ui.button("add a question").clicked() {
            // The same shape the first question starts as: something this reads, compared with something. A new
            // row used to be a "fired by" whatever the first one was, so a second question arrived with no value
            // and no comparison.
            edited.rows.push(crate::macros::QuestionRow {
                not: false,
                question: crate::macros::Question::Value {
                    name: names.first.clone(),
                    op: g13_values::Compare::Gt,
                    number: Some(80.0),
                    text: None,
                },
            });
            changed = true;
        }
        ui.label(format!("now: {}", form.describe()));
    });
    match changed {
        true => Some(edited),
        false => None,
    }
}

/// Every value this build publishes: what it is, what it is now, and who reads it.
///
/// The point of the page is that nothing about values has to be remembered or discovered: the catalogue is the
/// framework, an applet names an entry, and this is the list. It is read from the same `PUBLISHED` the resolver
/// uses, so the page cannot offer something that does not work.
fn values(ui: &mut egui::Ui, window: &mut Window) {
    // Two columns: every value on the left, the chosen one's detail on the right - the shell every tab shares,
    // and the same `cell()` the applet and bindings tabs use, so all three scroll the same way.
    ui.columns(2, |panes| {
        cell(&mut panes[0], "values-table", None, |ui| {
            ui.strong("every value");
            ui.label("an applet uses one by naming it in its `sources`. Yours are the ones you named yourself in values.json.");
            ui.add_space(6.0);

            let rows = window.all_value_rows();
            let mut chosen: Option<String> = None;
            egui::Grid::new("published-values")
                .num_columns(4)
                .striped(true)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.strong("value");
                    ui.strong("what it is");
                    ui.strong("comes from");
                    ui.strong("now");
                    ui.end_row();
                    for row in &rows {
                        let open = window.value_selected.as_deref() == Some(row.name.as_str());
                        if ui
                            .selectable_label(open, egui::RichText::new(&row.name).monospace())
                            .clicked()
                        {
                            chosen = Some(row.name.clone());
                        }
                        ui.label(row.meaning.as_str());
                        ui.weak(match (row.mine, row.computed) {
                            (true, _) => "you",
                            (false, true) => "the machine",
                            (false, false) => "the driver",
                        });
                        if row.now.is_empty() {
                            // a stick nobody has touched is not published: say that rather than show a zero
                            ui.weak("not published yet");
                        } else {
                            ui.monospace(row.now.as_str());
                        }
                        ui.end_row();
                    }
                });
            if let Some(name) = chosen {
                window.value_selected = match window.value_selected.as_deref() == Some(name.as_str()) {
                    true => None,
                    false => Some(name),
                };
            }

            ui.add_space(8.0);
            // Named once and used anywhere, which is the whole point: the same spec typed into five applets drifts.
            ui.horizontal(|ui| {
                ui.label("new value:");
                ui.add(
                    egui::TextEdit::singleline(&mut window.new_value_name)
                        .hint_text("name")
                        .desired_width(120.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut window.new_value_spec)
                        .hint_text("cmd:uptime  |  file:/proc/loadavg  |  http:gpu/load#value")
                        .desired_width(280.0),
                );
                if ui.button("add it").clicked() {
                    let (name, spec) = (
                        window.new_value_name.clone(),
                        window.new_value_spec.clone(),
                    );
                    match window.add_custom_value(&name, &spec) {
                        Ok(()) => {
                            window.new_value_name.clear();
                            window.new_value_spec.clear();
                            window.value_problem = None;
                        }
                        // a refusal is said, never done quietly
                        Err(problem) => window.value_problem = Some(problem),
                    }
                }
            });
            if let Some(problem) = window.value_problem.clone() {
                ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), problem);
            }

            ui.add_space(4.0);
            let mine = rows.iter().filter(|row| row.mine).count();
            let used = rows.iter().filter(|row| !row.used_by.is_empty()).count();
            ui.weak(format!(
                "{} value(s), {mine} of them yours, {used} used by an applet.",
                rows.len()
            ));
        });

        cell(&mut panes[1], "value-detail", None, |ui| {
            let rows = window.all_value_rows();
            let Some(row) = window
                .value_selected
                .as_ref()
                .and_then(|name| rows.iter().find(|row| &row.name == name))
            else {
                ui.strong("a value");
                ui.weak("click one on the left to see what it is, what it reads now, and who uses it.");
                return;
            };
            ui.strong(row.name.as_str());
            ui.label(row.meaning.as_str());
            ui.add_space(4.0);
            if row.mine {
                // A value the user named: its spec is the thing to change, and it writes as it is typed, like
                // everything else.
                ui.horizontal(|ui| {
                    ui.label("spec:");
                    let mut spec = row.meaning.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut spec)
                                .desired_width(320.0)
                                .hint_text("cmd:uptime  |  file:/proc/loadavg  |  http:gpu/load#value"),
                        )
                        .changed()
                    {
                        window.set_custom_value(&row.name, &spec);
                    }
                    if ui
                        .button("remove")
                        .on_hover_text("takes this value out of values.json")
                        .clicked()
                    {
                        window.set_custom_value(&row.name, "");
                        window.value_selected = None;
                    }
                });
                ui.add_space(4.0);
            }
            egui::Grid::new("value-detail-grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.label("comes from");
                    ui.label(match (row.mine, row.computed) {
                        (true, _) => "you, in values.json",
                        (false, true) => "the machine, read when it is asked for",
                        (false, false) => "the running driver",
                    });
                    ui.end_row();
                    ui.label("now");
                    if row.now.is_empty() {
                        ui.weak("not published: nothing is reading it");
                    } else {
                        ui.monospace(row.now.as_str());
                    }
                    ui.end_row();
                    ui.label("used by");
                    if row.used_by.is_empty() {
                        ui.weak("no applet or macro reads it yet");
                    } else {
                        ui.label(row.used_by.join(", "));
                    }
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.label("to read it in an applet, declare it:");
            ui.monospace(format!(
                "{{\"sources\": {{\"{}\": \"{}\"}}}}",
                row.name, row.name
            ));
            if !row.computed {
                ui.weak("only the running driver can answer this one: with no driver it reads as nothing.");
            }
        });
    });
}

/// The endpoints applets read: what exists, what each is pointed at, and which applets need it.
///
/// The file is where a credential lives, so a token is masked unless it is being changed and no message this
/// tab writes ever contains one. Every change is written the moment it is made - there is no Apply button
/// anywhere in this window.
fn sources(ui: &mut egui::Ui, window: &mut Window) {
    ui.label(
        "an applet reads the network through a named endpoint rather than carrying a host itself:",
    );
    ui.monospace("http:<endpoint>/<path>#<field>");
    // The tab is named for the file it writes. It used to be called Sources, which read as "an applet's
    // sources" and is not what this is - those are per applet, on the Applets tab.
    ui.label("this tab is the endpoints file, shared by every applet. an applet's own sources are on the Applets tab.");
    ui.label(format!("written to {}", window.endpoints_path().display()));
    ui.add_space(6.0);

    if let Some(problem) = window.endpoints_problem.clone() {
        ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), problem);
        ui.colored_label(
            egui::Color32::from_rgb(0xff, 0x9c, 0x60),
            "the table below is empty because the file could not be read, not because nothing is set up",
        );
        ui.add_space(6.0);
    }

    // Two cells: what exists on the left, and everything about the chosen one on the right. Editing used to be a
    // field per row in the table, which meant the table was also a form - and grew a column every time an endpoint
    // learned something new. The setting belongs to one endpoint, so it belongs in one place.
    let names: Vec<String> = window.endpoints.keys().cloned().collect();
    ui.columns(2, |panes| {
        cell(&mut panes[0], "endpoints-table", None, |ui| {
            ui.strong("endpoints");
            if names.is_empty() && window.endpoints_problem.is_none() {
                ui.label("none yet. an applet that reads the network needs one.");
            }
            for name in &names {
                let endpoint = window.endpoints.get(name).cloned().unwrap_or_default();
                let used = window
                    .endpoint_use
                    .iter()
                    .find(|(endpoint, _)| endpoint == name)
                    .map(|(_, applets)| applets.clone())
                    .unwrap_or_default();
                let selected = window.endpoint_selected.as_deref() == Some(name.as_str());
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(selected, name.as_str()).clicked() {
                        window.endpoint_selected = Some(name.clone());
                    }
                    // masked, and the length only: enough to tell one token from another and from none, without
                    // putting the credential on the screen of a machine somebody may be looking at
                    match &endpoint.token {
                        Some(token) => {
                            ui.weak(format!("token set, {} characters", token.chars().count()))
                        }
                        None => ui.weak("no token"),
                    };
                    if used.is_empty() {
                        ui.weak("nothing uses it");
                    } else {
                        ui.weak(format!("used by {}", used.join(", ")));
                    }
                });
                // an endpoint whose url is not a host cannot work - a `cmd:` line put here would be pasted into a
                // url and fetched - so it is said on its row rather than left to fail later, silently, in an applet
                match g13_sources::endpoint_url_problem(&endpoint.url) {
                    Some(problem) => {
                        ui.colored_label(
                            egui::Color32::from_rgb(0xff, 0x9c, 0x60),
                            format!("   {problem}"),
                        );
                    }
                    None => {
                        ui.weak(format!("   {}", endpoint.url));
                    }
                }
                // and a credential on a link that cannot keep it. Not a refusal: an `http:` host on a private
                // network is a real setup, and `insecure` is written down for a reason. Not silence either,
                // because a token sent where somebody can read it is not something to find out later
                if let Some(risk) = g13_sources::endpoint_risk(&endpoint) {
                    ui.colored_label(
                        egui::Color32::from_rgb(0xff, 0x9c, 0x60),
                        format!("   {risk}"),
                    );
                }
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("add:");
                ui.add(
                    egui::TextEdit::singleline(&mut window.new_endpoint_name)
                        .hint_text("name")
                        .desired_width(110.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut window.new_endpoint_url)
                        .hint_text("https://host")
                        .desired_width(180.0),
                );
                if ui.button("add").clicked() {
                    let (name, url) = (
                        window.new_endpoint_name.clone(),
                        window.new_endpoint_url.clone(),
                    );
                    window.add_endpoint(&name, &url);
                    window.endpoint_selected = Some(name.trim().to_string());
                    window.new_endpoint_name.clear();
                    window.new_endpoint_url.clear();
                }
            });
        });

        cell(&mut panes[1], "endpoint-settings", None, |ui| {
            ui.strong("this endpoint");
            let chosen = window
                .endpoint_selected
                .clone()
                .filter(|name| window.endpoints.contains_key(name));
            let Some(name) = chosen else {
                ui.label("pick one on the left, or add one below the list.");
                return;
            };
            let mut endpoint = window.endpoints.get(&name).cloned().unwrap_or_default();
            let mut changed = false;
            let mut remove = false;
            let mut revealing_after: Option<Option<String>> = None;

            ui.horizontal(|ui| {
                ui.strong(name.as_str());
                if ui.button("remove").clicked() {
                    remove = true;
                }
            });
            ui.add_space(4.0);
            ui.label("url");
            if ui.text_edit_singleline(&mut endpoint.url).changed() {
                changed = true;
            }
            ui.add_space(4.0);
            ui.label("token");
            ui.horizontal(|ui| {
                let revealing = window.revealing.as_deref() == Some(name.as_str());
                if revealing {
                    let mut token = endpoint.token.clone().unwrap_or_default();
                    if ui.text_edit_singleline(&mut token).changed() {
                        endpoint.token = if token.trim().is_empty() {
                            None
                        } else {
                            Some(token)
                        };
                        changed = true;
                    }
                    if ui.button("done").clicked() {
                        revealing_after = Some(None);
                    }
                } else {
                    match &endpoint.token {
                        Some(token) => {
                            ui.label(format!("{} hidden", "*".repeat(6)));
                            ui.weak(format!("{} characters", token.chars().count()));
                        }
                        None => {
                            ui.weak("none");
                        }
                    }
                    if ui.button("change").clicked() {
                        revealing_after = Some(Some(name.clone()));
                    }
                    if endpoint.token.is_some() && ui.button("clear").clicked() {
                        endpoint.token = None;
                        changed = true;
                    }
                }
            });
            ui.add_space(4.0);
            ui.label("how long to wait, and whether to check the certificate");
            ui.horizontal(|ui| {
                let mut timeout = endpoint.timeout.unwrap_or(0.0);
                if ui
                    .add(
                        egui::DragValue::new(&mut timeout)
                            .range(0.0..=60.0)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    endpoint.timeout = if timeout > 0.0 { Some(timeout) } else { None };
                    changed = true;
                }
                if ui
                    .checkbox(&mut endpoint.insecure, "no certificate check")
                    .changed()
                {
                    changed = true;
                }
            });

            if let Some(revealing) = revealing_after {
                window.revealing = revealing;
            }
            if remove {
                window.remove_endpoint(&name);
                window.endpoint_selected = None;
            } else if changed {
                window.endpoints.insert(name.clone(), endpoint);
                window.commit_endpoints();
            }
        });
    });

    // an applet naming an endpoint nothing answers to is a fault the renderer reports in its own list; saying
    // it here is what makes it fixable rather than merely reported
    let missing = window.endpoint_missing.clone();
    if !missing.is_empty() {
        ui.add_space(8.0);
        for (endpoint, applets) in missing {
            ui.colored_label(
                egui::Color32::from_rgb(0xff, 0x9c, 0x60),
                format!(
                    "{} is named by {} but no endpoint has that name",
                    endpoint,
                    applets.join(", ")
                ),
            );
        }
    }
}

/// The Bindings tab: the control map on the left, the stick's own settings on the right.
fn bindings(ui: &mut egui::Ui, window: &mut Window) {
    // Two columns: the map on the left, and everything about the stick on the right, stacked vertically. The
    // stick's settings used to be a tab of their own with its blocks side by side, which is one more place to
    // look for something that is part of the same thing - what a control does.
    ui.columns(2, |panes| {
        cell(&mut panes[0], "bindings-table", None, |ui| {
            // The screen's colour is a line in this profile's own file, so it is set from this tab. Live: the driver
            // watches the file, so a change here reaches the pad within a second. A profile that names no colour says
            // so instead of showing a swatch that would mean nothing.
            ui.horizontal(|ui| {
        ui.strong("screen colour");
        match window.binding_colour() {
            Some(colour) => {
                let mut rgb = [colour.red, colour.green, colour.blue];
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    let colour = g13_config::Colour {
                        red: rgb[0],
                        green: rgb[1],
                        blue: rgb[2],
                    };
                    window.status = match window.set_binding_colour(colour) {
                        Ok(()) => format!("the screen is now {}", colour.text()),
                        Err(problem) => problem,
                    };
                }
                ui.weak(colour.text());
            }
            None => {
                if ui.button("set a colour").clicked() {
                    window.status = match window.set_binding_colour(g13_config::Colour {
                        red: 255,
                        green: 255,
                        blue: 255,
                    }) {
                        Ok(()) => "this profile now names white".to_string(),
                        Err(problem) => problem,
                    };
                }
                ui.weak("this profile names none, so the screen keeps whatever it was last told");
            }
        }
    });

            ui.horizontal(|ui| {
                ui.label("binding set:");
                let profiles = window.profiles.clone();
                for profile in profiles {
                    let name = window.profile_name(profile);
                    let key = window.profile_key(profile).map(|k| format!("  ({k})"));
                    let label = match key {
                        Some(key) => format!("{name}{key}"),
                        None => name,
                    };
                    if ui
                        .selectable_label(window.profile == profile, label)
                        .clicked()
                    {
                        window.select_profile(profile);
                    }
                }
                if ui.button("new set").clicked() {
                    window.new_profile();
                    window.profile_name_draft.clear();
                }
                let only_one = window.profiles.len() <= 1;
                ui.add_enabled_ui(!only_one, |ui| {
                    if ui.button("delete this set").clicked() {
                        let current = window.profile;
                        window.delete_profile(current);
                        window.profile_name_draft.clear();
                    }
                });
                if only_one {
                    ui.weak("the last set cannot be deleted");
                }
            });

            // The set's own name, and which M key switches to it. The M keys auto-bind to the set with the same
            // number, so a set with no key on it still answers to the number printed on the pad - assigning one
            // is how that is changed.
            ui.horizontal(|ui| {
                let current = window.profile;
                ui.label("name:");
                if window.profile_name_draft.is_empty() {
                    window.profile_name_draft = window.profile_name(current);
                }
                let response = ui.add(
                    egui::TextEdit::singleline(&mut window.profile_name_draft)
                        .desired_width(160.0)
                        .hint_text("Profile N"),
                );
                if response.changed() {
                    let draft = window.profile_name_draft.clone();
                    window.rename_profile(current, &draft);
                }
                if ui.button("clear name").clicked() {
                    window.rename_profile(current, "");
                    window.profile_name_draft.clear();
                }
                ui.label("  M key:");
                let assigned = window.profile_key(current).map(|k| k.to_string());
                for key in ["M1", "M2", "M3"] {
                    if ui
                        .selectable_label(assigned.as_deref() == Some(key), key)
                        .clicked()
                    {
                        if assigned.as_deref() == Some(key) {
                            window.assign_profile_key(current, None);
                        } else {
                            window.assign_profile_key(current, Some(key));
                        }
                    }
                }
                if assigned.is_none() {
                    ui.weak(format!(
                        "no key given, so {current} is what the M key with that number switches to"
                    ));
                }
            });
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("bindings")
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("control");
                        ui.strong("bit");
                        ui.strong("action");
                        ui.strong("what it does");
                        ui.end_row();

                        let rows = window.rows.clone();
                        for row in rows {
                            ui.label(&row.control);
                            ui.label(match row.bit {
                                Some(bit) => bit.to_string(),
                                None => "-".to_string(),
                            });

                            if window.editing.as_deref() == Some(row.control.as_str()) {
                                action_editor(ui, window);
                            } else {
                                binding_button(ui, window, &row.control, &row.action);
                            }

                            // what it does when held, and the way to set it: the same editor, editing `LR.hold`
                            let held_name = format!("{}.hold", row.control);
                            if window.editing.as_deref() == Some(held_name.as_str()) {
                                action_editor(ui, window);
                            } else {
                                let label = match &row.held {
                                    Some(held) => format!("held: {}", window.bound_label(held)),
                                    None => "+ hold".to_string(),
                                };
                                if ui.button(label).clicked() {
                                    window.begin_edit(&held_name);
                                }
                            }

                            ui.label(&row.effect);
                            ui.end_row();
                        }
                    });
            });
        });
        cell(&mut panes[1], "bindings-stick", None, |ui| {
            controls(ui, window);
        });
    });
}

/// What one control does: pick a macro for it, or press the thing to set the key.
fn action_editor(ui: &mut egui::Ui, window: &mut Window) {
    // A macro is *picked*: the box that used to be here took `m,4,1` by hand, which is the file's syntax and not
    // something a person should have to know, and typing `x` into it did nothing because `x` is not a key. The
    // bindings themselves are set by pressing the thing, which is what this button is for.
    let listening = window.capture.is_some();
    let press_clicked = ui
        .add_enabled(!listening, egui::Button::new("press to set"))
        .clicked();
    let held = window.binding_being_edited();
    let macros: Vec<(u32, String)> = window
        .macro_rows
        .iter()
        .filter(|row| !row.empty)
        .map(|row| (row.id, row.name.clone()))
        .collect();
    egui::ComboBox::from_id_salt("binding-macro")
        .selected_text(match held.is_empty() {
            true => "a macro...".to_string(),
            false => held.clone(),
        })
        .show_ui(ui, |ui| {
            for (id, name) in &macros {
                let shown = match name.is_empty() {
                    true => format!("macro {id}"),
                    false => name.clone(),
                };
                if ui.selectable_label(false, shown).clicked() {
                    window.bind_macro(*id);
                }
            }
            ui.separator();
            if ui.selectable_label(held == "nothing", "nothing").clicked() {
                window.bind_nothing();
            }
        });
    let cancel_clicked = ui.button("cancel").clicked();
    if press_clicked {
        window.start_capture();
    } else if cancel_clicked {
        window.cancel_edit();
    }
    if listening {
        ui.colored_label(
            egui::Color32::from_rgb(0x8d, 0xd0, 0xff),
            "listening - press a key, a mouse button or a gamepad button",
        );
    }
}

/// What a name is bound to right now, as a button. Clicking it opens the editor.
fn binding_button(ui: &mut egui::Ui, window: &mut Window, name: &str, action: &str) {
    // the key it is, the macro's own name, or the action as it stands - never `p,k.16` on a row a person reads
    let label = window.bound_label(action);
    if ui.button(label).clicked() {
        window.begin_edit(name);
    }
}

/// What the pad is doing right now, and the calibration of its stick.
///
/// The driver holds the pad, so this shows what the driver reports rather than opening the pad for itself.
/// The stick's two bytes have no established meaning yet, so they are shown as they are and plotted as they
/// move: watching them go where the stick goes is how the meaning gets settled.
fn controls(ui: &mut egui::Ui, window: &mut Window) {
    ui.horizontal(|ui| {
        ui.label(format!("profile {}", window.profile));
        ui.separator();
        ui.label(format!(
            "driver: {}",
            match window.live.age() {
                Some(age) => format!("reporting, last read {:.1}s ago", age.as_secs_f32()),
                None => "nothing published".to_string(),
            }
        ));
    });
    ui.separator();

    ui.vertical(|ui| {
        ui.label("the stick, as the driver reports it");
        let size = 200.0_f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, egui::Color32::from_gray(20));
        // a cross through the middle, to judge a centre by
        painter.line_segment(
            [rect.center_top(), rect.center_bottom()],
            egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
        );
        painter.line_segment(
            [rect.left_center(), rect.right_center()],
            egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
        );

        match window.live.stick() {
            Some((first, second)) => {
                // plotted as they are: byte to x, byte to y. Until the meaning is established this is a
                // plot of the raw numbers, and it exists so the movement can be seen at all.
                let at = rect.min
                    + egui::vec2(first as f32 / 255.0 * size, second as f32 / 255.0 * size);
                painter.circle_filled(at, 4.0, egui::Color32::from_rgb(0x8d, 0xd0, 0xff));
                ui.label(format!("raw: {}", state::show((first, second))));
            }
            None => {
                ui.label("raw: nothing reported");
            }
        }
    });

    ui.separator();
    ui.vertical(|ui| {
            ui.label("the stick drives");
            ui.horizontal(|ui| {
                for mode in g13_config::stick::Mode::ALL {
                    let chosen = window.stick.mode == mode;
                    if ui.selectable_label(chosen, mode.name()).clicked() && !chosen {
                        window.set_stick_mode(mode);
                    }
                }
            });
            match window.stick.mode {
                g13_config::stick::Mode::Off => {
                    ui.label("turn it on and the stick sends the keys its directions are bound to.");
                }
                g13_config::stick::Mode::Keyboard => {
                    ui.label(
                        "the turn is broken into sectors, and each one does whatever it is bound to - a key, \
                         a mouse button or a controller button.",
                    );
                    // how many sectors. Four is a d-pad and eight is what the old stack did; anything from
                    // two to sixty-four is a valid set of ranges, so this is a number rather than a choice.
                    ui.horizontal(|ui| {
                        ui.label("sectors:");
                        for count in [4u32, 8, 16, 32] {
                            let chosen = window.stick.sectors.count == count;
                            if ui.selectable_label(chosen, format!("{count}")).clicked() && !chosen {
                                window.set_sector_count(count);
                            }
                        }
                        let mut count = window.stick.sectors.count;
                        if ui
                            .add(
                                egui::DragValue::new(&mut count)
                                    .range(g13_config::stick::Sectors::LEAST..=g13_config::stick::Sectors::MOST),
                            )
                            .changed()
                        {
                            window.set_sector_count(count);
                        }
                        ui.label("(any number)");
                    });
                    ui.label("sector 0 is up and they count clockwise");
                    // Every sector, so the shape of the radial is visible and an unbound one is visible too,
                    // and every one of them settable - by name, or by pressing the thing.
                    let sectors = window.stick.sectors;
                    let rows = window.sector_rows();
                    let here = window.stick_sector();
                    let bound = rows.iter().filter(|row| !row.effect.is_empty() && row.effect != "nothing").count();
                    egui::Grid::new("sectors")
                        .num_columns(3)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("sector");
                            ui.strong("bound to");
                            ui.strong("what it does");
                            ui.end_row();
                            for row in rows {
                                let name = egui::RichText::new(format!(
                                    "{}  {}",
                                    row.name,
                                    sectors.words(row.index)
                                ));
                                ui.label(if here == Some(row.index) {
                                    name.strong()
                                        .background_color(egui::Color32::from_rgb(0x2f, 0x5a, 0x38))
                                } else {
                                    name
                                });
                                if window.editing.as_deref() == Some(row.name.as_str()) {
                                    action_editor(ui, window);
                                } else if row.inherited && row.action.is_empty() {
                                    // nothing of its own: it falls back to the cardinals beside it, and the
                                    // button that would set one of its own says so
                                    let button = ui.add(egui::Button::new(
                                        egui::RichText::new("(the cardinals beside it)").weak(),
                                    ));
                                    if button.clicked() {
                                        window.begin_edit(&row.name);
                                    }
                                } else {
                                    binding_button(ui, window, &row.name, &row.action);
                                }
                                ui.label(&row.effect);
                                ui.end_row();
                            }
                        });
                    if bound == 0 {
                        ui.colored_label(
                            egui::Color32::from_rgb(0xff, 0x9c, 0x60),
                            "nothing is bound, so the stick sends nothing yet",
                        );
                    } else {
                        ui.label(format!("{bound} of the {} sectors do something", sectors.count));
                    }
                }
                g13_config::stick::Mode::Mouse => {
                    ui.label("the pointer moves faster the further the stick is held.");
                    let mut speed = window.stick.speed as f32;
                    if ui
                        .add(
                            egui::Slider::new(&mut speed, 20.0..=3000.0)
                                .suffix(" px/s")
                                .show_value(false),
                        )
                        .changed()
                    {
                        window.set_speed(speed as f64);
                    }
                    ui.label(format!(
                        "{:.0} pixels a second at full deflection",
                        window.stick.speed
                    ));
                }
                g13_config::stick::Mode::Joystick => {
                    ui.label("the stick is reported as a gamepad's analogue stick.");
                    ui.label(
                        "Each side can go somewhere different: steering on the stick, throttle and brake \
                         on the triggers, for a game that drives.",
                    );
                    ui.add_space(4.0);
                    for (side, chosen) in window.stick.route.sides() {
                        ui.horizontal(|ui| {
                            ui.label(format!("{side}:"));
                            let mut wanted = chosen;
                            egui::ComboBox::from_id_salt(format!("route-{side}"))
                                .selected_text(chosen.name())
                                .show_ui(ui, |ui| {
                                    for target in g13_config::stick::Target::ALL {
                                        ui.selectable_value(&mut wanted, target, target.name());
                                    }
                                });
                            if wanted != chosen {
                                window.set_route(side, wanted);
                            }
                            // a trigger only opens one way, which is worth knowing before choosing one
                            if matches!(
                                chosen,
                                g13_config::stick::Target::LeftTrigger
                                    | g13_config::stick::Target::RightTrigger
                            ) {
                                ui.weak("opens one way");
                            }
                        });
                    }
                    ui.add_space(4.0);
                    if !window.stick.route.is_a_plain_stick() {
                        ui.label("two sides go to different places, so the stick is split.");
                    }
                    if !window.stick.missing(&state::CALIBRATION_NAMES).is_empty() {
                        ui.colored_label(
                            egui::Color32::from_rgb(0xff, 0x9c, 0x60),
                            "the calibration is incomplete, so it is centred on the defaults",
                        );
                    }
                }
            }
            ui.add_space(6.0);
            ui.label("how far it must move");
            let mut deadzone = window.stick.stick.deadzone as f32;
            if ui
                .add(egui::Slider::new(&mut deadzone, 0.05..=0.9).show_value(false))
                .changed()
            {
                window.set_deadzone(deadzone as f64);
            }
            ui.label(format!(
                "{:.0}% - a resting hand does not quite reach it",
                window.stick.stick.deadzone * 100.0
            ));
            ui.add_space(6.0);
            // the same calculation the driver does, from the same numbers, so this is what it would do
            match window.stick_sector() {
                Some(index) => {
                    ui.colored_label(
                        egui::Color32::from_rgb(0x8d, 0xd0, 0xff),
                        format!(
                            "reading: sector {} - {}",
                            window.stick.sectors.name(index),
                            window.stick.sectors.words(index)
                        ),
                    );
                }
                None => {
                    ui.label("reading: centred");
                }
            }
            if !window.stick.missing(&state::CALIBRATION_NAMES).is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(0xff, 0x9c, 0x60),
                    "the calibration is incomplete, so the directions are being read from the defaults",
                );
            }
        });

    ui.separator();
    ui.vertical(|ui| {
        ui.label("calibrate the stick");
        ui.label("rest or hold the stick at each point, then take the reading.");
        ui.label(format!(
            "{} of {} points taken",
            state::CALIBRATION_POINTS.len() - window.stick.missing(&state::CALIBRATION_NAMES).len(),
            state::CALIBRATION_POINTS.len()
        ));

        match window.taking {
            Some(index) => {
                let (name, instruction) = state::CALIBRATION_POINTS[index];
                ui.label(egui::RichText::new(format!("{name}: {instruction}")).strong());
                if ui.button("take reading").clicked() {
                    window.take_calibration_point();
                }
                if ui.button("stop").clicked() {
                    window.taking = None;
                }
            }
            None => {
                if ui.button("start").clicked() {
                    window.taking = Some(
                        state::CALIBRATION_POINTS
                            .iter()
                            .position(|(name, _)| !window.stick.points.contains_key(*name))
                            .unwrap_or(0),
                    );
                }
            }
        }

        ui.separator();
        for (name, _) in state::CALIBRATION_POINTS {
            match window.stick.points.get(name) {
                Some(pair) => ui.label(format!("  {name:<11} {}", state::show(*pair))),
                None => ui.colored_label(egui::Color32::from_gray(120), format!("  {name:<11} -")),
            };
        }
        ui.separator();
        ui.label(format!("written to {}", window.stick.path.display()));
    });

    ui.separator();
    ui.vertical(|ui| {
        ui.label("recently pressed");
        for key in window.live.recent_keys().iter().rev().take(10) {
            ui.label(format!("  {key}"));
        }
        ui.separator();
        for key in ["cpu", "memory", "last_key", "screen_visual"] {
            ui.label(format!("{key}: {}", window.live.text(key)));
        }
    });
}

/// The themes there are, and the one being edited.
///
/// A theme is a file, so this shows the file: the fields on the right are the theme's own keys, and a change is
/// written at once because the driver re-reads the file as it draws. Nothing here is a copy kept in step by hand.
fn themes(ui: &mut egui::Ui, window: &mut Window) {
    ui.columns(2, |panes| {
        cell(&mut panes[0], "themes-list", None, |ui| {
            ui.label("themes/");
            ui.separator();
            for (name, summary) in window.theme_rows() {
                let summary = summary.unwrap_or_default();
                let open = window.theme_editing.as_deref() == Some(name.as_str());
                ui.horizontal(|ui| {
                    if ui.selectable_label(open, &name).clicked() {
                        window.open_theme(&name);
                    }
                    ui.label(egui::RichText::new(summary).weak());
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("new theme:");
                ui.add(
                    egui::TextEdit::singleline(&mut window.new_theme_name)
                        .desired_width(120.0)
                        .hint_text("its name"),
                );
                if ui.button("make it").clicked() {
                    let name = std::mem::take(&mut window.new_theme_name);
                    window.new_theme(&name);
                }
            });
            if let Some(problem) = window.theme_problem.clone() {
                ui.colored_label(egui::Color32::LIGHT_RED, problem);
            }
        });

        cell(&mut panes[1], "theme-fields", None, |ui| {
            let Some(name) = window.theme_editing.clone() else {
                ui.label("Choose a theme on the left to see what it asks for.");
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "A theme is a file in themes/. An applet wears one by naming it in the Applets tab, \
                         and any widget there can be told what to do when it moves.",
                    )
                    .weak(),
                );
                return;
            };
            ui.label(format!("{name}.json"));
            ui.separator();
            // The numbers a theme starts with are shown rather than zeroes, so a field never lies about what is
            // in force; and a zero writes the key away instead of writing a zero, because absent means "off".
            let scanline = window.theme_number("scanline");
            theme_number_row(ui, window, "scanline, pixels a second:", "scanline", scanline, 0.0, 400.0);
            let every = window.theme_inner_number("glitch", "every");
            theme_inner_row(ui, window, "glitch every, seconds:", ("glitch", "every"), every, 0.0, 60.0);
            let shake = window.theme_inner_number_or("glitch", "shake", 1.0);
            theme_inner_row(ui, window, "glitch shake, pixels:", ("glitch", "shake"), shake, 0.0, 20.0);
            let on = window.theme_inner_number_or("pulse", "on", 1.0);
            theme_inner_row(ui, window, "pulse on, seconds:", ("pulse", "on"), on, 0.0, 60.0);
            let off = window.theme_inner_number_or("pulse", "off", 1.0);
            theme_inner_row(ui, window, "pulse off, seconds:", ("pulse", "off"), off, 0.0, 60.0);
            let typing = window.theme_inner_number_or("typewriter", "seconds", 12.0);
            theme_number_row(
                ui,
                window,
                "typewriter, characters a second:",
                "typewriter",
                typing,
                0.0,
                200.0,
            );
            ui.horizontal(|ui| {
                ui.label("frame:");
                let held = window.theme_word("frame");
                let mut chosen = held.clone();
                let shown = |word: &str| match word.is_empty() {
                    true => "none".to_string(),
                    false => word.to_string(),
                };
                egui::ComboBox::from_id_salt("theme-frame")
                    .selected_text(shown(&held))
                    .show_ui(ui, |ui| {
                        for option in ["none", "single", "brackets"] {
                            ui.selectable_value(&mut chosen, option.to_string(), option);
                        }
                    });
                if chosen != held {
                    match chosen.as_str() {
                        "none" => window.set_theme("frame", serde_json::Value::Null),
                        other => window.set_theme("frame", serde_json::json!(other)),
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "written to {}, which the driver re-reads as it draws, so a change here is on the pad at once",
                    window.themes_dir().join(format!("{name}.json")).display()
                ))
                .weak(),
            );
        });
    });
}

/// One of a theme's own numbers. Zero means the effect is not in the file at all, which is what "off" is.
fn theme_number_row(
    ui: &mut egui::Ui,
    window: &mut Window,
    label: &str,
    key: &str,
    held: f64,
    low: f64,
    high: f64,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut value = held;
        if ui
            .add(
                egui::DragValue::new(&mut value)
                    .range(low..=high)
                    .speed(0.5),
            )
            .changed()
        {
            match value <= 0.0 {
                true => window.set_theme(key, serde_json::Value::Null),
                false => window.set_theme(key, serde_json::json!(value)),
            }
        }
    });
}

/// The same, one level down, for `glitch` and `pulse`: their settings live in an object of their own.
fn theme_inner_row(
    ui: &mut egui::Ui,
    window: &mut Window,
    label: &str,
    field: (&str, &str),
    held: f64,
    low: f64,
    high: f64,
) {
    let (key, inner) = field;
    ui.horizontal(|ui| {
        ui.label(label);
        let mut value = held;
        if ui
            .add(
                egui::DragValue::new(&mut value)
                    .range(low..=high)
                    .speed(0.5),
            )
            .changed()
        {
            match value <= 0.0 {
                true => window.set_theme_inner(key, inner, serde_json::Value::Null),
                false => window.set_theme_inner(key, inner, serde_json::json!(value)),
            }
        }
    });
}

/// The pad's screen as it will look, pixel for pixel, from the same renderer the driver uses.
///
/// One function, shared by every surface that draws a screen: a second renderer would
/// eventually disagree with the pad, and the pad is the thing that matters.
fn preview_pixels(ui: &mut egui::Ui, window: &Window) {
    let scale = 2.5_f32;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(
            g13_screen::WIDTH as f32 * scale,
            g13_screen::VISIBLE_HEIGHT as f32 * scale,
        ),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
    for y in 0..g13_screen::VISIBLE_HEIGHT {
        for x in 0..g13_screen::WIDTH {
            if window.preview.get(x, y) {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        rect.min + egui::vec2(x as f32 * scale, y as f32 * scale),
                        egui::vec2(scale, scale),
                    ),
                    0.0,
                    egui::Color32::from_rgb(0x8d, 0xd0, 0xff),
                );
            }
        }
    }
}

/// What a number in a widget is measured in, for the field grid's last column.
///
/// A number with no unit is a number nobody can check: `w: 100` on a 160-wide screen is most of it, and
/// `max: 100` on a bar is a percentage. Said in one place, so the same field says the same thing everywhere.
fn field_unit(key: &str) -> &'static str {
    match key {
        "x" | "y" => "px",
        "w" | "len" => "px wide",
        "h" => "px high",
        "r" => "px radius",
        "rows" => "rows",
        "count" => "blocks",
        "thick" => "px",
        "max" => "the bar's full width",
        "scroll_width" => "characters",
        "interval" => "seconds",
        "screen" => "1 is the first",
        _ => "",
    }
}

/// What this widget's `command` can say, offered rather than remembered.
///
/// `screen:` is the one worth a button: the screens are a list the applet already has, so picking one writes a
/// target that exists. `cmd:` is a prefix to finish by hand, because a shell command is not something to guess at.
fn command_offers(ui: &mut egui::Ui, window: &mut Window, row: &WidgetRow) {
    let titles = window.applet_screen_titles();
    if titles.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("runs:");
        for title in titles {
            if ui.button(format!("go to {title}")).clicked() {
                window.set_widget_text(row.index, "command", &format!("screen:{title}"));
            }
        }
    });
}

/// The alert on one widget: what it does, and the question it asks.
///
/// The question is edited with the macros tab's own rows - `condition_rows`, given this applet's sources as the
/// names to pick from - so "when is this true" is written one way in this program whichever tab it is written on.
fn alert_editor(ui: &mut egui::Ui, window: &mut Window, row: &WidgetRow) {
    ui.add_space(4.0);
    let held = window.widget_alert_look(row.index);
    ui.horizontal(|ui| {
        ui.label("alert:");
        let mut chosen = held.clone();
        egui::ComboBox::from_id_salt(format!("alert-{}", row.index))
            .selected_text(match held.is_empty() {
                true => "none".to_string(),
                false => held.clone(),
            })
            .width(90.0)
            .show_ui(ui, |ui| {
                for option in ["none", "flash", "invert", "show", "border"] {
                    ui.selectable_value(&mut chosen, option.to_string(), option);
                }
            });
        if chosen != held {
            window.set_widget_alert_look(
                row.index,
                match chosen.as_str() {
                    "none" => "",
                    other => other,
                },
            );
        }
        // `show` needs nothing else: it is this widget's own drawing, shown only while its question is true
    });
    if held.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        match window.widget_alert_question(row.index) {
            Some(form) => {
                ui.label(format!("when {}", form.describe()));
            }
            None => {
                ui.label("no question yet:");
            }
        }
        let editing = window.alert_editing == Some(row.index);
        if ui.selectable_label(editing, "write the question").clicked() {
            // a question to start from, so opening the rows never means an empty one: the first thing this applet
            // reads, compared with something, is a working question that can be changed rather than a blank to invent
            if !editing && window.widget_alert_question(row.index).is_none() {
                // a question to start from, made in one place so the window cannot offer one thing and the test
                // another: the first thing this applet reads, compared with something
                let first = window.new_alert_question();
                window.set_widget_alert_question(row.index, &first);
            }
            window.alert_editing = match editing {
                true => None,
                false => Some(row.index),
            };
        }
    });
    if window.alert_editing != Some(row.index) {
        return;
    }
    let names = crate::macros::Names::for_applet(&window.applet_source_names());
    if let Some(form) = window.widget_alert_question(row.index) {
        let id = format!("alert-{}", row.index);
        if let Some(edited) = condition_rows(ui, &id, &form, &names) {
            window.set_widget_alert_question(row.index, &edited);
        }
    }
}

/// The picture a `bitmap` widget draws, as a grid you click.
///
/// Rows of dots and hashes are not something to type into a file. A picture is drawn,
/// not spelled - so this is the drawing: a cell per pixel, lit while you click, written as you go.
fn bitmap_editor(ui: &mut egui::Ui, window: &mut Window, row: &WidgetRow) {
    let named = row
        .fields
        .iter()
        .find(|field| field.key == "name")
        .map(|field| field.value.text().trim().to_string())
        .unwrap_or_default();
    ui.add_space(4.0);
    if named.is_empty() {
        ui.weak("give it a name above and the picture to draw appears here");
        return;
    }
    window.refresh_bitmap(&named);
    let rows = window
        .bitmap_frames
        .get(window.bitmap_frame)
        .cloned()
        .unwrap_or_default();
    let h = rows.len();
    let w = rows.first().map(Vec::len).unwrap_or(0);
    let frames = window.bitmap_frames.len();
    let frame = window.bitmap_frame;

    // The frames of an animation, and which one the pixels below belong to. A still bitmap has one, and this
    // strip is then one button wide - which is what it always looked like.
    {
        ui.horizontal(|ui| {
            ui.label("frame");
            for at in 0..frames {
                let here = at == frame;
                if ui.selectable_label(here, format!("{}", at + 1)).clicked() {
                    window.set_bitmap_frame(at);
                }
            }
            if ui.button("+ frame").clicked() {
                window.add_bitmap_frame();
            }
            if frames > 1 && ui.button("remove frame").clicked() {
                window.remove_bitmap_frame();
            }
            if frames > 1 {
                let mut ms = window.bitmap_ms;
                if ui
                    .add(
                        egui::DragValue::new(&mut ms)
                            .range(20.0..=2000.0)
                            .suffix(" ms a frame"),
                    )
                    .changed()
                {
                    window.set_bitmap_ms(ms);
                }
                // at twenty drawings a second the pad cannot show more than that, and saying so here saves the
                // question of why a 30ms sprite looks the same as a 50ms one
                ui.weak(format!(
                    "{frames} frames, {:.0} a second",
                    1000.0 / window.bitmap_ms
                ));
            }
        });
    }

    ui.horizontal(|ui| {
        ui.label("draw");
        ui.label(format!("{named}, {w} by {h}"));
        let mut width = w;
        let mut height = h;
        if ui
            .add(egui::DragValue::new(&mut width).range(1..=32).prefix("w "))
            .changed()
        {
            window.set_bitmap_size(width, h);
        }
        if ui
            .add(egui::DragValue::new(&mut height).range(1..=32).prefix("h "))
            .changed()
        {
            window.set_bitmap_size(w, height);
        }
        if ui.button("clear").clicked() {
            for y in 0..h {
                for x in 0..w {
                    window.set_bitmap_pixel(x, y, false);
                }
            }
        }
    });
    if let Some(problem) = window.bitmap_problem.clone() {
        ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), problem);
    }

    // One cell per pixel. A lit one is filled in, so the drawing looks like what the pad will draw.
    let cell = 14.0;
    for y in 0..h {
        ui.horizontal(|ui| {
            for x in 0..w {
                let lit = rows
                    .get(y)
                    .and_then(|row| row.get(x))
                    .copied()
                    .unwrap_or(false);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(cell, cell), egui::Sense::click());
                let painter = ui.painter_at(rect);
                let colour = if lit {
                    egui::Color32::from_rgb(0xe8, 0xe8, 0xe8)
                } else {
                    egui::Color32::from_rgb(0x2a, 0x2a, 0x2a)
                };
                painter.rect_filled(rect, 0.0, colour);
                if response.clicked() {
                    window.set_bitmap_pixel(x, y, !lit);
                }
            }
        });
    }
    ui.weak("click a cell to light it. Written as you click, into this applet's own bitmaps file.");
}

/// The inspector for one widget: what it is, what it reads, and its fields.
///
/// The window's field grid: the label in a fixed column, the control in a column after it, and what the number
/// is measured in last. Two of these fields are *names* - `source` and `format` - and a name comes from somewhere,
/// so they are pickers rather than boxes. A name typed by hand is a name that will be a typo, and a typo is
/// drawn on the pad as itself.
fn widget_inspector(ui: &mut egui::Ui, window: &mut Window, row: &WidgetRow) {
    command_offers(ui, window, row);
    ui.strong(format!("{}. {}", row.index + 1, row.kind));
    // What it reads, and where each name comes from. A name this applet declares and a name nothing provides are
    // the difference between a number and the widget's own text on the pad, and the table's reads column has no
    // room to say which is which.
    let mut declare: Option<String> = None;
    if !row.reads.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.weak("reads");
            for read in &row.reads {
                match read.origin {
                    Origin::Declared => {
                        let _ = ui.weak(format!("{} (declared above)", read.name));
                    }
                    Origin::Missing => {
                        let _ = ui.colored_label(
                            egui::Color32::from_rgb(0xff, 0x9c, 0x60),
                            format!("{} - nothing provides this.", read.name),
                        );
                        // A name this applet does not declare cannot be read, and one of the machine's own
                        // readings is declared by naming it: `"media_title": "media_title"`. That is one click
                        // here rather than something to work out from a message.
                        if ui
                            .add(egui::Button::new(format!("declare {}", read.name)).small())
                            .on_hover_text(
                                "write it into this applet's sources, reading the machine's own value of that \
                                 name",
                            )
                            .clicked()
                        {
                            declare = Some(read.name.clone());
                        }
                    }
                };
            }
        });
    }
    if let Some(name) = declare {
        window.add_applet_source(&name, &name);
    }
    if row.unknown {
        ui.colored_label(
            egui::Color32::from_rgb(0xff, 0x9c, 0x60),
            "not a widget this build draws - every key of it is left exactly as it is",
        );
        return;
    }

    egui::Grid::new(("widget-fields", row.index))
        .num_columns(3)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            for field in &row.fields {
                ui.label(field.key);
                match (&field.value, field.key) {
                    // a name this applet can already use, or one of the machine's own, or a new one
                    (FieldValue::Text(_), "source") => {
                        source_picker(ui, window, row.index, field.value.text());
                    }
                    // a format is text with names in it, so the box stays and a list of names is beside it
                    (FieldValue::Text(_), "format") => {
                        format_with_names(ui, window, row.index, field.value.text());
                    }
                    _ => widget_field_control(ui, window, row, field),
                }
                // what the number is measured in, so a range means something
                ui.weak(field_unit(field.key));
                ui.end_row();
            }
        });

    // The picture, if this is a bitmap: drawn here rather than spelled into a file.
    if row.kind == "bitmap" {
        bitmap_editor(ui, window, row);
        alert_editor(ui, window, row);
    }

    // Adding a resource without leaving the widget, which is what "type or add a resource" means.
    if window.adding_source == Some(row.index) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("new source:");
            let mut name = window.new_source_name.clone();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut name)
                        .desired_width(120.0)
                        .hint_text("gputemp"),
                )
                .changed()
            {
                window.new_source_name = name;
            }
            let mut spec = window.new_source_spec.clone();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut spec)
                        .desired_width(260.0)
                        .hint_text("cmd:cat /sys/.../temp1_input"),
                )
                .changed()
            {
                window.new_source_spec = spec;
            }
            if ui.button("add it").clicked() {
                let name = window.new_source_name.trim().to_string();
                let spec = window.new_source_spec.trim().to_string();
                // declares it and points the widget that asked, whether that is a `source` field or a name
                // inside a `format`
                window.add_source_for_widget(row.index, &name, &spec);
                window.adding_source = None;
            }
            if ui.button("cancel").clicked() {
                window.adding_source = None;
            }
        });
        ui.weak("a cmd:, file:, json: or http: spec, or one of the machine's own names");
    }
}

/// One field of a widget that is not a name: a number, a tick, a choice, or a box.
fn widget_field_control(
    ui: &mut egui::Ui,
    window: &mut Window,
    row: &WidgetRow,
    field: &state::WidgetField,
) {
    match &field.value {
        FieldValue::Number(held) => {
            let mut value = *held;
            if ui
                .add(
                    egui::DragValue::new(&mut value)
                        .range(0.0..=400.0)
                        .speed(0.5),
                )
                .changed()
            {
                window.set_widget_number(row.index, field.key, value);
            }
        }
        FieldValue::OptionalNumber(held) => {
            // left out on purpose for the fields where leaving one out means something, so the control says
            // whether it is there rather than showing a zero that is not in the file at all
            let mut present = held.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut present, "").changed() {
                    let value = if present {
                        Some(held.unwrap_or(0.0))
                    } else {
                        None
                    };
                    window.set_widget_optional_number(row.index, field.key, value);
                }
                if let Some(held) = held {
                    let mut value = *held;
                    if ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .range(0.0..=400.0)
                                .speed(0.5),
                        )
                        .changed()
                    {
                        window.set_widget_number(row.index, field.key, value);
                    }
                } else {
                    ui.weak("not set");
                }
            });
        }
        FieldValue::Text(held) => {
            let mut text = held.clone();
            if ui
                .add(egui::TextEdit::singleline(&mut text).desired_width(260.0))
                .changed()
            {
                window.set_widget_text(row.index, field.key, &text);
            }
        }
        FieldValue::OptionalText(held) => {
            let mut text = held.clone();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(300.0)
                        .hint_text("cmd:docker restart {screen_item}"),
                )
                .changed()
            {
                // emptied means the key goes, because "no command" and "a command that runs nothing" are
                // different instructions
                let written = text.trim();
                window.set_widget_optional_text(
                    row.index,
                    field.key,
                    if written.is_empty() {
                        None
                    } else {
                        Some(written)
                    },
                );
            }
        }
        FieldValue::Tick(held) => {
            let mut ticked = *held;
            if ui.checkbox(&mut ticked, "").changed() {
                window.set_widget_flag(row.index, field.key, ticked);
            }
        }
        FieldValue::Choice(held, options) => {
            let mut chosen = held.clone();
            // An empty word is shown as `none`, because that is what it means: the key is not in the file at all.
            // That is how `animate` is taken off a widget again, without hand-editing the JSON.
            let shown = |word: &str| match word.is_empty() {
                true => "none".to_string(),
                false => word.to_string(),
            };
            egui::ComboBox::from_id_salt(format!("widget-{}-{}", row.index, field.key))
                .selected_text(shown(&chosen))
                .width(80.0)
                .show_ui(ui, |ui| {
                    for option in *options {
                        ui.selectable_value(&mut chosen, option.to_string(), shown(option));
                    }
                });
            if &chosen != held {
                match chosen.is_empty() {
                    true => window.set_widget_optional_text(row.index, field.key, None),
                    false => window.set_widget_text(row.index, field.key, &chosen),
                }
            }
        }
    }
}

/// A name a widget reads: what this applet declares, then what the machine reports, then adding one.
fn source_picker(ui: &mut egui::Ui, window: &mut Window, index: usize, held: &str) {
    let declared: Vec<String> = window
        .applet_sources()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut chosen = held.to_string();
    egui::ComboBox::from_id_salt(format!("widget-source-{index}"))
        .selected_text(match held.is_empty() {
            true => "(nothing)".to_string(),
            false => held.to_string(),
        })
        .width(200.0)
        .show_ui(ui, |ui| {
            // first, not last: under thirty names of the machine's own it could not be found at all
            if ui.button("add a resource...").clicked() {
                window.adding_source = Some(index);
            }
            ui.separator();
            if declared.is_empty() {
                ui.label("this applet declares nothing yet");
            }
            for name in &declared {
                ui.selectable_value(&mut chosen, name.clone(), format!("{name} (declared here)"));
            }
            ui.separator();
            for published in g13_sources::PUBLISHED.iter() {
                if declared.iter().any(|name| name == published.name) {
                    continue;
                }
                ui.selectable_value(
                    &mut chosen,
                    published.name.to_string(),
                    format!("{} (the machine's: {})", published.name, published.meaning),
                );
            }
        });
    if chosen != held {
        // A widget's `source` is a name from *this applet's* vocabulary, so choosing one of the machine's names
        // has to declare it: `"media_title": "media_title"` is how an applet says it wants that reading. The
        // picker used to offer names that only resolve once declared, and declare nothing: picking `media_title`
        // got "nothing provides this", while an applet that declared it worked. The choice writes the declaration.
        if declared.iter().all(|name| name != &chosen) {
            window.add_applet_source(&chosen, &chosen);
        }
        window.set_widget_text(index, "source", &chosen);
    }
}

/// A format is text with names in it: the box stays, and the names this applet can use are beside it.
fn format_with_names(ui: &mut egui::Ui, window: &mut Window, index: usize, held: &str) {
    let mut text = held.to_string();
    if ui
        .add(egui::TextEdit::singleline(&mut text).desired_width(240.0))
        .changed()
    {
        window.set_widget_text(index, "format", &text);
    }
    let names: Vec<String> = window
        .applet_sources()
        .into_iter()
        .map(|(name, _)| name)
        .chain(g13_sources::PUBLISHED.iter().map(|p| p.name.to_string()))
        .collect();
    egui::ComboBox::from_id_salt(format!("widget-insert-{index}"))
        .selected_text("insert a name")
        .width(130.0)
        .show_ui(ui, |ui| {
            // the same entry the `source` menu has: this list could insert a name but never add one, and a text
            // widget has no `source` field to add it from
            if ui.button("add a resource...").clicked() {
                window.adding_source = Some(index);
            }
            ui.separator();
            for name in names {
                if ui.button(format!("{{{name}}}")).clicked() {
                    // appended rather than replacing, because a format is usually text and names together
                    window.set_widget_text(index, "format", &format!("{text}{{{name}}}"));
                    // and if this applet does not declare it, inserting it declares it: otherwise the name is
                    // drawn on the pad as itself and this button has written a fault into the file
                    if window.applet_sources().iter().all(|(key, _)| key != &name) {
                        window.add_applet_source(&name, &name);
                    }
                }
            }
        });
}

/// One cell of the applet editor: its own scroll, so nothing in one cell takes another cell's contents off the
/// screen with it.
///
/// `max_height` is for the cells in the lower section, which share what is left of the window; the upper section
/// is as tall as what it holds, because a title and a row of screens are short whatever else is on the tab.
fn cell(ui: &mut egui::Ui, id: &str, max_height: Option<f32>, add: impl FnOnce(&mut egui::Ui)) {
    // **Both** ways, not just down. A pane narrower than what it holds - a window dragged small, a sectors
    // table, a field beside its control - has to scroll rather than run over the pane next to it: overlapping
    // text is what "the columns have no logic" looks like.
    let mut area = egui::ScrollArea::both().id_salt(id);
    if let Some(height) = max_height {
        area = area.max_height(height);
    }
    area.show(ui, |ui| add(ui));
}

/// Designing an applet: its own fields, the sources it reads, and the screen as it will look.
///
/// The preview is the window's own renderer, retargeted at the applet being edited, so what is shown here is
/// what the driver draws - not a second implementation that would eventually disagree with the pad.
fn applets(ui: &mut egui::Ui, window: &mut Window) {
    // A file that was read and would land on a name you already have: what is different, then what to do.
    // Nothing is written until one of these is pressed.
    if let Some(waiting) = window.waiting_import.clone() {
        ui.group(|ui| {
            ui.label(format!(
                "importing {:?} - you already have one called that:",
                waiting.bundle.name
            ));
            if waiting.lines.is_empty() {
                ui.label("   nothing differs - the one in the file is the one you have");
            }
            for (what, mine, its) in &waiting.lines {
                ui.label(format!("   {what}: yours {mine}  ->  its {its}"));
            }
            ui.horizontal(|ui| {
                if ui
                    .button("replace mine with it")
                    .on_hover_text("writes the file's applet over yours, keeping your own fonts, endpoints and values")
                    .clicked()
                {
                    let name = waiting.bundle.name.clone();
                    window.waiting_import = None;
                    window.land_applet(waiting.bundle.clone(), &name);
                }
                if ui
                    .button("import as a copy")
                    .on_hover_text("brings it in under the next free name, leaving yours alone")
                    .clicked()
                {
                    let name = waiting.copy_name.clone();
                    window.waiting_import = None;
                    window.land_applet(waiting.bundle.clone(), &name);
                }
                if ui.button("keep mine").clicked() {
                    window.waiting_import = None;
                    window.status = "import cancelled - yours is untouched".to_string();
                }
            });
        });
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("applet:");
        for name in window.applet_names() {
            let chosen = window.applet_editing.as_deref() == Some(name.as_str());
            if ui.selectable_label(chosen, &name).clicked() {
                window.open_applet(&name);
            }
        }
        if window.applet_editing.is_some() && ui.button("stop editing").clicked() {
            window.close_applet();
        }
    });
    // Making one, beside the list of the ones there are, the way the Macros tab makes a macro. An applet's name
    // is its file's name, so this is the one place a name is typed.
    ui.horizontal(|ui| {
        ui.label("new applet:");
        let mut name = window.new_applet_name.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut name)
                    .desired_width(200.0)
                    .hint_text("what it is for"),
            )
            .changed()
        {
            window.new_applet_name = name;
        }
        if ui.button("make it").clicked() {
            window.create_applet();
        }
        if ui
            .button("import")
            .on_hover_text(
                "opens a file saved by export: an applet with its pictures, its font, its theme and \
                 the endpoints it uses",
            )
            .clicked()
        {
            window.import_applet();
        }
        ui.label(" -  written straight away, then opened here to build on");
    });
    // The auto-cycle came from the Screen tab with the tick: `cycle the enabled ones` every so many seconds. It is
    // about the screens as a whole, so it sits above them rather than on any one. Written as it is changed - the
    // Screen tab's `save` button was the only one in the program, and it is not coming with it.
    ui.horizontal(|ui| {
        let mut on = window.cycle;
        if ui
            .checkbox(&mut on, "cycle the enabled ones")
            .on_hover_text("the pad moves on by itself through the screens ticked above")
            .changed()
        {
            window.cycle = on;
            let _ = window.save_visuals();
        }
        if window.cycle {
            let mut seconds = window.cycle_seconds;
            if ui
                .add(
                    egui::DragValue::new(&mut seconds)
                        .range(1.0..=600.0)
                        .suffix("s"),
                )
                .changed()
            {
                window.cycle_seconds = seconds;
                let _ = window.save_visuals();
            }
        }
    });
    ui.separator();

    let Some(name) = window.applet_editing.clone() else {
        ui.label("Pick one above to edit it, or make a new one above that. An applet is a file in the applets");
        ui.label("folder; this edits it where it is, so the pad's screen follows as you type and nothing has to");
        ui.label("be copied anywhere. A new applet is a blank screen: add widgets, and give the ones the pad");
        ui.label("should act on a `command`.");
        return;
    };

    if let Some(problem) = window.applet_problem.clone() {
        ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), problem);
        ui.add_space(6.0);
    }

    // Section 1: what this applet is, the screens it has, and what it reads.
    ui.columns(2, |panes| {
        cell(&mut panes[0], "applet-title-and-screens", None, |ui| {
    ui.horizontal(|ui| {
        ui.label(format!("{name}.json"));
        // The tick that used to live on the Screen tab, where it was one of a list of every screen. It belongs
        // beside the applet it is about: a screen is either on the pad or it is not, and this is the applet's own
        // page. `enabled` holds what `visuals.json` holds, named the way the walk names it.
        if let Some(visual) = window.visual_name_for(&name) {
            // The Screen tab used to hold the list of screens and a `showing:` line with a save button. The list
            // became this tick, and the show-it-now is this button: the same `select_visual` the list's own click
            // ran, so it writes what the pad is on the same way a press of LR does.
            if ui
                .button("show it now")
                .on_hover_text("puts this screen on the pad now")
                .clicked()
            {
                window.select_visual(&visual);
            }

            let mut on = window.enabled.contains(&visual);
            if ui
                .checkbox(&mut on, "on the pad")
                .on_hover_text("in the LR walk and the menu; unticked it is here but the pad passes over it")
                .changed()
            {
                window.toggle_enabled(&visual);
            }
        }
        // beside the file's own name, because that is what it deletes
        if ui
            .button("delete")
            .on_hover_text("deletes this applet, and its pictures")
            .clicked()
        {
            window.remove_applet(&name);
        }
        if ui
            .button("export")
            .on_hover_text(
                "saves this applet, its pictures, its font, its theme and the endpoints it uses as \
                 one file, for a backup or another machine",
            )
            .clicked()
        {
            window.export_applet(&name);
        }
    });
            ui.horizontal(|ui| {
                ui.label("title:");
                let mut title = window.applet_field("title");
                if ui
                    .add(egui::TextEdit::singleline(&mut title).desired_width(180.0))
                    .changed()
                {
                    window.set_applet_text("title", &title);
                }
            });
            ui.horizontal(|ui| {
                ui.label("every");
                let mut interval = window.applet_number("interval");
                if ui
                    .add(
                        egui::DragValue::new(&mut interval)
                            .range(0.2..=600.0)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    window.set_applet_number("interval", interval);
                }
                let mut border = window.applet_flag("border");
                if ui.checkbox(&mut border, "border").changed() {
                    window.set_applet_flag("border", border);
                }
            });
            ui.horizontal(|ui| {
                // A program that writes this applet's data can take the screen by itself - and say which binding
                // set it wants with it. The driver watches the file behind the applet's own sources, because a
                // file being written *is* the program running.
                ui.label("follow:");
                let mut seconds = window.applet_follow_seconds();
                let profile = window.applet_follow_profile();
                let mut on = seconds > 0.0;
                if ui
                    .checkbox(&mut on, "a program feeds this")
                    .on_hover_text(
                        "while the file behind one of its sources is being written, this screen takes the \
                         pad and hands it back when the writing stops",
                    )
                    .changed()
                {
                    window.set_applet_follow(
                        if on { g13_applets::Follow::DEFAULT_SECONDS } else { 0.0 },
                        profile.clone(),
                    );
                }
                if on {
                    if ui
                        .add(
                            egui::DragValue::new(&mut seconds)
                                .range(1.0..=3600.0)
                                .suffix(" s"),
                        )
                        .on_hover_text("how long it keeps the screen after the writing stops")
                        .changed()
                    {
                        window.set_applet_follow(seconds, profile.clone());
                    }
                    ui.label("and the set:");
                    let held = window.applet_follow_profile().unwrap_or_default();
                    let sets: Vec<String> = window
                        .profiles
                        .iter()
                        .map(|number| window.profile_sets.name(*number))
                        .collect();
                    egui::ComboBox::from_id_salt("applet-follow-set")
                        .selected_text(match held.is_empty() {
                            true => "none".to_string(),
                            false => held.clone(),
                        })
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(held.is_empty(), "none").clicked() {
                                window.set_applet_follow(window.applet_follow_seconds(), None);
                            }
                            for name in &sets {
                                if ui.selectable_label(&held == name, name).clicked() {
                                    window.set_applet_follow(
                                        window.applet_follow_seconds(),
                                        Some(name.clone()),
                                    );
                                }
                            }
                        });
                }
            });
            ui.horizontal(|ui| {
                // The font this applet wears, the way it wears a theme. A font is a file in fonts/, so this lists
                // what is there; "the panel's own" is what every applet had before fonts existed.
                ui.label("font:");
                let held = window.applet_field("font");
                let names = window.fonts();
                egui::ComboBox::from_id_salt("applet-font")
                    .selected_text(match held.is_empty() {
                        true => "the panel's own".to_string(),
                        false => held.clone(),
                    })
                    .show_ui(ui, |ui| {
                        for option in std::iter::once(String::new()).chain(names.iter().cloned()) {
                            let shown = match option.is_empty() {
                                true => "the panel's own".to_string(),
                                false => option.clone(),
                            };
                            let mut picked = held.clone();
                            if ui.selectable_value(&mut picked, option.clone(), shown).clicked() {
                                match option.is_empty() {
                                    true => window.set_applet_optional_text("font", None),
                                    false => window.set_applet_text("font", &option),
                                }
                            }
                        }
                    });
                if names.is_empty() {
                    ui.weak("none yet - drop a font in ~/.config/g13/fonts/");
                }
            });
            ui.horizontal(|ui| {
                ui.label("theme:");
                let held = window.applet_field("theme");
                // what is in the folder, which is what makes a theme a file to write and drop in
                let names = window.themes();
                egui::ComboBox::from_id_salt("applet-theme")
                    .selected_text(match held.is_empty() {
                        true => "none".to_string(),
                        false => held.clone(),
                    })
                    .show_ui(ui, |ui| {
                        for option in std::iter::once(String::new()).chain(names.iter().cloned()) {
                            let shown = match option.is_empty() {
                                true => "none".to_string(),
                                false => option.clone(),
                            };
                            let mut picked = held.clone();
                            if ui.selectable_value(&mut picked, option.clone(), shown).clicked() {
                                match option.is_empty() {
                                    true => window.set_applet_optional_text("theme", None),
                                    false => window.set_applet_text("theme", &option),
                                }
                            }
                        }
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut window.new_theme_name)
                        .desired_width(90.0)
                        .hint_text("new theme"),
                );
                if ui.button("make it").clicked() {
                    let name = std::mem::take(&mut window.new_theme_name);
                    window.new_theme(&name);
                }
                if let Some(problem) = window.theme_problem.clone() {
                    ui.colored_label(egui::Color32::LIGHT_RED, problem);
                }
            });
            ui.add_space(6.0);


    ui.separator();
    // An applet can have more than one screen, and the pad's L1 moves between them. The list is here because
    // which screen the fields below are editing is the thing a person has to be able to see.
    let screens = window.screen_count();
    ui.strong("screens");
    if screens == 1 {
        ui.label(
            "this applet is one screen. add another and L1 on the pad moves between them while it is \
             showing.",
        );
    } else {
        ui.label("L1 on the pad moves to the next one while this applet is showing.");
    }
    ui.horizontal(|ui| {
        for index in 0..screens {
            let label = if screens == 1 {
                "screen 1".to_string()
            } else {
                format!("{}. {}", index + 1, window.screen_label(index))
            };
            let chosen = index == window.screen_editing;
            if ui.selectable_label(chosen, label).clicked() {
                window.select_screen(index);
            }
        }
        if ui.button(if screens == 1 { "add a screen" } else { "add another screen" }).clicked() {
            window.add_screen();
            window.status = "added a screen: the widgets moved into screen 1, so nothing was lost".to_string();
        }
        // the last screen cannot go: an applet with no screens is a shape this build does not draw
        if screens > 1 && ui.button("remove this screen").clicked() {
            let was = window.screen_editing + 1;
            window.remove_screen();
            window.status = format!("removed screen {was}");
        }
    });
    if screens > 1 {
        ui.horizontal(|ui| {
            ui.label("this screen is called");
            let mut title = window.screen_title(window.screen_editing);
            if ui
                .add(
                    egui::TextEdit::singleline(&mut title)
                        .desired_width(180.0)
                        .hint_text("containers"),
                )
                .changed()
            {
                window.set_screen_title(window.screen_editing, &title);
            }
        });
    }

    ui.add_space(8.0);
        });
        cell(&mut panes[1], "applet-sources", None, |ui| {
    ui.strong("sources this applet declares");
            ui.label("a cmd:, file: or built-in name goes here. endpoints (hosts) are on the Endpoints tab.");
            let used = window.applet_sources_used();
            let sources = window.applet_sources();
            if sources.is_empty() {
                ui.label("none. a widget's format refers to one as {name}.");
            }
            let mut remove: Option<String> = None;
            for (source, spec) in &sources {
                ui.horizontal(|ui| {
                    ui.label(source.as_str());
                    let mut spec_text = spec.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut spec_text)
                                .desired_width(260.0)
                                .hint_text("cmd:uptime   file:/proc/loadavg   cpu"),
                        )
                        .changed()
                    {
                        window.set_applet_source(source, &spec_text);
                    }
                    if used.contains(source) {
                        ui.weak("used");
                    } else {
                        // a source nothing reads is not an error, but it is worth seeing: a command or a
                        // fetch costs something on every redraw and draws nothing
                        ui.colored_label(
                            egui::Color32::from_rgb(0xff, 0x9c, 0x60),
                            "nothing reads it",
                        );
                    }
                    if ui.button("remove").clicked() {
                        remove = Some(source.clone());
                    }
                });
            }
            if let Some(source) = remove {
                window.remove_applet_source(&source);
            }
            ui.horizontal(|ui| {
                ui.label("add:");
                ui.add(
                    egui::TextEdit::singleline(&mut window.new_source_name)
                        .hint_text("name")
                        .desired_width(90.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut window.new_source_spec)
                        .hint_text("cmd:uptime  file:...  cpu")
                        .desired_width(240.0),
                );
                if ui.button("add").clicked() {
                    let (source, spec) = (
                        window.new_source_name.clone(),
                        window.new_source_spec.clone(),
                    );
                    window.add_applet_source(&source, &spec);
                    window.new_source_name.clear();
                    window.new_source_spec.clear();
                }
            });
        });
    });
    ui.separator();

    // Section 2: the widgets, and the one that is open beside them. Both cells take the height left in the
    // window, so each scrolls inside itself rather than pushing the other's contents out of sight.
    let rest = ui.available_height();
    ui.columns(2, |panes| {
        cell(&mut panes[0], "applet-widgets", Some(rest), |ui| {
    ui.separator();
    ui.strong("widgets");
    ui.label(
        "drawn in this order, on top of each other, on a screen 160 wide and 43 high. Click one to open its \
         fields; a later widget is drawn over an earlier one.",
    );

    // One column per fact, so the list is read *down* rather than across and every row lines up: the table
    // shape the window uses, worked out on this tab first.
    let rows = window.applet_widgets();
    let mut open_widget: Option<Option<usize>> = None;
    let mut remove_widget: Option<usize> = None;
    let mut move_widget: Option<(usize, bool)> = None;

    egui::Grid::new("applet-widgets")
        .num_columns(4)
        .striped(true)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            // the headers are the columns, said once
            ui.strong("#");
            ui.strong("type");
            ui.strong("what it reads");
            ui.strong("do");
            ui.end_row();

            if rows.is_empty() {
                ui.label("-");
                ui.label("none yet - add a widget below");
                ui.label("");
                ui.label("");
                ui.end_row();
            }

            for row in &rows {
                let open = window.widget_editing == Some(row.index);
                let toggle = if open { None } else { Some(row.index) };
                if ui
                    .selectable_label(open, format!("{}", row.index + 1))
                    .clicked()
                {
                    open_widget = Some(toggle);
                }
                let kind = match row.unknown {
                    true => format!("{} (not drawn)", row.kind),
                    false => row.kind.clone(),
                };
                if ui.selectable_label(open, kind).clicked() {
                    open_widget = Some(toggle);
                }
                // what it reads, with a name nothing provides named where it is, not in a sentence below
                let missing: Vec<&str> = row
                    .reads
                    .iter()
                    .filter(|read| matches!(read.origin, Origin::Missing))
                    .map(|read| read.name.as_str())
                    .collect();
                if missing.is_empty() {
                    let names: Vec<&str> =
                        row.reads.iter().map(|read| read.name.as_str()).collect();
                    ui.weak(names.join(", "));
                } else {
                    ui.colored_label(
                        egui::Color32::from_rgb(0xff, 0x9c, 0x60),
                        format!("nothing provides {}", missing.join(", ")),
                    );
                }
                // the actions are the last column, always in the same order
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(row.index > 0, egui::Button::new("^").small())
                        .on_hover_text(
                            "move this widget earlier: a later one is drawn over an earlier one",
                        )
                        .clicked()
                    {
                        move_widget = Some((row.index, true));
                    }
                    if ui
                        .add_enabled(row.index + 1 < rows.len(), egui::Button::new("v").small())
                        .on_hover_text("move this widget later")
                        .clicked()
                    {
                        move_widget = Some((row.index, false));
                    }
                    if ui.add(egui::Button::new("remove").small()).clicked() {
                        remove_widget = Some(row.index);
                    }
                });
                ui.end_row();
            }

        });

        // The add row sits under the table rather than inside it. Inside a grid cell the buttons wrap to that
        // cell's width - which is one button per line down the page, exactly the wall of controls this layout is
        // getting away from. Out here they wrap across the pane.
        let mut add_now: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("add");
            for kind in WIDGET_KINDS {
                if ui
                    .add(egui::Button::new(kind).small())
                    .on_hover_text("one widget type this build draws")
                    .clicked()
                {
                    add_now = Some(kind.to_string());
                }
            }
        });
        if let Some(kind) = add_now.as_deref() {
            window.add_widget(kind);
        }

    // The selected widget's fields are the inspector, in the right-hand pane - the shell every tab shares: a
    // list on the left, the selected thing on the right. A column of fields under the table is the wall this
    // replaces.
    if let Some(open) = open_widget {
        window.widget_editing = open;
    }
    if let Some((index, up)) = move_widget {
        window.move_widget(index, up);
    }
    if let Some(index) = remove_widget {
        window.remove_widget(index);
    }
        });
        cell(
            &mut panes[1],
            "applet-preview-and-inspector",
            Some(rest),
            |ui| {
            ui.label("the pad's screen, as it will look");

    // What the pad does with the ones that are ticked. One line, because the render tests harvest a capped amount
    // of this tab's text and the applet's own fields have to fit inside it.
    ui.label("LR shows the next ticked one. L1-L4 are for screens inside an applet, which are not built yet.");
    // A name that is ticked and cannot be drawn is stepped over, which is right and invisible: the driver says it
    // once at startup and this is where the list is, so this is where the fix is.
    let drawable = g13_agent::visuals::drawable_visuals(&window.config_dir.join("applets"));
    for name in window
        .enabled
        .iter()
        .filter(|name| !drawable.contains(name))
    {
        ui.colored_label(
            egui::Color32::from_rgb(0xff, 0x9c, 0x60),
            format!("{name} is ticked and is not a screen this build can draw, so the rotation steps over it"),
        );
    }

            preview_pixels(ui, window);

            // The selected widget's fields, in this pane beside the preview: the list says what there is,
            // this says what the chosen one is.
            if let Some(open) = window.widget_editing {
                if let Some(row) = window
                    .applet_widgets()
                    .into_iter()
                    .find(|row| row.index == open)
                {
                    ui.add_space(8.0);
                    ui.separator();
                    widget_inspector(ui, window, &row);
                }
            }
            },
        );
    });
}

/// The Menu tab: the menu the pad's own button opens, in menu.json.
fn menu_tab(ui: &mut egui::Ui, window: &mut Window) {
    // The screens themselves are on the Applets tab now - each one's tick, its `show it now`, and the cycle - so
    // what is left here is the one thing that was never about a screen's contents: the menu the pad's own button
    // opens, which is a file of its own.
    ui.label("what holding LR opens. With no menu.json of your own this is the rotation of the ticked screens.");
    ui.add_space(6.0);
    // The menu the pad's own button opens. Until a file of your own is written this is the rotation; write one
    // and it is yours, with submenus and an action per item.
    window.refresh_menu();
    ui.strong("the menu (hold LR)");
    let file_there = window.menu_path().exists();
    if file_there {
        ui.label("From menu.json. Anything you put here replaces the rotation above.");
    } else {
        ui.label(
            "No menu.json yet, so the menu is the ticked screens above. Add an item to start a menu of your own.",
        );
    }
    if let Some(problem) = window.menu_problem.clone() {
        ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), problem);
    }

    let rows = window.menu_rows();
    let mut remove: Option<(usize, Option<usize>)> = None;
    let mut move_it: Option<((usize, Option<usize>), bool)> = None;
    let mut add_child: Option<usize> = None;
    for row in &rows {
        ui.horizontal(|ui| {
            // children are indented, because a menu is a shape and the file is flat
            ui.label(if row.at.1.is_some() { "under" } else { "item " });
            let mut label = row.label.clone();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut label)
                        .desired_width(150.0)
                        .hint_text("what it says"),
                )
                .changed()
            {
                window.set_menu_label(row.at, &label);
            }
            let mut does = row.does;
            egui::ComboBox::from_id_salt(format!("menu-does-{:?}", row.at))
                .selected_text(does.words())
                .width(130.0)
                .show_ui(ui, |ui| {
                    for option in [MenuDoes::Screen, MenuDoes::Command, MenuDoes::List] {
                        ui.selectable_value(&mut does, option, option.words());
                    }
                });
            if does != row.does {
                window.set_menu_does(row.at, does);
            }
            match row.does {
                MenuDoes::List => {
                    if ui.button("+ item under it").clicked() {
                        add_child = Some(row.at.0);
                    }
                }
                MenuDoes::Screen => {
                    let mut value = row.value.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut value)
                                .desired_width(160.0)
                                .hint_text("clock, applet:docker"),
                        )
                        .changed()
                    {
                        window.set_menu_value(row.at, &value);
                    }
                    let mut screen = row.screen.unwrap_or(1);
                    if ui
                        .add(
                            egui::DragValue::new(&mut screen)
                                .range(1..=16)
                                .prefix("screen "),
                        )
                        .changed()
                    {
                        window.set_menu_screen(row.at, Some(screen));
                    }
                }
                MenuDoes::Command => {
                    let mut value = row.value.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut value)
                                .desired_width(300.0)
                                .hint_text("cmd:playerctl play-pause"),
                        )
                        .changed()
                    {
                        window.set_menu_value(row.at, &value);
                    }
                }
            }
            if ui.button("^").clicked() {
                move_it = Some((row.at, true));
            }
            if ui.button("v").clicked() {
                move_it = Some((row.at, false));
            }
            if ui.button("remove").clicked() {
                remove = Some(row.at);
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label("new item:");
        let mut label = window.new_menu_label.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut label)
                    .desired_width(150.0)
                    .hint_text("what it says"),
            )
            .changed()
        {
            window.new_menu_label = label;
        }
        if ui.button("add it").clicked() {
            window.add_menu_item();
        }
        if !rows.is_empty() && ui.button("remove the whole menu").clicked() {
            window.menu_json = Some(serde_json::json!({"items": []}));
            window.commit_menu();
            let path = window.menu_path();
            let _ = std::fs::remove_file(path);
        }
    });
    if let Some(top) = add_child {
        window.add_menu_child(top);
    }
    if let Some((at, up)) = move_it {
        window.move_menu_item(at, up);
    }
    if let Some(at) = remove {
        window.remove_menu_item(at);
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    /// Every piece of text that would reach the screen, in the order it was drawn.
    ///
    /// The shapes are walked rather than the calls inspected: a label that is constructed and never added to
    /// a layout draws nothing, and that is exactly the kind of gap this is here to catch.
    fn drawn_text(output: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::epaint::Shape, into: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(text) => into.push(text.galley.text().to_string()),
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }
        let mut into = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut into);
        }
        into
    }

    fn draw_controls(window: &mut Window) -> Vec<String> {
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 1000.0),
            )),
            ..Default::default()
        };
        // Twice: the first frame is where egui builds its font atlas, and text drawn before that has no
        // glyphs behind it. Each frame's texture deltas are cleared before the output is dropped, which
        // egui insists on - a delta nobody applied is a texture the caller would have leaked.
        let mut warm_up = context.run_ui(input.clone(), |ui| controls(ui, window));
        let text = drawn_text(&warm_up);
        warm_up.textures_delta.clear();
        let _ = text;
        let mut output = context.run_ui(input, |ui| controls(ui, window));
        let drawn = drawn_text(&output);
        output.textures_delta.clear();
        drawn
    }

    /// Draw the Menu tab the way the window would, so what it says can be asserted.
    fn draw_menu_tab(window: &mut Window) -> Vec<String> {
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 1000.0),
            )),
            ..Default::default()
        };
        let mut warm_up = context.run_ui(input.clone(), |ui| menu_tab(ui, window));
        warm_up.textures_delta.clear();
        let mut output = context.run_ui(input, |ui| menu_tab(ui, window));
        let drawn = drawn_text(&output);
        output.textures_delta.clear();
        drawn
    }

    #[test]
    fn the_menu_tab_shows_the_menu_and_nothing_that_moved_to_the_applets_tab() {
        let dir = std::env::temp_dir().join(format!("g13-gui-render-menu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(dir.join("applets").join("gpu.json"), "{}").unwrap();
        std::fs::write(
            dir.join("visuals.json"),
            r#"{"enabled": ["clock", "custom", "applet:gpu"], "active": "applet:gpu", "cycle": false, "cycle_seconds": 10}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        let drawn = draw_menu_tab(&mut window);
        let text = drawn.join(" | ");
        assert!(
            text.contains("what holding LR opens"),
            "the Menu tab does not say what it is: {text}"
        );
        // the menu editor is here, and this is the one string in the menu block the capped harvest reaches
        assert!(
            text.contains("what it says"),
            "the menu editor is not on the Menu tab: {text}"
        );
        // and what moved to the Applets tab is not still here: two switches for one setting is how they drift
        for moved in [
            "showing:",
            "cycle the enabled ones",
            "The round button (LR) shows",
        ] {
            assert!(
                !text.contains(moved),
                "`{moved}` is on the Menu tab as well as the Applets tab: {text}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Render the Sources tab the way the window would, so what it draws can be asserted.
    fn draw_sources(window: &mut Window) -> Vec<String> {
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 1000.0),
            )),
            ..Default::default()
        };
        let mut warm_up = context.run_ui(input.clone(), |ui| sources(ui, window));
        warm_up.textures_delta.clear();
        let mut output = context.run_ui(input, |ui| sources(ui, window));
        let drawn = drawn_text(&output);
        output.textures_delta.clear();
        drawn
    }

    /// Render the Applets tab the way the window would.
    fn draw_applets(window: &mut Window) -> Vec<String> {
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 2400.0),
            )),
            ..Default::default()
        };
        let mut warm_up = context.run_ui(input.clone(), |ui| applets(ui, window));
        warm_up.textures_delta.clear();
        let mut output = context.run_ui(input, |ui| applets(ui, window));
        let drawn = drawn_text(&output);
        output.textures_delta.clear();
        drawn
    }
    fn draw_values(window: &mut Window) -> Vec<String> {
        let context = egui::Context::default();
        // Wide enough for the whole five-column table whatever the live values say: this page reads the
        // driver's own file, and one long value was enough to push the last column out of the drawing and
        // fail a test that is about the page rather than about the data.
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(3600.0, 2400.0),
            )),
            ..Default::default()
        };
        let mut warm_up = context.run_ui(input.clone(), |ui| values(ui, window));
        warm_up.textures_delta.clear();
        let mut output = context.run_ui(input, |ui| values(ui, window));
        let drawn = drawn_text(&output);
        output.textures_delta.clear();
        drawn
    }

    fn draw_macros(window: &mut Window) -> Vec<String> {
        let context = egui::Context::default();
        // wide enough for both panes whatever the rows say: this page reads the macros on the machine
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(2600.0, 2400.0),
            )),
            ..Default::default()
        };
        let mut warm_up = context.run_ui(input.clone(), |ui| macros(ui, window));
        warm_up.textures_delta.clear();
        let mut output = context.run_ui(input, |ui| macros(ui, window));
        let drawn = drawn_text(&output);
        output.textures_delta.clear();
        drawn
    }

    /// The Macros tab: the list, the steps of an open macro, and the nodes of one that decides things.
    /// The list must not be drowned by the empty slots the previous stack left behind.
    #[test]
    fn the_list_hides_the_empty_slots_until_they_are_asked_for() {
        let dir = std::env::temp_dir().join(format!("g13-gui-empty-macros-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // a real installation has one of these for every id the previous stack could use
        for id in 0..190 {
            std::fs::write(
                dir.join(format!("macro-{id}.properties")),
                format!("#predecessor\nid={id}\nsequence=\nname=\n"),
            )
            .unwrap();
        }
        // and a handful that do something
        std::fs::write(
            dir.join("macro-200.properties"),
            "name=mine\nid=200\nsequence=kd.20,d.40,ku.20,d.100\n",
        )
        .unwrap();

        let mut window = Window::load(&dir);
        assert_eq!(window.empty_macro_count(), 190);
        let drawn = draw_macros(&mut window);
        eprintln!("the list, with 190 empty slots in the configuration:");
        for line in &drawn {
            eprintln!("  {line}");
        }
        let text = drawn.join(" | ");
        assert!(
            text.contains("200 mine"),
            "the macro that does something is missing: {text}"
        );
        assert!(
            !text.contains("0 (empty)"),
            "the empty slots are in the list anyway: {text}"
        );
        assert!(
            text.contains("show the 190 empty one(s)"),
            "there is no way to see the empty ones, and no sign they exist: {text}"
        );
        // the list is the handful, not the two hundred
        let entries = drawn.iter().filter(|line| line.contains("(empty)")).count();
        assert_eq!(entries, 0);

        // asking for them shows them
        window.show_empty_macros = true;
        let drawn = draw_macros(&mut window);
        assert!(
            drawn.iter().any(|line| line.contains("0 (empty)")),
            "asking for the empty ones did not show them"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Macros tab, through the window's own tab bar.
    ///
    /// Called through `App::draw` rather than by naming the function, so the assertion is that the tab is
    /// *reachable* - the bug this guards against is a tab that exists and cannot be opened.
    #[test]
    fn the_macros_tab_is_reachable_and_edits_the_macro_it_shows() {
        let dir =
            std::env::temp_dir().join(format!("g13-gui-window-macros-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("macro-7.properties"),
            "name=opened through the window\nid=7\nsequence=kd.20,d.40,ku.20,d.100\n",
        )
        .unwrap();

        let mut app = App::new(Window::load(&dir));
        app.window.tab = Tab::Macros;
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(2600.0, 2400.0),
            )),
            ..Default::default()
        };
        let mut warm_up = context.run_ui(input.clone(), |ui| app.draw(ui));
        warm_up.textures_delta.clear();
        let output = context.run_ui(input, |ui| app.draw(ui));
        let drawn = drawn_text(&output);
        eprintln!("the whole window, on the Macros tab, draws:");
        for line in &drawn {
            eprintln!("  {line}");
        }
        assert!(
            drawn.iter().any(|line| line.trim() == "Macros"),
            "the Macros tab is not in the tab bar: {drawn:?}"
        );
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("7 opened through the window")),
            "the tab did not draw what is there: {drawn:?}"
        );

        // and opening it through the window writes that macro's file when it is edited
        app.window.open_macro(7);
        app.window.add_macro_step("wait");
        let written = std::fs::read_to_string(dir.join("macro-7.properties")).unwrap();
        assert!(written.contains("d.50"), "{written}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_macros_tab_shows_them_and_edits_where_they_live() {
        let dir =
            std::env::temp_dir().join(format!("g13-gui-render-macros-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("macro-1.properties"),
            "name=plain\nid=1\nsequence=kd.20,d.40,ku.20,d.100\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("macro-2.properties"),
            "name=decides\nid=2\nsequence=kd.194,d.20,ku.194,d.100\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("macro-2.nodes.json"),
            r#"{"start": "ask",
                "sources": {"cpu_temp": "cmd:sensors"},
                "nodes": [
                    {"id": "ask", "kind": "if", "when": {"control": "G7"}, "x": 40, "y": 30},
                    {"id": "again", "kind": "repeat", "times": 3, "x": 40, "y": 100},
                    {"id": "tap", "kind": "key", "code": 194, "hold": 20, "mode": "tap", "x": 40, "y": 170},
                    {"id": "tell", "kind": "run", "spec": "cmd:notify-send done", "x": 40, "y": 240}
                ],
                "edges": [
                    {"from": "ask", "port": "then", "to": "again"},
                    {"from": "again", "port": "body", "to": "tap"},
                    {"from": "tap", "to": "again"},
                    {"from": "again", "port": "then", "to": "tell"}
                ]}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        let listed = draw_macros(&mut window);
        eprintln!("the Macros tab lists:");
        for line in &listed {
            eprintln!("  {line}");
        }
        assert!(
            listed.iter().any(|line| line.contains("1 plain")),
            "the first macro is not in the list: {listed:?}"
        );
        assert!(
            listed.iter().any(|line| line.contains("2 decides")),
            "the second macro is not in the list: {listed:?}"
        );

        // the plain one: its steps, and the way to give it a decision
        window.open_macro(1);
        let drawn = draw_macros(&mut window);
        let text = drawn.join(" | ");
        assert!(text.contains("macro-1.properties"), "{text}");
        assert!(text.contains("2."), "the steps are not numbered: {text}");
        assert!(
            text.contains("make it decide things"),
            "there is no way to give a plain macro an if: {text}"
        );

        // and the one that decides something: its boxes, each saying what it does
        window.open_macro(2);
        let drawn = draw_macros(&mut window);
        eprintln!("the Macros tab draws, for a macro with a graph:");
        for line in &drawn {
            eprintln!("  {line}");
        }
        let text = drawn.join(" | ");
        assert!(text.contains("macro-2.nodes.json"), "{text}");
        assert!(text.contains("ask"), "the boxes are not drawn: {text}");
        assert!(
            text.contains("If fired by G7"),
            "a box does not say what it does: {text}"
        );
        assert!(
            text.contains("Repeat 3 times"),
            "the repeat box is not drawn: {text}"
        );
        assert!(
            text.contains("Press key 194"),
            "the key box is not drawn: {text}"
        );
        // an output box says what it will do, and the macro says where its questions can look - without any
        // box being selected, because the sources belong to the macro and not to a box
        assert!(
            text.contains("Run notify-send done"),
            "the run box is not drawn: {text}"
        );
        assert!(
            text.contains("what this macro reads"),
            "the sources are not shown unless a box is selected: {text}"
        );
        assert!(
            text.contains("cpu_temp"),
            "the macro's own source is not listed: {text}"
        );
        assert!(
            text.contains("add this source"),
            "there is no way to add a source: {text}"
        );
        // the flow says where it starts, and the picture explains itself: the names and dots were the first
        // things a first-time reader has to have explained.
        assert!(
            text.contains("when this macro runs"),
            "the flow does not say where it starts: {text}"
        );
        assert!(
            text.contains("click a box to edit what it does"),
            "the picture does not explain itself: {text}"
        );

        assert!(
            text.contains("play it now"),
            "there is no way to play it: {text}"
        );

        // clicking a box shows its fields: the question, and its ways out
        window.macro_selected = Some("ask".to_string());
        let drawn = draw_macros(&mut window);
        let text = drawn.join(" | ");
        assert!(
            text.contains("fired by"),
            "the question has no fields: {text}"
        );
        assert!(text.contains("now: fired by G7"), "{text}");
        assert!(
            text.contains("then (if yes"),
            "a port does not say what it means: {text}"
        );
        // clicking a run box shows what it does, and where that is written
        window.macro_selected = Some("tell".to_string());
        let drawn = draw_macros(&mut window);
        let text = drawn.join(" | ");
        assert!(text.contains("do this:"), "{text}");
        assert!(
            text.contains("cmd:notify-send done"),
            "the run box cannot be edited: {text}"
        );
        assert!(text.contains("->"), "the ways out are not listed: {text}");
        assert!(
            text.contains("(nothing)"),
            "a port with no line is not shown as nothing: {text}"
        );
        window.macro_selected = None;

        // editing writes the file it is editing: a step added through the window is in that macro's file,
        // and the file it wrote reads back as a macro
        window.open_macro(1);
        window.add_macro_step("wait");
        let written = std::fs::read_to_string(dir.join("macro-1.properties")).unwrap();
        assert!(
            written.contains("d.50"),
            "a step added in the window is not in the file: {written}"
        );
        let read_back = g13_config::parse_macro(&written);
        // the three it had, and the wait that was added
        assert_eq!(read_back.steps.len(), 5, "{written}");
        assert_eq!(read_back.name, "plain", "{written}");

        // and the macro it was not editing is untouched
        let other = std::fs::read_to_string(dir.join("macro-2.properties")).unwrap();
        assert!(
            other.contains("d.100") && !other.contains("d.50"),
            "{other}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_values_page_lists_what_an_applet_can_use() {
        let dir = std::env::temp_dir().join("g13-gui-render-values");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("applets/mine.json"),
            r#"{"name":"mine","title":"MINE","interval":1,"border":true,
                "sources":{"cpu":"cpu","gpu":"cmd:echo 22"},
                "widgets":[{"type":"text","x":3,"y":2,"format":"{cpu:.0f}% {host}"}]}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        let drawn = draw_values(&mut window);
        eprintln!("the Values tab draws:");
        for line in &drawn {
            eprintln!("  {line}");
        }
        // the table says a value's meaning and which half answers for it
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("how busy the processor is")),
            "a value's meaning is not shown: {drawn:?}"
        );
        // which half of the system answers for each one
        assert!(
            drawn.iter().any(|line| line.trim() == "the machine")
                && drawn.iter().any(|line| line.trim() == "the driver"),
            "where a value comes from is not shown: {drawn:?}"
        );
        // The detail is the right-hand column and belongs to the chosen value: what it reads now, who uses it,
        // and the line to paste into an applet.
        window.value_selected = Some("cpu".to_string());
        let drawn = draw_values(&mut window);
        assert!(
            drawn.iter().any(|line| line.contains("\"cpu\": \"cpu\"")),
            "the detail does not show how to read it: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| line.trim() == "mine"),
            "the applet that reads it is not named in its detail: {drawn:?}"
        );
        // and a value nothing reads says so rather than looking important
        // `memory` is in the catalogue and the fixture reads only `cpu` and `host`
        window.value_selected = Some("memory".to_string());
        let drawn = draw_values(&mut window);
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("no applet or macro reads it yet")),
            "an unread value is not marked: {drawn:?}"
        );
        // every name in the catalogue is offered
        for name in g13_sources::published_names() {
            assert!(
                drawn.iter().any(|line| line.trim() == name),
                "{name} is in the catalogue and not on the page: {drawn:?}"
            );
        }
    }

    #[test]
    fn the_bindings_tab_carries_the_screen_colour() {
        // a profile that names one says what it is; one that names none says so rather than showing a swatch
        let dir = std::env::temp_dir().join("g13-gui-render-colour");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("bindings-0.properties"),
            "G1=p,k.30\ncolor=255,0,0\n",
        )
        .unwrap();
        std::fs::write(dir.join("bindings-1.properties"), "G1=p,k.31\n").unwrap();
        std::fs::write(dir.join("active-profile"), "0\n").unwrap();

        let mut window = Window::load(&dir);
        window.profile = 0;
        let drawn = draw_bindings(&mut window);
        assert!(
            drawn.iter().any(|line| line.contains("screen colour")),
            "the colour is not on the tab: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| line.trim() == "255,0,0"),
            "the colour it holds is not shown: {drawn:?}"
        );

        window.profile = 1;
        let drawn = draw_bindings(&mut window);
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("keeps whatever it was last told")),
            "a profile with no colour does not say so: {drawn:?}"
        );
    }

    #[test]
    fn the_applets_tab_draws_the_applet_its_sources_and_the_screen() {
        let dir = std::env::temp_dir().join("g13-gui-render-applets");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("applets/temps.json"),
            r#"{"name":"temps","title":"TEMPS","interval":2,"border":true,
                "sources":{"cpu":"cmd:echo 11","spare":"cmd:uptime"},
                "widgets":[{"type":"text","x":3,"y":12,"format":"cpu {cpu}C"},
                    {"type":"text","x":3,"y":20,"format":"{gpu}%"}]}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        window.open_applet("temps");
        let drawn = draw_applets(&mut window);
        eprintln!("the Applets tab draws:");
        for line in &drawn {
            eprintln!("  {line}");
        }
        // which applets can be edited, and which one is open
        assert!(
            drawn.iter().any(|line| line.trim() == "temps"),
            "the applet is not offered: {drawn:?}"
        );
        assert!(drawn.iter().any(|line| line.contains("temps.json")));
        // and the order the tab draws them in is the layout: title and screens in the upper left, the sources
        // in the upper right, the widgets and the inspector in the lower section. It was one tall column per
        // side, which meant scrolling down to add a widget took the preview away with it.
        let at = |needle: &str| {
            drawn
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle} is not drawn"))
        };
        assert!(
            at("temps.json") < at("screens") && at("screens") < at("cpu"),
            "the upper left must be the title and the screens, with the sources beside them: {drawn:?}"
        );
        assert!(
            at("widgets") < at("the pad's screen, as it will look"),
            "the widgets and the preview belong in the lower section, side by side: {drawn:?}"
        );
        // its own fields, with the values from the file rather than defaults
        assert!(drawn.iter().any(|line| line.trim() == "title:"));
        assert!(drawn.iter().any(|line| line.trim() == "TEMPS"));
        // its sources, by name, and which nothing reads
        assert!(drawn.iter().any(|line| line.trim() == "cpu"));
        assert!(drawn.iter().any(|line| line.contains("cmd:uptime")));
        assert!(
            drawn.iter().any(|line| line.contains("nothing reads it")),
            "an unused source should be visible: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| line.trim() == "used"),
            "a source a widget reads should say so: {drawn:?}"
        );
        // and the screen as it will look, from the same renderer the driver uses
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("the pad's screen, as it will look")),
            "no preview: {drawn:?}"
        );
        // with the row that adds a source
        assert!(drawn.iter().any(|line| line.trim() == "add:"));

        // the widget list is a table: one row per widget, one column per fact, with headers
        for header in ["#", "type", "what it reads", "do"] {
            assert!(
                drawn.iter().any(|line| line.trim() == header),
                "the widget table has no `{header}` column: {drawn:?}"
            );
        }
        // the number and the kind are separate cells, so they line up down the list
        assert!(
            drawn.iter().any(|line| line.trim() == "1"),
            "the widget's number is not a cell of its own: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| line.trim() == "text"),
            "the widget's kind is not a cell of its own: {drawn:?}"
        );
        // a name nothing provides is named where it is, in the reads column
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("nothing provides gpu")),
            "a value nothing provides is not in the reads column: {drawn:?}"
        );

        // Nothing's fields are drawn until its row is picked: that is the point of the table. Two widgets at
        // once is the wall of controls this replaced.
        assert!(
            !drawn.iter().any(|line| line.trim() == "scroll_width"),
            "a widget's fields are drawn before its row is opened: {drawn:?}"
        );
        window.widget_editing = Some(0);
        let drawn = draw_applets(&mut window);
        for key in ["x", "y", "align", "format", "scroll_width"] {
            assert!(
                drawn.iter().any(|line| line.trim() == key),
                "the field {key} is not drawn when the widget is open: {drawn:?}"
            );
        }
        assert!(
            drawn.iter().any(|line| line.contains("cpu {cpu}C")),
            "the format the file holds is not shown: {drawn:?}"
        );
        // and the numbers say what they are measured in, which is the column the grid added
        assert!(
            drawn.iter().any(|line| line.trim() == "px"),
            "a number has no unit beside it: {drawn:?}"
        );
        // a format is text with names in it, so the box stays and the names this applet can use are beside it
        assert!(
            drawn.iter().any(|line| line.contains("insert a name")),
            "a format has no list of names to insert: {drawn:?}"
        );
        // the open widget names what it reads and where each name comes from: `cpu` is declared by this applet,
        // and `gpu` is provided by nothing - the difference between a number and the widget's own name on the pad
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("cpu (declared above)")),
            "a declared value is not marked as declared: {drawn:?}"
        );
        // The second widget reads a name nothing provides: that is only said when *its* row is the one open,
        // because the inspector holds one widget at a time.
        assert!(
            !drawn
                .iter()
                .any(|line| line.contains("gpu - nothing provides this")),
            "one widget's readings are drawn while another's fields are open: {drawn:?}"
        );
        window.widget_editing = Some(1);
        let drawn = draw_applets(&mut window);
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("gpu - nothing provides this")),
            "a value nothing provides is not reported: {drawn:?}"
        );
        // both widgets here are `text`, so a field key cannot tell them apart: their own values can
        assert!(
            drawn.iter().any(|line| line.contains("{gpu}%")),
            "the opened widget's own format is not drawn: {drawn:?}"
        );
        assert!(
            !drawn.iter().any(|line| line.contains("cpu {cpu}C")),
            "the first widget's fields are still drawn: {drawn:?}"
        );

        // Every row carries its own move and remove buttons, in the last column so they are always in the same
        // place. They used to be drawn before the kind, to keep a long note from pushing them off the edge; the
        // column does that job now, and does it for every row at once.
        assert!(
            drawn.iter().any(|line| line.trim() == "remove"),
            "no remove button on the widget: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| line.trim() == "^")
                && drawn.iter().any(|line| line.trim() == "v"),
            "no move buttons on the widget: {drawn:?}"
        );

        // and one button per kind, so every widget this build draws can be added
        for kind in WIDGET_KINDS {
            assert!(
                drawn.iter().any(|line| line.trim() == kind),
                "{kind} cannot be added from the tab: {drawn:?}"
            );
        }
        // what the coordinates are measured against, said rather than left to be worked out
        assert!(
            drawn.iter().any(|line| line.contains("160 wide")),
            "the screen size is not stated: {drawn:?}"
        );
    }

    #[test]
    fn the_sources_tab_draws_the_endpoints_and_says_who_needs_them() {
        // A tab that exists in the code and not on the screen is the failure this project keeps meeting, and
        // this one edits the file a credential lives in, so it is worth being sure of.
        let dir = std::env::temp_dir().join("g13-gui-render-sources");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        let secret = "a-token-nobody-should-see-in-a-window";
        std::fs::write(
            dir.join("endpoints.json"),
            format!(
                r#"{{"weather": {{"url": "http://wttr.in", "insecure": true}},
                    "github": {{"url": "https://api.github.com", "token": "{secret}"}}}}"#
            ),
        )
        .unwrap();
        // an applet naming one of them, so the "used by" column has something to say
        std::fs::write(
            dir.join("applets/ci.json"),
            r#"{"name":"ci","sources":{"build":"http:github/repos/x/y/actions/runs#a.b"}}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        let drawn = draw_sources(&mut window);
        eprintln!("the Sources tab draws:");
        for line in &drawn {
            eprintln!("  {line}");
        }
        for wanted in [
            "weather",
            "github",
            "http://wttr.in",
            "https://api.github.com",
        ] {
            assert!(
                drawn.iter().any(|line| line.contains(wanted)),
                "{wanted} is not on the tab. It drew: {drawn:?}"
            );
        }
        // the endpoints are drawn by name, and what uses one is said
        assert!(
            drawn.iter().any(|line| line.contains("used by ci")),
            "nothing says which applet needs the endpoint: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| line.contains("nothing uses it")),
            "an endpoint nothing uses should say so: {drawn:?}"
        );
        // the token is on the screen as a length, never as itself
        assert!(
            !drawn.iter().any(|line| line.contains(secret)),
            "the token was drawn: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| line.contains("characters")),
            "the hidden token should still be identifiable by its length: {drawn:?}"
        );
        // and the shape an applet uses is explained rather than assumed
        assert!(drawn.iter().any(|line| line.contains("http:<endpoint>")));
    }

    #[test]
    fn a_token_on_a_link_that_cannot_keep_it_is_called_out() {
        let dir = std::env::temp_dir().join("g13-gui-render-sources-risk");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("endpoints.json"),
            r#"{"github": {"url": "https://api.github.com", "token": "a-token"},
                "intranet": {"url": "http://10.0.0.2:8080", "token": "another"},
                "weather": {"url": "https://wttr.in", "insecure": true}}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        let drawn = draw_sources(&mut window);
        let said = drawn.join(" | ");
        assert!(
            said.contains("anyone on the way to it can read the token"),
            "a token over http is not called out: {said}"
        );
        // once, and only for the endpoint whose token the link cannot keep: the https token and the
        // insecure host with no token at all are not warnings
        assert_eq!(
            said.matches("can read the token").count(),
            1,
            "only the endpoint that cannot keep its token is called out: {said}"
        );
        assert!(
            !said.contains("certificate is not checked and a token"),
            "the shipped weather endpoint has no token, so it is not a warning: {said}"
        );
    }

    #[test]
    fn an_applet_naming_an_endpoint_that_does_not_exist_is_said_out_loud() {
        let dir = std::env::temp_dir().join("g13-gui-render-sources-missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("applets")).unwrap();
        std::fs::write(
            dir.join("endpoints.json"),
            r#"{"weather": {"url": "http://wttr.in"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("applets/gone.json"),
            r#"{"name":"gone","sources":{"build":"http:nowhere/thing#field"}}"#,
        )
        .unwrap();

        let mut window = Window::load(&dir);
        let drawn = draw_sources(&mut window);
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("nowhere") && line.contains("no endpoint has that name")),
            "an applet naming a missing endpoint is not reported: {drawn:?}"
        );
    }

    #[test]
    fn a_broken_endpoints_file_is_reported_rather_than_looking_like_nothing_set_up() {
        let dir = std::env::temp_dir().join("g13-gui-render-sources-broken");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("endpoints.json"), "{ not json at all").unwrap();

        let mut window = Window::load(&dir);
        assert!(
            window.endpoints_problem.is_some(),
            "the fault should be kept"
        );
        let drawn = draw_sources(&mut window);
        assert!(
            drawn.iter().any(|line| line.contains("not valid JSON")),
            "the reason is not on the screen: {drawn:?}"
        );
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("not because nothing is set up")),
            "an empty table after a broken file must not look like an empty file: {drawn:?}"
        );
    }

    #[test]
    fn the_controls_tab_draws_the_radial_and_says_what_each_sector_does() {
        // The stick's sectors are exactly the sort of thing that ends up in the code and not on the screen, so
        // this renders the tab and reports which sectors it drew and what it says each one does.
        let dir = std::env::temp_dir().join("g13-gui-render-sectors");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("stick.json"),
            r#"{"mode":"keyboard","sectors":8,"points":{"centre":[130,119],"up":[126,11],
                "down":[126,228],"left":[10,130],"right":[236,123]}}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        // Written to the file the window says it is reading, rather than to a name guessed from the active
        // profile: the first version of this test wrote bindings-0 while the window was reading profile 1, so
        // it passed alone and failed in the suite, having drawn a stick with nothing bound at all.
        std::fs::write(window.bindings_file(), "JUP=p,k.17\nJRIGHT=gb,dpad-right\n").unwrap();
        window.reload();
        let drawn = draw_controls(&mut window);
        eprintln!("the Controls tab draws, for a bound eight-sector stick:");
        for line in &drawn {
            eprintln!("  {line}");
        }
        assert!(
            drawn.iter().any(|line| line.contains("sectors:")),
            "no sector count on the tab. It drew: {drawn:?}"
        );
        // Every sector is drawn by name, in order, saying where it points - so an unbound sector is visible
        // rather than silent, and the words come from the same code the driver uses.
        for expected in [
            "JUP  up",
            "J1  up and right",
            "JRIGHT  right",
            "J3  down and right",
            "J4  down",
            "J5  down and left",
            "J6  left",
            "J7  up and left",
        ] {
            assert!(
                drawn.iter().any(|line| line.trim() == expected),
                "the sector {expected:?} was not drawn. It drew: {drawn:?}"
            );
        }
        // The names already written in the file are kept rather than renumbered, and a sector with no line of
        // its own says where it falls back to.
        assert!(
            drawn
                .iter()
                .any(|line| line.contains("the cardinals beside it")),
            "a sector falling back to its cardinals does not say so: {drawn:?}"
        );
        // Every sector has something to click to set it: the bound ones show what they are bound to - the key's
        // own name rather than `p,k.17` - and the others the fallback they would replace. Without this the tab
        // showed the radial and offered no way to set it.
        for settable in ["w", "gb,dpad-right"] {
            assert!(
                drawn.iter().any(|line| line.contains(settable)),
                "no way to set the sector bound to {settable}: {drawn:?}"
            );
        }
        // what each one will actually press, in words
        assert!(drawn.iter().any(|line| line.trim() == "sends w"));
        assert!(drawn.iter().any(|line| line.trim() == "gamepad dpad-right"));
        // and the direction the sectors count in
        assert!(
            drawn.iter().any(|line| line.contains("sector 0 is up")),
            "the direction the sectors count in is not said: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| line.contains("5 of the 8 sectors")),
            "how many sectors do something is not said: {drawn:?}"
        );
    }

    #[test]
    fn the_controls_tab_draws_the_joystick_routing_that_can_be_set() {
        // The routing is what turns the stick into a controller, and it is exactly the sort of thing that ends up
        // in the code and not on the screen - which is how mouse mode was once reported as being there while the
        // binary had never been rebuilt. This renders the tab in joystick mode and says what it drew.
        //
        // Its own directory, not the one `joystick_mode_offers_a_destination_for_each_side` uses: two tests
        // sharing a path delete each other's file, and this one removes the directory before writing. Sharing
        // it made this test fail seven runs in eight while passing alone, having drawn a stick on defaults.
        let dir = std::env::temp_dir().join("g13-gui-render-route-drawn");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("stick.json"),
            r#"{"mode":"joystick","points":{"centre":[130,119],"up":[126,11],"down":[126,228],
                "left":[10,130],"right":[236,123]},
                "route":{"right":"left-x","left":"left-x","down":"left-trigger","up":"right-trigger"}}"#,
        )
        .unwrap();
        let mut window = Window::load(&dir);
        let drawn = draw_controls(&mut window);
        eprintln!("the Controls tab draws, in joystick mode:");
        for line in &drawn {
            eprintln!("  {line}");
        }
        // every side of the stick has its own row
        for side in ["right:", "left:", "down:", "up:"] {
            assert!(
                drawn.iter().any(|line| line.trim() == side),
                "no {side} dropdown was drawn. It drew: {drawn:?}"
            );
        }
        // and each row shows what is actually chosen rather than a default: steering on two sides, the
        // triggers on the other two
        assert!(
            drawn.iter().filter(|line| line.trim() == "left-x").count() >= 2,
            "the steering target is not shown on both sides it is set to: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| line.trim() == "right-trigger"),
            "the throttle is not shown: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| line.trim() == "left-trigger"),
            "the brake is not shown: {drawn:?}"
        );
        // a trigger opening one way is said before one is chosen, not after
        assert!(
            drawn.iter().any(|line| line.contains("opens one way")),
            "nothing says a trigger opens one way: {drawn:?}"
        );
    }

    #[test]
    fn the_controls_tab_draws_every_stick_mode_it_offers() {
        // What this replaced: mouse mode was written, tested, committed, and reported as being on screen -
        // while the binary being run had never been rebuilt and the window had never been looked at. This is
        // the check that says what the tab actually draws, so the claim does not rest on anybody's word.
        let dir = std::env::temp_dir().join("g13-gui-render-modes");
        let _ = std::fs::create_dir_all(&dir);
        let mut window = Window::load(&dir);
        let drawn = draw_controls(&mut window);
        // printed rather than only asserted, so the list is visible when somebody asks what it draws
        eprintln!("the Controls tab draws:");
        for line in &drawn {
            eprintln!("  {line}");
        }
        for mode in g13_config::stick::Mode::ALL {
            assert!(
                drawn.iter().any(|line| line.trim() == mode.name()),
                "the Controls tab drew no {} button. It drew: {:?}",
                mode.name(),
                drawn
            );
        }
        // the mode names are the ones the config file uses, not a second spelling that would drift
        assert!(drawn.iter().any(|line| line.contains("the stick drives")));
    }

    /// The Bindings tab drawn, which is a different tab from the one the other helper draws.
    fn draw_bindings(window: &mut Window) -> Vec<String> {
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1200.0),
            )),
            ..Default::default()
        };
        let mut warm_up = context.run_ui(input.clone(), |ui| bindings(ui, window));
        warm_up.textures_delta.clear();
        let mut output = context.run_ui(input, |ui| bindings(ui, window));
        let drawn = drawn_text(&output);
        output.textures_delta.clear();
        drawn
    }

    #[test]
    fn joystick_mode_offers_a_destination_for_each_side() {
        // The four sides have to be there with their targets, or the split a driving game needs is unavailable
        // however well the driver underneath supports it.
        let dir = std::env::temp_dir().join("g13-gui-render-route");
        let _ = std::fs::create_dir_all(&dir);
        let mut window = Window::load(&dir);
        window.set_stick_mode(g13_config::stick::Mode::Joystick);
        window.set_route("up", g13_config::stick::Target::RightTrigger);
        let drawn = draw_controls(&mut window);
        for side in ["right:", "left:", "down:", "up:"] {
            assert!(
                drawn.iter().any(|line| line.trim() == side),
                "no destination for {side}. It drew: {:?}",
                drawn
            );
        }
        assert!(
            drawn.iter().any(|line| line.trim() == "right-trigger"),
            "the chosen trigger is not shown. It drew: {:?}",
            drawn
        );
        // and it is written, because the driver reads the file rather than this window
        let written = std::fs::read_to_string(dir.join("stick.json")).unwrap_or_default();
        assert!(
            written.contains("\"up\": \"right-trigger\""),
            "wrote: {written}"
        );
        // the other three are left alone by that one change
        assert!(
            written.contains("\"right\": \"left-x\""),
            "wrote: {written}"
        );
    }

    #[test]
    fn setting_a_binding_offers_to_detect_the_press() {
        // Nobody should have to know that a mouse button is written "mb,left". The button has to be there
        // while a control is being edited, and the listening state has to be visible once it is clicked.
        let dir = std::env::temp_dir().join("g13-gui-render-detect");
        let _ = std::fs::create_dir_all(&dir);
        let mut window = Window::load(&dir);
        window.begin_edit("G1");
        let drawn = draw_bindings(&mut window);
        assert!(
            drawn.iter().any(|line| line.trim() == "press to set"),
            "no way to set a binding by pressing it. The tab drew: {:?}",
            drawn
        );
    }

    #[test]
    fn choosing_mouse_in_the_window_puts_the_speed_beside_it() {
        let dir = std::env::temp_dir().join("g13-gui-render-speed");
        let _ = std::fs::create_dir_all(&dir);
        let mut window = Window::load(&dir);
        window.set_stick_mode(g13_config::stick::Mode::Mouse);
        let drawn = draw_controls(&mut window);
        assert!(
            drawn.iter().any(|line| line.contains("pixels a second")),
            "mouse mode drew no speed. It drew: {:?}",
            drawn
        );
        // and that is the file, not just the window: the driver reads this file
        let written = std::fs::read_to_string(dir.join("stick.json")).unwrap_or_default();
        assert!(written.contains("\"mode\": \"mouse\""), "wrote: {written}");
    }
}
