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
pub mod channel_gate;
pub mod chip;
pub mod chip_mix;
pub mod credits;
pub mod dac_stream;
pub mod decompress;
pub mod engine;
pub mod limiter;
pub mod opl;
pub mod opl_adapter;
pub mod opl_chip_mix;
pub mod peak;
pub mod registry;
pub mod resample;
mod song_gate;
pub mod split;
#[cfg(test)]
pub(crate) mod testing;
pub mod vgm_engine;
pub mod wav;
pub mod waveform;
pub mod write_queue;

pub use banks::{Banks, BlockKind};
pub use channel_gate::{ChannelGate, GateAction};
pub use chip::{ChipCore, Playability, RecordingChip, core_for, core_for_realtime, playability};
pub use chip_mix::{ChipMuting, ChipPanning, ChipTrims};
pub use credits::{CoreCredit, credits, credits_text};
pub use dac_stream::{DacStreams, PendingWrite, StreamTarget};
pub use decompress::{DecompressionTable, decompress};
pub use engine::{DroEngine, FrameClock, LoopConfig, LoopCount, Muting, Panning, Position};
pub use limiter::BoostLimiter;
#[cfg(feature = "c-parity")]
pub use opl::CReferenceOpl3;
#[cfg(feature = "nuked-opl")]
pub use opl::NukedOpl3;
pub use opl::{DefaultOplChip, OplChip, SilentOpl};
pub use opl_adapter::OplCoreAdapter;
pub use opl_chip_mix::{
    opl_chip_muting, opl_chip_panning, opl_muting_from_chip, opl_projection_kind,
};
pub use peak::{
    Peak, measure_dro_peak, measure_dro_peak_cancellable, measure_vgm_peak,
    measure_vgm_peak_cancellable,
};
pub use registry::{
    CoreChoices, CoreInfo, CoreMaker, CoreRegistry, LEVEL_UNITY, gate_without_forwarding, install,
    opl_hardware_core, registry, with_render_choices,
};
pub use split::{SplitData, SplitFormat, SplitOutput, VgmSplitOptions, split_vgm_cancellable};
pub use vgm_engine::VgmEngine;
pub use wav::{
    RenderMix, VgmRenderMix, render_dro_wav, render_dro_wav_boosted_with_progress,
    render_dro_wav_cancellable, render_dro_wav_mixed, render_vgm_wav, render_vgm_wav_cancellable,
    render_vgm_wav_mixed_cancellable,
};
pub use waveform::{
    WaveformBucket, WaveformBucketer, render_dro_waveform, render_dro_waveform_cancellable,
    render_dro_waveform_progressive, render_vgm_waveform, render_vgm_waveform_progressive,
};
pub use write_queue::WriteQueue;

/// What is being played: a decoded OPL stream, or a VGM for any chips.
///
/// The two go to different engines -- [`DroEngine`] carries the OPL register
/// policy (muting, panning, the buffered-write spacing), [`VgmEngine`] carries
/// none at all -- so an audio backend has to know which it has. This is that
/// question, asked once and answered where the backend can see it.
///
/// It is [`vgms_core::DocSource`] under the name the synth's public API has
/// always used. The type lives in the core so the UI's loop-search, WAV and
/// split sources are the same type, not four copies of it, and so `vgms-synth`
/// can take it by value without either crate re-declaring the pair.
pub use vgms_core::DocSource as AudioSource;

/// The OPL3's native sample rate. Rendering here avoids the chip's resampler.
pub const NATIVE_SAMPLE_RATE: u32 = 49_716;
