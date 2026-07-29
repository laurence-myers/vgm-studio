//! The TI SN76489 and its Sega VDP variant: three square waves and a noise
//! channel.
//!
//! The chip behind the Master System, Game Gear, BBC Micro, ColecoVision and
//! the Mega Drive's second voice. It is the simplest chip the VGM format
//! carries, which is why it is the first one here: everything it does is
//! documented behaviour with no analogue subtlety to approximate.
//!
//! **Registers.** One data port, eight registers, addressed by the write
//! itself. A byte with bit 7 set latches: `1 cc t dddd` picks channel `cc`
//! (0-2 tone, 3 noise), register type `t` (0 = tone/noise, 1 = volume), and
//! carries the low four data bits. A byte with bit 7 clear is a follow-up:
//! `0 dddddd` supplies the *high* six bits of the latched tone register,
//! making ten, or replaces a volume or noise register outright.
//!
//! **Tone.** Each channel counts its ten-bit period down at clock/16 and flips
//! its output when it reaches zero, giving a square wave of clock/(32 × period).
//! A period of zero never expires, so the output stays high -- which is how
//! games play samples through the chip, by writing volumes at audio rate.
//!
//! **Noise.** A shift register -- fifteen, sixteen or seventeen bits wide, with
//! taps to match, both named by the file's header -- clocked at one of three
//! fixed divisors or at tone 2's rate. With feedback on it is white noise;
//! with it off the register cycles, giving a periodic buzz an octave and a half
//! below the tone.
//!
//! **Volume.** Four bits of *attenuation*, 2 dB a step, 15 being silence.
//!
//! Route B: written from the documented behaviour, not ported, which is what
//! lets it sit in a `MIT OR Apache-2.0` crate. Every number here is derived in
//! a test rather than transcribed.
//!
//! Not modelled yet, and worth knowing: the Game Gear's stereo register
//! (`addr` 1 -- consumed so it cannot masquerade as a latch byte, but the
//! panning itself is not applied) and the T6W28's split-chip addressing.

use crate::chip::ChipCore;

/// The chip divides its input clock by 16 to get its internal tick.
const CLOCK_DIVIDER: u32 = 16;

/// Peak amplitude of one channel at full volume.
///
/// **Measured, not chosen.** With the attenuator curve corrected, this chip
/// came out a uniform 2.005x louder than VGMPlay across twelve single-chip
/// corpus files -- every partial in the spectrum at the same ratio, which is
/// what a single scalar looks like and what a curve error does not. 8000/2.005
/// is 3990; 4000 is taken because a quarter of a percent is far inside the
/// spread and this is also exactly an eighth of full scale, so the four
/// channels together cannot reach half of it and a chip playing flat out has
/// no way to clip the mixer on its own.
const PEAK: i32 = 4000;

/// Attenuation, 2 dB a step, from full scale to silence.
///
/// `LEVELS[n] = PEAK x 10^(-2n/20)`, with `LEVELS[15] = 0` by definition -- the
/// attenuator's last step is off, not merely quiet.
/// `the_volume_table_is_two_decibels_a_step` recomputes it.
///
/// **The decibel is a ratio of powers only when the quantity is a power.** This
/// table held `10^(-0.1 x 2n)` until the reference-parity harness caught it:
/// that is 2 dB a step of *power*, which is 4 dB a step of the amplitude the
/// table actually holds, so every step but the first was an octave of
/// attenuation too dark. The old table is precisely this one with every other
/// entry taken. Nothing local could have found it -- the self-test recomputed
/// the same wrong formula and agreed with itself, and by ear a chiptune that is
/// uniformly a little top-heavy sounds like a chiptune. What found it was the
/// SN76489 scoring 0.58 against VGMPlay with its *noise* matching to within
/// 0.7%: partials at attenuation 0 agreed exactly, and the rest fell away as
/// `0.795^n`, which is the ratio between the two formulas.
const LEVELS: [i32; 16] = [
    PEAK, 3177, 2524, 2005, 1592, 1265, 1005, 798, 634, 504, 400, 318, 252, 200, 159, 0,
];

