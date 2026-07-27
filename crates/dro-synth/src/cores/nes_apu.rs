//! The NES / Famicom APU: two pulses, a triangle, a noise channel and a DMC.
//!
//! Fourth by weight in the VGMRips corpus -- 7,191 files, 9.9% -- and the
//! second-most-recorded chip after the Mega Drive's pair.
//!
//! **Route B, from the NESdev documentation**, which is why it can live in a
//! permissively-licensed crate at all. The chip is exhaustively described:
//! every table here is a documented constant and every derived number is
//! recomputed in a test rather than transcribed, the same discipline
//! [`Sn76489`](super::Sn76489) set.
//!
//! # How it is clocked
//!
//! Everything runs off the CPU clock (1.789773 MHz on NTSC). The pulse and
//! noise timers tick on *APU* cycles, which are every second CPU cycle; the
//! triangle ticks every CPU cycle, which is why it can sound an octave higher
//! than its period suggests. A frame sequencer divides the CPU clock into
//! quarter- and half-frames that clock the envelopes, the sweeps and the length
//! counters.
//!
//! Rather than render at 1.79 MHz and make the engine resample from it, this
//! averages [`CYCLES_PER_SAMPLE`] cycles into one output sample. The average is
//! the decimation filter -- crude, but the alternative is aliasing the chip's
//! square edges straight into the audible band.
//!
//! # The mixer is not linear
//!
//! The two DACs sum through a resistor network, so twice the level is not twice
//! the volume. The documented approximation is two lookup tables, one indexed
//! by the summed pulse volumes and one by `3*triangle + 2*noise + dmc`. Tables
//! rather than the formula in the hot loop, because [`ChipCore`] requires
//! output that cannot differ across targets and floating point is exactly how
//! that promise gets broken.
//!
//! Not modelled: the FDS add-on (the header's bit 31, and its own register
//! range), and the open-bus behaviour of reads -- nothing here reads.

use crate::chip::ChipCore;

/// CPU cycles averaged into one output sample.
///
/// 32 puts the native rate at about 55.9 kHz on NTSC, comfortably above
/// anything the output stage wants, and makes the averaging window short enough
/// not to smear a DMC transient.
const CYCLES_PER_SAMPLE: u32 = 32;

/// Peak amplitude of the whole chip, mono.
///
/// The documented mixer formulas are normalised so that everything at once
/// approaches 1.0, so this is simply the scale that maps onto. It is *baked
/// into* [`PULSE_TABLE`] and [`TND_TABLE`] rather than multiplied at render
/// time, which is why nothing outside the tests names it -- and why the test
/// that regenerates those tables from the formulas is the only thing keeping
/// the three in step.
#[cfg(test)]
const PEAK: i32 = 24_000;

/// Length-counter reload values, indexed by the top five bits of `$4003`-style
/// writes. A documented table with no formula behind it.
const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22,
    192, 24, 72, 26, 16, 28, 32, 30,
];

/// Noise timer periods in APU cycles, NTSC. Also documented outright.
const NOISE_PERIODS: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];

/// DMC sample-rate periods in CPU cycles, NTSC.
const DMC_PERIODS: [u16; 16] = [
    428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54,
];

/// The four pulse duty cycles, as the eight-step sequences they really are.
const DUTY: [[u8; 8]; 4] = [
    [0, 1, 0, 0, 0, 0, 0, 0], // 12.5%
    [0, 1, 1, 0, 0, 0, 0, 0], // 25%
    [0, 1, 1, 1, 1, 0, 0, 0], // 50%
    [1, 0, 0, 1, 1, 1, 1, 1], // 25% inverted
];

/// The triangle's 32-step sequence: down from 15 and back up.
const TRIANGLE_STEPS: [i32; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    13, 14, 15,
];

/// Frame-sequencer step boundaries in CPU cycles, NTSC. Four-step mode.
const FRAME_STEPS_4: [u32; 4] = [7457, 14_913, 22_371, 29_829];
/// Five-step mode. The fourth step clocks nothing, which is the whole point of
/// the mode: writing `$4017` with bit 7 set silences the frame IRQ.
const FRAME_STEPS_5: [u32; 5] = [7457, 14_913, 22_371, 29_829, 37_281];

/// A four-bit envelope generator, shared by the pulses and the noise.
#[derive(Debug, Default, Clone, Copy)]
struct Envelope {
    /// The period, and the constant volume when `constant` is set -- the same
    /// four bits mean both, depending on the flag.
    volume: u8,
    constant: bool,
    /// Also the length counter's halt flag: one bit, two jobs.
    loops: bool,
    start: bool,
    divider: u8,
    decay: u8,
}

impl Envelope {
    /// One quarter-frame tick.
    fn clock(&mut self) {
        if self.start {
            self.start = false;
            self.decay = 15;
            self.divider = self.volume;
        } else if self.divider == 0 {
            self.divider = self.volume;
            if self.decay > 0 {
                self.decay -= 1;
            } else if self.loops {
                self.decay = 15;
            }
        } else {
            self.divider -= 1;
        }
    }

    fn output(&self) -> i32 {
        i32::from(if self.constant {
            self.volume
        } else {
            self.decay
        })
    }
}

/// A length counter: the gate that silences a channel when its note ends.
#[derive(Debug, Default, Clone, Copy)]
struct Length {
    counter: u8,
    enabled: bool,
}

