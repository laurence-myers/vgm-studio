//! The Game Boy's APU: two pulses, a wavetable channel and a noise channel.
//!
//! 3,827 files in the VGMRips corpus, and the only chip here with **stereo of
//! its own** -- every channel can be routed to either side independently, which
//! is why a Game Boy rip has a width that a Master System one does not.
//!
//! **Route B, from the Pan Docs**, so it can live in a permissively-licensed
//! crate. The plan allowed for consuming SameBoy's APU as a submodule instead;
//! that C is written against SameBoy's whole `GB_gameboy_t`, so carving it out
//! would have meant editing it -- which the sourcing policy forbids and which
//! would have to be redone on every upstream pull.
//!
//! # What is peculiar about this chip
//!
//! - **The DAC and the channel are separate.** Setting a pulse's volume and
//!   direction to zero switches its DAC *off*, which is not the same as playing
//!   silence: it also stops the channel. Writing NR52's enable bit low clears
//!   every register, which is how a driver resets the sound hardware.
//! - **The wave channel's volume is a shift, not a multiply** -- full, half,
//!   quarter, or off. There is no quieter setting than a quarter, which is why
//!   Game Boy basslines sit where they do.
//! - **Lengths differ per channel**: 64 steps for the pulses and the noise, 256
//!   for the wave.
//!
//! Not modelled: the wave RAM's read/write quirks while the channel is running
//! (nothing here reads), and the CGB's extra registers.

use crate::chip::ChipCore;

/// Master clocks averaged into one output sample.
///
/// 64 puts the native rate at 65536 Hz for the standard 4.194304 MHz clock --
/// exact, which keeps the resampler's arithmetic honest.
const CYCLES_PER_SAMPLE: u32 = 64;

/// The frame sequencer runs at 512 Hz: one tick every 8192 master clocks.
const FRAME_PERIOD: u32 = 8192;

/// Peak amplitude with everything at full volume, both sides.
const PEAK: i32 = 24_000;

/// The four duty cycles, as the eight-step sequences they are.
const DUTY: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5%
    [1, 0, 0, 0, 0, 0, 0, 1], // 25%
    [1, 0, 0, 0, 0, 1, 1, 1], // 50%
    [0, 1, 1, 1, 1, 1, 1, 0], // 75%
];

/// The noise channel's divisor codes. Code 0 is a half-step, not a zero.
const NOISE_DIVISORS: [u32; 8] = [8, 16, 32, 48, 64, 80, 96, 112];

/// A volume envelope, shared by the pulses and the noise.
#[derive(Debug, Default, Clone, Copy)]
struct Envelope {
    /// The initial volume, which with `increasing` also decides whether the
    /// channel's DAC is powered at all.
    initial: u8,
    increasing: bool,
    period: u8,
    volume: u8,
    timer: u8,
}

impl Envelope {
    /// A channel whose envelope has zero initial volume and points downward has
    /// its DAC switched off -- silent, and *disabled*, which is different from
    /// being quiet.
    fn dac_on(&self) -> bool {
        self.initial != 0 || self.increasing
    }

    fn trigger(&mut self) {
        self.volume = self.initial;
        self.timer = self.period;
    }

    /// One envelope tick (every eighth frame-sequencer step).
    fn clock(&mut self) {
        if self.period == 0 {
            return;
        }
        if self.timer > 0 {
            self.timer -= 1;
        }
        if self.timer == 0 {
            self.timer = self.period;
            if self.increasing && self.volume < 15 {
                self.volume += 1;
            } else if !self.increasing && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }
}

/// A length counter. The pulses and noise count to 64, the wave to 256.
#[derive(Debug, Default, Clone, Copy)]
struct Length {
    counter: u16,
    enabled: bool,
    max: u16,
}

impl Length {
    fn new(max: u16) -> Self {
        Self {
            counter: 0,
            enabled: false,
            max,
        }
    }

    fn load(&mut self, value: u16) {
        self.counter = self.max - value;
    }

