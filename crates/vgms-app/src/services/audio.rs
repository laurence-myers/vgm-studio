//! The native `AudioService`, over `vgms-audio-native`'s cpal stream.
//!
//! `NativeAudio` is effectively `!Send` (a cpal stream), so this whole service
//! lives on the UI thread -- which is exactly where eframe calls it. Each
//! loaded song gets a fresh stream: the engine inside the audio callback owns
//! an immutable snapshot, so there is nothing to swap in place.
//!
//! Seeks and mute changes are *deferred while paused*: the stream's 64-slot
//! command queue is only drained by the audio callback, which a paused cpal
//! stream never runs, so pushing per-click would eventually overflow it and
//! silently drop a later Play's seek. Only the latest seek matters anyway --
//! seeking replays chip state from the start -- so the pending seek and the
//! muting are flushed as two commands when playback (re)starts.

use vgms_audio_native::NativeAudio;
use vgms_core::config::AudioConfig;
use vgms_synth::{
    AudioSource, ChipMuting, ChipPanning, ChipTrims, LoopConfig, Muting, Panning, Position,
};
use vgms_ui::AudioService;

#[derive(Debug, Clone, Copy)]
enum PendingSeek {
    Ms(u32),
    Pos(usize),
    Rewind,
}

#[derive(Debug)]
pub struct NativeAudioService {
    audio: Option<NativeAudio>,
    /// Whether playback was started and not yet paused. The stream itself has
    /// no queryable transport state.
    playing: bool,
    /// The latest requested muting, flushed on every play.
    muting: Muting,
    /// The latest requested panning, flushed on every play.
    panning: Panning,
    /// The latest requested any-chip mutes (the generic engine's), flushed on
    /// every play like the OPL muting.
    chip_muting: ChipMuting,
    /// The latest requested any-chip pans, likewise.
    chip_panning: ChipPanning,
    /// The latest requested per-chip trims, likewise.
    chip_trims: ChipTrims,
    /// The latest requested boost, flushed on every play.
    boost: f32,
    /// The latest requested loop region, flushed on every play.
    loop_config: Option<LoopConfig>,
    /// The seek to apply on the next play, when one arrived while paused.
    pending_seek: Option<PendingSeek>,
}

impl Default for NativeAudioService {
    fn default() -> Self {
        Self {
            audio: None,
            playing: false,
            muting: Muting::all(),
            panning: Panning::Original,
            chip_muting: ChipMuting::new(),
            chip_panning: ChipPanning::new(),
            chip_trims: ChipTrims::new(),
            boost: 1.0,
            loop_config: None,
            pending_seek: None,
        }
    }
}

impl NativeAudioService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the stream is running its callback and draining the command
    /// queue, so a command may be pushed immediately.
    fn stream_live(&self) -> bool {
        self.playing && self.audio.is_some()
    }

    /// The any-chip mutes this service last accepted. For the sibling module's
    /// test that the switching wrapper forwards them -- reading the stored
    /// state needs no audio device, which is the point.
    #[cfg(test)]
    pub(super) const fn last_chip_muting(&self) -> &ChipMuting {
        &self.chip_muting
    }

    /// The any-chip pans this service last accepted, as above.
    #[cfg(test)]
    pub(super) const fn last_chip_panning(&self) -> &ChipPanning {
        &self.chip_panning
    }

    /// The per-chip trims this service last accepted, as above.
    #[cfg(test)]
    pub(super) const fn last_chip_trims(&self) -> &ChipTrims {
        &self.chip_trims
    }
}

impl AudioService for NativeAudioService {
    fn load(&mut self, source: AudioSource, config: &AudioConfig) -> Result<(), String> {
        // Drop any current stream first: two open output streams would play
        // over each other.
        self.unload();
        self.audio = Some(NativeAudio::new(&source, config).map_err(|e| e.to_string())?);
        // Adopt the config's boost, or the play-time flush below would clobber a
        // config-loaded boost with the service's stale value.
        self.boost = config.boost;
        Ok(())
    }

    fn unload(&mut self) {
        self.audio = None;
        self.playing = false;
        self.pending_seek = None;
    }

