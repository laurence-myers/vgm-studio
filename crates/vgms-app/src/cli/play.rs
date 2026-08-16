//! `vgmstudio play`: play a DRO or VGM song through the speakers.
//!
//! Interactive channel soloing (number keys during playback) is not offered
//! here -- it needs raw-terminal handling and cannot be exercised without
//! an audio device. The same soloing lives in the GUI; the CLI keeps to playing.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use vgms_audio_native::NativeAudio;
use vgms_core::config::AudioConfig;
use vgms_core::util::ms_to_timestr;

use crate::{LoadedSong, load_config, read_any_song_from_path};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The DRO or VGM file to play.
    pub input: PathBuf,
    /// Volume boost multiplier, applied through a limiter that prevents
    /// clipping. Overrides vgmstudio.ini. Emulated output only.
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
    match &args.retrowave {
        Some(port) => {
            // The board is an OPL3; only an OPL stream can drive it, through the
            // same `VgmEngine` the emulator uses. A DRO projects to an OPL VGM
            // (the same projection the hardware service makes); an OPL VGM plays
            // from its own bytes. `opl` is `Some` for a DRO, whose OPL panel
            // vocabulary the pump translates. Same refusal the GUI makes for a
            // VGM of other chips.
            let (file, opl) = match song {
                LoadedSong::Dro(song) => (
                    Arc::new(
                        vgms_core::convert::opl_song_to_vgm_file(&song)
                            .context("projecting the DRO for the OPL3")?,
                    ),
                    Some(song.playback_opl_type()),
                ),
                LoadedSong::Vgm(file) if file.is_opl() => (Arc::new(*file), None),
                LoadedSong::Vgm(_) => anyhow::bail!(
                    "{} is not an OPL song, and the RetroWave output is an OPL3.",
                    args.input.display()
                ),
            };
            play_on_hardware(file, opl, port.as_str(), total_ms)
        }
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

impl Playable for vgms_retrowave::RetroWaveAudio {
    fn is_finished(&self) -> bool {
        vgms_retrowave::RetroWaveAudio::is_finished(self)
    }
    fn elapsed_ms(&self) -> u32 {
        self.position().elapsed_ms
    }
}

/// Plays `song` through the default output device, showing progress until it
/// finishes or the process is interrupted.
fn play(song: LoadedSong, audio: &AudioConfig, total_ms: u32) -> Result<()> {
    crate::warn_missing_cores(&song.chips(), "playing it would be silence")?;
    let player = NativeAudio::new(&song.audio_source(), audio)
        .context("opening the audio device (is one available?)")?;
    player.play()?;
    show_progress(&player, total_ms);
    Ok(())
}

/// Plays an OPL `file` through a RetroWave board. `port` may be empty to
/// auto-detect; `opl` is the OPL type when the source was a DRO (whose panel
/// vocabulary the pump translates), `None` for an OPL VGM.
fn play_on_hardware(
    file: Arc<vgms_core::VgmFile>,
    opl: Option<vgms_core::OplType>,
    port: &str,
    total_ms: u32,
) -> Result<()> {
    let port = if port.is_empty() {
        let found = vgms_retrowave::default_port()
            .context("finding a RetroWave device (try `vgmstudio retrowave-probe --list`)")?;
        println!("Using {}.", found.label);
        found.port_name
    } else {
        port.to_owned()
    };

    let device = vgms_retrowave::Device::open(&port).with_context(|| format!("opening {port}"))?;
    let mut player = vgms_retrowave::RetroWaveAudio::new(device, file, opl);
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
