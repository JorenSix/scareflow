//! Scareflow OSC event module.
//!
//! An [`OscListener`] receives OSC over UDP and forwards every message as an
//! [`OscEvent`]. A [`Router`] matches event addresses against configured
//! patterns and hands each resulting [`Action`] to the [`Responder`] that is
//! registered for the action's kind. Responders are the plug-in point: play a
//! sound, drive a WLED controller, send OSC to another device, or forward to the application's state model
//! with a [`ChannelResponder`].

mod action;
mod event;
mod listener;
pub mod osc_out;
mod responder;
mod router;
#[cfg(feature = "wled")]
pub mod wled;
#[cfg(feature = "wled")]
pub mod tasmota;

pub use action::{Action, Num, OscOutAction, OscOutCommand, SoundAction, WledAction, WledCommand, resolve_text};
pub use event::OscEvent;
pub use listener::OscListener;
pub use responder::{ChannelResponder, Responder, ResponderError};
pub use router::{Route, Router};

/// Re-exported so users can build and inspect arguments without a direct dependency.
pub use rosc::OscType;
