//! The General Instrument AY-3-8910 and Yamaha's YM2149 clone of it: three
//! squares, a noise generator and one envelope shared between them.
//!
//! 4,228 files in the VGMRips corpus on its own account -- and rather more than
//! that in effect, because **this chip is the SSG section of every OPN part**.
//! A YM2203, YM2608 or YM2610 is an FM synthesiser bolted to one of these, and
//! the VGM header says so outright: the AY type and flags at `0x78`-`0x7B` are
//! shared between the standalone chip and the OPN chips' SSGs. So this core is
//! written to be reused there rather than to serve one chip.
//!
//! **Route B, from the datasheets**, so it lives in the permissive crate.
//!
//! # Two chips, one register set
//!
//! The AY-3-8910 and the YM2149 answer identically; what differs is the DAC.
//! The Yamaha part has **32 levels** at 1.5 dB a step and the GI part has
//! **16**, which are the odd entries of the same curve -- so one table serves
//! both, indexed by twice the volume for the AY. The envelope always resolves
//! to the 32-level scale, which is why an enveloped channel on a real AY sounds
//! finer-grained than a fixed-volume one.
//!
//! Not modelled: the two I/O ports (registers 14 and 15 carry no audio), and
//! the VGM `0x31` stereo-mask command, which is a command rather than a
//! register and would need engine support to route.

use crate::chip::ChipCore;

/// The chip divides its input clock by 8 to get its internal tick.
///
/// Datasheets usually quote tone frequency as `clock / (16 × period)`, which is
/// this divider plus the square wave's own half-period.
const CLOCK_DIVIDER: u32 = 8;

/// Peak amplitude of one channel at full volume.
///
/// Three channels, so a chip playing flat out reaches three times this -- the
/// same headroom convention [`Sn76489`](super::Sn76489) uses. Baked into
/// [`LEVELS`] rather than multiplied at render time, which is why only the test
/// that regenerates that table names it.
#[cfg(test)]
const PEAK: i32 = 8_000;

/// The DAC curve: 32 levels, 1.5 dB a step, silence at the bottom.
///
/// `LEVELS[n] = PEAK × 10^(-1.5 × (31 - n) / 20)`, with `LEVELS[0] = 0` because
/// the bottom step is off rather than merely quiet. Recomputed in
/// `the_volume_curve_is_one_and_a_half_decibels_a_step` rather than transcribed.
const LEVELS: [i32; 32] = [
    0, 45, 53, 64, 76, 90, 107, 127, 151, 179, 213, 253, 301, 357, 425, 505, 600, 713, 847, 1007,
    1197, 1423, 1691, 2010, 2388, 2839, 3374, 4009, 4765, 5664, 6731, 8000,
];

/// One square-wave channel.
#[derive(Debug, Default, Clone, Copy)]
struct Tone {
    /// Twelve bits. Zero behaves as one -- the counter still expires.
    period: u16,
    counter: u16,
    output: bool,
    /// Four bits, or "follow the envelope" when [`Self::use_envelope`].
    volume: u8,
    use_envelope: bool,
    /// From the mixer register: these are *disable* bits on the chip, kept the
    /// right way round here so the code reads as what is heard.
    tone_on: bool,
    noise_on: bool,
}

impl Tone {
    /// One internal tick.
    ///
    /// Counts *down to* the reload rather than through it: a period of `n` must
    /// be exactly `n` ticks to a flip, giving `clock / (16 × period)`. Reloading
    /// on zero instead spends `n + 1`, which is a flat note -- 1.5% at period
    /// 64 and a whole semitone at period 12.
    fn clock(&mut self) {
        if self.counter > 1 {
            self.counter -= 1;
        } else {
            // A period of zero behaves as one rather than dividing by nothing.
            self.counter = self.period.max(1);
            self.output = !self.output;
        }
    }

    /// Whether this channel's gate is open, given the noise generator's state.
    ///
    /// The two enables are *ORed*, which is the part that surprises: a channel
    /// with both tone and noise on is not a mix of them but a gate that opens
    /// when either does, and one with neither on sits at full amplitude rather
    /// than silent. Drivers use that last case to play the envelope alone.
    fn gate(&self, noise: bool) -> bool {
        (self.output || !self.tone_on) && (noise || !self.noise_on)
    }
}

