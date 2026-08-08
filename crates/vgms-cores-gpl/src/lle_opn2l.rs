//! YMF276-LLE as a [`ChipCore`]: the OPN2's *serial-DAC* die, clocked pin by
//! pin.
//!
//! A different decap from the YM2612 die that `lle_opn2` wraps: the YMF276
//! (OPN2L) computes the same FM but leaves the chip through an external
//! serial DAC instead of the YM2612's internal nine-bit ladder -- no ladder
//! asymmetry, no `+-1` sign residue. That makes it the die-level witness for
//! the *clean* half of the OPN2 family, beside the 2612 die's famously dirty
//! one. (The upstream can also build the YM3438 configuration, but only the
//! YMF276's DAC reaches output pins there, so this wrapper runs the YMF276
//! configuration for either variant -- a stated approximation.)
//!
//! `realtime: false`, like every die: below realtime on today's CPUs, and
//! honoured anyway when chosen -- playback included.
//!
//! # The serial interface
//!
//! Audio leaves as 16-bit two's-complement words, MSB first, on the `SO`
//! data line -- paced by the `BCO` bit clock, cut into words by the `WCO`
//! word clock's transitions, sides told apart by the `LRO` level; see
//! [`DacCapture`] for the framing, pinned by probe.

use std::collections::VecDeque;
use vgms_core::vgm::ChipKind;
use vgms_synth::ChipCore;

use crate::ffi::{Opn2Pins, Opn2lDacPins, Opn2lLleChip};

/// The registry id.
pub(crate) const CORE_ID: &str = "ym2612.ymf276-lle";

/// Master clocks per output sample: 24 internal slots at clock/6.
const CLOCKS_PER_SAMPLE: u32 = 144;

/// Master clocks the bus signals are held asserted for one byte.
const WRITE_HOLD: u32 = 8;

/// Master clocks of bus silence after an address byte, hold included 48 --
/// the shared-data-latch commit margin the slower-prescale dies need.
/// Address writes do not raise the die's BUSY flag, so this stays short.
const ADDRESS_RECOVER: u32 = 48 - WRITE_HOLD;

/// Master clocks of bus silence after a value byte, hold included 288: this
/// die drops writes while its BUSY counter runs, thirty-two internal cycles
/// (192 master clocks at the /6 prescale) after every data write -- the real
/// chip's infamous wait, which real drivers poll the status register for.
/// Two whole samples clears it with margin.
const VALUE_RECOVER: u32 = (2 * CLOCKS_PER_SAMPLE) - WRITE_HOLD;

/// Master clocks with `IC` held low at reset -- two whole samples.
const RESET_HOLD: u32 = 2 * CLOCKS_PER_SAMPLE;

