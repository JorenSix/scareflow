//! Client for Tasmota smart plugs via the HTTP command API (`GET /cm?cmnd=Power On`).
//!
//! Requests are queued to a worker thread, like WLED commands, so an
//! unreachable plug never stalls the caller; when the queue is full new
//! commands are dropped. The plug's small web server handles one client at a
//! time, so every command is one plain HTTP/1.0 request on its own connection:
//! sent, read to the end, and shut down in both directions before the next.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use crate::ResponderError;

const QUEUE_LEN: usize = 32;

pub struct TasmotaClient {
    queue: SyncSender<(String, String)>,
}

impl TasmotaClient {
    pub fn new(timeout: Duration) -> Self {
        let (queue, rx) = mpsc::sync_channel::<(String, String)>(QUEUE_LEN);
        thread::Builder::new()
            .name("scareflow-tasmota".into())
            .spawn(move || {
                for (host, path) in rx {
                    let started = Instant::now();
                    match get(&host, &path, timeout) {
                        Ok(body) => log::info!("Tasmota {host}{path} -> {} in {:?}", body.trim(), started.elapsed()),
                        Err(e) => log::warn!("Tasmota {host}{path} failed after {:?}: {e}", started.elapsed()),
                    }
                }
            })
            .expect("spawn Tasmota worker");
        Self { queue }
    }

    /// Switches the plug's relay. `host` is `"192.168.1.162"`, `"plug.local:8080"` or an `http://…` base URL.
    pub fn power(&self, host: &str, on: bool) -> Result<(), ResponderError> {
        let host = host.trim_start_matches("http://").trim_end_matches('/').to_owned();
        match self.queue.try_send((host, command_path(if on { "Power On" } else { "Power Off" }))) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("Tasmota queue full, command dropped".into()),
            Err(TrySendError::Disconnected(_)) => Err("Tasmota worker stopped".into()),
        }
    }
}

fn command_path(command: &str) -> String {
    format!("/cm?cmnd={}", command.replace(' ', "%20"))
}

/// One `GET` on a fresh connection that is always closed afterwards. Returns the body.
fn get(host: &str, path: &str, timeout: Duration) -> Result<String, ResponderError> {
    let with_port = if host.contains(':') { host.to_owned() } else { format!("{host}:80") };
    let addr = with_port.to_socket_addrs()?.next().ok_or_else(|| format!("cannot resolve {host}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let result = (|| -> Result<String, ResponderError> {
        write!(stream, "GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n")?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        let text = String::from_utf8_lossy(&response);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = head.lines().next().unwrap_or_default();
        if !status.contains(" 200") {
            return Err(format!("unexpected reply {status:?}").into());
        }
        Ok(body.to_owned())
    })();
    let _ = stream.shutdown(Shutdown::Both);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_command_paths() {
        assert_eq!(command_path("Power On"), "/cm?cmnd=Power%20On");
    }

    #[test]
    fn sends_one_closed_http_request() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = server.local_addr().unwrap().to_string();
        let reply = thread::spawn(move || {
            let (stream, _) = server.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                lines.push(line.trim().to_owned());
            }
            reader.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"POWER\":\"ON\"}").unwrap();
            lines
        });
        let body = get(&host, &command_path("Power On"), Duration::from_secs(2)).unwrap();
        let lines = reply.join().unwrap();
        assert_eq!(body, "{\"POWER\":\"ON\"}");
        assert_eq!(lines[0], "GET /cm?cmnd=Power%20On HTTP/1.0");
        assert!(lines.contains(&"Connection: close".to_owned()));
    }
}
