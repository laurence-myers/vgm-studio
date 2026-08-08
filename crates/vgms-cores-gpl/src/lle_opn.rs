//! YM2203-LLE as a [`ChipCore`]: the OPN die itself, clocked pin by pin.
//!
//! The first alternative core the YM2203 has had here -- libvgm alone served
//! it -- and it comes from John McMaster's YM2203C decap. Three FM channels
//! and the licensed SSG on one die: FM leaves as the YM3014's floating-point
//! serial stream on the `OPO` pin, and the SSG's three channels leave on
//! analog pins the shim sums, scaled into the mix as the OPNA die's wrapper
//! scales its SSG.
//!
//! `realtime: false`, like every die: below realtime on today's CPUs, and
//! honoured anyway when chosen -- playback included.
//!
//! # Driving a die instead of an API
//!
//! The bus is the OPM's shape: one address line, a byte of data, chip-select
//! and write-strobe held across master clocks by the pin-level state machine
//! here, with the SSG's GPIO ports tied low as an unwired sound board leaves
//! them. The default prescaler (a clean /6 out of reset) makes one FM sample
//! every 72 master clocks; a song that reprograms the prescaler shifts the
//! die's own cadence under a fixed output rate, a stated approximation.

use std::collections::VecDeque;
use vgms_core::vgm::ChipKind;
use vgms_synth::ChipCore;

use crate::ffi::{Opl2Pins, OpnLleChip};

/// The registry id.
pub(crate) const CORE_ID: &str = "ym2203.lle";

/// Master clocks per output sample: 12 internal slots at the default clock/6
/// prescale.
const CLOCKS_PER_SAMPLE: u32 = 72;

/// Master clocks the bus signals are held asserted for one byte.
const WRITE_HOLD: u32 = 8;

/// Master clocks of bus silence after an address byte, hold included 48: the
/// commit chain runs at the /6 prescale, and the data latch is shared between
/// address and value bytes -- the same race the OPL2 die demonstrated, slower.
const ADDRESS_RECOVER: u32 = 48 - WRITE_HOLD;

/// Master clocks of bus silence after a value byte: past the datasheet's
/// 83-cycle worst case for the FM registers.
const VALUE_RECOVER: u32 = 96 - WRITE_HOLD;

/// Master clocks with `IC` held low at reset -- two whole samples.
const RESET_HOLD: u32 = 2 * CLOCKS_PER_SAMPLE;

/// Scales the analog (SSG) pins' average into the serial DAC's range -- the
/// same figure the OPNA die's wrapper uses, for the same `volume_lut`.
const SSG_SCALE: f32 = 8000.0;

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
/// The bit clock (`SY`) is the die's analog clock; the tap shifts only on its
/// rising edges, as the YM3014 on a real board does.
#[derive(Debug, Default, Clone, Copy)]
struct DacCapture {
    strobe_was_high: bool,
    sy_was_high: bool,
    /// Bit-clock samples of `OPO`, most recent in bit 0.
    stream: u32,
}

impl DacCapture {
    /// Feeds one master clock's pin state; returns a decoded linear sample
    /// on the strobe's falling edge.
    fn feed(&mut self, strobe: bool, opo: bool, sy: bool) -> Option<i32> {
        if sy && !self.sy_was_high {
            self.stream = (self.stream << 1) | u32::from(opo);
        }
        self.sy_was_high = sy;
        let decoded = (!strobe && self.strobe_was_high).then(|| decode_ym3014(self.stream));
        self.strobe_was_high = strobe;
        decoded
    }
}

/// The last serial word before the strobe's falling edge, to linear PCM.
///
/// The YM3012-family float, as on the OPL2 die: mantissa `D0..D9` LSB first
/// (512 meaning zero), then the exponent `E0 E1 E2`, `linear = (mantissa -
/// 512) << (exponent - 1)`, the word ending at the strobe's fall -- pinned by
/// the idle-decodes-to-zero probe.
fn decode_ym3014(stream: u32) -> i32 {
    let bit = |i: u32| ((stream >> i) & 1) as i32;
    let exponent = (bit(0) << 2) | (bit(1) << 1) | bit(2);
    let mut mantissa = 0;
    for i in 0..10 {
        mantissa = (mantissa << 1) | bit(3 + i);
    }
    (mantissa - 512) << (exponent.max(1) - 1)
}

