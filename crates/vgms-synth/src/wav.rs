//! Offline WAV rendering.
//!
//! A plain loop over an engine's `render`, writing into an in-memory `hound`
//! WAV. The same bytes result on native and web -- the caller writes them to disk
//! or offers them as a download.
//!
//! One engine for every document: [`VgmEngine`](crate::vgm_engine::VgmEngine)
//! plays any VGM, and a DRO is projected to its VGM before it reaches here, so a
//! DRO's export sounds exactly like its playback.

use std::io::Cursor;

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::chip_mix::{ChipMuting, ChipPanning};
use crate::resample::ResampleMode;
use std::sync::Arc;

use vgms_core::VgmFile;

use crate::limiter::BoostLimiter;
use crate::vgm_engine::VgmEngine;

/// Renders a VGM for whatever chips it declares, through the multi-chip engine.
///
/// The unmixed convenience over [`render_vgm_wav_mixed_cancellable`]. A chip with
/// no core renders silence, so a file this app only half knows comes out half
/// played -- check [`playability`](crate::chip::playability) first if that
/// matters.
///
/// # Errors
/// If the WAV cannot be written.
pub fn render_vgm_wav(
    file: Arc<VgmFile>,
    sample_rate: u32,
    bit_depth: u16,
    boost: f32,
    resampling: ResampleMode,
) -> Result<Vec<u8>, hound::Error> {
    render_vgm_wav_cancellable(
        file,
        sample_rate,
        bit_depth,
        boost,
        resampling,
        &mut |_| {},
        &mut || true,
    )
    .map(|bytes| bytes.unwrap_or_default())
}

/// As [`render_vgm_wav`], reporting progress and stopping when `keep_going`
/// returns `false`. `Ok(None)` iff it did.
///
/// # Errors
/// See [`render_vgm_wav`].
pub fn render_vgm_wav_cancellable(
    file: Arc<VgmFile>,
    sample_rate: u32,
    bit_depth: u16,
    boost: f32,
    resampling: ResampleMode,
    on_progress: &mut dyn FnMut(u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<u8>>, hound::Error> {
    let mix = VgmRenderMix {
        boost,
        ..VgmRenderMix::default()
    };
    render_vgm_wav_mixed_cancellable(
        file,
        sample_rate,
        bit_depth,
        &mix,
        resampling,
        on_progress,
        keep_going,
    )
}

/// A multichip render's mix: which channels of which chips are silenced or
/// placed, and how hard the signal is driven.
///
/// [`Default`] is the faithful render: nothing muted, every chip's own
/// stereo image, no boost.
#[derive(Debug, Clone, PartialEq)]
pub struct VgmRenderMix {
    pub muting: ChipMuting,
    pub panning: ChipPanning,
    /// Multiplies the signal through the playback peak limiter. `1.0` is
    /// bit-transparent.
    pub boost: f32,
}

impl Default for VgmRenderMix {
    fn default() -> Self {
        Self {
            muting: ChipMuting::new(),
            panning: ChipPanning::new(),
            boost: 1.0,
        }
    }
}

/// As [`render_vgm_wav_cancellable`], with channel mutes and pans baked into
/// the render -- what the GUI's toggles and knobs export, and what the
/// per-channel split renders each solo through.
///
/// # Errors
/// See [`render_vgm_wav`].
pub fn render_vgm_wav_mixed_cancellable(
    file: Arc<VgmFile>,
    sample_rate: u32,
    bit_depth: u16,
    mix: &VgmRenderMix,
    resampling: ResampleMode,
    on_progress: &mut dyn FnMut(u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<u8>>, hound::Error> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: bit_depth,
        sample_format: SampleFormat::Int,
    };
    let mut engine = VgmEngine::new(file, sample_rate);
    // The render honours the same choice playback does: a user who picked the
    // crunchy conversion exports the sound they hear, not a cleaned-up cousin.
    engine.set_resample_mode(resampling);
    // Only when they say something: the faithful render stays exactly the
    // engine's own output, mirroring the OPL render's `Panning::Original`
    // rule.
    if !mix.muting.is_neutral() {
        engine.set_muting(mix.muting.clone());
    }
    if !mix.panning.is_neutral() {
        engine.set_panning(mix.panning.clone());
    }
    let mut rendered = 0u64;
    write_render(
        spec,
        bit_depth,
        mix.boost,
        &mut |buffer| {
            let frames = engine.render(buffer);
            rendered += frames as u64;
            (frames, rendered)
        },
        on_progress,
        keep_going,
    )
}

/// The write loop both renderers share: pull frames, boost and limit them,
/// encode them, and stop when the source runs out or the caller says so.
///
/// `pull` fills the buffer and reports `(frames written, frames rendered so
/// far)` -- the second being what a progress bar counts, which the two engines
/// track differently.
fn write_render(
    spec: WavSpec,
    bit_depth: u16,
    boost: f32,
    pull: &mut dyn FnMut(&mut [i16]) -> (usize, u64),
    on_progress: &mut dyn FnMut(u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<u8>>, hound::Error> {
    let mut cursor = Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut cursor, spec)?;
    let mut limiter = BoostLimiter::new(spec.sample_rate, boost);
    let mut buffer = vec![0i16; 4096 * 2];
    loop {
        // Between chunks, as the waveform render does: often enough that an
        // abandoned export stops promptly, never mid-buffer.
        if !keep_going() {
            return Ok(None);
        }
        let (frames, rendered) = pull(&mut buffer);
        // Boost and limit exactly as the live audio callback does, so a boosted
        // render matches boosted playback. Bit-transparent when boost is 1.0, so
        // the faithful unboosted render path is unchanged.
        limiter.process(&mut buffer[..frames * 2]);
        for &sample in &buffer[..frames * 2] {
            if bit_depth == 8 {
                // WAV 8-bit is written through hound's i8 sample; the top byte of
                // the 16-bit render is the natural down-conversion.
                writer.write_sample((sample >> 8) as i8)?;
            } else {
                writer.write_sample(sample)?;
            }
        }
        on_progress(rendered);
        if frames < buffer.len() / 2 {
            break;
        }
    }

    writer.finalize()?;
    Ok(Some(cursor.into_inner()))
}
