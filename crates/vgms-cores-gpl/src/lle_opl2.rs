//! YM3812-LLE as a [`ChipCore`]: the OPL2 die itself, clocked pin by pin.
//!
//! The DRO heartland's own silicon: every AdLib and every Sound Blaster
//! before the 16 carried this exact die, and this simulates it from the
//! decap, gate by gate. A genuine OPL2 is not "an OPL3 with the extras off" --
//! composite sine mode and the DAC's behaviour differ -- so this is the
//! authenticity option for OPL2-era captures, beside the faithful-but-modelled
//! Nuked-OPL3.
//!
//! `realtime: false`, like every die: below realtime on today's CPUs, and
//! honoured anyway when chosen -- playback included.
//!
//! # Driving a die instead of an API
//!
//! As with the OPM die: there is no `write()` upstream, there is a bus. A
//! register write asserts chip-select and write-strobe with the byte on the
//! data pins, holds them, releases them, then leaves the chip alone long
//! enough to take it -- the YM3812's datasheet asks roughly 12 master clocks
//! after an address byte and 84 after a data byte, enforced here by the same
//! pin-level state machine as its siblings.
//!
//! Audio leaves as a serial bit stream on the `MO` pin in the YM3014B DAC's
//! floating-point format, paced by the `SY` bit clock (one bit per internal
//! cycle, a quarter of the master clock) and framed by the `SH` strobe; see
//! [`decode_ym3014`] for the wire order, which is *not* the YM3012's.

use std::collections::VecDeque;
use vgms_core::vgm::ChipKind;
use vgms_synth::ChipCore;

use crate::ffi::{Opl2LleChip, Opl2Pins};

/// The registry id: the OPL family shares the `opl3` slot, so this names the
/// die within it.
pub(crate) const CORE_ID: &str = "opl3.ym3812-lle";

/// Master clocks per output sample: 18 internal slots at clock/4.
const CLOCKS_PER_SAMPLE: u32 = 72;

/// Master clocks the bus signals are held asserted for one byte.
const WRITE_HOLD: u32 = 8;

/// Master clocks of bus silence after an address byte, hold included 32.
///
/// Wider than the datasheet's twelve-cycle address wait, and necessarily so:
/// the die commits a write through a latch chain clocked at clock/4 (about
/// sixteen master clocks end to end), and `data_latch` is shared between the
/// address and value bytes -- presenting the value before the address commit
/// has consumed the latch decodes the wrong byte as the register select.
const ADDRESS_RECOVER: u32 = 32 - WRITE_HOLD;

/// Master clocks of bus silence after a value byte: the datasheet's
/// eighty-four-cycle data-write wait, hold included.
const VALUE_RECOVER: u32 = 84 - WRITE_HOLD;

/// Master clocks with `IC` held low at reset -- two whole samples, far past
/// the prescaler and FSM reset chains, and free offline.
const RESET_HOLD: u32 = 2 * CLOCKS_PER_SAMPLE;

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
    /// Holding the byte; bit 31 remembers it was a value byte, which recovers
    /// longer than an address byte.
    Holding(u32),
    Recovering(u32),
}

/// Taps the serial stream and cuts words at the strobe's falling edge.
///
/// The bit clock (`SY`) runs at a quarter of the master clock, so sampling at
/// master rate would smear the word; the tap shifts only on the bit clock's
/// rising edges, as the real YM3014B on the board does.
#[derive(Debug, Default, Clone, Copy)]
struct DacCapture {
    strobe_was_high: bool,
    sy_was_high: bool,
    /// Bit-clock samples of `MO`, most recent in bit 0.
    stream: u32,
}

impl DacCapture {
    /// Feeds one master clock's pin state; returns a decoded linear sample
    /// on the strobe's falling edge.
    fn feed(&mut self, strobe: bool, mo: bool, sy: bool) -> Option<i32> {
        if sy && !self.sy_was_high {
            self.stream = (self.stream << 1) | u32::from(mo);
        }
        self.sy_was_high = sy;
        let decoded = (!strobe && self.strobe_was_high).then(|| decode_ym3014(self.stream));
        self.strobe_was_high = strobe;
        decoded
    }
}

/// The last serial word before the strobe's falling edge, to linear PCM.
///
/// The same wire format as the OPM's YM3012, one channel: a ten-bit
/// offset-binary mantissa LSB first (`D0..D9`, 512 meaning zero, `D9` the
/// sign), then the exponent LSB first (`E0 E1 E2`), `linear = (mantissa -
/// 512) << (exponent - 1)`. One bit per `SY` bit clock, and the word's last
/// bit (`E2`) is the last bit captured while the strobe is high -- pinned by
/// probe: the idle die transmits exactly two set bits, `D9` and `E0`
/// (mantissa 512, exponent 1, linear zero), three bit clocks before the
/// strobe falls. Reading *backwards* from the edge gives `E2 E1 E0`, `D9..D0`.
fn decode_ym3014(stream: u32) -> i32 {
    let bit = |i: u32| ((stream >> i) & 1) as i32;
    let exponent = (bit(0) << 2) | (bit(1) << 1) | bit(2);
    let mut mantissa = 0;
    for i in 0..10 {
        mantissa = (mantissa << 1) | bit(3 + i);
    }
    (mantissa - 512) << (exponent.max(1) - 1)
}

/// The YM3812, as its own die computes it.
#[derive(Debug)]
pub struct Ym3812Lle {
    chip: Opl2LleChip,
    rate: u32,
    writes: VecDeque<BusByte>,
    bus: Bus,
    capture: DacCapture,
    /// The last decoded sample, held between strobes.
    held: i32,
}

