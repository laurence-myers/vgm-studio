//! `drotrim split`: split a song into one file per channel.
//!
//! The splitting logic is [`dro_synth::split`], tested there; this parses
//! arguments, loads the config, and writes the outputs next to the input.

use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use anyhow::{Context, Result};
use dro_core::io::write_song;
use dro_core::util::ms_to_timestr;
use dro_synth::{
    Position, SplitData, SplitFormat, SplitOptions, SplitOutput, VgmSplitOptions, split,
    split_vgm_cancellable,
};

use crate::{LoadedSong, load_config, read_any_song_from_path};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The DRO or VGM file to split.
    pub input: PathBuf,
    /// Split to song files -- DRO or VGM, matching the input -- instead of WAV.
    // `-d`/`--dro` was this flag's earlier name, and the output was always a
    // DRO. Kept working, but out of the help.
    #[arg(short = 's', long = "song", visible_alias = "dro", short_alias = 'd')]
    pub song: bool,
    /// Render each drum on the percussion channel to its own file.
    #[arg(short = 'i', long = "isolate-percussion")]
    pub isolate_percussion: bool,
}

/// Splits `args.input`, writing one file per used channel beside it.
///
/// # Errors
/// If the song cannot be read, a channel cannot be rendered or captured, or an
/// output cannot be written.
pub fn run(args: &Args) -> Result<()> {
    let song = read_any_song_from_path(&args.input)?;
    println!("{}", song.pretty_string());

    let config = load_config();
    let frequency = config.audio.frequency;
    // Report skipped channels and a live per-file render line. Both callbacks
    // share one printer through a RefCell so a skip can close an open progress
    // line before printing over it.
    let progress = RefCell::new(RenderProgress::new(frequency));
    let outputs = match song {
        LoadedSong::Opl(song) => {
            let options = SplitOptions {
                format: if args.song {
                    SplitFormat::Song
                } else {
                    SplitFormat::Wav
                },
                isolate_percussion: args.isolate_percussion,
                audio: config.audio,
            };
            split(
                &song,
                &options,
                &mut |channel| {
                    progress.borrow_mut().finish_line();
                    let bank = channel >> 8;
                    let channel_num = (channel & 0xFF) - 0xAF;
                    println!("Skipping bank {bank}, channel {channel_num:02} (unused)");
                },
                &mut |base, frames| progress.borrow_mut().update(base, frames),
            )?
        }
        LoadedSong::Vgm(file) => {
            // A per-channel song output needs per-chip write gating that only
            // the OPL path has; a generic VGM splits to WAV.
            if args.song {
                anyhow::bail!(
                    "--song split is OPL-only; {} splits to WAV",
                    args.input.display()
                );
            }
            crate::warn_missing_cores(
                &file
                    .header
                    .chips()
                    .iter()
                    .map(|chip| chip.kind)
                    .collect::<Vec<_>>(),
                "there is nothing to split",
            )?;
            let resampling = dro_synth::resample::ResampleMode::from_slug(&config.audio.resampling)
                .unwrap_or_default();
            let options = VgmSplitOptions {
                audio: config.audio,
                resampling,
            };
            split_vgm_cancellable(
                &Arc::new(*file),
                &options,
                &mut |name| {
                    progress.borrow_mut().finish_line();
                    println!("Skipping {name} (silent)");
                },
                &mut |base, frames| progress.borrow_mut().update(base, frames),
                &mut || true,
            )?
            .unwrap_or_default()
        }
    };
    progress.borrow_mut().finish_line();

    write_outputs(&args.input, outputs)
}

/// Writes each split output beside `input`, reporting each and the total.
///
/// # Errors
/// If a captured song cannot be serialised, or a file cannot be written.
fn write_outputs(input: &Path, outputs: Vec<SplitOutput>) -> Result<()> {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let count = outputs.len();
    // Consume the outputs so a WAV's bytes move straight into the write.
    for output in outputs {
        let path = dir.join(&output.name);
        let bytes = match output.data {
            SplitData::Wav(bytes) => bytes,
            SplitData::Song(song) => write_song(&song)?,
        };
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        println!("Wrote {}", path.display());
    }
    println!("Done -- {count} file(s).");
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