impl Length {
    /// One half-frame tick. `halt` is the channel's own halt/loop flag.
    fn clock(&mut self, halt: bool) {
        if !halt && self.counter > 0 {
            self.counter -= 1;
        }
    }

    /// Disabling a channel through `$4015` clears its counter outright.
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.counter = 0;
        }
    }

    fn load(&mut self, index: u8) {
        if self.enabled {
            self.counter = LENGTH_TABLE[(index & 0x1F) as usize];
        }
    }

    fn active(&self) -> bool {
        self.counter > 0
    }
}

/// One of the two pulse channels.
#[derive(Debug, Default, Clone, Copy)]
struct Pulse {
    envelope: Envelope,
    length: Length,
    duty: u8,
    /// Eleven bits, and one *more* than the divider it drives.
    timer: u16,
    counter: u16,
    step: u8,
    // Sweep unit.
    sweep_enabled: bool,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_reload: bool,
    sweep_divider: u8,
    /// Pulse 1 negates with an extra -1; pulse 2 does not. The one place the
    /// two channels genuinely differ.
    ones_complement: bool,
}

impl Pulse {
    /// One APU cycle.
    fn clock(&mut self) {
        if self.counter == 0 {
            self.counter = self.timer;
            self.step = (self.step + 1) & 7;
        } else {
            self.counter -= 1;
        }
    }

    /// What the sweep unit would set the period to, used both to sweep and to
    /// decide whether the channel is muted.
    fn target_period(&self) -> u16 {
        let change = self.timer >> self.sweep_shift;
        if self.sweep_negate {
            let change = if self.ones_complement {
                change + 1
            } else {
                change
            };
            self.timer.saturating_sub(change)
        } else {
            self.timer.wrapping_add(change)
        }
    }

    /// A period below 8, or a sweep target past 11 bits, silences the channel
    /// even though the sequencer keeps running.
    fn muted(&self) -> bool {
        self.timer < 8 || self.target_period() > 0x7FF
    }

    /// One half-frame tick of the sweep unit.
    fn clock_sweep(&mut self) {
        if self.sweep_divider == 0 && self.sweep_enabled && self.sweep_shift > 0 && !self.muted() {
            self.timer = self.target_period();
        }
        if self.sweep_divider == 0 || self.sweep_reload {
            self.sweep_divider = self.sweep_period;
            self.sweep_reload = false;
        } else {
            self.sweep_divider -= 1;
        }
    }

    fn output(&self) -> i32 {
        if self.muted()
            || !self.length.active()
            || DUTY[self.duty as usize][self.step as usize] == 0
        {
            0
        } else {
            self.envelope.output()
        }
    }
}

/// The triangle channel, which has no volume control at all -- only on or off.
#[derive(Debug, Default, Clone, Copy)]
struct Triangle {
    length: Length,
    timer: u16,
    counter: u16,
    step: u8,
    linear_reload_value: u8,
    linear_counter: u8,
    linear_reload: bool,
    /// The control flag, which doubles as the length counter's halt.
    control: bool,
}

impl Triangle {
    /// One CPU cycle -- not one APU cycle, which is why the triangle reaches an
    /// octave above the pulses for the same period.
    fn clock(&mut self) {
        if self.counter == 0 {
            self.counter = self.timer;
            // Below period 2 the channel runs so fast it produces a DC-ish
            // whine rather than a note; hardware still steps, so this does too,
            // but `output` gates it.
            if self.linear_counter > 0 && self.length.active() {
                self.step = (self.step + 1) & 31;
            }
        } else {
            self.counter -= 1;
        }
    }

    /// One quarter-frame tick.
    fn clock_linear(&mut self) {
        if self.linear_reload {
            self.linear_counter = self.linear_reload_value;
        } else if self.linear_counter > 0 {
            self.linear_counter -= 1;
        }
        if !self.control {
            self.linear_reload = false;
        }
    }

    fn output(&self) -> i32 {
        // Ultrasonic periods are muted rather than emitted: hardware produces a
        // pop that no recording wants reproduced.
        if self.timer < 2 || !self.length.active() || self.linear_counter == 0 {
            0
        } else {
            TRIANGLE_STEPS[self.step as usize]
        }
    }
}

/// The noise channel: a fifteen-bit shift register and an envelope.
#[derive(Debug, Clone, Copy)]
struct Noise {
    envelope: Envelope,
    length: Length,
    /// Feedback from bit 6 instead of bit 1, giving a short, tonal buzz.
    short_mode: bool,
    period: u16,
    counter: u16,
    shift: u16,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            envelope: Envelope::default(),
            length: Length::default(),
            short_mode: false,
            period: NOISE_PERIODS[0],
            counter: NOISE_PERIODS[0],
            // Power-on state. A zero register would never produce feedback and
            // the channel would be silent for ever.
            shift: 1,
        }
    }
}

impl Noise {
    /// One APU cycle.
    fn clock(&mut self) {
        if self.counter == 0 {
            self.counter = self.period;
            let tap = if self.short_mode { 6 } else { 1 };
            let feedback = (self.shift & 1) ^ ((self.shift >> tap) & 1);
            self.shift = (self.shift >> 1) | (feedback << 14);
        } else {
            self.counter -= 1;
        }
    }

