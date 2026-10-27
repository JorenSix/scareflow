//! The node canvas: how blocks, pins and cables look and are edited.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Ui};
use egui_snarl::ui::{PinInfo, SnarlViewer};
use egui_snarl::{InPin, NodeId, OutPin, OutPinId, Snarl};

use scareflow_sound::OutputDevice;

use crate::graph::{Category, Node, PinKind, WledDo, can_connect};
use crate::show::{KEYS, key_label, label};
use crate::theme::{BONE, CANDLE, NIGHT, SLIME, TRIGGER, VALUE, category_color};

/// How long a block's header glows after it fired.
pub const GLOW: Duration = Duration::from_millis(350);

fn pin(kind: PinKind) -> PinInfo {
    match kind {
        PinKind::Trigger => PinInfo::triangle().with_fill(TRIGGER).with_wire_color(TRIGGER),
        PinKind::Value => PinInfo::circle().with_fill(VALUE).with_wire_color(VALUE),
    }
}

/// Borrowed panel state the blocks need for drawing.
pub struct Viewer<'a> {
    pub backgrounds: &'a [PathBuf],
    /// Every file a one-shot Sound block may play.
    pub sounds: &'a [PathBuf],
    pub devices: &'a [OutputDevice],
    pub fired: &'a HashMap<usize, Instant>,
    /// Background block playing on each output device.
    pub playing: &'a HashMap<Option<String>, usize>,
    /// Event blocks whose ▶ was clicked this frame.
    pub fire: Vec<usize>,
}

impl SnarlViewer<Node> for Viewer<'_> {
    fn title(&mut self, node: &Node) -> String {
        node.title().into()
    }

    fn header_frame(&mut self, default: egui::Frame, node: NodeId, _: &[InPin], _: &[OutPin], snarl: &Snarl<Node>) -> egui::Frame {
        let glowing = self.fired.get(&node.0).is_some_and(|t| t.elapsed() < GLOW);
        let fill = if glowing { CANDLE } else { category_color(snarl[node].category()) };
        default.fill(fill)
    }

    fn show_header(&mut self, node: NodeId, _: &[InPin], _: &[OutPin], ui: &mut Ui, snarl: &mut Snarl<Node>) {
        let block = &snarl[node];
        let glowing = self.fired.get(&node.0).is_some_and(|t| t.elapsed() < GLOW);
        ui.label(egui::RichText::new(block.title()).strong().color(if glowing { NIGHT } else { BONE }));
        if self.is_playing(node.0, block) {
            ui.label(egui::RichText::new("▶ playing").strong().color(SLIME));
        }
        if block.category() == Category::Event && ui.small_button("▶").on_hover_text("Fire this event").clicked() {
            self.fire.push(node.0);
        }
    }

    fn inputs(&mut self, node: &Node) -> usize {
        node.inputs().len()
    }

    fn outputs(&mut self, node: &Node) -> usize {
        node.outputs().len()
    }

    #[allow(refining_impl_trait)]
    fn show_input(&mut self, pin_ref: &InPin, ui: &mut Ui, snarl: &mut Snarl<Node>) -> PinInfo {
        let (name, kind) = snarl[pin_ref.id.node].inputs()[pin_ref.id.input].clone();
        ui.label(name);
        pin(kind)
    }

    #[allow(refining_impl_trait)]
    fn show_output(&mut self, pin_ref: &OutPin, ui: &mut Ui, snarl: &mut Snarl<Node>) -> PinInfo {
        let (name, kind) = snarl[pin_ref.id.node].outputs()[pin_ref.id.output].clone();
        ui.label(name);
        pin(kind)
    }

    fn has_body(&mut self, _: &Node) -> bool {
        true
    }

    fn show_body(&mut self, node: NodeId, _: &[InPin], _: &[OutPin], ui: &mut Ui, snarl: &mut Snarl<Node>) {
        let before = snarl[node].outputs().len();
        ui.push_id(node.0, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(210.0);
                self.body(ui, node.0, &mut snarl[node]);
            });
        });
        // Fewer outputs (OSC args, random picks): drop cables on pins that are gone.
        let after = snarl[node].outputs().len();
        for output in after..before {
            snarl.drop_outputs(OutPinId { node, output });
        }
    }

    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<Node>) {
        let (a, b) = (&snarl[from.id.node], &snarl[to.id.node]);
        if !can_connect(a, from.id.output, b, to.id.input) {
            return;
        }
        // A value input listens to one source at a time.
        if b.inputs()[to.id.input].1 == PinKind::Value {
            snarl.drop_inputs(to.id);
        }
        snarl.connect(from.id, to.id);
    }

    fn has_graph_menu(&mut self, _: egui::Pos2, _: &mut Snarl<Node>) -> bool {
        true
    }

    fn show_graph_menu(&mut self, pos: egui::Pos2, ui: &mut Ui, snarl: &mut Snarl<Node>) {
        if let Some(node) = add_menu(ui) {
            snarl.insert_node(pos, node);
            ui.close();
        }
    }

    fn has_node_menu(&mut self, _: &Node) -> bool {
        true
    }

    fn show_node_menu(&mut self, node: NodeId, _: &[InPin], _: &[OutPin], ui: &mut Ui, snarl: &mut Snarl<Node>) {
        if ui.button("Duplicate").clicked() {
            let copy = snarl[node].clone();
            let pos = snarl.get_node_info(node).map(|n| n.pos).unwrap_or_default() + egui::vec2(30.0, 30.0);
            snarl.insert_node(pos, copy);
            ui.close();
        }
        if ui.button("Delete").clicked() {
            snarl.remove_node(node);
            ui.close();
        }
    }
}

