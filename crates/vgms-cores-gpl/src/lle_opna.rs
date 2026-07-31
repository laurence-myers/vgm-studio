//! The YM2608 die as a [`ChipCore`], clocked pin by pin -- memory included.
//!
//! The drums live in the chip's internal mask ROM, which a VGM does not carry,
//! so the clean-room YM2608 plays them silent. The decap does carry it:
//! `fmopna_rom.h` in this submodule is that ROM read off the die, so this core
//! renders a YM2608 rip complete. (The 2610 die was the original target but its
//! configuration of the upstream does not compile -- unguarded 2608-only GPIO
//! writes -- so the 2608 die is the family's OPNA witness until that heals.)
//!
//! The harness serves the package's external memory: the Delta-T sample store
//! is DRAM on a RAS/CAS-multiplexed nine-bit bus, addresses out and data either
//! direction -- the die writes it through register `$08` and reads it back at
//! playback, so the wrapper is a memory chip, holding whatever the VGM's `0x81`
//! data blocks preloaded and whatever the die stores. FM, rhythm and ADPCM
//! leave on the serial DAC as one 16-bit linear word per side per sample -- the
//! YM3016's offset-binary format, LSB first, *not* the OPM's floating point;
//! see [`decode_serial`] -- and the SSG leaves on the analog pin, scaled into
//! the mix.
//!
//! `realtime: false`, like every die: render and oracle only.

use std::collections::VecDeque;
use vgms_core::vgm::ChipKind;
use vgms_synth::ChipCore;

use crate::ffi::{DramBus, Opn2Pins, OpnaLleChip};

/// The registry id.
pub(crate) const CORE_ID: &str = "ym2608.lle";

/// Master clocks per output sample: 24 internal slots at clock/6, as the
/// shipping core declares for this kind.
const CLOCKS_PER_SAMPLE: u32 = 144;

/// Master clocks the bus signals are held asserted for one byte.
const WRITE_HOLD: u32 = 8;

/// Master clocks of bus silence after an address byte.
const ADDRESS_RECOVER: u32 = 4;

/// Master clocks of bus silence after a value byte: pair time equals the
/// shipping core's sample-period pacing.
const VALUE_RECOVER: u32 = CLOCKS_PER_SAMPLE - (2 * WRITE_HOLD) - ADDRESS_RECOVER;

/// Master clocks with `IC` held low at reset.
const RESET_HOLD: u32 = 288;

/// The Delta-T DRAM: nine row bits and nine column bits, 256 KiB -- the
/// largest arrangement the bus can address in its 8-bit-plus-A8 form.
const DRAM_SIZE: usize = 1 << 18;

/// One queued byte for the write bus.
#[derive(Debug, Clone, Copy)]
struct BusByte {
    a1: bool,
    a0: bool,
    data: u8,
}

/// Where the write-bus state machine is.
#[derive(Debug, Clone, Copy)]
enum Bus {
    Idle,
    Holding(u32),
    Recovering(u32),
}

/// Serves the Delta-T DRAM: latches the multiplexed address halves on the
/// strobes' asserting edges, stores on write cycles, serves on reads.
#[derive(Debug, Default)]
struct DramPort {
    row: u32,
    column: u32,
    ras_was: bool,
    cas_was: bool,
}

impl DramPort {
    /// Feeds one clock's bus state against the memory; returns the byte to
    /// keep on the data-in lines.
    fn feed(&mut self, bus: DramBus, memory: &mut [u8]) -> u8 {
        let lines = (bus.dm as u32 & 0xFF) | (u32::from(bus.a8) << 8);
        if bus.ras && !self.ras_was {
            self.row = lines;
        }
        if bus.cas && !self.cas_was {
            self.column = lines;
        }
        self.ras_was = bus.ras;
        self.cas_was = bus.cas;

        // Row-major, as the die's address counter fills the bus: the low
        // half rides RAS, the high half CAS. If that order is ever wrong it
        // shows up as 512-byte-page scrambling in the oracle, not here.
        let address = ((self.column << 9) | self.row) as usize % memory.len().max(1);
        if bus.cas && bus.we && !bus.reading {
            memory[address] = (bus.dm & 0xFF) as u8;
        }
        memory[address]
    }
}