    fn output(&self) -> i32 {
        if self.shift & 1 == 1 || !self.length.active() {
            0
        } else {
            self.envelope.output()
        }
    }
}

/// The delta-modulation channel: seven-bit PCM played by stepping a level.
///
/// On hardware it reads its samples from CPU memory. A VGM has no CPU, so the
/// bytes arrive as a RAM-write block and [`Dmc::memory`] stands in for the
/// address space.
#[derive(Debug, Default, Clone)]
struct Dmc {
    enabled: bool,
    loops: bool,
    period: u16,
    counter: u16,
    /// The seven-bit output level, which is what makes the DMC audible even
    /// with no sample playing: writing `$4011` is how games play one-shot
    /// clicks and how some play whole tunes.
    level: u8,
    sample_address: u16,
    sample_length: u16,
    current_address: u16,
    bytes_remaining: u16,
    shift: u8,
    bits_remaining: u8,
    buffer: Option<u8>,
    silence: bool,
    /// Sample bytes, as delivered by RAM-write blocks.
    memory: Vec<u8>,
}

impl Dmc {
    fn restart(&mut self) {
        self.current_address = self.sample_address;
        self.bytes_remaining = self.sample_length;
    }

    fn fetch(&mut self) {
        if self.bytes_remaining == 0 || self.buffer.is_some() {
            return;
        }
        // Hardware addresses `$C000..$FFFF`; the RAM block is that window.
        let index = usize::from(self.current_address.wrapping_sub(0xC000));
        self.buffer = Some(self.memory.get(index).copied().unwrap_or(0));
        self.current_address = if self.current_address == 0xFFFF {
            0x8000
        } else {
            self.current_address + 1
        };
        self.bytes_remaining -= 1;
        if self.bytes_remaining == 0 && self.loops {
            self.restart();
        }
    }

    /// One CPU cycle.
    fn clock(&mut self) {
        if self.counter > 0 {
            self.counter -= 1;
            return;
        }
        self.counter = self.period;

        if !self.silence {
            // Each bit nudges the level by two, clamped to seven bits.
            if self.shift & 1 == 1 {
                if self.level <= 125 {
                    self.level += 2;
                }
            } else if self.level >= 2 {
                self.level -= 2;
            }
        }
        self.shift >>= 1;

        if self.bits_remaining > 0 {
            self.bits_remaining -= 1;
        }
        if self.bits_remaining == 0 {
            self.bits_remaining = 8;
            match self.buffer.take() {
                Some(byte) => {
                    self.silence = false;
                    self.shift = byte;
                }
                None => self.silence = true,
            }
        }
        self.fetch();
    }

    fn output(&self) -> i32 {
        i32::from(self.level)
    }
}

/// The frame sequencer: the divider that clocks envelopes and length counters.
#[derive(Debug, Default, Clone, Copy)]
struct FrameCounter {
    cycles: u32,
    step: usize,
    five_step: bool,
}

/// What a frame-sequencer step clocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameTick {
    None,
    /// Envelopes and the triangle's linear counter.
    Quarter,
    /// Those, plus length counters and sweeps.
    Half,
}

impl FrameCounter {
    /// One CPU cycle, returning what this cycle clocks.
    fn clock(&mut self) -> FrameTick {
        self.cycles += 1;
        let steps: &[u32] = if self.five_step {
            &FRAME_STEPS_5
        } else {
            &FRAME_STEPS_4
        };
        let Some(&boundary) = steps.get(self.step) else {
            return FrameTick::None;
        };
        if self.cycles < boundary {
            return FrameTick::None;
        }
        self.step += 1;
        let last = self.step == steps.len();
        if last {
            self.step = 0;
            self.cycles = 0;
        }
        // Five-step mode inserts a step that clocks nothing at all. That is the
        // whole difference between the modes -- the half-frames fall in the
        // same places either way -- and it is why writing `$4017` changes how
        // fast a driver's envelopes and note lengths run.
        if self.five_step && self.step == 4 {
            return FrameTick::None;
        }
        if self.step == 2 || last {
            FrameTick::Half
        } else {
            FrameTick::Quarter
        }
    }
}

/// The NES / Famicom APU.
#[derive(Debug, Default)]
pub struct NesApu {
    rate: u32,
    /// Toggles every CPU cycle; the pulses and noise tick on the false half.
    apu_phase: bool,
    pulses: [Pulse; 2],
    triangle: Triangle,
    noise: Noise,
    dmc: Dmc,
    frame: FrameCounter,
    /// DC-blocker state, in the fixed point [`DC_SHIFT`] describes.
    dc_prev_in: i64,
    dc_prev_out: i64,
}

/// Fixed-point fraction bits for the DC blocker.
const DC_SHIFT: u32 = 16;
/// The blocker's pole, `0.9975` in [`DC_SHIFT`] fixed point.
///
/// The APU's output is unipolar -- silence is zero, everything else is above it
/// -- so feeding it straight out would put a step in the waveform every time a
/// channel starts. Hardware has a coupling capacitor; this is it.
const DC_POLE: i64 = 65_372;

