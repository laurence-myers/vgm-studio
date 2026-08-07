//! YMF262-LLE as a [`ChipCore`]: the OPL3 die itself, clocked pin by pin.
//!
//! The chip on every Sound Blaster Pro 2 and SB16, simulated from John
//! McMaster's decap gate by gate -- the ground-truth counterpart to the
//! modelled Nuked-OPL3, in the same shared `opl3` picker slot.
//!
//! `realtime: false`, like every die: below realtime on today's CPUs, and
//! honoured anyway when chosen -- playback included.
//!
//! # Driving a die instead of an API
//!
//! As with its siblings: there is no `write()` upstream, there is a bus --
//! two address lines (`a0` address/value, `a1` the register bank), a byte of
//! data, and chip-select/write strobes held across master clocks by the
//! pin-level state machine here.
//!
//! Audio leaves as four 16-bit linear words per sample on two serial data
//! lines, the YAC512 DAC's format: `DOAB` time-multiplexes channels A and B,
//! `DOCD` channels C and D, paced by the `SY` bit clock and framed by the
//! `SMPAC`/`SMPBD` strobes. A stereo card wires A and B (and, when a second
//! YAC512 is fitted, C and D) to left and right, so the wrapper mixes
//! `left = A + C`, `right = B + D` -- every output pin a song can pan to is
//! heard, and plain stereo material rides A/B untouched.
//!
//! # One die, both OPL generations
//!
//! Like Nuked-OPL3 and the CQM, this row serves the whole OPL family: an
//! OPL2 song on OPL3 silicon is exactly what an SB16 owner heard. A song
//! clocking an OPL2-generation chip supplies the OPL2's ~3.58 MHz master
//! clock, which the real upgrade path quadrupled; [`reset`](ChipCore::reset)
//! recognises the generation by its clock and runs the die at four times an
//! OPL2-generation clock, landing both generations on the same 49716 Hz.

use std::collections::VecDeque;
use vgms_core::vgm::ChipKind;
use vgms_synth::ChipCore;

use crate::ffi::{Opl3LleChip, Opn2Pins};

/// The registry id: the OPL family shares the `opl3` slot, so this names the
/// die within it.
pub(crate) const CORE_ID: &str = "opl3.ymf262-lle";

/// Master clocks per output sample: 36 internal slots at clock/8.
const CLOCKS_PER_SAMPLE: u32 = 288;

/// A chip clock at or above this is an OPL3-generation master clock
/// (~14.3 MHz); below it, an OPL2-generation clock (~3.58 MHz) the die runs
/// at four times, as the hardware upgrade path did.
const OPL3_CLOCK_FLOOR: u32 = 8_000_000;

/// Master clocks the bus signals are held asserted for one byte.
const WRITE_HOLD: u32 = 8;

/// Master clocks of bus silence after any byte, hold included 64.
///
/// The datasheet asks 32 after either byte of a pair; the die commits a write
/// through a latch chain clocked at clock/8 (roughly four internal cycles, 32
/// master clocks end to end), and as on the OPL2 die the data latch is shared
/// between address and value bytes -- racing the commit decodes the wrong
/// byte. Twice the datasheet figure keeps clear of the chain at the cost of
/// under half a sample per register pair.
const WRITE_RECOVER: u32 = 64 - WRITE_HOLD;

/// Master clocks with `IC` held low at reset -- two whole samples, far past
/// the reset latch chains, and free offline.
const RESET_HOLD: u32 = 2 * CLOCKS_PER_SAMPLE;

/// One queued byte for the bus: `a0` low presents an address, high a value;
/// `a1` selects the register bank.
#[derive(Debug, Clone, Copy)]
struct BusByte {
    a1: bool,
    a0: bool,
    data: u8,
}

/// Where the bus state machine is in delivering a byte.
#[derive(Debug, Clone, Copy)]
enum Bus {
    Idle,
    Holding(u32),
    Recovering(u32),
}

