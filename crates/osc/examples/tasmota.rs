//! Switches a Tasmota plug on and off a few times, for testing a plug.
//!
//! cargo run -p scareflow-osc --example tasmota -- 192.168.1.162 [cycles] [seconds]

use std::time::Duration;

use scareflow_osc::tasmota::TasmotaClient;

fn main() {
    let mut args = std::env::args().skip(1);
    let host = args.next().expect("usage: tasmota host [cycles] [seconds]");
    let cycles: u32 = args.next().map(|a| a.parse().expect("cycles")).unwrap_or(2);
    let secs: f32 = args.next().map(|a| a.parse().expect("seconds")).unwrap_or(3.0);
    let plug = TasmotaClient::new(Duration::from_secs(3));
    for cycle in 1..=cycles {
        for on in [true, false] {
            println!("cycle {cycle}: power {}", if on { "on" } else { "off" });
            plug.power(&host, on).expect("queue command");
            std::thread::sleep(Duration::from_secs_f32(secs));
        }
    }
}
