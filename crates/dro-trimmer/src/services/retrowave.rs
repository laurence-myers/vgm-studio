//! Hardware playback as an `AudioService`, and the switch that chooses between
//! it and the emulator.
//!
//! Unlike the native service, seeks and mutes are never deferred: the pump
//! thread drains its queue whether or not playback is running. What *is*
//! deferred is their effect on the chip -- a seek while paused updates the
//! shadow only, and resuming plays it out (see `SerialOpl3Chip`). A real chip
//! sounds continuously, so a paused seek that reached it would be audible.

use std::sync::Arc;

use dro_core::{
    Song,
    config::{AudioConfig, OutputBackend},
};
use dro_retrowave::{Device, RetroWaveAudio};
use dro_synth::{LoopConfig, Muting, Panning, Position};
use dro_ui::{AudioService, platform::HardwarePortInfo};

use super::NativeAudioService;

/// Playback through a RetroWave board.
#[derive(Debug, Default)]
pub struct RetroWaveAudioService {
    audio: Option<RetroWaveAudio>,
    /// The open port, parked here between songs.
    ///
    /// Every editor edit invalidates the loaded song, so a reopen-per-load would
    /// pay the chip's reset settle on each edit-then-play -- and a Windows CDC
    /// port can refuse to reopen immediately after closing.
    device: Option<Device>,
    playing: bool,
    muting: Muting,
    panning: Panning,
    loop_config: Option<LoopConfig>,
}

impl RetroWaveAudioService {
    #[must_use]
    pub fn new() -> Self {
        Self {
            muting: Muting::all(),
            panning: Panning::Original,
            ..Self::default()
        }
    }

    /// Closes the port. The next load opens a fresh one.
    pub fn release_device(&mut self) {
        self.unload();
        self.device = None;
    }

    /// The open port, or a newly opened one for `config`.
    fn acquire_device(&mut self, config: &AudioConfig) -> Result<Device, String> {
        if let Some(device) = self.device.take() {
            return Ok(device);
        }
        let port = match &config.retrowave_port {
            Some(port) => port.clone(),
            None => {
                dro_retrowave::default_port()
                    .map_err(|error| error.to_string())?
                    .port_name
            }
        };
        Device::open(&port).map_err(|error| error.to_string())
    }
}

impl AudioService for RetroWaveAudioService {
    fn load(&mut self, song: Arc<Song>, config: &AudioConfig) -> Result<(), String> {
        self.unload();
        let device = self.acquire_device(config)?;
        self.audio = Some(RetroWaveAudio::new(device, song));
        Ok(())
    }

    fn unload(&mut self) {
        if let Some(audio) = self.audio.take() {
            // Keep the port for the next song rather than closing it.
            self.device = audio.into_device();
        }
        self.playing = false;
    }

    fn play(&mut self) -> Result<(), String> {
        let audio = self
            .audio
            .as_mut()
            .ok_or("No song is loaded into the RetroWave device.")?;
        audio.set_muting(self.muting);
        audio.set_panning(self.panning);
        audio.set_loop(self.loop_config);
        audio.play();
        self.playing = true;
        Ok(())
    }

    fn pause(&mut self) {
        if let Some(audio) = &mut self.audio {
            audio.pause();
        }
        self.playing = false;
    }

    fn seek_ms(&mut self, ms: u32) {
        if let Some(audio) = &mut self.audio {
            audio.seek_ms(ms);
        }
    }

    fn seek_pos(&mut self, pos: usize) {
        if let Some(audio) = &mut self.audio {
            audio.seek_pos(pos);
        }
    }

    fn rewind(&mut self) {
        if let Some(audio) = &mut self.audio {
            audio.rewind();
        }
    }

    fn set_muting(&mut self, muting: Muting) {
        self.muting = muting;
        if let Some(audio) = &mut self.audio {
            audio.set_muting(muting);
        }
    }

    fn set_panning(&mut self, panning: Panning) {
        self.panning = panning;
        if let Some(audio) = &mut self.audio {
            audio.set_panning(panning);
        }
    }

    /// Nothing to do: the boost is a property of the rendered signal, and this
    /// backend renders none.
    fn set_boost(&mut self, _boost: f32) {}

    fn set_loop(&mut self, config: Option<LoopConfig>) {
        self.loop_config = config;
        if let Some(audio) = &mut self.audio {
            audio.set_loop(config);
        }
    }

    fn is_playing(&self) -> bool {
        self.playing
            && self
                .audio
                .as_ref()
                .is_some_and(|audio| !audio.is_finished())
    }

    fn is_finished(&self) -> bool {
        self.audio.as_ref().is_some_and(RetroWaveAudio::is_finished)
    }

    fn position(&self) -> Option<Position> {
        self.audio.as_ref().map(RetroWaveAudio::position)
    }

    /// No samples pass through this program, so there is nothing to measure.
    /// The meter decays to silence on `None`, which is the honest reading.
    fn take_peaks(&mut self) -> Option<[f32; 2]> {
        None
    }

    /// Always the OPL's native rate: no sound card is involved to impose one,
    /// and the app derives the position readout and loop frames from this.
    fn output_rate(&self) -> Option<u32> {
        self.audio.as_ref().map(RetroWaveAudio::sample_rate)
    }

    fn min_engaged_boost(&self) -> Option<f32> {
        None
    }

    fn list_hardware_ports(&self) -> Vec<HardwarePortInfo> {
        list_ports()
    }

    fn last_error(&mut self) -> Option<String> {
        self.audio.as_mut()?.take_error()
    }
}

