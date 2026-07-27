//! The two OKI ADPCM chips: the OKIM6295 sample player and the OKIM6258
//! streaming codec.
//!
//! 6,365 files in the VGMRips corpus between them -- the OKIM6295 is on a great
//! deal of Capcom, Toaplan and Kaneko arcade hardware, and the OKIM6258 in the
//! X68000 and a run of Data East boards.
//!
//! **Route B, from the documented ADPCM algorithm**, so both live in the
//! permissive crate. They share [`Adpcm`], because they share a codec: the same
//! four-bit nibble, the same twelve-bit accumulator, the same 49-entry step
//! table. What differs is where the nibbles come from -- a sample ROM the chip
//! addresses itself, or a stream the host feeds it one byte at a time.
//!
//! # The OKIM6295
//!
//! Four voices, each playing a sample out of a ROM the VGM delivers as a data
//! block. The first kilobyte of that ROM is a **table of contents**: eight bytes
//! per sample, of which the first six are the start and end addresses. A driver
//! plays a sound by naming its index, not its address.
//!
//! Its command interface is two writes, and stateful: one latches a sample
//! number, the next says which voices to start it on and how loud. Miss that
//! pairing and nothing plays.
//!
//! Boards wanting more than the chip's 256 KB sit banking in front of its
//! address bus, and VGM records both flavours as pseudo-registers: the plain
//! whole-view latch at `$0F`, and the NMK112's per-64 KB-quarter banks at
//! `$0E`/`$10`-`$13`. Both are modelled, at read time, in
//! [`Okim6295::rom_byte`].
//!
//! # The OKIM6258
//!
//! No ROM and no voices -- the host writes ADPCM bytes and the chip converts
//! them. In a VGM those bytes usually arrive through the DAC-stream engine
//! rather than as register writes, which the engine already routes here.
//!
//! Not modelled: the OKIM6295's mid-stream clock retune (`$08`-`$0B`), and the
//! OKIM6258's 3-bit ADPCM mode (its flags byte's divider select *is* honoured,
//! via [`configure`](ChipCore::configure)).

use crate::chip::ChipCore;

/// How much the accumulator moves per step, doubling roughly every six entries.
///
/// The documented Dialogic/OKI table. Regenerated in
/// `the_step_table_grows_by_a_ninth_each_entry` rather than trusted, since a
/// single wrong entry is a distortion rather than a failure.
const STEP_TABLE: [i32; 49] = [
    16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130,
    143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796,
    876, 963, 1060, 1166, 1282, 1411, 1552,
];

/// How the step index moves for each nibble magnitude, 0-7 then repeated for
/// the negative half. Documented outright.
const INDEX_DELTA: [i32; 8] = [-1, -1, -1, -1, 2, 5, 7, 9];

/// The shared four-bit ADPCM decoder.
///
/// One nibble in, one twelve-bit sample out, and a step index that adapts. Both
/// chips run exactly this; only their sources differ.
#[derive(Debug, Default, Clone, Copy)]
struct Adpcm {
    signal: i32,
    step: i32,
}

impl Adpcm {
    /// Resets to silence. A voice does this when it is triggered, which is what
    /// stops the last sample's final level bleeding into the next.
    fn restart(&mut self) {
        self.signal = 0;
        self.step = 0;
    }

    /// Decodes one nibble, returning the new twelve-bit signal.
    fn decode(&mut self, nibble: u8) -> i32 {
        let magnitude = i32::from(nibble & 7);
        let size = STEP_TABLE[self.step as usize];
        // The documented reconstruction: an eighth of the step per bit, plus
        // half a step of bias, which is what keeps the decoder centred.
        let mut delta = size >> 3;
        if magnitude & 4 != 0 {
            delta += size;
        }
        if magnitude & 2 != 0 {
            delta += size >> 1;
        }
        if magnitude & 1 != 0 {
            delta += size >> 2;
        }
        if nibble & 8 != 0 {
            self.signal -= delta;
        } else {
            self.signal += delta;
        }
        // Twelve bits signed, clamped rather than wrapped: a wrap is an audible
        // crack where a clamp is the chip's own behaviour.
        self.signal = self.signal.clamp(-2048, 2047);
        self.step = (self.step + INDEX_DELTA[magnitude as usize]).clamp(0, 48);
        self.signal
    }
}

/// Peak amplitude of one voice at full volume.
const PEAK: i32 = 6_000;

