//! rs-2: the song-format channel split re-render check.
//!
//! The song-format split ([`split_vgm_cancellable`] with [`SplitFormat::Song`])
//! rewrites a VGM's command stream into one standalone VGM per channel, dropping
//! or transforming the writes that would sound on any other channel. This proves
//! the *emitted* VGM is faithful: rendered back through a real core, each stem
//! sounds like the same channel soloed by that core's own native mute -- the
//! WAV-split reference.
//!
//! Like the gate A/B harness ([`gate_ab_parity`](super)), it is a lag-tolerant
//! correlation plus a level check, not byte equality: the split reaches silence
//! by forcing volumes where native mute masks the output, so tiny deviations are
//! expected. A wrong rewrite -- the wrong channel silenced, timing skewed, a
//! transformed write mis-encoded -- shows up as a stem that fails to match its
//! native solo.

use std::sync::Arc;

use vgms_app::parity::{self, Render, Settings};
use vgms_core::VgmFile;
use vgms_core::config::AudioConfig;
use vgms_core::vgm::ChipKind;
use vgms_synth::resample::ResampleMode;
use vgms_synth::vgm_engine::VgmEngine;
use vgms_synth::{
    ChipMuting, CoreChoices, SplitData, SplitFormat, VgmSplitOptions, registry,
    split_vgm_cancellable,
};

/// Render at the engine's native rate so the resampler never enters the
/// comparison.
const RATE: u32 = vgms_synth::NATIVE_SAMPLE_RATE;
/// Half a second is plenty of tone for a stable RMS.
const FRAMES: usize = RATE as usize / 2;

/// A synthetic SN76489 capture with all four channels sounding at distinct
/// pitches, so soloing each produces different, non-silent audio.
fn sn76489_all_four() -> Arc<VgmFile> {
    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
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
    // The header's total-sample field agrees with the stream's one wait.
    put_u32(&mut bytes, 0x18, 22_050);
    bytes.extend_from_slice(stream);
    let eof = bytes.len();
    put_u32(&mut bytes, 0x04, (eof - 4) as u32);
    Arc::new(vgms_core::vgm::file::read("sn76489.vgm", &bytes).expect("a walkable VGM"))
}

/// The pinned libvgm SN76489 core, whose `channel_mute: true` is the native
/// reference the gate and the split are measured against.
fn libvgm_only() -> CoreChoices {
    CoreChoices::from([("sn76489".to_owned(), "libvgm".to_owned())])
}

/// Runs `engine` for [`FRAMES`] frames into a [`Render`].
fn drain(mut engine: VgmEngine) -> Render {
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

/// Renders `file` through the libvgm core with only `channel` audible, isolated
/// by the core's own native mute -- the WAV-split reference.
fn native_solo(file: &Arc<VgmFile>, channel: u8) -> Render {
    let choices = libvgm_only();
    let factory = move |kind: ChipKind| registry().build_with(&choices, kind);
    let mut engine = VgmEngine::with_cores(Arc::clone(file), RATE, factory);
    let mut muting = ChipMuting::new();
    muting.set(ChipKind::Sn76489, 0, 0xF & !(1u32 << channel));
    engine.set_muting(muting);
    drain(engine)
}

/// Song-splits `file` and returns the stem VGM for SN76489 channel `channel`.
fn song_stem(file: &Arc<VgmFile>, channel: usize) -> Arc<VgmFile> {
    let options = VgmSplitOptions {
        format: SplitFormat::Song,
        audio: AudioConfig::default(),
        resampling: ResampleMode::default(),
        panning: vgms_synth::ChipPanning::new(),
        boost: 1.0,
        skip_muted: None,
        core_choices: CoreChoices::new(),
    };
    let outputs = split_vgm_cancellable(file, &options, &mut |_| {}, &mut |_, _| {}, &mut || true)
        .expect("the split succeeds")
        .expect("not cancelled");

    let fragment = format!("sn76489.{channel:02}-");
    let stem = outputs
        .into_iter()
        .find(|o| o.name.contains(&fragment))
        .unwrap_or_else(|| panic!("no stem for channel {channel}"));
    match stem.data {
        SplitData::Vgm(file) => Arc::new(*file),
        other => panic!("a song split must produce a VGM, got {other:?}"),
    }
}

/// Renders `file` through the libvgm core with nothing muted -- how a stem is
/// played back, its muted channels already forced silent in the stream.
fn render_plain(file: &Arc<VgmFile>) -> Render {
    let choices = libvgm_only();
    let factory = move |kind: ChipKind| registry().build_with(&choices, kind);
    drain(VgmEngine::with_cores(Arc::clone(file), RATE, factory))
}

/// Each SN76489 stem the song split emits, rendered back through the core,
/// sounds like the same channel soloed by the core's native mute.
#[test]
fn each_song_stem_renders_like_its_native_mute_solo() {
    vgms_app::install_cores();
    let file = sn76489_all_four();

    for channel in 0usize..4 {
        let native = native_solo(&file, channel as u8);
        let rendered = render_plain(&song_stem(&file, channel));

        // The soloed channel must actually sound, or the comparison proves
        // nothing.
        let peak = native.channels()[0]
            .iter()
            .fold(0.0f64, |m, s| m.max(s.abs()));
        assert!(peak > 0.01, "channel {channel} produced no native audio");

        let score = parity::compare(&native, &rendered, Settings::default());
        let correlation = score.worst_correlation();
        assert!(
            correlation >= 0.99,
            "channel {channel}: the song stem diverges from the native solo \
             (correlation {correlation:.4})"
        );
        for lr in 0..2 {
            let rms = score.channels[lr].rms_ratio;
            assert!(
                (0.9..1.1).contains(&rms),
                "channel {channel}: stem vs native level differs (rms_ratio {rms:.3})"
            );
        }
    }
}
