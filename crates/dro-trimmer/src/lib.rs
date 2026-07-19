//! Shared logic for the native binaries: the `dro_player`, `dro2to1` and
//! `dro_split` CLI tools, and the platform services behind the `drotrim` GUI.

use std::path::Path;

use anyhow::{Context, Result};
use dro_core::Song;
use dro_core::io::read_song;

pub mod config;
pub mod rip_zip;
pub mod services;
pub mod split;

pub use config::load_config;
pub use rip_zip::{RipZipOutput, build_rip_zip};
pub use split::{SplitData, SplitFormat, SplitOptions, SplitOutput, split};

/// Reads and parses the song at `path`, naming it after the file (falling back to
/// `input.dro`) so format detection follows the file's extension. The three CLI
/// binaries -- `dro_player`, `dro_split`, `dro2to1` -- all open their one input
/// exactly this way.
///
/// # Errors
/// If the file cannot be read, or is not a song `dro_core` can parse.
pub fn read_song_from_path(path: &Path) -> Result<Song> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("input.dro");
    Ok(read_song(name, &bytes)?)
}
