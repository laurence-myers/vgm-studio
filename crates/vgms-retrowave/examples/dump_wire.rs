// SPDX-License-Identifier: GPL-2.0-or-later
//! Dumps the exact wire bytes a RetroWave bring-up + test chord sends, so the
//! Web Serial hardware spike (wt-9b) can replay a provably-correct sequence
//! without re-porting the protocol to JavaScript.
//!
//! Run: `cargo run -p vgms-retrowave --example dump_wire`. It prints two hex
//! blobs -- the init + key-on, and the key-off -- to paste into
//! `docs/web-target-2026-07/web-serial-spike/serial-hardware-spike.html`.

use std::io;
use std::sync::{Arc, Mutex};

use vgms_retrowave::{Bank, Device, SerialIo, test_tone};

/// A `SerialIo` that captures every byte instead of sending it.
#[derive(Debug, Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl SerialIo for Capture {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), io::Error> {
        self.0.lock().expect("lock").extend_from_slice(bytes);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), io::Error> {
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

fn main() {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let capture = Capture(Arc::clone(&buffer));

    // `with_io` runs the whole bring-up: IO expander init, chip reset (with the
    // 200 ms settle), and the mute sweep.
    let mut device = Device::with_io(Box::new(capture), "capture".to_owned()).expect("bring-up");
    test_tone::enable_opl3(&mut device).expect("NEW on");
    test_tone::key_on_chord(&mut device, Bank::Zero).expect("key on");

    // Everything captured so far: init + enable + key-on. Split the key-off out
    // so the spike can hold the chord, then release it.
    let key_on = buffer.lock().expect("lock").clone();
    buffer.lock().expect("lock").clear();

    test_tone::key_off_chord(&mut device, Bank::Zero).expect("key off");
    let key_off = buffer.lock().expect("lock").clone();

    println!("// init + enable + key-on ({} bytes)", key_on.len());
    println!("const KEY_ON_HEX = \"{}\";", hex(&key_on));
    println!("// key-off ({} bytes)", key_off.len());
    println!("const KEY_OFF_HEX = \"{}\";", hex(&key_off));
}