/// The shift register's taps and width when the file does not say.
///
/// **The file usually does say, and it usually says something else.** VGM 1.10
/// added a feedback mask and a register width to the header precisely because
/// the family disagrees, and the corpus exercises the disagreement: the Sega
/// VDP's PSG is 16 bits tapped at 0 and 3, the discrete TI part 15 bits tapped
/// at 0 and 1, and Konami's arcade rips declare **seventeen** bits tapped at 2
/// and 3 -- a width a `u16` register cannot hold at all, which is why the one
/// here is a `u32`. This core had the Sega shape compiled in and ignored the
/// header entirely, so on every one of those files its noise channel was not a
/// poor match for the reference's but an unrelated pseudo-random sequence.
///
/// Reading the header did *not* move this chip's parity score (0.5844 before,
/// 0.5848 after), so it was not what that score was measuring. It is still the
/// difference between emitting the sequence the file asks for and emitting a
/// different part's.
///
/// These stay as the fallback for the pre-1.10 files that carry no such field,
/// where Sega hardware is the safer guess.
const DEFAULT_TAPS: u32 = 0x0009;
const DEFAULT_WIDTH: u8 = 16;

/// One square-wave channel.
#[derive(Debug, Default, Clone, Copy)]
struct Tone {
    /// The ten-bit period, in internal ticks per half-cycle.
    period: u16,
    /// Ticks left before the output flips.
    counter: i32,
    /// The square wave's current half: `true` is high.
    high: bool,
    /// Attenuation, 0 (loudest) to 15 (silent).
    attenuation: u8,
}

impl Tone {
    /// Advances one internal tick and returns this channel's contribution.
    fn tick(&mut self) -> i32 {
        // A period of zero never expires: the output holds high, which is what
        // sample playback through the volume register depends on.
        if self.period > 0 {
            self.counter -= 1;
            if self.counter <= 0 {
                self.counter = i32::from(self.period);
                self.high = !self.high;
            }
        } else {
            self.high = true;
        }
        let level = LEVELS[usize::from(self.attenuation & 0x0F)];
        if self.high { level } else { -level }
    }
}

/// The noise channel: a shift register clocked like a tone channel.
#[derive(Debug, Clone, Copy)]
struct Noise {
    /// The low two bits of the noise register: which divisor clocks it.
    rate: u8,
    /// Bit 2 of the noise register: white noise rather than periodic.
    white: bool,
    counter: i32,
    /// Toggles every expiry; the register shifts on every second one, so the
    /// noise runs at half the rate the divisor suggests -- the same halving a
    /// tone channel's square wave gets.
    half: bool,
    /// Wider than the sixteen bits the Sega part uses: Konami's arcade rips
    /// declare a seventeen-bit register, which a `u16` cannot hold at all.
    shift: u32,
    attenuation: u8,
    /// Which bits feed back, from the file's header.
    taps: u32,
    /// How many bits of shift register, from the file's header. The feedback
    /// re-enters at the top of *this*, not at any fixed bit.
    width: u8,
}

impl Noise {
    /// The value a reset leaves in the register: the top bit of its width.
    const fn seed(width: u8) -> u32 {
        1 << (width.saturating_sub(1) & 0x1f)
    }
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            rate: 0,
            white: false,
            counter: 0,
            half: false,
            shift: Self::seed(DEFAULT_WIDTH),
            attenuation: 0x0F,
            taps: DEFAULT_TAPS,
            width: DEFAULT_WIDTH,
        }
    }
}

impl Noise {
    /// Internal ticks per half-cycle, given tone 2's period for rate 3.
    const fn period(&self, tone2: u16) -> u16 {
        match self.rate & 0x03 {
            0 => 0x10,
            1 => 0x20,
            2 => 0x40,
            _ => tone2,
        }
    }

    fn tick(&mut self, tone2: u16) -> i32 {
        let period = self.period(tone2);
        if period > 0 {
            self.counter -= 1;
            if self.counter <= 0 {
                self.counter = i32::from(period);
                self.half = !self.half;
                if self.half {
                    // White noise feeds back the parity of the tapped bits;
                    // periodic noise feeds back bit 0, so the register simply
                    // cycles and the "noise" is a very low square wave.
                    let feedback = if self.white {
                        (self.shift & self.taps).count_ones() & 1
                    } else {
                        self.shift & 1
                    };
                    // Back in at the top of the *declared* width, not at bit
                    // 15: a 15-bit register fed at bit 15 grows a sixteenth bit
                    // that never leaves, and the sequence is then neither
                    // part's.
                    let top = u32::from(self.width.saturating_sub(1) & 0x1f);
                    self.shift = (self.shift >> 1) | (feedback << top);
                }
            }
        }
        let level = LEVELS[usize::from(self.attenuation & 0x0F)];
        if self.shift & 1 != 0 { level } else { -level }
    }
}

