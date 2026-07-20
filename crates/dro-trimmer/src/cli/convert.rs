//! `drotrim convert`: convert a DRO v2 file to DRO v1 (Python `dro2to1.py`).
//!
//! The conversion itself is `dro_core::convert::dro2_to_dro1`, tested there; this
//! is only argument parsing and file I/O.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use dro_core::convert::dro2_to_dro1;
use dro_core::io::write_song;

use crate::read_song_from_path;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The DRO v2 file to convert.
    pub input: PathBuf,
    /// The output file. Defaults to `<input>_1.<ext>`.
    pub output: Option<PathBuf>,
}

/// Converts `args.input` to DRO v1.
///
/// # Errors
/// If the song cannot be read, is not a DRO v2, the output already exists, or
/// the write fails.
pub fn run(args: Args) -> Result<()> {
    let song = read_song_from_path(&args.input)?;
    let v1 = dro2_to_dro1(&song)?;

    let output = args.output.unwrap_or_else(|| default_output(&args.input));
    if output.exists() {
        bail!(
            "Output file already exists; delete it or choose another name: {}",
            output.display()
        );
    }
    std::fs::write(&output, write_song(&v1)?)
        .with_context(|| format!("writing {}", output.display()))?;

    println!("Converted {} -> {}", args.input.display(), output.display());
    Ok(())
}

/// `<input>_1.<ext>`, matching the Python default.
fn default_output(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let name = match input.extension().and_then(|s| s.to_str()) {
        Some(ext) => format!("{stem}_1.{ext}"),
        None => format!("{stem}_1"),
    };
    input.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_output_suffixes_the_stem() {
        assert_eq!(
            default_output(Path::new("song.dro")),
            PathBuf::from("song_1.dro")
        );
        assert_eq!(
            default_output(Path::new("music/song.dro")),
            PathBuf::from("music/song_1.dro")
        );
        assert_eq!(
            default_output(Path::new("capture")),
            PathBuf::from("capture_1")
        );
    }
}
