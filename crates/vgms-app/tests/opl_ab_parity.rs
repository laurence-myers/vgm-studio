//! Stage K gate #3: an OPL VGM must sound the same through both engines.
//!
//! Today an OPL VGM plays through `PlayerEngine` over its OPL projection; Stage K
//! (docs/review-2026-08/PLAN.md 12b) wants it to take the `VgmEngine` path -- the
//! registered nuked-opl3 core -- like every other VGM, so the projection can be
//! retired. Before any such routing can flip, the two engines must produce the
//! same sound. This is that gate.
//!
//! It will never be byte-identical: `PlayerEngine` buffers register writes,
//! `VgmEngine` paces them. So it compares the way the VGMPlay parity harness
//! does -- a 20 Hz high-passed, lag-tolerant cross-correlation, "no audible
//! difference", not byte equality -- and separately watches the level
//! (`rms_ratio`), which is where a per-chip balance the VGM path applies but the
//! OPL path does not would show (the volume-model concern PLAN.md 12b raises).
//!
//! **ou-1 landed, and this now passes** -- perfectly: all four fixtures score
//! correlation 1.0000, rms_ratio 1.000. Both paths drive the same Nuked OPL3 at
//! its native rate with the same buffered writes in the same order, so the
//! [`OplCoreAdapter`](vgms_synth::OplCoreAdapter) that `CoreInfo::build` now
//! wraps a `CoreMaker::Opl` in reproduces `PlayerEngine` 1:1. When it measured
//! silence (correlation 0.0000) it was because `CoreInfo::build()` returned
//! `None` for `CoreMaker::Opl` -- OPL cores answered only to `PlayerEngine` (as
//! `Box<dyn OplChip>` via `build_opl`), not to `VgmEngine` (which pulls samples
//! from a `Box<dyn ChipCore>`). The adapter closed that gap.

use std::sync::Arc;

use vgms_app::parity::{self, Render, Settings};
use vgms_core::Bank;
use vgms_synth::vgm_engine::VgmEngine;
use vgms_synth::{ChipMuting, Muting, RenderMix, render_wav_mixed};

/// Render both engines at the OPL core's native rate, so neither side's
/// resampler enters the measurement -- both drive the same Nuked OPL3 core 1:1.
const RATE: u32 = vgms_synth::NATIVE_SAMPLE_RATE;
/// A cap on how much to compare; the fixtures are all shorter than this.
const MAX_SECONDS: usize = 8;

/// The projection path: `file` -> `to_song` -> `PlayerEngine`, exactly as the
/// WAV export renders an OPL document today.
fn render_projection(name: &str, bytes: &[u8]) -> Render {
    let file = vgms_core::vgm::file::read(name, bytes).expect("the fixture reads");
    let song = file.to_song().expect("an OPL VGM projects to an OPL song");
    let wav = vgms_synth::render_wav(&song, RATE, 16).expect("the projection renders");
    let (samples, wav_rate) = parity::reference::read_wav(&wav).expect("a valid WAV comes back");
    let wanted = RATE as usize * MAX_SECONDS * 2;
    Render::from_interleaved_i16(&samples[..samples.len().min(wanted)], wav_rate)
}

