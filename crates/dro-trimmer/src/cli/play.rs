//! `drotrim play`: play a DRO or VGM song through the speakers.
//!
//! Interactive channel soloing (number keys during playback) is not offered
//! here -- it needs raw-terminal handling and cannot be exercised without
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
    /// clipping. Overrides drotrim.ini. Emulated output only.
    #[arg(short = 'b', long = "boost")]
    pub boost: Option<f32>,
    /// Play through RetroWave OPL3 hardware instead of the emulator. Give a
    /// port (COM3, /dev/ttyACM0) to choose one, or leave it bare to auto-detect.
    #[arg(long = "retrowave", value_name = "PORT", num_args = 0..=1, default_missing_value = "")]
    pub retrowave: Option<String>,
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
    match &args.retrowave {
        Some(port) => play_on_hardware(song, port.as_str(), total_ms),
        None => play(song, &config.audio, total_ms),
    }
}

/// What a backend has to offer for the progress loop below.
trait Playable {
    fn is_finished(&self) -> bool;
    fn elapsed_ms(&self) -> u32;
}

impl Playable for NativeAudio {
    fn is_finished(&self) -> bool {
        NativeAudio::is_finished(self)
    }
    fn elapsed_ms(&self) -> u32 {
        self.position().elapsed_ms
    }
}

impl Playable for dro_retrowave::RetroWaveAudio {
    fn is_finished(&self) -> bool {
        dro_retrowave::RetroWaveAudio::is_finished(self)
    }
    fn elapsed_ms(&self) -> u32 {
        self.position().elapsed_ms
    }
}

/// Plays `song` through the default output device, showing progress until it
/// finishes or the process is interrupted.
fn play(song: dro_core::Song, audio: &AudioConfig, total_ms: u32) -> Result<()> {
    let player = NativeAudio::new(&dro_synth::AudioSource::Opl(Arc::new(song)), audio)
        .context("opening the audio device (is one available?)")?;
    player.play()?;
    show_progress(&player, total_ms);
    Ok(())
}

/// Plays `song` through a RetroWave board. `port` may be empty to auto-detect.
fn play_on_hardware(song: dro_core::Song, port: &str, total_ms: u32) -> Result<()> {
    let port = if port.is_empty() {
        let found = dro_retrowave::default_port()
            .context("finding a RetroWave device (try `drotrim retrowave-probe --list`)")?;
        println!("Using {}.", found.label);
        found.port_name
    } else {
        port.to_owned()
    };

    let device = dro_retrowave::Device::open(&port).with_context(|| format!("opening {port}"))?;
    let mut player = dro_retrowave::RetroWaveAudio::new(device, Arc::new(song));
    player.play();
    show_progress(&player, total_ms);

    if let Some(error) = player.take_error() {
        anyhow::bail!("{error}");
    }
    Ok(())
}

/// Prints elapsed time until the song ends or the process is interrupted.
fn show_progress(player: &impl Playable, total_ms: u32) {
    let mut stdout = std::io::stdout();
    while !player.is_finished() {
        write!(
            stdout,
            "\r{} / {}",
            ms_to_timestr(player.elapsed_ms()),
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
}
