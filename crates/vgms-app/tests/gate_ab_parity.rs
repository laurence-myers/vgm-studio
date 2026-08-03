//! pm-5: the channel-gate A/B harness.
//!
//! The gate ([`vgms_synth::ChannelGate`]) makes per-channel muting work on a core
//! whose emulator has none of its own, by *write-gating* -- forcing a muted
//! channel silent through the register stream. This proves it isolates a channel
//! the same way a real emulator's *native* mute does, by rendering each channel
//! soloed two ways and comparing:
//!
//! - **native**: the libvgm core, muted through its own `set_channel_mutes`.
//! - **gate**: the same libvgm core wrapped in the gate with mask-forwarding
//!   *disabled* ([`gate_without_forwarding`]), so only the gate's write-filtering
//!   silences the channel -- otherwise the libvgm core underneath would mute
//!   natively too and the comparison would be vacuous.
//!
//! It is a lag-tolerant correlation plus a level check, not byte equality: the two
//! paths reach silence differently (native masks the output, the gate forces the
//! volume register), so tiny deviations are expected. A wrong gate table -- the
//! wrong channel silenced, the wrong register touched -- shows up as a channel
//! that fails to match its native solo.
//!
//! This is the harness the plan makes a prerequisite for growing the exotic gate
//! tables (rs-2): each new table earns its `exists()` by passing here.

use std::sync::Arc;

use vgms_app::parity::{self, Render, Settings};
use vgms_core::VgmFile;
use vgms_core::vgm::ChipKind;
use vgms_synth::vgm_engine::VgmEngine;
use vgms_synth::{ChipMuting, CoreChoices, gate_without_forwarding, registry};

/// Render at the engine's native rate so both arms resample identically (and so
/// the resampler never enters the comparison).
const RATE: u32 = vgms_synth::NATIVE_SAMPLE_RATE;
/// Half a second is plenty of tone for a stable RMS.
const FRAMES: usize = RATE as usize / 2;

/// A synthetic SN76489 capture with all four channels sounding at distinct
/// pitches, so soloing each produces different, non-silent audio.
fn sn76489_all_four() -> Arc<VgmFile> {
    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    // Command port writes (`0x50 dd`) that latch three tones at different periods
    // and start the noise, each at full volume, then wait half a second.
    let stream: &[u8] = &[
        0x50, 0x8F, 0x50, 0x0F, 0x50, 0x90, // tone 0: period 0x0FF, loud
        0x50, 0xA0, 0x50, 0x08, 0x50, 0xB0, // tone 1: period 0x080, loud
        0x50, 0xC0, 0x50, 0x04, 0x50, 0xD0, // tone 2: period 0x040, loud
        0x50, 0xE4, 0x50, 0xF0, // noise: white/medium, loud
        0x61, 0x22, 0x56, // wait 0x5622 = 22050 samples
        0x66, // end
    ];
    let mut bytes = vec![0u8; 0x100];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, 0x08, 0x171);
    put_u32(&mut bytes, 0x34, 0x100 - 0x34);
    put_u32(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
    bytes.extend_from_slice(stream);
    let eof = bytes.len();
    put_u32(&mut bytes, 0x04, (eof - 4) as u32);
    Arc::new(vgms_core::vgm::file::read("sn76489.vgm", &bytes).expect("a walkable VGM"))
}

/// Renders `file` with only `channel` audible, isolated either by the core's
/// native mute or by the gate (with forwarding off).
fn render_solo(file: &Arc<VgmFile>, channel: u8, gate: bool) -> Render {
    // Pin the libvgm core (its `channel_mute: true` is the native reference); the
    // gate arm wraps that exact core with forwarding disabled.
    let choices = CoreChoices::from([("sn76489".to_owned(), "libvgm".to_owned())]);
    let factory = move |kind: ChipKind| -> Option<Box<dyn vgms_synth::ChipCore>> {
        let core = registry().build_with(&choices, kind)?;
        if gate {
            gate_without_forwarding(core, kind)
        } else {
            Some(core)
        }
    };
    let mut engine = VgmEngine::with_cores(Arc::clone(file), RATE, factory);

    // Solo `channel`: mute every other channel of the four.
    let solo = 0xF & !(1u32 << channel);
    let mut muting = ChipMuting::new();
    muting.set(ChipKind::Sn76489, 0, solo);
    engine.set_muting(muting);

    let mut samples = Vec::with_capacity(FRAMES * 2);
    let mut buffer = vec![0i16; 4096 * 2];
    while samples.len() < FRAMES * 2 {
        let rendered = engine.render(&mut buffer);
        if rendered == 0 {
            break;
        }
        samples.extend_from_slice(&buffer[..rendered * 2]);
    }
    Render::from_interleaved_i16(&samples, RATE)
}

/// Every SN76489 channel, soloed by the gate, sounds like the same channel soloed
/// by the core's native mute.
#[test]
fn sn76489_gate_solos_match_native_mute() {
    vgms_app::install_cores();
    let file = sn76489_all_four();

    for channel in 0u8..4 {
        let native = render_solo(&file, channel, false);
        let gated = render_solo(&file, channel, true);

        // The soloed channel must actually be sounding, or the comparison proves
        // nothing.
        let peak = native.channels()[0]
            .iter()
            .fold(0.0f64, |m, s| m.max(s.abs()));
        assert!(
            peak > 0.01,
            "channel {channel} produced no native audio to compare"
        );

        let score = parity::compare(&native, &gated, Settings::default());
        let correlation = score.worst_correlation();
        assert!(
            correlation >= 0.99,
            "channel {channel}: the gate's solo diverges from the native mute \
             (correlation {correlation:.4}); the gate table may be wrong"
        );
        for lr in 0..2 {
            let rms = score.channels[lr].rms_ratio;
            assert!(
                (0.9..1.1).contains(&rms),
                "channel {channel}: gate vs native level differs (rms_ratio {rms:.3})"
            );
        }
    }
}

/// With nothing muted, the gate wrapper is transparent: a full render through the
/// gate is identical to one through the bare core, so the gate never changes the
/// faithful render.
#[test]
fn an_unmuted_gate_render_matches_the_bare_core() {
    vgms_app::install_cores();
    let file = sn76489_all_four();

    let choices = CoreChoices::from([("sn76489".to_owned(), "libvgm".to_owned())]);
    let render = |gate: bool| -> Vec<i16> {
        let choices = choices.clone();
        let factory = move |kind: ChipKind| -> Option<Box<dyn vgms_synth::ChipCore>> {
            let core = registry().build_with(&choices, kind)?;
            if gate {
                gate_without_forwarding(core, kind)
            } else {
                Some(core)
            }
        };
        let mut engine = VgmEngine::with_cores(Arc::clone(&file), RATE, factory);
        // No muting applied at all.
        let mut samples = Vec::with_capacity(FRAMES * 2);
        let mut buffer = vec![0i16; 4096 * 2];
        while samples.len() < FRAMES * 2 {
            let rendered = engine.render(&mut buffer);
            if rendered == 0 {
                break;
            }
            samples.extend_from_slice(&buffer[..rendered * 2]);
        }
        samples
    };

    assert_eq!(
        render(true),
        render(false),
        "an unmuted gate must be byte-for-byte the bare core"
    );
}
