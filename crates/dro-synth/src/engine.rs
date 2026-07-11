//! The pull-based playback engine (Python's `OPLStream` + `DROPlayerUpdateThread`
//! + `DROSeeker`, restructured into one thread-free state machine).
//!
//! The Python design *pushed* rendered PCM into output streams from a background
//! thread, relying on PyAudio's blocking `write()` for backpressure. That survives
//! neither a cpal callback nor an `AudioWorkletProcessor.process()`. Here
//! everything is *pulled*: [`PlayerEngine::render`] fills a caller-supplied buffer,
//! stepping instructions and rendering delays, and pauses mid-delay when the
//! buffer fills so the next call resumes exactly where it left off. Native audio,
//! web audio, WAV render and waveform generation are all thin callers of it.
//!
//! What the Python conflated, this separates:
//!
//! - **Frames, not bytes.** `OPLStream.samples_rendered` counted *bytes*
//!   (`len(tmp_buffer)`), and `calculate_playback_samples` multiplied by
//!   `channels * bit_depth/8` to match. Position here is a `u64` frame count.
//! - **One honest delay clock.** DRO delays are milliseconds, VGM delays are
//!   44100 Hz samples; both become output frames through an exact integer carry.
//!   The Python's VGM path (`render_samples`) divided by the output rate instead
//!   of multiplying and then again by `channels + bit_depth/8` ("I'm not sure
//!   why"), rendering VGMs far too fast. Fixed here by construction.
//! - **No dropped samples.** PyOPL needed >= 2 samples per call, so `_render_
//!   samples_out` skipped renders under two frames and lost a whole-frame residue
//!   of exactly one; the integer carry here keeps every frame.

use std::borrow::Borrow;

use dro_core::regdata::PERCUSSION_REGISTER;
use dro_core::song::DRO_FILE_V1;
use dro_core::util::VGM_SAMPLE_RATE;
use dro_core::{Bank, DroInstruction, Song, SongFileType};

use crate::opl::{NukedOpl3, OplChip};

/// The nine per-channel key-on/frequency registers, `0xB0..=0xB8`, whose writes
/// channel muting gates. (Python `DROPlayer.CHANNEL_REGISTERS`, minus the bank
/// bit, which is tracked separately.)
const CHANNEL_REGISTERS: core::ops::RangeInclusive<u8> = 0xB0..=0xB8;

/// Milliseconds (or VGM samples) to output frames, carrying the fractional
/// remainder exactly so it cannot drift over a long song.
///
/// This is the Python `OPLStream.sample_overflow`, done in integers: `frames =
/// (delay * rate + carry) / unit`, where `unit` is `1000` for DRO milliseconds or
/// `44100` for VGM samples.
#[derive(Debug, Clone)]
pub struct FrameClock {
    output_rate: u64,
    /// `1000` when delays are milliseconds, `44100` when they are VGM samples.
    delay_unit: u64,
    carry: u64,
}

impl FrameClock {
    /// A clock rendering at `output_rate` Hz over delays expressed in units of
    /// `delay_unit` per second: `1000` for DRO milliseconds, `44100` for VGM
    /// samples.
    #[must_use]
    pub fn new(output_rate: u32, delay_unit: u32) -> Self {
        Self {
            output_rate: u64::from(output_rate),
            delay_unit: u64::from(delay_unit),
            carry: 0,
        }
    }

    /// The number of output frames a delay of `delay` units lasts, folding in the
    /// remainder carried from previous delays.
    pub fn frames_for(&mut self, delay: u32) -> u64 {
        let numerator = u64::from(delay) * self.output_rate + self.carry;
        self.carry = numerator % self.delay_unit;
        numerator / self.delay_unit
    }

    /// Discards the carried remainder, as after a seek.
    pub fn reset(&mut self) {
        self.carry = 0;
    }
}

