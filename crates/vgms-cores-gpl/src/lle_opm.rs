//! YM2151-LLE as a [`ChipCore`]: the die itself, clocked pin by pin.
//!
//! Nuke.YKT's very-low-level emulators simulate the chip from its decapped die
//! shot, gate by gate, which buys an oracle to measure the fast cores against.
//! It costs speed -- the master clock runs two edges at a time through a
//! die-sized function -- so the registry entry is `realtime: false` and the
//! core is for offline render and the oracle diff, not playback.
//!
//! # Driving a die instead of an API
//!
//! There is no `write()` upstream; there is a bus. A register write asserts
//! chip-select and write-strobe (both active low) with the byte on the data
//! pins, holds them across a few master clocks, releases them, then leaves the
//! chip alone long enough to latch it -- enforced here by a pin-level state
//! machine.
//!
//! Audio leaves as a serial bit stream on the `SO` pin, framed by the two
//! sample-and-hold strobes, in the YM3012 DAC's floating-point format: a
//! ten-bit two's-complement mantissa (LSB first on the wire) and a three-bit
//! exponent. The decode to linear PCM here is the wrapper's, from the YM3012
//! datasheet.

use vgms_core::vgm::ChipKind;
use vgms_synth::ChipCore;
use std::collections::VecDeque;

use crate::ffi::{LlePins, OpmLleChip};

/// The registry id.
pub(crate) const CORE_ID: &str = "ym2151.lle";

/// Master clocks per output sample: 32 internal slots at clock/2.
const CLOCKS_PER_SAMPLE: u32 = 64;

/// Master clocks the bus signals are held asserted for one write.
const WRITE_HOLD: u32 = 8;

/// Master clocks of silence on the bus after a write before the next.
///
/// Hold + recover is 64 master clocks a byte (128 a register pair): deliberately
/// Nuked-OPM's pacing, not a hardware BUSY window, so both cores spread a write
/// burst over the same samples and the oracle diff lines up.
const WRITE_RECOVER: u32 = 56;

/// Master clocks with `IC` held low at reset -- the datasheet asks for at
/// least 24; a whole sample of them costs nothing offline.
const RESET_HOLD: u32 = 64;

/// One queued byte for the bus: `a0` low presents an address, high a value.
#[derive(Debug, Clone, Copy)]
struct BusByte {
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

/// The YM2151, as its own die computes it.
#[derive(Debug)]
pub struct Ym2151Lle {
    chip: OpmLleChip,
    rate: u32,
    writes: VecDeque<BusByte>,
    bus: Bus,
    /// Serial DAC capture, one per strobe: bits collected while the strobe
    /// is high, LSB first.
    capture: [DacCapture; 2],
    /// The last decoded stereo pair, held between strobes.
    held: [i32; 2],
}

/// Taps the serial stream and cuts words at a strobe's falling edges.
///
/// The strobe window is *shorter than the word*: the serial clock is half the
/// master clock, so thirteen serial bits span 26 master clocks against the
/// strobe's 16 -- the word runs into its window from behind, and only the
/// falling edge means anything, marking the word's end. So the tap remembers
/// the recent stream and decodes backwards from each falling edge.
#[derive(Debug, Default, Clone, Copy)]
struct DacCapture {
    strobe_was_high: bool,
    /// Master-rate samples of `SO`, most recent in bit 0.
    stream: u32,
}

impl DacCapture {
    /// Feeds one master clock's pin state; returns a decoded linear sample
    /// on the strobe's falling edge.
    fn feed(&mut self, strobe: bool, so: bool) -> Option<i32> {
        self.stream = (self.stream << 1) | u32::from(so);
        let decoded = (!strobe && self.strobe_was_high).then(|| decode_ym3012(self.stream));
        self.strobe_was_high = strobe;
        decoded
    }
}

/// The last serial word before a strobe's falling edge, to linear PCM.
///
/// Each serial bit occupies two master clocks, and the edge trails the word by
/// one master bit -- so serial bit `i` back from the edge sits at stream bit
/// `2i + 1`. The wire order is mantissa first, LSB first, then the exponent:
/// reading *backwards* from the edge gives E2 E1 E0, then D9 down to D0. The
/// mantissa is offset binary (512 is zero, what the idle chip transmits) and
/// linear is `(mantissa - 512) << (exponent - 1)`, full scale `+-511 << 6`.
/// The die transmits louder samples with larger exponents.
fn decode_ym3012(stream: u32) -> i32 {
    let bit = |i: u32| ((stream >> (2 * i + 1)) & 1) as i32;
    let exponent = (bit(0) << 2) | (bit(1) << 1) | bit(2);
    let mut mantissa = 0;
    for i in 0..10 {
        mantissa = (mantissa << 1) | bit(3 + i);
    }
    (mantissa - 512) << (exponent.max(1) - 1)
}

impl Ym2151Lle {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: OpmLleChip::new(),
            rate: 55_930,
            writes: VecDeque::new(),
            bus: Bus::Idle,
            capture: [DacCapture::default(); 2],
            held: [0; 2],
        }
    }

    /// One master clock: both edges, the bus state machine, and the DAC
    /// capture.
    fn master_clock(&mut self, pins_base: LlePins) {
        let mut pins = pins_base;
        match self.bus {
            Bus::Idle => {
                if let Some(byte) = self.writes.pop_front() {
                    pins.cs = false;
                    pins.wr = false;
                    pins.a0 = byte.a0;
                    pins.data = byte.data;
                    self.bus = Bus::Holding(WRITE_HOLD);
                    self.chip.set_pins(pins);
                }
            }
            Bus::Holding(left) => {
                // Keep the byte presented; the pins were set on entry.
                pins = LlePins::default();
                self.bus = if left > 1 {
                    Bus::Holding(left - 1)
                } else {
                    self.chip.set_pins(pins);
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

        let (sh1, sh2, so) = self.chip.dac_pins();
        if let Some(sample) = self.capture[0].feed(sh1, so) {
            self.held[0] = sample;
        }
        if let Some(sample) = self.capture[1].feed(sh2, so) {
            self.held[1] = sample;
        }
    }
}

impl Default for Ym2151Lle {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ym2151Lle {
    /// `variant` selects the YM2164 (OPP), which the die model carries as an
    /// input pin -- the two dies differ and both were decapped.
    fn reset(&mut self, clock: u32, variant: bool) {
        self.writes.clear();
        self.bus = Bus::Idle;
        self.capture = [DacCapture::default(); 2];
        self.held = [0; 2];
        self.rate = (clock / CLOCKS_PER_SAMPLE).max(1);

        self.chip.power_cycle();
        let mut pins = LlePins {
            ym2164: variant,
            ..LlePins::default()
        };
        // The electrical reset: IC low while the clock runs, then released.
        pins.ic = false;
        self.chip.set_pins(pins);
        for _ in 0..RESET_HOLD {
            self.chip.clock_edge(false);
            self.chip.clock_edge(true);
        }
        pins.ic = true;
        self.chip.set_pins(pins);
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// One address/value pair onto the bus queue.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        self.writes.push_back(BusByte {
            a0: false,
            data: (addr & 0xFF) as u8,
        });
        self.writes.push_back(BusByte {
            a0: true,
            data: (data & 0xFF) as u8,
        });
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            for _ in 0..CLOCKS_PER_SAMPLE {
                self.master_clock(LlePins::default());
            }
            // SH1 frames the right channel's word, SH2 the left's, as the
            // YM3012 datasheet wires them. Doubling matches Nuked-OPM's
            // OUTPUT_GAIN = 2 so the oracle diff reads level 1.0.
            frame[0] = self.held[1] * 2;
            frame[1] = self.held[0] * 2;
        }
    }
}