    /// One length tick, returning whether it has just expired.
    fn clock(&mut self) -> bool {
        if !self.enabled || self.counter == 0 {
            return false;
        }
        self.counter -= 1;
        self.counter == 0
    }

    fn trigger(&mut self) {
        if self.counter == 0 {
            self.counter = self.max;
        }
    }
}

/// A pulse channel; channel 1 additionally has the sweep unit.
#[derive(Debug, Clone, Copy)]
struct Square {
    on: bool,
    envelope: Envelope,
    length: Length,
    duty: u8,
    step: u8,
    /// Eleven bits, counting *up* to 2048 -- the period is `2048 - frequency`.
    frequency: u16,
    timer: u32,
    // Sweep, channel 1 only.
    has_sweep: bool,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_timer: u8,
    sweep_enabled: bool,
    sweep_shadow: u16,
}

impl Square {
    fn new(has_sweep: bool) -> Self {
        Self {
            on: false,
            envelope: Envelope::default(),
            length: Length::new(64),
            duty: 0,
            step: 0,
            frequency: 0,
            timer: 0,
            has_sweep,
            sweep_period: 0,
            sweep_negate: false,
            sweep_shift: 0,
            sweep_timer: 0,
            sweep_enabled: false,
            sweep_shadow: 0,
        }
    }

    /// Master clocks per sequencer step: `(2048 - frequency) * 4`.
    fn period(&self) -> u32 {
        (2048 - u32::from(self.frequency)) * 4
    }

    fn clock(&mut self, cycles: u32) {
        let period = self.period();
        self.timer += cycles;
        while self.timer >= period {
            self.timer -= period;
            self.step = (self.step + 1) & 7;
        }
    }

    fn trigger(&mut self) {
        self.on = self.envelope.dac_on();
        self.length.trigger();
        self.timer = 0;
        self.envelope.trigger();
        if self.has_sweep {
            self.sweep_shadow = self.frequency;
            self.sweep_timer = if self.sweep_period == 0 {
                8
            } else {
                self.sweep_period
            };
            self.sweep_enabled = self.sweep_period > 0 || self.sweep_shift > 0;
            // A trigger with a shift set runs the overflow check immediately,
            // which can disable the channel before it makes a sound.
            if self.sweep_shift > 0 && self.next_sweep() > 2047 {
                self.on = false;
            }
        }
    }

    fn next_sweep(&self) -> u16 {
        let change = self.sweep_shadow >> self.sweep_shift;
        if self.sweep_negate {
            self.sweep_shadow.saturating_sub(change)
        } else {
            self.sweep_shadow + change
        }
    }

    /// One sweep tick (frame-sequencer steps 2 and 6).
    fn clock_sweep(&mut self) {
        if !self.has_sweep {
            return;
        }
        if self.sweep_timer > 0 {
            self.sweep_timer -= 1;
        }
        if self.sweep_timer != 0 {
            return;
        }
        self.sweep_timer = if self.sweep_period == 0 {
            8
        } else {
            self.sweep_period
        };
        if !self.sweep_enabled || self.sweep_period == 0 {
            return;
        }
        let next = self.next_sweep();
        if next > 2047 {
            self.on = false;
        } else if self.sweep_shift > 0 {
            self.sweep_shadow = next;
            self.frequency = next;
            // A second overflow check, on the *new* value, which hardware does
            // and which silences a runaway sweep one step sooner.
            if self.next_sweep() > 2047 {
                self.on = false;
            }
        }
    }

