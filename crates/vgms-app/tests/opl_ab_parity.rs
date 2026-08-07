//! The OPL channel gate isolates distinct channels through the one `VgmEngine`.
//!
//! OPL playback and rendering run through `VgmEngine` over the OPL core adapter
//! ([`CoreInfo::build`](vgms_synth::OplCoreAdapter) wraps a `CoreMaker::Opl`), not
//! a separate engine. This file once also held an A/B that rendered a DRO through
//! the now-deleted `DroEngine` and against its projection, to prove the reroute
//! was audio-clean; with `DroEngine` gone there is no second engine to compare,
//! and that proof lives on in the DRO-projection render tests (the
//! `render_regression` fixtures and the GUI waveform snapshots pin the projected
//! render). What remains here is the gate's own contract: muting every channel
//! silences the render, each soloed channel that sounds is audible, and two
//! solos are distinct -- a gate that leaked or mapped the wrong row would fail.
//!
//! Comparison uses the VGMPlay parity harness's 20 Hz high-passed, lag-tolerant
//! cross-correlation plus a level (`rms_ratio`) watch, not byte equality.

use std::sync::Arc;

use vgms_app::parity::{self, Render, Settings};
use vgms_synth::ChipMuting;
use vgms_synth::vgm_engine::VgmEngine;

/// Render both engines at the OPL core's native rate, so neither side's
/// resampler enters the measurement -- both drive the same Nuked OPL3 core 1:1.
const RATE: u32 = vgms_synth::NATIVE_SAMPLE_RATE;
/// Runs a `VgmEngine` for up to `secs` seconds into a [`Render`]. The gate test
/// uses a short window -- isolation, silence and distinctness all show in a couple
/// of seconds, and the Nuked OPL3 core renders slower than real time.
fn drain_secs(mut engine: VgmEngine, secs: usize) -> Render {
    let wanted = RATE as usize * secs * 2;
    let mut samples = Vec::with_capacity(wanted);
    let mut buffer = vec![0i16; 4096 * 2];
    while samples.len() < wanted {
        let rendered = engine.render(&mut buffer);
        if rendered == 0 {
            break;
        }
        samples.extend_from_slice(&buffer[..rendered * 2]);
    }
    Render::from_interleaved_i16(&samples, RATE)
}

/// The OPL channel gate isolates distinct channels through `VgmEngine`: muting
/// every channel silences the render, each soloed channel that sounds is audible,
/// and no two solos are the same audio -- which a gate that leaked or mapped the
/// wrong row would fail.
///
/// This once held the gate against a `DroEngine` A/B reference; `DroEngine` is
/// gone, so the gate's own contract is checked directly on the one engine that
/// remains.
#[test]
fn the_opl_channel_gate_isolates_channels_through_the_vgm_engine() {
    // A short window: the Nuked OPL3 core renders slower than real time, and every
    // property here shows within a couple of seconds.
    const SECS: usize = 2;

    vgms_app::install_cores();
    let bytes = include_bytes!("../../../tests/lsl3_score_up.vgm");
    let file =
        Arc::new(vgms_core::vgm::file::read("lsl3_score_up.vgm", &bytes[..]).expect("reads"));
    // The fixture's actual OPL chip (lsl3 is a YM3812 / OPL2), so the mask targets
    // the voice that exists.
    let kind = file.header.chips()[0].kind;

    let peak = |render: &Render| {
        render.channels()[0]
            .iter()
            .fold(0.0f64, |m, s| m.max(s.abs()))
    };
    let solo = |ch: u8| {
        let mut engine = VgmEngine::new(Arc::clone(&file), RATE);
        let mut muting = ChipMuting::new();
        muting.set(kind, 0, !(1u32 << u32::from(ch))); // mute all but `ch`
        engine.set_muting(muting);
        drain_secs(engine, SECS)
    };

    // Baseline: the whole song sounds.
    let full = drain_secs(VgmEngine::new(Arc::clone(&file), RATE), SECS);
    assert!(peak(&full) > 0.02, "the fixture should sound");

    // Muting every channel silences the render (a full mask, every bit set).
    let mut all_muted = VgmEngine::new(Arc::clone(&file), RATE);
    let mut muting = ChipMuting::new();
    muting.set(kind, 0, u32::MAX);
    all_muted.set_muting(muting);
    assert!(
        peak(&drain_secs(all_muted, SECS)) < 0.02,
        "muting every channel must silence the render"
    );

    // Solo low-bank melodic channels until two distinct sounding ones are found:
    // enough to prove the gate isolates rather than leaks or mis-maps.
    let mut soloed: Vec<(u8, Render)> = Vec::new();
    for ch in 0u8..9 {
        let render = solo(ch);
        if peak(&render) > 0.02 {
            soloed.push((ch, render));
            if soloed.len() == 2 {
                break;
            }
        }
    }
    assert_eq!(
        soloed.len(),
        2,
        "at least two channels should sound in isolation"
    );

    // The two solos are different audio -- a gate that leaked or mapped the wrong
    // row would make different solos identical.
    let correlation =
        parity::compare(&soloed[0].1, &soloed[1].1, Settings::default()).worst_correlation();
    assert!(
        correlation < 0.99,
        "channels {} and {} isolate to the same audio (correlation {correlation:.4}) -- \
         the gate leaked or mapped the wrong row",
        soloed[0].0,
        soloed[1].0
    );
}