/// Volume attenuation in 3 dB steps, as a fraction of unity in 16.16.
///
/// The OKIM6295's four-bit volume field is an attenuation of about 3 dB a step,
/// with the top codes silent. Regenerated in
/// `the_volume_curve_is_three_decibels_a_step`.
const VOLUME: [i32; 16] = [
    65536, 46396, 32846, 23253, 16462, 11654, 8250, 5841, 4135, 2927, 2072, 1467, 1039, 0, 0, 0,
];

/// One of the OKIM6295's four voices.
#[derive(Debug, Default, Clone, Copy)]
struct Voice {
    playing: bool,
    adpcm: Adpcm,
    /// Byte offsets into the sample ROM.
    position: u32,
    end: u32,
    /// Whether the next nibble is the high half of the current byte.
    high_nibble: bool,
    volume: usize,
}

/// The OKIM6295: four ADPCM voices reading from a sample ROM.
#[derive(Debug, Default)]
pub struct Okim6295 {
    rate: u32,
    voices: [Voice; 4],
    rom: Vec<u8>,
    /// The sample number a `0x80`-prefixed write latched, waiting for the write
    /// that says which voices play it.
    latched: Option<u8>,
    /// The banking window's base, in bytes.
    ///
    /// The chip itself addresses 256 KB; boards wanting more sit a latch in
    /// front of its address bus, and VGM records that latch as a write to
    /// pseudo-register `$0F` in 256 KB units. Everything the chip reads -- the
    /// phrase table *and* the sample data -- goes through the window, which is
    /// why the corpus files that select a bank were rendering pure silence: an
    /// unbanked read of their table finds zeros, and every phrase was refused.
    bank: u32,
    /// The NMK112 mode byte (`$0E`): non-zero replaces the plain latch with
    /// per-quarter banking, and bit 7 pages the phrase table too.
    nmk_mode: u8,
    /// The NMK112's four bank registers (`$10`-`$13`), in 64 KB units.
    nmk_banks: [u8; 4],
}

impl Okim6295 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 8_000,
            ..Self::default()
        }
    }

    /// One byte of sample memory, through whichever banking sits in front.
    ///
    /// Two schemes exist and are mutually exclusive. The plain board latch
    /// (`$0F`) offsets the chip's whole 256 KB view in 256 KB units. The
    /// NMK112 (`$0E` mode byte, `$10`-`$13` banks) remaps each 64 KB quarter
    /// of the view independently -- and, when the mode byte's bit 7 is set,
    /// serves each 256-byte page of the phrase table from its own quarter's
    /// bank, so four banks' tables interleave in the first 0x400 bytes. The
    /// exact semantics here are the reference player's, measured against.
    ///
    /// Translation happens per read, not at key-on: a bank write while a
    /// voice plays redirects the rest of the phrase, as the real bus does.
    fn rom_byte(&self, address: u32) -> Option<u8> {
        let physical = if self.nmk_mode == 0 {
            self.bank + address
        } else {
            let slot = if address < 0x400 && self.nmk_mode & 0x80 != 0 {
                address >> 8
            } else {
                address >> 16
            };
            (u32::from(self.nmk_banks[(slot & 3) as usize]) << 16) | (address & 0xFFFF)
        };
        self.rom.get(physical as usize).copied()
    }

    /// The start and end offsets of sample `index`, from the ROM's table of
    /// contents.
    ///
    /// Eight bytes per entry: three of start, three of end, two unused. A
    /// sample whose entry runs past the ROM, or whose end precedes its start,
    /// is refused rather than played as whatever follows it.
    fn sample_bounds(&self, index: u8) -> Option<(u32, u32)> {
        // The table's entries are addresses as the chip sees them, and the
        // table itself is read through the same banking as everything else.
        // The bounds stay chip-visible; [`rom_byte`](Self::rom_byte) does the
        // translation again at play time.
        let at = u32::from(index) * 8;
        let mut entry = [0u8; 6];
        for (offset, byte) in entry.iter_mut().enumerate() {
            *byte = self.rom_byte(at + offset as u32)?;
        }
        let read24 = |b: &[u8]| (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        // The chip drives eighteen address lines, so only the low eighteen
        // bits of a table entry exist as far as it is concerned. Rips for
        // banked boards prove the point: Dragon Master's entries read 0x80400
        // for data sitting 0x400 into a 256 KB window, and taking the entry at
        // face value walks off the ROM. Mask first, then translate.
        const WINDOW: u32 = 0x3FFFF;
        let (start, end) = (read24(&entry[..3]) & WINDOW, read24(&entry[3..]) & WINDOW);
        (start < end && self.rom_byte(end).is_some()).then_some((start, end))
    }
}