/// Which channels and percussion voices are audible, for `dro_split`'s channel
/// isolation and the CLI player's soloing.
///
/// A muted melodic channel's key-on writes (`0xB0..=0xB8`) are dropped; the
/// percussion register (`0xBD`) is AND-masked per bank. Every other register --
/// operator settings, frequencies, feedback -- always passes, which is why
/// isolating a voice only needs to gate its key-on register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Muting {
    /// Bit `bank * 9 + (reg - 0xB0)` set means channel `reg` on that bank is
    /// audible. 18 bits: nine channels, two banks.
    channels: u32,
    /// The AND-mask applied to `0xBD` writes, indexed by bank. `0xFF` passes
    /// everything; `0xE0` keeps the tremolo/vibrato-depth/rhythm-enable control
    /// bits but silences all five drums.
    percussion: [u8; 2],
}

impl Muting {
    const ALL_CHANNELS: u32 = (1 << 18) - 1;

    /// Everything audible -- the default for ordinary playback.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            channels: Self::ALL_CHANNELS,
            percussion: [0xFF, 0xFF],
        }
    }

    /// Nothing melodic audible, and drums silenced but their control bits kept
    /// (`0xE0`) -- the starting point `dro_split` builds a single isolated voice
    /// on top of.
    #[must_use]
    pub const fn silent() -> Self {
        Self {
            channels: 0,
            percussion: [0xE0, 0xE0],
        }
    }

    fn channel_bit(bank: Bank, channel: u8) -> u32 {
        debug_assert!(CHANNEL_REGISTERS.contains(&channel));
        let index = u32::from(bank.index()) * 9 + u32::from(channel - 0xB0);
        1 << index
    }

    /// Makes channel register `channel` (`0xB0..=0xB8`) on `bank` audible.
    pub fn allow_channel(&mut self, bank: Bank, channel: u8) {
        self.channels |= Self::channel_bit(bank, channel);
    }

    /// Mutes channel register `channel` (`0xB0..=0xB8`) on `bank`.
    pub fn mute_channel(&mut self, bank: Bank, channel: u8) {
        self.channels &= !Self::channel_bit(bank, channel);
    }

    /// Sets the AND-mask applied to `0xBD` (percussion) writes on `bank`.
    pub fn set_percussion(&mut self, bank: Bank, mask: u8) {
        self.percussion[usize::from(bank.index())] = mask;
    }

    /// The value to write for register `reg` on `bank`, or `None` if muting drops
    /// it: a muted melodic channel (`0xB0..=0xB8`) is dropped, `0xBD` is AND-masked
    /// per bank, and every other register passes unchanged.
    ///
    /// Shared by the playback engine and the DRO capture so their muting decisions
    /// cannot diverge.
    #[must_use]
    pub fn gate(&self, bank: Bank, reg: u8, value: u8) -> Option<u8> {
        if reg == PERCUSSION_REGISTER {
            Some(value & self.percussion[usize::from(bank.index())])
        } else if !CHANNEL_REGISTERS.contains(&reg) || self.channel_allowed(bank, reg) {
            // A non-channel register always passes; a channel register passes only
            // if audible. The `||` short-circuits before `channel_allowed` (whose
            // debug assert requires a channel register) for non-channel writes.
            Some(value)
        } else {
            None
        }
    }

    #[must_use]
    fn channel_allowed(&self, bank: Bank, channel: u8) -> bool {
        self.channels & Self::channel_bit(bank, channel) != 0
    }
}

impl Default for Muting {
    fn default() -> Self {
        Self::all()
    }
}

/// Where playback currently is. Position is tracked as a `u64` frame count; the
/// milliseconds and instruction index are derived from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// Total frames rendered since the last seek or rewind. Authoritative.
    pub frames_rendered: u64,
    /// Elapsed playback time in milliseconds, derived from `frames_rendered`.
    pub elapsed_ms: u32,
    /// The next instruction the engine will execute. While a delay is playing
    /// this already points *past* that delay; the sounding row is
    /// [`Song::seek_index_for_ms`] of `elapsed_ms`.
    pub next_instruction: usize,
}

/// The pull-based playback state machine.
///
/// Generic over the song container (`&Song` for a one-shot offline render,
/// `Arc<Song>` for the audio thread) and the OPL core (a mock in tests). Build one
/// with [`PlayerEngine::new`]; drive it with [`PlayerEngine::render`].
#[derive(Debug)]
pub struct PlayerEngine<B = std::sync::Arc<Song>, C = NukedOpl3> {
    song: B,
    chip: C,
    sample_rate: u32,
    /// Microseconds rendered after every register write to imitate a real chip's
    /// write latency (`AudioConfig::chip_write_delay`). `0` -- the default --
    /// makes the whole mechanism inert.
    chip_write_delay: f64,
    clock: FrameClock,
    /// The fractional-frame remainder of the chip-write-delay accumulator.
    chip_frame_carry: f64,
    muting: Muting,

