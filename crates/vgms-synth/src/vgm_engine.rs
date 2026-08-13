//! Playing a VGM for whatever chips it declares.
//!
//! The one playback engine, and it knows no chip at all: it walks the command
//! stream, hands each write to whichever [`ChipCore`] the header says owns it,
//! counts out the waits, and mixes what the cores render. Everything
//! chip-specific is behind the trait. A DRO reaches here too, projected to its
//! VGM at the seam, so OPL playback is the same code path as everything else.
//!
//! Cores render at their own rates, so each is resampled to the output rate on
//! the way into the mix. The pull contract is `render(&mut [i16]) -> usize`, so
//! the native audio thread, the WAV renderer and the waveform renderer all drive
//! it the same way whatever the document.
//!
//! Routing, banks and timing are all testable against
//! [`RecordingChip`](crate::chip::RecordingChip) without an emulator in sight,
//! so a chip with no registered core renders silence rather than failing.

use std::sync::Arc;

use vgms_core::VgmFile;
use vgms_core::vgm::header::ChipUse;
use vgms_core::vgm::stream::{ChipTarget, VgmCommand, VgmStream};

use crate::banks::{Banks, BlockKind, block_owner, ram_header, rom_header, stream_owner};
use crate::chip::ChipCore;
use crate::chip_mix::{ChipMuting, ChipPanning, ChipTrims};
use crate::clock::{FrameClock, LoopConfig, Position};
use crate::dac_stream::{DacStreams, PendingWrite};
use crate::decompress::{DecompressionTable, decompress};
use crate::resample::{ResampleMode, Resampler};

/// One chip instance, with the resampler that brings it to the output rate.
struct Voice {
    target: ChipTarget,
    core: Box<dyn ChipCore>,
    /// The rates the resampler was built from, kept so a mode change can
    /// rebuild it without asking the core to re-derive anything.
    native_rate: u32,
    output_rate: u32,
    /// The conversion method the resampler was built with, kept so a *rate*
    /// change can rebuild it without forgetting the user's mode choice.
    resample_mode: ResampleMode,
    /// Band-limited rate conversion from the chip's rate to the engine's.
    ///
    /// Band-limited because linear interpolation is a fair approximation near
    /// 1:1 and nothing of the kind at 5:1 -- the SN76489 renders at 223721 Hz,
    /// and everything it puts above 22 kHz would fold straight back into the
    /// audible band. See [`crate::resample`].
    resampler: Resampler,
    /// This voice's share of the file's cross-chip balance, 8.8 fixed point
    /// ([`balance::GAIN_UNITY`] = 1.0). Exactly unity for a single-chip file;
    /// see [`balance`](crate::balance) for what it mirrors and why it is a
    /// ratio. Applied per frame *before* the voices are summed, so the
    /// headroom exists before the mix clamps to 16 bits.
    balance: u32,
    /// A whole-chip stereo placement, `[left, right]` in the same 8.8 fixed point
    /// as `balance` ([`balance::GAIN_UNITY`] = 1.0). Unity on both sides for every
    /// ordinary voice; a `dro2vgm` dual-OPL2 (bit 31) instead hard-pans its two
    /// YM3812 instances -- instance 0 to `[2.0, 0]`, instance 1 to `[0, 2.0]` --
    /// the SB Pro image an OPL2 cannot make itself. Applied per side in
    /// [`Self::next_frame`] *after* `balance`; the doubled surviving side undoes
    /// the dual-declaration halving `balance` already applied, matching libvgm.
    stereo: [u32; 2],
    /// The user's listening trim for this chip instance, 8.8 fixed point
    /// ([`balance::GAIN_UNITY`] = 1.0 = 100%), derived from the panel's percent
    /// in [`apply_mix`](VgmEngine::apply_mix). Unity by default, so 100% is the
    /// reference balance untouched; it only attenuates. Applied as a mono factor
    /// alongside `balance` in [`Self::next_frame`], never saved or written to the
    /// file -- it lives only in the ear.
    trim: u32,
    /// How many channels this chip has, so [`apply_mix`](VgmEngine::apply_mix)
    /// can tell "every channel muted" from a partial mask.
    channel_count: u32,
    /// A mute mask covering every channel silences the voice here in the
    /// engine, whatever the core can do -- the whole-chip Mute/Solo controls
    /// rest on this, and it is what makes them work even for a core with no
    /// per-channel mute. The core still renders (mute is not pause; its state
    /// must stay where the music is), the frames just do not reach the sum.
    silenced: bool,
}

impl Voice {
    fn new(
        target: ChipTarget,
        mut core: Box<dyn ChipCore>,
        chip: &ChipUse,
        settings: &vgms_core::vgm::ChipSettings,
        output_rate: u32,
        balance: u32,
    ) -> Self {
        core.reset(chip.clock, chip.variant);
        // After the reset, which is what clears the state this configures.
        core.configure(settings);
        let native = core.native_rate().max(1);
        Self {
            target,
            core,
            native_rate: native,
            output_rate,
            resample_mode: ResampleMode::default(),
            resampler: Resampler::new(native, output_rate),
            balance,
            stereo: [crate::balance::GAIN_UNITY; 2],
            trim: crate::balance::GAIN_UNITY,
            channel_count: vgms_core::vgm::channels_of(chip.kind, chip.variant).len() as u32,
            silenced: false,
        }
    }

    /// Whether this voice takes `target`'s writes: the chip, and which of its
    /// (up to two) instances. The port is the chip's own business.
    fn accepts(&self, target: ChipTarget) -> bool {
        self.target.kind == target.kind && self.target.instance == target.instance
    }

    /// Rebuilds the resampler if the core's rate has moved since it was built.
    ///
    /// A core's rate is not a constant: the ES5503 re-derives its output rate
    /// from the oscillator-enable register (`clock / 8 / (oscillators + 2)`,
    /// so ~298 kHz at reset and ~26 kHz once a IIgs rip enables all 32), and
    /// libvgm announces the change through its sample-rate-change callback.
    /// Called after every register write -- the only place a rate can move --
    /// so a stale ratio cannot play a passage 11x too fast, which is what the
    /// ES5503 did before this existed (parity corr 0.0022). The rebuild drops
    /// the resampler's ~0.4 ms tail; rips change rate during driver init, not
    /// mid-note, so nothing audible is lost.
    fn follow_rate(&mut self) {
        let native = self.core.native_rate().max(1);
        if native != self.native_rate {
            self.native_rate = native;
            self.resampler = Resampler::with_mode(native, self.output_rate, self.resample_mode);
        }
    }

    /// The next output frame, band-limited from the chip's own rate.
    ///
    /// One source frame is pulled at a time. A core that would rather render in
    /// blocks can buffer internally; doing it here would mean either a lookahead
    /// the caller's chunk size could observe, or a buffer flushed between pulls
    /// -- and the contract is that neither is visible. It is also what keeps a
    /// register write landing where the music put it: a resampler that demanded
    /// a block of source frames would advance the chip past the next write
    /// before applying it, and chips whose drivers write at audio rate (the
    /// SN76489's sample playback, every DAC stream) depend on that not
    /// happening.
    fn next_frame(&mut self) -> [i32; 2] {
        let core = &mut self.core;
        let frame = self.resampler.next_frame(|| {
            let mut frame = [0i32; 2];
            core.render(&mut frame);
            frame
        });
        // After the pull: a silenced chip keeps running (mute is not pause),
        // its frames just never reach the sum.
        if self.silenced {
            return [0, 0];
        }
        // Unity on every axis is the overwhelming common case (a single, centred
        // chip at full trim): hand the core's frame straight back untouched. The
        // stereo check is load-bearing -- a hard-panned voice whose balance
        // happens to be unity must not skip its pan.
        if self.balance == crate::balance::GAIN_UNITY
            && self.trim == crate::balance::GAIN_UNITY
            && self.stereo == [crate::balance::GAIN_UNITY; 2]
        {
            return frame;
        }
        // Cross-chip balance and the user trim (both mono) first, then the
        // whole-chip stereo placement.
        let scale = |sample: i32, gain: u32| ((i64::from(sample) * i64::from(gain)) >> 8) as i32;
        let mono = |sample: i32| scale(scale(sample, self.balance), self.trim);
        [
            scale(mono(frame[0]), self.stereo[0]),
            scale(mono(frame[1]), self.stereo[1]),
        ]
    }
}

/// A VGM played through whatever cores this app has.
pub struct VgmEngine {
    file: Arc<VgmFile>,
    voices: Vec<Voice>,
    banks: Banks,
    /// The last `0x7F` table block seen, which the table-lookup and DPCM
    /// compression schemes decode against. A file may send several; the one in
    /// force is the last before the block being unpacked.
    table: Option<DecompressionTable>,
    /// The `0x90`-`0x95` streams, which write on their own clock rather than on
    /// the command stream's.
    streams: DacStreams,
    /// Scratch for the bytes a frame's streams fall due for. Held rather than
    /// allocated per frame: `mix` runs once per output sample.
    due: Vec<PendingWrite>,
    /// The YM2612 DAC fast path's read cursor into the concatenated type-`0x00`
    /// bank: where the next `0x8n` command reads its sample byte. `0xE0` seeks
    /// it; each `0x8n` advances it.
    pcm_pos: usize,
    /// A shadow of each HuC6280 instance's channel-select register (its
    /// register 0), fed by every write that passes through. A DAC stream's
    /// channel switch restores this value afterwards -- the reference reads the
    /// register back from the core; the shadow is that value without a read
    /// path through the FFI.
    huc6280_channel: [u8; 2],
    clock: FrameClock,
    /// The next command to execute.
    index: usize,
    /// Output frames still owed by the wait being served.
    pending: u64,
    /// Whether the stream has run out.
    finished: bool,
    /// Output frames rendered since the last rewind or seek. What a position
    /// readout counts, and what a seek has to restate.
    frames_rendered: u64,
    /// The region playback jumps back over, if any.
    loop_config: Option<LoopConfig>,
    /// Wraps still owed, or `None` for a loop that never stops.
    wraps_remaining: Option<u32>,
    /// Wraps done since the last seek, for the position readout.
    loops_done: u32,
    output_rate: u32,
    /// The channel mutes in force, kept so every reset (rewind, seek) can
    /// say them again -- a core's mask does not survive its own reset.
    muting: ChipMuting,
    /// The channel pans in force, reapplied the same way.
    panning: ChipPanning,
    /// The per-chip listening trims in force, reapplied the same way. Purely a
    /// gain the engine multiplies -- unlike mutes and pans it never travels to a
    /// core, so it holds on every chip regardless of what the core supports.
    trims: ChipTrims,
}