impl ChipCore for Okim6295 {
    fn reset(&mut self, clock: u32, variant: bool) {
        let rom = std::mem::take(&mut self.rom);
        // The divider is pin-selected on the board; the header's bit 31 is
        // pin 7, and **pin 7 high selects the divide-by-132 rate**. The first
        // version had the mapping the other way round, which pitched every
        // corpus file by the ratio of the two dividers -- 386 cents, far
        // outside the harness's +-60-cent detune search, so the scorecard read
        // it as pure decorrelation (corr 0.017) on files that were audibly
        // playing. A pitch this size is obvious to an ear and invisible to a
        // correlation, which is exactly the trap the plan warned metrics have
        // with systematic detuning.
        let divider = if variant { 132 } else { 165 };
        *self = Self {
            rate: (clock / divider).max(1),
            ..Self::default()
        };
        // The ROM arrives before the stream starts and must survive the reset
        // the engine does when it loads.
        self.rom = rom;
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The command port, which is a two-write state machine.
    ///
    /// A write with bit 7 set latches a sample number -- *but only when nothing
    /// is latched already*. The second write of a pair is a voice mask in its
    /// top nibble and an attenuation in its bottom, and **voice 4 is bit 7**,
    /// so the two commands are told apart by sequence rather than by that bit.
    /// Dispatching on bit 7 alone looks right and makes voice 4 impossible to
    /// trigger: every command that selects it is read as a sample latch
    /// instead.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let value = (data & 0xFF) as u8;
        match addr & 0x7F {
            0x00 => {}
            // `$0F`: the board's bank latch, recorded by VGM as a
            // pseudo-register, in 256 KB units.
            0x0F => {
                self.bank = u32::from(value) * 0x40000;
                return;
            }
            // The NMK112 mode byte: non-zero switches the address bus from
            // the plain latch to per-quarter banks.
            0x0E => {
                self.nmk_mode = value;
                return;
            }
            // The NMK112's four bank registers, one per 64 KB quarter.
            0x10..=0x13 => {
                self.nmk_banks[usize::from(addr & 0x03)] = value;
                return;
            }
            // `$08`-`$0B` retune the clock -- a known gap, ignored rather
            // than misrouted.
            _ => return,
        }
        if self.latched.is_none() && value & 0x80 != 0 {
            self.latched = Some(value & 0x7F);
            return;
        }
        // The top nibble is a voice mask. With a sample latched it starts
        // them; without one it stops them.
        let mask = value >> 4;
        match self.latched.take() {
            Some(sample) => {
                let bounds = self.sample_bounds(sample);
                for (index, voice) in self.voices.iter_mut().enumerate() {
                    if mask & (1 << index) == 0 {
                        continue;
                    }
                    let Some((start, end)) = bounds else { continue };
                    voice.playing = true;
                    voice.adpcm.restart();
                    voice.position = start;
                    voice.end = end;
                    voice.high_nibble = true;
                    voice.volume = usize::from(value & 0x0F);
                }
            }
            None => {
                for (index, voice) in self.voices.iter_mut().enumerate() {
                    if mask & (1 << index) != 0 {
                        voice.playing = false;
                    }
                }
            }
        }
    }

    /// The sample ROM, delivered as a data block.
    fn load_rom(&mut self, _block_type: u8, total_size: u32, start: u32, data: &[u8]) {
        let total = total_size as usize;
        if self.rom.len() < total {
            self.rom.resize(total, 0);
        }
        let at = start as usize;
        let end = (at + data.len()).min(self.rom.len());
        if at < end {
            self.rom[at..end].copy_from_slice(&data[..end - at]);
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let mut sum = 0i32;
            // By value and written back: reading the ROM through the banking
            // needs `&self` while a voice advances, and `Voice` is `Copy`.
            for index in 0..self.voices.len() {
                let mut voice = self.voices[index];
                if !voice.playing {
                    continue;
                }
                let Some(byte) = self.rom_byte(voice.position) else {
                    self.voices[index].playing = false;
                    continue;
                };
                let nibble = if voice.high_nibble {
                    byte >> 4
                } else {
                    byte & 0x0F
                };
                let signal = voice.adpcm.decode(nibble);
                // Two nibbles to a byte, high half first.
                if voice.high_nibble {
                    voice.high_nibble = false;
                } else {
                    voice.high_nibble = true;
                    voice.position += 1;
                    if voice.position > voice.end {
                        voice.playing = false;
                    }
                }
                // The signal is twelve bits signed; scale, then attenuate.
                sum += ((signal * PEAK / 2048) * VOLUME[voice.volume]) >> 16;
                self.voices[index] = voice;
            }
            // Mono: the chip has one output pin.
            frame[0] = sum;
            frame[1] = sum;
        }
    }
}

