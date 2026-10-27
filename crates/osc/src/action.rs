use serde::{Deserialize, Serialize};

use crate::OscEvent;

/// What a route does when its pattern matches. Deserialized from config, e.g.
///
/// ```toml
/// { type = "sound", cmd = "play", file = "scream.wav", volume = 0.8 }
/// { type = "wled", target = "porch", cmd = "preset", preset = 3 }
/// { type = "custom", kind = "app", data = { event = "scare" } }
/// ```
///
/// Text fields and [`Num`] fields may refer to OSC arguments with `$1`, `$2`, …
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Sound(SoundAction),
    Wled(WledAction),
    /// Send OSC to another device, e.g. a light controller.
    OscOut(OscOutAction),
    /// Anything else; dispatched to the responder registered under `kind`.
    Custom {
        kind: String,
        #[serde(default)]
        data: serde_json::Value,
    },
}

impl Action {
    /// The responder kind this action is dispatched to.
    pub fn kind(&self) -> &str {
        match self {
            Action::Sound(_) => "sound",
            Action::Wled(_) => "wled",
            Action::OscOut(_) => "osc_out",
            Action::Custom { kind, .. } => kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum SoundAction {
    /// One-shot effect on top of the background music.
    Play {
        file: String,
        #[serde(default)]
        volume: Option<Num>,
    },
    /// Replace the looping background track.
    Background { file: String },
    BackgroundVolume { volume: Num },
    StopBackground,
    StopEffects,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WledAction {
    /// Name of a configured WLED controller.
    pub target: String,
    #[serde(flatten)]
    pub cmd: WledCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum WledCommand {
    Preset { preset: Num },
    Power { on: bool },
    Brightness { bri: Num },
    Effect {
        fx: Num,
        #[serde(default)]
        sx: Option<Num>,
        #[serde(default)]
        ix: Option<Num>,
        #[serde(default)]
        col: Option<[u8; 3]>,
    },
    /// Sent as-is to `/json/state`.
    Raw { json: serde_json::Value },
    /// Random hard flashes on every segment, whatever the strip was doing (even off),
    /// then the previous state is restored.
    Lightning {
        #[serde(default = "Num::default_storm_ms")]
        duration_ms: Num,
        /// Flash colour, RGBW; the white channel is ignored by RGB strips.
        #[serde(default = "full_white")]
        col: [u8; 4],
    },
}

fn full_white() -> [u8; 4] {
    [255; 4]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OscOutAction {
    /// Name of a configured OSC destination.
    pub target: String,
    #[serde(flatten)]
    pub cmd: OscOutCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum OscOutCommand {
    /// One message; arguments are sent as floats.
    Send {
        address: String,
        #[serde(default)]
        args: Vec<Num>,
    },
    /// Flicker `address` between `on` and `off` at random intervals, ending on `off`.
    Lightning {
        address: String,
        #[serde(default = "Num::one")]
        on: Num,
        #[serde(default = "Num::zero")]
        off: Num,
        #[serde(default = "Num::default_storm_ms")]
        duration_ms: Num,
    },
}

/// A number given literally or taken from an OSC argument (`"$1"`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Num {
    Value(f32),
    Arg(String),
}

impl Num {
    fn one() -> Self {
        Num::Value(1.0)
    }

    fn zero() -> Self {
        Num::Value(0.0)
    }

    pub(crate) fn default_storm_ms() -> Self {
        Num::Value(3000.0)
    }

    pub fn get(&self, event: &OscEvent) -> Option<f32> {
        match self {
            Num::Value(v) => Some(*v),
            Num::Arg(s) => match arg_index(s) {
                Some(i) => event.arg_f32(i),
                None => s.trim().parse().ok(),
            },
        }
    }
}

/// Replaces every `$N` in `text` with the N-th (1-based) OSC argument, e.g. `"/light/$1"`.
/// Returns `None` when a referenced argument is missing.
pub fn resolve_text(text: &str, event: &OscEvent) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('$') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            out.push('$');
        } else {
            out.push_str(&event.arg_string(arg_index(&rest[pos..pos + 1 + digits])?)?);
        }
        rest = &after[digits..];
    }
    out.push_str(rest);
    Some(out)
}

fn arg_index(text: &str) -> Option<usize> {
    let n: usize = text.strip_prefix('$')?.parse().ok()?;
    n.checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::OscType;

    #[derive(Deserialize)]
    struct Actions {
        actions: Vec<Action>,
    }

    #[test]
    fn parses_actions_from_toml() {
        let parsed: Actions = toml::from_str(
            r#"
            actions = [
              { type = "sound", cmd = "play", file = "scream.wav", volume = 0.8 },
              { type = "sound", cmd = "stop_background" },
              { type = "wled", target = "porch", cmd = "preset", preset = 3 },
              { type = "wled", target = "porch", cmd = "effect", fx = "$1", col = [255, 80, 0] },
              { type = "custom", kind = "app", data = { event = "scare" } },
              { type = "osc_out", target = "lights", cmd = "lightning", address = "/light/1", duration_ms = "$1" },
            ]
            "#,
        )
        .unwrap();
        let a = parsed.actions;
        assert_eq!(
            a[0],
            Action::Sound(SoundAction::Play { file: "scream.wav".into(), volume: Some(Num::Value(0.8)) })
        );
        assert_eq!(a[1], Action::Sound(SoundAction::StopBackground));
        assert_eq!(
            a[2],
            Action::Wled(WledAction { target: "porch".into(), cmd: WledCommand::Preset { preset: Num::Value(3.0) } })
        );
        assert!(matches!(&a[3], Action::Wled(WledAction { cmd: WledCommand::Effect { fx: Num::Arg(_), col: Some([255, 80, 0]), .. }, .. })));
        assert_eq!(a[4].kind(), "app");
        assert_eq!(
            a[5],
            Action::OscOut(OscOutAction {
                target: "lights".into(),
                cmd: OscOutCommand::Lightning {
                    address: "/light/1".into(),
                    on: Num::Value(1.0),
                    off: Num::Value(0.0),
                    duration_ms: Num::Arg("$1".into()),
                },
            })
        );
    }

    #[test]
    fn substitutes_arguments() {
        let ev = OscEvent::new("/x", vec![OscType::String("boo.wav".into()), OscType::Int(7)]);
        assert_eq!(resolve_text("$1", &ev).as_deref(), Some("boo.wav"));
        assert_eq!(resolve_text("plain", &ev).as_deref(), Some("plain"));
        assert_eq!(resolve_text("$3", &ev), None);
        assert_eq!(resolve_text("/light/$2/on", &ev).as_deref(), Some("/light/7/on"));
        assert_eq!(resolve_text("cost $", &ev).as_deref(), Some("cost $"));
        assert_eq!(Num::Arg("$2".into()).get(&ev), Some(7.0));
        assert_eq!(Num::Arg("$0".into()).get(&ev), None);
        assert_eq!(Num::Value(0.5).get(&ev), Some(0.5));
    }
}
