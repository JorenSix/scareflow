//! Runs the show graph off the UI thread. Key and OSC events fire their event
//! blocks; triggers then travel along cables through flow blocks to response
//! blocks. Values (OSC arguments) travel along value cables and update the
//! settings they are wired to. Delays and plug auto-off use one timed queue,
//! so nothing here ever sleeps.

use std::collections::{BTreeMap, HashMap};
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use scareflow_osc::tasmota::TasmotaClient;
use scareflow_osc::wled::WledResponder;
use scareflow_osc::{Action, Num, OscEvent, OscType, Responder, WledAction, WledCommand};
use scareflow_sound::{SoundConfig, SoundEngine, SoundHandle};
use rosc::address::{Matcher, OscAddress};

use crate::graph::{GO, Graph, LEVEL, Node, OFF, ON, PLAY, STOP, VOLUME, WledDo};

/// Longest chain of blocks one trigger may pass without a delay; stops zero-delay loops.
const MAX_HOPS: u32 = 64;
/// A block (or one of its inputs) accepts at most one trigger per this window,
/// so a doubled key press or OSC message never plays a sound twice or sends a
/// device the same command twice.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// Pin key for an event block's own firing in `last_trigger`.
const EVENT: usize = usize::MAX;

pub enum Msg {
    /// The graph changed in the editor; later events use it.
    Graph(Arc<Graph>),
    Key(String),
    Osc(OscEvent),
    /// Fire event block `n` as if its event happened.
    Fire(usize),
    /// Cancel delayed triggers and silence everything: effects stop, music fades out.
    /// Plugs keep their state and auto-off timer.
    Panic,
}

pub enum Report {
    Log(String),
    /// Block `n` received or sent a trigger.
    Fired(usize),
    /// The Background block now playing on an output device (`None` = default device),
    /// or nothing playing there.
    Playing(Option<String>, Option<usize>),
}

pub struct Runner {
    pub tx: Sender<Msg>,
    pub reports: Receiver<Report>,
}

impl Runner {
    /// Starts the runner thread. `sound` is the template for each output device's engine.
    /// `notify` is called after every report so the UI can repaint.
    pub fn spawn(graph: Graph, sound: SoundConfig, notify: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let (report_tx, reports) = mpsc::channel();
        thread::Builder::new()
            .name("scareflow-showui-runner".into())
            .spawn(move || {
                let out = Out { tx: report_tx, notify: Box::new(notify) };
                State::new(graph, Some(sound), out).run(rx);
            })
            .expect("spawn runner thread");
        Self { tx, reports }
    }
}

struct Out {
    tx: Sender<Report>,
    notify: Box<dyn Fn() + Send>,
}

impl Out {
    fn send(&self, report: Report) {
        if self.tx.send(report).is_ok() {
            (self.notify)();
        }
    }

    fn log(&self, line: impl Into<String>) {
        self.send(Report::Log(line.into()));
    }
}

#[derive(Debug, PartialEq)]
enum Timed {
    /// Output `1` of block `0` fires (a Delay finishing).
    Fire(usize, usize),
    /// Plug block switches off, unless it was switched again since (generation).
    PlugOff(usize, u64),
}

struct State {
    graph: Arc<Graph>,
    matchers: BTreeMap<usize, Matcher>,
    /// Engine template; `None` disables audio (tests).
    sound: Option<SoundConfig>,
    /// One engine per output device, started on first use; `None` if it failed to start.
    engines: HashMap<Option<String>, Option<SoundHandle>>,
    wled: WledResponder,
    wled_hosts: Vec<String>,
    plugs: TasmotaClient,
    plug_generation: HashMap<usize, u64>,
    socket: Option<UdpSocket>,
    /// Latest value on each value input, `(node, input)`.
    values: HashMap<(usize, usize), f32>,
    cooldowns: HashMap<usize, Instant>,
    /// Background block playing on each output device.
    playing: HashMap<Option<String>, usize>,
    /// Last accepted trigger per `(node, input)`; see [`DEBOUNCE`].
    last_trigger: HashMap<(usize, usize), Instant>,
    debounce: Duration,
    pending: Vec<(Instant, Timed)>,
    rng: u64,
    out: Out,
}

