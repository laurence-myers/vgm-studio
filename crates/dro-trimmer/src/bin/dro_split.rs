//! `dro_split`: split a DRO song into one file per channel (Python `dro_split.py`).
//!
//! The splitting logic is `dro_trimmer::split`, tested there; this parses
//! arguments, loads the config, and writes the outputs next to the input.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use dro_core::io::write_song;
use dro_trimmer::{SplitData, SplitFormat, SplitOptions, load_config, read_song_from_path, split};

#[derive(Parser)]
#[command(
    name = "dro_split",
    version,
    about = "Split a DRO song into one WAV (or DRO) file per channel used."
)]
struct Args {
    /// The DRO file to split.
    input: PathBuf,
    /// Split to DRO files instead of WAV.
    #[arg(short = 'd', long = "dro")]
    dro: bool,
    /// Render each drum on the percussion channel to its own file.
    #[arg(short = 'i', long = "isolate-percussion")]
    isolate_percussion: bool,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    let song = read_song_from_path(&args.input)?;
    println!("{}", song.pretty_string());

    let options = SplitOptions {
        format: if args.dro {
            SplitFormat::Dro
        } else {
            SplitFormat::Wav
        },
        isolate_percussion: args.isolate_percussion,
        audio: load_config().audio,
    };

    let outputs = split(&song, &options)?;
    let dir = args.input.parent().unwrap_or_else(|| Path::new("."));
    let count = outputs.len();
    // Consume the outputs so a WAV's bytes move straight into the write.
    for output in outputs {
        let path = dir.join(&output.name);
        let bytes = match output.data {
            SplitData::Wav(bytes) => bytes,
            SplitData::Dro(dro) => write_song(&dro)?,
        };
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        println!("Wrote {}", path.display());
    }
    println!("Done -- {} file(s).", count);
    Ok(())
}