/// Every serial port, whether or not a device is open.
fn list_ports() -> Vec<HardwarePortInfo> {
    dro_retrowave::enumerate()
        .unwrap_or_else(|error| {
            log::warn!("could not list serial ports: {error}");
            Vec::new()
        })
        .into_iter()
        .map(|port| HardwarePortInfo {
            port_name: port.port_name,
            label: port.label,
            recognised: port.looks_like_retrowave,
        })
        .collect()
}

/// Plays through whichever backend the config asks for.
///
/// The app holds one `AudioService`; this is it. Loading a song picks the
/// backend and releases the other's hardware, so only one of the sound card and
/// the serial port is ever held.
#[derive(Debug, Default)]
pub struct SwitchingAudioService {
    native: NativeAudioService,
    retrowave: RetroWaveAudioService,
    active: OutputBackend,
}

impl SwitchingAudioService {
    #[must_use]
    pub fn new() -> Self {
        Self {
            native: NativeAudioService::new(),
            retrowave: RetroWaveAudioService::new(),
            active: OutputBackend::Emulated,
        }
    }

    fn active(&self) -> &dyn AudioService {
        match self.active {
            OutputBackend::Emulated => &self.native,
            OutputBackend::RetroWave => &self.retrowave,
        }
    }

    fn active_mut(&mut self) -> &mut dyn AudioService {
        match self.active {
            OutputBackend::Emulated => &mut self.native,
            OutputBackend::RetroWave => &mut self.retrowave,
        }
    }
}

impl AudioService for SwitchingAudioService {
    /// Activates the configured backend, releasing the other's device.
    fn load(&mut self, song: Arc<Song>, config: &AudioConfig) -> Result<(), String> {
        if self.active != config.output_backend {
            self.active_mut().unload();
            if self.active == OutputBackend::RetroWave {
                // Switching away: hand the port back to the system.
                self.retrowave.release_device();
            }
            self.active = config.output_backend;
        }
        self.active_mut().load(song, config)
    }

    fn unload(&mut self) {
        self.active_mut().unload();
    }

    fn play(&mut self) -> Result<(), String> {
        self.active_mut().play()
    }

    fn pause(&mut self) {
        self.active_mut().pause();
    }

    fn seek_ms(&mut self, ms: u32) {
        self.active_mut().seek_ms(ms);
    }

    fn seek_pos(&mut self, pos: usize) {
        self.active_mut().seek_pos(pos);
    }

    fn rewind(&mut self) {
        self.active_mut().rewind();
    }

    fn set_muting(&mut self, muting: Muting) {
        self.active_mut().set_muting(muting);
    }

    fn set_panning(&mut self, panning: Panning) {
        self.active_mut().set_panning(panning);
    }

    fn set_boost(&mut self, boost: f32) {
        self.active_mut().set_boost(boost);
    }

    fn set_loop(&mut self, config: Option<LoopConfig>) {
        self.active_mut().set_loop(config);
    }

    fn is_playing(&self) -> bool {
        self.active().is_playing()
    }

    fn is_finished(&self) -> bool {
        self.active().is_finished()
    }

    fn position(&self) -> Option<Position> {
        self.active().position()
    }

    fn take_peaks(&mut self) -> Option<[f32; 2]> {
        self.active_mut().take_peaks()
    }

    fn output_rate(&self) -> Option<u32> {
        self.active().output_rate()
    }

    fn min_engaged_boost(&self) -> Option<f32> {
        self.active().min_engaged_boost()
    }

    fn take_limited(&mut self) -> bool {
        self.active_mut().take_limited()
    }

    /// Answered directly rather than through the inner service, so the settings
    /// dialog can list ports while the emulator is the one playing -- which is
    /// exactly the state a first-time user is in.
    fn list_hardware_ports(&self) -> Vec<HardwarePortInfo> {
        list_ports()
    }

    fn last_error(&mut self) -> Option<String> {
        self.active_mut().last_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dro_core::{DroDataV2, OplType};

    fn song() -> Arc<Song> {
        Arc::new(Song::dro_v2(
            "test.dro".to_owned(),
            DroDataV2::new(vec![0x00, 0x01], vec![0x20], 0xFE, 0xFF).expect("a valid fixture"),
            0,
            OplType::Opl3,
        ))
    }

    /// Loading hardware output with no device present must fail loudly rather
    /// than silently falling back to the speakers.
    #[test]
    fn a_missing_device_reports_an_error_rather_than_switching_back() {
        let mut service = SwitchingAudioService::new();
        let config = AudioConfig {
            output_backend: OutputBackend::RetroWave,
            retrowave_port: Some("NO_SUCH_PORT".to_owned()),
            ..AudioConfig::default()
        };
        let error = service
            .load(song(), &config)
            .expect_err("opening a nonexistent port must fail");
        assert!(!error.is_empty());
        assert_eq!(service.active, OutputBackend::RetroWave);
    }

    #[test]
    fn hardware_output_reports_the_chips_own_rate_and_no_peaks() {
        let mut service = RetroWaveAudioService::new();
        assert_eq!(service.output_rate(), None, "nothing loaded yet");
        assert_eq!(service.take_peaks(), None);
        assert_eq!(service.min_engaged_boost(), None);
    }

    /// The port list must not depend on a device being open, or the settings
    /// dialog would be empty for anyone setting hardware output up.
    #[test]
    fn ports_can_be_listed_without_opening_anything() {
        let service = SwitchingAudioService::new();
        // Whatever this machine has, the call must succeed and stay on the
        // emulator.
        let _ = service.list_hardware_ports();
        assert_eq!(service.active, OutputBackend::Emulated);
    }
}
