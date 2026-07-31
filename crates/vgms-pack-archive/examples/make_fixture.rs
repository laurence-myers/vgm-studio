// SPDX-License-Identifier: GPL-2.0-or-later
//! Writes `tests/e2e-pack.zip`, the fixture the web e2e suite opens as a
//! zip-backed pack (wt-8). Two songs (the committed VGM, twice) and a minimal
//! VGMRips description so the pack parses a game name.
//!
//! Run: `cargo run -p vgms-pack-archive --example make_fixture`.

use std::io::Write as _;

use zip::write::{SimpleFileOptions, ZipWriter};

fn main() {
    let vgm: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");
    let description = "Game name:  E2E Zip Pack\nSystem:  Test System\n\nSong list:\n";

    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    for name in ["01 Alpha.vgm", "02 Beta.vgm"] {
        zip.start_file(name, options).unwrap();
        zip.write_all(vgm).unwrap();
    }
    zip.start_file("Game.txt", options).unwrap();
    zip.write_all(description.as_bytes()).unwrap();
    let bytes = zip.finish().unwrap().into_inner();

    let out = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/e2e-pack.zip");
    std::fs::write(out, &bytes).unwrap();
    println!("wrote {} ({} bytes)", out, bytes.len());
}