    fn output(&self) -> i32 {
        if !self.on || !self.envelope.dac_on() {
            0
        } else {
            i32::from(DUTY[self.duty as usize][self.step as usize] * self.envelope.volume)
        }
    }
}

/// The wavetable channel: 32 four-bit samples, played at a shift-controlled
/// volume.
#[derive(Debug, Clone, Copy)]
struct Wave {
    on: bool,
    dac_on: bool,
    length: Length,
    /// 0 mutes; 1, 2 and 3 mean full, half and quarter.
    volume_shift: u8,
    frequency: u16,
    timer: u32,
    position: u8,
    /// Sixteen bytes, two samples each.
    ram: [u8; 16],
}

impl Default for Wave {
    fn default() -> Self {
        Self {
            on: false,
            dac_on: false,
            length: Length::new(256),
            volume_shift: 0,
            frequency: 0,
            timer: 0,
            position: 0,
            ram: [0; 16],
        }
    }
}

impl Wave {
    /// Twice the pulses' rate for the same frequency: `(2048 - f) * 2`.
    fn period(&self) -> u32 {
        (2048 - u32::from(self.frequency)) * 2
    }

    fn clock(&mut self, cycles: u32) {
        let period = self.period();
        self.timer += cycles;
        while self.timer >= period {
            self.timer -= period;
            self.position = (self.position + 1) & 31;
        }
    }

    fn trigger(&mut self) {
        self.on = self.dac_on;
        self.length.trigger();
        self.timer = 0;
        self.position = 0;
    }

    fn output(&self) -> i32 {
        if !self.on || !self.dac_on || self.volume_shift == 0 {
            return 0;
        }
        let byte = self.ram[usize::from(self.position >> 1)];
        // The high nibble is the earlier sample.
        let nibble = if self.position & 1 == 0 {
            byte >> 4
        } else {
            byte & 0x0F
        };
        i32::from(nibble) >> (self.volume_shift - 1)
    }
}

/// The noise channel: a shift register clocked from a divisor and a shift.
#[derive(Debug, Clone, Copy)]
struct Noise {
    on: bool,
    envelope: Envelope,
    length: Length,
    divisor_code: u8,
    shift_amount: u8,
    /// Feedback into bit 6 as well, giving a short, buzzy sequence.
    short_mode: bool,
    timer: u32,
    shift: u16,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            on: false,
            envelope: Envelope::default(),
            length: Length::new(64),
            divisor_code: 0,
            shift_amount: 0,
            short_mode: false,
            timer: 0,
            // All ones: hardware's state after a trigger.
            shift: 0x7FFF,
        }
    }
}

impl Noise {
    fn period(&self) -> u32 {
        (NOISE_DIVISORS[usize::from(self.divisor_code & 7)] << self.shift_amount).max(1)
    }

    fn clock(&mut self, cycles: u32) {
        // A shift amount above 13 is documented as stopping the channel rather
        // than producing an enormous period.
        if self.shift_amount > 13 {
            return;
        }
        let period = self.period();
        self.timer += cycles;
        while self.timer >= period {
            self.timer -= period;
            let feedback = (self.shift & 1) ^ ((self.shift >> 1) & 1);
            self.shift = (self.shift >> 1) | (feedback << 14);
            if self.short_mode {
                self.shift = (self.shift & !(1 << 6)) | (feedback << 6);
            }
        }
    }

    fn trigger(&mut self) {
        self.on = self.envelope.dac_on();
        self.length.trigger();
        self.timer = 0;
        self.envelope.trigger();
        self.shift = 0x7FFF;
    }

    fn output(&self) -> i32 {
        if !self.on || !self.envelope.dac_on() {
            0
        } else {
            // The *inverted* low bit drives the output.
            i32::from((!self.shift & 1) as u8 * self.envelope.volume)
        }
    }
}

/// The Game Boy / Game Boy Color APU.
#[derive(Debug)]
pub struct GbDmg {
    rate: u32,
    /// Master clocks per output sample, tracked because the channels are
    /// stepped in bulk rather than one clock at a time.
    cycles_per_sample: u32,
    square1: Square,
    square2: Square,
    wave: Wave,
    noise: Noise,
    /// The master switch, NR52 bit 7. Clearing it zeroes every register.
    power: bool,
    /// Left and right master volumes, 0-7 each (NR50).
    volume: [u8; 2],
    /// Which channels reach which side (NR51), indexed `[side][channel]`.
    routing: [[bool; 4]; 2],
    frame_timer: u32,
    frame_step: u8,
}