impl Viewer<'_> {
    fn is_playing(&self, node: usize, block: &Node) -> bool {
        matches!(block, Node::Background { device, .. } if self.playing.get(device) == Some(&node))
    }

    fn body(&mut self, ui: &mut Ui, node: usize, block: &mut Node) {
        let playing = self.is_playing(node, block);
        match block {
            Node::Button { label } => {
                let button = egui::Button::new(egui::RichText::new(label.as_str()).strong().size(16.0)).min_size(egui::vec2(170.0, 34.0));
                if ui.add(button).clicked() {
                    self.fire.push(node);
                }
                ui.add(egui::TextEdit::singleline(label).desired_width(170.0).hint_text("button text"));
            }
            Node::Key { key } => {
                egui::ComboBox::from_id_salt("key").selected_text(key_label(key)).show_ui(ui, |ui| {
                    for k in KEYS {
                        ui.selectable_value(key, k.to_owned(), key_label(k));
                    }
                });
            }
            Node::OscIn { address, args } => {
                let valid = rosc::address::Matcher::new(address).is_ok();
                let edit = egui::TextEdit::singleline(address).desired_width(170.0).font(egui::TextStyle::Monospace);
                let edit = if valid { edit } else { edit.text_color(ui.visuals().error_fg_color) };
                ui.add(edit).on_hover_text("OSC address pattern; * ? [a-z] {a,b} wildcards");
                ui.add(egui::DragValue::new(args).range(0..=4).prefix("values: "))
                    .on_hover_text("How many OSC arguments to offer as value outputs");
            }
            Node::Delay { ms } => drop(ui.add(egui::DragValue::new(ms).range(0..=600_000).speed(10).suffix(" ms"))),
            Node::Random { outputs } => drop(ui.add(egui::DragValue::new(outputs).range(2..=6).prefix("outputs: "))),
            Node::Cooldown { secs } => drop(ui.add(egui::DragValue::new(secs).range(0.1..=3600.0).speed(0.1).suffix(" s"))),
            Node::Background { file, volume, device } => {
                file_picker(ui, "file", file, self.backgrounds);
                ui.add(egui::Slider::new(volume, 0.0..=1.0).text("volume"));
                device_picker(ui, device, self.devices);
                if playing {
                    ui.colored_label(SLIME, "▶ playing, the slider changes the volume live");
                } else {
                    ui.weak("loops until stopped");
                }
            }
            Node::Sound { file, volume, device } => {
                file_picker(ui, "file", file, self.sounds);
                ui.add(egui::Slider::new(volume, 0.0..=1.0).text("volume"));
                device_picker(ui, device, self.devices);
            }
            Node::Wled { host, action } => {
                ui.add(egui::TextEdit::singleline(host).desired_width(130.0).hint_text("host"));
                egui::ComboBox::from_id_salt("wled-do").selected_text(action.name()).show_ui(ui, |ui| {
                    for (name, template) in WledDo::templates() {
                        if ui.selectable_label(action.name() == name, name).clicked() && action.name() != name {
                            *action = template;
                        }
                    }
                });
                match action {
                    WledDo::Lightning { duration_ms } => {
                        ui.add(egui::DragValue::new(duration_ms).range(100..=60_000).speed(50).suffix(" ms"));
                        ui.weak("white flashes, even when off");
                    }
                    WledDo::Effect { fx, speed, intensity } => {
                        ui.add(egui::DragValue::new(fx).prefix("effect ")).on_hover_text("57 = Lightning, 23 = Strobe");
                        ui.add(egui::DragValue::new(speed).prefix("speed "));
                        ui.add(egui::DragValue::new(intensity).prefix("intensity "));
                    }
                    WledDo::Preset { preset } => drop(ui.add(egui::DragValue::new(preset).range(1..=250).prefix("preset "))),
                    WledDo::Power { on } => drop(ui.checkbox(on, "on")),
                }
            }
            Node::Plug { host, on_ms } => {
                ui.add(egui::TextEdit::singleline(host).desired_width(130.0).hint_text("host"));
                ui.add(egui::DragValue::new(on_ms).range(0..=86_400_000).speed(10).prefix("off after ").suffix(" ms"))
                    .on_hover_text("0 = stays on until the off input fires");
            }
            Node::OscOut { to, address, args } => {
                ui.add(egui::TextEdit::singleline(to).desired_width(150.0).hint_text("ip:port"));
                ui.add(egui::TextEdit::singleline(address).desired_width(150.0).font(egui::TextStyle::Monospace));
                ui.horizontal(|ui| {
                    for a in args.iter_mut() {
                        ui.add(egui::DragValue::new(a).speed(0.01));
                    }
                    if ui.small_button("+").clicked() {
                        args.push(0.0);
                    }
                    if !args.is_empty() && ui.small_button("-").clicked() {
                        args.pop();
                    }
                });
            }
        }
    }
}

