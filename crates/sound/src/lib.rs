//! Scareflow sound module.
//!
//! A [`SoundEngine`] owns the audio output on a dedicated thread and mixes a
//! looping background track with any number of one-shot effects, so effects
//! never interrupt the music. Control it from any thread with the cloneable
//! [`SoundHandle`]. Optionally the music is ducked while effects play.
//! Effects are peak-normalized when first loaded, so the loudest sample of
//! every effect reaches full scale and volume 1.0 means "peak at maximum".

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use rodio::buffer::SamplesBuffer;
use rodio::cpal::traits::HostTrait;
use rodio::cpal::{BufferSize, SampleFormat, SupportedBufferSize, SupportedStreamConfig};
use rodio::mixer::Mixer as Output;
use rodio::{Decoder, Device, DeviceSinkBuilder, DeviceSinkError, DeviceTrait, MixerDeviceSink, Player, Source};

const TICK: Duration = Duration::from_millis(20);

/// Default audio callback size in frames. The device buffers two of these:
/// 256 frames, about 6 ms at 44.1 kHz, between a trigger and the speakers.
/// Half of that underruns on a desktop PC.
pub const DEFAULT_BUFFER_FRAMES: u32 = 128;

#[derive(Clone, Debug)]
pub struct SoundConfig {
    /// Relative sound paths are resolved against this directory and may not leave it.
    pub dir: PathBuf,
    /// Background music volume, 0.0–1.0 (values above 1 amplify).
    pub background_volume: f32,
    /// Background volume factor while effects play, e.g. `Some(0.4)`. `None` disables ducking.
    pub duck_to: Option<f32>,
    /// Oldest effects are stopped beyond this many simultaneous voices.
    pub max_voices: usize,
    /// Time for background fades (start, stop, track change, ducking).
    pub fade: Duration,
    /// Output device name (see [`output_devices`]); `None` uses the system default.
    pub device: Option<String>,
    /// Audio callback size in frames; smaller starts sounds sooner but may crackle.
    /// `None` leaves it to the audio system, which can queue 50–100 ms.
    pub buffer_frames: Option<u32>,
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("."),
            background_volume: 0.5,
            duck_to: None,
            max_voices: 8,
            fade: Duration::from_millis(800),
            device: None,
            buffer_frames: Some(DEFAULT_BUFFER_FRAMES),
        }
    }
}

#[derive(Debug)]
pub enum SoundCommand {
    PlayEffect { path: PathBuf, volume: f32 },
    StopEffects,
    PlayBackground { path: PathBuf },
    StopBackground,
    SetBackgroundVolume(f32),
    /// Decodes an effect into the cache so its first play starts at once.
    Preload { path: PathBuf },
}

#[derive(Debug)]
pub struct SoundError(String);

impl fmt::Display for SoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SoundError {}

/// Cloneable, `Send` control handle. The engine stops when every handle is dropped.
#[derive(Clone)]
pub struct SoundHandle {
    tx: Sender<SoundCommand>,
}

impl SoundHandle {
    pub fn send(&self, cmd: SoundCommand) -> Result<(), SoundError> {
        self.tx.send(cmd).map_err(|_| SoundError("sound engine stopped".into()))
    }

    pub fn play_effect(&self, path: impl Into<PathBuf>, volume: f32) -> Result<(), SoundError> {
        self.send(SoundCommand::PlayEffect { path: path.into(), volume })
    }

    pub fn stop_effects(&self) -> Result<(), SoundError> {
        self.send(SoundCommand::StopEffects)
    }

    pub fn play_background(&self, path: impl Into<PathBuf>) -> Result<(), SoundError> {
        self.send(SoundCommand::PlayBackground { path: path.into() })
    }

    pub fn stop_background(&self) -> Result<(), SoundError> {
        self.send(SoundCommand::StopBackground)
    }

    pub fn set_background_volume(&self, volume: f32) -> Result<(), SoundError> {
        self.send(SoundCommand::SetBackgroundVolume(volume))
    }

    pub fn preload(&self, path: impl Into<PathBuf>) -> Result<(), SoundError> {
        self.send(SoundCommand::Preload { path: path.into() })
    }
}

pub struct SoundEngine;

impl SoundEngine {
    /// Opens the default output device on a new thread.
    pub fn start(config: SoundConfig) -> Result<SoundHandle, SoundError> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        thread::Builder::new()
            .name("scareflow-sound".into())
            .spawn(move || {
                // The output stream is !Send, so it lives and dies on this thread.
                let mut sink = match open_output(config.device.as_deref(), config.buffer_frames) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(SoundError(format!("no audio output: {e}"))));
                        return;
                    }
                };
                sink.log_on_drop(false);
                let _ = ready_tx.send(Ok(()));
                Mixer::new(config, sink.mixer().clone()).run(rx);
            })
            .map_err(|e| SoundError(e.to_string()))?;
        ready_rx.recv().map_err(|_| SoundError("sound thread exited".into()))??;
        Ok(SoundHandle { tx })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputDevice {
    pub name: String,
    /// False for outputs that are connected but not in use, e.g. a Bluetooth speaker
    /// that is not the system output, or the built-in speakers while headphones are in.
    /// They can still be opened.
    pub active: bool,
}