impl Default for GbDmg {
    fn default() -> Self {
        Self {
            rate: 65_536,
            cycles_per_sample: CYCLES_PER_SAMPLE,
            square1: Square::new(true),
            square2: Square::new(false),
            wave: Wave::default(),
            noise: Noise::default(),
            power: true,
            volume: [7, 7],
            routing: [[true; 4]; 2],
            frame_timer: 0,
            frame_step: 0,
        }
    }
}

impl GbDmg {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// One frame-sequencer step: lengths on the even steps, sweep on 2 and 6,
    /// envelopes on 7.
    fn clock_frame(&mut self) {
        let step = self.frame_step;
        self.frame_step = (self.frame_step + 1) & 7;

        if step.is_multiple_of(2) {
            if self.square1.length.clock() {
                self.square1.on = false;
            }
            if self.square2.length.clock() {
                self.square2.on = false;
            }
            if self.wave.length.clock() {
                self.wave.on = false;
            }
            if self.noise.length.clock() {
                self.noise.on = false;
            }
        }
        if step == 2 || step == 6 {
            self.square1.clock_sweep();
        }
        if step == 7 {
            self.square1.envelope.clock();
            self.square2.envelope.clock();
            self.noise.envelope.clock();
        }
    }

    /// Advances everything by `cycles` master clocks.
    fn advance(&mut self, cycles: u32) {
        if !self.power {
            return;
        }
        self.square1.clock(cycles);
        self.square2.clock(cycles);
        self.wave.clock(cycles);
        self.noise.clock(cycles);

        self.frame_timer += cycles;
        while self.frame_timer >= FRAME_PERIOD {
            self.frame_timer -= FRAME_PERIOD;
            self.clock_frame();
        }
    }

    /// The mixed stereo output, before scaling.
    fn mix(&self) -> [i32; 2] {
        if !self.power {
            return [0, 0];
        }
        let channels = [
            self.square1.output(),
            self.square2.output(),
            self.wave.output(),
            self.noise.output(),
        ];
        let mut out = [0i32; 2];
        for (side, sum) in out.iter_mut().enumerate() {
            let mixed: i32 = channels
                .iter()
                .zip(self.routing[side])
                .filter_map(|(&value, routed)| routed.then_some(value))
                .sum();
            // Master volume is 0-7 and never fully mutes: the documented
            // behaviour is `volume + 1` out of 8.
            *sum = mixed * i32::from(self.volume[side] + 1);
        }
        out
    }

    /// Clearing NR52's power bit resets every register, which is how a driver
    /// silences the hardware in one write.
    fn power_off(&mut self) {
        let ram = self.wave.ram;
        let rate = self.rate;
        let cycles_per_sample = self.cycles_per_sample;
        *self = Self {
            rate,
            cycles_per_sample,
            power: false,
            volume: [0, 0],
            routing: [[false; 4]; 2],
            ..Self::default()
        };
        // Wave RAM survives a power cycle; a driver relies on that to load a
        // waveform once and switch the chip on afterwards.
        self.wave.ram = ram;
    }
}

