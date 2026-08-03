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
//! **`#[ignore]` -- it cannot pass yet, and that is the point.** Running it
//! measured the real state of the gate: `VgmEngine` renders an OPL VGM as
//! **silence** (correlation 0.0000, rms_ratio 0.000). The cause is
//! architectural, not a bug -- `CoreInfo::build()` returns `None` for
//! `CoreMaker::Opl` (`registry.rs:152`), because OPL cores answer to
//! `PlayerEngine` (as `Box<dyn OplChip>` via `build_opl`), not to `VgmEngine`
//! (which pulls samples from a `Box<dyn ChipCore>`). So the first thing k-1
//! needs is not the channel gate but an `OplChip`->`ChipCore` adapter that lets
//! `VgmEngine` host an OPL chip at all; this test is that adapter's acceptance
//! gate. Run it once the adapter lands:
//!
//! ```text
//! cargo test -p vgms-app --test opl_ab_parity -- --ignored --nocapture
//! ```

use std::sync::Arc;

use vgms_app::parity::{self, Render, Settings};
use vgms_synth::vgm_engine::VgmEngine;

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

/// The generic path: `file` -> `VgmEngine` over the registered nuked-opl3 core.
fn render_vgm_engine(name: &str, bytes: &[u8]) -> Render {
    let file = vgms_core::vgm::file::read(name, bytes).expect("the fixture reads");
    let mut engine = VgmEngine::new(Arc::new(file), RATE);
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

#[test]
#[ignore = "k-1 not built: VgmEngine has no OPL ChipCore, so it renders OPL as silence (see module doc)"]
fn an_opl_vgm_sounds_the_same_through_both_engines() {
    // `install_cores()` registers opl3.nuked, but only as a `CoreMaker::Opl`
    // (an `OplChip` for `PlayerEngine`); `CoreInfo::build()` returns `None` for
    // it, so `VgmEngine` still cannot host it. This call is here for when the
    // k-1 adapter makes the OPL core buildable as a `ChipCore`.
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