impl State {
    fn new(graph: Graph, sound: Option<SoundConfig>, out: Out) -> Self {
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| out.log(format!("OSC out disabled: {e}"))).ok();
        let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        let mut state = Self {
            graph: Arc::default(),
            matchers: BTreeMap::new(),
            sound,
            engines: HashMap::new(),
            wled: WledResponder::new([], Duration::from_secs(2)),
            wled_hosts: Vec::new(),
            plugs: TasmotaClient::new(Duration::from_secs(3)),
            plug_generation: HashMap::new(),
            socket,
            values: HashMap::new(),
            cooldowns: HashMap::new(),
            playing: HashMap::new(),
            last_trigger: HashMap::new(),
            debounce: DEBOUNCE,
            pending: Vec::new(),
            rng: seed | 1,
            out,
        };
        state.set_graph(Arc::new(graph));
        state
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let timeout = self.pending.iter().map(|p| p.0).min().map(|due| due.saturating_duration_since(Instant::now()));
            let msg = match timeout {
                Some(t) => rx.recv_timeout(t),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match msg {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.run_due();
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Graph(graph) => self.set_graph(graph),
            Msg::Key(key) => {
                let hits: Vec<usize> = self.graph.nodes.iter().filter(|(_, n)| matches!(n, Node::Key { key: k } if *k == key)).map(|(id, _)| *id).collect();
                if hits.is_empty() {
                    self.out.log(format!("key {key}: no Key block"));
                }
                for node in hits {
                    self.out.log(format!("key {key}"));
                    self.trigger_event(node);
                }
            }
            Msg::Osc(event) => self.osc(&event),
            Msg::Fire(node) => match self.graph.nodes.get(&node) {
                Some(Node::OscIn { address, .. }) => self.osc_into(node, &OscEvent::new(address.clone(), vec![])),
                Some(Node::Key { .. } | Node::Button { .. }) => self.trigger_event(node),
                _ => {}
            },
            Msg::Panic => {
                let cancelled = self.pending.iter().filter(|p| matches!(p.1, Timed::Fire(..))).count();
                self.pending.retain(|p| matches!(p.1, Timed::PlugOff(..)));
                for (device, engine) in &self.engines {
                    let Some(engine) = engine else { continue };
                    let _ = engine.stop_effects();
                    let _ = engine.stop_background();
                    self.out.send(Report::Playing(device.clone(), None));
                }
                self.playing.clear();
                self.out.log(format!("stopped all sound, cancelled {cancelled} delayed trigger(s)"));
            }
        }
    }

    fn set_graph(&mut self, graph: Arc<Graph>) {
        let mut hosts: Vec<String> =
            graph.nodes.values().filter_map(|n| if let Node::Wled { host, .. } = n { Some(host.clone()) } else { None }).collect();
        hosts.sort();
        hosts.dedup();
        if hosts != self.wled_hosts {
            self.wled = WledResponder::new(hosts.iter().map(|h| (h.clone(), h.clone())), Duration::from_secs(2));
            self.wled_hosts = hosts;
        }
        self.matchers = graph
            .nodes
            .iter()
            .filter_map(|(id, n)| match n {
                Node::OscIn { address, .. } => Matcher::new(address).ok().map(|m| (*id, m)),
                _ => None,
            })
            .collect();
        self.follow_playing(&graph);
        self.preload(&graph);
        // Forget state of blocks that no longer exist.
        self.values.retain(|(node, _), _| graph.nodes.contains_key(node));
        self.cooldowns.retain(|node, _| graph.nodes.contains_key(node));
        self.last_trigger.retain(|(node, _), _| graph.nodes.contains_key(node));
        self.graph = graph;
    }

    /// Keeps the music in step with edits to the Background block that is playing:
    /// a moved volume slider applies now (replacing any OSC volume), and deleting
    /// the block stops its music.
    fn follow_playing(&mut self, graph: &Graph) {
        for (device, node) in self.playing.clone() {
            match graph.nodes.get(&node) {
                Some(Node::Background { volume, device: d, .. }) if *d == device => {
                    let before = match self.graph.nodes.get(&node) {
                        Some(Node::Background { volume, .. }) => Some(*volume),
                        _ => None,
                    };
                    if before != Some(*volume) {
                        self.values.remove(&(node, VOLUME));
                        if let Some(engine) = self.engine(&device) {
                            let _ = engine.set_background_volume(*volume);
                        }
                    }
                }
                _ => {
                    if let Some(engine) = self.engine(&device) {
                        let _ = engine.stop_background();
                    }
                    self.playing.remove(&device);
                    self.out.send(Report::Playing(device, None));
                }
            }
        }
    }

