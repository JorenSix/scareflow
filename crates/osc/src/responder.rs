use std::sync::mpsc::Sender;

use crate::{Action, OscEvent};

pub type ResponderError = Box<dyn std::error::Error + Send + Sync>;

/// A pluggable reaction to routed OSC events.
///
/// `handle` runs on the router thread, so implementations should hand slow
/// work (network, disk) to their own worker instead of blocking.
pub trait Responder: Send + 'static {
    fn name(&self) -> &str;
    fn handle(&mut self, action: &Action, event: &OscEvent) -> Result<(), ResponderError>;
}

/// Forwards actions to a channel, e.g. for the application to drain each frame.
pub struct ChannelResponder {
    name: String,
    tx: Sender<(Action, OscEvent)>,
}

impl ChannelResponder {
    pub fn new(name: impl Into<String>, tx: Sender<(Action, OscEvent)>) -> Self {
        Self { name: name.into(), tx }
    }
}

impl Responder for ChannelResponder {
    fn name(&self) -> &str {
        &self.name
    }

    fn handle(&mut self, action: &Action, event: &OscEvent) -> Result<(), ResponderError> {
        self.tx
            .send((action.clone(), event.clone()))
            .map_err(|_| "channel receiver dropped".into())
    }
}
