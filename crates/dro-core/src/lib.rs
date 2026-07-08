//! Core of DRO Trimmer: the DRO/VGM data model, file formats, analysis and undo.
//!
//! This crate deliberately has no audio, GUI or filesystem dependencies, so it
//! compiles unchanged for `wasm32-unknown-unknown`. Readers and writers take
//! `&[u8]` and return `Vec<u8>`; locating and opening files is the caller's job.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod regdata;
pub mod song;
pub mod undo;
pub mod util;

pub use error::{Error, Result};
pub use song::{
    Bank, DelayKind, DroData, DroDataV1, DroDataV2, DroInstruction, FindTarget, OplType, Song,
    SongFileType,
};
pub use undo::{UndoController, UndoableCommand};