/// The audio output devices, including inactive ones.
///
/// macOS answers "invalid property value" when asked for the formats of an
/// inactive device, so cpal's own output list leaves it out. Input-only devices
/// answer with an empty format list and are skipped.
pub fn output_devices() -> Vec<OutputDevice> {
    let devices = match rodio::cpal::default_host().devices() {
        Ok(devices) => devices,
        Err(e) => {
            log::warn!("cannot list audio devices: {e}");
            return Vec::new();
        }
    };
    devices
        .filter_map(|d| {
            let active = match d.supported_output_configs() {
                Ok(mut configs) => configs.next().is_some().then_some(true)?,
                Err(_) => false,
            };
            Some(OutputDevice { name: device_name(&d)?, active })
        })
        .collect()
}

fn device_name(device: &Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_owned())
}

/// Opens the named output, falling back to the default when it is missing.
/// Inactive devices do not report their formats, so common ones are tried too.
/// A buffer size the device refuses falls back to the device's own.
fn open_output(device: Option<&str>, buffer_frames: Option<u32>) -> Result<MixerDeviceSink, DeviceSinkError> {
    let host = rodio::cpal::default_host();
    let named = device.and_then(|name| {
        let found = host.devices().ok()?.find(|d| device_name(d).as_deref() == Some(name));
        if found.is_none() {
            log::warn!("audio output {name:?} not found, using the default");
        }
        found
    });
    let device = named.or_else(|| host.default_output_device()).ok_or(DeviceSinkError::NoDevice)?;
    let mut formats: Vec<SupportedStreamConfig> = device.default_output_config().into_iter().collect();
    formats.extend([48_000, 44_100].map(|rate| SupportedStreamConfig::new(2, rate, SupportedBufferSize::Unknown, SampleFormat::F32)));
    let mut first_error = None;
    for format in &formats {
        let builder = || DeviceSinkBuilder::default().with_device(device.clone()).with_supported_config(format);
        if let Some(frames) = buffer_frames {
            match builder().with_buffer_size(BufferSize::Fixed(frames)).open_stream() {
                Ok(sink) => return Ok(sink),
                Err(e) => log::warn!("audio buffer of {frames} frames refused: {e}"),
            }
        }
        match builder().open_stream() {
            Ok(sink) => return Ok(sink),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.unwrap_or(DeviceSinkError::NoDevice))
}

type Clip = SamplesBuffer;

struct Background {
    player: Player,
    level: f32,
}

struct Mixer {
    config: SoundConfig,
    output: Output,
    cache: HashMap<PathBuf, Clip>,
    background: Option<Background>,
    fading_out: Vec<Background>,
    effects: Vec<Player>,
}

impl Mixer {
    fn new(config: SoundConfig, output: Output) -> Self {
        Self { config, output, cache: HashMap::new(), background: None, fading_out: Vec::new(), effects: Vec::new() }
    }

    fn run(mut self, rx: Receiver<SoundCommand>) {
        let mut last = Instant::now();
        loop {
            match rx.recv_timeout(TICK) {
                Ok(cmd) => {
                    if let Err(e) = self.apply(cmd) {
                        log::warn!("{e}");
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let now = Instant::now();
            self.tick(now - last);
            last = now;
        }
    }

    fn apply(&mut self, cmd: SoundCommand) -> Result<(), SoundError> {
        match cmd {
            SoundCommand::PlayEffect { path, volume } => {
                let clip = self.clip(&path)?;
                let player = self.player();
                player.set_volume(volume.max(0.0));
                player.append(clip);
                self.effects.push(player);
                while self.effects.len() > self.config.max_voices.max(1) {
                    self.effects.remove(0).stop();
                }
            }
            SoundCommand::StopEffects => self.effects.drain(..).for_each(|s| s.stop()),
            SoundCommand::PlayBackground { path } => {
                let full = resolve(&self.config.dir, &path)?;
                let player = self.player();
                player.set_volume(0.0);
                player.append(decode(&full)?.repeat_infinite());
                self.fading_out.extend(self.background.take());
                self.background = Some(Background { player, level: 0.0 });
            }
            SoundCommand::StopBackground => self.fading_out.extend(self.background.take()),
            SoundCommand::SetBackgroundVolume(v) => self.config.background_volume = v.max(0.0),
            SoundCommand::Preload { path } => {
                self.clip(&path)?;
            }
        }
        Ok(())
    }

    fn tick(&mut self, dt: Duration) {
        self.effects.retain(|s| !s.empty());
        let step = fade_step(dt, self.config.fade);
        let target = background_target(&self.config, !self.effects.is_empty());
        if let Some(bg) = &mut self.background {
            bg.level = ramp(bg.level, target, step);
            bg.player.set_volume(bg.level);
        }
        self.fading_out.retain_mut(|bg| {
            bg.level = ramp(bg.level, 0.0, step);
            bg.player.set_volume(bg.level);
            if bg.level <= 0.0 {
                bg.player.stop();
            }
            bg.level > 0.0
        });
    }

    fn player(&self) -> Player {
        Player::connect_new(&self.output)
    }

    fn clip(&mut self, path: &Path) -> Result<Clip, SoundError> {
        let full = resolve(&self.config.dir, path)?;
        if let Some(clip) = self.cache.get(&full) {
            return Ok(clip.clone());
        }
        let clip = normalized(decode(&full)?);
        self.cache.insert(full, clip.clone());
        Ok(clip)
    }
}

/// Decodes the whole clip and scales it so its highest absolute sample is 1.0.
/// Silent clips are left as they are.
fn normalized(decoder: Decoder<BufReader<File>>) -> Clip {
    let (channels, rate) = (decoder.channels(), decoder.sample_rate());
    let mut samples: Vec<f32> = decoder.collect();
    let peak = samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        samples.iter_mut().for_each(|s| *s /= peak);
    }
    SamplesBuffer::new(channels, rate, samples)
}

fn decode(path: &Path) -> Result<Decoder<BufReader<File>>, SoundError> {
    let file = File::open(path).map_err(|e| SoundError(format!("{}: {e}", path.display())))?;
    Decoder::try_from(file).map_err(|e| SoundError(format!("{}: {e}", path.display())))
}

/// Joins a relative path onto `dir`, refusing absolute paths and `..` so OSC input stays inside it.
fn resolve(dir: &Path, path: &Path) -> Result<PathBuf, SoundError> {
    if path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
        Ok(dir.join(path))
    } else {
        Err(SoundError(format!("sound path must be relative to the sound directory: {}", path.display())))
    }
}

fn background_target(config: &SoundConfig, effects_playing: bool) -> f32 {
    match config.duck_to {
        Some(duck) if effects_playing => config.background_volume * duck,
        _ => config.background_volume,
    }
}

fn fade_step(dt: Duration, fade: Duration) -> f32 {
    if fade.is_zero() { f32::INFINITY } else { dt.as_secs_f32() / fade.as_secs_f32() }
}

fn ramp(current: f32, target: f32, max_step: f32) -> f32 {
    if (target - current).abs() <= max_step { target } else { current + max_step.copysign(target - current) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramps_toward_target() {
        assert_eq!(ramp(0.0, 1.0, 0.25), 0.25);
        assert_eq!(ramp(1.0, 0.0, 0.25), 0.75);
        assert_eq!(ramp(0.9, 1.0, 0.25), 1.0);
        assert_eq!(ramp(0.3, 0.0, f32::INFINITY), 0.0);
        assert_eq!(fade_step(Duration::from_millis(100), Duration::from_secs(1)), 0.1);
    }

    #[test]
    fn ducks_only_while_effects_play() {
        let config = SoundConfig { background_volume: 0.5, duck_to: Some(0.5), ..Default::default() };
        assert_eq!(background_target(&config, false), 0.5);
        assert_eq!(background_target(&config, true), 0.25);
        let no_duck = SoundConfig { duck_to: None, ..config };
        assert_eq!(background_target(&no_duck, true), 0.5);
    }

    #[test]
    fn lists_output_devices_without_panicking() {
        let _ = output_devices();
    }

    #[test]
    fn bundled_scare_samples_peak_at_full_scale() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scare");
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        for path in entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "wav")) {
            let peak = normalized(decode(&path).unwrap()).fold(0.0_f32, |m, s| m.max(s.abs()));
            assert!((peak - 1.0).abs() < 1e-6, "{}: peak {peak}", path.display());
        }
    }

    #[test]
    fn refuses_paths_outside_dir() {
        let dir = Path::new("/sounds");
        assert_eq!(resolve(dir, Path::new("fx/boo.wav")).unwrap(), Path::new("/sounds/fx/boo.wav"));
        assert!(resolve(dir, Path::new("../secret.wav")).is_err());
        assert!(resolve(dir, Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn decodes_and_buffers_wav() {
        let dir = std::env::temp_dir().join(format!("scareflow-sound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("beep.wav");
        let spec = hound::WavSpec { channels: 1, sample_rate: 8000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..800 {
            w.write_sample(((i as f32 * 0.3).sin() * 10000.0) as i16).unwrap();
        }
        w.finalize().unwrap();

        let clip = normalized(decode(&path).unwrap());
        assert_eq!(clip.sample_rate().get(), 8000);
        assert_eq!(clip.clone().count(), 800);
        let samples: Vec<f32> = clip.collect();
        assert_eq!(samples.len(), 800, "cached clips replay from the start");
        let peak = samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!((peak - 1.0).abs() < 1e-6, "peak {peak} is normalized to full scale");
        assert!(decode(&dir.join("missing.wav")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
