//! The OPN family: YM2203 (OPN), YM2608 (OPNA) and YM2610 (OPNB).
//!
//! 13,877 files in the VGMRips corpus between them -- 19.1%, the biggest block
//! still silent before this. They are what PC-88/98 games, the Neo Geo and a
//! great deal of arcade hardware sound like.
//!
//! # Assembled rather than written
//!
//! All three are an FM synthesiser bolted to an SSG, and both halves already
//! exist here:
//!
//! - **The FM is OPN2's.** The YM2612 *is* an OPN, and the family shares one
//!   four-operator engine, one envelope generator and one register map. What
//!   sets the YM2612 apart is its 9-bit ladder DAC and its channel-6 PCM mode;
//!   selecting upstream's CMOS (YM3438) mode turns the ladder off, and that is
//!   what the rest of the family's clean DAC sounds like. So Nuked-OPN2 drives
//!   the FM, in CMOS mode, always.
//! - **The SSG is an AY-3-8910**, which `dro-synth` already has clean-room.
//!
//! **What is not modelled: the ADPCM.** The YM2608 and YM2610 each carry an
//! ADPCM-A rhythm section and an ADPCM-B sample channel, and neither is here.
//! On a Neo Geo rip that means the drums are missing while the FM and SSG play
//! -- a real gap, stated rather than hidden, and the reason this is registered
//! as an approximation. `Playability::Partial` already exists to say so for a
//! whole chip; there is no vocabulary yet for "most of one".
//!
//! Two other simplifications, both recorded because they affect pitch rather
//! than presence: the YM2203's programmable prescaler (`$2D`-`$2F`) is assumed
//! to be its default, and the SSG clock is taken as a quarter of the chip
//! clock for all three.
//!
//! Why not a port of MAME's fmopn, as the plan's fallback said? Because
//! `nukeykt/Nuked-OPNB` is not usable -- version 0.0, a header that declares
//! two of its fields twice so it does not compile, no reset, no output function
//! at all -- and a port of thousands of lines of C++ buys, for the FM half,
//! what an already-shipped and byte-tested core gives for nothing. The ADPCM is
//! where a port would actually earn its keep, and that is where the gap is.

use dro_core::vgm::ChipKind;
use dro_synth::{Ay8910, ChipCore};

use crate::ffi::Opn2Chip;
use dro_synth::WriteQueue;

/// The registry ids, one per chip.
pub(crate) const YM2203_ID: &str = "ym2203.nuked";
pub(crate) const YM2608_ID: &str = "ym2608.nuked";
pub(crate) const YM2610_ID: &str = "ym2610.nuked";

/// Internal FM cycles per output sample, as for the YM2612.
const CLOCKS_PER_SAMPLE: u32 = 24;
/// Master clocks per internal cycle.
const MASTER_PER_CLOCK: u32 = 6;

/// The OPN2 pacing, measured for that core: address, value next cycle, then the
/// rest of the rotation. See [`WriteQueue`].
const ADDRESS_SETTLE: u32 = 0;
/// The address goes out on one cycle and its value on the next, so the settle
/// that follows is the rotation less those two -- and less one more, because
/// the queue reaches `Idle` on the cycle *after* the count runs out.
const VALUE_SETTLE: u32 = CLOCKS_PER_SAMPLE - 3;

/// What the chip clock is divided by to reach the SSG.
///
/// A simplification: the real divider follows the FM prescaler, which the
/// YM2203 can reprogram. Assumed at its default here, which is what nearly
/// every rip uses.
const SSG_DIVIDER: u32 = 4;

/// Brings the FM half up to the scale the SSG half already uses.
///
/// The YM2612 core's own gain is documented in `opn2.rs`; this is the same
/// figure for the same reason, and it is what balances FM against SSG. A
/// balance is a listening question -- see the A/B note in `PROVENANCE.md`.
const FM_GAIN: i32 = 5;

/// Which of the family this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpnKind {
    /// Three FM channels, one port, no stereo.
    Ym2203,
    /// Six FM channels over two ports, stereo, ADPCM-A + ADPCM-B (absent here).
    Ym2608,
    /// Four FM channels over two ports, stereo, ADPCM-A + ADPCM-B (absent here).
    Ym2610,
}

impl OpnKind {
    /// Master clocks per output sample.
    ///
    /// The YM2203 runs three FM channels through half the rotation, so it
    /// samples twice as often as its six-channel relatives for the same clock.
    const fn master_per_sample(self) -> u32 {
        match self {
            Self::Ym2203 => MASTER_PER_CLOCK * CLOCKS_PER_SAMPLE / 2,
            Self::Ym2608 | Self::Ym2610 => MASTER_PER_CLOCK * CLOCKS_PER_SAMPLE,
        }
    }

    /// Whether a second register bank exists. The YM2203 has one port.
    const fn has_second_bank(self) -> bool {
        !matches!(self, Self::Ym2203)
    }

    /// Whether the chip pans. The YM2203's FM output is mono, so its rips never
    /// write `$B4` and the FM core has to be told both speakers are on or it
    /// renders silence -- see [`OpnCore::open_the_speakers`].
    const fn is_mono(self) -> bool {
        matches!(self, Self::Ym2203)
    }