/// Taps the two serial streams and cuts words at the strobes' falling edges.
///
/// Each data line carries two 16-bit words per sample, framed by different
/// strobes: `SMPAC` ends the A word on `DOAB` and the C word on `DOCD`,
/// `SMPBD` the B and D words. The bit clock (`SY`) pulses once per internal
/// cycle, so the tap shifts only on its rising edges, as the YAC512s on the
/// board do.
#[derive(Debug, Default, Clone, Copy)]
struct DacCapture {
    smpac_was_high: bool,
    smpbd_was_high: bool,
    sy_was_high: bool,
    /// Bit-clock samples of each data line, most recent in bit 0.
    doab: u32,
    docd: u32,
}

/// One sample's four decoded channels.
#[derive(Debug, Default, Clone, Copy)]
struct Channels {
    a: i32,
    b: i32,
    c: i32,
    d: i32,
}

impl DacCapture {
    /// Feeds one master clock's pin state into `held`, decoding whichever
    /// words a strobe's falling edge just ended.
    fn feed(&mut self, pins: crate::ffi::Opl3DacPins, held: &mut Channels) {
        if pins.sy && !self.sy_was_high {
            self.doab = (self.doab << 1) | u32::from(pins.doab);
            self.docd = (self.docd << 1) | u32::from(pins.docd);
        }
        self.sy_was_high = pins.sy;
        if !pins.smpac && self.smpac_was_high {
            held.a = decode_yac512(self.doab);
            held.c = decode_yac512(self.docd);
        }
        self.smpac_was_high = pins.smpac;
        if !pins.smpbd && self.smpbd_was_high {
            held.b = decode_yac512(self.doab);
            held.d = decode_yac512(self.docd);
        }
        self.smpbd_was_high = pins.smpbd;
    }
}

/// The last serial word before a strobe's falling edge, to linear PCM.
///
/// The YAC512's 16-bit linear format, as the die's shifter transmits it: LSB
/// first, the low fifteen bits of the accumulator with the sign *inverted*
/// into bit 15 (offset binary), an overflowed accumulator saturated by
/// holding the sign on the line. The strobe's fall trails the word by one bit
/// clock (as on the OPNA die, unlike the OPL2's), so the word rides stream
/// bits 1..=16 -- pinned by the idle-decodes-to-zero probe (positive zero is
/// `0x8000`, whose single set bit lands loudly elsewhere if the framing
/// slips).
fn decode_yac512(stream: u32) -> i32 {
    let mut value: u32 = 0;
    for i in 0..16 {
        // Skip the trailing bit at stream bit 0; the word's LSB is the oldest.
        value |= ((stream >> (16 - i)) & 1) << i;
    }
    i32::from((value as u16 ^ 0x8000) as i16)
}

/// The YMF262, as its own die computes it.
#[derive(Debug)]
pub struct Ymf262Lle {
    chip: Opl3LleChip,
    rate: u32,
    writes: VecDeque<BusByte>,
    bus: Bus,
    capture: DacCapture,
    /// The last decoded words, held between strobes.
    held: Channels,
}

impl Ymf262Lle {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: Opl3LleChip::new(),
            rate: 49_716,
            writes: VecDeque::new(),
            bus: Bus::Idle,
            capture: DacCapture::default(),
            held: Channels::default(),
        }
    }

    /// One master clock: the bus state machine, both edges, the DAC tap.
    fn master_clock(&mut self) {
        match self.bus {
            Bus::Idle => {
                if let Some(byte) = self.writes.pop_front() {
                    self.chip.set_pins(Opn2Pins {
                        cs: false,
                        wr: false,
                        a0: byte.a0,
                        a1: byte.a1,
                        data: byte.data,
                        ..Opn2Pins::default()
                    });
                    self.bus = Bus::Holding(WRITE_HOLD);
                }
            }
            Bus::Holding(left) => {
                self.bus = if left > 1 {
                    Bus::Holding(left - 1)
                } else {
                    self.chip.set_pins(Opn2Pins::default());
                    Bus::Recovering(WRITE_RECOVER)
                };
            }
            Bus::Recovering(left) => {
                self.bus = if left > 1 {
                    Bus::Recovering(left - 1)
                } else {
                    Bus::Idle
                };
            }
        }

        self.chip.clock_edge(false);
        self.chip.clock_edge(true);

        let pins = self.chip.dac_pins();
        self.capture.feed(pins, &mut self.held);
    }
}

