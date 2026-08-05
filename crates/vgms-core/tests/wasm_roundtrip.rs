//! The file-format round trips, executed as real `wasm32-unknown-unknown` code.
//!
//! `vgms-core` is integer-dominated and has no platform-specific paths, so byte
//! parity between native and web *should* be free. This is what makes it a tested
//! property rather than an assumption -- the web build writes the files users keep.
//!
//! ```text
//! cargo install wasm-bindgen-cli --locked
//! cargo test -p vgms-core --target wasm32-unknown-unknown
//! ```

#![cfg(target_arch = "wasm32")]

use vgms_core::convert::dro_to_vgm;
use vgms_core::io::{read_song, write_song};
use vgms_core::vgm::io as vgm_io;
use wasm_bindgen_test::wasm_bindgen_test;

const DRO_V2_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up_dro2.dro");
const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

#[wasm_bindgen_test]
fn dro_v2_round_trips_byte_for_byte() {
    let song = read_song("lsl3_score_up_dro2.dro", DRO_V2_FIXTURE).unwrap();
    assert_eq!(song.len(), 299);
    assert_eq!(song.ms_length, 2683);
    assert_eq!(write_song(&song).unwrap(), DRO_V2_FIXTURE);
}

#[wasm_bindgen_test]
fn vgm_round_trips_byte_for_byte() {
    let song = read_song("lsl3_score_up.vgm", VGM_FIXTURE).unwrap();
    assert_eq!(song.len(), 299);
    assert_eq!(song.total_delay_samples(), 118_320);
    assert_eq!(write_song(&song).unwrap(), VGM_FIXTURE);
}

/// `flate2`'s `rust_backend` is pure Rust, so VGZ works on wasm with no C.
#[wasm_bindgen_test]
fn vgz_round_trips() {
    let song = read_song("f.vgm", VGM_FIXTURE).unwrap();
    let compressed = vgm_io::write_gzipped(&song).unwrap();
    assert!(vgm_io::is_gzipped(&compressed));

    let reread = read_song("f.vgz", &compressed).unwrap();
    assert_eq!(reread.data(), song.data());
    assert_eq!(vgm_io::write(&reread).unwrap(), VGM_FIXTURE);
}

/// The whole-file conversion, on wasm. If the sample clock's integer arithmetic
/// ever differs between targets, this is where it shows.
#[wasm_bindgen_test]
fn dro_to_vgm_reproduces_the_vgm_fixture() {
    let dro = read_song("lsl3_score_up_dro2.dro", DRO_V2_FIXTURE).unwrap();
    let vgm = dro_to_vgm(&dro).unwrap();
    assert_eq!(vgm.header.total_samples(), 118_320);
    assert_eq!(vgms_core::vgm::file::write(&vgm).unwrap(), VGM_FIXTURE);
}

/// `usize` is 32 bits here, so a header claiming 2^31 pairs must not wrap.
#[wasm_bindgen_test]
fn a_corrupt_length_field_is_rejected_not_wrapped() {
    let mut bytes = DRO_V2_FIXTURE.to_vec();
    bytes[0x0C..0x10].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    assert!(read_song("f.dro", &bytes).is_err());
}

#[wasm_bindgen_test]
fn delete_and_undo_round_trip() {
    use vgms_core::undo::{DeleteInstructions, UndoController};

    let original = read_song("f.dro", DRO_V2_FIXTURE).unwrap();
    let mut song = original.clone();
    let mut undo = UndoController::new();

    undo.execute(Box::new(DeleteInstructions::new([1, 6, 3, 4])), &mut song);
    assert_eq!(song.len(), 295);
    undo.undo(&mut song);
    assert_eq!(song, original);
    assert_eq!(write_song(&song).unwrap(), DRO_V2_FIXTURE);
}
