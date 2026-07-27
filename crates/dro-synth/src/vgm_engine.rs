//! Playing a VGM for whatever chips it declares.
//!
//! [`PlayerEngine`](crate::engine::PlayerEngine) plays the OPL path and keeps
//! its OPL policy -- muting, panning, the buffered-write spacing Nuked needs.
//! This engine knows no chip at all. It walks the command stream, hands each
//! write to whichever [`ChipCore`] the header says owns it, counts out the
//! waits, and mixes what the cores render. Everything chip-specific is behind
//! the trait.
//!
//! Cores render at their own rates, so each is resampled to the output rate on
//! the way into the mix. The pull contract is
//! [`PlayerEngine`](crate::engine::PlayerEngine)'s exactly -- `render(&mut
//! [i16]) -> usize` -- so the native audio thread, the WAV renderer and the
//! waveform renderer drive either without knowing which they have.
//!
//! Until mc-8 registers a core, [`core_for`] returns `None` for everything and
//! this engine renders silence. That is not a placeholder: routing, banks and
//! timing are all testable against [`RecordingChip`](crate::chip::RecordingChip)
//! without an emulator in sight, and a core that arrives later inherits a engine
//! already proven to feed it correctly.

use std::sync::Arc;

use dro_core::VgmFile;
use dro_core::vgm::header::ChipUse;
use dro_core::vgm::stream::{ChipTarget, VgmCommand, VgmStream};

use crate::banks::{Banks, BlockKind, ram_header, rom_header};
use crate::chip::{ChipCore, core_for};
use crate::dac_stream::{DacStreams, PendingWrite};
use crate::decompress::{DecompressionTable, decompress};
use crate::engine::{FrameClock, LoopConfig, Position};
use crate::resample::Resampler;

/// One chip instance, with the resampler that brings it to the output rate.
struct Voice {
    target: ChipTarget,
    core: Box<dyn ChipCore>,
    /// Band-limited rate conversion from the chip's rate to the engine's.
    ///
    /// This used to be a linear interpolation between the two source frames
    /// straddling each output frame, which is a fair approximation at a ratio
    /// near 1:1 and nothing of the kind at 5:1 -- the SN76489 renders at
    /// 223721 Hz, and everything it puts above 22 kHz was folding straight back
    /// into the audible band. See [`crate::resample`].
    resampler: Resampler,
}

impl Voice {
    fn new(
        target: ChipTarget,
        mut core: Box<dyn ChipCore>,
        chip: &ChipUse,
        settings: &dro_core::vgm::ChipSettings,
        output_rate: u32,
    ) -> Self {
        core.reset(chip.clock, chip.variant);
        // After the reset, which is what clears the state this configures.
        core.configure(settings);
        let native = core.native_rate().max(1);
        Self {
            target,
            core,
            resampler: Resampler::new(native, output_rate),
        }
    }

