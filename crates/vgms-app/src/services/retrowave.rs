//! Hardware playback as an `AudioService`, and the switch that chooses between
//! it and the emulator.
//!
//! Unlike the native service, seeks and mutes are never deferred: the pump
//! thread drains its queue whether or not playback is running. What *is*
//! deferred is their effect on the chip -- a seek while paused updates the
//! shadow only, and resuming plays it out (see `SerialOpl3Chip`). A real chip
//! sounds continuously, so a paused seek that reached it would be audible.

use vgms_core::config::{AudioConfig, OutputBackend};
use vgms_retrowave::{Device, RetroWaveAudio};
use vgms_synth::{AudioSource, ChipMuting, ChipPanning, LoopConfig, Muting, Panning, Position};
use vgms_ui::{AudioService, platform::HardwarePortInfo};

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
                vgms_retrowave::default_port()
                    .map_err(|error| error.to_string())?
                    .port_name
            }
        };
        Device::open(&port).map_err(|error| error.to_string())
    }
}

impl AudioService for RetroWaveAudioService {
    fn load(&mut self, source: AudioSource, config: &AudioConfig) -> Result<(), String> {
        self.unload();
        // The hardware is an OPL3. A VGM for other chips has nothing to send it,
        // and saying so beats sending silence.
        let Some(song) = source.opl().cloned() else {
            return Err(format!(
                "{} is not an OPL song, and the RetroWave output is an OPL3.",
                source.name()
            ));
        };
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

    /// Nothing to do, and not an oversight: the board is an OPL3, `load`
    /// refuses anything else, so there is never a generic chip here to mute.
    /// Written out because the trait requires it -- see
    /// [`AudioService::set_chip_muting`].
    fn set_chip_muting(&mut self, _muting: ChipMuting) {}

    /// As above: no generic engine, so no per-chip pans to place.
    fn set_chip_panning(&mut self, _panning: ChipPanning) {}

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
    vgms_retrowave::enumerate()
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
    fn load(&mut self, source: AudioSource, config: &AudioConfig) -> Result<(), String> {
        // A source the hardware cannot play goes to the emulated output whatever
        // the setting says, because the alternative is refusing to play a file
        // this app can perfectly well render. The setting is about *OPL* output;
        // it was never a claim about every chip.
        let wanted = if source.opl().is_some() {
            config.output_backend()
        } else {
            OutputBackend::Emulated
        };
        if self.active != wanted {
            self.active_mut().unload();
            if self.active == OutputBackend::RetroWave {
                // Switching away: hand the port back to the system.
                self.retrowave.release_device();
            }
            self.active = wanted;
        }
        self.active_mut().load(source, config)
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

    /// Forwarded like every other live control -- and the reason
    /// [`AudioService::set_chip_muting`] is a required method: this pair was
    /// missing here while its OPL siblings above were not, and a non-OPL file's
    /// mutes and pans died silently on the way to a working engine.
    fn set_chip_muting(&mut self, muting: ChipMuting) {
        self.active_mut().set_chip_muting(muting);
    }

    fn set_chip_panning(&mut self, panning: ChipPanning) {
        self.active_mut().set_chip_panning(panning);
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
    use std::sync::Arc;

    use vgms_core::Song;

    use super::*;
    use vgms_core::{DroDataV2, OplType};

    fn song() -> Arc<Song> {
        Arc::new(Song::dro_v2(
            "test.dro".to_owned(),
            DroDataV2::new(vec![0x00, 0x01], vec![0x20], 0xFE, 0xFF).expect("a valid fixture"),
            0,
            OplType::Opl3,
        ))
    }

    /// The live controls reach the backend that is playing -- **including the
    /// any-chip pair**, which is what this test exists for.
    ///
    /// The bug it pins shipped: `set_chip_muting` / `set_chip_panning` had
    /// `{}` defaults on `AudioService`, and this wrapper -- the only service
    /// the desktop binary builds -- forwarded their OPL siblings while
    /// inheriting the defaults for these. Every channel mute, chip mute and pan
    /// on a non-OPL file was dropped here, between a UI that sent them and an
    /// engine that applied them; both ends tested green in isolation and the
    /// feature was dead in the app. The methods are required now, so a future
    /// wrapper cannot repeat it silently -- and this checks the forwarding
    /// itself, which needs no audio device because the backend stores what it
    /// is given whether or not a stream is live.
    #[test]
    fn the_switching_service_forwards_the_any_chip_controls() {
        use vgms_core::ChipKind;

        let mut service = SwitchingAudioService::new();
        assert_eq!(
            service.active,
            OutputBackend::Emulated,
            "the default backend"
        );

        let mut muting = ChipMuting::new();
        muting.set(ChipKind::Ym2612, 0, 0x7F);
        muting.set(ChipKind::Sn76489, 0, 0b0001);
        service.set_chip_muting(muting);
        let stored = service.native.last_chip_muting();
        assert_eq!(
            stored.mask_for(ChipKind::Ym2612, 0),
            0x7F,
            "a whole-chip mask must reach the backend"
        );
        assert_eq!(
            stored.mask_for(ChipKind::Sn76489, 0),
            0b0001,
            "and so must a single channel's"
        );

        let mut panning = ChipPanning::new();
        panning.set(ChipKind::Sn76489, 0, vec![-0x100; 4]);
        service.set_chip_panning(panning);
        assert_eq!(
            service
                .native
                .last_chip_panning()
                .pans_for(ChipKind::Sn76489, 0),
            Some(&[-0x100i16, -0x100, -0x100, -0x100][..]),
            "pans must reach the backend too"
        );
    }

    /// Loading hardware output with no device present must fail loudly rather
    /// than silently falling back to the speakers.
    #[test]
    fn a_missing_device_reports_an_error_rather_than_switching_back() {
        let mut service = SwitchingAudioService::new();
        let config = AudioConfig {
            // Hardware output, spelled as the OPL slot's core choice.
            cores: [("opl3".to_owned(), "retrowave".to_owned())].into(),
            retrowave_port: Some("NO_SUCH_PORT".to_owned()),
            ..AudioConfig::default()
        };
        let error = service
            .load(AudioSource::Opl(song()), &config)
            .expect_err("opening a nonexistent port must fail");
        assert!(!error.is_empty());
        assert_eq!(service.active, OutputBackend::RetroWave);
    }

    /// A VGM for chips the hardware cannot play routes to the emulated output
    /// whatever the setting says. Refusing to play a file this app can perfectly
    /// well render, because of a setting about *OPL* output, would be the wrong
    /// reading of what that setting means.
    #[test]
    fn a_non_opl_source_goes_to_the_emulator_whatever_the_setting_says() {
        fn mega_drive_vgm() -> Arc<vgms_core::VgmFile> {
            let mut bytes = vec![0u8; 0x100];
            bytes[..4].copy_from_slice(b"Vgm ");
            bytes[0x08..0x0C].copy_from_slice(&0x161u32.to_le_bytes());
            bytes[0x34..0x38].copy_from_slice(&(0x100u32 - 0x34).to_le_bytes());
            bytes[vgms_core::ChipKind::Ym2612.clock_offset()..][..4]
                .copy_from_slice(&7_670_454u32.to_le_bytes());
            bytes.extend_from_slice(&[0x52, 0x28, 0xF0, 0x66]);
            let eof = bytes.len();
            bytes[0x04..0x08].copy_from_slice(&((eof - 4) as u32).to_le_bytes());
            Arc::new(vgms_core::vgm::file::read("md.vgm", &bytes).expect("a walkable VGM"))
        }

        let mut service = SwitchingAudioService::new();
        let config = AudioConfig {
            // Hardware output, spelled as the OPL slot's core choice.
            cores: [("opl3".to_owned(), "retrowave".to_owned())].into(),
            retrowave_port: Some("NO_SUCH_PORT".to_owned()),
            ..AudioConfig::default()
        };
        // It never reaches the hardware, so the nonexistent port is never opened
        // -- the load either succeeds on the emulator or fails for want of an
        // audio device, which is a machine fact, not a routing one.
        let _ = service.load(AudioSource::Vgm(mega_drive_vgm()), &config);
        assert_eq!(
            service.active,
            OutputBackend::Emulated,
            "a non-OPL source never goes to an OPL3"
        );
    }

    /// And the hardware service itself says why, rather than sending silence.
    #[test]
    fn the_hardware_service_refuses_a_source_it_cannot_play() {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        bytes[0x08..0x0C].copy_from_slice(&0x161u32.to_le_bytes());
        bytes[0x34..0x38].copy_from_slice(&(0x100u32 - 0x34).to_le_bytes());
        bytes[vgms_core::ChipKind::Ym2612.clock_offset()..][..4]
            .copy_from_slice(&7_670_454u32.to_le_bytes());
        bytes.extend_from_slice(&[0x52, 0x28, 0xF0, 0x66]);
        let eof = bytes.len();
        bytes[0x04..0x08].copy_from_slice(&((eof - 4) as u32).to_le_bytes());
        let file = Arc::new(vgms_core::vgm::file::read("md.vgm", &bytes).expect("walks"));

        let mut service = RetroWaveAudioService::new();
        let error = service
            .load(AudioSource::Vgm(file), &AudioConfig::default())
            .expect_err("an OPL3 cannot play a YM2612");
        assert!(error.contains("md.vgm"), "{error}");
        assert!(error.contains("OPL"), "{error}");
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
