//! `vgmstudio render`: render a DRO or VGM song to a WAV file.
//!
//! A tight offline loop with a once-a-second progress line.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use anyhow::{Context, Result};
use vgms_core::util::ms_to_timestr;

use crate::{LoadedSong, load_config, read_any_song_from_path};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The DRO or VGM file to render.
    pub input: PathBuf,
    /// Volume boost multiplier, applied through a limiter that prevents
    /// clipping. Without it the WAV is rendered at the un-boosted level.
    #[arg(short = 'b', long = "boost")]
    pub boost: Option<f32>,
    /// Render a chip through a specific core, as `slot=name` (e.g.
    /// `--core opl3=nuked`). Repeatable; unnamed slots use the configured core.
    /// This render only -- vgmstudio.ini is left untouched.
    #[arg(long = "core", value_name = "SLOT=NAME", value_parser = crate::cli::parse_core_choice)]
    pub core: Vec<(String, String)>,
}

/// Renders `args.input` to `<input>.wav`.
///
/// # Errors
/// If the song cannot be read, the boost is out of range, or the WAV cannot be
/// written.
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
    let total_ms = song.total_ms();

    // A render is faithful to the source unless an explicit --boost is given;
    // the vgmstudio.ini / GUI boost never affects it, so default to 1.0 here
    // rather than reading `config.audio.boost`.
    let boost = args.boost.unwrap_or(1.0);
    let freq = config.audio.frequency;
    // A live progress line for a long render, refreshed once a second.
    let mut last_sec: Option<u32> = None;
    let mut on_progress = |frames: u64| {
        let ms = vgms_synth::Position::ms_from_frames(frames, freq);
        if last_sec != Some(ms / 1000) {
            last_sec = Some(ms / 1000);
            print!(
                "\rRendering {} / {}",
                ms_to_timestr(ms),
                ms_to_timestr(total_ms)
            );
            let _ = std::io::stdout().flush();
        }
    };
    // Any `--core slot=name` picks are active for this render only, on this
    // thread; an empty map (no flag) renders exactly as the configured cores.
    let choices = crate::cli::core_choices(&args.core);
    let wav = vgms_synth::with_render_choices(Some(choices), || -> Result<Vec<u8>> {
        Ok(match song {
            LoadedSong::Dro(song) => vgms_synth::render_wav_boosted_with_progress(
                &song,
                freq,
                config.audio.bit_depth,
                boost,
                &mut on_progress,
            )?,
            LoadedSong::Vgm(file) => {
                let chips: Vec<_> = file.header.chips().iter().map(|chip| chip.kind).collect();
                crate::warn_missing_cores(&chips, "the render would be silence")?;
                // The render honours the config's resampling choice, exactly as
                // playback does -- an export sounds like what the user hears.
                let resampling =
                    vgms_synth::resample::ResampleMode::from_slug(&config.audio.resampling)
                        .unwrap_or_default();
                vgms_synth::render_vgm_wav_cancellable(
                    Arc::new(*file),
                    freq,
                    config.audio.bit_depth,
                    boost,
                    resampling,
                    &mut on_progress,
                    &mut || true,
                )?
                .expect("a render that is never cancelled always completes")
            }
        })
    })?;
    println!(); // end the progress line

    let output = append_extension(&args.input, "wav");
    std::fs::write(&output, wav).with_context(|| format!("writing {}", output.display()))?;
    if boost == 1.0 {
        println!(
            "Rendered {} ({})",
            output.display(),
            ms_to_timestr(total_ms)
        );
    } else {
        println!(
            "Rendered {} ({}) at boost {boost}x",
            output.display(),
            ms_to_timestr(total_ms)
        );
    }
    Ok(())
}

/// Appends `.ext` to the whole path:
/// `song.dro` becomes `song.dro.wav`, not `song.wav`.
fn append_extension(path: &Path, extension: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(OsString::from(format!(".{extension}")));
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_is_appended_not_replaced() {
        assert_eq!(
            append_extension(Path::new("song.dro"), "wav"),
            PathBuf::from("song.dro.wav")
        );
        assert_eq!(
            append_extension(Path::new("capture"), "wav"),
            PathBuf::from("capture.wav")
        );
    }
}
