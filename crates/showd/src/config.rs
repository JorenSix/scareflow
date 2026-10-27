use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use scareflow_osc::Action;
use scareflow_sound::SoundConfig;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowConfig {
    #[serde(default)]
    pub osc: OscSection,
    pub sound: Option<SoundSection>,
    /// WLED controllers by name.
    #[serde(default)]
    pub wled: HashMap<String, WledTarget>,
    /// OSC destinations (light controllers, other show software) by name.
    #[serde(default)]
    pub osc_out: HashMap<String, OscTarget>,
    #[serde(default, rename = "route")]
    pub routes: Vec<RouteConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OscSection {
    pub listen: String,
    /// Also install the `/pumpkin/...` routes documented in the README.
    #[serde(default = "yes")]
    pub builtin_routes: bool,
}

impl Default for OscSection {
    fn default() -> Self {
        Self { listen: "0.0.0.0:9000".into(), builtin_routes: true }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundSection {
    /// Relative to the config file.
    #[serde(default = "dot")]
    pub dir: PathBuf,
    /// Looping track started at launch, relative to `dir`.
    pub background: Option<PathBuf>,
    #[serde(default = "half")]
    pub background_volume: f32,
    pub duck_to: Option<f32>,
    #[serde(default = "eight")]
    pub max_voices: usize,
    #[serde(default = "fade_ms")]
    pub fade_ms: u64,
    /// Audio callback size in frames; 0 leaves it to the audio system.
    #[serde(default = "buffer_frames")]
    pub buffer_frames: u32,
}

impl SoundSection {
    pub fn engine_config(&self, base: &Path) -> SoundConfig {
        SoundConfig {
            dir: base.join(&self.dir),
            background_volume: self.background_volume,
            duck_to: self.duck_to,
            max_voices: self.max_voices,
            fade: Duration::from_millis(self.fade_ms),
            device: None,
            buffer_frames: (self.buffer_frames > 0).then_some(self.buffer_frames),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WledTarget {
    pub host: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OscTarget {
    /// `host:port`, e.g. `"192.168.1.102:8000"`.
    pub addr: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    pub address: String,
    pub actions: Vec<Action>,
}

fn yes() -> bool {
    true
}
fn dot() -> PathBuf {
    PathBuf::from(".")
}
fn half() -> f32 {
    0.5
}
fn eight() -> usize {
    8
}
fn fade_ms() -> u64 {
    800
}
fn buffer_frames() -> u32 {
    scareflow_sound::DEFAULT_BUFFER_FRAMES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_config() {
        let text = include_str!("../../../show.example.toml");
        let config: ShowConfig = toml::from_str(text).unwrap();
        assert_eq!(config.osc.listen, "0.0.0.0:9000");
        assert!(config.sound.is_some());
        assert_eq!(config.osc_out["lights"].addr, "192.168.1.102:8000");
        assert!(!config.routes.is_empty());
    }

    #[test]
    fn minimal_config_uses_defaults() {
        let config: ShowConfig = toml::from_str("").unwrap();
        assert_eq!(config.osc.listen, "0.0.0.0:9000");
        assert!(config.osc.builtin_routes && config.sound.is_none() && config.routes.is_empty());
    }
}
