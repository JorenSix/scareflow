//! Responder for WLED controllers via the JSON API (`POST /json/state`).
//!
//! Requests are queued to a worker thread so an unreachable controller never
//! stalls OSC dispatch; when the queue is full new commands are dropped.
//! Lightning storms run on their own thread per controller: they save the
//! state, flash every segment, and restore the state when they end.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::osc_out::{lightning_schedule, seed};
use crate::{Action, OscEvent, Responder, ResponderError, WledCommand, resolve_text};

const QUEUE_LEN: usize = 64;

pub struct WledResponder {
    /// Controller name → `/json/state` URL.
    targets: HashMap<String, String>,
    queue: SyncSender<(String, Value)>,
    agent: ureq::Agent,
    storms: HashMap<String, Arc<Storm>>,
}

/// Lightning on one controller. A newer storm bumps `generation`, which stops the
/// older one without restoring, so the state saved before the first storm survives.
#[derive(Default)]
struct Storm {
    generation: AtomicU64,
    saved: Mutex<Option<Value>>,
}

impl WledResponder {
    /// `targets` maps controller names to hosts (`"192.168.1.50"`, `"wled.local:8080"`
    /// or a full `http://…` base URL).
    pub fn new(targets: impl IntoIterator<Item = (String, String)>, timeout: Duration) -> Self {
        let targets = targets.into_iter().map(|(name, host)| (name, state_url(&host))).collect();
        let (queue, rx) = mpsc::sync_channel::<(String, Value)>(QUEUE_LEN);
        let agent = ureq::AgentBuilder::new().timeout(timeout).build();
        let worker_agent = agent.clone();
        thread::Builder::new()
            .name("scareflow-wled".into())
            .spawn(move || {
                let agent = worker_agent;
                for (url, body) in rx {
                    let result = agent
                        .post(&url)
                        .set("Content-Type", "application/json")
                        .send_string(&body.to_string());
                    match result {
                        Ok(_) => log::debug!("WLED {url} <- {body}"),
                        Err(e) => log::warn!("WLED {url} failed: {e}"),
                    }
                }
            })
            .expect("spawn WLED worker");
        Self { targets, queue, agent, storms: HashMap::new() }
    }

    fn lightning(&mut self, url: &str, duration: Duration, col: [u8; 4]) -> Result<(), ResponderError> {
        let storm = self.storms.entry(url.to_owned()).or_default().clone();
        let generation = storm.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (agent, url) = (self.agent.clone(), url.to_owned());
        let schedule = lightning_schedule(seed(), duration);
        thread::Builder::new()
            .name("scareflow-wled-lightning".into())
            .spawn(move || run_storm(&agent, &url, &storm, generation, &schedule, col))?;
        Ok(())
    }
}

fn run_storm(agent: &ureq::Agent, url: &str, storm: &Storm, generation: u64, schedule: &[(Duration, bool)], col: [u8; 4]) {
    let current = || storm.generation.load(Ordering::SeqCst) == generation;
    let segments = {
        let mut saved = storm.saved.lock().unwrap_or_else(|e| e.into_inner());
        if saved.is_none() {
            let state = agent.get(url).call().map_err(|e| e.to_string()).and_then(|r| r.into_string().map_err(|e| e.to_string()));
            match state.and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string())) {
                Ok(state) => *saved = Some(state),
                Err(e) => log::warn!("WLED {url}: cannot read state before lightning, will end off: {e}"),
            }
        }
        segment_ids(saved.as_ref())
    };
    let (flash, dark) = (flash_state(&segments, col), json!({ "on": false, "tt": 0 }));
    let mut at = Instant::now();
    for (wait, lit) in schedule {
        at += *wait;
        thread::sleep(at.saturating_duration_since(Instant::now()));
        if !current() {
            return;
        }
        post(agent, url, if *lit { &flash } else { &dark });
    }
    // Restore under the lock so a storm starting now saves the restored state, not a flash.
    let mut saved = storm.saved.lock().unwrap_or_else(|e| e.into_inner());
    if current() {
        post(agent, url, &restore_state(saved.take()));
    }
}

fn post(agent: &ureq::Agent, url: &str, body: &Value) {
    if let Err(e) = agent.post(url).set("Content-Type", "application/json").send_string(&body.to_string()) {
        log::warn!("WLED {url} failed: {e}");
    }
}

/// Ids of the segments in a `/json/state` reply; segment 0 when unknown.
fn segment_ids(state: Option<&Value>) -> Vec<u64> {
    let ids: Vec<u64> = state
        .and_then(|s| s["seg"].as_array())
        .map(|segs| segs.iter().filter_map(|g| g["id"].as_u64()).collect())
        .unwrap_or_default();
    if ids.is_empty() { vec![0] } else { ids }
}

/// Full brightness, solid colour on every segment, without transition.
fn flash_state(segments: &[u64], col: [u8; 4]) -> Value {
    let seg: Vec<Value> = segments
        .iter()
        .map(|id| json!({ "id": id, "on": true, "bri": 255, "frz": false, "fx": 0, "col": [col, [0, 0, 0, 0], [0, 0, 0, 0]] }))
        .collect();
    json!({ "on": true, "bri": 255, "tt": 0, "seg": seg })
}

/// The saved power, brightness and segments, applied instantly; off if nothing was saved.
fn restore_state(saved: Option<Value>) -> Value {
    let Some(saved) = saved else {
        return json!({ "on": false, "tt": 0 });
    };
    let mut state = json!({ "tt": 0 });
    for key in ["on", "bri", "seg"] {
        if let Some(v) = saved.get(key) {
            state[key] = v.clone();
        }
    }
    state
}

