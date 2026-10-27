use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rosc::{OscPacket, decoder};

use crate::OscEvent;

/// Receives OSC over UDP on its own thread and forwards each message as an [`OscEvent`].
/// Stops on [`OscListener::shutdown`], on drop, or when the receiving side is dropped.
pub struct OscListener {
    local_addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl OscListener {
    pub fn spawn(addr: impl ToSocketAddrs, events: Sender<OscEvent>) -> io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let local_addr = socket.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new().name("scareflow-osc-listener".into()).spawn({
            let stop = stop.clone();
            move || receive_loop(&socket, &events, &stop)
        })?;
        log::info!("listening for OSC on {local_addr}");
        Ok(Self { local_addr, stop, thread: Some(thread) })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn shutdown(mut self) {
        self.stop_thread();
    }

    fn stop_thread(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for OscListener {
    fn drop(&mut self) {
        self.stop_thread();
    }
}

fn receive_loop(socket: &UdpSocket, events: &Sender<OscEvent>, stop: &AtomicBool) {
    let mut buf = [0u8; decoder::MTU];
    while !stop.load(Ordering::Relaxed) {
        let (len, from) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => continue,
            Err(e) => {
                log::warn!("OSC receive failed: {e}");
                continue;
            }
        };
        match decoder::decode_udp(&buf[..len]) {
            Ok((_, packet)) => {
                let mut out = Vec::new();
                flatten(packet, from, &mut out);
                for event in out {
                    if events.send(event).is_err() {
                        return;
                    }
                }
            }
            Err(e) => log::warn!("undecodable OSC packet from {from}: {e}"),
        }
    }
}

fn flatten(packet: OscPacket, from: SocketAddr, out: &mut Vec<OscEvent>) {
    match packet {
        OscPacket::Message(m) => out.push(OscEvent { address: m.addr, args: m.args, from: Some(from) }),
        OscPacket::Bundle(b) => b.content.into_iter().for_each(|p| flatten(p, from, out)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use rosc::{OscBundle, OscMessage, OscTime, OscType, encoder};

    use super::*;

    #[test]
    fn receives_messages_and_bundles() {
        let (tx, rx) = mpsc::channel();
        let listener = OscListener::spawn("127.0.0.1:0", tx).unwrap();
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let msg = |addr: &str, args| OscPacket::Message(OscMessage { addr: addr.into(), args });
        let bundle = OscPacket::Bundle(OscBundle {
            timetag: OscTime { seconds: 0, fractional: 1 },
            content: vec![msg("/b/1", vec![]), msg("/b/2", vec![OscType::Float(0.5)])],
        });
        for p in [msg("/a", vec![OscType::Int(1)]), bundle] {
            sock.send_to(&encoder::encode(&p).unwrap(), listener.local_addr()).unwrap();
        }
        let got: Vec<_> = (0..3).map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap()).collect();
        assert_eq!(got.iter().map(|e| e.address.as_str()).collect::<Vec<_>>(), ["/a", "/b/1", "/b/2"]);
        assert_eq!(got[2].arg_f32(0), Some(0.5));
        listener.shutdown();
    }
}
