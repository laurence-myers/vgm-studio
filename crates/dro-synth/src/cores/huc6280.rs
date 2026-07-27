//! The HuC6280's PSG: the PC Engine / TurboGrafx-16's six wavetable channels.
//!
//! 4,178 files in the VGMRips corpus. Unusual among the chips here in having
//! **no oscillator at all** -- every channel plays a 32-entry table of 5-bit
//! samples, so the same hardware produces a square, a sine, a bass or a
//! sampled drum depending only on what was written into it.
//!
//! **Route B, from the documented register interface**, so it lives in the
//! permissive crate.
//!
//! # Three things worth knowing
//!
//! - **The registers are banked behind a channel-select.** Writing `$0800`
//!   chooses which of the six channels every subsequent write lands on, so a
//!   core that ignores it puts every note on channel 1.
//! - **Stereo is per channel and coarse**: four bits of attenuation each side,
//!   multiplied by a global four-bit pair. This chip pans, and a rip sounds
//!   wrong in mono.
//! - **Channels 5 and 6 can be noise instead**, and channels 1 and 2 can be
//!   driven by an LFO. The noise is modelled; the LFO is not (see below).
//!
//! Not modelled: the LFO on channels 1-2 (`$0808`/`$0809`), which a handful of
//! games use for vibrato, and the timer/IRQ registers, which carry no audio.

use crate::chip::ChipCore;

/// Master clocks averaged into one output sample.
///
/// The chip's own rate is far above anything the output stage wants, so this
/// decimates on the way out -- 64 puts the native rate near 55.9 kHz for the
/// usual 3.58 MHz clock, and the averaging is the anti-alias filter.
const CYCLES_PER_SAMPLE: u32 = 64;

/// Samples in one channel's wavetable.
const WAVE_LEN: usize = 32;

/// Peak amplitude of one channel at full volume, per side.
///
/// Six channels, so a chip playing flat out reaches six times this -- the same
/// headroom convention the other clean-room cores use.
#[cfg(test)]
const PEAK: i32 = 4_000;

/// Attenuation in 1.5 dB steps, as a 16-bit fixed-point fraction of unity.
///
/// The volume registers are *attenuations*: 0 is loudest. Each of the 4-bit
/// balance fields and the 5-bit channel volume steps by 1.5 dB, and they add,
/// so one curve indexed by the summed steps serves all of them.
/// `the_attenuation_curve_is_one_and_a_half_decibels_a_step` regenerates it.
const ATTENUATION: [i32; 48] = [
    65536, 55142, 46396, 39037, 32846, 27636, 23253, 19565, 16462, 13851, 11654, 9806, 8250, 6942,
    5841, 4915, 4135, 3479, 2927, 2463, 2072, 1744, 1467, 1234, 1039, 874, 735, 619, 521, 438, 369,
    310, 261, 220, 185, 155, 131, 110, 93, 78, 66, 55, 46, 39, 33, 28, 23, 20,
];

/// One of the six channels.
#[derive(Debug, Clone, Copy)]
struct Channel {
    /// Twelve bits. The wave advances one step every `frequency` master clocks.
    frequency: u16,
    counter: u32,
    position: u8,
    /// Where the next `$0806` write lands while the channel is being loaded.
    write_index: u8,
    wave: [u8; WAVE_LEN],
    /// Five bits of *attenuation*, and the channel's on/off bit.
    volume: u8,
    on: bool,
    /// Direct D/A: `$0806` writes go straight to the output instead of the
    /// table, which is how the PC Engine plays samples.
    dda: bool,
    dda_sample: u8,
    /// Four bits of attenuation each, left then right.
    balance: [u8; 2],
    /// Channels 5 and 6 only.
    noise_on: bool,
    noise_frequency: u8,
    noise_counter: u32,
    noise_shift: u32,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            frequency: 0,
            counter: 0,
            position: 0,
            write_index: 0,
            wave: [0; WAVE_LEN],
            volume: 0,
            on: false,
            dda: false,
            dda_sample: 0,
            balance: [0; 2],
            noise_on: false,
            noise_frequency: 0,
            noise_counter: 0,
            // Non-zero, or the register would never produce feedback.
            noise_shift: 0x0001_FFFF,
        }
    }
}

impl Channel {
    /// The master-clock period of one wave step. Zero behaves as 4096, the
    /// full twelve-bit span, rather than dividing by nothing.
    fn period(&self) -> u32 {
        if self.frequency == 0 {
            0x1000
        } else {
            u32::from(self.frequency)
        }
    }

    /// The noise generator's period, from its own five-bit register.
    fn noise_period(&self) -> u32 {
        // Documented as `(31 - n) * 64`, so a register of 31 is the fastest.
        (u32::from(31 - (self.noise_frequency & 0x1F)) + 1) * 64
    }

