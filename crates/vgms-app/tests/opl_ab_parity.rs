//! Stage K: OPL playback runs through the one `VgmEngine`, not a separate
//! `PlayerEngine` over an OPL projection.
//!
//! The gate that proved this originally compared an OPL VGM through both engines
//! and scored correlation 1.0000 -- the [`OplCoreAdapter`](vgms_synth::OplCoreAdapter)
//! `CoreInfo::build` wraps a `CoreMaker::Opl` in reproduces `PlayerEngine` 1:1.
//! With the projection retired (k-5) there is no longer a `PlayerEngine`-over-OPL
//! reference to compare an OPL VGM against, so that direct A/B is gone; what
//! remains guards the two guarantees that outlive it:
//!
//! - **The DRO path is audio-clean through `VgmEngine`.** A DRO still plays
//!   through `PlayerEngine` (`render_wav`), so it is a live reference: rendered
//!   directly, and projected to a `VgmFile` and run through `VgmEngine`, the two
//!   match to a fraction of a percent in level (the ms->sample->frame
//!   requantization floor keeps correlation just under 1). This is the same
//!   round trip the shipping "Convert to VGM" makes.
//! - **The OPL channel gate isolates channels through `VgmEngine`.** Muting every
//!   channel silences the render, each soloed channel that sounds is audible, and
//!   two solos are distinct -- the gate's own contract, checked directly on the
//!   engine that renders it.
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
/// A cap on how much to compare; the fixtures are all shorter than this.
const MAX_SECONDS: usize = 8;

/// Runs a `VgmEngine` for up to [`MAX_SECONDS`] into a [`Render`].
fn drain(engine: VgmEngine) -> Render {
    drain_secs(engine, MAX_SECONDS)
}

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

/// The reroute ou-2 makes: a DRO, today played through `PlayerEngine`, projected
/// to a `VgmFile` ([`convert::opl_song_to_vgm_file`]) and played through
/// `VgmEngine` -- the path the transport takes once OPL documents route to the
/// generic engine. Proves that round trip is audio-clean before any transport
/// flips onto it.
///
/// An OPL VGM plays byte-identically through the two engines (both drive the same
/// Nuked OPL3 core off the same command stream). The DRO path instead re-quantizes
/// its delays: a DRO's
/// native timing is **milliseconds**, and `PlayerEngine` renders those straight
/// to output frames (one rounding), whereas the projection expands them to VGM
/// **sample** delays at 44100 (`dro_to_vgm`) and the engine then rounds those to
/// output frames -- two roundings through an intermediate rate the direct path
/// never touches. VGM delays are 44100 by format definition, so every VGM (native
/// or projected) carries this quantization; a DRO joining the VGM path inherits
/// it. The result is sub-millisecond jitter in note onsets with **identical
/// energy** -- musically lossless, and the very conversion the shipping
/// "Convert to VGM" feature already produces.
///
/// So the bar is not the VGM case's 0.99: the strict guarantee is on the level
/// (`rms_ratio`, which a dropped or mis-levelled channel would move), and the
/// correlation is held only above the requantization floor to still catch a real
/// divergence (a wrong or missing voice tanks both).
#[test]
fn a_dro_sounds_the_same_projected_through_the_vgm_engine() {
    vgms_app::install_cores();
    let bytes = include_bytes!("../../../tests/lsl3_score_up_dro2.dro");
    let dro =
        vgms_core::io::read_song("lsl3_score_up_dro2.dro", &bytes[..]).expect("the DRO reads");

    // Today's path: the DRO through PlayerEngine (what render_wav drives).
    let wav = vgms_synth::render_wav(&dro, RATE, 16).expect("the DRO renders");
    let (samples, wav_rate) = parity::reference::read_wav(&wav).expect("a valid WAV");
    let wanted = RATE as usize * MAX_SECONDS * 2;
    let player = Render::from_interleaved_i16(&samples[..samples.len().min(wanted)], wav_rate);

    // ou-2's path: the DRO projected to a VgmFile, through VgmEngine.
    let file =
        vgms_core::convert::opl_song_to_vgm_file(&dro).expect("the DRO projects to a VgmFile");
    let vgm_engine = drain(VgmEngine::new(Arc::new(file), RATE));

    let score = parity::compare(&player, &vgm_engine, Settings::default());
    let correlation = score.worst_correlation();
    let rms_lo = score.channels[0].rms_ratio.min(score.channels[1].rms_ratio);
    let rms_hi = score.channels[0].rms_ratio.max(score.channels[1].rms_ratio);
    eprintln!(
        "dro projection: correlation {correlation:.4}, rms_ratio {rms_lo:.3}..{rms_hi:.3}, \
         {} frames",
        score.frames
    );
    // The real guarantee: energy is preserved to a fraction of a percent. A
    // dropped channel or a volume-model error would move this well outside the
    // band; the measured value is 0.999.
    assert!(
        (0.99..1.01).contains(&rms_lo) && (0.99..1.01).contains(&rms_hi),
        "the DRO projection differs in level (rms_ratio {rms_lo:.3}..{rms_hi:.3}) -- \
         a channel or the volume model diverged, not just delay quantization"
    );
    // Above the ms->sample->frame requantization floor (measured ~0.977): high
    // enough that a wrong/missing voice (which tanks correlation) still fails,
    // loose enough to tolerate the inaudible onset jitter the projection adds.
    assert!(
        correlation >= 0.95,
        "the DRO diverges more than delay requantization explains (correlation \
         {correlation:.4}, want >= 0.95) -- likely a real defect in the projection"
    );
}
/// The OPL channel gate isolates distinct channels through `VgmEngine`: muting
/// every channel silences the render, each soloed channel that sounds is audible,
/// and no two solos are the same audio -- which a gate that leaked or mapped the
/// wrong row would fail.
///
/// This once held the gate against a `PlayerEngine` A/B reference built from the
/// OPL projection; that reference was the projection, now retired. `VgmEngine`
/// renders OPL identically to `PlayerEngine` (the DRO A/B above pins that), so the
/// gate's own contract is checked directly on the one engine that remains.
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