impl std::fmt::Debug for VgmEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VgmEngine")
            .field("file", &self.file.name)
            .field("voices", &self.voiced_chips())
            .field("banks", &self.banks.len())
            .field("index", &self.index)
            .field("pending", &self.pending)
            .field("finished", &self.finished)
            .finish()
    }
}

impl VgmEngine {
    /// Builds an engine for `file`, rendering at `output_rate` Hz.
    ///
    /// Every chip the header clocks gets a core if one is registered, and is
    /// skipped if not -- so a file with one known chip and one unknown plays the
    /// known one and leaves the other silent. [`playability`](crate::chip::playability)
    /// is how a caller finds that out before committing to it.
    #[must_use]
    pub fn new(file: Arc<VgmFile>, output_rate: u32) -> Self {
        // By value, so the factory closure owns them: the file's header
        // settings pick the default core where the promoted one cannot honour
        // them (the SN76489's noise parameters -- see `core_for_file`).
        let settings = *file.header.settings();
        Self::with_cores(file, output_rate, move |kind| {
            crate::chip::core_for_file(kind, &settings)
        })
    }

    /// The same, taking its cores from `factory` instead of the registry.
    ///
    /// How the engine is tested. Routing, banks and timing are assertions about
    /// what reached which chip, and a
    /// [`RecordingChip`](crate::chip::RecordingChip) answers those without an
    /// emulator -- which also means they keep answering when the real cores
    /// arrive and start disagreeing about samples.
    #[must_use]
    pub fn with_cores(
        file: Arc<VgmFile>,
        output_rate: u32,
        factory: impl Fn(vgms_core::vgm::ChipKind) -> Option<Box<dyn ChipCore>>,
    ) -> Self {
        let mut voices = Vec::new();
        for chip in file.header.chips() {
            // A dual-chip declaration is two instances of the same chip, each
            // needing its own core.
            let instances = if chip.dual { 2 } else { 1 };
            for instance in 0..instances {
                if let Some(core) = factory(chip.kind) {
                    let target = ChipTarget {
                        kind: chip.kind,
                        instance,
                        port: 0,
                    };
                    // The second instance's clock can differ: the v1.70 extra
                    // header lists per-instance clocks, and the reference
                    // resolves instance 1 through it (GetChipClock walks
                    // _xHdrChipClk). Without this, a dual-chip file whose two
                    // chips run at different clocks played the second at the
                    // first's clock -- wrong pitch and rate.
                    let mut chip = *chip;
                    if instance == 1
                        && let Some(entry) = file.header.extra().and_then(|extra| {
                            extra.clocks.iter().find(|entry| {
                                vgms_core::vgm::ChipKind::from_id(entry.chip_id) == Some(chip.kind)
                            })
                        })
                    {
                        chip.clock = entry.clock & 0x3FFF_FFFF;
                        chip.variant = entry.clock & 0x8000_0000 != 0;
                    }
                    let chip = &chip;
                    // The reference's cross-chip balance for this instance in
                    // this file's chip set -- unity for a single-chip file.
                    let balance = crate::balance::voice_gain(
                        file.header.chips(),
                        chip,
                        instance,
                        file.header.extra(),
                        &file.header.tail_chip_ids(),
                    );
                    let mut voice = Voice::new(
                        target,
                        core,
                        chip,
                        file.header.settings(),
                        output_rate,
                        balance,
                    );
                    // A dro2vgm dual-OPL2 plays its two YM3812s hard left and
                    // hard right (the SB Pro image) -- a whole-chip pan an OPL2
                    // cannot make itself. Double the surviving side, as libvgm
                    // does, to undo the dual-declaration halving `balance` applied.
                    if chip.is_dual_opl2_stereo() {
                        let live = crate::balance::GAIN_UNITY * 2;
                        voice.stereo = if instance == 0 { [live, 0] } else { [0, live] };
                    }
                    voices.push(voice);
                }
            }
        }
        Self {
            file,
            voices,
            banks: Banks::new(),
            table: None,
            streams: DacStreams::new(output_rate),
            due: Vec::new(),
            pcm_pos: 0,
            huc6280_channel: [0, 0],
            clock: FrameClock::new(output_rate, vgms_core::vgm::VGM_SAMPLE_RATE),
            index: 0,
            pending: 0,
            finished: false,
            frames_rendered: 0,
            loop_config: None,
            wraps_remaining: None,
            loops_done: 0,
            output_rate: output_rate.max(1),
            muting: ChipMuting::new(),
            panning: ChipPanning::new(),
            trims: ChipTrims::new(),
        }
    }

    /// Applies (and keeps) which channels are muted. Live: a playing stream
    /// changes mid-note, exactly as the OPL engine's muting does.
    pub fn set_muting(&mut self, muting: ChipMuting) {
        self.muting = muting;
        self.apply_mix();
    }

    /// Applies (and keeps) where each channel sits in the stereo image.
    /// Ignored by cores that cannot pan (see
    /// [`ChipCore::supports_pan`](crate::ChipCore::supports_pan)).
    pub fn set_panning(&mut self, panning: ChipPanning) {
        self.panning = panning;
        self.apply_mix();
    }

    /// Applies (and keeps) the per-chip listening trims. Live, like muting: a
    /// playing stream changes level mid-note. Purely a gain on the engine's
    /// side, so it holds on every core -- one with no per-channel controls
    /// included.
    pub fn set_trims(&mut self, trims: ChipTrims) {
        self.trims = trims;
        self.apply_mix();
    }

    /// Pushes the stored mutes, pans and trims into every voice.
    ///
    /// Also called after every reset (rewind, seek): a core's mask and pans
    /// do not survive its own reset, so the engine restates them -- the
    /// generic counterpart of the OPL engine's `mask_replay` rule, where the
    /// bug would be a muted channel coming back after a seek.
    fn apply_mix(&mut self) {
        for voice in &mut self.voices {
            let mask = self
                .muting
                .mask_for(voice.target.kind, voice.target.instance);
            voice.core.set_channel_mutes(mask);
            // A mask covering the whole chip is a promise the engine keeps
            // itself, so the whole-chip Mute/Solo hold on every core -- a core
            // with no per-channel mute included. Every channel's bit must be
            // set, not merely enough bits: a stray high bit is not a mute.
            let full = (1u64 << voice.channel_count) - 1;
            voice.silenced = voice.channel_count > 0 && u64::from(mask) & full == full;
            // The trim is the engine's own gain, so unlike the pans it never
            // reaches the core: 100% is unity, and a percent maps to 8.8 by
            // `percent * GAIN_UNITY / 100`.
            let percent = self
                .trims
                .percent_for(voice.target.kind, voice.target.instance);
            voice.trim = u32::from(percent) * crate::balance::GAIN_UNITY / 100;
            if let Some(pans) = self
                .panning
                .pans_for(voice.target.kind, voice.target.instance)
            {
                voice.core.set_channel_pans(pans);
            }
        }
    }

    /// The chips this engine actually has cores for.
    #[must_use]
    pub fn voiced_chips(&self) -> Vec<ChipTarget> {
        self.voices.iter().map(|voice| voice.target).collect()
    }