impl Default for Ymf262Lle {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ymf262Lle {
    /// `variant` carries nothing for this family; there is one YMF262 die.
    fn reset(&mut self, clock: u32, _variant: bool) {
        self.writes.clear();
        self.bus = Bus::Idle;
        self.capture = DacCapture::default();
        self.held = Channels::default();
        // An OPL2-generation clock runs the die at four times itself, as the
        // hardware upgrade path did; either way the output rate is the song's
        // clock divided by its own generation's divisor.
        let divisor = if clock >= OPL3_CLOCK_FLOOR { 288 } else { 72 };
        self.rate = (clock / divisor).max(1);

        self.chip.power_cycle();
        // The electrical reset: IC low while the clock runs, then released.
        self.chip.set_pins(Opn2Pins {
            ic: false,
            ..Opn2Pins::default()
        });
        for _ in 0..RESET_HOLD {
            self.chip.clock_edge(false);
            self.chip.clock_edge(true);
        }
        self.chip.set_pins(Opn2Pins::default());
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// One address/value pair onto the bus queue; `port` is the register
    /// bank, the `a1` pin.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        let a1 = port & 1 != 0;
        self.writes.push_back(BusByte {
            a1,
            a0: false,
            data: (addr & 0xFF) as u8,
        });
        self.writes.push_back(BusByte {
            a1,
            a0: true,
            data: (data & 0xFF) as u8,
        });
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            for _ in 0..CLOCKS_PER_SAMPLE {
                self.master_clock();
            }
            // A and B are the stereo pair every card wires; C and D are the
            // second YAC512's, summed in so four-output material loses
            // nothing. Plain stereo songs leave C and D silent.
            frame[0] = self.held.a + self.held.c;
            frame[1] = self.held.b + self.held.d;
        }
    }
}

