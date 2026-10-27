//! Runs one WLED lightning storm, for testing a controller.
//!
//! cargo run -p scareflow-osc --example wled_lightning -- 192.168.1.102 [duration_ms]

use std::time::Duration;

use scareflow_osc::wled::WledResponder;
use scareflow_osc::{Action, Num, OscEvent, Responder, WledAction, WledCommand};

fn main() {
    let mut args = std::env::args().skip(1);
    let host = args.next().expect("usage: wled_lightning host [duration_ms]");
    let ms: f32 = args.next().map(|a| a.parse().expect("duration_ms")).unwrap_or(3000.0);
    let mut wled = WledResponder::new([("strip".to_owned(), host)], Duration::from_secs(2));
    let cmd = WledCommand::Lightning { duration_ms: Num::Value(ms), col: [255; 4] };
    wled.handle(&Action::Wled(WledAction { target: "strip".into(), cmd }), &OscEvent::new("/test", vec![]))
        .expect("start storm");
    // The storm runs on its own thread; give it time to flash and restore.
    std::thread::sleep(Duration::from_millis(ms as u64 + 2500));
}