/// Runs a `VgmEngine` for up to [`MAX_SECONDS`] into a [`Render`].
fn drain(mut engine: VgmEngine) -> Render {
    let wanted = RATE as usize * MAX_SECONDS * 2;
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

/// The generic path: `file` -> `VgmEngine` over the registered nuked-opl3 core.
fn render_vgm_engine(name: &str, bytes: &[u8]) -> Render {
    let file = vgms_core::vgm::file::read(name, bytes).expect("the fixture reads");
    drain(VgmEngine::new(Arc::new(file), RATE))
}

/// The OPL fixtures embedded in the tree: the real single-chip capture, and the
/// three purpose-built projection goldens (a plain capture, a full loop, an
/// early-ending loop) from mg-0.
fn opl_fixtures() -> [(&'static str, &'static [u8]); 4] {
    [
        (
            "lsl3_score_up.vgm",
            include_bytes!("../../../tests/lsl3_score_up.vgm"),
        ),
        (
            "projection_base.opl.vgm",
            include_bytes!("../../../tests/golden/projection_base.opl.vgm"),
        ),
        (
            "projection_looping.opl.vgm",
            include_bytes!("../../../tests/golden/projection_looping.opl.vgm"),
        ),
        (
            "projection_early_loop.opl.vgm",
            include_bytes!("../../../tests/golden/projection_early_loop.opl.vgm"),
        ),
    ]
}

/// The reroute ou-2 makes: a DRO, today played through `PlayerEngine`, projected
/// to a `VgmFile` ([`convert::opl_song_to_vgm_file`]) and played through
/// `VgmEngine` -- the path the transport takes once OPL documents route to the
/// generic engine. Proves that round trip is audio-clean before any transport
/// flips onto it.
///
/// Unlike the OPL-VGM gate above (which scores 1.0000, both sides sharing the
/// exact same VGM byte stream), the DRO path re-quantizes its delays: a DRO's
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

#[test]
fn an_opl_vgm_sounds_the_same_through_both_engines() {
    // `install_cores()` registers opl3.nuked as a `CoreMaker::Opl`; since ou-1,
    // `CoreInfo::build()` wraps that in an `OplCoreAdapter`, so `VgmEngine` hosts
    // it as a `ChipCore` -- which is what makes this gate pass.
    vgms_app::install_cores();

    for (name, bytes) in opl_fixtures() {
        let projection = render_projection(name, bytes);
        let vgm_engine = render_vgm_engine(name, bytes);
        let score = parity::compare(&projection, &vgm_engine, Settings::default());

        let correlation = score.worst_correlation();
        let rms_lo = score.channels[0].rms_ratio.min(score.channels[1].rms_ratio);
        let rms_hi = score.channels[0].rms_ratio.max(score.channels[1].rms_ratio);
        eprintln!(
            "{name}: correlation {correlation:.4}, rms_ratio {rms_lo:.3}..{rms_hi:.3}, \
             {} frames",
            score.frames
        );

        // Both drive the same Nuked OPL3 core; only write pacing differs, so the
        // bar is the OPL control group's 0.99, not byte equality.
        assert!(
            correlation >= 0.99,
            "{name}: the two engines diverge (correlation {correlation:.4}, want >= 0.99). \
             If real, Stage K's routing flip needs calibration before it can proceed."
        );
        // Level parity: lsl3 is single-chip, so `VgmEngine`'s per-chip balance is
        // unity and the two paths should match closely; a wide ratio would mean
        // the OPL path and the VGM path disagree on loudness.
        assert!(
            (0.5..2.0).contains(&rms_lo) && (0.5..2.0).contains(&rms_hi),
            "{name}: the engines differ in level (rms_ratio {rms_lo:.3}..{rms_hi:.3})"
        );
    }
}

/// Soloing an OPL channel through `VgmEngine`'s write gate (the OPL
/// `ChannelGate` rows, engaged now that the OPL row is `channel_mute: false`)
/// isolates the **same channel at the same level** as `PlayerEngine`'s own
/// register gating -- the validation the OPL gate rows had not yet had.
///
/// It is not a byte-parity check: the gate keeps a muted channel's frequency and
/// clears only its key bit (so a seek replays like live play), where PlayerEngine
/// drops the whole write, and those extra key-cleared writes interleave with
/// Nuked's spaced write buffer differently -- phase-shifting the soloed channel.
/// So the isolation itself is checked (which channel, how loud, to a tight peak
/// ratio) plus a positive correlation confirming it *is* that channel and not a
/// leak or the wrong one -- a mis-mapped gate row would isolate a different
/// channel and fail the peak match.
#[test]
fn muting_an_opl_channel_matches_the_projection() {
    vgms_app::install_cores();
    let (name, bytes) = (
        "lsl3_score_up.vgm",
        include_bytes!("../../../tests/lsl3_score_up.vgm"),
    );
    let file = Arc::new(vgms_core::vgm::file::read(name, &bytes[..]).expect("the fixture reads"));
    let song = file.to_song().expect("an OPL VGM projects");
    // The fixture's actual OPL chip (lsl3 is a YM3812 / OPL2, not an OPL3), so the
    // VgmEngine mask targets the voice that exists.
    let kind = file.header.chips()[0].kind;

    let mut compared = 0;
    // Solo each low-bank melodic channel: `channels_of(Ymf262)` index `ch` is the
    // low-bank channel `0xB0 + ch`, so the two vocabularies line up.
    for ch in 0u8..9 {
        // PlayerEngine: mute everything, allow only channel `ch`.
        let mut muting = Muting::silent();
        muting.allow_channel(Bank::Low, 0xB0 + ch);
        let wav = render_wav_mixed(
            &song,
            RenderMix {
                muting,
                ..RenderMix::default()
            },
            RATE,
            16,
        )
        .expect("the projection renders");
        let (samples, wav_rate) = parity::reference::read_wav(&wav).expect("a valid WAV");
        let wanted = RATE as usize * MAX_SECONDS * 2;
        let projection =
            Render::from_interleaved_i16(&samples[..samples.len().min(wanted)], wav_rate);

        // A channel the song never sounds makes the comparison vacuous.
        let peak = projection.channels()[0]
            .iter()
            .fold(0.0f64, |m, s| m.max(s.abs()));
        if peak < 0.02 {
            continue;
        }

        // VgmEngine: solo the same channel through the gate.
        let mut engine = VgmEngine::new(Arc::clone(&file), RATE);
        let mut muting = ChipMuting::new();
        muting.set(kind, 0, !(1u32 << u32::from(ch)));
        engine.set_muting(muting);
        let gated = drain(engine);

        let gate_peak = gated.channels()[0]
            .iter()
            .fold(0.0f64, |m, s| m.max(s.abs()));
        let ratio = gate_peak / peak;
        assert!(
            (0.9..1.1).contains(&ratio),
            "channel {ch}: the gate isolated a different level than PlayerEngine \
             (peak ratio {ratio:.3}) -- the gate row may map the wrong channel"
        );
        let correlation =
            parity::compare(&projection, &gated, Settings::default()).worst_correlation();
        assert!(
            correlation >= 0.5,
            "channel {ch}: the gate solo does not track PlayerEngine's channel \
             (correlation {correlation:.4}) -- likely a leak or the wrong channel"
        );
        compared += 1;
    }
    assert!(compared > 0, "no channel sounded, so nothing was validated");
}
