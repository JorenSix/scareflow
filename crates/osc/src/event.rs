use std::net::SocketAddr;

use rosc::OscType;

/// One received OSC message. Bundles are flattened into their messages.
#[derive(Clone, Debug, PartialEq)]
pub struct OscEvent {
    pub address: String,
    pub args: Vec<OscType>,
    /// Sender, if the event came from the network.
    pub from: Option<SocketAddr>,
}

impl OscEvent {
    pub fn new(address: impl Into<String>, args: Vec<OscType>) -> Self {
        Self { address: address.into(), args, from: None }
    }

    /// Argument `index` (0-based) as a number, converting ints, floats, doubles and numeric strings.
    pub fn arg_f32(&self, index: usize) -> Option<f32> {
        match self.args.get(index)? {
            OscType::Float(v) => Some(*v),
            OscType::Double(v) => Some(*v as f32),
            OscType::Int(v) => Some(*v as f32),
            OscType::Long(v) => Some(*v as f32),
            OscType::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
            OscType::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    /// Argument `index` (0-based) rendered as text.
    pub fn arg_string(&self, index: usize) -> Option<String> {
        match self.args.get(index)? {
            OscType::String(s) => Some(s.clone()),
            OscType::Float(v) => Some(v.to_string()),
            OscType::Double(v) => Some(v.to_string()),
            OscType::Int(v) => Some(v.to_string()),
            OscType::Long(v) => Some(v.to_string()),
            OscType::Bool(v) => Some(v.to_string()),
            _ => None,
        }
    }
}