    /// Whether the stream has been played to its end -- every command consumed
    /// **and** the last wait's frames rendered out.
    ///
    /// The `pending == 0` term matters: `finished` flips as soon as the final
    /// command is read, but that command is usually a wait, and its frames (a
    /// held final note, the tail of a fade) have still to be rendered. Reporting
    /// "finished" while they are pending would cut them off -- which is exactly
    /// what the hardware pump does with this signal.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished && self.pending == 0
    }

    /// Chooses how every voice is brought to the output rate.
    ///
    /// [`ResampleMode::Sinc`] is the accurate default; [`ResampleMode::Linear`]
    /// is the old lerp kept as a deliberate option -- the aliased "crunchy"
    /// sound of VGMPlay and most classic players. Each voice's resampler is
    /// rebuilt empty, which is only seamless at a boundary the app already
    /// treats as one (load, rewind, seek) -- callers switch the mode and then
    /// restart or reload, exactly as they do for a core change.
    pub fn set_resample_mode(&mut self, mode: ResampleMode) {
        for voice in &mut self.voices {
            voice.resample_mode = mode;
            voice.resampler = Resampler::with_mode(voice.native_rate, voice.output_rate, mode);
        }
    }

    pub fn rewind(&mut self) {
        for voice in &mut self.voices {
            // The header's clock and variant are what it was built with; a reset
            // has to say them again, so read them back off the file.
            if let Some(chip) = self
                .file
                .header
                .chips()
                .iter()
                .find(|chip| chip.kind == voice.target.kind)
            {
                voice.core.reset(chip.clock, chip.variant);
                // A reset clears what `configure` set, so the two travel
                // together everywhere -- a rewound engine must be the same
                // chip it was.
                voice.core.configure(self.file.header.settings());
                // And it returns a rate-deriving core (the ES5503) to its
                // reset rate, so the ratio has to be re-read with it.
                voice.follow_rate();
            }
            // The resampler holds a tail of the passage just left -- about
            // 0.4 ms of it -- so it is cleared with the chip rather than
            // allowed to bleed across the seam.
            voice.resampler.reset();
        }
        self.banks.clear();
        self.table = None;
        self.streams.clear();
        self.due.clear();
        self.pcm_pos = 0;
        self.huc6280_channel = [0, 0];
        self.clock.reset();
        self.index = 0;
        self.pending = 0;
        self.finished = false;
        self.frames_rendered = 0;
        self.restart_loop_count();
        // The resets above cleared every core's mutes and pans; restate them.
        self.apply_mix();
    }

    /// Jumps to row `index`, putting the chips in the state the music expects
    /// to find them in.
    ///
    /// Not by replaying the stream. The commands before `index` are folded into
    /// a per-chip state -- each cell's last write, the data blocks (cumulative,
    /// so all of them), the DAC-stream setup and the PCM seek -- and only those
    /// are executed. A minute into a Mega Drive rip that is a few hundred writes
    /// instead of a few hundred thousand.
    ///
    /// The fold is [`vgms_core::chip_state`], the same one the crop edit and the
    /// splitter's prelude use, so a seek and a crop agree by construction about
    /// what "the state at row N" means. Notes sounding at `index` re-attack,
    /// which is what `vgm_trim` does too, and a stream that was mid-playback
    /// restarts from its own offset rather than from where it had got to.
    pub fn seek_to_row(&mut self, index: usize) {
        let file = Arc::clone(&self.file);
        let Some(stream) = file.stream() else {
            return;
        };
        let index = index.min(stream.len());
        self.rewind();

        let state = vgms_core::chip_state::ChipState::fold(stream, index);
        for restore in state.restore_indices() {
            if let Some(command) = stream.get(restore) {
                // The return is a wait length, and a restore never waits: the
                // fold keeps writes and blocks, not the time between them. A
                // restore is a replay, so a buffered core applies it immediately.
                self.execute(stream, restore, command, true);
            }
        }

        self.index = index;
        self.finished = index >= stream.len();
        // The clock's carried remainder belongs to the time that was skipped.
        self.clock.reset();
        self.restart_loop_count();
        // The position readout counts from the start of the song, not from the
        // seek, so it restates where the seek landed rather than resetting.
        let samples = stream.samples_before(index);
        self.frames_rendered =
            samples * u64::from(self.output_rate) / u64::from(vgms_core::vgm::VGM_SAMPLE_RATE);
    }

    /// Jumps to the row playing at `ms`, for a transport that seeks by time.
    ///
    /// A target inside a delay stops on that delay, never after it -- a
    /// prefix-sum over the command stream's waits.
    pub fn seek_to_ms(&mut self, ms: u32) {
        let Some(stream) = self.file.stream() else {
            return;
        };
        let target = u64::from(ms) * u64::from(vgms_core::vgm::VGM_SAMPLE_RATE) / 1000;
        self.seek_to_row(stream.seek_index_for_samples(target));
    }

    /// Sets (or clears) the region playback loops over.
    ///
    /// A region that is empty or reaches past the stream is dropped, so playback
    /// simply does not loop -- the same rule the OPL player applies.
    pub fn set_loop(&mut self, config: Option<LoopConfig>) {
        let len = self.file.len();
        self.loop_config = config.filter(|config| config.start < config.end && config.end <= len);
        self.restart_loop_count();
    }

    /// The loop region in force, if any.
    #[must_use]
    pub const fn loop_config(&self) -> Option<LoopConfig> {
        self.loop_config
    }

    /// Re-arms the repeat count from the current config, for a fresh run at it.
    fn restart_loop_count(&mut self) {
        self.wraps_remaining = self.loop_config.and_then(|config| config.count.wraps());
        self.loops_done = 0;
    }

    /// Jumps back to the loop start when playback has reached the loop end with
    /// a repeat still owed, reporting whether it did.
    ///
    /// Deliberately does **not** reset or replay the chips, the same choice the
    /// OPL player made: a real VGM player carries its register state across the
    /// seam, and hearing whether that seam is clean is the whole point of
    /// auditioning a loop. `frames_rendered` rewinds to the loop start so the
    /// position readout and the waveform cursor wrap with the audio.
    fn wrap_to_loop_start(&mut self) -> bool {
        let Some(config) = self.loop_config else {
            return false;
        };
        if self.index != config.end || self.wraps_remaining == Some(0) {
            return false;
        }
        // A region holding no waits renders no audio, so looping it would spin
        // here forever without ever filling the caller's buffer.
        if self.frames_rendered == config.start_frames {
            log::warn!("the loop region renders no audio; playing on without looping");
            self.loop_config = None;
            return false;
        }
        if let Some(remaining) = self.wraps_remaining.as_mut() {
            *remaining -= 1;
        }
        self.index = config.start;
        self.frames_rendered = config.start_frames;
        self.loops_done += 1;
        true
    }

    /// Whether the next `render` would jump back rather than stop.
    fn owes_a_wrap(&self) -> bool {
        self.loop_config
            .is_some_and(|config| self.index == config.end && self.wraps_remaining != Some(0))
    }

    /// Where playback has reached, in the shape the transport already reads.
    #[must_use]
    pub fn position(&self) -> Position {
        Position::looping(
            self.frames_rendered,
            self.output_rate,
            self.index,
            self.loops_done,
        )
    }

    /// Fills `out` with interleaved stereo frames, returning how many frames it
    /// wrote. A short return means the stream ended.
    ///
    /// `out.len()` must be even. The output does not depend on how the caller
    /// chunks its calls.
    pub fn render(&mut self, out: &mut [i16]) -> usize {
        let wanted = out.len() / 2;
        let mut done = 0;
        while done < wanted {
            if self.pending == 0 {
                if self.finished {
                    break;
                }
                self.run_until_wait();
                if self.pending == 0 && self.finished {
                    break;
                }
            }
            let take = usize::try_from(self.pending)
                .unwrap_or(usize::MAX)
                .min(wanted - done);
            self.mix(&mut out[done * 2..(done + take) * 2]);
            self.pending -= take as u64;
            done += take;
            self.frames_rendered += take as u64;
        }
        // A short render leaves the tail as the caller found it; zero it so a
        // reused buffer cannot replay the previous pull's audio.
        out[done * 2..].fill(0);
        done
    }

    /// Executes commands until one asks for a wait, or the stream ends.
    fn run_until_wait(&mut self) {
        // A handle of its own, so executing a command can take `&mut self` while
        // the stream is being walked. The refcount bump is once per wait served,
        // not once per command.
        let file = Arc::clone(&self.file);
        let Some(stream) = file.stream() else {
            self.finished = true;
            return;
        };
        while self.pending == 0 {
            // The seam first: a loop whose end *is* the end of the stream must
            // jump back rather than finish.
            if self.wrap_to_loop_start() {
                continue;
            }
            if self.index >= stream.len() {
                break;
            }
            let index = self.index;
            self.index += 1;
            let Some(command) = stream.get(index) else {
                continue;
            };
            let samples = self.execute(stream, index, command, false);
            if samples > 0 {
                self.pending = self.clock.frames_for(samples);
            }
        }
        if self.index >= stream.len() && !self.owes_a_wrap() {
            self.finished = true;
        }
    }

    /// Performs one command, returning how many VGM samples it waits for.
    fn execute(
        &mut self,
        stream: &VgmStream,
        index: usize,
        command: VgmCommand,
        replay: bool,
    ) -> u32 {
        match command {
            VgmCommand::Write { target, addr, data } => {
                self.write(target, addr, data, replay);
                0
            }
            VgmCommand::Wait(samples) => samples,
            VgmCommand::DacWrite { wait } => {
                // The YM2612 DAC fast path: play the next byte of the PCM bank
                // -- a write to the DAC's sample port, `0x2A` -- then wait. The
                // cursor advances past the end without wrapping; a file that
                // reads past its bank just stops making samples, which is what
                // hardware fed no data does too.
                if let Some(byte) = self.banks.byte_at(0x00, self.pcm_pos) {
                    self.write(
                        ChipTarget {
                            kind: vgms_core::vgm::ChipKind::Ym2612,
                            instance: 0,
                            port: 0,
                        },
                        0x2A,
                        u16::from(byte),
                        replay,
                    );
                }
                self.pcm_pos = self.pcm_pos.saturating_add(1);
                wait
            }
            VgmCommand::DataBlock { kind, .. } => {
                self.data_block(stream, index, kind);
                0
            }
            VgmCommand::DacStream { opcode, stream_id } => {
                self.dac_stream(stream, index, opcode, stream_id);
                0
            }
            VgmCommand::PcmRamWrite { .. } => {
                self.pcm_ram_write(stream, index);
                0
            }
            VgmCommand::SeekPcm(offset) => {
                self.pcm_pos = offset as usize;
                0
            }
            VgmCommand::Raw { .. } => 0,
            VgmCommand::OverrideWait { .. } => {
                // `0x64` was a withdrawn v1.70 proposal -- its own author noted
                // "Not yet implemented. Am I really sure about this?" -- gone by
                // v1.71 and used by no real file; libvgm and legacy VGMPlay both
                // classify it invalid and stop playback. We keep decoding the
                // opcode so the file still opens, but ignore its effect: the short
                // waits keep their fixed 735/882-sample lengths.
                0
            }
        }
    }

    /// Performs one of the `0x90`-`0x95` stream-control commands.
    ///
    /// The operands are read back out of the command's own bytes: the decoder
    /// gives the opcode and the stream id, which is what the *editor* needs to
    /// label a row, and the rest is playback's business.
    fn dac_stream(&mut self, stream: &VgmStream, index: usize, opcode: u8, id: u8) {
        let Some(bytes) = stream.raw_command(index) else {
            return;
        };
        // Every operand is past the opcode and the stream id.
        let operands = &bytes[2.min(bytes.len())..];
        let u32_at = |at: usize| -> u32 {
            operands
                .get(at..at + 4)
                .and_then(|slice| slice.try_into().ok())
                .map_or(0, u32::from_le_bytes)
        };
        let u16_at = |at: usize| -> u16 {
            operands
                .get(at..at + 2)
                .and_then(|slice| slice.try_into().ok())
                .map_or(0, u16::from_le_bytes)
        };
        let byte = |at: usize| -> u8 { operands.get(at).copied().unwrap_or(0) };

        match opcode {
            0x90 => self.streams.setup(id, byte(0), byte(1), byte(2)),
            0x91 => self.streams.bind(id, byte(0), byte(1), byte(2)),
            0x92 => self.streams.set_rate(id, u32_at(0)),
            0x93 => {
                // The bank a `0x91` bound, as one run: the spec's offsets are
                // into the whole type, not into one block.
                let data = self.banks.concatenated(self.streams.bank_type(id));
                self.streams.start(id, &data, u32_at(0), byte(4), u32_at(5));
            }
            0x94 => self.streams.stop(id),
            0x95 => {
                // The fast form: play block `n` of the bound type, addressed as
                // an offset into the whole concatenated bank with a byte-count
                // length -- upstream's `bankOfs`/`bankSize` plus
                // `DCTRL_LMODE_BYTES`. Flag bit 0 loops, bit 4 reverses.
                let bank_type = self.streams.bank_type(id);
                let block = usize::from(u16_at(0));
                if let Some((offset, size)) = self.banks.nth_offset(bank_type, block) {
                    let data = self.banks.concatenated(bank_type);
                    let flags = byte(2);
                    let mode = 0x04 | (flags & 0x10) | ((flags & 0x01) << 7);
                    self.streams
                        .start(id, &data, offset as u32, mode, size as u32);
                }
            }
            _ => {}
        }
    }

    /// Routes a register write to the core that owns it.
    fn write(&mut self, target: ChipTarget, addr: u16, data: u16, replay: bool) {
        // Shadow the HuC6280's channel-select register on the way past, so a
        // DAC stream can restore the song's selected channel after its own
        // channel switch (the reference reads the register back instead).
        if target.kind == vgms_core::vgm::ChipKind::HuC6280 && target.port == 0 && addr == 0 {
            self.huc6280_channel[usize::from(target.instance != 0)] = data as u8;
        }
        for voice in &mut self.voices {
            if voice.accepts(target) {
                if replay {
                    voice.core.replay_write(target.port, addr, data);
                } else {
                    voice.core.write(target.port, addr, data);
                }
                voice.follow_rate();
                return;
            }
        }
    }

    /// Files a `0x67` block: kept as a bank, or handed to the chip that owns it.
    fn data_block(&mut self, stream: &VgmStream, index: usize, kind: u8) {
        // `0x67 0x66 tt ssssssss` -- seven bytes of header, then the payload.
        const HEADER: usize = 7;
        let Some(bytes) = stream.raw_command(index) else {
            return;
        };
        let payload = &bytes[HEADER.min(bytes.len())..];
        // Bit 31 of the size field marks a block for the second chip.
        let second_chip = bytes.get(HEADER - 1).is_some_and(|byte| byte & 0x80 != 0);
        let instance = u8::from(second_chip);

        // Stream banks and their tables load on the first pass only: a loop
        // wrap re-executes the commands, and re-appending the blocks would
        // grow the bank each pass -- a later play-to-end stream would then run
        // on into the duplicate. The reference skips types 0x00-0x7F whenever
        // `_curLoop > 0`; ROM and RAM writes still replay (idempotent).
        if self.loops_done > 0
            && matches!(
                BlockKind::of(kind),
                BlockKind::Stream | BlockKind::CompressedStream | BlockKind::DecompressionTable
            )
        {
            return;
        }

        match BlockKind::of(kind) {
            BlockKind::Stream => self.banks.push(kind, payload.to_vec()),
            BlockKind::DecompressionTable => {
                // Not a bank: the table the compressed blocks after it decode
                // against. A malformed one is dropped rather than fatal -- the
                // banks that need it come back empty, which is silence, and
                // refusing to play the file over it would be worse.
                self.table = DecompressionTable::parse(payload).ok();
            }
            BlockKind::CompressedStream => {
                // Unpacked on arrival, so everything downstream sees one kind
                // of bank -- and stored under the *uncompressed* type, so a
                // stream bound to type 0x00 finds its data whether the file
                // compressed it or not.
                let data = decompress(payload, self.table.as_ref()).unwrap_or_default();
                self.banks.push(BlockKind::uncompressed_type(kind), data);
            }
            BlockKind::Rom => {
                if let Ok((rom, data)) = rom_header(payload) {
                    self.deliver_to_core(kind, instance, |core| {
                        core.load_rom(kind, rom.total_size, rom.start, data);
                    });
                }
            }
            BlockKind::Ram => {
                if let Ok((ram, data)) = ram_header(kind, payload) {
                    self.deliver_to_core(kind, instance, |core| core.write_ram(ram.offset, data));
                }
            }
        }
    }

    /// Performs a `0x68` PCM RAM write: `0x68 0x66 cc oo3 dd3 ss3` copies
    /// `ss` bytes from offset `oo` of the type-`cc` stream bank to the
    /// owning chip's RAM at *absolute* address `dd`.
    ///
    /// This is the other way sample RAM arrives, alongside the `0xC0`-range
    /// blocks: the corpus's Mega CD rips send one type-`0x02` bank and then
    /// thousands of these copies (Dark Wizard alone has 8,300), which is why
    /// dropping the command left those files silent. The chip byte's high
    /// bit picks the second chip, as everywhere; a zero size means
    /// `0x0100_0000` per the spec, clamped here to what the bank holds.
    fn pcm_ram_write(&mut self, stream: &VgmStream, index: usize) {
        let Some(bytes) = stream.raw_command(index) else {
            return;
        };
        if bytes.len() < 12 {
            return;
        }
        let u24 = |at: usize| -> u32 {
            u32::from(bytes[at])
                | (u32::from(bytes[at + 1]) << 8)
                | (u32::from(bytes[at + 2]) << 16)
        };
        let kind = bytes[2] & 0x7F;
        let instance = u8::from(bytes[2] & 0x80 != 0);
        let (read_offset, address) = (u24(3), u24(6));
        let size = match u24(9) {
            0 => 0x0100_0000,
            other => other,
        };
        let Some(owner) = stream_owner(kind) else {
            return;
        };
        let data = self.banks.read(kind, read_offset as usize, size as usize);
        if data.is_empty() {
            return;
        }
        if let Some(voice) = self
            .voices
            .iter_mut()
            .find(|voice| voice.target.kind == owner && voice.target.instance == instance)
        {
            voice.core.write_ram_absolute(address, &data);
        }
    }

    /// Runs `act` against the core a ROM or RAM block of type `kind` belongs to.
    ///
    /// Which chip that is comes from the spec's block-type table
    /// ([`block_owner`]). A type the table does not know falls back to the
    /// old heuristic -- the only chip that could want it: if exactly one
    /// voice is clocked, it is that one, and otherwise nothing happens
    /// rather than something wrong.
    fn deliver_to_core(&mut self, kind: u8, instance: u8, act: impl FnOnce(&mut dyn ChipCore)) {
        if let Some(owner) = block_owner(kind) {
            if let Some(voice) = self
                .voices
                .iter_mut()
                .find(|voice| voice.target.kind == owner && voice.target.instance == instance)
            {
                act(voice.core.as_mut());
            }
            // The owner is known but absent (no core, or the file is lying):
            // dropping the block beats feeding it to a chip it was never for.
            return;
        }
        let mut candidates = self
            .voices
            .iter_mut()
            .filter(|voice| voice.target.instance == instance);
        if let Some(voice) = candidates.next()
            && candidates.next().is_none()
        {
            act(voice.core.as_mut());
        }
    }

    /// Renders `out.len() / 2` frames by summing every voice.
    fn mix(&mut self, out: &mut [i16]) {
        // A file whose chips have no cores and whose streams are idle has
        // nothing to render *or* to schedule, and that is the common case while
        // the registry is empty. Silence is silence however long it lasts.
        if self.voices.is_empty() && !self.streams.any_playing() {
            out.fill(0);
            return;
        }
        for frame in out.chunks_exact_mut(2) {
            let mut left = 0i64;
            let mut right = 0i64;
            for voice in &mut self.voices {
                let [l, r] = voice.next_frame();
                left += i64::from(l);
                right += i64::from(r);
            }
            frame[0] = left.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
            frame[1] = right.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;

            // Streams write on their own clock, so they are serviced per output
            // frame rather than per command -- that is the whole point of them.
            // After the frame's render, not before: the reference's loop runs
            // `daccontrol_update` after each sample's `Resmpl_Execute`, so a
            // command falling due at sample `n` reaches the chip at `n + 1`.
            // Delivering it a frame early moves every FIFO underrun on a chip
            // whose stream races its consumption (the OKIM6258 rips), and each
            // moved slip re-seeds the ADPCM decode from there on.
            self.due.clear();
            self.streams
                .advance_frame(&mut self.due, self.huc6280_channel);
            // By index, not by `mem::take`: taking the Vec would drop its
            // allocation each frame, and this runs inside the audio callback.
            for at in 0..self.due.len() {
                let write = self.due[at];
                self.write(
                    ChipTarget {
                        kind: write.target.kind,
                        instance: write.target.instance,
                        port: write.port,
                    },
                    write.addr,
                    write.value,
                    false,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use vgms_core::vgm::ChipKind;

    /// A shared log a test core appends to, so a test can read back what
    /// arrived after the engine has taken ownership of the core.
    type Log<T> = Arc<Mutex<Vec<T>>>;

    /// A VGM declaring `chips` (each `(kind, clock)`) with `stream` as its body.
    fn vgm(chips: &[(ChipKind, u32)], stream: &[u8]) -> Arc<VgmFile> {
        fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, 0x08, 0x171);
        put_u32(&mut bytes, 0x34, 0x100 - 0x34);
        for (kind, clock) in chips {
            put_u32(&mut bytes, kind.clock_offset(), *clock);
        }
        bytes.extend_from_slice(stream);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);
        Arc::new(vgms_core::vgm::file::read("test.vgm", &bytes).expect("a walkable VGM"))
    }

    /// A dual declaration whose second instance names its own clock in the
    /// v1.70 extra header: the reference resolves instance 1 through the
    /// extra-header list (GetChipClock), and so must the engine -- without it
    /// the second chip reset at the first's clock, wrong pitch and rate.
    #[test]
    fn the_second_instance_takes_its_extra_header_clock() {
        fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut bytes = vec![0u8; 0x120];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, 0x08, 0x171);
        put_u32(&mut bytes, 0x34, 0x120 - 0x34); // data at 0x120
        // A dual OKIM6295 (bit 30), first instance at 1.056 MHz.
        put_u32(
            &mut bytes,
            ChipKind::Okim6295.clock_offset(),
            1_056_000 | (1 << 30),
        );
        // The extra header at 0x100 (pointer at 0xBC): size 8, then the
        // clock-list offset, relative to its own position (0x104).
        put_u32(&mut bytes, 0xBC, 0x100 - 0xBC);
        put_u32(&mut bytes, 0x100, 8);
        put_u32(&mut bytes, 0x104, 0x10C - 0x104);
        bytes[0x10C] = 1; // one entry: the second OKIM6295 at 2.112 MHz
        bytes[0x10D] = ChipKind::Okim6295.id();
        put_u32(&mut bytes, 0x10E, 2_112_000);
        bytes.push(0x66);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);
        let file = Arc::new(vgms_core::vgm::file::read("dual.vgm", &bytes).expect("walkable"));

        let clocks: Log<u32> = Arc::new(Mutex::new(Vec::new()));
        struct ClockTap(Log<u32>);
        impl ChipCore for ClockTap {
            fn reset(&mut self, clock: u32, _variant: bool) {
                self.0.lock().expect("not poisoned").push(clock);
            }
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn render(&mut self, out: &mut [i32]) {
                out.fill(0);
            }
        }
        let for_factory = Arc::clone(&clocks);
        let _engine = VgmEngine::with_cores(file, 44_100, move |_| {
            Some(Box::new(ClockTap(Arc::clone(&for_factory))))
        });
        assert_eq!(
            *clocks.lock().expect("not poisoned"),
            [1_056_000, 2_112_000],
            "each instance resets at its own clock"
        );
    }

    /// A core that re-derives its rate from a register write (the ES5503's
    /// oscillator-enable divides its clock by `oscillators + 2`) must have its
    /// resampler follow, or the engine keeps consuming source frames at the
    /// stale ratio -- which for a real IIgs rip meant playing ~11x too fast.
    ///
    /// Measured by counting source pulls: at 44100 -> 44100 the resampler is a
    /// passthrough (one pull per output frame); after the write drops the core
    /// to 22050 it must pull about half a frame per output frame.
    #[test]
    fn a_rate_change_after_a_write_rebuilds_the_resampler() {
        use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

        struct RateShift {
            rate: Arc<AtomicU32>,
            pulls: Arc<AtomicUsize>,
        }
        impl ChipCore for RateShift {
            fn reset(&mut self, _clock: u32, _variant: bool) {
                self.rate.store(44_100, Ordering::Relaxed);
            }
            fn native_rate(&self) -> u32 {
                self.rate.load(Ordering::Relaxed)
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {
                self.rate.store(22_050, Ordering::Relaxed);
            }
            fn render(&mut self, out: &mut [i32]) {
                self.pulls.fetch_add(out.len() / 2, Ordering::Relaxed);
                out.fill(0);
            }
        }

        let rate = Arc::new(AtomicU32::new(44_100));
        let pulls = Arc::new(AtomicUsize::new(0));
        // 4410 frames at the start rate, one write, 4410 frames at the new one.
        let file = vgm(
            &[(ChipKind::Ym2151, 3_579_545)],
            &[0x61, 0x3A, 0x11, 0x54, 0x00, 0x00, 0x61, 0x3A, 0x11, 0x66],
        );
        let mut engine = VgmEngine::with_cores(Arc::clone(&file), 44_100, |_| {
            Some(Box::new(RateShift {
                rate: Arc::clone(&rate),
                pulls: Arc::clone(&pulls),
            }))
        });
        let mut out = vec![0i16; 2000];
        while engine.render(&mut out) > 0 {}

        // ~4410 pulls for the first half, ~2205 (plus the sinc kernel's
        // priming, a few dozen) for the second. A stale ratio would pull
        // one-per-frame throughout: 8820.
        let pulled = pulls.load(Ordering::Relaxed);
        assert!(
            (6_400..7_000).contains(&pulled),
            "the resampler did not follow the core's rate: {pulled} pulls \
             (expected ~6600; a stale 1:1 ratio pulls 8820)"
        );
    }

    #[test]
    fn a_file_with_no_cores_renders_silence_and_still_ends() {
        // Two waits and an end marker: 44100 + 735 samples at 44100 Hz out.
        let file = vgm(
            &[(ChipKind::Ym2612, 7_670_454)],
            &[0x52, 0x28, 0xF0, 0x61, 0x44, 0xAC, 0x62, 0x66],
        );
        let mut engine = VgmEngine::new(file, 44_100);
        assert!(
            engine.voiced_chips().is_empty(),
            "no cores are registered yet"
        );

        let mut out = vec![1i16; 2000];
        let frames = engine.render(&mut out);
        assert_eq!(frames, 1000);
        assert!(out.iter().all(|&s| s == 0), "and every frame is silent");

        // Drain the rest: 44835 samples in total, so 43835 frames left.
        let mut total = frames;
        loop {
            let n = engine.render(&mut out);
            total += n;
            if n == 0 {
                break;
            }
        }
        assert_eq!(total, 44_835);
        assert!(engine.is_finished());
    }

    /// A mask covering every channel silences the voice in the engine itself
    /// -- on a core that ignores `set_channel_mutes` entirely, which is the
    /// case the whole-chip Mute/Solo controls exist for. A partial mask on the
    /// same core changes nothing, which is honest: that core cannot mute one
    /// channel, and pretending otherwise here would hide it.
    #[test]
    fn a_full_mask_silences_a_voice_whose_core_cannot_mute() {
        /// Renders a constant and ignores mutes, like the Nuked-OPM.
        #[derive(Debug)]
        struct Constant;
        impl ChipCore for Constant {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn render(&mut self, out: &mut [i32]) {
                out.fill(1000);
            }
        }

        let file = vgm(&[(ChipKind::Ym2151, 3_579_545)], &[0x61, 0x00, 0x10, 0x66]);
        let mut engine =
            VgmEngine::with_cores(Arc::clone(&file), 44_100, |_| Some(Box::new(Constant)));
        let mut out = vec![0i16; 512];
        engine.render(&mut out);
        assert!(out.iter().any(|&s| s != 0), "sanity: the constant sounds");

        // Every one of the YM2151's eight channels -- the whole chip.
        let mut muting = ChipMuting::new();
        muting.set(ChipKind::Ym2151, 0, 0xFF);
        let mut engine =
            VgmEngine::with_cores(Arc::clone(&file), 44_100, |_| Some(Box::new(Constant)));
        engine.set_muting(muting);
        let mut out = vec![0i16; 512];
        engine.render(&mut out);
        assert!(
            out.iter().all(|&s| s == 0),
            "a whole-chip mask must silence the voice in the engine"
        );

        // One channel only: this core cannot honour it, and the engine must
        // not silence the other seven for it.
        let mut muting = ChipMuting::new();
        muting.set(ChipKind::Ym2151, 0, 0b1);
        let mut engine = VgmEngine::with_cores(file, 44_100, |_| Some(Box::new(Constant)));
        engine.set_muting(muting);
        let mut out = vec![0i16; 512];
        engine.render(&mut out);
        assert!(
            out.iter().any(|&s| s != 0),
            "a partial mask is the core's business"
        );
    }

    #[test]
    fn a_chip_trim_attenuates_the_voice_by_its_percent() {
        /// Renders a constant, so the trim's effect on level is all that moves.
        #[derive(Debug)]
        struct Constant;
        impl ChipCore for Constant {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn render(&mut self, out: &mut [i32]) {
                out.fill(1000);
            }
        }

        let file = vgm(&[(ChipKind::Ym2151, 3_579_545)], &[0x61, 0x00, 0x10, 0x66]);
        let peak = |trim: Option<u8>| -> i32 {
            let mut engine =
                VgmEngine::with_cores(Arc::clone(&file), 44_100, |_| Some(Box::new(Constant)));
            if let Some(percent) = trim {
                let mut trims = ChipTrims::new();
                trims.set(ChipKind::Ym2151, 0, percent);
                engine.set_trims(trims);
            }
            let mut out = vec![0i16; 512];
            engine.render(&mut out);
            out.iter().map(|&s| i32::from(s).abs()).max().unwrap_or(0)
        };

        let full = peak(None);
        assert!(full > 0, "sanity: the constant sounds at full trim");

        // 50% is exactly half in 8.8 (128/256), applied linearly before the
        // resample, so the peak lands within a sample of half the full peak.
        let half = peak(Some(50));
        let ratio = f64::from(half) / f64::from(full);
        assert!(
            (0.45..=0.55).contains(&ratio),
            "a 50% trim should roughly halve the level: {half} vs {full}"
        );

        assert_eq!(peak(Some(0)), 0, "a 0% trim silences the voice");
    }

    #[test]
    fn a_short_render_zeroes_the_tail_it_did_not_fill() {
        let file = vgm(&[(ChipKind::Sn76489, 3_579_545)], &[0x62, 0x66]);
        let mut engine = VgmEngine::new(file, 44_100);
        let mut out = vec![7i16; 2000];
        let frames = engine.render(&mut out);
        assert_eq!(frames, 735);
        assert!(
            out[frames * 2..].iter().all(|&s| s == 0),
            "a reused buffer must not replay what was in it"
        );
    }

    #[test]
    fn the_zero_64_wait_override_is_ignored() {
        // `0x64 0x62 nnnn` was a withdrawn v1.70 proposal no player implements;
        // the reference players treat it as invalid. The engine ignores it, so a
        // later `0x62` still waits its fixed 735 samples, not the overridden 16.
        let file = vgm(
            &[(ChipKind::Sn76489, 3_579_545)],
            &[0x64, 0x62, 0x10, 0x00, 0x62, 0x66],
        );
        let mut engine = VgmEngine::new(file, 44_100);
        let mut out = vec![0i16; 2000];
        assert_eq!(engine.render(&mut out), 735, "the override is ignored");
    }

    #[test]
    fn rewinding_replays_from_the_start() {
        let file = vgm(&[(ChipKind::Sn76489, 3_579_545)], &[0x62, 0x66]);
        let mut engine = VgmEngine::new(file, 44_100);
        let mut out = vec![0i16; 2000];
        assert_eq!(engine.render(&mut out), 735);
        assert_eq!(engine.render(&mut out), 0);
        assert!(engine.is_finished());

        engine.rewind();
        assert!(!engine.is_finished());
        assert_eq!(engine.render(&mut out), 735);
    }

    #[test]
    fn a_write_reaches_the_chip_the_opcode_names() {
        // 0x52 is YM2612 port 0; 0x53 is port 1; 0x50 is the SN76489.
        let file = vgm(
            &[
                (ChipKind::Ym2612, 7_670_454),
                (ChipKind::Sn76489, 3_579_545),
            ],
            &[0x52, 0x28, 0xF0, 0x53, 0x2B, 0x80, 0x50, 0x9F, 0x66],
        );
        let logs: Log<(ChipKind, u8, u16, u16)> = Arc::new(Mutex::new(Vec::new()));

        struct Tap {
            kind: ChipKind,
            log: Log<(ChipKind, u8, u16, u16)>,
        }
        impl ChipCore for Tap {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, port: u8, addr: u16, data: u16) {
                self.log
                    .lock()
                    .expect("not poisoned")
                    .push((self.kind, port, addr, data));
            }
            fn render(&mut self, out: &mut [i32]) {
                out.fill(0);
            }
        }

        let logs_for_factory = Arc::clone(&logs);
        let mut engine = VgmEngine::with_cores(file, 44_100, move |kind| {
            Some(Box::new(Tap {
                kind,
                log: Arc::clone(&logs_for_factory),
            }))
        });
        let mut out = vec![0i16; 16];
        engine.render(&mut out);

        assert_eq!(
            *logs.lock().expect("not poisoned"),
            [
                (ChipKind::Ym2612, 0, 0x28, 0xF0),
                (ChipKind::Ym2612, 1, 0x2B, 0x80),
                // The SN76489 has no register address at all -- one data port,
                // and the byte is the whole write.
                (ChipKind::Sn76489, 0, 0x00, 0x9F),
            ],
            "each write went to its own chip, on its own port"
        );
    }

    /// The generic counterpart of the OPL `mask_replay` guarantee: a mute
    /// mask lands on the instance it names, and a seek -- which resets every
    /// core, clearing whatever mask it held -- restates it.
    #[test]
    fn muting_reaches_its_instance_and_survives_a_seek() {
        // Bit 30 of the clock declares a second instance.
        let file = vgm(
            &[(ChipKind::Sn76489, 3_579_545 | 0x4000_0000)],
            &[0x50, 0x9F, 0x61, 0x44, 0xAC, 0x30, 0x8E, 0x66],
        );
        // Each event is (voice number in build order, event, value).
        let log: Log<(u8, &'static str, u32)> = Arc::new(Mutex::new(Vec::new()));

        struct Tap {
            voice: u8,
            log: Log<(u8, &'static str, u32)>,
        }
        impl ChipCore for Tap {
            fn reset(&mut self, _clock: u32, _variant: bool) {
                self.log
                    .lock()
                    .expect("not poisoned")
                    .push((self.voice, "reset", 0));
            }
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn render(&mut self, out: &mut [i32]) {
                out.fill(0);
            }
            fn set_channel_mutes(&mut self, muted: u32) {
                self.log
                    .lock()
                    .expect("not poisoned")
                    .push((self.voice, "mute", muted));
            }
            fn set_channel_pans(&mut self, pans: &[i16]) {
                self.log
                    .lock()
                    .expect("not poisoned")
                    .push((self.voice, "pan", pans.len() as u32));
            }
        }

        let counter = Arc::new(Mutex::new(0u8));
        let log_for_factory = Arc::clone(&log);
        let mut engine = VgmEngine::with_cores(file, 44_100, move |_| {
            let mut counter = counter.lock().expect("not poisoned");
            let voice = *counter;
            *counter += 1;
            Some(Box::new(Tap {
                voice,
                log: Arc::clone(&log_for_factory),
            }))
        });
        assert_eq!(engine.voiced_chips().len(), 2, "a dual chip is two voices");

        let mut muting = ChipMuting::new();
        muting.set(ChipKind::Sn76489, 1, 0b1000);
        engine.set_muting(muting);
        let mut panning = ChipPanning::new();
        panning.set(ChipKind::Sn76489, 0, vec![0, 0, 0, 0]);
        engine.set_panning(panning);

        log.lock().expect("not poisoned").clear();
        engine.seek_to_row(2);

        let events = log.lock().expect("not poisoned").clone();
        let after_resets: Vec<_> = events
            .iter()
            .skip_while(|(_, event, _)| *event != "reset")
            .filter(|(_, event, _)| *event != "reset")
            .copied()
            .collect();
        assert!(
            after_resets.contains(&(0, "mute", 0)),
            "instance 1's voice is restated as unmuted: {events:?}"
        );
        assert!(
            after_resets.contains(&(1, "mute", 0b1000)),
            "instance 2's mask came back after the reset: {events:?}"
        );
        assert!(
            after_resets.contains(&(0, "pan", 4)),
            "instance 1's pans came back after the reset: {events:?}"
        );
        assert!(
            !after_resets.contains(&(1, "pan", 4)),
            "instance 2 has no pan image set: {events:?}"
        );
    }

    /// A dro2vgm dual-OPL2 (clock bits 30+31) hard-pans its two YM3812s: chip 1
    /// left, chip 2 right, the SB Pro image an OPL2 cannot make itself, with each
    /// surviving side doubled to undo the dual-declaration balance halving. The
    /// same two chips with only the dual bit (a genuine mono arcade twin) stay
    /// centred.
    #[test]
    fn dual_opl2_bit31_hard_pans_the_two_instances() {
        // Instance 0 renders a constant of 1000 on both channels; instance 1
        // renders 2000 -- distinct so each side can be traced to one chip.
        struct Const(i32);
        impl ChipCore for Const {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn render(&mut self, out: &mut [i32]) {
                out.fill(self.0);
            }
        }

        // A steady-state frame (past any resampler warmup) of a one-second render.
        let steady = |clock: u32| -> (i64, i64) {
            let file = vgm(&[(ChipKind::Ym3812, clock)], &[0x61, 0x44, 0xAC, 0x66]);
            let consts = [1000i32, 2000i32];
            let counter = Arc::new(Mutex::new(0usize));
            let mut engine = VgmEngine::with_cores(file, 44_100, move |_| {
                let mut at = counter.lock().expect("not poisoned");
                let value = consts[*at];
                *at += 1;
                Some(Box::new(Const(value)) as Box<dyn ChipCore>)
            });
            assert_eq!(engine.voiced_chips().len(), 2, "a dual chip is two voices");
            let mut out = vec![0i16; 400];
            engine.render(&mut out);
            (i64::from(out[200]), i64::from(out[201])) // frame 100
        };

        const DUAL: u32 = 3_579_545 | 0x4000_0000; // bit 30 only -- mono twin
        const DUAL_STEREO: u32 = 3_579_545 | 0xC000_0000; // bits 30 + 31 -- SB Pro

        let (mono_l, mono_r) = steady(DUAL);
        assert!(mono_l > 0, "the mono twin is audible");
        assert_eq!(mono_l, mono_r, "without bit 31 the two chips stay centred");

        let (left, right) = steady(DUAL_STEREO);
        assert_ne!(left, right, "bit 31 splits the two chips across the image");
        assert!(left > 0 && right > 0, "both sides carry a chip");
        // Left is instance 0 (the 1000 chip), right is instance 1 (the 2000 chip);
        // the ratio is theirs, proving each side carries exactly one chip (a bleed
        // would pull the ratio toward 1). Small tolerance for the 8.8 truncation.
        assert!(
            (right - 2 * left).abs() <= 2,
            "right/left tracks 2000/1000: {left} {right}"
        );
        // The doubled surviving side restores full level: the stereo pair sums to
        // twice the centred per-side level, not the halved one.
        assert!(
            (left + right - 2 * mono_l).abs() <= 2,
            "doubling restores the pre-halving level: {left}+{right} vs 2*{mono_l}"
        );
    }

    #[test]
    fn a_mixed_render_does_not_depend_on_the_pull_size() {
        // A core that renders a rising ramp, so a difference in chunking shows.
        #[derive(Default)]
        struct Ramp(i32);
        impl ChipCore for Ramp {
            fn reset(&mut self, _clock: u32, _variant: bool) {
                self.0 = 0;
            }
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn render(&mut self, out: &mut [i32]) {
                for frame in out.chunks_exact_mut(2) {
                    self.0 = (self.0 + 37) % 3000;
                    frame[0] = self.0;
                    frame[1] = -self.0;
                }
            }
        }

        let stream = &[0x50, 0x9F, 0x61, 0x44, 0xAC, 0x66];
        let file = vgm(&[(ChipKind::Sn76489, 3_579_545)], stream);
        let build = || {
            VgmEngine::with_cores(Arc::clone(&file), 44_100, |_| {
                Some(Box::new(Ramp::default()))
            })
        };

        let mut whole = build();
        let mut all = vec![0i16; 44_100 * 2];
        assert_eq!(whole.render(&mut all), 44_100);

        let mut chunked = build();
        let mut pieced = Vec::new();
        loop {
            let mut buffer = vec![0i16; 128 * 2];
            let frames = chunked.render(&mut buffer);
            if frames == 0 {
                break;
            }
            pieced.extend_from_slice(&buffer[..frames * 2]);
        }
        assert_eq!(pieced, all, "128 frames at a time must sound like 44100");
    }

    #[test]
    fn a_rom_block_reaches_the_only_chip_that_could_want_it() {
        // `0x67 0x66 0x80 ssssssss {total, start, data}`.
        let mut payload = Vec::new();
        payload.extend_from_slice(&0x1000u32.to_le_bytes());
        payload.extend_from_slice(&0x40u32.to_le_bytes());
        payload.extend_from_slice(&[7, 7, 7]);
        let mut stream = vec![0x67, 0x66, 0x80];
        stream.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        stream.extend_from_slice(&payload);
        stream.push(0x66);

        let file = vgm(&[(ChipKind::SegaPcm, 4_000_000)], &stream);
        let seen: Log<(u8, u32, u32, usize)> = Arc::new(Mutex::new(Vec::new()));

        struct RomTap(Log<(u8, u32, u32, usize)>);
        impl ChipCore for RomTap {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn load_rom(&mut self, block_type: u8, total_size: u32, start: u32, data: &[u8]) {
                self.0.lock().expect("not poisoned").push((
                    block_type,
                    total_size,
                    start,
                    data.len(),
                ));
            }
            fn render(&mut self, out: &mut [i32]) {
                out.fill(0);
            }
        }

        let seen_for_factory = Arc::clone(&seen);
        let mut engine = VgmEngine::with_cores(file, 44_100, move |_| {
            Some(Box::new(RomTap(Arc::clone(&seen_for_factory))))
        });
        let mut out = vec![0i16; 16];
        engine.render(&mut out);

        assert_eq!(
            *seen.lock().expect("not poisoned"),
            [(0x80, 0x1000, 0x40, 3)],
            "the header was split off and the piece handed over"
        );
    }

    /// A `0x68` PCM RAM write copies a slice of the stream bank into the
    /// owning chip's RAM at an absolute address -- the Mega CD upload path.
    #[test]
    fn a_pcm_ram_write_copies_the_bank_into_the_chip() {
        // A type-0x02 (RF5C164) stream bank of ten counting bytes...
        let mut stream = vec![0x67, 0x66, 0x02];
        stream.extend_from_slice(&10u32.to_le_bytes());
        stream.extend_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        // ...then `0x68 0x66 cc oo3 dd3 ss3`: four bytes from offset 2 of
        // that bank to chip address 0x1234.
        stream.extend_from_slice(&[0x68, 0x66, 0x02, 2, 0, 0, 0x34, 0x12, 0, 4, 0, 0]);
        stream.push(0x66);

        let file = vgm(&[(ChipKind::Rf5c164, 12_500_000)], &stream);
        let seen: Log<(u32, Vec<u8>)> = Arc::new(Mutex::new(Vec::new()));

        struct RamTap(Log<(u32, Vec<u8>)>);
        impl ChipCore for RamTap {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn write_ram_absolute(&mut self, address: u32, data: &[u8]) {
                self.0
                    .lock()
                    .expect("not poisoned")
                    .push((address, data.to_vec()));
            }
            fn render(&mut self, out: &mut [i32]) {
                out.fill(0);
            }
        }

        let seen_for_factory = Arc::clone(&seen);
        let mut engine = VgmEngine::with_cores(file, 44_100, move |_| {
            Some(Box::new(RamTap(Arc::clone(&seen_for_factory))))
        });
        let mut out = vec![0i16; 16];
        engine.render(&mut out);

        assert_eq!(
            *seen.lock().expect("not poisoned"),
            [(0x1234, vec![2, 3, 4, 5])],
            "the copy read the bank at its offset and named the absolute address"
        );
    }

    /// A stream set up, bound, clocked and started reaches its chip's register
    /// on its own clock -- not the command stream's.
    /// A compressed bank arrives unpacked, and under the type a stream binds
    /// to -- so a file that compressed its samples is indistinguishable
    /// downstream from one that did not.
    /// The whole path, through the ambient registry: a real file, a core the
    /// registry built, and audio out the other end. The core is the test
    /// stub -- this crate ships none of its own -- so what this proves is the
    /// engine's routing and mixing; the same walk with real cores lives
    /// downstream where the providers are linked.
    #[test]
    fn a_sound_chip_this_app_has_a_core_for_actually_makes_a_sound() {
        crate::testing::install_registry_with_stub();
        // `0x50 nn` writes a byte to the SN76489. Set tone 0 to period 254 and
        // turn it up, then let it play for a second.
        let stream = &[
            0x50, 0x8E, // latch tone 0, low nibble of 254
            0x50, 0x0F, // high six bits
            0x50, 0x90, // tone 0 at full volume
            0x61, 0x44, 0xAC, // wait a second
            0x66,
        ];
        let file = vgm(&[(ChipKind::Sn76489, 3_579_545)], stream);
        let mut engine = VgmEngine::new(file, 44_100);
        assert_eq!(
            engine.voiced_chips().len(),
            1,
            "the registry has a core for this one"
        );

        let mut out = vec![0i16; 44_100 * 2];
        assert_eq!(engine.render(&mut out), 44_100);

        let peak = out.iter().copied().map(i16::abs).max().unwrap_or(0);
        assert!(peak > 1000, "audible, not silence: peak {peak}");
        // A square wave spends its time at its extremes, so the mean of the
        // absolute values sits near the peak rather than near zero.
        let mean = out.iter().map(|&s| i64::from(s.abs())).sum::<i64>() / out.len() as i64;
        assert!(
            mean > i64::from(peak) / 2,
            "a square wave, not a click: mean {mean} against peak {peak}"
        );
    }

    #[test]
    fn a_seek_restores_the_state_rather_than_replaying_the_stream() {
        // Four writes to the same register, parted by waits, then one to
        // another. Seeking past all of them should send the *last* value of
        // each register, not all five writes.
        let mut stream = Vec::new();
        for value in [0x01, 0x02, 0x03, 0x04] {
            stream.extend_from_slice(&[0x52, 0x30, value]);
            stream.extend_from_slice(&[0x61, 0x44, 0xAC]);
        }
        stream.extend_from_slice(&[0x52, 0x40, 0x99]);
        stream.extend_from_slice(&[0x61, 0x44, 0xAC]);
        stream.push(0x66);

        let file = vgm(&[(ChipKind::Ym2612, 7_670_454)], &stream);
        let seen: Log<(u8, u16, u16)> = Arc::new(Mutex::new(Vec::new()));

        struct Tap(Log<(u8, u16, u16)>);
        impl ChipCore for Tap {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, port: u8, addr: u16, data: u16) {
                self.0
                    .lock()
                    .expect("not poisoned")
                    .push((port, addr, data));
            }
            fn render(&mut self, out: &mut [i32]) {
                out.fill(0);
            }
        }

        let seen_for_factory = Arc::clone(&seen);
        let mut engine = VgmEngine::with_cores(Arc::clone(&file), 44_100, move |_| {
            Some(Box::new(Tap(Arc::clone(&seen_for_factory))))
        });

        // Row 9 is the last wait, past every write.
        engine.seek_to_row(9);
        assert_eq!(
            *seen.lock().expect("not poisoned"),
            [(0, 0x30, 0x04), (0, 0x40, 0x99)],
            "one write per register, at its last value"
        );
        assert_eq!(engine.position().next_instruction, 9);

        // And playing on from there covers only what is left.
        let mut out = vec![0i16; 44_100 * 4];
        assert_eq!(engine.render(&mut out), 44_100, "the last second");
        assert!(engine.is_finished());
    }

    #[test]
    fn the_position_counts_frames_and_names_the_next_row() {
        let file = vgm(
            &[(ChipKind::Sn76489, 3_579_545)],
            &[0x50, 0x9F, 0x61, 0x44, 0xAC, 0x50, 0x8E, 0x62, 0x66],
        );
        let mut engine = VgmEngine::new(Arc::clone(&file), 44_100);
        assert_eq!(engine.position().frames_rendered, 0);

        // Half a second in: the second-long wait is still being served.
        let mut out = vec![0i16; 22_050 * 2];
        assert_eq!(engine.render(&mut out), 22_050);
        assert_eq!(engine.position().frames_rendered, 22_050);
        assert_eq!(engine.position().next_instruction, 2);

        // A seek restates the position from the start of the song, not from the
        // seek -- row 3 is one second in.
        engine.seek_to_row(3);
        assert_eq!(engine.position().frames_rendered, 44_100);
        assert_eq!(engine.position().next_instruction, 3);
    }

    #[test]
    fn seeking_by_time_lands_on_the_row_playing_then() {
        // Three one-second waits, each after a write.
        let mut stream = Vec::new();
        for value in [0x9F, 0x8E, 0x80] {
            stream.extend_from_slice(&[0x50, value, 0x61, 0x44, 0xAC]);
        }
        stream.push(0x66);
        let file = vgm(&[(ChipKind::Sn76489, 3_579_545)], &stream);
        let mut engine = VgmEngine::new(file, 44_100);

        engine.seek_to_ms(0);
        assert_eq!(engine.position().next_instruction, 0);
        // A second in, the second write has not happened yet.
        engine.seek_to_ms(1000);
        assert_eq!(engine.position().next_instruction, 2);
        engine.seek_to_ms(2000);
        assert_eq!(engine.position().next_instruction, 4);
        // Past the end lands at the end.
        engine.seek_to_ms(99_000);
        assert_eq!(engine.position().next_instruction, 6);
        assert!(engine.is_finished());
    }

    /// Three one-second waits, each after a write: rows 0..6, six seconds of
    /// nothing much, and easy arithmetic.
    fn three_second_vgm() -> Arc<VgmFile> {
        let mut stream = Vec::new();
        for value in [0x9F, 0x8E, 0x80] {
            stream.extend_from_slice(&[0x50, value, 0x61, 0x44, 0xAC]);
        }
        stream.push(0x66);
        vgm(&[(ChipKind::Sn76489, 3_579_545)], &stream)
    }

    /// Renders until the engine stops, in chunks, and returns the frame total.
    fn drain(engine: &mut VgmEngine, cap: usize) -> usize {
        let mut out = vec![0i16; 4096 * 2];
        let mut total = 0;
        loop {
            let frames = engine.render(&mut out);
            if frames == 0 || total > cap {
                return total;
            }
            total += frames;
        }
    }

    #[test]
    fn a_loop_plays_its_region_again_and_stops_when_the_count_runs_out() {
        let file = three_second_vgm();
        let mut engine = VgmEngine::new(Arc::clone(&file), 44_100);
        // Rows 2..4 is the second write and its wait: one second. Played *twice
        // in total* -- forward playback already plays it once -- so the file
        // gains one second, not two.
        engine.set_loop(Some(LoopConfig::for_vgm(
            &file,
            2,
            4,
            crate::clock::LoopCount::Times(2),
            44_100,
        )));

        let frames = drain(&mut engine, 44_100 * 30);
        assert_eq!(frames, 44_100 * 4, "its own three seconds plus one more");
        assert!(engine.is_finished());
    }

    #[test]
    fn the_position_rewinds_with_the_audio_at_the_seam() {
        let file = three_second_vgm();
        let mut engine = VgmEngine::new(Arc::clone(&file), 44_100);
        engine.set_loop(Some(LoopConfig::for_vgm(
            &file,
            2,
            4,
            crate::clock::LoopCount::Times(2),
            44_100,
        )));

        // Two seconds and one frame in: the region ended at two seconds, and the
        // pull that needed a frame past it is the one that wrapped -- the same
        // "you learn by asking" rule that tells a caller a stream has finished.
        let mut out = vec![0i16; (44_100 * 2 + 1) * 2];
        assert_eq!(engine.render(&mut out), 44_100 * 2 + 1);
        let position = engine.position();
        assert_eq!(
            position.frames_rendered,
            44_100 + 1,
            "back to the loop start, plus the frame that was asked for"
        );
        assert_eq!(position.loop_iteration, 1);
    }

    #[test]
    fn a_loop_to_the_end_of_the_stream_never_finishes() {
        let file = three_second_vgm();
        let mut engine = VgmEngine::new(Arc::clone(&file), 44_100);
        engine.set_loop(Some(LoopConfig::for_vgm(
            &file,
            0,
            file.len(),
            crate::clock::LoopCount::Infinite,
            44_100,
        )));

        // Ten seconds of a three-second file, and it is still going.
        let mut out = vec![0i16; 44_100 * 10 * 2];
        assert_eq!(engine.render(&mut out), 44_100 * 10);
        assert!(!engine.is_finished(), "forever means forever");
        assert!(engine.position().loop_iteration >= 3);
    }

    #[test]
    fn a_region_that_renders_no_audio_is_dropped_rather_than_spun_on() {
        let file = three_second_vgm();
        let mut engine = VgmEngine::new(Arc::clone(&file), 44_100);
        // Rows 0..1 is one write and no wait at all.
        engine.set_loop(Some(LoopConfig::for_vgm(
            &file,
            0,
            1,
            crate::clock::LoopCount::Infinite,
            44_100,
        )));
        let frames = drain(&mut engine, 44_100 * 30);
        assert_eq!(frames, 44_100 * 3, "it played on instead of hanging");
        assert!(engine.loop_config().is_none(), "and the loop was dropped");
    }

    #[test]
    fn a_region_outside_the_stream_is_refused() {
        let file = three_second_vgm();
        let mut engine = VgmEngine::new(Arc::clone(&file), 44_100);
        for (start, end) in [(4, 4), (5, 4), (0, file.len() + 1)] {
            engine.set_loop(Some(LoopConfig::for_vgm(
                &file,
                start,
                end,
                crate::clock::LoopCount::Infinite,
                44_100,
            )));
            assert!(engine.loop_config().is_none(), "accepted {start}..{end}");
        }
    }

    #[test]
    fn a_seek_to_the_start_is_a_rewind() {
        let file = vgm(
            &[(ChipKind::Sn76489, 3_579_545)],
            &[0x50, 0x9F, 0x61, 0x44, 0xAC, 0x66],
        );
        let mut engine = VgmEngine::new(Arc::clone(&file), 44_100);
        let mut out = vec![0i16; 44_100 * 2];
        assert_eq!(engine.render(&mut out), 44_100);

        engine.seek_to_row(0);
        assert_eq!(engine.position().next_instruction, 0);
        assert!(!engine.is_finished());
        assert_eq!(engine.render(&mut out), 44_100);
    }

    #[test]
    fn a_compressed_bank_is_unpacked_on_arrival() {
        /// A `0x67` block of type `kind` carrying `payload`.
        fn block(kind: u8, payload: &[u8]) -> Vec<u8> {
            let mut bytes = vec![0x67, 0x66, kind];
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(payload);
            bytes
        }

        // Type 0x40 is a compressed 0x00: four 4-bit values shifted up to 8
        // bits, so 1, 2, 15 -> 0x10, 0x20, 0xF0.
        let mut sub_header = vec![0x00];
        sub_header.extend_from_slice(&3u32.to_le_bytes()); // uncompressed size
        sub_header.extend_from_slice(&[8, 4, 1]); // bits out, bits in, shift-left
        sub_header.extend_from_slice(&0u16.to_le_bytes()); // add value
        sub_header.extend_from_slice(&[0x12, 0xF0]); // 1, 2, 15, and a pad nibble

        let mut stream = block(0x40, &sub_header);
        stream.push(0x66);

        let file = vgm(&[(ChipKind::Ym2612, 7_670_454)], &stream);
        let mut engine = VgmEngine::new(file, 44_100);
        let mut out = vec![0i16; 16];
        engine.render(&mut out);

        assert_eq!(
            engine.banks.nth(0x00, 0),
            Some([0x10, 0x20, 0xF0].as_slice()),
            "unpacked, and filed under the uncompressed type"
        );
    }

    #[test]
    fn a_dac_stream_writes_its_bank_to_the_chip_it_was_pointed_at() {
        // The bank: four bytes of "PCM".
        let mut stream = vec![0x67, 0x66, 0x00];
        stream.extend_from_slice(&4u32.to_le_bytes());
        stream.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        // 0x90: stream 0 -> chip 0x02 (YM2612), port 0, register 0x2A.
        stream.extend_from_slice(&[0x90, 0x00, 0x02, 0x00, 0x2A]);
        // 0x91: bind to bank type 0x00, step 1, base 0.
        stream.extend_from_slice(&[0x91, 0x00, 0x00, 0x01, 0x00]);
        // 0x92: 11025 Hz -- one byte every four output frames at 44100.
        stream.push(0x92);
        stream.push(0x00);
        stream.extend_from_slice(&11_025u32.to_le_bytes());
        // 0x93: start at 0, length mode 3 (play to the end), length ignored.
        stream.push(0x93);
        stream.push(0x00);
        stream.extend_from_slice(&0u32.to_le_bytes());
        stream.push(0x03);
        stream.extend_from_slice(&0u32.to_le_bytes());
        // Then a second of silence for it to play into, and the end.
        stream.extend_from_slice(&[0x61, 0x44, 0xAC, 0x66]);

        let file = vgm(&[(ChipKind::Ym2612, 7_670_454)], &stream);
        let seen: Log<(u8, u16, u16)> = Arc::new(Mutex::new(Vec::new()));

        struct Tap(Log<(u8, u16, u16)>);
        impl ChipCore for Tap {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, port: u8, addr: u16, data: u16) {
                self.0
                    .lock()
                    .expect("not poisoned")
                    .push((port, addr, data));
            }
            fn render(&mut self, out: &mut [i32]) {
                out.fill(0);
            }
        }

        let seen_for_factory = Arc::clone(&seen);
        let mut engine = VgmEngine::with_cores(file, 44_100, move |_| {
            Some(Box::new(Tap(Arc::clone(&seen_for_factory))))
        });
        let mut out = vec![0i16; 44_100 * 2];
        engine.render(&mut out);

        assert_eq!(
            *seen.lock().expect("not poisoned"),
            [
                (0, 0x2A, 0x11),
                (0, 0x2A, 0x22),
                (0, 0x2A, 0x33),
                (0, 0x2A, 0x44),
            ],
            "the whole bank, one byte at a time, to the DAC register"
        );
    }

    /// The `0x8n` fast path: each command plays the next byte of the PCM bank
    /// into the YM2612's DAC port and waits its low nibble. `0xE0` moves the
    /// cursor, and reading past the bank stops writing rather than wrapping.
    #[test]
    fn dac_fast_path_commands_play_the_bank_through_the_dac_port() {
        // A four-byte type-0x00 bank.
        let mut stream = vec![0x67, 0x66, 0x00];
        stream.extend_from_slice(&4u32.to_le_bytes());
        stream.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        // Two DAC writes from the start of the bank...
        stream.extend_from_slice(&[0x80, 0x83]);
        // ...a seek to byte 3, one more, and one past the end.
        stream.extend_from_slice(&[0xE0, 0x03, 0x00, 0x00, 0x00]);
        stream.extend_from_slice(&[0x85, 0x82]);
        stream.push(0x66);

        let file = vgm(&[(ChipKind::Ym2612, 7_670_454)], &stream);
        let seen: Log<(u8, u16, u16)> = Arc::new(Mutex::new(Vec::new()));

        struct Tap(Log<(u8, u16, u16)>);
        impl ChipCore for Tap {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, port: u8, addr: u16, data: u16) {
                self.0
                    .lock()
                    .expect("not poisoned")
                    .push((port, addr, data));
            }
            fn render(&mut self, out: &mut [i32]) {
                out.fill(0);
            }
        }

        let seen_for_factory = Arc::clone(&seen);
        let mut engine = VgmEngine::with_cores(file, 44_100, move |_| {
            Some(Box::new(Tap(Arc::clone(&seen_for_factory))))
        });
        let mut out = vec![0i16; 64];
        engine.render(&mut out);

        assert_eq!(
            *seen.lock().expect("not poisoned"),
            [
                (0, 0x2A, 0x11),
                (0, 0x2A, 0x22),
                (0, 0x2A, 0x44), // after the seek to byte 3
                                 // the read past the end wrote nothing
            ],
            "bank bytes reached the DAC port, following the seek"
        );

        // A rewind starts the bank over from byte 0.
        engine.rewind();
        seen.lock().expect("not poisoned").clear();
        engine.render(&mut out);
        assert_eq!(
            seen.lock().expect("not poisoned")[0],
            (0, 0x2A, 0x11),
            "rewinding reset the PCM cursor"
        );
    }

    #[test]
    fn a_data_block_is_kept_as_a_bank() {
        // `0x67 0x66 tt ssssssss` with a four-byte payload.
        let mut stream = vec![0x67, 0x66, 0x00];
        stream.extend_from_slice(&4u32.to_le_bytes());
        stream.extend_from_slice(&[1, 2, 3, 4]);
        stream.push(0x66);

        let file = vgm(&[(ChipKind::Ym2612, 7_670_454)], &stream);
        let mut engine = VgmEngine::new(file, 44_100);
        let mut out = vec![0i16; 16];
        engine.render(&mut out);

        assert_eq!(engine.banks.nth(0x00, 0), Some([1, 2, 3, 4].as_slice()));
    }
}