/// Which register the next data byte completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Latch {
    /// 0-2 tone, 3 noise.
    channel: u8,
    /// Whether the latched register is the channel's volume.
    volume: bool,
}

/// The chip.
#[derive(Debug, Clone)]
pub struct Sn76489 {
    tones: [Tone; 3],
    noise: Noise,
    latch: Latch,
    rate: u32,
}

impl Default for Sn76489 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sn76489 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            // Silent at power-up, which is what a real chip's volume registers
            // read: full attenuation.
            tones: [Tone {
                attenuation: 0x0F,
                ..Tone::default()
            }; 3],
            noise: Noise::default(),
            latch: Latch {
                channel: 0,
                volume: false,
            },
            rate: 0,
        }
    }

    /// One byte written to the chip's data port.
    fn write_byte(&mut self, byte: u8) {
        if byte & 0x80 != 0 {
            // A latch byte: pick the register, and take the low four data bits.
            self.latch = Latch {
                channel: (byte >> 5) & 0x03,
                volume: byte & 0x10 != 0,
            };
            self.apply(u16::from(byte & 0x0F), true);
        } else {
            // A data byte: the high six bits of a ten-bit tone period, or a
            // whole volume or noise register.
            self.apply(u16::from(byte & 0x3F), false);
        }
    }

    /// Applies `data` to the latched register. `low` distinguishes the four-bit
    /// half of a tone period from the six-bit half.
    fn apply(&mut self, data: u16, low: bool) {
        let channel = usize::from(self.latch.channel);
        match (self.latch.volume, channel) {
            (true, 3) => self.noise.attenuation = (data & 0x0F) as u8,
            (true, _) => self.tones[channel].attenuation = (data & 0x0F) as u8,
            (false, 3) => {
                // The noise register is four bits however it arrives, and
                // writing it resets the shift register -- which is what makes a
                // drum hit start from the same noise every time.
                self.noise.rate = (data & 0x03) as u8;
                self.noise.white = data & 0x04 != 0;
                self.noise.shift = Noise::seed(self.noise.width);
                self.noise.counter = 0;
            }
            (false, _) => {
                let tone = &mut self.tones[channel];
                tone.period = if low {
                    (tone.period & 0x03F0) | (data & 0x0F)
                } else {
                    (tone.period & 0x000F) | ((data & 0x3F) << 4)
                };
            }
        }
    }
}

impl ChipCore for Sn76489 {
    fn reset(&mut self, clock: u32, _variant: bool) {
        *self = Self::new();
        self.rate = (clock / CLOCK_DIVIDER).max(1);
    }

    /// Takes the noise shape from the header, when the header states one.
    ///
    /// A zero in either field means the file predates VGM 1.10 or its ripper
    /// left it blank, and the compiled-in Sega shape stands. A width outside
    /// 1..=16 is refused rather than clamped: it is a corrupt header, and
    /// guessing at it would produce noise that is wrong in a way nothing
    /// downstream could attribute.
    fn configure(&mut self, settings: &dro_core::vgm::ChipSettings) {
        if settings.sn76489_feedback != 0 {
            self.noise.taps = u32::from(settings.sn76489_feedback);
        }
        if (1..=32).contains(&settings.sn76489_shift_width) {
            self.noise.width = settings.sn76489_shift_width;
        }
        self.noise.shift = Noise::seed(self.noise.width);
    }

    fn native_rate(&self) -> u32 {
        self.rate.max(1)
    }

    /// The chip has one data port and no register address: the byte *is* the
    /// write, which is how the VGM `0x50` command carries it.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        // `addr` 1 is the Game Gear stereo mask (`0x4F`/`0x3F`) -- a register
        // on the Game Gear's ASIC, not on the chip, which is exactly how the
        // decoder and libvgm's `SN76496_W_GGST` both file it. This mono core
        // does not model it, but *consuming* it is load-bearing all the same:
        // fallen into `write_byte` -- which this did until 2026-07-29 -- a
        // mask reads as a latch byte, and the overwhelmingly common `0xFF`
        // ("all four channels to both speakers") parses as "noise volume,
        // attenuation 15". The noise channel died on the spot, and the next
        // data byte landed on the wrong latch besides. Not a corner: 13,344
        // masks across 8,067 of the corpus's 11,845 SN76489 files -- 68% of
        // them -- and all but one of those bytes has bit 7 set.
        if addr != 0 {
            return;
        }
        self.write_byte((data & 0xFF) as u8);
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let tone2 = self.tones[2].period;
            let mut sample = 0i32;
            for tone in &mut self.tones {
                sample += tone.tick();
            }
            sample += self.noise.tick(tone2);
            // Mono: the chip has one output pin. (The Game Gear's stereo is a
            // separate port this does not model yet.)
            frame[0] = sample;
            frame[1] = sample;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chip clocked as a Master System's, and silent to start with.
    fn chip() -> Sn76489 {
        let mut chip = Sn76489::new();
        chip.reset(3_579_545, false);
        chip
    }