    fn play(&mut self) -> Result<(), String> {
        let audio = self
            .audio
            .as_mut()
            .ok_or("No song is loaded into the audio output.")?;
        match self.pending_seek.take() {
            Some(PendingSeek::Ms(ms)) => audio.seek_ms(ms),
            Some(PendingSeek::Pos(pos)) => audio.seek_pos(pos),
            Some(PendingSeek::Rewind) => audio.rewind(),
            None => {}
        }
        audio.set_muting(self.muting);
        audio.set_panning(self.panning);
        audio.set_chip_muting(self.chip_muting.clone());
        audio.set_chip_panning(self.chip_panning.clone());
        audio.set_chip_trims(self.chip_trims.clone());
        audio.set_boost(self.boost);
        audio.set_loop(self.loop_config);
        audio.play().map_err(|e| e.to_string())?;
        self.playing = true;
        Ok(())
    }

    fn pause(&mut self) {
        if let Some(audio) = &self.audio
            && let Err(error) = audio.pause()
        {
            log::warn!("could not pause the audio stream: {error}");
        }
        self.playing = false;
    }

    fn seek_ms(&mut self, ms: u32) {
        if self.stream_live() {
            self.audio
                .as_mut()
                .expect("stream_live checked")
                .seek_ms(ms);
        } else {
            self.pending_seek = Some(PendingSeek::Ms(ms));
        }
    }

    fn seek_pos(&mut self, pos: usize) {
        if self.stream_live() {
            self.audio
                .as_mut()
                .expect("stream_live checked")
                .seek_pos(pos);
        } else {
            self.pending_seek = Some(PendingSeek::Pos(pos));
        }
    }

    fn rewind(&mut self) {
        if self.stream_live() {
            self.audio.as_mut().expect("stream_live checked").rewind();
        } else {
            self.pending_seek = Some(PendingSeek::Rewind);
        }
    }

    fn set_muting(&mut self, muting: Muting) {
        self.muting = muting;
        if self.stream_live() {
            self.audio
                .as_mut()
                .expect("stream_live checked")
                .set_muting(muting);
        }
    }

    fn set_panning(&mut self, panning: Panning) {
        self.panning = panning;
        if self.stream_live() {
            self.audio
                .as_mut()
                .expect("stream_live checked")
                .set_panning(panning);
        }
    }

    fn set_chip_muting(&mut self, muting: ChipMuting) {
        self.chip_muting = muting.clone();
        if self.stream_live() {
            self.audio
                .as_mut()
                .expect("stream_live checked")
                .set_chip_muting(muting);
        }
    }

    fn set_chip_panning(&mut self, panning: ChipPanning) {
        self.chip_panning = panning.clone();
        if self.stream_live() {
            self.audio
                .as_mut()
                .expect("stream_live checked")
                .set_chip_panning(panning);
        }
    }

    fn set_chip_trims(&mut self, trims: ChipTrims) {
        self.chip_trims = trims.clone();
        if self.stream_live() {
            self.audio
                .as_mut()
                .expect("stream_live checked")
                .set_chip_trims(trims);
        }
    }

    fn set_boost(&mut self, boost: f32) {
        self.boost = boost;
        if self.stream_live() {
            self.audio
                .as_mut()
                .expect("stream_live checked")
                .set_boost(boost);
        }
    }

    fn set_loop(&mut self, config: Option<LoopConfig>) {
        self.loop_config = config;
        if self.stream_live() {
            self.audio
                .as_mut()
                .expect("stream_live checked")
                .set_loop(config);
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
        self.audio.as_ref().is_some_and(NativeAudio::is_finished)
    }

    fn position(&self) -> Option<Position> {
        self.audio.as_ref().map(NativeAudio::position)
    }

    fn take_peaks(&mut self) -> Option<[f32; 2]> {
        // A paused stream runs no callback, so the peaks it reports are simply
        // zero and the meter decays -- no deferred handling needed.
        self.audio.as_ref().map(NativeAudio::take_peaks)
    }

    fn output_rate(&self) -> Option<u32> {
        self.audio.as_ref().map(NativeAudio::sample_rate)
    }

    fn min_engaged_boost(&self) -> Option<f32> {
        self.audio.as_ref().and_then(NativeAudio::min_engaged_boost)
    }

    fn take_limited(&mut self) -> bool {
        self.audio.as_ref().is_some_and(NativeAudio::take_limited)
    }

    fn last_error(&mut self) -> Option<String> {
        // A device fault (unplugged mid-song) surfaces on cpal's error callback,
        // away from any call the app made, so this is where it reaches the UI.
        self.audio.as_mut().and_then(NativeAudio::take_error)
    }
}
