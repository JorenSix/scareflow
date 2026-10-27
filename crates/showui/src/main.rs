//! 🎃 Scareflow show editor: wire key and OSC events through flow blocks to
//! sound, WLED, smart plug and OSC out responses.
//!
//! cargo run -p scareflow-showui -- [showui.toml]

mod editor;
mod graph;
mod runner;
mod show;
mod theme;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_snarl::Snarl;
use scareflow_osc::{OscEvent, OscListener};
use scareflow_sound::{OutputDevice, SoundConfig, output_devices};

use crate::editor::{GLOW, Viewer, add_menu};
use crate::graph::{Graph, Node, OscSection, ShowFile, SoundSection};
use crate::runner::{Msg, Report, Runner};
use crate::show::{list_sounds, show_key};
use crate::theme::{BLOOD, TRIGGER, VALUE};

const LOG_LINES: usize = 200;
const ICON: &[u8] = include_bytes!("../assets/pumpkin.rgba");

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let path = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "showui.toml".into()));
    let icon = egui::IconData { rgba: ICON.to_vec(), width: 256, height: 256 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 860.0])
            .with_title("🎃 Scareflow")
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native("🎃 Scareflow", options, Box::new(|cc| Ok(Box::new(App::new(path, &cc.egui_ctx)))))
}

struct App {
    path: PathBuf,
    sound_root: PathBuf,
    osc: OscSection,
    sound: SoundSection,
    snarl: Snarl<Node>,
    saved: ShowFile,
    sent: Graph,
    runner: Runner,
    backgrounds: Vec<PathBuf>,
    sounds: Vec<PathBuf>,
    devices: Vec<OutputDevice>,
    osc_tx: Sender<OscEvent>,
    listener: Option<OscListener>,
    listen_status: String,
    fired: HashMap<usize, Instant>,
    playing: HashMap<Option<String>, usize>,
    log: VecDeque<String>,
    started: Instant,
}

impl App {
    fn new(path: PathBuf, ctx: &egui::Context) -> Self {
        theme::apply(ctx);
        let mut log = VecDeque::new();
        let file = if path.exists() {
            ShowFile::load(&path).unwrap_or_else(|e| {
                log.push_back(format!("{e}; starting from the example show"));
                example()
            })
        } else {
            log.push_back(format!("{} not found; starting from the example show, Save creates it", path.display()));
            example()
        };
        let (snarl, problems) = file.to_snarl();
        log.extend(problems);

        let sound_root = file.sound_root(&path);
        let template = SoundConfig {
            dir: sound_root.clone(),
            duck_to: file.sound.duck_to,
            buffer_frames: (file.sound.buffer_frames > 0).then_some(file.sound.buffer_frames),
            ..Default::default()
        };
        let graph = Graph::compile(&snarl);
        let ctx_for_runner = ctx.clone();
        let runner = Runner::spawn(graph.clone(), template, move || ctx_for_runner.request_repaint());

        // The listener's channel outlives restarts of the listener itself.
        let (osc_tx, osc_rx) = mpsc::channel();
        let runner_tx = runner.tx.clone();
        thread::Builder::new()
            .name("scareflow-showui-osc".into())
            .spawn(move || {
                for event in osc_rx {
                    if runner_tx.send(Msg::Osc(event)).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn OSC forwarder");

        let mut app = Self {
            path,
            sound_root,
            osc: file.osc.clone(),
            sound: file.sound.clone(),
            snarl,
            saved: file,
            sent: graph,
            runner,
            backgrounds: Vec::new(),
            sounds: Vec::new(),
            devices: Vec::new(),
            osc_tx,
            listener: None,
            listen_status: String::new(),
            fired: HashMap::new(),
            playing: HashMap::new(),
            log,
            started: Instant::now(),
        };
        app.rescan();
        app.listen();
        app
    }

    /// Reloads the audio output devices and the files in the sound folders.
    fn rescan(&mut self) {
        self.backgrounds = list_sounds(&self.sound_root, &self.sound.background_dir);
        self.sounds = list_sounds(&self.sound_root, &self.sound.scare_dir);
        self.sounds.extend(self.backgrounds.iter().cloned());
        self.devices = output_devices();
    }

    fn current(&self) -> ShowFile {
        ShowFile::from_snarl(&self.snarl, self.osc.clone(), self.sound.clone())
    }

    fn listen(&mut self) {
        self.listener = None; // release the port before binding again
        match OscListener::spawn(&self.osc.listen, self.osc_tx.clone()) {
            Ok(listener) => {
                self.listen_status = format!("listening on {}", listener.local_addr());
                self.listener = Some(listener);
            }
            Err(e) => self.listen_status = format!("not listening: {e}"),
        }
    }

    fn push_log(&mut self, line: String) {
        let t = self.started.elapsed().as_secs_f32();
        self.log.push_back(format!("{t:8.1}s  {line}"));
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let dirty = if self.current() != self.saved { " •" } else { "" };
            ui.strong(format!("🎃 {}{dirty}", self.path.display()));
            if ui.button("Save").clicked() {
                let file = self.current();
                match file.save(&self.path) {
                    Ok(()) => self.saved = file,
                    Err(e) => self.push_log(format!("save failed: {e}")),
                }
            }
            if ui.button("Revert").on_hover_text("Reload the saved show").clicked() {
                match ShowFile::load(&self.path) {
                    Ok(file) => {
                        let (snarl, problems) = file.to_snarl();
                        problems.into_iter().for_each(|p| self.push_log(p));
                        self.snarl = snarl;
                        self.osc = file.osc.clone();
                        self.sound = file.sound.clone();
                        self.saved = file;
                    }
                    Err(e) => self.push_log(e),
                }
            }
            ui.separator();
            ui.menu_button("+ Add block", |ui| {
                if let Some(node) = add_menu(ui) {
                    let k = self.snarl.nodes().count() % 10;
                    self.snarl.insert_node(egui::pos2(40.0 + 30.0 * k as f32, 40.0 + 30.0 * k as f32), node);
                    ui.close();
                }
            });
            ui.separator();
            ui.label("OSC");
            let edit = ui.add(egui::TextEdit::singleline(&mut self.osc.listen).desired_width(120.0));
            if ui.button("Listen").clicked() || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                self.listen();
            }
            ui.weak(&self.listen_status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let stop = egui::Button::new(egui::RichText::new("STOP ALL  (Esc)").strong().color(egui::Color32::WHITE))
                    .fill(BLOOD);
                if ui.add(stop).on_hover_text("Cancel delayed triggers, stop all sounds and fade out the music; plugs keep their state").clicked() {
                    let _ = self.runner.tx.send(Msg::Panic);
                }
                if ui.button("Refresh audio devices").on_hover_text("Reload the audio outputs (also inactive ones) and the sound folders").clicked() {
                    self.rescan();
                    let names: Vec<String> = self
                        .devices
                        .iter()
                        .map(|d| if d.active { d.name.clone() } else { format!("{} (inactive)", d.name) })
                        .collect();
                    let line = format!("{} audio output(s): {}; {} sound file(s)", names.len(), names.join(", "), self.sounds.len());
                    self.push_log(line);
                }
            });
        });
        ui.horizontal(|ui| {
            ui.colored_label(TRIGGER, "▶ trigger");
            ui.colored_label(VALUE, "● value");
            ui.weak("Drag between pins to connect · right-click the canvas to add, a block to delete · right-click a pin to unplug · keys work anywhere in this window except while typing");
        });
    }

    fn log_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Events");
            if ui.small_button("Clear").clicked() {
                self.log.clear();
            }
        });
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            for line in &self.log {
                ui.monospace(line);
            }
        });
    }
}

