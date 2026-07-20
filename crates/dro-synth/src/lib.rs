//! OPL emulation and (from Step 4) the pull-based playback engine.
//!
//! Like [`dro_core`], this crate has no threads, no audio device and no I/O, so
//! it compiles unchanged for `wasm32-unknown-unknown` and can be driven from a
//! cpal callback, an `AudioWorkletProcessor`, a WAV renderer or a tight offline
//! loop without knowing the difference.

#![forbid(unsafe_code)]

pub mod capture;
pub mod engine;
pub mod limiter;
pub mod opl;
pub mod split;
pub mod wav;
pub mod waveform;

pub use capture::capture;
pub use engine::{FrameClock, LoopConfig, LoopCount, Muting, Panning, PlayerEngine, Position};
pub use limiter::BoostLimiter;
#[cfg(feature = "c-parity")]
pub use opl::CReferenceOpl3;
pub use opl::{NukedOpl3, OplChip};
pub use split::{SplitData, SplitFormat, SplitOptions, SplitOutput, split};
pub use wav::{
    RenderMix, render_wav, render_wav_boosted, render_wav_boosted_with_progress, render_wav_mixed,
    render_wav_muted, render_wav_muted_with_progress,
};
pub use waveform::{
    WaveformBucket, WaveformBucketer, render_waveform, render_waveform_cancellable,
    render_waveform_progressive,
};

/// The OPL3's native sample rate. Rendering here avoids the chip's resampler.
pub const NATIVE_SAMPLE_RATE: u32 = 49_716;