/// Taps the serial stream and cuts words at a strobe's falling edge.
///
/// This package gates its bit clock (`o_s`): unlike the OPM die's steady
/// half-master serial line, sampling at master rate here smears the word across
/// a shifting alignment. The tap shifts only on the bit clock's rising edges,
/// as the real YM3016 on the board does.
#[derive(Debug, Default, Clone, Copy)]
struct DacCapture {
    strobe_was_high: bool,
    s_was_high: bool,
    stream: u32,
}

impl DacCapture {
    fn feed(&mut self, strobe: bool, so: bool, s: bool) -> Option<i32> {
        if s && !self.s_was_high {
            self.stream = (self.stream << 1) | u32::from(so);
        }
        self.s_was_high = s;
        let decoded = (!strobe && self.strobe_was_high).then(|| decode_serial(self.stream));
        self.strobe_was_high = strobe;
        decoded
    }
}

/// The serial word before a falling edge, to linear PCM.
///
/// Not the OPM's float format. The die loads its 18-bit accumulator (FM +
/// rhythm + Delta-T, already summed per side) into the shifter as a *16-bit
/// linear* word -- low 15 bits of the sum, the sign inverted into bit 15,
/// saturated by the shifter's clip gates -- and shifts it out LSB first
/// (`fmopna_impl.c`: `ac_shifter[0] = ac_shifter[1] >> 1`). That is the
/// YM3016's offset-binary input, where the OPM's YM3012 took a
/// three-bit-exponent float.
///
/// Framing, pinned by probe: 24 bit clocks per half-frame; the word rides bits
/// 7..22, the strobe rises at bit 16 and falls at bit 24, so at the falling
/// edge the stream holds one trailing bit and then the word, bit-reversed. The
/// idle die shifts exactly one set bit (positive zero's inverted sign) two
/// clocks before the fall, which pins the alignment.
fn decode_serial(stream: u32) -> i32 {
    let mut value: u32 = 0;
    for i in 0..16 {
        // Skip the trailing bit at stream bit 0; word LSB is the oldest bit.
        value |= ((stream >> (16 - i)) & 1) << i;
    }
    // Un-invert the sign and sign-extend: offset binary to two's complement.
    i32::from((value as u16 ^ 0x8000) as i16)
}

/// Scales the analog (SSG) pin's average into the serial DAC's range.
///
/// One SSG channel flat out is `1.0` on the pin (the die's `volume_lut` tops
/// there); the shipping core's AY plays the same channel at 8000. The FM word
/// needs no factor of its own, so matching the SSG alone lines the two mixes up.
const SSG_SCALE: f32 = 8000.0;

/// The YM2608, as its own die computes it -- mask ROM and all.
#[derive(Debug)]
pub struct Ym2608Lle {
    chip: OpnaLleChip,
    rate: u32,
    writes: VecDeque<BusByte>,
    bus: Bus,
    dram: Vec<u8>,
    port: DramPort,
    capture: [DacCapture; 2],
    held: [i32; 2],
}

impl Ym2608Lle {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: OpnaLleChip::new(),
            rate: 55_555,
            writes: VecDeque::new(),
            bus: Bus::Idle,
            dram: vec![0; DRAM_SIZE],
            port: DramPort::default(),
            capture: [DacCapture::default(); 2],
            held: [0; 2],
        }
    }

    /// One master clock: write bus, both edges, memory service, DAC tap.
    fn master_clock(&mut self) -> f32 {
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

        let bus = self.chip.dram_bus();
        let served = self.port.feed(bus, &mut self.dram);
        self.chip.serve_dm(served);

        let (sh1, sh2, so, analog) = self.chip.dac_pins();
        let s = self.chip.s_pin();
        if let Some(sample) = self.capture[0].feed(sh1, so, s) {
            self.held[0] = sample;
        }
        if let Some(sample) = self.capture[1].feed(sh2, so, s) {
            self.held[1] = sample;
        }
        analog
    }
}