impl Responder for WledResponder {
    fn name(&self) -> &str {
        "wled"
    }

    fn handle(&mut self, action: &Action, event: &OscEvent) -> Result<(), ResponderError> {
        let Action::Wled(wled) = action else {
            return Err(format!("unsupported action {:?}", action.kind()).into());
        };
        let target = resolve_text(&wled.target, event).ok_or("missing WLED target argument")?;
        let url = self.targets.get(&target).ok_or_else(|| format!("unknown WLED target {target:?}"))?.clone();
        if let WledCommand::Lightning { duration_ms, col } = &wled.cmd {
            let ms = duration_ms.get(event).ok_or("missing duration")?.max(0.0);
            return self.lightning(&url, Duration::from_millis(ms as u64), *col);
        }
        let body = payload(&wled.cmd, event)?;
        match self.queue.try_send((url.clone(), body)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("WLED queue full, command dropped".into()),
            Err(TrySendError::Disconnected(_)) => Err("WLED worker stopped".into()),
        }
    }
}

fn state_url(host: &str) -> String {
    let base = if host.starts_with("http://") || host.starts_with("https://") {
        host.trim_end_matches('/').to_owned()
    } else {
        format!("http://{host}")
    };
    format!("{base}/json/state")
}

/// JSON body for `/json/state`.
pub fn payload(cmd: &WledCommand, event: &OscEvent) -> Result<Value, ResponderError> {
    let num = |n: &crate::Num, what: &str| -> Result<i64, ResponderError> {
        n.get(event).map(|v| v.round() as i64).ok_or_else(|| format!("missing {what}").into())
    };
    Ok(match cmd {
        WledCommand::Preset { preset } => json!({ "ps": num(preset, "preset")? }),
        WledCommand::Power { on } => json!({ "on": on }),
        WledCommand::Brightness { bri } => json!({ "on": true, "bri": num(bri, "brightness")?.clamp(0, 255) }),
        WledCommand::Effect { fx, sx, ix, col } => {
            let mut seg = json!({ "fx": num(fx, "effect")? });
            if let Some(sx) = sx {
                seg["sx"] = json!(num(sx, "speed")?.clamp(0, 255));
            }
            if let Some(ix) = ix {
                seg["ix"] = json!(num(ix, "intensity")?.clamp(0, 255));
            }
            if let Some(col) = col {
                seg["col"] = json!([col]);
            }
            json!({ "on": true, "seg": [seg] })
        }
        WledCommand::Raw { json } => json.clone(),
        WledCommand::Lightning { .. } => return Err("lightning is a sequence of states, not one".into()),
    })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    use rosc::OscType;

    use super::*;
    use crate::{Num, WledAction};

    #[test]
    fn builds_payloads() {
        let ev = OscEvent::new("/x", vec![OscType::Float(4.0), OscType::Int(300)]);
        let p = |c| payload(&c, &ev).unwrap();
        assert_eq!(p(WledCommand::Preset { preset: Num::Arg("$1".into()) }), json!({"ps": 4}));
        assert_eq!(p(WledCommand::Power { on: false }), json!({"on": false}));
        assert_eq!(p(WledCommand::Brightness { bri: Num::Arg("$2".into()) }), json!({"on": true, "bri": 255}));
        assert_eq!(
            p(WledCommand::Effect { fx: Num::Value(9.0), sx: Some(Num::Value(128.0)), ix: None, col: Some([255, 80, 0]) }),
            json!({"on": true, "seg": [{"fx": 9, "sx": 128, "col": [[255, 80, 0]]}]})
        );
        assert!(payload(&WledCommand::Preset { preset: Num::Arg("$5".into()) }, &ev).is_err());
        assert_eq!(state_url("10.0.0.2"), "http://10.0.0.2/json/state");
        assert_eq!(state_url("http://wled.local/"), "http://wled.local/json/state");
    }

    #[test]
    fn posts_to_controller() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = server.local_addr().unwrap().to_string();
        let mut responder = WledResponder::new([("porch".to_owned(), host)], Duration::from_secs(2));
        let action = Action::Wled(WledAction { target: "porch".into(), cmd: WledCommand::Preset { preset: Num::Value(3.0) } });
        responder.handle(&action, &OscEvent::new("/x", vec![])).unwrap();

        let (stream, _) = server.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();

        assert!(request_line.starts_with("POST /json/state "));
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!({"ps": 3}));
    }

    #[test]
    fn lightning_flashes_every_segment_then_restores() {
        let before = json!({ "on": false, "bri": 128, "error": 12, "seg": [{ "id": 0, "fx": 57 }, { "id": 1, "fx": 9 }] });
        assert_eq!(segment_ids(Some(&before)), [0, 1]);
        assert_eq!(segment_ids(None), [0]);
        let flash = flash_state(&[0, 1], [255; 4]);
        assert_eq!(flash["on"], json!(true));
        assert_eq!(flash["seg"][1], json!({ "id": 1, "on": true, "bri": 255, "frz": false, "fx": 0, "col": [[255, 255, 255, 255], [0, 0, 0, 0], [0, 0, 0, 0]] }));
        assert_eq!(restore_state(Some(before.clone())), json!({ "tt": 0, "on": false, "bri": 128, "seg": before["seg"] }));
        assert_eq!(restore_state(None), json!({ "on": false, "tt": 0 }));
    }

    #[test]
    fn unknown_target_is_an_error() {
        let mut responder = WledResponder::new([], Duration::from_secs(1));
        let action = Action::Wled(WledAction { target: "nope".into(), cmd: WledCommand::Power { on: true } });
        assert!(responder.handle(&action, &OscEvent::new("/x", vec![])).is_err());
    }
}
