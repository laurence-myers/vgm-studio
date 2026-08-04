//! `vgmstudio split`: split a song into one file per channel.
//!
//! The splitting logic is [`vgms_synth::split_vgm_cancellable`], tested there;
//! this parses arguments, projects an OPL document to a VGM (ou-4), loads the
//! config, and writes the outputs next to the input.

use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use anyhow::{Context, Result};
use vgms_core::util::ms_to_timestr;
use vgms_synth::{
    Position, SplitData, SplitFormat, SplitOutput, VgmSplitOptions, split_vgm_cancellable,
};

use crate::{LoadedSong, load_config, read_any_song_from_path};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The DRO or VGM file to split.
    pub input: PathBuf,
    /// Split to song files -- DRO or VGM, matching the input -- instead of WAV.
    // `-d`/`--dro` is this flag's earlier name, kept working as a hidden alias.
    #[arg(short = 's', long = "song", visible_alias = "dro", short_alias = 'd')]
    pub song: bool,
    /// Render a chip through a specific core, as `slot=name` (e.g.
    /// `--core opl3=nuked`). Repeatable; unnamed slots use the configured core.
    /// This split only -- vgmstudio.ini is left untouched.
    #[arg(long = "core", value_name = "SLOT=NAME", value_parser = crate::cli::parse_core_choice)]
    pub core: Vec<(String, String)>,
    /// Volume boost multiplier for each WAV stem, applied through the same
    /// limiter a whole-song render uses. Ignored by `--song`. Without it the
    /// stems are rendered at the un-boosted level.
    #[arg(short = 'b', long = "boost")]
    pub boost: Option<f32>,
}

/// Splits `args.input`, writing one file per used channel beside it.
///
/// # Errors
/// If the song cannot be read, a channel cannot be rendered or captured, or an
/// output cannot be written.
pub fn run(args: &Args) -> Result<()> {
    let song = read_any_song_from_path(&args.input)?;
    println!("{}", song.pretty_string());

    let mut config = load_config();
    if let Some(boost) = args.boost {
        config.audio.boost = boost;
        // Reuse the config's 1..=16 range check for the CLI override.
        config
            .validate()
            .with_context(|| format!("invalid --boost {boost}"))?;
    }
    // A split is faithful unless an explicit --boost is given, like `render`.
    let boost = args.boost.unwrap_or(1.0);
    let frequency = config.audio.frequency;
    // Any `--core slot=name` picks apply to this split only, on this thread; an
    // empty map (no flag) renders exactly as the configured cores.
    let choices = crate::cli::core_choices(&args.core);
    // Report skipped channels and a live per-file render line. Both callbacks
    // share one printer through a RefCell so a skip can close an open progress
    // line before printing over it.
    let progress = RefCell::new(RenderProgress::new(frequency));
    // Every split goes through the generic splitter now (ou-4): a multichip VGM
    // directly, an OPL document over a VGM of its register stream. A DRO projects
    // to a canonical-clock VGM; a VGM (OPL or not) keeps its own header.
    let file = match song {
        LoadedSong::Opl(song) => Arc::new(
            vgms_core::convert::opl_song_to_vgm_file(&song)
                .context("projecting the OPL document for splitting")?,
        ),
        LoadedSong::Vgm(file) => Arc::new(*file),
    };
    let format = if args.song {
        SplitFormat::Song
    } else {
        SplitFormat::Wav
    };
    // A song-format split rewrites the command stream per channel (every chip a
    // write-gate covers; the rest are skipped per chip with a warning). A WAV
    // split renders, so it needs a core per chip.
    if format == SplitFormat::Wav {
        crate::warn_missing_cores(
            &file
                .header
                .chips()
                .iter()
                .map(|chip| chip.kind)
                .collect::<Vec<_>>(),
            "there is nothing to split",
        )?;
    }
    let resampling =
        vgms_synth::resample::ResampleMode::from_slug(&config.audio.resampling).unwrap_or_default();
    let options = VgmSplitOptions {
        format,
        audio: config.audio,
        resampling,
        // The CLI has no live mixer, so the pan/skip-muted opt-ins stay GUI-only
        // (neutral here); boost rides `-b/--boost`.
        panning: vgms_synth::ChipPanning::new(),
        boost,
        skip_muted: None,
        core_choices: choices,
    };
    let outputs = vgms_synth::with_render_choices(Some(options.core_choices.clone()), || {
        split_vgm_cancellable(
            &file,
            &options,
            &mut |name| {
                progress.borrow_mut().finish_line();
                println!("Skipping {name} (silent)");
            },
            &mut |base, frames| progress.borrow_mut().update(base, frames),
            &mut || true,
        )
    })?
    .unwrap_or_default();
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
            SplitData::Vgm(vgm) => vgms_core::vgm::file::write(&vgm)?,
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