/// The noise generator: a 17-bit shift register tapped at bits 0 and 3.
#[derive(Debug, Clone, Copy)]
struct Noise {
    /// Five bits.
    period: u8,
    counter: u8,
    shift: u32,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            period: 0,
            counter: 0,
            // Any non-zero seed works; zero would never produce feedback.
            shift: 1,
        }
    }
}

impl Noise {
    /// One internal tick. The register advances every *second* tick, which is
    /// why noise is an octave below a tone of the same period.
    fn clock(&mut self) {
        if self.counter > 1 {
            self.counter -= 1;
        } else {
            self.counter = self.period.max(1);
            let feedback = (self.shift ^ (self.shift >> 3)) & 1;
            self.shift = (self.shift >> 1) | (feedback << 16);
        }
    }

    fn output(&self) -> bool {
        self.shift & 1 != 0
    }
}

/// The shared envelope generator.
#[derive(Debug, Default, Clone, Copy)]
struct Envelope {
    /// Sixteen bits, ticked at a sixteenth of the tone rate.
    period: u16,
    counter: u32,
    /// The shape's four bits: continue, attack, alternate, hold.
    shape: u8,
    /// Where in the current ramp, 0-31 on the fine scale.
    step: u8,
    /// Whether the ramp is rising.
    rising: bool,
    /// Stopped at one end, or stopped at zero because `continue` was clear.
    holding: bool,
}

impl Envelope {
    /// A shape write restarts the envelope, which is how a driver retriggers it.
    fn set_shape(&mut self, shape: u8) {
        self.shape = shape & 0x0F;
        self.step = 0;
        self.holding = false;
        // Bit 2 is `attack`: set means the first ramp rises.
        self.rising = self.shape & 0x04 != 0;
        self.counter = 0;
    }

    /// One envelope tick.
    fn clock(&mut self) {
        if self.holding {
            return;
        }
        if self.step < 31 {
            self.step += 1;
            return;
        }
        // The ramp has finished. With `continue` clear the envelope stops at
        // silence whichever way it was going -- one ramp and done, which is the
        // classic percussive shape.
        if self.shape & 0x08 == 0 {
            self.holding = true;
            self.rising = false;
            self.step = 31;
            return;
        }
        if self.shape & 0x02 != 0 {
            // Alternate: turn round.
            self.rising = !self.rising;
        }
        if self.shape & 0x01 != 0 {
            // Hold: stop here. `alternate` has already turned the direction
            // round if it was set, and that flip is the *only* one -- so
            // attack+hold holds at the top and attack+alternate+hold at the
            // bottom, which is what the four hold shapes are for.
            self.holding = true;
            self.step = 31;
        } else {
            self.step = 0;
        }
    }

    /// The level on the 32-step scale.
    fn level(&self) -> u8 {
        if self.rising {
            self.step
        } else {
            31 - self.step
        }
    }
}

/// The AY-3-8910 / YM2149.
#[derive(Debug, Default)]
pub struct Ay8910 {
    rate: u32,
    tones: [Tone; 3],
    noise: Noise,
    envelope: Envelope,
    /// Halves the internal tick for the noise generator and the envelope, both
    /// of which run at half the tone rate.
    half_tick: bool,
    /// Counts sixteen half-ticks per envelope step.
    envelope_divider: u16,
    /// The last value written to each register, so a partial write to a
    /// two-register pair keeps the other half.
    registers: [u8; 16],
}

