//! The show as a graph: event blocks (key, OSC in) wired through flow blocks
//! (delay, random pick, cooldown) to response blocks (sound, WLED, OSC out).
//!
//! Cables carry either a **trigger** ("something happened") or a **value**
//! (a number, e.g. an OSC argument). Pins only connect to the same kind.
//! Stored as TOML (`showui.toml`); see `showui.example.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui_snarl::{InPinId, NodeId, OutPinId, Snarl};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinKind {
    Trigger,
    Value,
}

use PinKind::{Trigger, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "block", rename_all = "snake_case")]
pub enum Node {
    /// Fires when a key from [`crate::show::KEYS`] is pressed while the panel has focus.
    Key { key: String },
    /// A clickable button on the canvas, e.g. to stop the music by hand.
    Button { label: String },
    /// Fires on OSC messages matching `address`; outputs its first `args` arguments as values.
    OscIn {
        address: String,
        #[serde(default)]
        args: usize,
    },
    Delay { ms: u64 },
    /// Passes each trigger to one of its connected outputs at random.
    Random { outputs: usize },
    /// Passes a trigger, then drops triggers for `secs`.
    Cooldown { secs: f32 },
    /// Looping music; one loop per output device, a new one cross-fades.
    Background {
        file: Option<PathBuf>,
        volume: f32,
        #[serde(default)]
        device: Option<String>,
    },
    /// One-shot sound on top of the music.
    Sound {
        file: Option<PathBuf>,
        volume: f32,
        #[serde(default)]
        device: Option<String>,
    },
    Wled { host: String, action: WledDo },
    /// Tasmota smart plug: switches on, and off again after `on_ms` (0 = stays on).
    Plug { host: String, on_ms: u64 },
    /// Sends `address` with `args` (floats) to `to` (`host:port`).
    OscOut {
        to: String,
        address: String,
        #[serde(default)]
        args: Vec<f32>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum WledDo {
    /// Hard flashes on every segment, even when the strip is off; its state is restored after.
    Lightning { duration_ms: u64 },
    Effect { fx: u8, speed: u8, intensity: u8 },
    Preset { preset: u8 },
    Power { on: bool },
}

impl WledDo {
    pub fn templates() -> [(&'static str, WledDo); 4] {
        [
            ("Lightning", WledDo::Lightning { duration_ms: 3000 }),
            ("Effect", WledDo::Effect { fx: 57, speed: 128, intensity: 128 }),
            ("Preset", WledDo::Preset { preset: 1 }),
            ("Power", WledDo::Power { on: true }),
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            WledDo::Lightning { .. } => "Lightning",
            WledDo::Effect { .. } => "Effect",
            WledDo::Preset { .. } => "Preset",
            WledDo::Power { .. } => "Power",
        }
    }
}

/// Input pin numbers shared by the sound blocks.
pub const PLAY: usize = 0;
pub const STOP: usize = 1;
pub const VOLUME: usize = 2;
/// Input pin numbers of the WLED and OSC out blocks.
pub const GO: usize = 0;
pub const LEVEL: usize = 1;

/// Input and output pin numbers of the Plug block.
pub const ON: usize = 0;
pub const OFF: usize = 1;

pub const DEFAULT_WLED: &str = "192.168.1.102";
pub const DEFAULT_PLUG: &str = "192.168.1.162";
pub const DEFAULT_PLUG_MS: u64 = 1800;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Event,
    Flow,
    Response,
}

impl Node {
    /// Blocks offered by the add menus, with their default settings.
    pub fn templates() -> Vec<(Category, &'static str, Node)> {
        use Category::*;
        vec![
            (Event, "Key", Node::Key { key: "Space".into() }),
            (Event, "Button", Node::Button { label: "Stop music".into() }),
            (Event, "OSC in", Node::OscIn { address: "/pumpkin/*/scare".into(), args: 0 }),
            (Flow, "Delay", Node::Delay { ms: 500 }),
            (Flow, "Random pick", Node::Random { outputs: 3 }),
            (Flow, "Cooldown", Node::Cooldown { secs: 5.0 }),
            (Response, "Background (continuous)", Node::Background { file: None, volume: 0.4, device: None }),
            (Response, "Sound (triggered)", Node::Sound { file: None, volume: 0.9, device: None }),
            (Response, "WLED", Node::Wled { host: DEFAULT_WLED.into(), action: WledDo::Lightning { duration_ms: 3000 } }),
            (Response, "Smart plug (Tasmota)", Node::Plug { host: DEFAULT_PLUG.into(), on_ms: DEFAULT_PLUG_MS }),
            (Response, "OSC out", Node::OscOut { to: "127.0.0.1:9001".into(), address: "/pumpkin/out".into(), args: vec![1.0] }),
        ]
    }

    pub fn title(&self) -> &'static str {
        match self {
            Node::Key { .. } => "Key",
            Node::Button { .. } => "Button",
            Node::OscIn { .. } => "OSC in",
            Node::Delay { .. } => "Delay",
            Node::Random { .. } => "Random pick",
            Node::Cooldown { .. } => "Cooldown",
            Node::Background { .. } => "Background",
            Node::Sound { .. } => "Sound",
            Node::Wled { .. } => "WLED",
            Node::Plug { .. } => "Smart plug",
            Node::OscOut { .. } => "OSC out",
        }
    }

    pub fn category(&self) -> Category {
        match self {
            Node::Key { .. } | Node::Button { .. } | Node::OscIn { .. } => Category::Event,
            Node::Delay { .. } | Node::Random { .. } | Node::Cooldown { .. } => Category::Flow,
            _ => Category::Response,
        }
    }

    pub fn inputs(&self) -> Vec<(String, PinKind)> {
        let pins: &[(&str, PinKind)] = match self {
            Node::Key { .. } | Node::Button { .. } | Node::OscIn { .. } => &[],
            Node::Delay { .. } | Node::Random { .. } | Node::Cooldown { .. } => &[("in", Trigger)],
            Node::Background { .. } | Node::Sound { .. } => &[("play", Trigger), ("stop", Trigger), ("volume", Value)],
            Node::Wled { .. } => &[("go", Trigger), ("brightness", Value)],
            Node::Plug { .. } => &[("on", Trigger), ("off", Trigger)],
            Node::OscOut { .. } => &[("send", Trigger), ("value", Value)],
        };
        pins.iter().map(|(n, k)| (n.to_string(), *k)).collect()
    }

    pub fn outputs(&self) -> Vec<(String, PinKind)> {
        match self {
            Node::Key { .. } | Node::Button { .. } => vec![("pressed".into(), Trigger)],
            Node::OscIn { args, .. } => std::iter::once(("received".into(), Trigger))
                .chain((1..=*args).map(|i| (format!("arg{i}"), Value)))
                .collect(),
            Node::Delay { .. } | Node::Cooldown { .. } => vec![("out".into(), Trigger)],
            Node::Random { outputs } => (1..=*outputs).map(|i| (i.to_string(), Trigger)).collect(),
            Node::Plug { .. } => vec![("switched on".into(), Trigger), ("switched off".into(), Trigger)],
            _ => Vec::new(),
        }
    }
}

/// True when a cable from `from`'s output to `to`'s input is allowed.
pub fn can_connect(from: &Node, output: usize, to: &Node, input: usize) -> bool {
    match (from.outputs().get(output), to.inputs().get(input)) {
        (Some((_, a)), Some((_, b))) => a == b,
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OscSection {
    pub listen: String,
}

impl Default for OscSection {
    fn default() -> Self {
        Self { listen: "0.0.0.0:9000".into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SoundSection {
    /// Sound root, relative to the show file. Every sound path stays inside it.
    pub dir: PathBuf,
    /// Folders inside `dir` listed by the Background and Sound blocks.
    pub background_dir: PathBuf,
    pub scare_dir: PathBuf,
    /// Music level factor while effects play; applied when the panel starts.
    pub duck_to: Option<f32>,
    /// Audio callback size in frames; 0 leaves it to the audio system.
    pub buffer_frames: u32,
}

impl Default for SoundSection {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("."),
            background_dir: PathBuf::from("bg-music"),
            scare_dir: PathBuf::from("scare"),
            duck_to: Some(0.5),
            buffer_frames: scareflow_sound::DEFAULT_BUFFER_FRAMES,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowFile {
    pub osc: OscSection,
    pub sound: SoundSection,
    #[serde(rename = "node")]
    pub nodes: Vec<NodeEntry>,
    #[serde(rename = "wire")]
    pub wires: Vec<Wire>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeEntry {
    pub id: usize,
    pub pos: [f32; 2],
    #[serde(flatten)]
    pub node: Node,
}

/// `[node id, pin]` → `[node id, pin]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wire {
    pub from: [usize; 2],
    pub to: [usize; 2],
}

impl ShowFile {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut value: toml::Value = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        migrate(&mut value);
        value.try_into().map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Absolute sound root for a show stored at `path`.
    pub fn sound_root(&self, path: &Path) -> PathBuf {
        path.parent().unwrap_or(Path::new(".")).join(&self.sound.dir)
    }

    /// Builds the editor graph. Wires to missing nodes or pins, or between different
    /// pin kinds, are skipped and reported.
    pub fn to_snarl(&self) -> (Snarl<Node>, Vec<String>) {
        let mut snarl = Snarl::new();
        let mut ids = BTreeMap::new();
        for entry in &self.nodes {
            ids.insert(entry.id, snarl.insert_node(eframe::egui::pos2(entry.pos[0], entry.pos[1]), entry.node.clone()));
        }
        let mut problems = Vec::new();
        for wire in &self.wires {
            let (Some(&from), Some(&to)) = (ids.get(&wire.from[0]), ids.get(&wire.to[0])) else {
                problems.push(format!("wire {:?} -> {:?}: unknown block", wire.from, wire.to));
                continue;
            };
            if !can_connect(&snarl[from], wire.from[1], &snarl[to], wire.to[1]) {
                problems.push(format!("wire {:?} -> {:?}: pins do not match", wire.from, wire.to));
                continue;
            }
            snarl.connect(OutPinId { node: from, output: wire.from[1] }, InPinId { node: to, input: wire.to[1] });
        }
        (snarl, problems)
    }

    /// The file for `snarl` with these settings. Ids are renumbered from 1 in node order.
    pub fn from_snarl(snarl: &Snarl<Node>, osc: OscSection, sound: SoundSection) -> Self {
        let mut entries: Vec<_> = snarl.nodes_pos_ids().collect();
        entries.sort_by_key(|(id, _, _)| id.0);
        let number: BTreeMap<NodeId, usize> = entries.iter().enumerate().map(|(i, (id, _, _))| (*id, i + 1)).collect();
        let nodes = entries
            .iter()
            .map(|(id, pos, node)| NodeEntry { id: number[id], pos: [pos.x.round(), pos.y.round()], node: (*node).clone() })
            .collect();
        let mut wires: Vec<Wire> = snarl
            .wires()
            .map(|(out, inp)| Wire { from: [number[&out.node], out.output], to: [number[&inp.node], inp.input] })
            .collect();
        wires.sort_by_key(|w| (w.from, w.to));
        Self { osc, sound, nodes, wires }
    }
}

/// Upgrades older show files: plug `on_secs` (seconds) became `on_ms`.
fn migrate(file: &mut toml::Value) {
    let Some(nodes) = file.get_mut("node").and_then(|n| n.as_array_mut()) else { return };
    for node in nodes.iter_mut().filter_map(|n| n.as_table_mut()) {
        if let Some(secs) = node.remove("on_secs") {
            let secs = secs.as_float().or_else(|| secs.as_integer().map(|i| i as f64)).unwrap_or(0.0);
            node.insert("on_ms".into(), toml::Value::Integer((secs * 1000.0).round() as i64));
        }
    }
}

/// What the runner needs from the editor graph: blocks and cables by snarl node id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub nodes: BTreeMap<usize, Node>,
    /// `(node, output)` → `[(node, input)]`.
    pub wires: BTreeMap<(usize, usize), Vec<(usize, usize)>>,
}

impl Graph {
    pub fn compile(snarl: &Snarl<Node>) -> Self {
        let nodes: BTreeMap<usize, Node> = snarl.node_ids().map(|(id, node)| (id.0, node.clone())).collect();
        let mut wires: BTreeMap<(usize, usize), Vec<(usize, usize)>> = BTreeMap::new();
        for (out, inp) in snarl.wires() {
            if can_connect(&nodes[&out.node.0], out.output, &nodes[&inp.node.0], inp.input) {
                wires.entry((out.node.0, out.output)).or_default().push((inp.node.0, inp.input));
            }
        }
        wires.values_mut().for_each(|targets| targets.sort());
        Self { nodes, wires }
    }

    pub fn targets(&self, node: usize, output: usize) -> &[(usize, usize)] {
        self.wires.get(&(node, output)).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_round_trips_through_snarl_and_toml() {
        let file: ShowFile = toml::from_str(include_str!("../../../showui.example.toml")).unwrap();
        let (snarl, problems) = file.to_snarl();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(snarl.wires().count(), file.wires.len());
        let again = ShowFile::from_snarl(&snarl, file.osc.clone(), file.sound.clone());
        let text = toml::to_string_pretty(&again).unwrap();
        assert_eq!(toml::from_str::<ShowFile>(&text).unwrap(), again);
        assert_eq!(again.nodes.len(), file.nodes.len());
    }

    #[test]
    fn plug_seconds_from_older_files_become_milliseconds() {
        let mut value: toml::Value = toml::from_str("[[node]]\nid = 1\npos = [0.0, 0.0]\nblock = \"plug\"\nhost = \"h\"\non_secs = 1.5\n").unwrap();
        migrate(&mut value);
        let file: ShowFile = value.try_into().unwrap();
        assert_eq!(file.nodes[0].node, Node::Plug { host: "h".into(), on_ms: 1500 });
    }

    #[test]
    fn pins_only_connect_to_the_same_kind() {
        let osc = Node::OscIn { address: "/v".into(), args: 1 };
        let bg = Node::Background { file: None, volume: 0.5, device: None };
        assert!(can_connect(&osc, 0, &bg, PLAY));
        assert!(can_connect(&osc, 1, &bg, VOLUME));
        assert!(!can_connect(&osc, 0, &bg, VOLUME));
        assert!(!can_connect(&osc, 1, &bg, PLAY));
        assert!(!can_connect(&osc, 2, &bg, PLAY), "arg2 does not exist");
    }

    #[test]
    fn mismatched_wires_in_a_file_are_skipped() {
        let file = ShowFile {
            nodes: vec![
                NodeEntry { id: 1, pos: [0.0, 0.0], node: Node::Key { key: "Space".into() } },
                NodeEntry { id: 2, pos: [200.0, 0.0], node: Node::Sound { file: None, volume: 1.0, device: None } },
            ],
            wires: vec![Wire { from: [1, 0], to: [2, VOLUME] }, Wire { from: [1, 0], to: [3, 0] }, Wire { from: [1, 0], to: [2, PLAY] }],
            ..Default::default()
        };
        let (snarl, problems) = file.to_snarl();
        assert_eq!(problems.len(), 2);
        let graph = Graph::compile(&snarl);
        assert_eq!(graph.wires.values().map(Vec::len).sum::<usize>(), 1);
    }
}
