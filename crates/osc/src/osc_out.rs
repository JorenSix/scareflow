//! Responder that sends OSC to other devices, e.g. a light controller.
//!
//! Besides single messages it can play a lightning effect: random bursts of
//! short on/off flashes on one address for a given duration, always ending
//! off. Effects run on their own thread; retriggering the same address
//! cancels the storm already running there.

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rosc::{OscMessage, OscPacket, OscType, encoder};

use crate::{Action, OscEvent, OscOutCommand, Responder, ResponderError, resolve_text};

pub struct OscOutResponder {
    socket: Arc<UdpSocket>,
    targets: HashMap<String, SocketAddr>,
    storms: HashMap<(SocketAddr, String), Arc<AtomicBool>>,
}

impl OscOutResponder {
    /// `targets` maps destination names to `host:port` (e.g. `"192.168.1.102:8000"`).
    pub fn new(targets: impl IntoIterator<Item = (String, String)>) -> std::io::Result<Self> {
        let targets = targets
            .into_iter()
            .map(|(name, addr)| {
                let resolved = addr.to_socket_addrs()?.next().ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("cannot resolve {addr}"))
                })?;
                Ok((name, resolved))
            })
            .collect::<std::io::Result<_>>()?;
        Ok(Self { socket: Arc::new(UdpSocket::bind("0.0.0.0:0")?), targets, storms: HashMap::new() })
    }
}

impl Responder for OscOutResponder {
    fn name(&self) -> &str {
        "osc_out"
    }

    fn handle(&mut self, action: &Action, event: &OscEvent) -> Result<(), ResponderError> {
        let Action::OscOut(out) = action else {
            return Err(format!("unsupported action {:?}", action.kind()).into());
        };
        let target = resolve_text(&out.target, event).ok_or("missing OSC target argument")?;
        let to = *self.targets.get(&target).ok_or_else(|| format!("unknown OSC target {target:?}"))?;
        match &out.cmd {
            OscOutCommand::Send { address, args } => {
                let address = resolve_text(address, event).ok_or("missing address argument")?;
                let args = args
                    .iter()
                    .map(|a| a.get(event).map(OscType::Float).ok_or("missing argument"))
                    .collect::<Result<_, _>>()?;
                send(&self.socket, to, &address, args)?;
            }
            OscOutCommand::Lightning { address, on, off, duration_ms } => {
                let address = resolve_text(address, event).ok_or("missing address argument")?;
                let on = on.get(event).ok_or("missing on value")?;
                let off = off.get(event).ok_or("missing off value")?;
                let duration = Duration::from_millis(duration_ms.get(event).ok_or("missing duration")?.max(0.0) as u64);
                let cancel = Arc::new(AtomicBool::new(false));
                if let Some(previous) = self.storms.insert((to, address.clone()), cancel.clone()) {
                    previous.store(true, Ordering::Relaxed);
                }
                let socket = self.socket.clone();
                let schedule = lightning_schedule(seed(), duration);
                thread::Builder::new().name("scareflow-lightning".into()).spawn(move || {
                    for (wait, lit) in schedule {
                        thread::sleep(wait);
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Err(e) = send(&socket, to, &address, vec![OscType::Float(if lit { on } else { off })]) {
                            log::warn!("lightning to {to} failed: {e}");
                        }
                    }
                })?;
            }
        }
        Ok(())
    }
}

/// Sends one message to `to`.
pub fn send(socket: &UdpSocket, to: SocketAddr, address: &str, args: Vec<OscType>) -> Result<(), ResponderError> {
    let packet = OscPacket::Message(OscMessage { addr: address.to_owned(), args });
    socket.send_to(&encoder::encode(&packet)?, to)?;
    Ok(())
}

pub(crate) fn seed() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9e37_79b9_7f4a_7c15) | 1
}