    /// The whole-mix output scale, **measured against VGMPlay** by the parity
    /// scorecard's level column (n=12, native rates, no resampler in the
    /// path). One factor over the entire frame -- FM, SSG and ADPCM together
    /// -- so every internal balance survives the correction.
    ///
    /// - YM2203 measured 0.497: x2 closes it, and its mix is complete (no
    ///   ADPCM exists to be missing).
    /// - YM2610 measured 0.318 with the ADPCM sections in: x3 brings it to
    ///   ~0.95.
    /// - YM2608 stays at x1 **deliberately**: its measured 0.641 is depressed
    ///   by the rhythm section this project cannot ship (the chip's internal
    ///   mask ROM), so scaling to the number would make the parts we *do*
    ///   render too loud against the reference's. Its correction waits on a
    ///   measurement over rhythm-light files.
    const fn output_scale(self) -> i32 {
        match self {
            Self::Ym2203 => 2,
            Self::Ym2608 => 1,
            Self::Ym2610 => 3,
        }
    }
}

/// ADPCM-A's output scale: a full-level decode against the FM's range.
///
/// The decoder spans +-2048; x2 puts a full-level drum in the neighbourhood of
/// one FM channel's peak, which is where the hardware balance sits. Judged by
/// the parity scorecard's level column, as every balance here now is.
const ADPCM_A_GAIN: i32 = 2;

/// One ADPCM-A voice: a slice of the shared ROM, keyed on and off.
#[derive(Debug, Default, Clone, Copy)]
struct AdpcmAVoice {
    playing: bool,
    decoder: dro_synth::adpcm::AdpcmA,
    /// Raw register values. Start and end are in 256-byte pages; the level is
    /// five bits with 31 loudest -- a LEVEL, not an attenuation, the polarity
    /// this codebase has mis-read four times elsewhere.
    start: u16,
    end: u16,
    level: u8,
    pan: [bool; 2],
    /// Byte offset into the ROM, and which nibble of it is next.
    position: u32,
    high_nibble: bool,
    /// The last decoded sample, held between the section's 18.5 kHz steps.
    current: i32,
}

/// The six-voice ADPCM-A section the YM2610 carries.
///
/// (The YM2608's rhythm section speaks the same format but reads the chip's
/// *internal* mask ROM, which a VGM does not carry and this project cannot
/// ship -- that gap stands, and the registry label says so.)
#[derive(Debug, Default)]
struct AdpcmASection {
    rom: Vec<u8>,
    voices: [AdpcmAVoice; 6],
    /// Raw six-bit total level, 63 loudest.
    total_level: u8,
    /// The section steps at clock/432 -- one third of the FM sample rate --
    /// and this counts the frames of each triplet.
    phase: u8,
}

impl AdpcmASection {
    fn write(&mut self, register: u8, value: u8) {
        match register {
            0x00 => {
                // Key-on mask, or key-off when bit 7 is set.
                for (index, voice) in self.voices.iter_mut().enumerate() {
                    if value & (1 << index) == 0 {
                        continue;
                    }
                    if value & 0x80 != 0 {
                        voice.playing = false;
                    } else {
                        voice.playing = true;
                        voice.decoder.restart();
                        voice.position = u32::from(voice.start) << 8;
                        voice.high_nibble = true;
                        voice.current = 0;
                    }
                }
            }
            0x01 => self.total_level = value & 0x3F,
            0x08..=0x0D => {
                let voice = &mut self.voices[usize::from(register - 0x08)];
                voice.pan = [value & 0x80 != 0, value & 0x40 != 0];
                voice.level = value & 0x1F;
            }
            0x10..=0x15 => {
                let voice = &mut self.voices[usize::from(register - 0x10)];
                voice.start = (voice.start & 0xFF00) | u16::from(value);
            }
            0x18..=0x1D => {
                let voice = &mut self.voices[usize::from(register - 0x18)];
                voice.start = (voice.start & 0x00FF) | (u16::from(value) << 8);
            }
            0x20..=0x25 => {
                let voice = &mut self.voices[usize::from(register - 0x20)];
                voice.end = (voice.end & 0xFF00) | u16::from(value);
            }
            0x28..=0x2D => {
                let voice = &mut self.voices[usize::from(register - 0x28)];
                voice.end = (voice.end & 0x00FF) | (u16::from(value) << 8);
            }
            _ => {}
        }
    }

    /// One output frame's contribution. Decoding advances on every third
    /// frame; between steps each voice holds its level, exactly as the
    /// hardware's slower DAC clock does.
    fn render(&mut self) -> (i32, i32) {
        let step = self.phase == 0;
        self.phase = (self.phase + 1) % 3;
        let (mut left, mut right) = (0, 0);
        for voice in &mut self.voices {
            if !voice.playing {
                continue;
            }
            if step {
                // Inclusive end: the last page's final byte still plays.
                let last = (u32::from(voice.end) + 1) << 8;
                if voice.position >= last || voice.position as usize >= self.rom.len() {
                    voice.playing = false;
                    continue;
                }
                let byte = self.rom[voice.position as usize];
                let nibble = if voice.high_nibble {
                    byte >> 4
                } else {
                    voice.position += 1;
                    byte & 0x0F
                };
                voice.high_nibble = !voice.high_nibble;
                let att = attenuation_steps(self.total_level, voice.level);
                voice.current = (voice.decoder.decode(nibble) * ADPCM_A_GAIN * gain16(att)) >> 16;
            }
            if voice.pan[0] {
                left += voice.current;
            }
            if voice.pan[1] {
                right += voice.current;
            }
        }
        (left, right)
    }
}