impl NesApu {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 44_100,
            ..Self::default()
        }
    }

    /// One CPU cycle of everything.
    fn clock_cpu(&mut self) {
        self.triangle.clock();
        self.dmc.clock();
        self.apu_phase = !self.apu_phase;
        if self.apu_phase {
            for pulse in &mut self.pulses {
                pulse.clock();
            }
            self.noise.clock();
        }
        match self.frame.clock() {
            FrameTick::None => {}
            FrameTick::Quarter => self.clock_quarter(),
            FrameTick::Half => {
                self.clock_quarter();
                self.clock_half();
            }
        }
    }

    fn clock_quarter(&mut self) {
        for pulse in &mut self.pulses {
            pulse.envelope.clock();
        }
        self.noise.envelope.clock();
        self.triangle.clock_linear();
    }

    fn clock_half(&mut self) {
        for pulse in &mut self.pulses {
            let halt = pulse.envelope.loops;
            pulse.length.clock(halt);
            pulse.clock_sweep();
        }
        self.noise.length.clock(self.noise.envelope.loops);
        self.triangle.length.clock(self.triangle.control);
    }

    /// The mixed output of one cycle, before decimation.
    fn mix(&self) -> i32 {
        let pulse = (self.pulses[0].output() + self.pulses[1].output()) as usize;
        let tnd =
            (3 * self.triangle.output() + 2 * self.noise.output() + self.dmc.output()) as usize;
        PULSE_TABLE[pulse.min(PULSE_TABLE.len() - 1)] + TND_TABLE[tnd.min(TND_TABLE.len() - 1)]
    }

    /// Removes the unipolar signal's DC, in integer arithmetic so the result
    /// cannot differ between a native build and a wasm one.
    fn block_dc(&mut self, sample: i32) -> i32 {
        let input = i64::from(sample);
        // Division, *not* an arithmetic shift. `>>` rounds toward negative
        // infinity, so on the negative half of the waveform `y * 0.9975` gives
        // back `y` unchanged for any small magnitude -- the filter acquires a
        // fixed point and parks there. It settled at -399 of 24000 before this
        // was a division, which is a standing DC offset a listener hears as a
        // click at the start of the next note.
        let output = input - self.dc_prev_in + (self.dc_prev_out * DC_POLE) / (1 << DC_SHIFT);
        self.dc_prev_in = input;
        self.dc_prev_out = output;
        output.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }
}

impl ChipCore for NesApu {
    /// `variant` is the header's bit 31: an FDS add-on is present. Its extra
    /// channel is not modelled, so the flag is noted and ignored -- an FDS rip
    /// plays its five stock channels and misses the sixth.
    fn reset(&mut self, clock: u32, _variant: bool) {
        let memory = std::mem::take(&mut self.dmc.memory);
        *self = Self {
            rate: (clock / CYCLES_PER_SAMPLE).max(1),
            ..Self::default()
        };
        // ROM and RAM arrive before the stream starts and must survive the
        // reset the engine does when it loads.
        self.dmc.memory = memory;
        // **A deliberate departure from hardware.** At power-on the real chip
        // has every channel disabled, and a channel stays mute until `$4015`
        // enables it -- but a VGM is a *register log*, and a ripper is free to
        // start it after the driver has done its initialisation. Rips that
        // never write `$4015` at all are not rare: `Lemmings (NES)` is one, and
        // from hardware's power-on state it plays in complete silence.
        //
        // So the channels start enabled. The cost is that a rip which really
        // does mean "silent until I say so" makes a sound a fraction earlier;
        // the alternative cost is whole rips that never make one.
        self.write(0, 0x15, 0x0F);
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// One flat register file at `$4000`, which is how the VGM `0xB4` command
    /// addresses it.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let value = (data & 0xFF) as u8;
        match addr & 0xFF {
            reg @ 0x00..=0x07 => {
                let pulse = &mut self.pulses[usize::from(reg >= 0x04)];
                pulse.ones_complement = reg < 0x04;
                match reg & 3 {
                    0 => {
                        pulse.duty = value >> 6;
                        pulse.envelope.loops = value & 0x20 != 0;
                        pulse.envelope.constant = value & 0x10 != 0;
                        pulse.envelope.volume = value & 0x0F;
                    }
                    1 => {
                        pulse.sweep_enabled = value & 0x80 != 0;
                        pulse.sweep_period = (value >> 4) & 7;
                        pulse.sweep_negate = value & 0x08 != 0;
                        pulse.sweep_shift = value & 7;
                        pulse.sweep_reload = true;
                    }
                    2 => pulse.timer = (pulse.timer & 0x700) | u16::from(value),
                    _ => {
                        pulse.timer = (pulse.timer & 0x0FF) | (u16::from(value & 7) << 8);
                        pulse.length.load(value >> 3);
                        // A write here restarts the envelope and the sequence,
                        // which is what makes a retrigger audible.
                        pulse.envelope.start = true;
                        pulse.step = 0;
                    }
                }
            }
            0x08 => {
                self.triangle.control = value & 0x80 != 0;
                self.triangle.linear_reload_value = value & 0x7F;
            }
            0x0A => {
                self.triangle.timer = (self.triangle.timer & 0x700) | u16::from(value);
            }
            0x0B => {
                self.triangle.timer = (self.triangle.timer & 0x0FF) | (u16::from(value & 7) << 8);
                self.triangle.length.load(value >> 3);
                self.triangle.linear_reload = true;
            }
            0x0C => {
                self.noise.envelope.loops = value & 0x20 != 0;
                self.noise.envelope.constant = value & 0x10 != 0;
                self.noise.envelope.volume = value & 0x0F;
            }
            0x0E => {
                self.noise.short_mode = value & 0x80 != 0;
                self.noise.period = NOISE_PERIODS[usize::from(value & 0x0F)];
            }
            0x0F => {
                self.noise.length.load(value >> 3);
                self.noise.envelope.start = true;
            }
            0x10 => {
                self.dmc.loops = value & 0x40 != 0;
                self.dmc.period = DMC_PERIODS[usize::from(value & 0x0F)];
            }
            // Writing the level directly is a channel in its own right: games
            // play samples through it with no DMC sequence at all.
            0x11 => self.dmc.level = value & 0x7F,
            0x12 => self.dmc.sample_address = 0xC000 + (u16::from(value) << 6),
            0x13 => self.dmc.sample_length = (u16::from(value) << 4) + 1,
            0x15 => {
                self.pulses[0].length.set_enabled(value & 0x01 != 0);
                self.pulses[1].length.set_enabled(value & 0x02 != 0);
                self.triangle.length.set_enabled(value & 0x04 != 0);
                self.noise.length.set_enabled(value & 0x08 != 0);
                self.dmc.enabled = value & 0x10 != 0;
                if self.dmc.enabled {
                    if self.dmc.bytes_remaining == 0 {
                        self.dmc.restart();
                        self.dmc.fetch();
                    }
                } else {
                    self.dmc.bytes_remaining = 0;
                }
            }
            0x17 => {
                self.frame.five_step = value & 0x80 != 0;
                self.frame.cycles = 0;
                self.frame.step = 0;
                // Setting the mode bit clocks a half-frame immediately, which
                // is how a driver forces its envelopes into step.
                if self.frame.five_step {
                    self.clock_quarter();
                    self.clock_half();
                }
            }
            // 0x09, 0x0D, 0x14, 0x16 and anything above are not APU registers.
            _ => {}
        }
    }