/// Menu of all blocks grouped by category; returns the chosen block.
pub fn add_menu(ui: &mut Ui) -> Option<Node> {
    let mut chosen = None;
    for (category, heading) in [(Category::Event, "Events"), (Category::Flow, "Flow"), (Category::Response, "Responses")] {
        ui.label(egui::RichText::new(heading).strong().color(category_color(category)));
        for (c, name, template) in Node::templates() {
            if c == category && ui.button(name).clicked() {
                chosen = Some(template);
            }
        }
    }
    chosen
}

fn file_picker(ui: &mut Ui, id: &str, selected: &mut Option<PathBuf>, files: &[PathBuf]) {
    let text = selected.as_deref().map(label).unwrap_or_else(|| "(choose a file)".into());
    egui::ComboBox::from_id_salt(id).width(190.0).selected_text(text).show_ui(ui, |ui| {
        if files.is_empty() {
            ui.weak("no audio files found");
        }
        for file in files {
            ui.selectable_value(selected, Some(file.clone()), label(file));
        }
    });
}

fn device_picker(ui: &mut Ui, device: &mut Option<String>, devices: &[OutputDevice]) {
    let text = device.clone().unwrap_or_else(|| "System default output".into());
    egui::ComboBox::from_id_salt("device").width(190.0).selected_text(text).show_ui(ui, |ui| {
        ui.selectable_value(device, None, "System default output");
        for d in devices {
            let label = if d.active { d.name.clone() } else { format!("{} (inactive)", d.name) };
            ui.selectable_value(device, Some(d.name.clone()), label);
        }
        if device.as_ref().is_some_and(|name| !devices.iter().any(|d| &d.name == name)) {
            ui.weak("selected device not found; using the default");
        }
    });
}
