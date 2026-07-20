//! `drotrim play`: play a DRO or VGM song through the speakers (Python
//! `dro_player.py`).
//!
//! The Python's interactive channel soloing (number keys during playback) is not
//! ported here -- it needs raw-terminal handling and cannot be exercised without
//! an audio device. The same soloing lives in the GUI; the CLI keeps to playing.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use dro_audio_native::NativeAudio;
use dro_core::config::AudioConfig;
use dro_core::util::ms_to_timestr;

use crate::{load_config, read_song_from_path};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The DRO or VGM file to play.
    pub input: PathBuf,
    /// Volume boost multiplier, applied through a limiter that prevents
    /// clipping. Overrides drotrim.ini.
    #[arg(short = 'b', long = "boost")]
    pub boost: Option<f32>,
}

/// Plays `args.input` until it ends or the process is interrupted.
///
/// # Errors
/// If the song cannot be read, the boost is out of range, or no audio device
/// can be opened.
pub fn run(args: &Args) -> Result<()> {
    let song = read_song_from_path(&args.input)?;
    println!("{}", song.pretty_string());

    let mut config = load_config();
    if let Some(boost) = args.boost {
        config.audio.boost = boost;
        // Reuse the config's 1..=16 range check for the CLI override.
        config
            .validate()
            .with_context(|| format!("invalid --boost {boost}"))?;
    }
    let total_ms = song.total_delay_ms();
    play(song, &config.audio, total_ms)
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
