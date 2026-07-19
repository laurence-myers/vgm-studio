//! `dro_split`: split a DRO song into one file per channel (Python `dro_split.py`).
//!
//! The splitting logic is `dro_trimmer::split`, tested there; this parses
//! arguments, loads the config, and writes the outputs next to the input.

use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use dro_core::io::write_song;
use dro_core::util::ms_to_timestr;
use dro_synth::Position;
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

    // Report skipped channels and a live per-file render line. Both callbacks
    // share one printer through a RefCell so a skip can close an open progress
    // line before printing over it.
    let progress = RefCell::new(RenderProgress::new(options.audio.frequency));
    let outputs = split(
        &song,
        &options,
        &mut |channel| {
            progress.borrow_mut().finish_line();
            let bank = channel >> 8;
            let channel_num = (channel & 0xFF) - 0xAF;
            println!("Skipping bank {bank}, channel {channel_num:02} (unused)");
        },
        &mut |base, frames| progress.borrow_mut().update(base, frames),
    )?;
    progress.borrow_mut().finish_line();

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

/// Prints a live, in-place `MM:SS` line for the file currently rendering. Renders
/// run faster than realtime, so this only visibly ticks on large inputs; a short
/// file just flashes its final time. `finish_line` closes the open line before any
/// other output (a skip notice, or the write phase) prints over it.
struct RenderProgress {
    frequency: u32,
    /// The base name of the file whose line is open, if any.
    current: Option<String>,
    /// The last whole second printed, so the line only redraws once a second.
    last_second: Option<u32>,
}

impl RenderProgress {
    fn new(frequency: u32) -> Self {
        Self {
            frequency,
            current: None,
            last_second: None,
        }
    }

    fn update(&mut self, base: &str, frames: u64) {
        if self.current.as_deref() != Some(base) {
            self.finish_line(); // a new file: close the previous line
            self.current = Some(base.to_owned());
        }
        let ms = Position::ms_from_frames(frames, self.frequency);
        if self.last_second != Some(ms / 1000) {
            self.last_second = Some(ms / 1000);
            print!("\rRendering {base}  {}", ms_to_timestr(ms));
            let _ = std::io::stdout().flush();
        }
    }

    /// Ends the open in-place line with a newline, if one is open. Idempotent.
    fn finish_line(&mut self) {
        if self.current.take().is_some() {
            println!();
            self.last_second = None;
        }
    }
}
