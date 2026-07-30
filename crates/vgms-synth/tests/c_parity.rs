//! Is the pure-Rust `nuked-opl3` bit-identical to the Nuked-OPL3 **C** sources?
//! `opl3-rs` compiles Nuke.YKT's original C, so we run both and compare.
//!
//! Off by default -- it needs a C compiler and libclang:
//!
//! ```text
//! cargo test -p vgms-synth --features c-parity
//! ```

#![cfg(feature = "c-parity")]

mod common;

use common::{Op, render, script};
use vgms_synth::{CReferenceOpl3, NATIVE_SAMPLE_RATE, NukedOpl3, OplChip};

/// Reports the first differing sample rather than dumping 260k values.
fn assert_pcm_eq(rust: &[i16], reference: &[i16], context: &str) {
    assert_eq!(
        rust.len(),
        reference.len(),
        "{context}: frame counts differ"
    );
    if let Some(index) = rust.iter().zip(reference).position(|(a, b)| a != b) {
        panic!(
            "{context}: PCM diverges at sample {index} of {}: pure-Rust={} C={}",
            rust.len(),
            rust[index],
            reference[index],
        );
    }
}

#[test]
fn the_dro_fixture_renders_identically() {
    let script = script();
    let rust = render(
        &mut NukedOpl3::new(NATIVE_SAMPLE_RATE),
        NATIVE_SAMPLE_RATE,
        &script,
        128,
    );
    let reference = render(
        &mut CReferenceOpl3::new(NATIVE_SAMPLE_RATE),
        NATIVE_SAMPLE_RATE,
        &script,
        128,
    );

    assert!(!rust.is_empty());
    assert!(rust.iter().any(|&sample| sample != 0), "expected audio");
    assert_pcm_eq(&rust, &reference, "DRO fixture");
}

#[test]
fn opl3_high_bank_writes_render_identically() {
    // The DRO fixture is an OPL2 capture, so it never touches the high bank.
    let script = [
        Op::Write(0x105, 0x01), // OPL3 mode enable
        Op::Write(0x104, 0x3F), // four-operator enable, all pairs
        Op::Write(0x120, 0x21),
        Op::Write(0x140, 0x00),
        Op::Write(0x160, 0xF0),
        Op::Write(0x180, 0x77),
        Op::Write(0x1A0, 0x98),
        Op::Write(0x1B0, 0x31), // key on, high bank
        Op::Write(0x0BD, 0x20), // percussion mode, low bank
        Op::Delay(500),
    ];

    let rust = render(
        &mut NukedOpl3::new(NATIVE_SAMPLE_RATE),
        NATIVE_SAMPLE_RATE,
        &script,
        128,
    );
    let reference = render(
        &mut CReferenceOpl3::new(NATIVE_SAMPLE_RATE),
        NATIVE_SAMPLE_RATE,
        &script,
        128,
    );

    assert!(rust.iter().any(|&sample| sample != 0), "expected audio");
    assert_pcm_eq(&rust, &reference, "OPL3 high bank");
}

#[test]
fn random_register_traffic_renders_identically() {
    // SplitMix64: reproducible without pulling in a random-number dependency.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };

    let mut rust_chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
    let mut reference_chip = CReferenceOpl3::new(NATIVE_SAMPLE_RATE);
    let mut rust_buffer = [0i16; 64];
    let mut reference_buffer = [0i16; 64];

    for operation in 0..20_000u32 {
        let random = next();
        let reg = (random & 0x1FF) as u16; // both banks
        let mut value = ((random >> 16) & 0xFF) as u8;
        // `nuked-opl3` has `stereo-ext` but the C oracle (opl3-rs) does not, so a
        // write enabling stereo-ext (0x105 bit 1) would engage the Rust panpots
        // while C ignores it, diverging by construction. Clear that bit; the panpot
        // path is covered by tests/panning.rs.
        if reg == 0x105 {
            value &= !0x02;
        }
        rust_chip.write_reg(reg, value);
        reference_chip.write_reg(reg, value);

        if random % 7 == 0 {
            let frames = ((random >> 32) % 32 + 1) as usize;
            let rust = &mut rust_buffer[..frames * 2];
            let reference = &mut reference_buffer[..frames * 2];
            rust_chip.generate_samples(rust);
            reference_chip.generate_samples(reference);
            assert_pcm_eq(
                rust,
                reference,
                &format!("after {operation} random ops (reg={reg:#05X} value={value:#04X})"),
            );
        }
    }
}

#[test]
fn reset_leaves_both_cores_in_the_same_state() {
    let mut rust_chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
    let mut reference_chip = CReferenceOpl3::new(NATIVE_SAMPLE_RATE);

    for chip in [&mut rust_chip as &mut dyn OplChip, &mut reference_chip] {
        chip.write_reg(0x20, 0x01);
        chip.write_reg(0xA0, 0x98);
        chip.write_reg(0xB0, 0x31);
        let mut scratch = [0i16; 1024];
        chip.generate_samples(&mut scratch);
        chip.reset(NATIVE_SAMPLE_RATE);
    }

    let script = script();
    let rust = render(&mut rust_chip, NATIVE_SAMPLE_RATE, &script, 128);
    let reference = render(&mut reference_chip, NATIVE_SAMPLE_RATE, &script, 128);
    assert_pcm_eq(&rust, &reference, "after reset");
}
