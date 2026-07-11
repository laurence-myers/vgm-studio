//! OPL emulation and (from Step 4) the pull-based playback engine.
//!
//! Like [`dro_core`], this crate has no threads, no audio device and no I/O, so
//! it compiles unchanged for `wasm32-unknown-unknown` and can be driven from a
//! cpal callback, an `AudioWorkletProcessor`, a WAV renderer or a tight offline
//! loop without knowing the difference.

#![forbid(unsafe_code)]

pub mod engine;
pub mod opl;
pub mod wav;
pub mod waveform;

pub use engine::{FrameClock, Muting, PlayerEngine, Position};
#[cfg(feature = "c-parity")]
pub use opl::CReferenceOpl3;
pub use opl::{NukedOpl3, OplChip};
pub use wav::render_wav;
pub use waveform::{WaveformBucket, WaveformBucketer, render_waveform};

/// The OPL3's native sample rate. Rendering here avoids the chip's resampler.
pub const NATIVE_SAMPLE_RATE: u32 = 49_716;