    /// Renders `frames` and returns the left channel of each.
    fn render(chip: &mut Sn76489, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|frame| frame[0]).collect()
    }

    /// The Game Gear stereo mask (`addr` 1) must not be read as a latch byte.
    /// `0xFF` -- "everything to both speakers", written by 68% of the corpus's
    /// SN76489 files -- parses as "noise volume, attenuation 15" otherwise,
    /// and the mask corrupts the latch for the data byte after it too.
    #[test]
    fn a_game_gear_stereo_mask_is_not_a_latch_byte() {
        let mut chip = chip();
        chip.write(0, 0, 0xF0); // latch: noise volume, full
        let loud: i64 = render(&mut chip, 200)
            .iter()
            .map(|&s| i64::from(s.abs()))
            .sum();
        assert!(loud > 0, "the noise channel sounds");

        chip.write(0, 1, 0xFF); // the stereo mask, on its own address
        let after: i64 = render(&mut chip, 200)
            .iter()
            .map(|&s| i64::from(s.abs()))
            .sum();
        assert!(after > 0, "a mask is not \"noise volume, attenuation 15\"");

        // And the latch survives it: a mask between a latch byte and its data
        // byte must not redirect the data byte.
        chip.write(0, 0, 0x8E); // latch: tone 0 period, low nibble 0xE
        chip.write(0, 1, 0xFF); // mask in between
        chip.write(0, 0, 0x1F); // data: the period's high six bits
        assert_eq!(chip.tones[0].period, (0x1F << 4) | 0xE);
    }

    #[test]
    fn the_volume_table_is_two_decibels_a_step() {
        for (step, &level) in LEVELS.iter().enumerate().take(15) {
            // Amplitude, so 20 log10 -- see the note on LEVELS for what
            // getting this wrong cost and what caught it.
            let expected = (f64::from(PEAK) * 10f64.powf(-2.0 * step as f64 / 20.0)).round() as i32;
            assert_eq!(level, expected, "step {step}");
        }
        assert_eq!(LEVELS[15], 0, "the last step is off, not quiet");

        // Two steps must halve the amplitude *four* times over fifteen steps,
        // not eight: an independent statement of the same fact, so a future
        // edit cannot make the formula and the table agree on something wrong
        // together the way the previous pair did.
        let full = f64::from(LEVELS[0]);
        let quietest = f64::from(LEVELS[14]);
        let span_db = 20.0 * (full / quietest).log10();
        assert!(
            (span_db - 28.0).abs() < 0.1,
            "fourteen steps of 2 dB is 28 dB of range, not {span_db:.1}"
        );
    }

    #[test]
    fn the_native_rate_is_the_clock_over_sixteen() {
        let chip = chip();
        assert_eq!(chip.native_rate(), 3_579_545 / 16);
    }

    #[test]
    fn a_latch_byte_and_a_data_byte_make_a_ten_bit_period() {
        let mut chip = chip();
        // 0x80 | 0x0E: latch tone 0's period, low nibble 0x0E.
        chip.write(0, 0, 0x8E);
        assert_eq!(chip.tones[0].period, 0x00E);
        // 0x3F: the high six bits.
        chip.write(0, 0, 0x3F);
        assert_eq!(chip.tones[0].period, 0x3FE);
        // A second data byte replaces only the high bits again.
        chip.write(0, 0, 0x01);
        assert_eq!(chip.tones[0].period, 0x01E);
    }

    #[test]
    fn a_volume_latch_writes_four_bits_and_stays_latched() {
        let mut chip = chip();
        chip.write(0, 0, 0x90); // latch tone 0's volume, 0 = loudest
        assert_eq!(chip.tones[0].attenuation, 0x00);
        // A following data byte writes the volume again, not a period.
        chip.write(0, 0, 0x07);
        assert_eq!(chip.tones[0].attenuation, 0x07);
        assert_eq!(chip.tones[0].period, 0, "the period was never touched");
    }

    #[test]
    fn each_channel_has_its_own_registers() {
        let mut chip = chip();
        chip.write(0, 0, 0xA0); // tone 1 period, low nibble 0
        chip.write(0, 0, 0x02); // high bits
        chip.write(0, 0, 0xC5); // tone 2 period, low nibble 5
        chip.write(0, 0, 0xB1); // tone 1 volume 1
        assert_eq!(chip.tones[1].period, 0x020);
        assert_eq!(chip.tones[2].period, 0x005);
        assert_eq!(chip.tones[1].attenuation, 1);
        assert_eq!(
            chip.tones[0].attenuation, 0x0F,
            "untouched, so still silent"
        );
    }

    #[test]
    fn a_tone_flips_every_period_ticks() {
        let mut chip = chip();
        chip.write(0, 0, 0x84); // tone 0 period = 4
        chip.write(0, 0, 0x00);
        chip.write(0, 0, 0x90); // tone 0 at full volume
        // Everything else silent, so the sum is this channel alone.
        let samples = render(&mut chip, 16);
        // It starts low, flips after four ticks, and holds four at a time.
        let signs: Vec<i32> = samples.iter().map(|&s| s.signum()).collect();
        assert_eq!(
            signs,
            [1, 1, 1, 1, -1, -1, -1, -1, 1, 1, 1, 1, -1, -1, -1, -1],
            "a square wave of period 4, in ticks per half-cycle"
        );
    }

    #[test]
    fn a_period_sounds_at_the_frequency_the_datasheet_says() {
        // f = clock / (32 x period). A period of 254 at 3.579545 MHz is an A
        // just above concert pitch, which is what a Master System playing 440 Hz
        // actually writes.
        const CLOCK: u32 = 3_579_545;
        const PERIOD: u32 = 254;
        let mut chip = chip();
        chip.write(0, 0, 0x80 | (PERIOD & 0x0F) as u16);
        chip.write(0, 0, (PERIOD >> 4) as u16);
        chip.write(0, 0, 0x90);

        // Count the rising edges over a second of the chip's own rate.
        let rate = chip.native_rate() as usize;
        let samples = render(&mut chip, rate);
        let edges = samples
            .windows(2)
            .filter(|pair| pair[0] < 0 && pair[1] > 0)
            .count();

        let expected = CLOCK as f64 / (32.0 * f64::from(PERIOD));
        assert!(
            (edges as f64 - expected).abs() < 2.0,
            "{edges} cycles a second, the datasheet says {expected:.1}"
        );
    }

    #[test]
    fn a_zero_period_holds_the_output_high() {
        // What sample playback through the volume register depends on.
        let mut chip = chip();
        chip.write(0, 0, 0x80); // tone 0 period = 0
        chip.write(0, 0, 0x00);
        chip.write(0, 0, 0x90); // full volume
        let samples = render(&mut chip, 8);
        assert!(
            samples.iter().all(|&s| s > 0),
            "a zero period never expires, so the output never flips: {samples:?}"
        );
    }

    #[test]
    fn silence_is_silence() {
        // Every channel at full attenuation sums to nothing at all.
        let mut chip = chip();
        chip.write(0, 0, 0x84);
        chip.write(0, 0, 0x00);
        assert_eq!(render(&mut chip, 32), vec![0; 32]);
    }

    #[test]
    fn periodic_noise_repeats_and_white_noise_does_not() {
        // The shift register is 16 bits, so periodic noise cycles in 16 shifts.
        let mut periodic = chip();
        periodic.write(0, 0, 0xE0); // noise: periodic, fastest rate
        periodic.write(0, 0, 0xF0); // noise at full volume
        let period = 0x10 * 2; // ticks per shift: the divisor, halved twice over
        let run = render(&mut periodic, period * 16 * 2);
        let (first, second) = run.split_at(period * 16);
        assert_eq!(first, second, "periodic noise is periodic");

        let mut white = chip();
        white.write(0, 0, 0xE4); // noise: white, fastest rate
        white.write(0, 0, 0xF0);
        let run = render(&mut white, period * 16 * 2);
        let (first, second) = run.split_at(period * 16);
        assert_ne!(first, second, "white noise is not");
    }

    /// The header's noise shape has to reach the noise channel, and reaching it
    /// has to *change* the sound. Every SN76489 file in the corpus declares the
    /// 15-bit TI part while this core defaulted to the 16-bit Sega one, so for
    /// every one of them the noise was an unrelated sequence -- a fault no
    /// local test could see, because both sequences are perfectly good noise.
    #[test]
    fn the_headers_noise_shape_reaches_the_shift_register() {
        fn noise_with(feedback: u16, width: u8) -> Vec<i32> {
            let mut chip = chip();
            chip.configure(&dro_core::vgm::ChipSettings {
                sn76489_feedback: feedback,
                sn76489_shift_width: width,
                ..Default::default()
            });
            chip.write(0, 0, 0xE4); // white noise, fastest rate
            chip.write(0, 0, 0xF0); // full volume
            render(&mut chip, 0x10 * 2 * 40)
        }

        let sega = noise_with(0x0009, 16);
        let ti = noise_with(0x0003, 15);
        assert_ne!(sega, ti, "two parts, two sequences");

        // Periodic noise cycles in exactly as many shifts as the register is
        // wide, which is the sharpest statement that the width took effect.
        fn periodic_cycle(width: u8) -> usize {
            let mut chip = chip();
            chip.configure(&dro_core::vgm::ChipSettings {
                sn76489_feedback: 0x0003,
                sn76489_shift_width: width,
                ..Default::default()
            });
            chip.write(0, 0, 0xE0); // periodic, fastest rate
            chip.write(0, 0, 0xF0);
            let step = 0x10 * 2;
            let run = render(&mut chip, step * usize::from(width) * 2);
            let (first, second) = run.split_at(step * usize::from(width));
            assert_eq!(
                first, second,
                "width {width} should cycle in {width} shifts"
            );
            usize::from(width)
        }
        assert_eq!(periodic_cycle(15), 15);
        assert_eq!(periodic_cycle(16), 16);
        // Seventeen, because the corpus asks for it and a `u16` register
        // cannot answer.
        assert_eq!(periodic_cycle(17), 17);

        // A blank or impossible header leaves the compiled-in shape alone,
        // rather than producing noise nothing downstream could account for.
        let mut chip = chip();
        let before = chip.noise.taps;
        chip.configure(&dro_core::vgm::ChipSettings::default());
        assert_eq!(chip.noise.taps, before);
        assert_eq!(chip.noise.width, DEFAULT_WIDTH);
        chip.configure(&dro_core::vgm::ChipSettings {
            sn76489_shift_width: 200,
            ..Default::default()
        });
        assert_eq!(
            chip.noise.width, DEFAULT_WIDTH,
            "a corrupt width is refused"
        );
    }

    #[test]
    fn writing_the_noise_register_restarts_the_shift_register() {
        // A drum hit starts from the same noise every time.
        let mut chip = chip();
        chip.write(0, 0, 0xE4);
        chip.write(0, 0, 0xF0);
        let first = render(&mut chip, 200);
        chip.write(0, 0, 0xE4);
        let again = render(&mut chip, 200);
        assert_eq!(first, again);
    }

    #[test]
    fn noise_rate_three_follows_tone_two() {
        let mut chip = chip();
        chip.write(0, 0, 0xC8); // tone 2 period = 8
        chip.write(0, 0, 0x00);
        chip.write(0, 0, 0xE3); // noise: periodic, rate 3
        assert_eq!(chip.noise.period(chip.tones[2].period), 8);
        // And it moves when tone 2 does.
        chip.write(0, 0, 0xC2);
        chip.write(0, 0, 0x00);
        assert_eq!(chip.noise.period(chip.tones[2].period), 2);
    }

    #[test]
    fn a_reset_silences_everything_it_was_told() {
        let mut chip = chip();
        chip.write(0, 0, 0x84);
        chip.write(0, 0, 0x90);
        chip.reset(3_579_545, false);
        assert_eq!(render(&mut chip, 8), vec![0; 8]);
        assert_eq!(chip.tones[0].period, 0);
        assert_eq!(chip.tones[0].attenuation, 0x0F);
    }

    #[test]
    fn a_full_chip_stays_inside_the_mixers_headroom() {
        let mut chip = chip();
        for latch in [0x80, 0xA0, 0xC0] {
            chip.write(0, 0, latch | 0x01);
            chip.write(0, 0, 0x00);
        }
        chip.write(0, 0, 0xE4);
        for volume in [0x90, 0xB0, 0xD0, 0xF0] {
            chip.write(0, 0, volume);
        }
        let peak = render(&mut chip, 4096)
            .into_iter()
            .map(i32::abs)
            .max()
            .unwrap_or(0);
        assert!(
            peak <= PEAK * 4 && peak > PEAK,
            "four channels flat out: {peak}"
        );
        assert!(
            peak < i32::from(i16::MAX),
            "and still short of clipping the mix on its own"
        );
    }
}
