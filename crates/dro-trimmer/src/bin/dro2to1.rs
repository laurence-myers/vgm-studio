//! `dro2to1`: convert a DRO v2 file to DRO v1 (Python `dro2to1.py`).
//!
//! The conversion itself is `dro_core::convert::dro2_to_dro1`, tested there; this
//! is only argument parsing and file I/O.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use dro_core::convert::dro2_to_dro1;
use dro_core::io::{read_song, write_song};

#[derive(Parser)]
#[command(name = "dro2to1", version, about = "Convert a DRO v2 file to DRO v1.")]
struct Args {
    /// The DRO v2 file to convert.
    input: PathBuf,
    /// The output file. Defaults to `<input>_1.<ext>`.
    output: Option<PathBuf>,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    let bytes =
        std::fs::read(&args.input).with_context(|| format!("reading {}", args.input.display()))?;
    let name = file_name(&args.input);
    let song = read_song(&name, &bytes)?;
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

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("input.dro")
        .to_owned()
}