    fn osc(&mut self, event: &OscEvent) {
        let hits: Vec<usize> = match OscAddress::new(event.address.clone()) {
            Ok(address) => self.matchers.iter().filter(|(_, m)| m.match_address(&address)).map(|(id, _)| *id).collect(),
            Err(_) => Vec::new(),
        };
        if hits.is_empty() {
            self.out.log(format!("OSC {}: no OSC in block", event.address));
        }
        for node in hits {
            self.osc_into(node, event);
        }
    }

    /// Values first, so responses triggered by this message already see them.
    fn osc_into(&mut self, node: usize, event: &OscEvent) {
        let Some(Node::OscIn { args, .. }) = self.graph.nodes.get(&node) else { return };
        for i in 0..*args {
            if let Some(v) = event.arg_f32(i) {
                for (to, input) in self.graph.targets(node, i + 1).to_vec() {
                    self.set_value(to, input, v);
                }
            }
        }
        self.out.log(format!("OSC {}", event.address));
        self.trigger_event(node);
    }

    fn trigger_event(&mut self, node: usize) {
        if self.bounced(node, EVENT) {
            return;
        }
        self.out.send(Report::Fired(node));
        self.fire(node, 0, 0);
    }

    fn fire(&mut self, node: usize, output: usize, hops: u32) {
        for (to, input) in self.graph.targets(node, output).to_vec() {
            self.deliver(to, input, hops + 1);
        }
    }

    fn deliver(&mut self, node: usize, input: usize, hops: u32) {
        if hops > MAX_HOPS {
            self.out.log(format!("stopped a loop without delay at block {node}"));
            return;
        }
        let Some(block) = self.graph.nodes.get(&node).cloned() else { return };
        if self.bounced(node, input) {
            return;
        }
        self.out.send(Report::Fired(node));
        let now = Instant::now();
        match block {
            Node::Key { .. } | Node::OscIn { .. } => {}
            Node::Delay { ms } => self.pending.push((now + Duration::from_millis(ms), Timed::Fire(node, 0))),
            Node::Random { outputs } => {
                let wired: Vec<usize> = (0..outputs).filter(|o| !self.graph.targets(node, *o).is_empty()).collect();
                if !wired.is_empty() {
                    let pick = wired[(self.next_random() % wired.len() as u64) as usize];
                    self.fire(node, pick, hops);
                }
            }
            Node::Cooldown { secs } => {
                let ready = self.cooldowns.get(&node).is_none_or(|last| now.duration_since(*last).as_secs_f32() >= secs);
                if ready {
                    self.cooldowns.insert(node, now);
                    self.fire(node, 0, hops);
                }
            }
            Node::Plug { host, on_ms } => self.plug(node, &host, input == ON, on_ms, hops),
            response => {
                if let Err(e) = self.respond(node, input, response) {
                    self.out.log(format!("block {node}: {e}"));
                }
            }
        }
    }