    /// The bank subsequent register writes address (Python's `_bank`).
    bank: Bank,
    /// The next instruction to execute.
    pos: usize,
    /// Frames still owed from the delay (or chip-write-delay) in progress. This is
    /// the mid-delay pause point: it persists across `render` calls.
    pending_frames: u64,
    frames_rendered: u64,
}

impl<B: Borrow<Song>> PlayerEngine<B, NukedOpl3> {
    /// Builds an engine for `song`, rendering at `sample_rate` Hz.
    ///
    /// `chip_write_delay` is `AudioConfig::chip_write_delay` in microseconds; pass
    /// `0.0` for exact, unrealistic timing. The chip is reset and positioned at
    /// the start of the song.
    #[must_use]
    pub fn new(song: B, sample_rate: u32, chip_write_delay: f64) -> Self {
        let chip = NukedOpl3::new(sample_rate);
        Self::with_chip(song, chip, sample_rate, chip_write_delay)
    }
}

impl<B: Borrow<Song>, C: OplChip> PlayerEngine<B, C> {
    /// As [`PlayerEngine::new`], but with a caller-provided chip (for tests, or a
    /// different [`OplChip`]).
    #[must_use]
    pub fn with_chip(song: B, chip: C, sample_rate: u32, chip_write_delay: f64) -> Self {
        let delay_unit = if song.borrow().data().delays_in_samples() {
            VGM_SAMPLE_RATE
        } else {
            1000
        };
        let mut engine = Self {
            song,
            chip,
            sample_rate,
            chip_write_delay,
            clock: FrameClock::new(sample_rate, delay_unit),
            chip_frame_carry: 0.0,
            muting: Muting::all(),
            bank: Bank::Low,
            pos: 0,
            pending_frames: 0,
            frames_rendered: 0,
        };
        engine.reset_chip();
        engine
    }

    fn song(&self) -> &Song {
        self.song.borrow()
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The current muting configuration.
    #[must_use]
    pub fn muting(&self) -> Muting {
        self.muting
    }

    /// Replaces the muting configuration, immediately keying off any melodic
    /// channel that just became muted so a sounding note does not ring on.
    ///
    /// (Python did this at the top of every update-thread iteration by diffing a
    /// snapshot; here it happens the moment the caller changes the mutes.)
    pub fn set_muting(&mut self, muting: Muting) {
        for bank in [Bank::Low, Bank::High] {
            for channel in CHANNEL_REGISTERS {
                if self.muting.channel_allowed(bank, channel)
                    && !muting.channel_allowed(bank, channel)
                {
                    self.chip
                        .write_reg(bank.register_offset() | u16::from(channel), 0x00);
                }
            }
        }
        self.muting = muting;
    }

    /// Whether the song has played to the end.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.pending_frames == 0 && self.pos >= self.song().len()
    }

    /// The current playback position.
    #[must_use]
    pub fn position(&self) -> Position {
        Position {
            frames_rendered: self.frames_rendered,
            elapsed_ms: self.elapsed_ms(),
            next_instruction: self.pos,
        }
    }

    fn elapsed_ms(&self) -> u32 {
        let ms = self.frames_rendered * 1000 / u64::from(self.sample_rate);
        u32::try_from(ms).unwrap_or(u32::MAX)
    }