/// The chip this core serves.
pub(crate) const CHIPS: [ChipKind; 1] = [ChipKind::Ym2151];

#[cfg(test)]
mod tests {
    use super::*;

    /// The usual OPM clock in arcade machines.
    const CLOCK: u32 = 3_579_545;

    fn render(chip: &mut Ym2151Lle, frames: usize) -> Vec<(i32, i32)> {
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

    /// A simple patch keyed on: one operator, full level, both outputs.
    fn key_on(chip: &mut Ym2151Lle) {
        chip.write(0, 0x20, 0xC7); // channel 0: both outputs, feedback 0, alg 7
        chip.write(0, 0x28, 0x4A); // octave 4, note A
        chip.write(0, 0x80, 0x1F); // M1 attack rate: instant
        chip.write(0, 0x60, 0x00); // M1 total level: loudest
        chip.write(0, 0xE0, 0x0F); // M1 release
        chip.write(0, 0x08, 0x78); // key on channel 0, all slots
    }

    /// The die simulation must link, reset, take a bus write, and produce a
    /// decodable serial stream -- silence before, sound after.
    #[test]
    fn the_die_makes_sound_after_a_key_on() {
        let mut chip = Ym2151Lle::new();
        chip.reset(CLOCK, false);
        let quiet = energy(&render(&mut chip, 512));

        key_on(&mut chip);
        // The bus is slow by design: give the writes time to land, then
        // listen.
        render(&mut chip, 256);
        let loud = energy(&render(&mut chip, 2048));
        assert!(
            loud > quiet * 4 && loud > 10_000,
            "pin-level write, die, serial DAC decode -- one of them failed: \
             loud={loud} quiet={quiet}"
        );
    }

    /// `clock / 64`, the same rate Nuked-OPM declares -- the oracle and the
    /// shipping core must agree on time itself before anything else.
    #[test]
    fn the_native_rate_matches_the_shipping_core() {
        let mut chip = Ym2151Lle::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 64);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// Encodes (mantissa, exponent) as the wire would carry it: each serial
    /// bit doubled at master rate, D0 first, edge trailing by one bit.
    fn encode_ym3012(mantissa: u32, exponent: u32) -> u32 {
        let mut serial = Vec::new();
        for i in 0..10 {
            serial.push((mantissa >> i) & 1); // D0 first
        }
        for i in 0..3 {
            serial.push((exponent >> i) & 1); // then E0, E1, E2
        }
        let mut stream = 0u32;
        for bit in serial {
            // Oldest first: each new master bit shifts the stream left.
            stream = (stream << 1) | bit;
            stream = (stream << 1) | bit;
        }
        // The falling edge is read one master bit after the word ends.
        stream << 1
    }

    /// The decode against its own encode: offset-binary mantissa, trailing
    /// exponent-as-attenuation, the idle word exactly zero.
    #[test]
    fn the_dac_decode_is_the_wire_format() {
        assert_eq!(decode_ym3012(encode_ym3012(512, 1)), 0, "idle is zero");
        assert_eq!(decode_ym3012(encode_ym3012(1023, 7)), 511 << 6);
        assert_eq!(decode_ym3012(encode_ym3012(0, 7)), -512 << 6);
        assert_eq!(
            decode_ym3012(encode_ym3012(513, 1)),
            1,
            "exponent 1 is quietest"
        );
        assert_eq!(decode_ym3012(encode_ym3012(511, 2)), -2);
    }

    /// A resting die transmits mantissa 512, exponent 1 -- linear zero. If
    /// the framing here slipped by even one bit, this is the test that
    /// notices, because 512 is `1000000000` and a shifted read of it is
    /// loudly not zero.
    #[test]
    fn a_resting_die_decodes_to_silence() {
        let mut chip = Ym2151Lle::new();
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