    /// Whether this voice takes `target`'s writes: the chip, and which of its
    /// (up to two) instances. The port is the chip's own business.
    fn accepts(&self, target: ChipTarget) -> bool {
        self.target.kind == target.kind && self.target.instance == target.instance
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
        self.resampler.next_frame(|| {
            let mut frame = [0i32; 2];
            core.render(&mut frame);
            frame
        })
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
    /// What `0x62` and `0x63` wait for, which `0x64` can override.
    wait_60hz: u32,
    wait_50hz: u32,
    output_rate: u32,
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

/// `0x62`'s wait: one 60 Hz frame.
const WAIT_60HZ: u32 = 735;
/// `0x63`'s wait: one 50 Hz frame.
const WAIT_50HZ: u32 = 882;

impl VgmEngine {
    /// Builds an engine for `file`, rendering at `output_rate` Hz.
    ///
    /// Every chip the header clocks gets a core if one is registered, and is
    /// skipped if not -- so a file with one known chip and one unknown plays the
    /// known one and leaves the other silent. [`playability`](crate::chip::playability)
    /// is how a caller finds that out before committing to it.
    #[must_use]
    pub fn new(file: Arc<VgmFile>, output_rate: u32) -> Self {
        Self::with_cores(file, output_rate, core_for)
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
        factory: impl Fn(dro_core::vgm::ChipKind) -> Option<Box<dyn ChipCore>>,
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
                    voices.push(Voice::new(
                        target,
                        core,
                        chip,
                        file.header.settings(),
                        output_rate,
                    ));
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
            clock: FrameClock::new(output_rate, dro_core::vgm::VGM_SAMPLE_RATE),
            index: 0,
            pending: 0,
            finished: false,
            frames_rendered: 0,
            loop_config: None,
            wraps_remaining: None,
            loops_done: 0,
            wait_60hz: WAIT_60HZ,
            wait_50hz: WAIT_50HZ,
            output_rate: output_rate.max(1),
        }
    }

    /// The chips this engine actually has cores for.
    #[must_use]
    pub fn voiced_chips(&self) -> Vec<ChipTarget> {
        self.voices.iter().map(|voice| voice.target).collect()
    }

    /// Whether the stream has been played to its end.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// Restarts from the first command with every chip reset.
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
        self.clock.reset();
        self.index = 0;
        self.pending = 0;
        self.finished = false;
        self.frames_rendered = 0;
        self.restart_loop_count();
        self.wait_60hz = WAIT_60HZ;
        self.wait_50hz = WAIT_50HZ;
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
    /// The fold is [`dro_core::chip_state`], the same one the crop edit and the
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

        let state = dro_core::chip_state::ChipState::fold(stream, index);
        for restore in state.restore_indices() {
            if let Some(command) = stream.get(restore) {
                // The return is a wait length, and a restore never waits: the
                // fold keeps writes and blocks, not the time between them.
                self.execute(stream, restore, command);
            }
        }

        self.index = index;
        self.finished = index >= stream.len();
        // The clock's carried remainder belongs to the time that was skipped.
        self.clock.reset();
        self.restart_loop_count();
        // The position readout counts from the start of the song, not from the
        // seek, so it restates where the seek landed rather than resetting.
        let samples = stream.total_samples() - stream.samples_from(index);
        self.frames_rendered =
            samples * u64::from(self.output_rate) / u64::from(dro_core::vgm::VGM_SAMPLE_RATE);
    }