impl Ay8910 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 44_100,
            ..Self::default()
        }
    }

    /// Applies a register write.
    ///
    /// Public because **this chip is the SSG section of every OPN part**, and
    /// those cores live in another crate. They drive this one through
    /// [`write_register`](Self::write_register), [`tick`](Self::tick) and
    /// [`output`](Self::output) rather than keeping a copy of it, which is the
    /// whole reason the three are exposed at all.
    pub fn write_register(&mut self, register: u8, value: u8) {
        let register = register & 0x0F;
        self.registers[usize::from(register)] = value;
        match register {
            0 | 2 | 4 => {
                let channel = usize::from(register / 2);
                self.tones[channel].period =
                    (self.tones[channel].period & 0x0F00) | u16::from(value);
            }
            1 | 3 | 5 => {
                let channel = usize::from(register / 2);
                self.tones[channel].period =
                    (self.tones[channel].period & 0x00FF) | (u16::from(value & 0x0F) << 8);
            }
            6 => self.noise.period = value & 0x1F,
            7 => {
                for (channel, tone) in self.tones.iter_mut().enumerate() {
                    // The register's bits *disable*; inverted here so the field
                    // means what it says.
                    tone.tone_on = value & (1 << channel) == 0;
                    tone.noise_on = value & (0x08 << channel) == 0;
                }
            }
            8..=10 => {
                let tone = &mut self.tones[usize::from(register - 8)];
                tone.volume = value & 0x0F;
                tone.use_envelope = value & 0x10 != 0;
            }
            11 => self.envelope.period = (self.envelope.period & 0xFF00) | u16::from(value),
            12 => self.envelope.period = (self.envelope.period & 0x00FF) | (u16::from(value) << 8),
            13 => self.envelope.set_shape(value),
            // 14 and 15 are the I/O ports, which carry no audio.
            _ => {}
        }
    }

    /// One internal tick of everything.
    ///
    /// For a host that clocks this chip itself rather than through
    /// [`render`](ChipCore::render) -- see [`write_register`](Self::write_register).
    pub fn tick(&mut self) {
        for tone in &mut self.tones {
            tone.clock();
        }
        // The noise generator and the envelope both run at half the tone rate.
        self.half_tick = !self.half_tick;
        if !self.half_tick {
            return;
        }
        self.noise.clock();
        // Sixteen of those to one envelope step, and the period divides that.
        self.envelope_divider += 1;
        if self.envelope_divider >= self.envelope.period.max(1) {
            self.envelope_divider = 0;
            self.envelope.clock();
        }
    }

    /// The mixed output of the current state, mono.
    pub fn output(&self) -> i32 {
        let noise = self.noise.output();
        let envelope = self.envelope.level();
        self.tones
            .iter()
            .map(|tone| {
                if !tone.gate(noise) {
                    return 0;
                }
                let level = if tone.use_envelope {
                    envelope
                } else if tone.volume == 0 {
                    // Volume 0 is *off*, not the quietest step -- so it maps to
                    // the curve's silent entry rather than to `2n + 1`.
                    0
                } else {
                    // Otherwise a fixed volume addresses the coarse half of the
                    // curve: the GI part's sixteen levels are the odd entries
                    // of the Yamaha part's thirty-two.
                    (tone.volume << 1) | 1
                };
                LEVELS[usize::from(level & 31)]
            })
            .sum()
    }

    /// The internal tick rate for a given input clock.
    ///
    /// A host clocking this chip alongside a faster one needs it to work out
    /// how many ticks fall in one of its own samples.
    #[must_use]
    pub const fn tick_rate(clock: u32) -> u32 {
        clock / CLOCK_DIVIDER
    }
}

impl ChipCore for Ay8910 {
    /// `variant` is unused: the header's AY *type* byte at `0x78` distinguishes
    /// the parts, and the two answer identically -- only the DAC differs, and
    /// one curve serves both.
    fn reset(&mut self, clock: u32, _variant: bool) {
        *self = Self {
            rate: Self::tick_rate(clock).max(1),
            ..Self::default()
        };
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        self.write_register((addr & 0xFF) as u8, (data & 0xFF) as u8);
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            self.tick();
            let sample = self.output();
            // Mono: the chip has three pins but one output on every board the
            // VGM format records. (The `0x31` stereo mask is not modelled.)
            frame[0] = sample;
            frame[1] = sample;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A common AY clock: half the NTSC colour burst, as the Spectrum 128 and
    /// countless arcade boards run it.
    const CLOCK: u32 = 1_789_773;

    fn render(chip: &mut Ay8910, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|frame| frame[0]).collect()
    }

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    /// Every level recomputed from "1.5 dB a step", rather than transcribed.
    #[test]
    fn the_volume_curve_is_one_and_a_half_decibels_a_step() {
        assert_eq!(LEVELS[0], 0, "the bottom step is off, not quiet");
        for (step, &level) in LEVELS.iter().enumerate().skip(1) {
            let expected = f64::from(PEAK) * 10f64.powf(-1.5 * (31 - step) as f64 / 20.0);
            assert_eq!(level, expected.round() as i32, "step {step}");
        }
        assert_eq!(LEVELS[31], PEAK, "the top step is full scale");
        // And the defining relationship: two steps is about a factor of √2 each,
        // so four steps double.
        let ratio = f64::from(LEVELS[31]) / f64::from(LEVELS[27]);
        assert!((1.9..=2.1).contains(&ratio), "four steps came to {ratio}x");
    }