/// The chips this core serves: the whole OPL family, exactly as Nuked-OPL3
/// and the CQM do -- an OPL3 is an OPL2 with more of it, and OPL2-era songs
/// on OPL3 silicon are what the SB16 generation heard. (The Y8950's ADPCM is
/// not on this die; those rips lose their sample channel, the same stated
/// approximation every OPL3-family core makes.)
pub(crate) const CHIPS: [ChipKind; 4] = [
    ChipKind::Ymf262,
    ChipKind::Ym3812,
    ChipKind::Ym3526,
    ChipKind::Y8950,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The OPL3 master clock: four times the NTSC colourburst.
    const CLOCK: u32 = 14_318_180;

    /// The OPL2's own clock, as an OPL2 song's header carries it.
    const OPL2_CLOCK: u32 = 3_579_545;

    fn render(chip: &mut Ymf262Lle, frames: usize) -> Vec<(i32, i32)> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| (f[0], f[1])).collect()
    }

    fn energy(samples: &[(i32, i32)]) -> i64 {
        samples
            .iter()
            .map(|&(l, r)| i64::from(l.abs()) + i64::from(r.abs()))
            .sum()
    }

    /// A simple two-operator patch keyed on, OPL2-style: no NEW bit, no pan
    /// bits -- exactly what an OPL2 capture writes.
    fn key_on_opl2_style(chip: &mut Ymf262Lle) {
        chip.write(0, 0x20, 0x01); // modulator: multiple 1
        chip.write(0, 0x23, 0x01); // carrier: multiple 1
        chip.write(0, 0x40, 0x10); // modulator level
        chip.write(0, 0x43, 0x00); // carrier at full volume
        chip.write(0, 0x60, 0xF0); // fast attack
        chip.write(0, 0x63, 0xF0);
        chip.write(0, 0x80, 0x77); // sustain high, slow release
        chip.write(0, 0x83, 0x77);
        chip.write(0, 0xA0, 0x98); // frequency
        chip.write(0, 0xB0, 0x31); // block 2, key on
    }

    /// The same patch in OPL3 mode: NEW set, both output bits on.
    fn key_on_opl3_style(chip: &mut Ymf262Lle) {
        chip.write(1, 0x05, 0x01); // NEW: OPL3 mode
        chip.write(0, 0xC0, 0x30); // channel 0 to outputs A and B
        key_on_opl2_style(chip);
    }

    /// The die simulation must link, reset, take banked bus writes, and
    /// produce decodable serial streams on both sides.
    #[test]
    fn the_die_makes_sound_after_an_opl3_key_on() {
        let mut chip = Ymf262Lle::new();
        chip.reset(CLOCK, false);
        let quiet = energy(&render(&mut chip, 512));

        key_on_opl3_style(&mut chip);
        // The bus is slow by design: give the writes time to land, then
        // listen.
        render(&mut chip, 256);
        let loud = render(&mut chip, 2048);
        assert!(
            energy(&loud) > quiet * 4 && energy(&loud) > 10_000,
            "pin-level write, die, serial DAC decode -- one of them failed: \
             loud={} quiet={quiet}",
            energy(&loud)
        );
        // Both output bits are set, so both sides must carry the note.
        assert!(
            loud.iter().any(|&(l, _)| l != 0) && loud.iter().any(|&(_, r)| r != 0),
            "channel A feeds left and channel B right"
        );
    }

    /// An OPL2 song knows nothing of pan bits or the NEW flag; the real die
    /// in compat mode must still route its channels to the outputs, exactly
    /// as OPL2 games sounded on real OPL3 cards.
    #[test]
    fn an_opl2_style_song_still_sounds() {
        let mut chip = Ymf262Lle::new();
        chip.reset(OPL2_CLOCK, false);
        let quiet = energy(&render(&mut chip, 512));

        key_on_opl2_style(&mut chip);
        render(&mut chip, 256);
        let loud = energy(&render(&mut chip, 2048));
        assert!(
            loud > quiet * 4 && loud > 10_000,
            "the compat path must reach the DACs: loud={loud} quiet={quiet}"
        );
    }

    /// Both generations land on the OPL rate: an OPL3 clock divides by 288,
    /// an OPL2-generation clock is quadrupled by the die and divides by 72.
    #[test]
    fn the_native_rate_covers_both_generations() {
        let mut chip = Ymf262Lle::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 288);
        chip.reset(OPL2_CLOCK, false);
        assert_eq!(chip.native_rate(), OPL2_CLOCK / 72);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// A resting die transmits positive zero -- `0x8000`, one set bit whose
    /// position pins the 16-bit framing; a slip decodes loudly non-zero.
    #[test]
    fn a_resting_die_decodes_to_silence() {
        let mut chip = Ymf262Lle::new();
        chip.reset(CLOCK, false);
        render(&mut chip, 8); // let the reset transient clear
        let rest = render(&mut chip, 64);
        assert!(
            rest.iter().all(|&(l, r)| l == 0 && r == 0),
            "the idle stream must decode to exactly zero: {:?}",
            &rest[..8]
        );
    }

    /// The decode against a hand-built wire word: LSB first, sign inverted
    /// into bit 15, one trailing bit clock before the strobe's fall.
    #[test]
    fn the_dac_decode_is_the_wire_format() {
        fn encode(word: u16) -> u32 {
            let mut stream = 0u32;
            for i in 0..16 {
                stream = (stream << 1) | u32::from((word >> i) & 1);
            }
            // The trailing bit the strobe's fall lags the word by.
            stream << 1
        }

        assert_eq!(decode_yac512(encode(0x8000)), 0, "positive zero");
        assert_eq!(decode_yac512(encode(0x8005)), 5);
        assert_eq!(decode_yac512(encode(0x7FFB)), -5);
        assert_eq!(decode_yac512(encode(0xFFFF)), 32767, "positive rail");
        assert_eq!(decode_yac512(encode(0x0000)), -32768, "negative rail");
    }

    /// Chunking must not change the audio: the die's state advances one
    /// master clock at a time regardless of how the caller slices its pulls.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        let run = |chunk: usize| {
            let mut chip = Ymf262Lle::new();
            chip.reset(CLOCK, false);
            key_on_opl3_style(&mut chip);
            let mut out = vec![0i32; 512 * 2];
            for piece in out.chunks_mut(chunk * 2) {
                chip.render(piece);
            }
            out
        };
        assert_eq!(run(512), run(128));
    }
}