impl ChipCore for GbDmg {
    fn reset(&mut self, clock: u32, _variant: bool) {
        let cycles_per_sample = CYCLES_PER_SAMPLE;
        *self = Self {
            rate: (clock / cycles_per_sample).max(1),
            cycles_per_sample,
            ..Self::default()
        };
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// Registers as the VGM numbers them: the offset from `$FF10`, with wave
    /// RAM at `0x20`-`0x2F`.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let value = (data & 0xFF) as u8;
        let register = (addr & 0xFF) as u8;

        // Wave RAM is writable with the chip powered down, and often is.
        if (0x20..=0x2F).contains(&register) {
            self.wave.ram[usize::from(register - 0x20)] = value;
            return;
        }
        // With the power off, only NR52 answers.
        if !self.power && register != 0x16 {
            return;
        }

        match register {
            0x00 => {
                self.square1.sweep_period = (value >> 4) & 7;
                self.square1.sweep_negate = value & 0x08 != 0;
                self.square1.sweep_shift = value & 7;
            }
            0x01 | 0x06 => {
                let square = if register == 0x01 {
                    &mut self.square1
                } else {
                    &mut self.square2
                };
                square.duty = value >> 6;
                square.length.load(u16::from(value & 0x3F));
            }
            0x02 | 0x07 => {
                let square = if register == 0x02 {
                    &mut self.square1
                } else {
                    &mut self.square2
                };
                square.envelope.initial = value >> 4;
                square.envelope.increasing = value & 0x08 != 0;
                square.envelope.period = value & 7;
                if !square.envelope.dac_on() {
                    square.on = false;
                }
            }
            0x03 | 0x08 => {
                let square = if register == 0x03 {
                    &mut self.square1
                } else {
                    &mut self.square2
                };
                square.frequency = (square.frequency & 0x700) | u16::from(value);
            }
            0x04 | 0x09 => {
                let square = if register == 0x04 {
                    &mut self.square1
                } else {
                    &mut self.square2
                };
                square.frequency = (square.frequency & 0x0FF) | (u16::from(value & 7) << 8);
                square.length.enabled = value & 0x40 != 0;
                if value & 0x80 != 0 {
                    square.trigger();
                }
            }
            0x0A => {
                self.wave.dac_on = value & 0x80 != 0;
                if !self.wave.dac_on {
                    self.wave.on = false;
                }
            }
            0x0B => self.wave.length.load(u16::from(value)),
            0x0C => self.wave.volume_shift = (value >> 5) & 3,
            0x0D => self.wave.frequency = (self.wave.frequency & 0x700) | u16::from(value),
            0x0E => {
                self.wave.frequency = (self.wave.frequency & 0x0FF) | (u16::from(value & 7) << 8);
                self.wave.length.enabled = value & 0x40 != 0;
                if value & 0x80 != 0 {
                    self.wave.trigger();
                }
            }
            0x10 => self.noise.length.load(u16::from(value & 0x3F)),
            0x11 => {
                self.noise.envelope.initial = value >> 4;
                self.noise.envelope.increasing = value & 0x08 != 0;
                self.noise.envelope.period = value & 7;
                if !self.noise.envelope.dac_on() {
                    self.noise.on = false;
                }
            }
            0x12 => {
                self.noise.shift_amount = value >> 4;
                self.noise.short_mode = value & 0x08 != 0;
                self.noise.divisor_code = value & 7;
            }
            0x13 => {
                self.noise.length.enabled = value & 0x40 != 0;
                if value & 0x80 != 0 {
                    self.noise.trigger();
                }
            }
            0x14 => {
                self.volume[1] = value & 7; // right, NR50's low nibble
                self.volume[0] = (value >> 4) & 7; // left
            }
            0x15 => {
                for channel in 0..4 {
                    self.routing[1][channel] = value & (1 << channel) != 0;
                    self.routing[0][channel] = value & (0x10 << channel) != 0;
                }
            }
            0x16 => {
                let on = value & 0x80 != 0;
                if on && !self.power {
                    let ram = self.wave.ram;
                    let (rate, cycles) = (self.rate, self.cycles_per_sample);
                    *self = Self {
                        rate,
                        cycles_per_sample: cycles,
                        // Powering on starts from silence, not from the
                        // everything-routed default: a driver sets NR50/NR51
                        // itself, and assuming otherwise makes the first frame
                        // of a rip a burst.
                        volume: [0, 0],
                        routing: [[false; 4]; 2],
                        ..Self::default()
                    };
                    self.wave.ram = ram;
                } else if !on && self.power {
                    self.power_off();
                }
            }
            _ => {}
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        // Full scale is four channels at 15, times the eightfold master volume,
        // per side.
        const FULL: i32 = 4 * 15 * 8;
        for frame in out.chunks_exact_mut(2) {
            self.advance(self.cycles_per_sample);
            let [left, right] = self.mix();
            frame[0] = left * PEAK / FULL;
            frame[1] = right * PEAK / FULL;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Game Boy's master clock, which is what a VGM header carries.
    const DMG: u32 = 4_194_304;

    fn render(chip: &mut GbDmg, frames: usize) -> Vec<[i32; 2]> {
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

    /// Powers the chip up and opens both sides at full volume, as a driver's
    /// first few writes do.
    fn power_on(chip: &mut GbDmg) {
        chip.write(0, 0x16, 0x80); // NR52: on
        chip.write(0, 0x14, 0x77); // NR50: both sides at 7
        chip.write(0, 0x15, 0xFF); // NR51: everything to both sides
    }

    /// A plain note on channel 2 -- no sweep to complicate it.
    fn key_on_square2(chip: &mut GbDmg) {
        chip.write(0, 0x06, 0x80); // 50% duty, length 0
        chip.write(0, 0x07, 0xF0); // volume 15, no envelope decay
        chip.write(0, 0x08, 0x00); // frequency low
        chip.write(0, 0x09, 0x87); // frequency high 7, trigger
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_triggered_note_is_not() {
        let mut chip = GbDmg::new();
        chip.reset(DMG, false);
        assert_eq!(energy(&render(&mut chip, 2000)), 0, "nothing triggered yet");

        power_on(&mut chip);
        key_on_square2(&mut chip);
        assert!(energy(&render(&mut chip, 2000)) > 0);
    }

    /// The rate the engine resamples from, exact for the standard clock.
    #[test]
    fn the_native_rate_divides_the_master_clock() {
        let mut chip = GbDmg::new();
        chip.reset(DMG, false);
        assert_eq!(chip.native_rate(), 65_536);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// **The chip's distinguishing feature.** Every channel routes to either
    /// side independently, so a rip has a stereo image no other chip here
    /// produces. Getting NR51's two nibbles the wrong way round swaps it.
    #[test]
    fn each_channel_routes_to_either_side_on_its_own() {
        let mut chip = GbDmg::new();
        chip.reset(DMG, false);
        power_on(&mut chip);
        // NR51 bit 1 is channel 2 on the *right*; bit 5 is channel 2 on the
        // left. Ask for the right only.
        chip.write(0, 0x15, 0x02);
        key_on_square2(&mut chip);

        let frames = render(&mut chip, 4000);
        let left: i64 = frames.iter().map(|f| i64::from(f[0].abs())).sum();
        let right: i64 = frames.iter().map(|f| i64::from(f[1].abs())).sum();
        assert_eq!(left, 0, "channel 2 was not asked for on the left");
        assert!(right > 0, "channel 2 was asked for on the right");
    }

    /// A pulse's pitch is `131072 / (2048 - frequency)` Hz. Counted in sequence
    /// wraps rather than asserted from the formula.
    #[test]
    fn a_pulse_sounds_at_the_documented_frequency() {
        let mut chip = GbDmg::new();
        chip.reset(DMG, false);
        power_on(&mut chip);
        key_on_square2(&mut chip);
        let frequency = chip.square2.frequency;

        // One second of master clocks, stepped as the render loop steps them.
        let mut wraps = 0u32;
        let mut last = chip.square2.step;
        for _ in 0..(DMG / CYCLES_PER_SAMPLE) {
            chip.advance(CYCLES_PER_SAMPLE);
            if chip.square2.step < last {
                wraps += 1;
            }
            last = chip.square2.step;
        }
        let expected = 131_072 / (2048 - u32::from(frequency));
        let drift = wraps.abs_diff(expected);
        assert!(
            drift * 100 <= expected,
            "counted {wraps} cycles a second, expected about {expected}"
        );
    }

    /// The wave channel's volume is a *shift*: full, half, quarter or nothing.
    /// A multiply would give a smooth fade the chip cannot actually produce.
    #[test]
    fn the_wave_volume_is_a_shift_not_a_multiply() {
        fn peak_at(shift: u8) -> i32 {
            let mut chip = GbDmg::new();
            chip.reset(DMG, false);
            power_on(&mut chip);
            // A ramp across all 32 samples, so the peak is the top nibble.
            for byte in 0..16u8 {
                chip.write(0, u16::from(0x20 + byte), u16::from(byte * 0x11));
            }
            chip.write(0, 0x0A, 0x80); // DAC on
            chip.write(0, 0x0C, u16::from(shift) << 5);
            chip.write(0, 0x0D, 0x00);
            chip.write(0, 0x0E, 0x87); // trigger
            render(&mut chip, 4000)
                .iter()
                .map(|f| f[0].abs())
                .max()
                .unwrap_or(0)
        }

        let full = peak_at(1);
        let half = peak_at(2);
        let quarter = peak_at(3);
        assert_eq!(peak_at(0), 0, "shift 0 is mute, not full volume");
        assert!(full > 0);
        // Halving, within the rounding the output scaling introduces.
        assert!(
            (half * 2).abs_diff(full) <= full as u32 / 8,
            "half came to {half} against a full {full}"
        );
        assert!(
            (quarter * 4).abs_diff(full) <= full as u32 / 4,
            "quarter came to {quarter} against a full {full}"
        );
    }

    /// Zero initial volume pointing downward switches a channel's DAC off,
    /// which disables it rather than merely making it quiet. A core that treats
    /// it as "volume 0" leaves the channel running and audible the moment the
    /// envelope moves.
    #[test]
    fn a_dac_switched_off_disables_its_channel() {
        let mut chip = GbDmg::new();
        chip.reset(DMG, false);
        power_on(&mut chip);
        key_on_square2(&mut chip);
        assert!(energy(&render(&mut chip, 1000)) > 0);

        chip.write(0, 0x07, 0x00); // volume 0, decreasing: DAC off
        assert_eq!(energy(&render(&mut chip, 2000)), 0);
        assert!(
            !chip.square2.on,
            "the channel must be disabled, not just quiet"
        );
    }

    /// A length counter expiring stops its channel. The pulses count to 64 and
    /// the wave to 256, so the same written value means different durations --
    /// which is the sort of thing that sounds like a tempo bug.
    #[test]
    fn a_length_counter_stops_its_channel() {
        let mut chip = GbDmg::new();
        chip.reset(DMG, false);
        power_on(&mut chip);
        chip.write(0, 0x06, 0xBF); // duty 50%, length 63 -> one step left
        chip.write(0, 0x07, 0xF0);
        chip.write(0, 0x08, 0x00);
        chip.write(0, 0x09, 0xC7); // length enabled, trigger

        // The sequencer runs at 512 Hz and lengths tick on half of those, so
        // one step is about 4 ms.
        let early = energy(&render(&mut chip, 100));
        assert!(early > 0, "the note must start");
        let _ = render(&mut chip, 2000);
        assert!(!chip.square2.on, "the length counter did not stop it");
        assert_eq!(energy(&render(&mut chip, 1000)), 0);
    }

    /// A sweep that runs the frequency past eleven bits disables the channel,
    /// which is how the classic Game Boy "shoot" effect ends.
    #[test]
    fn an_overflowing_sweep_disables_channel_one() {
        let mut chip = GbDmg::new();
        chip.reset(DMG, false);
        power_on(&mut chip);
        chip.write(0, 0x00, 0x11); // sweep: period 1, upward, shift 1
        chip.write(0, 0x01, 0x80);
        chip.write(0, 0x02, 0xF0);
        chip.write(0, 0x03, 0x00);
        chip.write(0, 0x04, 0x84); // frequency 0x400: the first step fits
        assert!(chip.square1.on, "0x400 + 0x200 is still inside eleven bits");

        let _ = render(&mut chip, 20_000);
        assert!(
            !chip.square1.on,
            "an upward sweep from 0x400 must overflow within a few steps"
        );
    }

    /// A trigger whose *first* sweep calculation already overflows disables the
    /// channel before it makes a sound. Documented, easy to leave out, and the
    /// difference between a silent effect and a stuck note.
    #[test]
    fn a_trigger_that_already_overflows_never_starts() {
        let mut chip = GbDmg::new();
        chip.reset(DMG, false);
        power_on(&mut chip);
        chip.write(0, 0x00, 0x11); // sweep: period 1, upward, shift 1
        chip.write(0, 0x01, 0x80);
        chip.write(0, 0x02, 0xF0);
        chip.write(0, 0x03, 0x00);
        chip.write(0, 0x04, 0x87); // 0x700 + 0x380 is past 0x7FF at the trigger
        assert!(!chip.square1.on, "it must not start at all");
        assert_eq!(energy(&render(&mut chip, 2000)), 0);
    }

    /// Clearing NR52's power bit resets the registers, and wave RAM survives
    /// it. A driver loads a waveform and *then* powers the chip up, so losing
    /// the RAM makes channel 3 play a flat line.
    #[test]
    fn powering_off_clears_the_registers_but_keeps_wave_ram() {
        let mut chip = GbDmg::new();
        chip.reset(DMG, false);
        power_on(&mut chip);
        for byte in 0..16u8 {
            chip.write(0, u16::from(0x20 + byte), 0xA5);
        }
        key_on_square2(&mut chip);
        assert!(energy(&render(&mut chip, 500)) > 0);

        chip.write(0, 0x16, 0x00); // power off
        assert_eq!(energy(&render(&mut chip, 2000)), 0);
        assert!(
            chip.wave.ram.iter().all(|&byte| byte == 0xA5),
            "RAM was lost"
        );

        // And a write while powered down is ignored, except to wave RAM.
        chip.write(0, 0x07, 0xF0);
        assert_eq!(chip.square2.envelope.initial, 0, "a write got through");
    }

    /// Chunking must not change the audio.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        fn set_up(chip: &mut GbDmg) {
            chip.reset(DMG, false);
            power_on(chip);
            key_on_square2(chip);
            chip.write(0, 0x11, 0xF0); // noise volume
            chip.write(0, 0x12, 0x34);
            chip.write(0, 0x13, 0x80); // trigger
        }
        let mut whole = GbDmg::new();
        set_up(&mut whole);
        let mut one_go = vec![0i32; 1024 * 2];
        whole.render(&mut one_go);

        let mut chunked = GbDmg::new();
        set_up(&mut chunked);
        let mut piecemeal = vec![0i32; 1024 * 2];
        for chunk in piecemeal.chunks_mut(64 * 2) {
            chunked.render(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// Everything at once should use the range without leaning on the clamp.
    #[test]
    fn a_full_chip_uses_the_range_without_clipping_it() {
        let mut chip = GbDmg::new();
        chip.reset(DMG, false);
        power_on(&mut chip);
        key_on_square2(&mut chip);
        chip.write(0, 0x01, 0x80);
        chip.write(0, 0x02, 0xF0);
        chip.write(0, 0x03, 0x40);
        chip.write(0, 0x04, 0x87);
        for byte in 0..16u8 {
            chip.write(0, u16::from(0x20 + byte), 0xF0);
        }
        chip.write(0, 0x0A, 0x80);
        chip.write(0, 0x0C, 0x20);
        chip.write(0, 0x0D, 0x00);
        chip.write(0, 0x0E, 0x87);
        chip.write(0, 0x11, 0xF0);
        chip.write(0, 0x12, 0x34);
        chip.write(0, 0x13, 0x80);

        let frames = render(&mut chip, 8000);
        let loudest = frames
            .iter()
            .flat_map(|f| [f[0].abs(), f[1].abs()])
            .max()
            .unwrap_or(0);
        assert!(
            loudest > PEAK / 4,
            "a full chip peaked at {loudest}, far below its own scale"
        );
        assert!(
            loudest <= PEAK,
            "a full chip peaked at {loudest}, past the scale it declares"
        );
    }
}