    /// A tone's pitch is `clock / (16 × period)`, counted in flips rather than
    /// asserted from the formula.
    #[test]
    fn a_tone_sounds_at_the_documented_frequency() {
        let mut chip = Ay8910::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0, 0x40); // channel A period low
        chip.write(0, 1, 0x00);
        chip.write(0, 7, 0x3E); // tone A on, everything else off
        chip.write(0, 8, 0x0F); // full volume

        let ticks = Ay8910::tick_rate(CLOCK);
        let mut flips = 0u32;
        let mut last = chip.tones[0].output;
        for _ in 0..ticks {
            chip.tick();
            if chip.tones[0].output != last {
                flips += 1;
            }
            last = chip.tones[0].output;
        }
        // Two flips to a cycle.
        let cycles = flips / 2;
        let expected = CLOCK / (16 * 0x40);
        let drift = cycles.abs_diff(expected);
        assert!(
            drift * 200 <= expected,
            "counted {cycles} cycles a second, expected about {expected}"
        );
    }

    /// **The mixer's enables are disables, and they OR.** A channel with
    /// neither tone nor noise selected sits at full amplitude rather than
    /// silent -- which is how a driver plays the envelope on its own, and which
    /// inverting the bits would turn into silence.
    #[test]
    fn the_mixer_bits_disable_rather_than_enable() {
        let mut chip = Ay8910::new();
        chip.reset(CLOCK, false);
        chip.write(0, 8, 0x0F); // channel A at full volume

        // 0xFF: every tone and noise bit set, so everything is *off* -- and the
        // gate is therefore permanently open.
        chip.write(0, 7, 0xFF);
        let open = render(&mut chip, 500);
        assert!(
            open.iter().all(|&s| s == LEVELS[31]),
            "a channel with nothing selected must sit at full amplitude              (B and C are at volume 0, which is off)"
        );

        // Selecting the tone makes it a square wave again.
        chip.write(0, 0, 0x20);
        chip.write(0, 7, 0xFE);
        let square = render(&mut chip, 2000);
        assert!(square.contains(&0), "the gate must close sometimes");
        assert!(square.iter().any(|&s| s != 0), "and open sometimes");
    }

    /// Volume 0 is silence, and the fixed-volume scale addresses the odd
    /// entries of the fine curve -- so a fixed level and the envelope reaching
    /// the same place agree.
    #[test]
    fn a_fixed_volume_uses_the_coarse_half_of_the_curve() {
        let mut chip = Ay8910::new();
        chip.reset(CLOCK, false);
        chip.write(0, 7, 0xFF); // gate open on every channel
        chip.write(0, 9, 0x00);
        chip.write(0, 10, 0x00); // B and C silent

        chip.write(0, 8, 0x00); // volume 0
        assert_eq!(render(&mut chip, 100)[0], 0, "volume 0 is off, not quiet");

        chip.write(0, 8, 0x01); // volume 1
        assert_eq!(
            render(&mut chip, 100)[0],
            LEVELS[3],
            "volume 1 is the floor"
        );

        chip.write(0, 8, 0x0F); // volume 15
        assert_eq!(
            render(&mut chip, 100)[0],
            LEVELS[31],
            "volume 15 is the top"
        );

        chip.write(0, 8, 0x07);
        assert_eq!(
            render(&mut chip, 100)[0],
            LEVELS[15],
            "volume 7 is halfway up"
        );
    }

    /// The eight distinct envelope shapes, checked by where each *ends up*.
    /// Getting the continue/hold/alternate logic wrong is inaudible on a single
    /// ramp and obvious on a sustained note.
    #[test]
    fn the_envelope_shapes_end_where_they_should() {
        /// Runs an envelope well past several ramps and returns its level.
        fn settle(shape: u8) -> u8 {
            let mut envelope = Envelope::default();
            envelope.set_shape(shape);
            for _ in 0..(32 * 8) {
                envelope.clock();
            }
            envelope.level()
        }

        // Continue clear (0x00-0x07): one ramp, then silence, whichever way it
        // was going. That is the whole point of the shape.
        for shape in 0x00..=0x07u8 {
            assert_eq!(settle(shape), 0, "shape {shape:#04x} must decay to silence");
        }
        // 0x08: saw down, repeating -- never holds.
        let mut sawtooth = Envelope::default();
        sawtooth.set_shape(0x08);
        for _ in 0..(32 * 8) {
            sawtooth.clock();
        }
        assert!(!sawtooth.holding, "a repeating shape must not hold");
        // 0x09: down once, then hold at the bottom.
        assert_eq!(settle(0x09), 0);
        // 0x0B: down once, then hold at the *top*.
        assert_eq!(settle(0x0B), 31);
        // 0x0D: up once, then hold at the top.
        assert_eq!(settle(0x0D), 31, "attack + hold holds at the top");
        // 0x0F: up once, alternate turns it round, hold at the bottom.
        assert_eq!(
            settle(0x0F),
            0,
            "attack + alternate + hold holds at the bottom"
        );
    }

    /// A shape write restarts the envelope even if the shape has not changed --
    /// which is how a driver retriggers it, and a common source of "the
    /// envelope only fires once" bugs.
    #[test]
    fn writing_the_shape_register_retriggers_the_envelope() {
        let mut chip = Ay8910::new();
        chip.reset(CLOCK, false);
        chip.write(0, 13, 0x09); // decay once and hold
        for _ in 0..2000 {
            chip.tick();
        }
        assert!(chip.envelope.holding, "it should have finished");

        chip.write(0, 13, 0x09); // the same shape again
        assert!(!chip.envelope.holding, "the write must restart it");
        assert_eq!(chip.envelope.step, 0);
    }

    /// The noise register's period is five bits and it advances at half the
    /// tone rate, so a shared period puts noise an octave below the tone.
    #[test]
    fn the_noise_register_runs_at_half_the_tone_rate() {
        let mut chip = Ay8910::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0, 0x01); // tone A period 1
        chip.write(0, 6, 0x01); // noise period 1

        let (mut tone_flips, mut noise_flips) = (0u32, 0u32);
        let (mut last_t, mut last_n) = (chip.tones[0].output, chip.noise.output());
        for _ in 0..20_000 {
            chip.tick();
            if chip.tones[0].output != last_t {
                tone_flips += 1;
            }
            if chip.noise.output() != last_n {
                noise_flips += 1;
            }
            last_t = chip.tones[0].output;
            last_n = chip.noise.output();
        }
        assert!(noise_flips > 0, "the noise register must run");
        assert!(
            tone_flips > noise_flips,
            "noise at the same period must be slower: {tone_flips} vs {noise_flips}"
        );
    }

    /// A zero period must not divide by zero, and must behave as one.
    #[test]
    fn a_zero_period_behaves_as_one() {
        let mut chip = Ay8910::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0, 0x00);
        chip.write(0, 1, 0x00);
        chip.write(0, 6, 0x00);
        chip.write(0, 11, 0x00);
        chip.write(0, 12, 0x00);
        chip.write(0, 13, 0x08);
        chip.write(0, 7, 0x38);
        chip.write(0, 8, 0x10); // follow the envelope
        let out = render(&mut chip, 4000);
        assert!(energy(&out) > 0, "a zero period must still run");
    }

    #[test]
    fn the_native_rate_divides_the_clock_by_eight() {
        let mut chip = Ay8910::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 8);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// Chunking must not change the audio.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        fn set_up(chip: &mut Ay8910) {
            chip.reset(CLOCK, false);
            for (register, value) in [
                (0u16, 0x40u16),
                (2, 0x60),
                (4, 0x80),
                (6, 0x0F),
                (7, 0x00),
                (8, 0x0F),
                (9, 0x10),
                (10, 0x0A),
                (11, 0x00),
                (12, 0x01),
                (13, 0x0E),
            ] {
                chip.write(0, register, value);
            }
        }
        let mut whole = Ay8910::new();
        set_up(&mut whole);
        let mut one_go = vec![0i32; 2048 * 2];
        whole.render(&mut one_go);

        let mut chunked = Ay8910::new();
        set_up(&mut chunked);
        let mut piecemeal = vec![0i32; 2048 * 2];
        for chunk in piecemeal.chunks_mut(64 * 2) {
            chunked.render(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// Three channels at full volume, and the headroom convention that implies.
    #[test]
    fn a_full_chip_uses_the_range_without_clipping_it() {
        let mut chip = Ay8910::new();
        chip.reset(CLOCK, false);
        chip.write(0, 7, 0xFF); // every gate open
        for register in 8..=10 {
            chip.write(0, register, 0x0F);
        }
        let loudest = render(&mut chip, 500)
            .iter()
            .map(|&s| s.abs())
            .max()
            .unwrap_or(0);
        assert_eq!(loudest, PEAK * 3, "three channels at the top");
        assert!(
            loudest < i32::from(i16::MAX),
            "three channels must not need the mixer's clamp on their own"
        );
    }
}
