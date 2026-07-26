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
use crate::engine::FrameClock;

/// Fixed-point fractional bits for the per-chip resampler.
const FRAC_BITS: u32 = 16;
const FRAC_ONE: u64 = 1 << FRAC_BITS;

/// One chip instance, with the resampler that brings it to the output rate.
struct Voice {
    target: ChipTarget,
    core: Box<dyn ChipCore>,
    /// Source frames per output frame, in `FRAC_BITS` fixed point.
    step: u64,
    /// Position between `prev` and `next`, in the same fixed point.
    position: u64,
    prev: [i32; 2],
    next: [i32; 2],
}

impl Voice {
    fn new(
        target: ChipTarget,
        mut core: Box<dyn ChipCore>,
        chip: &ChipUse,
        output_rate: u32,
    ) -> Self {
        core.reset(chip.clock, chip.variant);
        let native = core.native_rate().max(1);
        Self {
            target,
            core,
            step: (u64::from(native) << FRAC_BITS) / u64::from(output_rate.max(1)),
            // Past the end, so the first frame pulled primes both samples.
            position: FRAC_ONE * 2,
            prev: [0; 2],
            next: [0; 2],
        }
    }

    /// Whether this voice takes `target`'s writes: the chip, and which of its
    /// (up to two) instances. The port is the chip's own business.
    fn accepts(&self, target: ChipTarget) -> bool {
        self.target.kind == target.kind && self.target.instance == target.instance
    }

    /// The next output frame, linearly interpolated between the source frames
    /// bracketing it.
    ///
    /// One source frame is pulled at a time. A core that would rather render in
    /// blocks can buffer internally; doing it here would mean either a lookahead
    /// the caller's chunk size could observe, or a buffer flushed between pulls
    /// -- and the contract is that neither is visible.
    fn next_frame(&mut self) -> [i32; 2] {
        while self.position >= FRAC_ONE {
            self.prev = self.next;
            let mut frame = [0i32; 2];
            self.core.render(&mut frame);
            self.next = frame;
            self.position -= FRAC_ONE;
        }
        let t = self.position;
        let lerp = |a: i32, b: i32| -> i32 {
            let a = i64::from(a);
            let b = i64::from(b);
            let value = a + (((b - a) * t as i64) >> FRAC_BITS);
            value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        };
        let frame = [
            lerp(self.prev[0], self.next[0]),
            lerp(self.prev[1], self.next[1]),
        ];
        self.position += self.step;
        frame
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
    /// What `0x62` and `0x63` wait for, which `0x64` can override.
    wait_60hz: u32,
    wait_50hz: u32,
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
                    voices.push(Voice::new(target, core, chip, output_rate));
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
            wait_60hz: WAIT_60HZ,
            wait_50hz: WAIT_50HZ,
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
            }
            voice.position = FRAC_ONE * 2;
            voice.prev = [0; 2];
            voice.next = [0; 2];
        }
        self.banks.clear();
        self.table = None;
        self.streams.clear();
        self.due.clear();
        self.clock.reset();
        self.index = 0;
        self.pending = 0;
        self.finished = false;
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
    }

    /// The row the next command will come from.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.index
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
        while self.pending == 0 && self.index < stream.len() {
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
        if self.index >= stream.len() {
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
        assert_eq!(engine.position(), 9);

        // And playing on from there covers only what is left.
        let mut out = vec![0i16; 44_100 * 4];
        assert_eq!(engine.render(&mut out), 44_100, "the last second");
        assert!(engine.is_finished());
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
        assert_eq!(engine.position(), 0);
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