/// The OKIM6258: the same codec with no ROM and no voices, fed a byte at a time.
#[derive(Debug, Default)]
pub struct Okim6258 {
    rate: u32,
    /// Kept so [`configure`](ChipCore::configure) can recompute the rate: the
    /// divider arrives in the header's flags byte *after* the reset that
    /// carries the clock.
    clock: u32,
    adpcm: Adpcm,
    /// The most recent decoded level, held between the nibbles the host feeds.
    level: i32,
    /// The low nibble of the byte just written, still to be decoded.
    pending: Option<u8>,
    playing: bool,
}

impl Okim6258 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 8_000,
            ..Self::default()
        }
    }
}

impl ChipCore for Okim6258 {
    /// The divider arrives separately, in the header's flags byte, via
    /// [`configure`](ChipCore::configure); until it does, the documented
    /// default of 512 stands.
    ///
    /// An earlier comment here claimed the engine folded the flags into the
    /// clock before it arrived. Nothing did -- the claim was aspiration
    /// recorded as fact -- so every file whose flags select the 1024 divider
    /// played an octave sharp, invisibly to any local test because the local
    /// tests had no opinion about the header.
    fn reset(&mut self, clock: u32, _variant: bool) {
        *self = Self {
            rate: (clock / 512).max(1),
            clock,
            ..Self::default()
        };
    }

    /// The header's flags: bits 0-1 select the clock divider.
    ///
    /// (Bit 2 selects 3-bit ADPCM, which this core does not model -- a
    /// documented gap rather than a silent misdecode: no corpus file sampled
    /// so far sets it.)
    fn configure(&mut self, settings: &dro_core::vgm::ChipSettings) {
        let divider = match settings.okim6258_flags & 0x03 {
            0 => 1024,
            1 => 768,
            _ => 512,
        };
        self.rate = (self.clock / divider).max(1);
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// `$00` is the control register and `$01` the data port. A data byte is
    /// two nibbles, high half first.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let value = (data & 0xFF) as u8;
        match addr & 0xFF {
            0x00 => {
                // **Bit 1 plays; bit 0 stops.** The first version read bit 0 as
                // the start bit, and every real rip proved it wrong the same
                // way: X68000 files write $03 (stop, while a sample is set up)
                // and then $02 (play) -- under the inverted reading playback
                // switched OFF at the exact moment the music started, and the
                // chip sat decoding into a closed output for the whole song.
                // That is what the parity scorecard's corr 0.0000 / drop 1.000
                // row was. Stop wins when both bits are set, as on the
                // hardware, and stopping resets the codec, which is what keeps
                // one sample's tail out of the next.
                if value & 0x01 != 0 {
                    self.playing = false;
                    self.adpcm.restart();
                    self.level = 0;
                    self.pending = None;
                } else if value & 0x02 != 0 {
                    if !self.playing {
                        self.adpcm.restart();
                        self.level = 0;
                        self.pending = None;
                    }
                    self.playing = true;
                }
            }
            0x01 => {
                self.level = self.adpcm.decode(value >> 4);
                self.pending = Some(value & 0x0F);
            }
            // `$02` is the pan register on the variants that have one, and
            // carries no audio here.
            _ => {}
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            // A byte is two samples: the second nibble falls due on the frame
            // after the write that carried it.
            if let Some(nibble) = self.pending.take() {
                self.level = self.adpcm.decode(nibble);
            }
            let sample = if self.playing {
                self.level * PEAK / 2048
            } else {
                0
            };
            frame[0] = sample;
            frame[1] = sample;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **NMK112 banking replaces the plain latch entirely.** With the mode
    /// byte set, each 64 KB quarter of the chip's view maps through its own
    /// bank register -- table reads included -- and the `$0F` latch no longer
    /// participates.
    #[test]
    fn nmk112_banks_each_quarter_and_eclipses_the_plain_latch() {
        // Two 64 KB banks; everything real lives in bank 1: the phrase table
        // at its base, the sample data 0x1000 in.
        let mut rom = vec![0u8; 0x20000];
        let (start, end) = (0x1000u32, 0x1100u32);
        rom[0x10008..0x1000B].copy_from_slice(&start.to_be_bytes()[1..]);
        rom[0x1000B..0x1000E].copy_from_slice(&end.to_be_bytes()[1..]);
        for (index, byte) in rom[0x11000..=0x11100].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x77 } else { 0xFF };
        }

        let mut chip = Okim6295::new();
        chip.reset(M6295_CLOCK, false);
        chip.load_rom(0x8B, rom.len() as u32, 0, &rom);
        chip.write(0, 0x0F, 0x02); // a stale plain latch that must not matter
        chip.write(0, 0x0E, 0x01); // NMK112 mode on, table unpaged
        chip.write(0, 0x10, 0x01); // quarter 0 reads bank 1
        chip.write(0, 0, 0x81);
        chip.write(0, 0, 0x10);
        assert!(
            energy(&render6295(&mut chip, 2000)) > 0,
            "quarter 0 must read through its bank register"
        );

        // Point quarter 0 at the zeroed bank instead: the same phrase is
        // refused, proving the reads went through the register both times.
        chip.write(0, 0x10, 0x00);
        chip.write(0, 0, 0x81);
        chip.write(0, 0, 0x10);
        assert_eq!(energy(&render6295(&mut chip, 2000)), 0);
    }