/// One queued byte for the bus.
#[derive(Debug, Clone, Copy)]
struct BusByte {
    /// Bank select: part I or part II registers.
    a1: bool,
    /// Address or value.
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

/// Taps the serial stream: a word starts at each word clock's *rising* edge.
///
/// Unlike the float-format dies, this stream is MSB first: the die's shifter
/// shifts *left*, presenting bit 15, so the first bit after the word clock
/// rises is the sign. Pinned by probe: the idle die transmits `0xFFF0` (the
/// OPN2's small idle DC, minus sixteen) as exactly twelve ones then four
/// zeros starting flush with the rise -- two words per sample, 24 bit clocks
/// apart, `LRO`'s level at the rise telling the sides apart.
#[derive(Debug, Default, Clone, Copy)]
struct DacCapture {
    bco_was_high: bool,
    wco_was_high: bool,
    /// The word being collected, oldest bit at the top; `None` between words.
    collecting: Option<(u32, u8)>,
    /// `LRO` as latched at the word clock's rise.
    word_lro: bool,
}

/// One sample's two decoded sides.
#[derive(Debug, Default, Clone, Copy)]
struct Sides {
    left: i32,
    right: i32,
}

impl DacCapture {
    /// Feeds one master clock's pin state into `held`, decoding a word
    /// sixteen bit clocks after each word-clock rise.
    fn feed(&mut self, pins: Opn2lDacPins, held: &mut Sides) {
        if pins.wco && !self.wco_was_high {
            self.collecting = Some((0, 0));
            self.word_lro = pins.lro;
        }
        self.wco_was_high = pins.wco;

        if pins.bco
            && !self.bco_was_high
            && let Some((bits, count)) = self.collecting
        {
            let bits = (bits << 1) | u32::from(pins.so);
            let count = count + 1;
            if count == 16 {
                let sample = i32::from(bits as u16 as i16);
                // LRO high at the rise frames the left word -- the pan test
                // is what pins this polarity.
                if self.word_lro {
                    held.left = sample;
                } else {
                    held.right = sample;
                }
                self.collecting = None;
            } else {
                self.collecting = Some((bits, count));
            }
        }
        self.bco_was_high = pins.bco;
    }
}

/// The YMF276, as its own die computes it.
#[derive(Debug)]
pub struct Ymf276Lle {
    chip: Opn2lLleChip,
    rate: u32,
    writes: VecDeque<BusByte>,
    bus: Bus,
    capture: DacCapture,
    held: Sides,
}

impl Ymf276Lle {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: Opn2lLleChip::new(),
            rate: 53_267,
            writes: VecDeque::new(),
            bus: Bus::Idle,
            capture: DacCapture::default(),
            held: Sides::default(),
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
                    self.bus = Bus::Holding(WRITE_HOLD | (u32::from(byte.a0) << 31));
                }
            }
            Bus::Holding(state) => {
                let left = state & !(1 << 31);
                let was_value = state & (1 << 31) != 0;
                self.bus = if left > 1 {
                    Bus::Holding((left - 1) | (u32::from(was_value) << 31))
                } else {
                    self.chip.set_pins(Opn2Pins::default());
                    Bus::Recovering(if was_value {
                        VALUE_RECOVER
                    } else {
                        ADDRESS_RECOVER
                    })
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

impl Default for Ymf276Lle {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ymf276Lle {
    /// `variant` names the YM3438; this upstream only drives the YMF276
    /// configuration's DAC out on pins, so both variants run that die -- a
    /// stated approximation, the mirror of the 2612 die rendering a YM3438.
    fn reset(&mut self, clock: u32, _variant: bool) {
        self.writes.clear();
        self.bus = Bus::Idle;
        self.capture = DacCapture::default();
        self.held = Sides::default();
        self.rate = (clock / CLOCKS_PER_SAMPLE).max(1);

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

    /// `port` selects the register bank; each write is an address byte then
    /// a value byte on the bus.
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
            frame[0] = self.held.left;
            frame[1] = self.held.right;
        }
    }
}

/// The chip this core serves.
pub(crate) const CHIPS: [ChipKind; 1] = [ChipKind::Ym2612];

#[cfg(test)]
mod tests {
    use super::*;

    /// The Mega Drive's OPN2 clock.
    const CLOCK: u32 = 7_670_453;

    fn render(chip: &mut Ymf276Lle, frames: usize) -> Vec<(i32, i32)> {
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

    fn side_energy(samples: &[(i32, i32)]) -> (i64, i64) {
        samples.iter().fold((0, 0), |(l, r), &(sl, sr)| {
            (l + i64::from(sl.abs()), r + i64::from(sr.abs()))
        })
    }

    /// A loud FM note on channel 1, algorithm 7, all four operators
    /// configured, panned as `pan` (register `0xB4`'s top bits).
    fn key_on_fm(chip: &mut Ymf276Lle, pan: u16) {
        for (reg, value) in [
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
            (0xB0, 0x07), // algorithm 7
            (0xB4, pan),
            (0xA4, 0x22), // block, F-number high
            (0xA0, 0x69), // F-number low
            (0x28, 0xF0), // key on channel 1, all slots
        ] {
            chip.write(0, reg, value);
        }
    }

    /// The die must link, reset, take banked writes and put sound on both
    /// sides through the serial interface.
    #[test]
    fn the_die_makes_sound_on_both_sides() {
        let mut chip = Ymf276Lle::new();
        chip.reset(CLOCK, false);
        let quiet = energy(&render(&mut chip, 256));

        key_on_fm(&mut chip, 0xC0); // both speakers
        render(&mut chip, 128);
        let loud = render(&mut chip, 1024);
        let (left, right) = side_energy(&loud);
        assert!(
            energy(&loud) > quiet * 4 && energy(&loud) > 10_000,
            "pin-level write, die, serial decode -- one of them failed: \
             loud={} quiet={quiet}",
            energy(&loud)
        );
        assert!(
            left > 10_000 && right > 10_000,
            "a centre-panned note must reach both sides: left={left} right={right}"
        );
    }

    /// The pan bits must land on the sides they name -- this is the test
    /// that pins which `LRO` level frames the left word.
    #[test]
    fn a_left_panned_note_stays_left() {
        let mut chip = Ymf276Lle::new();
        chip.reset(CLOCK, false);
        key_on_fm(&mut chip, 0x80); // left only
        render(&mut chip, 128);
        let (left, right) = side_energy(&render(&mut chip, 1024));
        assert!(
            left > right * 8 && left > 10_000,
            "the left-panned note leaked: left={left} right={right}"
        );
    }

    /// `clock / 144`, the OPN2's own rate -- 53267 Hz on a Mega Drive.
    #[test]
    fn the_native_rate_is_the_opn2s_own() {
        let mut chip = Ymf276Lle::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 144);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// A freshly reset die transmits all-zero words -- exactly zero decoded.
    /// (Once slots have been touched the die idles at its small DC instead,
    /// so the *note* tests above are what pin the framing; this pins that
    /// the quiet path is exactly quiet.)
    #[test]
    fn a_resting_die_decodes_to_silence() {
        let mut chip = Ymf276Lle::new();
        chip.reset(CLOCK, false);
        render(&mut chip, 8); // let the reset transient clear
        let rest = render(&mut chip, 64);
        assert!(
            rest.iter().all(|&(l, r)| l == 0 && r == 0),
            "the idle stream must decode to exactly zero: {:?}",
            &rest[..8]
        );
    }
}