/// The summed attenuation, in 0.75 dB units, of a total level and a voice
/// level. Both registers hold LEVELS -- 63 and 31 are loudest -- so both are
/// inverted here, at the one place the sum is taken.
fn attenuation_steps(total_level: u8, voice_level: u8) -> u32 {
    u32::from(0x3F - (total_level & 0x3F)) + u32::from(0x1F - (voice_level & 0x1F))
}

/// `10^(-0.75 * steps / 20)` in 16.16, the ADPCM sections' volume curve.
///
/// Built by compounding the one-step ratio, since const fns have no `powf`;
/// `the_adpcm_volume_curve_is_three_quarter_decibels` checks the compounding
/// against the closed form. Past the table it is silence.
fn gain16(steps: u32) -> i32 {
    const CURVE: [i32; 96] = {
        let mut table = [0i32; 96];
        // 65536 * 10^(-0.75/20), the one-step ratio in 16.16. The running
        // value carries sixteen guard bits, because eighty truncating
        // multiplies of a bare 16.16 lose several percent by the quiet end of
        // the curve -- the test against the closed form is what said so.
        let ratio: i64 = 60_119;
        let mut value: i64 = 65536 << 16;
        let mut index = 0;
        while index < 96 {
            table[index] = (value >> 16) as i32;
            value = value * ratio / 65536;
            index += 1;
        }
        table
    };
    CURVE.get(steps as usize).copied().unwrap_or(0)
}

/// The single ADPCM-B ("Delta-T") channel.
#[derive(Debug, Default)]
struct DeltaTChannel {
    rom: Vec<u8>,
    decoder: dro_synth::adpcm::DeltaT,
    playing: bool,
    repeat: bool,
    pan: [bool; 2],
    start: u16,
    stop: u16,
    /// The address unit the chip kind implies: 256-byte pages on the YM2610's
    /// ROM (shift 8), smaller units on the YM2608's RAM (shift 5 or 2, chosen
    /// by the bus-width bit in control 2).
    shift: u32,
    delta_n: u16,
    /// Linear output level, 0-255.
    level: u8,
    position: u32,
    /// Nibble-clock accumulator against `delta_n`, 16.16.
    fraction: u32,
    high_nibble: bool,
    current: i32,
}

impl DeltaTChannel {
    fn write(&mut self, register: u8, value: u8) {
        match register {
            0x00 => {
                if value & 0x01 != 0 {
                    // Reset: stop and forget.
                    self.playing = false;
                    self.decoder.restart();
                    self.current = 0;
                } else if value & 0x80 != 0 {
                    self.playing = true;
                    self.repeat = value & 0x10 != 0;
                    self.decoder.restart();
                    self.position = u32::from(self.start) << self.shift;
                    self.fraction = 0;
                    self.high_nibble = true;
                    self.current = 0;
                }
            }
            0x01 => {
                self.pan = [value & 0x80 != 0, value & 0x40 != 0];
                // Bit 3 selects the RAM bus width on the YM2608, and with it
                // the address unit. The YM2610's ROM stays on 256-byte pages.
                if self.shift != 8 {
                    self.shift = if value & 0x08 != 0 { 5 } else { 2 };
                }
            }
            0x02 => self.start = (self.start & 0xFF00) | u16::from(value),
            0x03 => self.start = (self.start & 0x00FF) | (u16::from(value) << 8),
            0x04 => self.stop = (self.stop & 0xFF00) | u16::from(value),
            0x05 => self.stop = (self.stop & 0x00FF) | (u16::from(value) << 8),
            0x09 => self.delta_n = (self.delta_n & 0xFF00) | u16::from(value),
            0x0A => self.delta_n = (self.delta_n & 0x00FF) | (u16::from(value) << 8),
            0x0B => self.level = value,
            _ => {}
        }
    }

    /// One output frame's contribution.
    ///
    /// The nibble clock runs at `clock/72 * deltaN/65536` -- twice the FM
    /// sample rate at full deltaN -- so the accumulator gains `2 * deltaN` per
    /// frame and each carry decodes one nibble.
    fn render(&mut self) -> (i32, i32) {
        if !self.playing {
            return (0, 0);
        }
        self.fraction += u32::from(self.delta_n) * 2;
        while self.fraction >= 1 << 16 {
            self.fraction -= 1 << 16;
            let last = (u32::from(self.stop) + 1) << self.shift;
            if self.position >= last || self.position as usize >= self.rom.len() {
                if self.repeat {
                    self.position = u32::from(self.start) << self.shift;
                    self.decoder.restart();
                    self.high_nibble = true;
                    continue;
                }
                self.playing = false;
                self.current = 0;
                return (0, 0);
            }
            let byte = self.rom[self.position as usize];
            let nibble = if self.high_nibble {
                byte >> 4
            } else {
                self.position += 1;
                byte & 0x0F
            };
            self.high_nibble = !self.high_nibble;
            // A 16-bit decode scaled by the linear level, brought down to sit
            // beside the FM in the mix.
            self.current = (self.decoder.decode(nibble) * i32::from(self.level)) >> 9;
        }
        (
            if self.pan[0] { self.current } else { 0 },
            if self.pan[1] { self.current } else { 0 },
        )
    }
}

