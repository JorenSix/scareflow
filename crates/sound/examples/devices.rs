//! Lists the audio output devices a Sound block can pick.
//!
//! cargo run -p scareflow-sound --example devices

fn main() {
    for device in scareflow_sound::output_devices() {
        println!("{}{}", device.name, if device.active { "" } else { " (inactive)" });
    }
}