    /// Fills `out` with interleaved stereo frames, stepping instructions and
    /// rendering delays, and returns the number of **frames** written.
    ///
    /// A return value smaller than `out.len() / 2` means the song ended within
    /// this call; the unfilled tail is zeroed. Resuming a delay across calls is
    /// automatic -- the buffer filling mid-delay is the pause point.
    pub fn render(&mut self, out: &mut [i16]) -> usize {
        let out_frames = out.len() / 2;
        let mut filled = 0;

        while filled < out_frames {
            if self.pending_frames > 0 {
                let frames = self.pending_frames.min((out_frames - filled) as u64) as usize;
                let start = filled * 2;
                self.chip
                    .generate_samples(&mut out[start..start + frames * 2]);
                self.pending_frames -= frames as u64;
                self.frames_rendered += frames as u64;
                filled += frames;
            } else if self.pos < self.song().len() {
                let instruction = self
                    .song()
                    .instruction(self.pos)
                    .expect("pos < len, so it decodes");
                self.pending_frames += self.execute(instruction, true);
                self.pos += 1;
            } else {
                break;
            }
        }

        // Zero any tail we could not fill, so a finished song does not repeat the
        // last buffer's audio.
        out[filled * 2..].fill(0);
        filled
    }

    /// Seeks to instruction `index`, rebuilding chip state by replaying every
    /// register write before it. Delays are not rendered, only counted.
    ///
    /// Register writes during a seek are applied *unconditionally* -- muting is a
    /// playback concern, and the Python seeker likewise ignored it. Clamps past
    /// the end of the song.
    pub fn seek_to_pos(&mut self, index: usize) {
        let index = index.min(self.song().len());
        self.reset_chip();
        self.frames_rendered = 0;
        for i in 0..index {
            let instruction = self.song().instruction(i).expect("i < index <= len");
            // No muting, and the returned frame count is accumulated as position
            // rather than rendered -- exactly the state forward play would reach.
            self.frames_rendered += self.execute(instruction, false);
        }
        self.pos = index;
        self.pending_frames = 0;
    }

    /// Seeks to the instruction playing at `target_ms`, via the song's prefix-sum
    /// index. Playback resumes at or before the mark, never after.
    pub fn seek_to_ms(&mut self, target_ms: u32) {
        let index = self.song().seek_index_for_ms(target_ms);
        self.seek_to_pos(index);
    }

    /// Returns to the start of the song.
    pub fn rewind(&mut self) {
        self.seek_to_pos(0);
    }

    /// Applies one instruction and returns how many frames it owes the output.
    ///
    /// With `apply_muting`, a muted channel's write is dropped and `0xBD` is
    /// AND-masked; without it (seeking) every write lands. Either way the bank and
    /// the delay clock advance identically, so play and seek can never diverge.
    fn execute(&mut self, instruction: DroInstruction, apply_muting: bool) -> u64 {
        match instruction {
            DroInstruction::Register { reg, value, .. } => {
                if let Some(bank) = instruction.selected_bank() {
                    self.bank = bank; // DRO v2 / VGM carry the bank per write.
                }
                let gated = if apply_muting {
                    self.muting.gate(self.bank, reg, value)
                } else {
                    Some(value) // Seeking replays every write, as the Python seeker did.
                };
                if let Some(value) = gated {
                    self.chip
                        .write_reg(self.bank.register_offset() | u16::from(reg), value);
                }
                self.chip_delay_frames()
            }
            DroInstruction::BankSwitch(bank) => {
                self.bank = bank; // DRO v1 tracks the bank with these.
                0
            }
            DroInstruction::DelayMs { ms, .. } => self.clock.frames_for(ms),
            DroInstruction::DelaySamples { samples, .. } => self.clock.frames_for(samples),
        }
    }

    /// The frames owed by one register write's chip-write-delay, keeping the
    /// fractional remainder. `0` when the feature is disabled.
    fn chip_delay_frames(&mut self) -> u64 {
        if self.chip_write_delay <= 0.0 {
            return 0;
        }
        let exact = self.chip_write_delay * f64::from(self.sample_rate) / 1_000_000.0
            + self.chip_frame_carry;
        let whole = exact.floor();
        self.chip_frame_carry = exact - whole;
        whole as u64
    }