    fn advance(&mut self, cycles: u32) {
        if !self.on {
            return;
        }
        if self.noise_on {
            let period = self.noise_period();
            self.noise_counter += cycles;
            while self.noise_counter >= period {
                self.noise_counter -= period;
                let feedback = (self.noise_shift ^ (self.noise_shift >> 1)) & 1;
                self.noise_shift = (self.noise_shift >> 1) | (feedback << 16);
            }
            return;
        }
        if self.dda {
            // Direct D/A holds whatever was written; nothing steps.
            return;
        }
        let period = self.period();
        self.counter += cycles;
        while self.counter >= period {
            self.counter -= period;
            self.position = (self.position + 1) % WAVE_LEN as u8;
        }
    }

    /// The channel's raw 5-bit sample, centred on zero.
    fn sample(&self) -> i32 {
        if !self.on {
            return 0;
        }
        let raw = if self.noise_on {
            // Full swing either way, which is what makes the noise channel far
            // louder than a wave of the same volume.
            if self.noise_shift & 1 == 0 { 0 } else { 31 }
        } else if self.dda {
            u32::from(self.dda_sample)
        } else {
            u32::from(self.wave[usize::from(self.position)])
        };
        // The table holds unsigned 5-bit samples; the DAC centres them.
        raw as i32 - 16
    }
}

/// The HuC6280's PSG.
#[derive(Debug, Default)]
pub struct HuC6280 {
    rate: u32,
    channels: [Channel; 6],
    /// Which channel the banked registers address.
    selected: usize,
    /// Global attenuation, left then right, four bits each.
    global_balance: [u8; 2],
}

impl HuC6280 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 44_100,
            ..Self::default()
        }
    }

    /// Applies one attenuation step count to a sample.
    fn attenuate(sample: i32, steps: usize) -> i32 {
        // Past the end of the curve is silence, not a wrap.
        let Some(&gain) = ATTENUATION.get(steps) else {
            return 0;
        };
        (sample * gain) >> 16
    }
}