    /// True (and logged) when `(node, input)` already took a trigger within the debounce window.
    fn bounced(&mut self, node: usize, input: usize) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_trigger.get(&(node, input))
            && now.duration_since(*last) < self.debounce
        {
            let pin = if input == EVENT { "event".to_owned() } else { format!("input {input}") };
            self.out.log(format!("ignored a second trigger on block {node} ({pin}) within {} ms", self.debounce.as_millis()));
            return true;
        }
        self.last_trigger.insert((node, input), now);
        false
    }

    fn respond(&mut self, node: usize, input: usize, block: Node) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let volume = |default: f32| self.values.get(&(node, VOLUME)).copied().unwrap_or(default);
        match (block, input) {
            (Node::Background { file, volume: v, device }, PLAY) => {
                let file = file.ok_or("no background file chosen")?;
                let v = volume(v);
                let engine = self.engine(&device).ok_or("no audio output")?;
                engine.set_background_volume(v)?;
                engine.play_background(&file)?;
                self.playing.insert(device.clone(), node);
                self.out.send(Report::Playing(device, Some(node)));
            }
            (Node::Background { device, .. }, STOP) => {
                self.engine(&device).ok_or("no audio output")?.stop_background()?;
                self.playing.remove(&device);
                self.out.send(Report::Playing(device, None));
            }
            (Node::Sound { file, volume: v, device }, PLAY) => {
                let file = file.ok_or("no sound file chosen")?;
                let v = volume(v);
                self.engine(&device).ok_or("no audio output")?.play_effect(file, v)?;
            }
            (Node::Sound { device, .. }, STOP) => self.engine(&device).ok_or("no audio output")?.stop_effects()?,
            (Node::Wled { host, action }, GO) => {
                let cmd = match action {
                    WledDo::Lightning { duration_ms } => WledCommand::Lightning { duration_ms: Num::Value(duration_ms as f32), col: [255; 4] },
                    WledDo::Effect { fx, speed, intensity } => WledCommand::Effect {
                        fx: Num::Value(fx.into()),
                        sx: Some(Num::Value(speed.into())),
                        ix: Some(Num::Value(intensity.into())),
                        col: None,
                    },
                    WledDo::Preset { preset } => WledCommand::Preset { preset: Num::Value(preset.into()) },
                    WledDo::Power { on } => WledCommand::Power { on },
                };
                self.wled_send(host, cmd)?;
            }
            (Node::OscOut { to, address, mut args }, GO) => {
                if let Some(v) = self.values.get(&(node, LEVEL)) {
                    match args.first_mut() {
                        Some(first) => *first = *v,
                        None => args.push(*v),
                    }
                }
                let socket = self.socket.as_ref().ok_or("OSC out disabled")?;
                let addr = to.to_socket_addrs()?.next().ok_or_else(|| format!("cannot resolve {to}"))?;
                scareflow_osc::osc_out::send(socket, addr, &address, args.into_iter().map(OscType::Float).collect())?;
            }
            _ => {}
        }
        Ok(())
    }

    /// A value arrived on a value input. Live settings apply now; others on the next trigger.
    fn set_value(&mut self, node: usize, input: usize, v: f32) {
        self.values.insert((node, input), v);
        let result = match self.graph.nodes.get(&node).cloned() {
            // Only the block that is playing may change the music's volume.
            Some(Node::Background { device, .. }) if input == VOLUME && self.playing.get(&device) == Some(&node) => {
                match self.engine(&device) {
                    Some(engine) => engine.set_background_volume(v.max(0.0)).map_err(Into::into),
                    None => Ok(()),
                }
            }
            Some(Node::Wled { host, .. }) if input == LEVEL => {
                // OSC faders send 0–1; anything above 1 is taken as a 0–255 brightness.
                let bri = if v <= 1.0 { v * 255.0 } else { v };
                self.wled_send(host, WledCommand::Brightness { bri: Num::Value(bri.clamp(0.0, 255.0)) })
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.out.log(format!("block {node}: {e}"));
        }
    }

    fn plug(&mut self, node: usize, host: &str, on: bool, on_ms: u64, hops: u32) {
        let generation = self.plug_generation.entry(node).or_default();
        *generation += 1;
        let generation = *generation;
        if let Err(e) = self.plugs.power(host, on) {
            self.out.log(format!("plug {host}: {e}"));
            return;
        }
        self.out.log(format!("plug {host} {}", if on { "on" } else { "off" }));
        if on && on_ms > 0 {
            self.pending.push((Instant::now() + Duration::from_millis(on_ms), Timed::PlugOff(node, generation)));
        }
        self.fire(node, if on { ON } else { OFF }, hops);
    }

    fn run_due(&mut self) {
        let now = Instant::now();
        let (mut due, later): (Vec<_>, Vec<_>) = self.pending.drain(..).partition(|p| p.0 <= now);
        self.pending = later;
        due.sort_by_key(|p| p.0);
        for (_, timed) in due {
            match timed {
                Timed::Fire(node, output) => {
                    self.out.send(Report::Fired(node));
                    self.fire(node, output, 0);
                }
                Timed::PlugOff(node, generation) => {
                    if self.plug_generation.get(&node) == Some(&generation)
                        && let Some(Node::Plug { host, .. }) = self.graph.nodes.get(&node).cloned()
                    {
                        self.out.send(Report::Fired(node));
                        self.plug(node, &host, false, 0, 0);
                    }
                }
            }
        }
    }

    /// Opens every output device and decodes every effect now, so the first
    /// trigger does not wait for the device or the decoder.
    fn preload(&mut self, graph: &Graph) {
        for node in graph.nodes.values() {
            let (Node::Sound { file, device, .. } | Node::Background { file, device, .. }) = node else { continue };
            let Some(engine) = self.engine(device) else { continue };
            if let (Node::Sound { .. }, Some(file)) = (node, file) {
                let _ = engine.preload(file);
            }
        }
    }

    fn engine(&mut self, device: &Option<String>) -> Option<&SoundHandle> {
        let template = self.sound.as_ref()?;
        let out = &self.out;
        self.engines
            .entry(device.clone())
            .or_insert_with(|| {
                let config = SoundConfig { device: device.clone(), ..template.clone() };
                SoundEngine::start(config).map_err(|e| out.log(format!("audio output {device:?}: {e}"))).ok()
            })
            .as_ref()
    }

    fn wled_send(&mut self, host: String, cmd: WledCommand) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.wled.handle(&Action::Wled(WledAction { target: host, cmd }), &OscEvent::new("/panel", vec![]))
    }

    fn next_random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(nodes: Vec<(usize, Node)>, wires: Vec<((usize, usize), (usize, usize))>) -> (State, Receiver<Report>) {
        let mut graph = Graph { nodes: nodes.into_iter().collect(), wires: BTreeMap::new() };
        for (from, to) in wires {
            graph.wires.entry(from).or_default().push(to);
        }
        let (tx, rx) = mpsc::channel();
        let mut state = State::new(graph, None, Out { tx, notify: Box::new(|| {}) });
        state.debounce = Duration::ZERO;
        (state, rx)
    }

    #[test]
    fn second_trigger_within_300_ms_is_ignored() {
        let (mut s, rx) = state(vec![(1, Node::Key { key: "Space".into() }), (2, delay(10)), (3, delay(10))], vec![((1, 0), (2, 0)), ((1, 0), (3, 0))]);
        s.debounce = DEBOUNCE;
        s.handle(Msg::Key("Space".into()));
        s.handle(Msg::Key("Space".into()));
        assert_eq!(s.pending.len(), 2, "one trigger reached each delay");
        assert!(rx.try_iter().any(|r| matches!(r, Report::Log(l) if l.contains("ignored a second trigger"))));
        // Two different events into the same block within the window: only the first passes.
        s.last_trigger.clear();
        s.pending.clear();
        s.deliver(2, 0, 0);
        s.deliver(2, 0, 0);
        assert_eq!(s.pending.len(), 1);
    }

    fn delay(ms: u64) -> Node {
        Node::Delay { ms }
    }

    #[test]
    fn key_through_delay_schedules_its_output() {
        let (mut s, _rx) = state(vec![(1, Node::Key { key: "Space".into() }), (2, delay(300))], vec![((1, 0), (2, 0))]);
        let start = Instant::now();
        s.handle(Msg::Key("Space".into()));
        assert_eq!(s.pending.len(), 1);
        assert_eq!(s.pending[0].1, Timed::Fire(2, 0));
        let wait = s.pending[0].0 - start;
        assert!(wait >= Duration::from_millis(300) && wait < Duration::from_millis(400));
    }

    #[test]
    fn random_fires_exactly_one_connected_output() {
        // Output 2 of the random block is left unconnected and must never be picked.
        let (mut s, _rx) = state(
            vec![(1, Node::Random { outputs: 3 }), (2, delay(10)), (3, delay(20))],
            vec![((1, 0), (2, 0)), ((1, 1), (3, 0))],
        );
        for _ in 0..50 {
            s.deliver(1, 0, 0);
            assert_eq!(s.pending.len(), 1);
            assert!(matches!(s.pending[0].1, Timed::Fire(2 | 3, 0)));
            s.pending.clear();
        }
    }

    #[test]
    fn button_fires_its_output() {
        let (mut s, _rx) = state(vec![(1, Node::Button { label: "Stop".into() }), (2, delay(10))], vec![((1, 0), (2, 0))]);
        s.handle(Msg::Fire(1));
        assert_eq!(s.pending.len(), 1);
    }

    #[test]
    fn deleting_the_playing_background_reports_it_stopped() {
        let bg = Node::Background { file: None, volume: 0.4, device: None };
        let (mut s, rx) = state(vec![(1, bg)], vec![]);
        s.playing.insert(None, 1);
        s.set_graph(Arc::new(Graph::default()));
        assert!(s.playing.is_empty());
        assert!(rx.try_iter().any(|r| matches!(r, Report::Playing(None, None))));
    }

    #[test]
    fn slider_change_on_the_playing_background_replaces_its_osc_volume() {
        let bg = |volume| Node::Background { file: None, volume, device: None };
        let (mut s, _rx) = state(vec![(1, bg(0.4)), (2, bg(0.4))], vec![]);
        s.playing.insert(None, 1);
        s.values.insert((1, VOLUME), 0.9);
        s.values.insert((2, VOLUME), 0.9);
        let moved = Graph { nodes: [(1, bg(0.7)), (2, bg(0.2))].into_iter().collect(), wires: BTreeMap::new() };
        s.set_graph(Arc::new(moved));
        assert_eq!(s.values.get(&(1, VOLUME)), None, "the playing block's slider wins");
        assert_eq!(s.values.get(&(2, VOLUME)), Some(&0.9), "blocks that are not playing keep their value");
        assert_eq!(s.playing.get(&None), Some(&1));
    }

    #[test]
    fn cooldown_drops_triggers_inside_the_window() {
        let (mut s, _rx) = state(vec![(1, Node::Cooldown { secs: 60.0 }), (2, delay(10))], vec![((1, 0), (2, 0))]);
        s.deliver(1, 0, 0);
        s.deliver(1, 0, 0);
        assert_eq!(s.pending.len(), 1);
    }

    #[test]
    fn osc_argument_reaches_a_value_input_without_a_trigger() {
        let (mut s, _rx) = state(
            vec![(1, Node::OscIn { address: "/pumpkin/vol".into(), args: 1 }), (2, Node::Background { file: None, volume: 0.4, device: None })],
            vec![((1, 1), (2, VOLUME))],
        );
        s.handle(Msg::Osc(OscEvent::new("/pumpkin/vol", vec![OscType::Float(0.25)])));
        assert_eq!(s.values.get(&(2, VOLUME)), Some(&0.25));
    }

    #[test]
    fn loop_without_delay_stops_at_the_hop_limit() {
        let (mut s, rx) = state(vec![(1, Node::Cooldown { secs: 0.0 }), (2, Node::Cooldown { secs: 0.0 })], vec![((1, 0), (2, 0)), ((2, 0), (1, 0))]);
        s.deliver(1, 0, 0);
        assert!(rx.try_iter().any(|r| matches!(r, Report::Log(l) if l.contains("stopped a loop"))));
    }

    #[test]
    fn plug_switches_off_after_its_time_unless_switched_again() {
        let (mut s, _rx) = state(vec![(1, Node::Plug { host: "127.0.0.1:9".into(), on_ms: 30_000 })], vec![]);
        s.deliver(1, ON, 0);
        s.deliver(1, ON, 0);
        assert_eq!(s.pending.len(), 2);
        assert_eq!(s.pending[1].1, Timed::PlugOff(1, 2), "the second switch-on owns the timer");
        // The first timer is stale: running it must not switch off.
        s.pending.iter_mut().for_each(|p| p.0 = Instant::now());
        s.pending.truncate(1);
        s.run_due();
        assert_eq!(s.plug_generation[&1], 2);
    }

    #[test]
    fn panic_cancels_delays_but_keeps_plug_timers() {
        let (mut s, rx) = state(vec![(1, delay(10_000)), (2, Node::Plug { host: "127.0.0.1:9".into(), on_ms: 30_000 })], vec![]);
        s.deliver(1, 0, 0);
        s.deliver(2, ON, 0);
        s.handle(Msg::Panic);
        assert_eq!(s.pending.len(), 1);
        assert!(matches!(s.pending[0].1, Timed::PlugOff(2, 1)));
        assert!(rx.try_iter().any(|r| matches!(r, Report::Log(l) if l.contains("cancelled 1"))));
    }
}