impl Ym3812Lle {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: Opl2LleChip::new(),
            rate: 49_716,
            writes: VecDeque::new(),
            bus: Bus::Idle,
            capture: DacCapture::default(),
            held: 0,
        }
    }

    /// One master clock: the bus state machine, both edges, the DAC tap.
    fn master_clock(&mut self) {
        match self.bus {
            Bus::Idle => {
                if let Some(byte) = self.writes.pop_front() {
                    self.chip.set_pins(Opl2Pins {
                        cs: false,
                        wr: false,
                        a0: byte.a0,
                        data: byte.data,
                        ..Opl2Pins::default()
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
                    self.chip.set_pins(Opl2Pins::default());
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

        let (sh, mo, sy) = self.chip.dac_pins();
        if let Some(sample) = self.capture.feed(sh, mo, sy) {
            self.held = sample;
        }
    }
}

impl Default for Ym3812Lle {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ym3812Lle {
    /// `variant` carries nothing for this family; there is one YM3812 die.
    fn reset(&mut self, clock: u32, _variant: bool) {
        self.writes.clear();
        self.bus = Bus::Idle;
        self.capture = DacCapture::default();
        self.held = 0;
        self.rate = (clock / CLOCKS_PER_SAMPLE).max(1);

        self.chip.power_cycle();
        // The electrical reset: IC low while the clock runs, then released.
        self.chip.set_pins(Opl2Pins {
            ic: false,
            ..Opl2Pins::default()
        });
        for _ in 0..RESET_HOLD {
            self.chip.clock_edge(false);
            self.chip.clock_edge(true);
        }
        self.chip.set_pins(Opl2Pins::default());
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// One address/value pair onto the bus queue. The OPL2 has one register
    /// bank, so `port` carries nothing.
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
                self.master_clock();
            }
            // A mono chip: the one DAC word to both sides. Doubled to sit at
            // the same level as Nuked-OPL3's output for the same material.
            frame[0] = self.held * 2;
            frame[1] = self.held * 2;
        }
    }
}

/// The chips this core serves: the YM3812 itself, and its register-compatible
/// family -- the YM3526 is this register file minus waveform select, and the
/// Y8950 is a YM3526 with ADPCM bolted on (which this die does not have; those
/// rips lose their sample channel, a stated approximation). The YMF262 is
/// *not* here: an OPL3 song needs the second register bank this die lacks.
pub(crate) const CHIPS: [ChipKind; 3] = [ChipKind::Ym3812, ChipKind::Ym3526, ChipKind::Y8950];

#[cfg(test)]
mod tests {
    use super::*;

    /// The usual OPL2 clock: NTSC colourburst, as on every AdLib card.
    const CLOCK: u32 = 3_579_545;

    fn render(chip: &mut Ym3812Lle, frames: usize) -> Vec<(i32, i32)> {
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

    /// A simple two-operator patch keyed on: full level, fast attack.
    fn key_on(chip: &mut Ym3812Lle) {
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

    /// The die simulation must link, reset, take a bus write, and produce a
    /// decodable serial stream -- silence before, sound after.
    #[test]
    fn the_die_makes_sound_after_a_key_on() {
        let mut chip = Ym3812Lle::new();
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

    /// `clock / 72`, the OPL2's own rate -- 49716 Hz at the NTSC clock, the
    /// same figure every OPL core in the app declares.
    #[test]
    fn the_native_rate_is_the_opl2s_own() {
        let mut chip = Ym3812Lle::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 72);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// A resting die transmits mantissa 512, exponent 1 -- linear zero. The
    /// only set bit in the whole idle stream is the sign, so if the framing
    /// here slipped by even one bit clock this decodes loudly non-zero.
    #[test]
    fn a_resting_die_decodes_to_silence() {
        let mut chip = Ym3812Lle::new();
        chip.reset(CLOCK, false);
        render(&mut chip, 8); // let the reset transient clear
        let rest = render(&mut chip, 64);
        assert!(
            rest.iter().all(|&(l, r)| l == 0 && r == 0),
            "the idle stream must decode to exactly zero: {:?}",
            &rest[..8]
        );
    }

    /// The decode against a hand-built wire word: mantissa LSB first, then
    /// the exponent LSB first, word ending at the strobe's fall.
    #[test]
    fn the_dac_decode_is_the_wire_format() {
        // Builds the stream as the wire carries it: D0..D9, E0, E1, E2,
        // oldest bit leftmost, the last bit landing at stream bit 0.
        fn encode(mantissa: u32, exponent: u32) -> u32 {
            let mut stream = 0u32;
            let mut push = |bit: u32| stream = (stream << 1) | (bit & 1);
            for i in 0..10 {
                push(mantissa >> i); // D0..D9
            }
            for i in 0..3 {
                push(exponent >> i); // E0, E1, E2
            }
            stream
        }

        assert_eq!(decode_ym3014(encode(512, 1)), 0, "idle is zero");
        assert_eq!(decode_ym3014(encode(1023, 7)), 511 << 6);
        assert_eq!(decode_ym3014(encode(0, 7)), -512 << 6);
        assert_eq!(decode_ym3014(encode(513, 1)), 1, "exponent 1 is quietest");
        assert_eq!(decode_ym3014(encode(511, 2)), -2);
    }

    /// Chunking must not change the audio: the die's state advances one
    /// master clock at a time regardless of how the caller slices its pulls.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        let run = |chunk: usize| {
            let mut chip = Ym3812Lle::new();
            chip.reset(CLOCK, false);
            key_on(&mut chip);
            let mut out = vec![0i32; 512 * 2];
            for piece in out.chunks_mut(chunk * 2) {
                chip.render(piece);
            }
            out
        };
        assert_eq!(run(512), run(128));
    }
}