/// One of the OPN family: OPN2's FM, an AY's SSG, and -- on the chips that
/// have them -- the ADPCM sections.
///
/// The header's per-chip *AY flags* bytes (`ym2203_ay_flags`,
/// `ym2608_ay_flags`) are deliberately unread: like the standalone AY's flags
/// byte, they are emulator output-mixing options rather than chip behaviour.
/// The SSG here is a Yamaha part by construction, so it keeps [`Ay8910`]'s
/// default fine-grained envelope without needing a type byte to say so.
#[derive(Debug)]
pub struct OpnCore {
    kind: OpnKind,
    fm: Opn2Chip,
    ssg: Ay8910,
    writes: WriteQueue,
    rate: u32,
    /// SSG ticks owed per output sample, in 16.16 fixed point -- the two halves
    /// run at unrelated rates and only the FM's is declared.
    ssg_step: u64,
    ssg_owed: u64,
    /// YM2610 only; empty and inert on the others.
    adpcm_a: AdpcmASection,
    /// YM2608 and YM2610.
    delta_t: DeltaTChannel,
}

impl OpnCore {
    /// A chip of this kind, with no clock yet.
    #[must_use]
    pub fn new(kind: OpnKind) -> Self {
        Self {
            kind,
            fm: Opn2Chip::new(),
            ssg: Ay8910::new(),
            writes: WriteQueue::new(ADDRESS_SETTLE, VALUE_SETTLE),
            rate: 44_100,
            ssg_step: 0,
            ssg_owed: 0,
            adpcm_a: AdpcmASection::default(),
            delta_t: DeltaTChannel {
                // The YM2610 addresses its Delta-T ROM in 256-byte pages; the
                // YM2608's RAM unit arrives with the control-2 write.
                shift: if matches!(kind, OpnKind::Ym2610) {
                    8
                } else {
                    5
                },
                ..DeltaTChannel::default()
            },
        }
    }

    /// Turns both speakers on for every FM channel.
    ///
    /// The FM core comes up with its panning bits clear, because a YM2612 rip
    /// always writes `$B4` before it plays. A YM2203 has no such register --
    /// its FM output is mono -- so without this it would render silence no
    /// matter what the song did.
    fn open_the_speakers(&mut self) {
        if !self.kind.is_mono() {
            return;
        }
        for channel in 0..3u8 {
            self.writes.push(0, 0xB4 + channel, 0xC0);
        }
    }
}

impl ChipCore for OpnCore {
    /// `variant` is the YM2610B's extra two FM channels, which need no
    /// different handling here: the FM core has six either way and a 2610 rip
    /// simply never addresses the last two.
    fn reset(&mut self, clock: u32, _variant: bool) {
        self.writes.clear();
        // The ROMs arrive before the stream starts and must survive the reset
        // the engine does when it loads -- the same contract the OKIM keeps.
        let adpcm_a_rom = std::mem::take(&mut self.adpcm_a.rom);
        let delta_t_rom = std::mem::take(&mut self.delta_t.rom);
        self.adpcm_a = AdpcmASection {
            rom: adpcm_a_rom,
            ..AdpcmASection::default()
        };
        self.delta_t = DeltaTChannel {
            rom: delta_t_rom,
            shift: if matches!(self.kind, OpnKind::Ym2610) {
                8
            } else {
                5
            },
            ..DeltaTChannel::default()
        };
        self.rate = (clock / self.kind.master_per_sample()).max(1);
        // CMOS mode, always: the ladder DAC is the YM2612's alone, and the rest
        // of the family has a clean one.
        self.fm.reset(false);
        let ssg_clock = (clock / SSG_DIVIDER).max(1);
        self.ssg.reset(ssg_clock, false);
        // How many SSG ticks fall in one FM sample, carried in fixed point so
        // the two rates need no common factor.
        self.ssg_step =
            (u64::from(Ay8910::tick_rate(ssg_clock).max(1)) << 16) / u64::from(self.rate);
        self.ssg_owed = 0;
        self.open_the_speakers();
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The sample ROMs, by their VGM block types: `0x82` is the YM2610's
    /// ADPCM-A ROM, `0x83` its Delta-T ROM, and `0x81` the YM2608's Delta-T
    /// memory image.
    fn load_rom(&mut self, block_type: u8, total_size: u32, start: u32, data: &[u8]) {
        // Only the types this chip actually owns: a YM2203 has no sample
        // memory at all, and a YM2610's blocks must not land on a YM2608.
        let rom = match (self.kind, block_type) {
            (OpnKind::Ym2610, 0x82) => &mut self.adpcm_a.rom,
            (OpnKind::Ym2610, 0x83) | (OpnKind::Ym2608, 0x81) => &mut self.delta_t.rom,
            _ => return,
        };
        let total = total_size as usize;
        if rom.len() < total {
            rom.resize(total, 0);
        }
        let at = start as usize;
        let end = (at + data.len()).min(rom.len());
        if at < end {
            rom[at..end].copy_from_slice(&data[..end - at]);
        }
    }

    /// The family's register map: the SSG at `$00`-`$0F` on the first port,
    /// FM above it, and a second bank on port 1 where the chip has one.
    ///
    /// The ADPCM registers -- `$10`-`$1F` on either port, depending on the chip
    /// -- are accepted and dropped. They are the documented gap.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        // Only ports 0 and 1 are bus writes. Anything above is one of the
        // decoder's own spaces -- the YM2203 receives the `0x31` stereo mask
        // on `STEREO_PORT` -- and reading it as `port & 1` below would fold
        // it onto port 0, where `addr` 0 is the SSG's channel A fine period.
        if port > 1 {
            return;
        }
        let register = (addr & 0xFF) as u8;
        let value = (data & 0xFF) as u8;
        let second = port & 1 == 1;

        if second && !self.kind.has_second_bank() {
            return;
        }
        // The SSG lives at the bottom of the *first* port only.
        if !second && register < 0x10 {
            self.ssg.write_register(register, value);
            return;
        }
        // The ADPCM blocks, which sit at different addresses per chip:
        //
        // - YM2610: ADPCM-A fills port 1 `$00`-`$2D`; Delta-T is port 0
        //   `$10`-`$1C`.
        // - YM2608: Delta-T is port 1 `$00`-`$10`; port 0 `$10`-`$1D` is the
        //   rhythm section, whose samples live in the chip's *internal* mask
        //   ROM -- a VGM does not carry it, so those writes are dropped and
        //   the gap is documented at the registry entry.
        //
        // The FM range starts at `$20` on port 0 -- the mode block with the
        // LFO, the timers and `$28` key-on -- and `$30` on port 1 for the
        // second bank's operators.
        match self.kind {
            OpnKind::Ym2610 => {
                if second && register < 0x30 {
                    self.adpcm_a.write(register, value);
                    return;
                }
                if !second && (0x10..0x20).contains(&register) {
                    self.delta_t.write(register - 0x10, value);
                    return;
                }
            }
            OpnKind::Ym2608 => {
                if second && register < 0x20 {
                    self.delta_t.write(register, value);
                    return;
                }
                if !second && (0x10..0x20).contains(&register) {
                    return;
                }
            }
            OpnKind::Ym2203 => {}
        }
        self.writes.push(u32::from(port & 1) * 2, register, value);
    }

