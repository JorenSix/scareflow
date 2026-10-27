//! Standalone Scareflow show daemon: listens for OSC and plays sounds, drives WLED
//! controllers and sends OSC to other devices according to `show.toml`.
//!
//! cargo run -p scareflow-showd -- show.toml

mod config;

use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use scareflow_osc::osc_out::OscOutResponder;
use scareflow_osc::wled::WledResponder;
use scareflow_osc::{
    Action, Num, OscEvent, OscListener, OscOutAction, OscOutCommand, Responder, ResponderError, Router, SoundAction,
    WledAction, WledCommand, resolve_text,
};
use scareflow_sound::{SoundEngine, SoundHandle};

use crate::config::ShowConfig;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let path = std::env::args().nth(1).unwrap_or_else(|| "show.toml".into());
    match run(Path::new(&path)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(path: &Path) -> Result<(), ResponderError> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let config: ShowConfig = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));

    let mut router = Router::new();
    if let Some(sound) = &config.sound {
        match SoundEngine::start(sound.engine_config(base)) {
            Ok(handle) => {
                if let Some(bg) = &sound.background {
                    handle.play_background(bg)?;
                }
                router.register("sound", SoundResponder(handle));
            }
            Err(e) => log::warn!("sound disabled: {e}"),
        }
    }
    router.register(
        "wled",
        WledResponder::new(config.wled.iter().map(|(k, v)| (k.clone(), v.host.clone())), Duration::from_secs(2)),
    );
    router.register("osc_out", OscOutResponder::new(config.osc_out.iter().map(|(k, v)| (k.clone(), v.addr.clone())))?);

    for route in config.routes {
        router.add_route(&route.address, route.actions).map_err(|e| format!("route {}: {e}", route.address))?;
    }
    if config.osc.builtin_routes {
        for (address, actions) in builtin_routes() {
            router.add_route(address, actions)?;
        }
    }

    let (tx, rx) = mpsc::channel();
    let _listener = OscListener::spawn(&config.osc.listen, tx)?;
    router.spawn(rx).join().map_err(|_| "router thread panicked")?;
    Ok(())
}

/// Direct control addresses, available without configuring routes. Arguments are `$1`, `$2`, …
fn builtin_routes() -> Vec<(&'static str, Vec<Action>)> {
    let arg = |n: &str| Num::Arg(n.into());
    vec![
        ("/pumpkin/sound/play", vec![Action::Sound(SoundAction::Play { file: "$1".into(), volume: Some(arg("$2")) })]),
        ("/pumpkin/sound/stop", vec![Action::Sound(SoundAction::StopEffects)]),
        ("/pumpkin/bg/play", vec![Action::Sound(SoundAction::Background { file: "$1".into() })]),
        ("/pumpkin/bg/volume", vec![Action::Sound(SoundAction::BackgroundVolume { volume: arg("$1") })]),
        ("/pumpkin/bg/stop", vec![Action::Sound(SoundAction::StopBackground)]),
        (
            "/pumpkin/wled/preset",
            vec![Action::Wled(WledAction { target: "$1".into(), cmd: WledCommand::Preset { preset: arg("$2") } })],
        ),
        (
            "/pumpkin/lightning",
            vec![Action::OscOut(OscOutAction {
                target: "$1".into(),
                cmd: OscOutCommand::Lightning {
                    address: "$2".into(),
                    on: Num::Value(1.0),
                    off: Num::Value(0.0),
                    duration_ms: arg("$3"),
                },
            })],
        ),
    ]
}

/// Adapts [`SoundAction`]s to the sound engine.
struct SoundResponder(SoundHandle);

impl Responder for SoundResponder {
    fn name(&self) -> &str {
        "sound"
    }

    fn handle(&mut self, action: &Action, event: &OscEvent) -> Result<(), ResponderError> {
        let Action::Sound(sound) = action else {
            return Err(format!("unsupported action {:?}", action.kind()).into());
        };
        let file = |f: &str| resolve_text(f, event).ok_or("missing file argument");
        match sound {
            SoundAction::Play { file: f, volume } => {
                let volume = volume.as_ref().and_then(|v| v.get(event)).unwrap_or(1.0);
                self.0.play_effect(file(f)?, volume)?;
            }
            SoundAction::Background { file: f } => self.0.play_background(file(f)?)?,
            SoundAction::BackgroundVolume { volume } => {
                self.0.set_background_volume(volume.get(event).ok_or("missing volume")?)?
            }
            SoundAction::StopBackground => self.0.stop_background()?,
            SoundAction::StopEffects => self.0.stop_effects()?,
        }
        Ok(())
    }
}
