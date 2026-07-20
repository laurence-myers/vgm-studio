//! Core of DRO Trimmer: the DRO/VGM data model, file formats, analysis and undo.
//!
//! This crate deliberately has no audio, GUI or filesystem dependencies, so it
//! compiles unchanged for `wasm32-unknown-unknown`. Readers and writers take
//! `&[u8]` and return `Vec<u8>`; locating and opening files is the caller's job.

#![forbid(unsafe_code)]

pub mod analysis;
pub mod config;
pub mod convert;
pub mod error;
pub mod io;
pub mod regdata;
pub mod rip;
pub mod song;
pub mod undo;
pub mod util;
pub mod vgm;

pub use analysis::{RegisterAnalyzer, RegisterUsage, RowAnalysis, initial_channel_pans};
pub use error::{Error, Result};
pub use rip::{RipMeta, TrackEntry};
pub use song::{
    Bank, DelayKind, DroDataV1, DroDataV2, DroInstruction, FindTarget, OplType, Song, SongData,
    SongFileType, slide_index_past_deletion,
};
pub use undo::{UndoController, UndoableCommand};
pub use vgm::{Gd3Tag, VgmData, VgmMeta};