    fn render(&mut self, out: &mut [i32]) {
        let mut clocking = self.fm.clocking();
        for frame in out.chunks_exact_mut(2) {
            let mut left = 0i32;
            let mut right = 0i32;
            for _ in 0..CLOCKS_PER_SAMPLE {
                self.writes.advance(|port, byte| clocking.write(port, byte));
                let (l, r) = clocking.clock();
                left += l;
                right += r;
            }
            left *= FM_GAIN;
            right *= FM_GAIN;

            // The SSG runs on its own clock, so it is advanced by however many
            // of its ticks fall inside this FM sample.
            self.ssg_owed += self.ssg_step;
            while self.ssg_owed >= 1 << 16 {
                self.ssg_owed -= 1 << 16;
                self.ssg.tick();
            }
            let ssg = self.ssg.output();

            let (a_left, a_right) = self.adpcm_a.render();
            let (b_left, b_right) = self.delta_t.render();

            // The SSG is mono and sums into both sides, as it does on the
            // chip; the kind's measured scale then lifts the whole mix.
            let scale = self.kind.output_scale();
            frame[0] = (left + ssg + a_left + b_left) * scale;
            frame[1] = (right + ssg + a_right + b_right) * scale;
        }
    }
}

/// Registers all three of the family.
///
/// One entry each rather than one shared: a maker is a plain function pointer,
/// so the kind has to be baked into the function rather than captured.
pub(crate) fn register(registry: &mut dro_synth::CoreRegistry) {
    fn ym2203() -> Box<dyn ChipCore> {
        Box::new(OpnCore::new(OpnKind::Ym2203))
    }
    fn ym2608() -> Box<dyn ChipCore> {
        Box::new(OpnCore::new(OpnKind::Ym2608))
    }
    fn ym2610() -> Box<dyn ChipCore> {
        Box::new(OpnCore::new(OpnKind::Ym2610))
    }

    for (id, chip, make) in [
        (
            YM2203_ID,
            ChipKind::Ym2203,
            ym2203 as fn() -> Box<dyn ChipCore>,
        ),
        (YM2608_ID, ChipKind::Ym2608, ym2608),
        (YM2610_ID, ChipKind::Ym2610, ym2610),
    ] {
        registry.register(dro_synth::CoreInfo {
            id,
            chip,
            // The label states what each chip carries -- and, for the
            // YM2608, the one gap that cannot close: its rhythm samples live
            // in the chip's internal mask ROM, which a VGM does not carry.
            label: match chip {
                ChipKind::Ym2610 => "Nuked-OPN2 FM + SSG + ADPCM",
                ChipKind::Ym2608 => "Nuked-OPN2 FM + SSG + Delta-T (no rhythm ROM)",
                _ => "Nuked-OPN2 FM + SSG",
            },
            authors: "Nuke.YKT (FM); this project (SSG, assembly)",
            license: "LGPL-2.1-or-later",
            upstream: "https://github.com/nukeykt/Nuked-OPN2",
            realtime: true,
            level: dro_synth::LEVEL_UNITY,
            make: dro_synth::CoreMaker::Generic(make),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PC-88's YM2203 clock, and a Neo Geo's YM2610.
    const YM2203_CLOCK: u32 = 3_993_600;
    const YM2610_CLOCK: u32 = 8_000_000;

    fn render(chip: &mut OpnCore, frames: usize) -> Vec<[i32; 2]> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| [f[0], f[1]]).collect()
    }

    fn energy(frames: &[[i32; 2]]) -> i64 {
        frames
            .iter()
            .map(|f| i64::from(f[0].abs()) + i64::from(f[1].abs()))
            .sum()
    }

    /// A loud FM note on channel 1 of `port`, algorithm 7.
    fn fm_key_on(chip: &mut OpnCore, port: u8) {
        for (register, value) in [
            (0x30u16, 0x01u16),
            (0x34, 0x01),
            (0x38, 0x01),
            (0x3C, 0x01),
            (0x40, 0x00),
            (0x44, 0x00),
            (0x48, 0x00),
            (0x4C, 0x00),
            (0x50, 0x1F),
            (0x54, 0x1F),
            (0x58, 0x1F),
            (0x5C, 0x1F),
            (0x60, 0x00),
            (0x64, 0x00),
            (0x68, 0x00),
            (0x6C, 0x00),
            (0x80, 0x00),
            (0x84, 0x00),
            (0x88, 0x00),
            (0x8C, 0x00),
            (0xB0, 0x07),
            (0xB4, 0xC0),
            (0xA4, 0x22),
            (0xA0, 0x69),
        ] {
            chip.write(port, register, value);
        }
        // Key-on is always on port 0, whichever bank the channel is in.
        chip.write(0, 0x28, if port == 0 { 0xF0 } else { 0xF4 });
    }

    /// A loud SSG tone on channel A.
    fn ssg_key_on(chip: &mut OpnCore) {
        chip.write(0, 0x00, 0x40); // period low
        chip.write(0, 0x01, 0x00);
        chip.write(0, 0x07, 0x3E); // tone A on, the rest off
        chip.write(0, 0x08, 0x0F); // full volume
    }

    #[test]
    fn a_fresh_chip_is_silent() {
        for kind in [OpnKind::Ym2203, OpnKind::Ym2608, OpnKind::Ym2610] {
            let mut chip = OpnCore::new(kind);
            chip.reset(YM2610_CLOCK, false);
            assert_eq!(energy(&render(&mut chip, 2000)), 0, "{kind:?}");
        }
    }

    /// Ports above 1 are the decoder's own spaces, not bus writes -- the
    /// YM2203 receives the `0x31` stereo mask on `STEREO_PORT` -- and folding
    /// one onto `port & 1` lands it in the SSG register file, where the mask's
    /// `addr` 0 is channel A's fine period. This test aims a sharper register
    /// at it: folded, the write is "SSG volume A <- 0" and the tone dies.
    #[test]
    fn a_port_beyond_the_second_is_not_a_bus_write() {
        let mut chip = OpnCore::new(OpnKind::Ym2203);
        chip.reset(YM2203_CLOCK, false);
        ssg_key_on(&mut chip);
        let loud = energy(&render(&mut chip, 2000));
        assert!(loud > 0);
        chip.write(dro_core::vgm::stream::STEREO_PORT, 0x08, 0x00);
        let after = energy(&render(&mut chip, 2000));
        assert!(after * 2 > loud, "the write reached the register file");
    }

    /// **Both halves must sound, and separately.** This core is an assembly of
    /// two, so the failure worth catching is one of them silently missing --
    /// which on a real rip reads as a thin mix rather than as a broken core.
    #[test]
    fn the_fm_and_the_ssg_each_make_a_sound_on_their_own() {
        let mut fm_only = OpnCore::new(OpnKind::Ym2610);
        fm_only.reset(YM2610_CLOCK, false);
        fm_key_on(&mut fm_only, 0);
        let fm = energy(&render(&mut fm_only, 8000));
        assert!(fm > 0, "the FM half is silent");

        let mut ssg_only = OpnCore::new(OpnKind::Ym2610);
        ssg_only.reset(YM2610_CLOCK, false);
        ssg_key_on(&mut ssg_only);
        let ssg = energy(&render(&mut ssg_only, 8000));
        assert!(ssg > 0, "the SSG half is silent");

        // And together they are louder than either alone.
        let mut both = OpnCore::new(OpnKind::Ym2610);
        both.reset(YM2610_CLOCK, false);
        fm_key_on(&mut both, 0);
        ssg_key_on(&mut both);
        let together = energy(&render(&mut both, 8000));
        assert!(
            together > fm.max(ssg),
            "{together} vs FM {fm} and SSG {ssg}"
        );
    }

    /// **The YM2203 is mono and has no panning register**, so the FM core has to
    /// be told its speakers are on. Without that it renders perfect silence --
    /// and every PC-88 rip in the corpus is a YM2203.
    #[test]
    fn the_mono_chip_still_makes_fm_sound() {
        let mut chip = OpnCore::new(OpnKind::Ym2203);
        chip.reset(YM2203_CLOCK, false);
        // Deliberately *without* the `$B4` write a YM2612 rip would make.
        for (register, value) in [
            (0x30u16, 0x01u16),
            (0x40, 0x00),
            (0x50, 0x1F),
            (0x60, 0x00),
            (0x80, 0x00),
            (0x34, 0x01),
            (0x44, 0x00),
            (0x54, 0x1F),
            (0x64, 0x00),
            (0x84, 0x00),
            (0x38, 0x01),
            (0x48, 0x00),
            (0x58, 0x1F),
            (0x68, 0x00),
            (0x88, 0x00),
            (0x3C, 0x01),
            (0x4C, 0x00),
            (0x5C, 0x1F),
            (0x6C, 0x00),
            (0x8C, 0x00),
            (0xB0, 0x07),
            (0xA4, 0x22),
            (0xA0, 0x69),
        ] {
            chip.write(0, register, value);
        }
        chip.write(0, 0x28, 0xF0);
        assert!(
            energy(&render(&mut chip, 8000)) > 0,
            "a YM2203 rip writes no panning register, so the core must open the \\
             speakers itself"
        );
    }

    /// The second register bank is real on the OPNA and OPNB and absent on the
    /// OPN. Writing to it on a YM2203 must do nothing rather than land on the
    /// first bank.
    #[test]
    fn only_the_six_channel_parts_have_a_second_bank() {
        let mut ym2610 = OpnCore::new(OpnKind::Ym2610);
        ym2610.reset(YM2610_CLOCK, false);
        fm_key_on(&mut ym2610, 1);
        assert!(
            energy(&render(&mut ym2610, 8000)) > 0,
            "channel 4 lives in the second bank"
        );

        let mut ym2203 = OpnCore::new(OpnKind::Ym2203);
        ym2203.reset(YM2203_CLOCK, false);
        fm_key_on(&mut ym2203, 1);
        assert_eq!(
            energy(&render(&mut ym2203, 8000)),
            0,
            "a YM2203 has one port; the second must be dropped, not folded back"
        );
    }

    /// **The documented gap.** ADPCM registers are accepted and dropped rather
    /// than passed to the FM core, where they would write real FM registers and
    /// corrupt a voice. Silence from the drums is a missing feature; a mangled
    /// FM patch would be a bug.
    #[test]
    fn adpcm_registers_are_dropped_rather_than_misrouted() {
        let mut chip = OpnCore::new(OpnKind::Ym2610);
        chip.reset(YM2610_CLOCK, false);
        fm_key_on(&mut chip, 0);
        let clean = render(&mut chip, 4000);

        let mut disturbed = OpnCore::new(OpnKind::Ym2610);
        disturbed.reset(YM2610_CLOCK, false);
        fm_key_on(&mut disturbed, 0);
        // A rhythm section a Neo Geo rip would really write.
        for register in 0x10..0x1Cu16 {
            disturbed.write(0, register, 0xFF);
        }
        // And an ADPCM-B run on the second port.
        for register in 0x00..0x1Cu16 {
            disturbed.write(1, register, 0xFF);
        }
        assert_eq!(
            clean,
            render(&mut disturbed, 4000),
            "ADPCM writes reached the FM core"
        );
    }

    /// The two halves run on unrelated clocks, so the SSG is advanced by a
    /// fixed-point count of its own ticks per FM sample. A zero step would
    /// freeze it -- silently, since the FM would still play.
    #[test]
    fn the_ssg_advances_on_its_own_clock() {
        let mut chip = OpnCore::new(OpnKind::Ym2610);
        chip.reset(YM2610_CLOCK, false);
        assert!(chip.ssg_step > 0, "the SSG would never tick");
        // Its rate is well above the FM's, so more than one tick per sample.
        assert!(
            chip.ssg_step > 1 << 16,
            "the SSG runs faster than the FM samples: step {}",
            chip.ssg_step
        );

        let mut slow = OpnCore::new(OpnKind::Ym2203);
        slow.reset(YM2203_CLOCK, false);
        assert!(slow.ssg_step > 0);
    }

    /// Chunking must not change the audio -- and this core has two clocks to
    /// keep in step, so it is more at risk than most.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        fn set_up(chip: &mut OpnCore) {
            chip.reset(YM2610_CLOCK, false);
            fm_key_on(chip, 0);
            ssg_key_on(chip);
        }
        let mut whole = OpnCore::new(OpnKind::Ym2610);
        set_up(&mut whole);
        let mut one_go = vec![0i32; 2048 * 2];
        whole.render(&mut one_go);

        let mut chunked = OpnCore::new(OpnKind::Ym2610);
        set_up(&mut chunked);
        let mut piecemeal = vec![0i32; 2048 * 2];
        for chunk in piecemeal.chunks_mut(64 * 2) {
            chunked.render(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// A reset must clear the pending writes as well as both chips: a seek must
    /// not deliver registers the song wrote before it.
    #[test]
    fn a_reset_clears_everything_including_the_queue() {
        let mut chip = OpnCore::new(OpnKind::Ym2610);
        chip.reset(YM2610_CLOCK, false);
        fm_key_on(&mut chip, 0);
        assert!(chip.writes.pending() > 0);

        chip.reset(YM2610_CLOCK, false);
        assert_eq!(chip.writes.pending(), 0);
        assert_eq!(energy(&render(&mut chip, 2000)), 0);
    }

    /// The rate the engine resamples from. The YM2203 runs three channels
    /// through half the rotation, so it samples twice as often for its clock.
    #[test]
    fn the_native_rate_follows_the_channel_count() {
        let mut ym2610 = OpnCore::new(OpnKind::Ym2610);
        ym2610.reset(YM2610_CLOCK, false);
        assert_eq!(ym2610.native_rate(), YM2610_CLOCK / 144);

        let mut ym2203 = OpnCore::new(OpnKind::Ym2203);
        ym2203.reset(YM2203_CLOCK, false);
        assert_eq!(ym2203.native_rate(), YM2203_CLOCK / 72);

        ym2203.reset(0, false);
        assert!(
            ym2203.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// The compounded 0.75 dB curve against its closed form, and the register
    /// polarity in one place: full registers (63, 31) are zero attenuation.
    #[test]
    fn the_adpcm_volume_curve_is_three_quarter_decibels() {
        for steps in [0u32, 1, 8, 24, 48, 80] {
            let expected = 65536.0 * 10f64.powf(-0.75 * f64::from(steps) / 20.0);
            let got = f64::from(gain16(steps));
            assert!(
                (got - expected).abs() / expected < 0.01,
                "{steps} steps: {got} vs {expected}"
            );
        }
        assert_eq!(gain16(200), 0, "past the table is silence");
        assert_eq!(
            attenuation_steps(0x3F, 0x1F),
            0,
            "full registers are LOUDEST -- the registers hold levels"
        );
        assert_eq!(attenuation_steps(0x3F, 0x1E), 1);
        assert_eq!(attenuation_steps(0x00, 0x00), 63 + 31);
    }

    /// A YM2610 drum hit, end to end: ROM in, key-on, sound out, one-shot end.
    /// The register sequence is the one every Neo Geo driver performs.
    #[test]
    fn a_neo_geo_drum_plays_and_stops() {
        let mut chip = OpnCore::new(OpnKind::Ym2610);
        chip.reset(8_000_000, false);

        // One page of alternating extremes at page 1 of the ADPCM-A ROM.
        let mut rom = vec![0u8; 0x300];
        for (index, byte) in rom[0x100..0x200].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x77 } else { 0xFF };
        }
        chip.load_rom(0x82, rom.len() as u32, 0, &rom);

        chip.write(1, 0x01, 0x3F); // total level: full
        chip.write(1, 0x08, 0xDF); // voice 0: both sides, full level
        chip.write(1, 0x10, 0x01); // start page 1
        chip.write(1, 0x18, 0x00);
        chip.write(1, 0x20, 0x01); // end page 1
        chip.write(1, 0x28, 0x00);
        chip.write(1, 0x00, 0x01); // key on voice 0

        let mut out = vec![0i32; 4096 * 2];
        chip.render(&mut out);
        let energy: i64 = out.iter().map(|&s| i64::from(s.abs())).sum();
        assert!(energy > 0, "the drum must sound");

        // 512 nibbles at one step per three output frames is 1536 frames; by
        // 4096 the one-shot voice has ended on its own.
        let mut tail = vec![0i32; 512 * 2];
        chip.render(&mut tail);
        assert!(
            tail.iter().all(|&s| s == 0),
            "a one-shot voice stops at its end register"
        );
    }

    /// Key-off by mask: bit 7 set stops exactly the masked voices.
    #[test]
    fn the_key_off_mask_stops_what_it_names() {
        let mut chip = OpnCore::new(OpnKind::Ym2610);
        chip.reset(8_000_000, false);
        let mut rom = vec![0u8; 0x300];
        for byte in &mut rom[0x100..0x200] {
            *byte = 0x77;
        }
        chip.load_rom(0x82, rom.len() as u32, 0, &rom);
        chip.write(1, 0x01, 0x3F);
        for voice in 0..2u16 {
            chip.write(1, 0x08 + voice, 0xDF);
            chip.write(1, 0x10 + voice, 0x01);
            chip.write(1, 0x18 + voice, 0x00);
            chip.write(1, 0x20 + voice, 0x01);
            chip.write(1, 0x28 + voice, 0x00);
        }
        chip.write(1, 0x00, 0x03); // both on
        chip.write(1, 0x00, 0x81); // voice 0 off
        assert!(!chip.adpcm_a.voices[0].playing);
        assert!(chip.adpcm_a.voices[1].playing);
    }

    /// The Delta-T channel end to end on a YM2610: 256-byte pages, repeat off,
    /// level applied.
    #[test]
    fn the_delta_t_channel_plays_from_its_rom() {
        let mut chip = OpnCore::new(OpnKind::Ym2610);
        chip.reset(8_000_000, false);
        let mut rom = vec![0u8; 0x300];
        for (index, byte) in rom[0x100..0x200].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x77 } else { 0xFF };
        }
        chip.load_rom(0x83, rom.len() as u32, 0, &rom);

        // Delta-T sits at $10-$1C on port 0 of a YM2610.
        chip.write(0, 0x11, 0xC0); // both sides
        chip.write(0, 0x12, 0x01); // start page 1
        chip.write(0, 0x13, 0x00);
        chip.write(0, 0x14, 0x01); // stop page 1
        chip.write(0, 0x15, 0x00);
        chip.write(0, 0x19, 0x00); // delta-N = 0x4000: one nibble every 2 frames
        chip.write(0, 0x1A, 0x40);
        chip.write(0, 0x1B, 0xFF); // full level
        chip.write(0, 0x10, 0x80); // start

        let mut out = vec![0i32; 2048 * 2];
        chip.render(&mut out);
        let energy: i64 = out.iter().map(|&s| i64::from(s.abs())).sum();
        assert!(energy > 0, "the sample must sound");

        // 512 nibbles at half a nibble per frame is 1024 frames; by 2048 the
        // non-repeating channel has finished.
        let mut tail = vec![0i32; 256 * 2];
        chip.render(&mut tail);
        assert!(tail.iter().all(|&s| s == 0), "no repeat, so it ends");
    }

    /// A YM2203 has no ADPCM at all: the registers that would reach a section
    /// on its siblings do nothing, and the ROM types are refused.
    #[test]
    fn the_ym2203_has_no_adpcm_to_reach() {
        let mut chip = OpnCore::new(OpnKind::Ym2203);
        chip.reset(3_000_000, false);
        chip.load_rom(0x82, 0x100, 0, &[0x77; 0x100]);
        assert!(chip.adpcm_a.rom.is_empty(), "no section, no ROM");
    }
}