impl ChipCore for HuC6280 {
    fn reset(&mut self, clock: u32, _variant: bool) {
        *self = Self {
            rate: (clock / CYCLES_PER_SAMPLE).max(1),
            ..Self::default()
        };
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The registers as the VGM numbers them, `0x00`-`0x09`.
    ///
    /// Everything from `0x02` on is *banked*: it lands on whichever channel
    /// `0x00` last selected. Missing that puts the whole tune on channel 1.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let value = (data & 0xFF) as u8;
        match addr & 0x0F {
            0x00 => self.selected = usize::from(value & 0x07).min(5),
            0x01 => {
                self.global_balance = [(value >> 4) & 0x0F, value & 0x0F];
            }
            register => {
                let channel = &mut self.channels[self.selected];
                match register {
                    0x02 => {
                        channel.frequency = (channel.frequency & 0x0F00) | u16::from(value);
                    }
                    0x03 => {
                        channel.frequency =
                            (channel.frequency & 0x00FF) | (u16::from(value & 0x0F) << 8);
                    }
                    0x04 => {
                        let was_on = channel.on;
                        channel.on = value & 0x80 != 0;
                        channel.dda = value & 0x40 != 0;
                        channel.volume = value & 0x1F;
                        // Switching a channel on resets its table pointer, which
                        // is what lets a driver retrigger a waveform cleanly.
                        if channel.on && !was_on {
                            channel.position = 0;
                            channel.counter = 0;
                        }
                        // Leaving DDA resets the write pointer, so the next
                        // table load starts at the beginning.
                        if !channel.dda {
                            channel.write_index = 0;
                        }
                    }
                    0x05 => channel.balance = [(value >> 4) & 0x0F, value & 0x0F],
                    0x06 => {
                        if channel.dda {
                            channel.dda_sample = value & 0x1F;
                        } else {
                            channel.wave[usize::from(channel.write_index)] = value & 0x1F;
                            channel.write_index = (channel.write_index + 1) % WAVE_LEN as u8;
                        }
                    }
                    // Noise is channels 5 and 6 only; on the others the
                    // register does nothing at all, as on hardware.
                    0x07 if self.selected >= 4 => {
                        channel.noise_on = value & 0x80 != 0;
                        channel.noise_frequency = value & 0x1F;
                    }
                    // 0x08 and 0x09 are the LFO, which is not modelled.
                    _ => {}
                }
            }
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let mut sides = [0i32; 2];
            for channel in &mut self.channels {
                channel.advance(CYCLES_PER_SAMPLE);
                let sample = channel.sample();
                if sample == 0 && !channel.on {
                    continue;
                }
                for (side, sum) in sides.iter_mut().enumerate() {
                    // The three attenuations add, in steps, before the curve is
                    // consulted -- which is what makes one table serve all of
                    // them and keeps the arithmetic integer.
                    let steps = usize::from(channel.volume)
                        + usize::from(channel.balance[side])
                        + usize::from(self.global_balance[side]);
                    *sum += Self::attenuate(sample, steps);
                }
            }
            // A 5-bit sample centred on zero spans -16..15, so scale it to the
            // per-channel peak the other cores use.
            frame[0] = sides[0] * 250;
            frame[1] = sides[1] * 250;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PC Engine's clock.
    const PCE: u32 = 3_579_545;

    fn render(chip: &mut HuC6280, frames: usize) -> Vec<[i32; 2]> {
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

    /// Loads channel `index` with a square wave and switches it on, loud and
    /// centred.
    fn key_on(chip: &mut HuC6280, index: u16, frequency: u16) {
        chip.write(0, 0x00, index); // select the channel
        chip.write(0, 0x04, 0x00); // off, and out of DDA, resetting the pointer
        for step in 0..WAVE_LEN as u16 {
            chip.write(0, 0x06, if step < 16 { 0x1F } else { 0x00 });
        }
        chip.write(0, 0x02, frequency & 0xFF);
        chip.write(0, 0x03, frequency >> 8);
        chip.write(0, 0x05, 0x00); // no balance attenuation
        chip.write(0, 0x04, 0x80); // on, volume 0 == loudest
    }

    /// Every gain recomputed from "1.5 dB a step" rather than transcribed.
    #[test]
    fn the_attenuation_curve_is_one_and_a_half_decibels_a_step() {
        for (step, &gain) in ATTENUATION.iter().enumerate() {
            let expected = 65536.0 * 10f64.powf(-1.5 * step as f64 / 20.0);
            assert_eq!(gain, expected.round() as i32, "step {step}");
        }
        assert_eq!(ATTENUATION[0], 65536, "step 0 is unity, not silence");
        // Four steps halve, which is what 1.5 dB a step means.
        let ratio = f64::from(ATTENUATION[0]) / f64::from(ATTENUATION[4]);
        assert!((1.9..=2.1).contains(&ratio), "four steps came to {ratio}x");
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_loaded_channel_is_not() {
        let mut chip = HuC6280::new();
        chip.reset(PCE, false);
        assert_eq!(energy(&render(&mut chip, 2000)), 0, "nothing loaded yet");

        key_on(&mut chip, 0, 0x100);
        assert!(energy(&render(&mut chip, 2000)) > 0);
    }

    /// **The banked registers.** `$0800` chooses which channel every later
    /// write lands on; ignoring it puts the whole tune on channel 1, which
    /// sounds like a mix that has lost most of its parts.
    #[test]
    fn the_channel_select_banks_every_later_write() {
        let mut chip = HuC6280::new();
        chip.reset(PCE, false);
        key_on(&mut chip, 3, 0x100);

        assert!(chip.channels[3].on, "channel 4 must be the one switched on");
        assert!(
            chip.channels
                .iter()
                .enumerate()
                .all(|(i, c)| i == 3 || !c.on),
            "no other channel may have been touched"
        );
        assert!(energy(&render(&mut chip, 2000)) > 0);
    }

    /// **Stereo is per channel.** Four bits of attenuation each side, and this
    /// chip really does pan -- a rip mixed to mono loses part of the
    /// arrangement.
    #[test]
    fn each_channel_has_its_own_balance() {
        let mut chip = HuC6280::new();
        chip.reset(PCE, false);
        key_on(&mut chip, 0, 0x100);
        // Left at full, right attenuated well past the curve.
        chip.write(0, 0x05, 0x0F);

        let frames = render(&mut chip, 4000);
        let left: i64 = frames.iter().map(|f| i64::from(f[0].abs())).sum();
        let right: i64 = frames.iter().map(|f| i64::from(f[1].abs())).sum();
        assert!(left > 0, "the left side must sound");
        assert!(
            right * 4 < left,
            "fifteen steps of attenuation is 22 dB: {left} vs {right}"
        );
    }

    /// The wavetable is what makes this chip: the same channel plays whatever
    /// was written into it, so two different tables must not sound the same.
    #[test]
    fn a_channel_plays_the_table_it_was_given() {
        fn render_table(fill: &dyn Fn(u16) -> u16) -> Vec<[i32; 2]> {
            let mut chip = HuC6280::new();
            chip.reset(PCE, false);
            chip.write(0, 0x00, 0);
            chip.write(0, 0x04, 0x00);
            for step in 0..WAVE_LEN as u16 {
                chip.write(0, 0x06, fill(step));
            }
            chip.write(0, 0x02, 0x40);
            chip.write(0, 0x03, 0x00);
            chip.write(0, 0x05, 0x00);
            chip.write(0, 0x04, 0x80);
            render(&mut chip, 2000)
        }
        let square = render_table(&|step| if step < 16 { 0x1F } else { 0x00 });
        let ramp = render_table(&|step| step & 0x1F);
        assert!(energy(&square) > 0 && energy(&ramp) > 0);
        assert_ne!(square, ramp, "the table must reach the output");
    }

    /// Direct D/A is how the PC Engine plays samples: writes go straight to the
    /// output instead of the table, and nothing steps.
    #[test]
    fn direct_d_to_a_writes_the_output_rather_than_the_table() {
        let mut chip = HuC6280::new();
        chip.reset(PCE, false);
        chip.write(0, 0x00, 0);
        chip.write(0, 0x05, 0x00);
        chip.write(0, 0x04, 0xC0); // on, DDA, loudest

        // A run of alternating extremes, which is the steepest thing the
        // channel can produce -- and impossible if the writes went to a table.
        let mut moved = 0i64;
        for step in 0..40 {
            chip.write(0, 0x06, if step % 2 == 0 { 0x00 } else { 0x1F });
            moved += energy(&render(&mut chip, 20));
        }
        assert!(moved > 0, "direct writes made no sound");
        assert_eq!(
            chip.channels[0].wave, [0; WAVE_LEN],
            "the table was written"
        );
    }

    /// Noise is channels 5 and 6 only, which is a real restriction rather than
    /// an accident of the register map.
    #[test]
    fn only_the_last_two_channels_can_be_noise() {
        let mut chip = HuC6280::new();
        chip.reset(PCE, false);

        chip.write(0, 0x00, 0); // channel 1
        chip.write(0, 0x07, 0x9F);
        assert!(
            !chip.channels[0].noise_on,
            "channel 1 has no noise generator"
        );

        chip.write(0, 0x00, 4); // channel 5
        chip.write(0, 0x07, 0x9F);
        assert!(chip.channels[4].noise_on);

        chip.write(0, 0x05, 0x00);
        chip.write(0, 0x04, 0x80);
        assert!(energy(&render(&mut chip, 4000)) > 0, "the noise must sound");
    }

    /// Switching a channel on restarts its table, so a retrigger begins at the
    /// same point in the waveform every time.
    #[test]
    fn switching_a_channel_on_restarts_its_table() {
        let mut chip = HuC6280::new();
        chip.reset(PCE, false);
        key_on(&mut chip, 0, 0x100);
        let _ = render(&mut chip, 500);
        assert_ne!(chip.channels[0].position, 0, "it should have moved on");

        chip.write(0, 0x04, 0x00); // off
        chip.write(0, 0x04, 0x80); // on again
        assert_eq!(
            chip.channels[0].position, 0,
            "a retrigger starts at the top"
        );
    }

    /// A zero frequency must not divide by zero; the documented behaviour is
    /// the full twelve-bit span.
    #[test]
    fn a_zero_frequency_behaves_as_the_full_span() {
        let mut chip = HuC6280::new();
        chip.reset(PCE, false);
        key_on(&mut chip, 0, 0x000);
        assert_eq!(chip.channels[0].period(), 0x1000);
        let _ = render(&mut chip, 4000);
    }

    #[test]
    fn the_native_rate_divides_the_clock() {
        let mut chip = HuC6280::new();
        chip.reset(PCE, false);
        assert_eq!(chip.native_rate(), PCE / CYCLES_PER_SAMPLE);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// Chunking must not change the audio.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        fn set_up(chip: &mut HuC6280) {
            chip.reset(PCE, false);
            for (index, frequency) in [(0u16, 0x100u16), (1, 0x180), (2, 0x0C0)] {
                key_on(chip, index, frequency);
            }
        }
        let mut whole = HuC6280::new();
        set_up(&mut whole);
        let mut one_go = vec![0i32; 2048 * 2];
        whole.render(&mut one_go);

        let mut chunked = HuC6280::new();
        set_up(&mut chunked);
        let mut piecemeal = vec![0i32; 2048 * 2];
        for chunk in piecemeal.chunks_mut(64 * 2) {
            chunked.render(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// Six channels at full volume, and the headroom that implies.
    #[test]
    fn a_full_chip_uses_the_range_without_clipping_it() {
        let mut chip = HuC6280::new();
        chip.reset(PCE, false);
        for index in 0..6u16 {
            key_on(&mut chip, index, 0x100 + index * 0x20);
        }
        let loudest = render(&mut chip, 4000)
            .iter()
            .flat_map(|f| [f[0].abs(), f[1].abs()])
            .max()
            .unwrap_or(0);
        assert!(
            loudest > PEAK,
            "six channels at the top peaked at only {loudest}"
        );
        assert!(
            loudest < i32::from(i16::MAX),
            "six channels must not need the mixer's clamp on their own: {loudest}"
        );
    }
}