/// The YM2203, as its own die computes it.
#[derive(Debug)]
pub struct Ym2203Lle {
    chip: OpnLleChip,
    rate: u32,
    writes: VecDeque<BusByte>,
    bus: Bus,
    capture: DacCapture,
    /// The last decoded FM sample, held between strobes.
    held: i32,
}

impl Ym2203Lle {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: OpnLleChip::new(),
            rate: 55_466,
            writes: VecDeque::new(),
            bus: Bus::Idle,
            capture: DacCapture::default(),
            held: 0,
        }
    }

    /// One master clock: the bus state machine, both edges, the DAC tap.
    /// Returns the SSG analog level this clock.
    fn master_clock(&mut self) -> f32 {
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

        let (sh, opo, sy, analog) = self.chip.dac_pins();
        if let Some(sample) = self.capture.feed(sh, opo, sy) {
            self.held = sample;
        }
        analog
    }
}

impl Default for Ym2203Lle {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ym2203Lle {
    /// `variant` carries nothing for this chip.
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

    /// One address/value pair onto the bus queue. The YM2203 has one register
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
            let mut analog_sum = 0.0f32;
            for _ in 0..CLOCKS_PER_SAMPLE {
                analog_sum += self.master_clock();
            }
            // A mono chip: FM from the serial DAC, the SSG averaged over the
            // sample and scaled into the same range, to both sides.
            let ssg = (analog_sum / CLOCKS_PER_SAMPLE as f32 * SSG_SCALE) as i32;
            frame[0] = self.held + ssg;
            frame[1] = self.held + ssg;
        }
    }
}

/// The chip this core serves.
pub(crate) const CHIPS: [ChipKind; 1] = [ChipKind::Ym2203];

#[cfg(test)]
mod tests {
    use super::*;

    /// The PC-88's OPN clock.
    const CLOCK: u32 = 3_993_600;

    fn render(chip: &mut Ym2203Lle, frames: usize) -> Vec<(i32, i32)> {
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

    /// A loud FM note on channel 1, algorithm 7, all four operators
    /// configured -- the die needs the full patch, as its OPNA sibling does.
    fn key_on_fm(chip: &mut Ym2203Lle) {
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
            (0xA4, 0x22), // block, F-number high
            (0xA0, 0x69), // F-number low
            (0x28, 0xF0), // key on channel 1, all slots
        ] {
            chip.write(0, reg, value);
        }
    }

    /// The die must link, reset, take writes and make FM sound.
    #[test]
    fn the_die_makes_fm_sound_after_a_key_on() {
        let mut chip = Ym2203Lle::new();
        chip.reset(CLOCK, false);
        let quiet = energy(&render(&mut chip, 256));

        key_on_fm(&mut chip);
        render(&mut chip, 128);
        let loud = energy(&render(&mut chip, 1024));
        assert!(
            loud > quiet * 4 && loud > 10_000,
            "pin-level write, die, serial decode -- one of them failed: \
             loud={loud} quiet={quiet}"
        );
    }

    /// The licensed SSG on the die must sound through the analog pins.
    #[test]
    fn the_ssg_half_makes_sound() {
        let mut chip = Ym2203Lle::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0x00, 0xFE); // tone A period, low
        chip.write(0, 0x01, 0x00);
        chip.write(0, 0x07, 0x3E); // mixer: tone A on, the rest off
        chip.write(0, 0x08, 0x0F); // channel A: full volume
        let quiet = energy(&render(&mut chip, 64));

        render(&mut chip, 64);
        let loud = energy(&render(&mut chip, 1024));
        assert!(
            loud > quiet * 4 && loud > 10_000,
            "the SSG must reach the analog pins: loud={loud} quiet={quiet}"
        );
    }

    /// `clock / 72` at the default /6 prescale -- 55.5 kHz on a PC-88.
    #[test]
    fn the_native_rate_is_the_opns_own() {
        let mut chip = Ym2203Lle::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 72);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// A resting die transmits mantissa 512, exponent 1 -- linear zero; a
    /// framing slip decodes loudly non-zero.
    #[test]
    fn a_resting_die_decodes_to_silence() {
        let mut chip = Ym2203Lle::new();
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
