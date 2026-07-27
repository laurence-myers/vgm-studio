// SPDX-License-Identifier: GPL-2.0-or-later
//! Everything behind the `drotrim` executable: its `play`, `render`, `split` and
//! `optimize` subcommands, and the platform services the GUI runs on.

use std::path::Path;

use anyhow::{Context, Result};
use dro_core::Song;
use dro_core::io::read_song;

pub mod cli;
pub mod config;
pub mod pack_zip;
pub mod services;

pub use config::load_config;
pub use pack_zip::{PackZipOutput, build_pack_zip};

/// Reads and parses the song at `path`, naming it after the file (falling back to
/// `input.dro`) so format detection follows the file's extension. Every
/// subcommand opens its one input exactly this way.
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
