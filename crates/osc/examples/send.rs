//! Sends one OSC message, for testing a running listener.
//!
//! cargo run -p scareflow-osc --example send -- [--to 127.0.0.1:9000] /pumpkin/porch/scare [args…]
//!
//! Arguments are sent as int, float or string, whichever parses first.

use std::net::UdpSocket;

use rosc::{OscMessage, OscPacket, OscType, encoder};

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let mut to = "127.0.0.1:9000".to_owned();
    if args.peek().map(String::as_str) == Some("--to") {
        args.next();
        to = args.next().expect("--to needs an address");
    }
    let addr = args.next().expect("usage: send [--to host:port] /address [args…]");
    let osc_args = args
        .map(|a| {
            if let Ok(i) = a.parse() {
                OscType::Int(i)
            } else if let Ok(f) = a.parse() {
                OscType::Float(f)
            } else {
                OscType::String(a)
            }
        })
        .collect();
    let packet = OscPacket::Message(OscMessage { addr, args: osc_args });
    let socket = UdpSocket::bind("0.0.0.0:0").expect("bind");
    socket.send_to(&encoder::encode(&packet).expect("encode"), &to).expect("send");
    println!("sent {packet:?} to {to}");
}