impl eframe::App for App {
    /// Takes the show keys out of the input before any widget sees them, so they reach
    /// the Key blocks wherever the focus is (Space no longer presses the focused button).
    /// Only a focused text field keeps its keys.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if ctx.wants_keyboard_input() {
            return;
        }
        raw_input.events.retain(|event| {
            let egui::Event::Key { key, pressed, repeat, modifiers, .. } = event else { return true };
            log::debug!("key event {key:?} pressed={pressed} repeat={repeat} modifiers={modifiers:?}");
            if !modifiers.is_none() {
                return true;
            }
            let name = show_key(*key);
            let ours = *key == egui::Key::Escape || name.is_some();
            if ours && *pressed && !*repeat {
                let msg = match name {
                    Some(name) => Msg::Key(name.to_owned()),
                    None => Msg::Panic,
                };
                let _ = self.runner.tx.send(msg);
            }
            !ours
        });
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(report) = self.runner.reports.try_recv() {
            match report {
                Report::Log(line) => self.push_log(line),
                Report::Fired(node) => drop(self.fired.insert(node, Instant::now())),
                Report::Playing(device, Some(node)) => drop(self.playing.insert(device, node)),
                Report::Playing(device, None) => drop(self.playing.remove(&device)),
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(2.0);
        });
        egui::TopBottomPanel::bottom("log").resizable(true).default_height(140.0).show(ctx, |ui| self.log_panel(ui));
        let mut viewer = Viewer {
            backgrounds: &self.backgrounds,
            sounds: &self.sounds,
            devices: &self.devices,
            fired: &self.fired,
            playing: &self.playing,
            fire: Vec::new(),
        };
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
            self.snarl.show(&mut viewer, &theme::snarl_style(), "show-graph", ui);
        });
        for node in std::mem::take(&mut viewer.fire) {
            let _ = self.runner.tx.send(Msg::Fire(node));
        }

        let graph = Graph::compile(&self.snarl);
        if graph != self.sent {
            let _ = self.runner.tx.send(Msg::Graph(Arc::new(graph.clone())));
            self.sent = graph;
        }
        self.fired.retain(|_, t| t.elapsed() < GLOW);
        if !self.fired.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
}

fn example() -> ShowFile {
    toml::from_str(include_str!("../../../showui.example.toml")).expect("bundled example show parses")
}