    /// DPCM sample bytes, delivered as a RAM-write block.
    fn write_ram(&mut self, offset: u32, data: &[u8]) {
        let start = offset as usize;
        let end = start + data.len();
        if self.dmc.memory.len() < end {
            self.dmc.memory.resize(end, 0);
        }
        self.dmc.memory[start..end].copy_from_slice(data);
    }

    /// A `0x07` block is DPCM data, which the spec files under ROM rather than
    /// RAM even though the chip reads it as memory.
    fn load_rom(&mut self, _block_type: u8, total_size: u32, start: u32, data: &[u8]) {
        let total = total_size as usize;
        if self.dmc.memory.len() < total {
            self.dmc.memory.resize(total, 0);
        }
        let at = start as usize;
        let end = (at + data.len()).min(self.dmc.memory.len());
        if at < end {
            self.dmc.memory[at..end].copy_from_slice(&data[..end - at]);
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let mut sum = 0i32;
            for _ in 0..CYCLES_PER_SAMPLE {
                self.clock_cpu();
                sum += self.mix();
            }
            let sample = self.block_dc(sum / CYCLES_PER_SAMPLE as i32);
            // Mono: the chip has one output pin.
            frame[0] = sample;
            frame[1] = sample;
        }
    }
}

/// `PULSE_TABLE[n]` is what the summed pulse volumes `n` are worth.
///
/// From the documented `95.52 / (8128 / n + 100)`, scaled by [`PEAK`] and
/// recomputed in `the_mixer_tables_match_the_documented_formulas`.
const PULSE_TABLE: [i32; 31] = [
    0, 279, 551, 816, 1075, 1329, 1576, 1818, 2054, 2285, 2511, 2733, 2949, 3161, 3368, 3572, 3771,
    3965, 4156, 4344, 4527, 4707, 4883, 5056, 5226, 5393, 5556, 5716, 5874, 6028, 6180,
];