/// Steps of `(wait before, light on?)`: bursts of 1–4 flashes of 30–150 ms separated by
/// 40–180 ms, with 200–1500 ms of darkness between bursts. Always ends with the light off.
pub fn lightning_schedule(seed: u64, duration: Duration) -> Vec<(Duration, bool)> {
    let mut rng = XorShift(seed.max(1));
    let ms = |rng: &mut XorShift, lo: u64, hi: u64| Duration::from_millis(rng.range(lo, hi));
    let mut steps = Vec::new();
    let mut elapsed = Duration::ZERO;
    let mut wait = Duration::ZERO;
    while elapsed + wait < duration {
        elapsed += wait;
        for flash in 0..rng.range(1, 4) {
            let gap = if flash == 0 { wait } else { ms(&mut rng, 40, 180) };
            let lit = ms(&mut rng, 30, 150);
            if flash > 0 {
                elapsed += gap;
            }
            steps.push((gap, true));
            steps.push((lit, false));
            elapsed += lit;
        }
        wait = ms(&mut rng, 200, 1500);
    }
    steps
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform-ish value in `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

#[cfg(test)]
mod tests {
    use rosc::decoder;

    use super::*;
    use crate::{Num, OscOutAction};

    #[test]
    fn schedule_alternates_and_ends_dark() {
        for seed in 1..50 {
            let steps = lightning_schedule(seed, Duration::from_secs(3));
            assert!(!steps.is_empty());
            assert!(steps.iter().enumerate().all(|(i, (_, lit))| *lit == (i % 2 == 0)));
            assert!(!steps.last().unwrap().1);
            let total: Duration = steps.iter().map(|(w, _)| *w).sum();
            assert!(total < Duration::from_millis(3000 + 4 * 330), "seed {seed}: {total:?}");
        }
        assert!(lightning_schedule(7, Duration::ZERO).is_empty());
    }

    fn receiver() -> (UdpSocket, String) {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let addr = rx.local_addr().unwrap().to_string();
        (rx, addr)
    }

    fn recv(rx: &UdpSocket) -> OscMessage {
        let mut buf = [0; 1024];
        let n = rx.recv(&mut buf).unwrap();
        match decoder::decode_udp(&buf[..n]).unwrap().1 {
            OscPacket::Message(m) => m,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn sends_message_with_substituted_args() {
        let (rx, addr) = receiver();
        let mut responder = OscOutResponder::new([("lights".to_owned(), addr)]).unwrap();
        let action = Action::OscOut(OscOutAction {
            target: "lights".into(),
            cmd: OscOutCommand::Send { address: "/light/$1".into(), args: vec![Num::Arg("$2".into())] },
        });
        responder.handle(&action, &OscEvent::new("/x", vec![OscType::Int(2), OscType::Float(0.7)])).unwrap();
        let m = recv(&rx);
        assert_eq!(m.addr, "/light/2");
        assert_eq!(m.args, [OscType::Float(0.7)]);
    }

    #[test]
    fn lightning_flashes_then_turns_off() {
        let (rx, addr) = receiver();
        let mut responder = OscOutResponder::new([("lights".to_owned(), addr)]).unwrap();
        let action = Action::OscOut(OscOutAction {
            target: "lights".into(),
            cmd: OscOutCommand::Lightning {
                address: "/light/1".into(),
                on: Num::Value(1.0),
                off: Num::Value(0.0),
                duration_ms: Num::Value(1.0),
            },
        });
        responder.handle(&action, &OscEvent::new("/x", vec![])).unwrap();
        let mut values = vec![recv(&rx)];
        rx.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let mut buf = [0; 1024];
        while let Ok(n) = rx.recv(&mut buf) {
            if let OscPacket::Message(m) = decoder::decode_udp(&buf[..n]).unwrap().1 {
                values.push(m);
            }
        }
        assert!(values.iter().all(|m| m.addr == "/light/1"));
        let args: Vec<_> = values.into_iter().map(|m| m.args).collect();
        assert_eq!(args.first().unwrap(), &[OscType::Float(1.0)]);
        assert_eq!(args.last().unwrap(), &[OscType::Float(0.0)]);
    }
}