    /// With the mode byte's bit 7 set, the first 0x400 bytes of the table are
    /// paged: each 256-byte run reads through its own quarter's bank.
    #[test]
    fn nmk112_bit_seven_pages_the_phrase_table() {
        let mut rom = vec![0u8; 0x20000];
        // Phrase 0x20's entry sits at table address 0x100 -- page 1 -- so in
        // paged mode it reads from bank 1's memory at that offset.
        let (start, end) = (0x1000u32, 0x1100u32);
        rom[0x10100..0x10103].copy_from_slice(&start.to_be_bytes()[1..]);
        rom[0x10103..0x10106].copy_from_slice(&end.to_be_bytes()[1..]);
        // The sample data the entry names is in quarter 0's bank -- bank 0.
        for (index, byte) in rom[0x1000..=0x1100].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x77 } else { 0xFF };
        }

        let mut chip = Okim6295::new();
        chip.reset(M6295_CLOCK, false);
        chip.load_rom(0x8B, rom.len() as u32, 0, &rom);
        chip.write(0, 0x0E, 0x81); // NMK112 mode on, paged table
        chip.write(0, 0x11, 0x01); // page 1 of the table reads bank 1
        chip.write(0, 0, 0x80 | 0x20);
        chip.write(0, 0, 0x10);
        assert!(
            energy(&render6295(&mut chip, 2000)) > 0,
            "table page 1 must read through quarter 1's bank"
        );
    }

    /// A common OKIM6295 clock on Capcom hardware.
    const M6295_CLOCK: u32 = 1_000_000;
    const M6258_CLOCK: u32 = 4_000_000;

    fn render6295(chip: &mut Okim6295, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| f[0]).collect()
    }

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    /// A ROM whose table of contents points sample 1 at a run of loud nibbles.
    fn rom_with_one_sample() -> Vec<u8> {
        let mut rom = vec![0u8; 0x2000];
        let (start, end) = (0x1000u32, 0x1200u32);
        // Entry 1 lives at offset 8: three bytes of start, three of end.
        rom[8..11].copy_from_slice(&start.to_be_bytes()[1..]);
        rom[11..14].copy_from_slice(&end.to_be_bytes()[1..]);
        // Alternating extremes: the steepest waveform the codec can produce.
        for (index, byte) in rom[start as usize..=end as usize].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x77 } else { 0xFF };
        }
        rom
    }

    /// The step table is documented, but it is also a geometric series -- each
    /// entry is about 1.1 times the last. That relationship is what a
    /// transcription error breaks.
    #[test]
    fn the_step_table_grows_by_a_ninth_each_entry() {
        assert_eq!(STEP_TABLE[0], 16);
        assert_eq!(STEP_TABLE[48], 1552);
        for pair in STEP_TABLE.windows(2) {
            let ratio = f64::from(pair[1]) / f64::from(pair[0]);
            assert!(
                (1.06..=1.14).contains(&ratio),
                "{} to {} is a ratio of {ratio}",
                pair[0],
                pair[1]
            );
        }
        // And the series doubles about every six entries, which is what "3 dB a
        // step" means for this codec.
        assert!((STEP_TABLE[6] as f64 / STEP_TABLE[0] as f64 - 1.75).abs() < 0.2);
    }

    /// Every volume level recomputed from 3 dB a step.
    #[test]
    fn the_volume_curve_is_three_decibels_a_step() {
        for (step, &gain) in VOLUME.iter().enumerate().take(13) {
            let expected = 65536.0 * 10f64.powf(-3.0 * step as f64 / 20.0);
            assert_eq!(gain, expected.round() as i32, "step {step}");
        }
        assert_eq!(&VOLUME[13..], &[0, 0, 0], "the top codes are silent");
    }

    /// The codec must move, clamp and adapt -- a decoder that saturates
    /// immediately produces a square wave out of every sample.
    #[test]
    fn the_decoder_tracks_and_clamps() {
        let mut adpcm = Adpcm::default();
        // Driving it hard one way must reach the ceiling and stop there.
        for _ in 0..200 {
            adpcm.decode(0x7);
        }
        assert_eq!(adpcm.signal, 2047, "it must clamp, not wrap");
        for _ in 0..400 {
            adpcm.decode(0xF);
        }
        assert_eq!(adpcm.signal, -2048);
        // And the step index must have adapted upward on the way.
        assert_eq!(adpcm.step, 48);

        adpcm.restart();
        assert_eq!((adpcm.signal, adpcm.step), (0, 0));
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_triggered_voice_is_not() {
        let mut chip = Okim6295::new();
        chip.reset(M6295_CLOCK, false);
        assert_eq!(energy(&render6295(&mut chip, 2000)), 0);

        let rom = rom_with_one_sample();
        chip.load_rom(0x8B, rom.len() as u32, 0, &rom);
        chip.write(0, 0, 0x81); // latch sample 1
        // The low nibble is an *attenuation*, so 0 is loudest and 15 silent.
        chip.write(0, 0, 0x10); // voice 1, full volume
        assert!(energy(&render6295(&mut chip, 2000)) > 0);
    }

    /// **The command interface is stateful and paired.** A write with bit 7 set
    /// latches a sample; the next says which voices play it. Treating either as
    /// a standalone command means nothing ever plays.
    #[test]
    fn a_sample_needs_both_halves_of_the_command() {
        let rom = rom_with_one_sample();
        let mut chip = Okim6295::new();
        chip.reset(M6295_CLOCK, false);
        chip.load_rom(0x8B, rom.len() as u32, 0, &rom);

        // The latch on its own does nothing.
        chip.write(0, 0, 0x81);
        assert_eq!(energy(&render6295(&mut chip, 500)), 0);

        // And a voice mask with nothing latched *stops* rather than starts.
        chip.write(0, 0, 0x10);
        let started = energy(&render6295(&mut chip, 2000));
        assert!(started > 0, "the pair must start it");

        chip.write(0, 0, 0x10); // mask with no latch: stop voice 1
        assert_eq!(
            energy(&render6295(&mut chip, 2000)),
            0,
            "a mask with nothing latched must stop the voice"
        );
    }

    /// **Voice 4's select bit is bit 7**, the same bit that marks a sample
    /// latch. The chip tells the two apart by sequence, so a core that
    /// dispatches on the bit alone can never start voice 4 -- every command
    /// selecting it is swallowed as a latch, and three voices play where four
    /// should.
    #[test]
    fn the_fourth_voice_can_be_triggered() {
        let rom = rom_with_one_sample();
        let mut chip = Okim6295::new();
        chip.reset(M6295_CLOCK, false);
        chip.load_rom(0x8B, rom.len() as u32, 0, &rom);

        chip.write(0, 0, 0x81); // latch sample 1
        chip.write(0, 0, 0x80); // voice 4 only, full volume
        assert!(chip.voices[3].playing, "voice 4 must start");
        assert!(
            chip.voices[..3].iter().all(|voice| !voice.playing),
            "and no other voice"
        );
        assert!(energy(&render6295(&mut chip, 2000)) > 0);
    }

    /// A sample whose table entry is nonsense must be refused, not played as
    /// whatever bytes happen to follow. A ROM that has not arrived yet is the
    /// common case, and it must not produce noise.
    #[test]
    fn a_bad_table_entry_plays_nothing() {
        let mut chip = Okim6295::new();
        chip.reset(M6295_CLOCK, false);
        // No ROM at all.
        chip.write(0, 0, 0x81);
        chip.write(0, 0, 0x10);
        assert_eq!(energy(&render6295(&mut chip, 2000)), 0);

        // A ROM whose entry runs past its end.
        let mut rom = vec![0u8; 0x100];
        rom[8..11].copy_from_slice(&[0x00, 0x00, 0x40]);
        rom[11..14].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
        chip.load_rom(0x8B, rom.len() as u32, 0, &rom);
        chip.write(0, 0, 0x81);
        chip.write(0, 0, 0x10);
        assert_eq!(energy(&render6295(&mut chip, 2000)), 0);
    }

    /// A voice stops at the end of its sample rather than reading on.
    #[test]
    fn a_voice_stops_at_the_end_of_its_sample() {
        let mut rom = vec![0u8; 0x2000];
        rom[8..11].copy_from_slice(&[0x00, 0x10, 0x00]);
        rom[11..14].copy_from_slice(&[0x00, 0x10, 0x0F]); // sixteen bytes
        for byte in &mut rom[0x1000..=0x100F] {
            *byte = 0x77;
        }
        let mut chip = Okim6295::new();
        chip.reset(M6295_CLOCK, false);
        chip.load_rom(0x8B, rom.len() as u32, 0, &rom);
        chip.write(0, 0, 0x81);
        chip.write(0, 0, 0x10);

        // Sixteen bytes is 32 nibbles, so 32 frames.
        let _ = render6295(&mut chip, 40);
        assert!(!chip.voices[0].playing, "it must stop at the end");
    }

    /// The engine resets a core when it loads, and the ROM arrives first.
    #[test]
    fn a_reset_keeps_the_sample_rom() {
        let mut chip = Okim6295::new();
        let rom = rom_with_one_sample();
        chip.load_rom(0x8B, rom.len() as u32, 0, &rom);
        chip.reset(M6295_CLOCK, false);
        assert_eq!(chip.rom.len(), rom.len(), "the reset threw the ROM away");
    }

    /// The pin-selected divider changes the rate; the header's bit 31 is
    /// pin 7, and pin 7 *high* is the faster divide-by-132.
    ///
    /// This test used to assert the opposite mapping, and passed, because it
    /// was written from the same misreading as the code -- the third such pair
    /// this programme has caught. What finally told them apart was external:
    /// with the mapping inverted both ways, every corpus file played 386 cents
    /// off against VGMPlay, which correlation reads as noise and an ear reads
    /// as the wrong key.
    #[test]
    fn the_divider_flag_changes_the_rate() {
        let mut chip = Okim6295::new();
        chip.reset(M6295_CLOCK, false);
        assert_eq!(chip.native_rate(), M6295_CLOCK / 165);
        chip.reset(M6295_CLOCK, true);
        assert_eq!(chip.native_rate(), M6295_CLOCK / 132);
        chip.reset(0, false);
        assert!(chip.native_rate() >= 1);
    }

    /// The 6258 has no ROM: the host feeds it bytes, and each is two samples.
    #[test]
    fn the_streaming_codec_converts_what_it_is_fed() {
        let mut chip = Okim6258::new();
        chip.reset(M6258_CLOCK, false);
        chip.write(0, 0x00, 0x02); // play -- bit 1, not bit 0

        let mut out = vec![0i32; 64 * 2];
        let mut total = 0i64;
        for step in 0..32 {
            // Alternating extremes again.
            chip.write(0, 0x01, if step % 2 == 0 { 0x77 } else { 0xFF });
            chip.render(&mut out[..2 * 2]);
            total += energy(&out[..4].iter().step_by(2).copied().collect::<Vec<_>>());
        }
        assert!(total > 0, "the streaming codec produced nothing");

        // Stopping resets it, so the next sample does not inherit a level.
        chip.write(0, 0x00, 0x01);
        chip.render(&mut out);
        assert!(
            out.iter().all(|&s| s == 0),
            "a stopped codec must be silent"
        );
    }

    /// The bank latch, taken from the corpus (Castle of Dracula, 01 Title
    /// Screen): the file selects bank 2 with `$0F`, loads its table and data at
    /// 0x80000+, and only then plays. Without the window every read landed in
    /// the zeroed low ROM, every phrase was refused, and the chip rendered
    /// *nothing* -- four of the parity scorecard's twelve OKIM6295 files were
    /// exactly this.
    #[test]
    fn the_bank_latch_moves_the_whole_window() {
        let mut chip = Okim6295::new();
        chip.reset(M6295_CLOCK, false);
        // A ROM whose only sample lives in bank 1: table and data both above
        // 256 KB, nothing below.
        let bank = 0x40000u32;
        let mut rom = vec![0u8; 0x50000];
        let (start, end) = (0x1000u32, 0x1200u32);
        // High bits beyond the chip's eighteen address lines, as Dragon
        // Master's real table spells them: masked, not trusted.
        let spelled = start | 0x80000;
        rom[bank as usize + 8..bank as usize + 11].copy_from_slice(&spelled.to_be_bytes()[1..]);
        rom[bank as usize + 11..bank as usize + 14].copy_from_slice(&end.to_be_bytes()[1..]);
        for (index, byte) in rom[(bank + start) as usize..=(bank + end) as usize]
            .iter_mut()
            .enumerate()
        {
            *byte = if index % 2 == 0 { 0x77 } else { 0xFF };
        }
        chip.load_rom(0x8B, rom.len() as u32, 0, &rom);

        // Unbanked, phrase 1 resolves to zeros and is refused.
        chip.write(0, 0, 0x81);
        chip.write(0, 0, 0x10);
        assert_eq!(energy(&render6295(&mut chip, 256)), 0, "no bank, no sound");

        // Banked, the same command plays.
        chip.write(0, 0x0F, 1);
        chip.write(0, 0, 0x81);
        chip.write(0, 0, 0x10);
        assert!(
            energy(&render6295(&mut chip, 256)) > 0,
            "the window must carry both the table and the data"
        );
    }

    /// The exact control sequence every X68000 rip performs, taken from the
    /// corpus (Detana!! TwinBee, 04 Credit): $03 to the command register while
    /// the sample is set up, then $02 to play. Under the inverted bit reading
    /// this fixed, playback switched off at the moment the music started, and
    /// the whole chip decoded into a closed output -- the parity scorecard's
    /// corr 0.0000 row. The sequence is pinned so the bits cannot swap back.
    #[test]
    fn the_x68000_control_sequence_plays() {
        let mut chip = Okim6258::new();
        chip.reset(M6258_CLOCK, false);
        chip.write(0, 0x00, 0x03); // stop, as the rips spell it during setup
        chip.write(0, 0x02, 0x80); // pan, carried but not audio
        chip.write(0, 0x00, 0x02); // play

        let mut out = vec![0i32; 4 * 2];
        let mut total = 0i64;
        for step in 0..64 {
            chip.write(0, 0x01, if step % 2 == 0 { 0x77 } else { 0xFF });
            chip.render(&mut out);
            total += energy(&out.iter().step_by(2).copied().collect::<Vec<_>>());
        }
        assert!(total > 0, "the play command must open the output");

        // And $03 -- stop with bit 1 also set -- must stop: stop wins.
        chip.write(0, 0x00, 0x03);
        chip.render(&mut out);
        assert!(out.iter().all(|&s| s == 0), "stop wins over play");
    }

    /// Chunking must not change the audio.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        fn set_up(chip: &mut Okim6295, rom: &[u8]) {
            chip.reset(M6295_CLOCK, false);
            chip.load_rom(0x8B, rom.len() as u32, 0, rom);
            chip.write(0, 0, 0x81);
            chip.write(0, 0, 0x10);
        }
        let rom = rom_with_one_sample();
        let mut whole = Okim6295::new();
        set_up(&mut whole, &rom);
        let mut one_go = vec![0i32; 1024 * 2];
        whole.render(&mut one_go);

        let mut chunked = Okim6295::new();
        set_up(&mut chunked, &rom);
        let mut piecemeal = vec![0i32; 1024 * 2];
        for chunk in piecemeal.chunks_mut(64 * 2) {
            chunked.render(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// Four voices at full volume, and the headroom that implies.
    #[test]
    fn a_full_chip_uses_the_range_without_clipping_it() {
        let rom = rom_with_one_sample();
        let mut chip = Okim6295::new();
        chip.reset(M6295_CLOCK, false);
        chip.load_rom(0x8B, rom.len() as u32, 0, &rom);
        chip.write(0, 0, 0x81);
        chip.write(0, 0, 0xF0); // every voice, full volume

        let loudest = render6295(&mut chip, 2000)
            .iter()
            .map(|&s| s.abs())
            .max()
            .unwrap_or(0);
        assert!(loudest > PEAK, "four voices peaked at only {loudest}");
        assert!(
            loudest < i32::from(i16::MAX),
            "four voices must not need the mixer's clamp on their own: {loudest}"
        );
    }
}