/// `TND_TABLE[n]` is what `3*triangle + 2*noise + dmc == n` is worth.
///
/// From the documented `163.67 / (24329 / n + 100)`, same scaling and the same
/// test.
const TND_TABLE: [i32; 203] = [
    0, 161, 320, 478, 635, 791, 945, 1099, 1251, 1401, 1551, 1699, 1846, 1992, 2137, 2281, 2424,
    2565, 2706, 2845, 2984, 3121, 3257, 3393, 3527, 3660, 3793, 3924, 4054, 4184, 4312, 4439, 4566,
    4692, 4816, 4940, 5063, 5185, 5307, 5427, 5546, 5665, 5783, 5900, 6016, 6131, 6246, 6360, 6473,
    6585, 6697, 6807, 6917, 7027, 7135, 7243, 7350, 7456, 7562, 7667, 7771, 7874, 7977, 8080, 8181,
    8282, 8382, 8482, 8581, 8679, 8777, 8874, 8970, 9066, 9161, 9256, 9350, 9443, 9536, 9629, 9720,
    9811, 9902, 9992, 10082, 10170, 10259, 10347, 10434, 10521, 10607, 10693, 10778, 10863, 10947,
    11031, 11114, 11197, 11279, 11361, 11442, 11523, 11604, 11684, 11763, 11842, 11921, 11999,
    12076, 12154, 12230, 12307, 12383, 12458, 12533, 12608, 12682, 12756, 12829, 12902, 12975,
    13047, 13119, 13190, 13262, 13332, 13402, 13472, 13542, 13611, 13680, 13748, 13816, 13884,
    13951, 14018, 14085, 14151, 14217, 14282, 14348, 14413, 14477, 14541, 14605, 14669, 14732,
    14795, 14857, 14920, 14982, 15043, 15105, 15166, 15226, 15287, 15347, 15407, 15466, 15525,
    15584, 15643, 15701, 15759, 15817, 15874, 15932, 15988, 16045, 16101, 16158, 16213, 16269,
    16324, 16379, 16434, 16488, 16543, 16597, 16650, 16704, 16757, 16810, 16863, 16915, 16967,
    17019, 17071, 17123, 17174, 17225, 17276, 17326, 17377, 17427, 17476, 17526, 17576, 17625,
    17674, 17722, 17771, 17819,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The NTSC CPU clock, which is what a VGM header carries for this chip.
    const NTSC: u32 = 1_789_773;

    fn render(chip: &mut NesApu, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|frame| frame[0]).collect()
    }

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    /// Whether a render has *stopped*, as opposed to never having started.
    ///
    /// Cutting a channel does not put zeroes on the wire: the DC blocker is a
    /// filter with a tail, and it rings down over a few milliseconds exactly as
    /// the coupling capacitor it stands in for does. So a silenced channel is
    /// judged by where the render ends up, not by the whole window being zero
    /// -- which would be asserting the blocker away.
    fn falls_silent(samples: &[i32]) -> bool {
        let tail = &samples[samples.len() * 3 / 4..];
        tail.iter().all(|&s| s.abs() < PEAK / 200)
    }

    /// Every number in the mixer tables, recomputed from the documented
    /// formulas rather than trusted.
    ///
    /// The tables exist because [`ChipCore`] forbids output that could differ
    /// across targets, and floating point is how that promise gets broken; this
    /// is what keeps them honest to the arithmetic they stand in for.
    #[test]
    fn the_mixer_tables_match_the_documented_formulas() {
        assert_eq!(PULSE_TABLE[0], 0, "silence is silence");
        for (n, &value) in PULSE_TABLE.iter().enumerate().skip(1) {
            let expected = f64::from(PEAK) * 95.52 / (8128.0 / n as f64 + 100.0);
            assert_eq!(value, expected.round() as i32, "pulse {n}");
        }
        assert_eq!(TND_TABLE[0], 0);
        for (n, &value) in TND_TABLE.iter().enumerate().skip(1) {
            let expected = f64::from(PEAK) * 163.67 / (24329.0 / n as f64 + 100.0);
            assert_eq!(value, expected.round() as i32, "tnd {n}");
        }
        // The formulas are normalised so everything at once approaches full
        // scale. If that stopped being true the chip would clip or whisper.
        let loudest = PULSE_TABLE[30] + TND_TABLE[202];
        assert!(
            (PEAK * 99 / 100..=PEAK * 101 / 100).contains(&loudest),
            "everything at once came to {loudest}, not about {PEAK}"
        );
    }

    /// The length table has no formula behind it, so what can be checked is its
    /// documented shape: the odd entries after the first count 2, 4, 6 ... 30,
    /// which is the half a driver uses for ordinary note lengths. Entry 1 is
    /// 254, the outlier that makes a note effectively hold.
    #[test]
    fn the_length_table_is_the_documented_one() {
        let odd: Vec<u8> = LENGTH_TABLE.iter().skip(3).step_by(2).copied().collect();
        assert_eq!(
            odd,
            (1..=15).map(|n| n * 2).collect::<Vec<u8>>(),
            "the odd entries after the first count 2, 4, 6 ... 30"
        );
        assert_eq!(LENGTH_TABLE[1], 254, "the hold-forever entry");
        assert_eq!(LENGTH_TABLE[0], 10);
        assert_eq!(LENGTH_TABLE[2], 20);
    }

    /// A pulse's period sets its pitch as `CPU / (16 * (t + 1))`. Counted in
    /// edges rather than asserted from the formula, the same way the SN76489
    /// core counts its own.
    #[test]
    fn a_pulse_sounds_at_the_documented_frequency() {
        let mut chip = NesApu::new();
        chip.reset(NTSC, false);
        chip.write(0, 0x15, 0x01); // enable pulse 1
        chip.write(0, 0x00, 0xBF); // 50% duty, constant volume 15, halt length
        chip.write(0, 0x02, 0xFD); // period low
        chip.write(0, 0x03, 0x00); // period high 0 -> timer 0xFD
        let timer = chip.pulses[0].timer;
        assert_eq!(timer, 0xFD);

        // Count sequencer wraps over a known number of CPU cycles.
        let cycles = NTSC; // one second
        let mut wraps = 0u32;
        let mut last = chip.pulses[0].step;
        for _ in 0..cycles {
            chip.clock_cpu();
            let step = chip.pulses[0].step;
            if step == 0 && last == 7 {
                wraps += 1;
            }
            last = step;
        }
        let expected = NTSC / (16 * (u32::from(timer) + 1));
        let drift = wraps.abs_diff(expected);
        assert!(
            drift <= 1,
            "counted {wraps} cycles a second, expected about {expected}"
        );
    }

    /// The triangle ticks on every CPU cycle where the pulses tick on every
    /// other, so the same period is an octave apart. That relationship is the
    /// one worth pinning: it is what a driver relies on when it writes the same
    /// value to both.
    #[test]
    fn the_triangle_runs_twice_as_fast_as_a_pulse_for_the_same_period() {
        let mut chip = NesApu::new();
        chip.reset(NTSC, false);
        chip.write(0, 0x15, 0x05); // pulse 1 + triangle
        chip.write(0, 0x00, 0xBF);
        chip.write(0, 0x02, 0x40);
        chip.write(0, 0x03, 0x08);
        chip.write(0, 0x08, 0xFF); // triangle: control set, linear counter max
        chip.write(0, 0x0A, 0x40);
        chip.write(0, 0x0B, 0x08);

        let cycles = NTSC / 4;
        let (mut pulse_wraps, mut tri_wraps) = (0u32, 0u32);
        let (mut last_p, mut last_t) = (chip.pulses[0].step, chip.triangle.step);
        for _ in 0..cycles {
            chip.clock_cpu();
            if chip.pulses[0].step == 0 && last_p == 7 {
                pulse_wraps += 1;
            }
            if chip.triangle.step == 0 && last_t == 31 {
                tri_wraps += 1;
            }
            last_p = chip.pulses[0].step;
            last_t = chip.triangle.step;
        }
        assert!(pulse_wraps > 0 && tri_wraps > 0, "both must run");
        // Pulse: CPU / (16 * (t+1)). Triangle: CPU / (32 * (t+1)). The
        // triangle's sequence is 32 steps to the pulse's 8, so its *wrap* rate
        // is half the pulse's -- while its step rate is four times.
        let ratio = f64::from(pulse_wraps) / f64::from(tri_wraps);
        assert!(
            (1.8..=2.2).contains(&ratio),
            "pulse wrapped {pulse_wraps} times to the triangle's {tri_wraps}"
        );
    }

    /// A channel disabled through `$4015` is silent, and enabling it is not
    /// enough on its own -- the length counter has to be loaded too. Getting
    /// this backwards makes every note play for ever.
    #[test]
    fn the_status_register_gates_each_channel() {
        let mut chip = NesApu::new();
        chip.reset(NTSC, false);
        // Explicitly disabled *before* anything sounds, so the length counter
        // cannot load and nothing has rung to decay from. (Reset leaves the
        // channels enabled -- see `reset` for why -- so this has to be asked
        // for rather than assumed.)
        chip.write(0, 0x15, 0x00);
        chip.write(0, 0x00, 0xBF);
        chip.write(0, 0x02, 0x40);
        chip.write(0, 0x03, 0x08);
        assert_eq!(energy(&render(&mut chip, 2000)), 0);

        chip.write(0, 0x15, 0x01);
        chip.write(0, 0x03, 0x08); // reload the length now it is enabled
        assert!(energy(&render(&mut chip, 2000)) > 0);

        // And disabling clears the counter outright rather than pausing it.
        chip.write(0, 0x15, 0x00);
        assert!(falls_silent(&render(&mut chip, 4000)));
    }

    /// A period below 8, or a sweep target past eleven bits, mutes a pulse
    /// while its sequencer keeps running. Both are documented and both are easy
    /// to leave out, which shows up as a squeal on notes that should be silent.
    #[test]
    fn a_pulse_mutes_on_a_short_period_or_an_overflowing_sweep() {
        let mut chip = NesApu::new();
        chip.reset(NTSC, false);
        chip.write(0, 0x15, 0x01);
        chip.write(0, 0x00, 0xBF);
        chip.write(0, 0x02, 0x07); // period 7, below the floor
        chip.write(0, 0x03, 0x08);
        assert!(
            falls_silent(&render(&mut chip, 4000)),
            "period 7 must be muted"
        );

        chip.write(0, 0x02, 0x08); // period 8, the first audible one
        chip.write(0, 0x03, 0x08);
        assert!(energy(&render(&mut chip, 2000)) > 0);

        // An upward sweep whose target overflows mutes immediately.
        chip.write(0, 0x02, 0x00);
        chip.write(0, 0x03, 0x0F); // timer 0x700
        chip.write(0, 0x01, 0x81); // sweep enabled, shift 1, adding
        assert!(
            falls_silent(&render(&mut chip, 4000)),
            "a sweep target past 0x7FF must mute"
        );
    }

    /// The noise channel's two modes come from where the shift register is
    /// tapped, and the short one repeats far sooner. Counting the period is how
    /// to tell them apart without an ear.
    #[test]
    fn the_noise_shift_register_has_two_periods() {
        fn period(short: bool) -> usize {
            let mut noise = Noise {
                short_mode: short,
                period: 0,
                counter: 0,
                ..Noise::default()
            };
            let start = noise.shift;
            for step in 1..100_000 {
                noise.clock();
                if noise.shift == start {
                    return step;
                }
            }
            0
        }
        // The documented lengths: 32767 for the long sequence, 93 for the short.
        assert_eq!(period(false), 32_767);
        assert_eq!(period(true), 93);
    }

    /// Writing `$4011` moves the DMC's output level directly, with no sample
    /// involved. Games play whole tunes this way, so a core that only responds
    /// to DPCM sequences renders them silent.
    #[test]
    fn the_dmc_level_is_audible_without_a_sample() {
        let mut chip = NesApu::new();
        chip.reset(NTSC, false);
        let quiet = energy(&render(&mut chip, 1000));

        // Step the level about, which is what such a driver does.
        let mut moved = 0i64;
        for step in 0..40 {
            chip.write(0, 0x11, if step % 2 == 0 { 0x00 } else { 0x7F });
            moved += energy(&render(&mut chip, 50));
        }
        assert!(
            moved > quiet * 10,
            "writing the DMC level made no sound: {moved} vs {quiet}"
        );
    }

    /// DPCM bytes arrive as memory rather than as register writes, and the
    /// channel addresses them from `$C000`. An off-by-one in that window plays
    /// the wrong sample, or noise.
    #[test]
    fn the_dmc_plays_bytes_from_the_ram_block() {
        let mut chip = NesApu::new();
        chip.reset(NTSC, false);
        // Alternating bits: the steepest waveform the DMC can produce.
        chip.write_ram(0, &[0b1010_1010; 256]);

        chip.write(0, 0x10, 0x0F); // fastest rate, no loop
        chip.write(0, 0x12, 0x00); // sample at $C000
        chip.write(0, 0x13, 0x0F); // a few hundred bytes
        chip.write(0, 0x11, 0x40); // start mid-range so it can move both ways
        chip.write(0, 0x15, 0x10); // enable the DMC

        let out = render(&mut chip, 4000);
        assert!(
            energy(&out) > 0,
            "the DMC played nothing from a full RAM block"
        );
    }

    /// The engine resets a core when it loads a file, and the sample memory
    /// arrives *before* the stream starts. A reset that dropped it would play
    /// every DPCM sample as silence -- and only on real files, never in a test
    /// that writes its RAM afterwards.
    #[test]
    fn a_reset_keeps_the_sample_memory_it_was_given() {
        let mut chip = NesApu::new();
        chip.write_ram(0, &[0xFF; 64]);
        chip.reset(NTSC, false);
        assert_eq!(
            chip.dmc.memory.len(),
            64,
            "the reset threw the samples away"
        );
        assert!(chip.dmc.memory.iter().all(|&byte| byte == 0xFF));
    }

    /// The rate the engine resamples from.
    #[test]
    fn the_native_rate_divides_the_cpu_clock() {
        let mut chip = NesApu::new();
        chip.reset(NTSC, false);
        assert_eq!(chip.native_rate(), NTSC / CYCLES_PER_SAMPLE);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// Chunking must not change the audio.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        fn set_up(chip: &mut NesApu) {
            chip.reset(NTSC, false);
            chip.write(0, 0x15, 0x0F);
            chip.write(0, 0x00, 0xBF);
            chip.write(0, 0x02, 0x40);
            chip.write(0, 0x03, 0x08);
            chip.write(0, 0x0C, 0x3F);
            chip.write(0, 0x0E, 0x04);
            chip.write(0, 0x0F, 0x08);
        }
        let mut whole = NesApu::new();
        set_up(&mut whole);
        let mut one_go = vec![0i32; 1024 * 2];
        whole.render(&mut one_go);

        let mut chunked = NesApu::new();
        set_up(&mut chunked);
        let mut piecemeal = vec![0i32; 1024 * 2];
        for chunk in piecemeal.chunks_mut(64 * 2) {
            chunked.render(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// The output is unipolar before the DC blocker, so without one every note
    /// would start with a step. Silence must settle back to zero.
    #[test]
    fn the_dc_blocker_returns_silence_to_zero() {
        let mut chip = NesApu::new();
        chip.reset(NTSC, false);
        chip.write(0, 0x15, 0x01);
        chip.write(0, 0x00, 0xBF);
        chip.write(0, 0x02, 0x40);
        chip.write(0, 0x03, 0x08);
        let _ = render(&mut chip, 2000);

        chip.write(0, 0x15, 0x00); // everything off
        let tail = render(&mut chip, 20_000);
        let settled = &tail[tail.len() - 100..];
        let loudest = settled.iter().map(|&s| s.abs()).max().unwrap_or(0);
        assert!(
            loudest < PEAK / 500,
            "silence settled only to {loudest}, which is a DC offset a listener              would hear as a click when the next note starts"
        );
    }

    /// A loud patch should use the range without leaning on the mixer's clamp.
    #[test]
    fn a_full_chip_uses_the_range_without_clipping_it() {
        let mut chip = NesApu::new();
        chip.reset(NTSC, false);
        chip.write(0, 0x15, 0x0F);
        for base in [0x00u16, 0x04] {
            chip.write(0, base, 0xBF);
            chip.write(0, base + 2, 0x40);
            chip.write(0, base + 3, 0x08);
        }
        chip.write(0, 0x08, 0xFF);
        chip.write(0, 0x0A, 0x20);
        chip.write(0, 0x0B, 0x08);
        chip.write(0, 0x0C, 0x3F);
        chip.write(0, 0x0E, 0x02);
        chip.write(0, 0x0F, 0x08);

        let out = render(&mut chip, 8000);
        let loudest = out.iter().map(|&s| s.abs()).max().unwrap_or(0);
        assert!(
            loudest > PEAK / 8,
            "a full chip peaked at {loudest}, far below its own scale"
        );
        assert!(
            loudest < i32::from(i16::MAX),
            "a full chip peaked at {loudest}, which the mixer would clamp"
        );
    }
}
