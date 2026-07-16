//! `dro_player`: play a DRO/VGM song, or render it to a WAV (Python
//! `dro_player.py` `main`).
//!
//! Rendering is a tight offline loop; playback drives the native audio engine and
//! shows a live progress line until the song ends or Ctrl+C.
//!
//! The Python's interactive channel soloing (number keys during playback) is not
//! ported here -- it needs raw-terminal handling and cannot be exercised without
//! an audio device. The same soloing lives in the GUI (Step 6); the CLI keeps to
//! play and render.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use dro_audio_native::NativeAudio;
use dro_core::config::AudioConfig;
use dro_core::io::read_song;
use dro_core::total_delay_with_write_delay_ms;
use dro_core::util::ms_to_timestr;
use dro_trimmer::load_config;

#[derive(Parser)]
#[command(
    name = "dro_player",
    about = "Play a DRO or VGM song, or render it to a WAV file."
)]
struct Args {
    /// The DRO or VGM file to play.
    input: PathBuf,
    /// Render to a WAV file instead of playing through the speakers.
    #[arg(short = 'r', long = "render")]
    render: bool,
    /// Volume boost multiplier, applied through a limiter that prevents
    /// clipping. Overrides drotrim.ini for playback; with --render it boosts the
    /// WAV, which is otherwise rendered at the un-boosted level.
    #[arg(short = 'b', long = "boost")]
    boost: Option<f32>,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    let bytes =
        std::fs::read(&args.input).with_context(|| format!("reading {}", args.input.display()))?;
    let name = args
        .input
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("input.dro");
    let song = read_song(name, &bytes)?;
    println!("{}", song.pretty_string());

    let mut config = load_config();
    if let Some(boost) = args.boost {
        config.audio.boost = boost;
        // Reuse the config's 1..=16 range check for the CLI override.
        config
            .validate()
            .with_context(|| format!("invalid --boost {boost}"))?;
    }
    let total_ms = total_delay_with_write_delay_ms(&song, config.audio.chip_write_delay);

    if args.render {
        // A render is faithful to the source unless an explicit --boost is given;
        // the drotrim.ini / GUI boost never affects it, so default to 1.0 here
        // rather than reading `config.audio.boost`.
        let boost = args.boost.unwrap_or(1.0);
        let wav = dro_synth::render_wav_boosted(
            &song,
            config.audio.frequency,
            config.audio.bit_depth,
            config.audio.chip_write_delay,
            boost,
        )?;
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
    } else {
        play(song, &config.audio, total_ms)?;
    }
    Ok(())
}

/// Plays `song` through the default output device, showing progress until it
/// finishes or the process is interrupted.
fn play(song: dro_core::Song, audio: &AudioConfig, total_ms: u32) -> Result<()> {
    let player = NativeAudio::new(Arc::new(song), audio)
        .context("opening the audio device (is one available?)")?;
    player.play()?;

    let mut stdout = std::io::stdout();
    while !player.is_finished() {
        let elapsed = player.position().elapsed_ms;
        write!(
            stdout,
            "\r{} / {}",
            ms_to_timestr(elapsed),
            ms_to_timestr(total_ms)
        )
        .ok();
        stdout.flush().ok();
        std::thread::sleep(Duration::from_millis(50));
    }
    println!(
        "\r{} / {}",
        ms_to_timestr(total_ms),
        ms_to_timestr(total_ms)
    );
    Ok(())
}

/// Appends `.ext` to the whole path, as the Python `"{}.wav".format(name)` did:
/// `song.dro` becomes `song.dro.wav`, not `song.wav`.
fn append_extension(path: &Path, extension: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(OsString::from(format!(".{extension}")));
    PathBuf::from(name)
}
