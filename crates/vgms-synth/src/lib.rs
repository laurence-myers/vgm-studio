// SPDX-License-Identifier: MIT OR Apache-2.0
//! OPL emulation and the pull-based playback engine.
//!
//! Like [`vgms_core`], this crate has no threads, no audio device and no I/O, so
//! it compiles unchanged for `wasm32-unknown-unknown` and can be driven from a
//! cpal callback, an `AudioWorkletProcessor`, a WAV renderer or a tight offline
//! loop without knowing the difference.

#![forbid(unsafe_code)]

pub(crate) mod balance;
pub mod banks;
pub mod capture;
pub mod chip;
pub mod chip_mix;
pub mod credits;
pub mod dac_stream;
pub mod decompress;
pub mod engine;
pub mod limiter;
pub mod opl;
pub mod peak;
pub mod registry;
pub mod resample;
pub mod split;
#[cfg(test)]
pub(crate) mod testing;
pub mod vgm_engine;
pub mod wav;
pub mod waveform;
pub mod write_queue;

pub use banks::{Banks, BlockKind};
pub use capture::capture;
pub use chip::{ChipCore, Playability, RecordingChip, core_for, core_for_realtime, playability};
pub use chip_mix::{ChipMuting, ChipPanning};
pub use credits::{CoreCredit, credits, credits_text};
pub use dac_stream::{DacStreams, PendingWrite, StreamTarget};
pub use decompress::{DecompressionTable, decompress};
pub use engine::{FrameClock, LoopConfig, LoopCount, Muting, Panning, PlayerEngine, Position};
pub use limiter::BoostLimiter;
#[cfg(feature = "c-parity")]
pub use opl::CReferenceOpl3;
#[cfg(feature = "nuked-opl")]
pub use opl::NukedOpl3;
pub use opl::{DefaultOplChip, OplChip, SilentOpl};
pub use peak::{
    Peak, measure_peak, measure_peak_cancellable, measure_vgm_peak, measure_vgm_peak_cancellable,
};
pub use registry::{CoreInfo, CoreMaker, CoreRegistry, LEVEL_UNITY, install, registry};
pub use split::{
    SplitData, SplitFormat, SplitOptions, SplitOutput, VgmSplitOptions, split, split_cancellable,
    split_vgm_cancellable,
};
pub use vgm_engine::VgmEngine;
pub use wav::{
    RenderMix, VgmRenderMix, render_vgm_wav, render_vgm_wav_cancellable,
    render_vgm_wav_mixed_cancellable, render_wav, render_wav_boosted_with_progress,
    render_wav_cancellable, render_wav_mixed,
};
pub use waveform::{
    WaveformBucket, WaveformBucketer, render_vgm_waveform, render_vgm_waveform_progressive,
    render_waveform, render_waveform_cancellable, render_waveform_progressive,
};
pub use write_queue::WriteQueue;

/// What is being played: a decoded OPL stream, or a VGM for any chips.
///
/// The two go to different engines -- [`PlayerEngine`] carries the OPL register
/// policy (muting, panning, the buffered-write spacing), [`VgmEngine`] carries
/// none at all -- so an audio backend has to know which it has. This is that
/// question, asked once and answered where the backend can see it.
#[derive(Debug, Clone)]
pub enum AudioSource {
    Opl(std::sync::Arc<vgms_core::Song>),
    Vgm(std::sync::Arc<vgms_core::VgmFile>),
}

impl AudioSource {
    /// The file's name, for logs and errors.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Opl(song) => &song.name,
            Self::Vgm(file) => &file.name,
        }
    }

    /// The OPL stream, when there is one. A backend that can only play OPL --
    /// the RetroWave hardware -- asks this and refuses when the answer is `None`.
    #[must_use]
    pub fn opl(&self) -> Option<&std::sync::Arc<vgms_core::Song>> {
        match self {
            Self::Opl(song) => Some(song),
            Self::Vgm(_) => None,
        }
    }
}

/// The OPL3's native sample rate. Rendering here avoids the chip's resampler.
pub const NATIVE_SAMPLE_RATE: u32 = 49_716;