    /// Jumps to the row playing at `ms`, for a transport that seeks by time.
    pub fn seek_to_ms(&mut self, ms: u32) {
        let Some(stream) = self.file.stream() else {
            return;
        };
        let target = u64::from(ms) * u64::from(dro_core::vgm::VGM_SAMPLE_RATE) / 1000;
        let mut elapsed = 0u64;
        let mut row = stream.len();
        for index in 0..stream.len() {
            if elapsed >= target {
                row = index;
                break;
            }
            elapsed += u64::from(stream.wait_samples(index));
        }
        self.seek_to_row(row);
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
            let samples = self.execute(stream, index, command);
            if samples > 0 {
                self.pending = self.clock.frames_for(samples);
            }
        }
        if self.index >= stream.len() && !self.owes_a_wrap() {
            self.finished = true;
        }
    }

    /// Performs one command, returning how many VGM samples it waits for.
    fn execute(&mut self, stream: &VgmStream, index: usize, command: VgmCommand) -> u32 {
        match command {
            VgmCommand::Write { target, addr, data } => {
                self.write(target, addr, data);
                0
            }
            VgmCommand::Wait(samples) => match samples {
                // The two fixed waits are the ones `0x64` can redefine; every
                // other wait carries its own length.
                WAIT_60HZ => self.wait_60hz,
                WAIT_50HZ => self.wait_50hz,
                other => other,
            },
            VgmCommand::DacWrite { wait } => {
                // The YM2612 DAC fast path: play the next byte of the PCM bank,
                // then wait. mc-6's DAC-stream work fills in the byte; the wait
                // is timing, and timing is this loop's job either way.
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
            VgmCommand::PcmRamWrite { .. } | VgmCommand::SeekPcm(_) | VgmCommand::Raw { .. } => 0,
            VgmCommand::OverrideWait { which, samples } => {
                // `0x64 62|63 nnnn` redefines what the short waits mean.
                match which {
                    0x62 => self.wait_60hz = u32::from(samples),
                    0x63 => self.wait_50hz = u32::from(samples),
                    _ => {}
                }
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
                // The fast form: play block `n` of the bound type from its
                // start to its end. Bit 0 of the flags loops it.
                let bank_type = self.streams.bank_type(id);
                let block = usize::from(u16_at(0));
                let data = self
                    .banks
                    .nth(bank_type, block)
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default();
                let loops = byte(2) & 0x01 != 0;
                self.streams
                    .start(id, &data, 0, if loops { 0x80 } else { 0x00 }, 0);
            }
            _ => {}
        }
    }

    /// Routes a register write to the core that owns it.
    fn write(&mut self, target: ChipTarget, addr: u16, data: u16) {
        for voice in &mut self.voices {
            if voice.accepts(target) {
                voice.core.write(target.port, addr, data);
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

    /// Runs `act` against the core a ROM or RAM block of type `kind` belongs to.
    ///
    /// Which chip that is comes from the spec's block-type table. Until that
    /// table is filled in (it arrives with the cores that need it, in mc-8/mc-9),
    /// a block goes to the only chip that could want it: if exactly one voice is
    /// clocked, it is that one, and otherwise nothing happens rather than
    /// something wrong.
    fn deliver_to_core(&mut self, _kind: u8, instance: u8, act: impl FnOnce(&mut dyn ChipCore)) {
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
            // Streams write on their own clock, so they are serviced per output
            // frame rather than per command -- that is the whole point of them.
            self.due.clear();
            self.streams.advance_frame(&mut self.due);
            for write in std::mem::take(&mut self.due) {
                self.write(
                    ChipTarget {
                        kind: write.target.kind,
                        instance: write.target.instance,
                        port: write.target.port,
                    },
                    u16::from(write.target.register),
                    u16::from(write.value),
                );
            }

            let mut left = 0i64;
            let mut right = 0i64;
            for voice in &mut self.voices {
                let [l, r] = voice.next_frame();
                left += i64::from(l);
                right += i64::from(r);
            }
            frame[0] = left.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
            frame[1] = right.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use dro_core::vgm::ChipKind;

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
        Arc::new(dro_core::vgm::file::read("test.vgm", &bytes).expect("a walkable VGM"))
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
    fn the_short_waits_can_be_redefined() {
        // `0x64 0x62 nnnn` redefines what a `0x62` waits for.
        let file = vgm(
            &[(ChipKind::Sn76489, 3_579_545)],
            &[0x64, 0x62, 0x10, 0x00, 0x62, 0x66],
        );
        let mut engine = VgmEngine::new(file, 44_100);
        let mut out = vec![0i16; 2000];
        assert_eq!(engine.render(&mut out), 16, "not 735");
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

    /// A stream set up, bound, clocked and started reaches its chip's register
    /// on its own clock -- not the command stream's.
    /// A compressed bank arrives unpacked, and under the type a stream binds
    /// to -- so a file that compressed its samples is indistinguishable
    /// downstream from one that did not.
    /// The whole path, through the registry the app uses: a real file, a real
    /// core, and audio out the other end.
    #[test]
    fn a_sound_chip_this_app_has_a_core_for_actually_makes_a_sound() {
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
            crate::engine::LoopCount::Times(2),
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
            crate::engine::LoopCount::Times(2),
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
            crate::engine::LoopCount::Infinite,
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
            crate::engine::LoopCount::Infinite,
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
                crate::engine::LoopCount::Infinite,
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
        // 0x93: start at 0, length mode 0 (to the end), length ignored.
        stream.push(0x93);
        stream.push(0x00);
        stream.extend_from_slice(&0u32.to_le_bytes());
        stream.push(0x00);
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