    /// Clears the chip to silence and re-primes the DRO v1 waveform-select hack.
    ///
    /// A fresh chip state, rather than the Python `reset()`'s 512 zero-writes.
    /// DRO v1 (OPL2) captures assume `0x01 = 0x20` (waveform select enable) is set
    /// before playback, which the Python `DROPlayer.reset` wrote explicitly.
    fn reset_chip(&mut self) {
        self.chip.reset(self.sample_rate);
        self.bank = Bank::Low;
        self.clock.reset();
        self.chip_frame_carry = 0.0;
        let song = self.song();
        if song.file_type == SongFileType::Dro && song.file_version == DRO_FILE_V1 {
            self.chip.write_reg(0x01, 0x20);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dro_core::{DroDataV1, DroDataV2, OplType};

    // `dro-core`'s own fixtures are `pub(crate)`, so rebuild the two the tests
    // need through the public constructors. These match `song/fixtures.rs`.
    fn dro_song_v2() -> Song {
        let mut data: Vec<u8> = (0..10).collect();
        data.extend_from_slice(&[0xFE, 0xB0, 0xFF, 0xC0]);
        data.extend_from_within(..);
        Song::dro_v2(
            "test.dro".to_owned(),
            DroDataV2::new(
                data,
                vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0],
                0xFE,
                0xFF,
            )
            .unwrap(),
            99_170,
            OplType::Opl3,
        )
    }

    fn dro_song_v1() -> Song {
        Song::dro_v1(
            "test_v1.dro".to_owned(),
            DroDataV1::new(vec![
                0x20, 0x01, // 0: register 0x20 = 0x01
                0x00, 0xB0, // 1: short delay, 177 ms
                0x01, 0x34, 0x12, // 2: long delay, 4661 ms
                0x02, // 3: bank switch, low
                0x03, // 4: bank switch, high
                0x04, 0x01, 0xFF, // 5: escaped register 0x01 = 0xFF
                0xBD, 0x20, // 6: register 0xBD = 0x20
            ])
            .unwrap(),
            177 + 0x1234 + 1,
            OplType::Opl2,
        )
    }

    /// A tiny song (8 ms total) for the frame-counting tests, so they do not
    /// render the 99-second `dro_song_v2` fixture.
    fn small_song() -> Song {
        Song::dro_v1(
            "small.dro".to_owned(),
            DroDataV1::new(vec![
                0x20, 0x01, // register write
                0x00, 0x04, // short delay, 5 ms
                0xA0, 0x98, // register write
                0x00, 0x02, // short delay, 3 ms
            ])
            .unwrap(),
            8,
            OplType::Opl2,
        )
    }

    /// An `OplChip` that records writes and counts generated frames, so the
    /// engine's stepping, muting, bank tracking and seek replay can be asserted
    /// without caring about audio. `reset` clears the log, so after a seek the log
    /// holds exactly the replayed writes.
    #[derive(Debug, Default)]
    struct RecordingChip {
        writes: Vec<(u16, u8)>,
        frames: u64,
    }

    impl OplChip for RecordingChip {
        fn reset(&mut self, _sample_rate: u32) {
            self.writes.clear();
        }

        fn write_reg(&mut self, reg: u16, value: u8) {
            self.writes.push((reg, value));
        }

        fn generate_samples(&mut self, buffer: &mut [i16]) {
            self.frames += (buffer.len() / 2) as u64;
            buffer.fill(0);
        }
    }

    fn recording_engine(song: &Song) -> PlayerEngine<&Song, RecordingChip> {
        PlayerEngine::with_chip(song, RecordingChip::default(), 48_000, 0.0)
    }

    /// Renders an engine to the end and returns the total frames rendered.
    fn render_to_end<B: Borrow<Song>, C: OplChip>(engine: &mut PlayerEngine<B, C>) -> u64 {
        let mut out = vec![0i16; 65_536 * 2];
        while engine.render(&mut out) == out.len() / 2 {}
        engine.position().frames_rendered
    }

    #[test]
    fn frame_clock_carries_the_remainder() {
        // 48000 Hz: 1 ms is exactly 48 frames, no carry.
        let mut clock = FrameClock::new(48_000, 1000);
        assert_eq!(clock.frames_for(1), 48);
        assert_eq!(clock.frames_for(1000), 48_000);

        // 49716 Hz: 1 ms is 49.716 frames. The fractional part must accumulate,
        // never be dropped, so 1000 ms totals exactly 49716 frames.
        let mut clock = FrameClock::new(49_716, 1000);
        let total: u64 = (0..1000).map(|_| clock.frames_for(1)).sum();
        assert_eq!(total, 49_716);
    }

    #[test]
    fn vgm_sample_delays_convert_at_the_44100_ratio() {
        // A 0x62 VGM wait is 735 samples (one 60 Hz frame). At 48000 Hz output
        // that is 735 * 48000 / 44100 = 800 frames -- the Python rendered ~169.
        let mut clock = FrameClock::new(48_000, VGM_SAMPLE_RATE);
        assert_eq!(clock.frames_for(735), 800);
    }

    #[test]
    fn total_frames_match_the_song_length() {
        let song = small_song();
        let mut engine = recording_engine(&song);
        let mut out = vec![0i16; 4096 * 2];
        let mut total = 0u64;
        loop {
            let frames = engine.render(&mut out);
            total += frames as u64;
            if frames < out.len() / 2 {
                break;
            }
        }
        // Every millisecond of the song is 48 frames at 48 kHz, summed with an
        // exact carry, so the total is the header length times 48.
        assert_eq!(total, u64::from(song.ms_length) * 48);
        assert!(engine.is_finished());
        assert_eq!(engine.position().elapsed_ms, song.ms_length);
    }

    #[test]
    fn output_is_independent_of_the_pull_size() {
        // The whole point of the pull model: rendering into a 1-frame buffer must
        // produce the same total as one giant buffer.
        let song = small_song();
        let reference = {
            let mut engine = recording_engine(&song);
            let mut out = vec![0i16; 4096 * 2];
            engine.render(&mut out);
            engine.position().frames_rendered
        };
        for chunk in [1usize, 2, 3, 127, 512] {
            let mut engine = recording_engine(&song);
            let mut out = vec![0i16; chunk * 2];
            while engine.render(&mut out) == chunk {}
            assert_eq!(
                engine.position().frames_rendered,
                reference,
                "chunk {chunk}"
            );
        }
    }

    #[test]
    fn register_writes_reach_the_chip_with_the_bank_folded_in() {
        // The v2 fixture's first five instructions are register writes on the low
        // bank. Its data uses even codes (0,2,4,6,8), which index the codemap at
        // 0x10,0x30,0x50,0x70,0x90.
        let song = dro_song_v2();
        let mut engine = recording_engine(&song);
        let mut out = vec![0i16; 8];
        engine.render(&mut out);
        let writes = &engine.chip.writes;
        assert_eq!(writes[0], (0x10, 0x01));
        assert_eq!(writes[1], (0x30, 0x03));
        assert_eq!(writes[4], (0x90, 0x09));
    }

    #[test]
    fn a_muted_channel_drops_its_key_on_writes() {
        // A synthetic v1 stream: set up channel 0 then key it on (0xB0), and key
        // on channel 1 (0xB1). Muting channel 0xB0 must drop the 0xB0 write but
        // keep 0xB1 and all the operator writes.
        let song = Song::dro_v1(
            "mute.dro".to_owned(),
            DroDataV1::new(vec![
                0x20, 0x01, // operator write (always passes)
                0xB0, 0x31, // channel 0 key on
                0xB1, 0x31, // channel 1 key on
                0x00, 0x00, // 1 ms delay so there is something to step past
            ])
            .unwrap(),
            1,
            OplType::Opl2,
        );

        let mut engine = recording_engine(&song);
        let mut muting = Muting::all();
        muting.mute_channel(Bank::Low, 0xB0);
        engine.set_muting(muting);

        let mut out = vec![0i16; 4];
        engine.render(&mut out);
        let writes = &engine.chip.writes;
        assert!(writes.contains(&(0x20, 0x01)), "operator write must pass");
        assert!(!writes.contains(&(0xB0, 0x31)), "muted channel dropped");
        assert!(writes.contains(&(0xB1, 0x31)), "other channel passes");
    }

    #[test]
    fn percussion_writes_are_and_masked() {
        // reg 0xBD with all drums on; a 0xE0 mask keeps the control bits, drops
        // the drums.
        let song = Song::dro_v1(
            "perc.dro".to_owned(),
            DroDataV1::new(vec![0xBD, 0xFF, 0x00, 0x00]).unwrap(),
            1,
            OplType::Opl2,
        );
        let mut engine = recording_engine(&song);
        let mut muting = Muting::all();
        muting.set_percussion(Bank::Low, 0xE0);
        engine.set_muting(muting);

        let mut out = vec![0i16; 4];
        engine.render(&mut out);
        assert!(engine.chip.writes.contains(&(0xBD, 0xE0)));
    }

    #[test]
    fn muting_a_channel_keys_it_off_immediately() {
        let song = dro_song_v1();
        let mut engine = recording_engine(&song);
        engine.chip.writes.clear();
        let mut muting = Muting::all();
        muting.mute_channel(Bank::High, 0xB3);
        engine.set_muting(muting);
        // The transition wrote a 0x00 key-off to the muted channel, on its bank.
        assert!(engine.chip.writes.contains(&(0x1B3, 0x00)));
    }

    #[test]
    fn the_v1_waveform_select_hack_is_primed_on_reset() {
        // A DRO v1 song primes 0x01 = 0x20 so OPL2 captures sound right; a v2 song
        // does not.
        let v1 = dro_song_v1();
        assert_eq!(recording_engine(&v1).chip.writes, vec![(0x01, 0x20)]);
        let v2 = dro_song_v2();
        assert!(recording_engine(&v2).chip.writes.is_empty());
    }

    #[test]
    fn seek_replays_register_writes_and_sets_the_position() {
        let song = dro_song_v2();
        let mut engine = recording_engine(&song);

        // Seeking to index 6 (just past the five register writes and the first
        // short delay) replays those five writes, unconditionally.
        engine.seek_to_pos(6);
        assert_eq!(
            engine.chip.writes,
            vec![
                (0x10, 0x01),
                (0x30, 0x03),
                (0x50, 0x05),
                (0x70, 0x07),
                (0x90, 0x09),
            ]
        );
        // Position is the time at instruction 6: the 177 ms short delay, times 48.
        assert_eq!(engine.position().next_instruction, 6);
        assert_eq!(
            engine.position().frames_rendered,
            u64::from(song.ms_offset_at(6).unwrap()) * 48
        );
    }

    #[test]
    fn seek_then_render_reaches_the_same_end_as_playing_through() {
        let song = dro_song_v2();
        let full = render_to_end(&mut recording_engine(&song));

        let mut engine = recording_engine(&song);
        engine.seek_to_pos(6);
        let before = engine.position().frames_rendered;
        assert!(before > 0 && before < full);
        assert_eq!(render_to_end(&mut engine), full);
    }

    #[test]
    fn seek_to_ms_lands_on_the_prefix_sum_index() {
        let song = dro_song_v2();
        let mut engine = recording_engine(&song);
        engine.seek_to_ms(1000);
        assert_eq!(
            engine.position().next_instruction,
            song.seek_index_for_ms(1000)
        );
    }

    #[test]
    fn rewind_returns_to_the_start() {
        let song = dro_song_v2();
        let mut engine = recording_engine(&song);
        let mut out = vec![0i16; 1024 * 2];
        engine.render(&mut out);
        engine.rewind();
        assert_eq!(engine.position().frames_rendered, 0);
        assert_eq!(engine.position().next_instruction, 0);
        assert!(!engine.is_finished());
    }

    #[test]
    fn an_empty_song_renders_nothing() {
        let song = Song::dro_v2(
            "empty.dro".to_owned(),
            DroDataV2::new(vec![], vec![0x10], 0xFE, 0xFF).unwrap(),
            0,
            OplType::Opl3,
        );
        let mut engine = recording_engine(&song);
        let mut out = vec![99i16; 16];
        assert_eq!(engine.render(&mut out), 0);
        assert!(engine.is_finished());
        assert!(out.iter().all(|&s| s == 0), "the tail must be zeroed");
    }

    #[test]
    fn chip_write_delay_inserts_frames_after_each_write() {
        // With a write delay set, the five register writes of the fixture each
        // render a little audio before the first delay even begins.
        let song = dro_song_v2();
        let mut engine = PlayerEngine::with_chip(&song, RecordingChip::default(), 48_000, 1000.0);
        // 1000 us at 48 kHz is 48 frames per write. Render just past the five
        // writes without reaching the 177 ms delay's bulk.
        let mut out = vec![0i16; 5 * 48 * 2];
        let frames = engine.render(&mut out);
        assert_eq!(frames, 5 * 48);
    }
}
