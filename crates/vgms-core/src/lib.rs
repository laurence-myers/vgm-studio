// SPDX-License-Identifier: MIT OR Apache-2.0
//! Core of VGM Studio: the DRO/VGM data model, file formats, analysis and undo.
//!
//! This crate deliberately has no audio, GUI or filesystem dependencies, so it
//! compiles unchanged for `wasm32-unknown-unknown`. Readers and writers take
//! `&[u8]` and return `Vec<u8>`; locating and opening files is the caller's job.

#![forbid(unsafe_code)]

pub mod analysis;
pub mod chip_docs;
pub mod chip_state;
pub mod config;
pub mod convert;
pub mod crop;
pub mod doc_source;
pub mod error;
pub mod io;
pub mod loopfind;
pub mod opl_state;
pub mod optimize;
pub mod pack;
pub mod regdata;
pub mod song;
pub mod split_songs;
pub(crate) mod state_patch;
pub mod undo;
pub mod util;
pub mod vgm;
pub mod volume;

pub use analysis::{
    RegisterAnalyzer, RegisterUsage, RowAnalysis, initial_channel_pans, initial_channel_pans_vgm,
};
pub use chip_state::ChipState;
pub use crop::{CropOutcome, crop_to_region, delete_region};
pub use doc_source::DocSource;
pub use error::{Error, Result};
pub use loopfind::{Candidate, find_loops, find_loops_ranked, rank};
pub use opl_state::OplState;
pub use pack::{PackMeta, TrackEntry};
pub use song::{
    Bank, DelayKind, DroDataV1, DroDataV2, FindTarget, Instruction, OplType, Song, SongData,
    SongFileType, StreamSnapshot, slide_index_past_deletion,
};
pub use split_songs::{Segment, detect_segments};
pub use undo::{ReplaceStream, UndoController, UndoableCommand};
pub use vgm::{
    ChipKind, ChipSettings, ChipTarget, ChipUse, ExtraHeader, Gd3Tag, VgmBody, VgmCommand, VgmData,
    VgmFile, VgmHeader, VgmMeta, VgmStream,
};
pub use volume::{
    boost_for_peak, encode_volume_modifier, matched_volume, nearest_volume_modifier, peak_dbfs,
    suggest_volume_modifier, volume_modifier_factor, volume_step_down, volume_step_up,
};