impl Default for Ym2608Lle {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ym2608Lle {
    fn reset(&mut self, clock: u32, _variant: bool) {
        self.writes.clear();
        self.bus = Bus::Idle;
        self.port = DramPort::default();
        self.capture = [DacCapture::default(); 2];
        self.held = [0; 2];
        self.rate = (clock / CLOCKS_PER_SAMPLE).max(1);
        // The DRAM survives reset: the `0x81` blocks arrive before it, as the
        // clean-room cores keep their ROMs.
        self.chip.power_cycle();
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

    /// The VGM's `0x81` blocks: the Delta-T memory image, preloaded into
    /// the DRAM this wrapper serves.
    fn load_rom(&mut self, block_type: u8, _total_size: u32, start: u32, data: &[u8]) {
        if block_type != 0x81 {
            return;
        }
        let at = start as usize;
        let end = (at + data.len()).min(self.dram.len());
        if at < end {
            self.dram[at..end].copy_from_slice(&data[..end - at]);
        }
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
            let mut analog_sum = 0.0f32;
            for _ in 0..CLOCKS_PER_SAMPLE {
                analog_sum += self.master_clock();
            }
            // The serial DAC carries FM, rhythm and Delta-T; the analog pin
            // carries the SSG, averaged over the sample and scaled into the
            // same range.
            let ssg = (analog_sum / CLOCKS_PER_SAMPLE as f32 * SSG_SCALE) as i32;
            // SH1 frames the LEFT word: accumulator 1 collects the pan bits
            // the register map calls left (`$B4` bit 7, rhythm bit 7), and in
            // Delta-T DA mode the die gates `o_sh1` with `ad_reg_l`, the left
            // enable.
            frame[0] = self.held[0] + ssg;
            frame[1] = self.held[1] + ssg;
        }
    }
}

/// The chip this core serves.
pub(crate) const CHIPS: [ChipKind; 1] = [ChipKind::Ym2608];

#[cfg(test)]
mod tests {
    use super::*;

    /// The PC-88/98 sound board clock.
    const CLOCK: u32 = 7_987_200;

    fn render(chip: &mut Ym2608Lle, frames: usize) -> Vec<(i32, i32)> {
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

    /// A loud FM note on channel 1, algorithm 7, all four operators configured.
    /// The die needs the full patch where a single-op one whispers: unkeyed
    /// operators' power-on envelope state is not the clean silence an
    /// abstraction resets to.
    fn key_on_fm(chip: &mut Ym2608Lle) {
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
            (0xB4, 0xC0), // both speakers
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
        let mut chip = Ym2608Lle::new();
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

    /// The drums the clean-room core cannot play. The rhythm section reads the
    /// chip's internal mask ROM -- here that ROM is the decap's, so a bass-drum
    /// key-on must sound with no sample block loaded at all.
    #[test]
    fn the_rhythm_section_plays_from_the_mask_rom() {
        let mut chip = Ym2608Lle::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0x29, 0x80); // OPNA mode
        chip.write(0, 0x11, 0x3F); // rhythm total level: full
        chip.write(0, 0x18, 0xDF); // bass drum: both sides, full level
        let quiet = energy(&render(&mut chip, 256));

        chip.write(0, 0x10, 0x01); // key on the bass drum
        render(&mut chip, 64);
        let loud = energy(&render(&mut chip, 1024));
        assert!(
            loud > quiet * 4 && loud > 10_000,
            "the mask ROM must sound with nothing loaded: loud={loud} quiet={quiet}"
        );
    }

    /// `clock / 144`, as the shipping core declares for the YM2608.
    #[test]
    fn the_native_rate_matches_the_shipping_core() {
        let mut chip = Ym2608Lle::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 144);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }
}
