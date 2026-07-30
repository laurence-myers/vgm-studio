//! NES APU (2A03) register documentation, with the FDS expansion the VGM
//! chip type carries.
//!
//! Sources: the NESdev wiki "APU registers" page
//! (<https://www.nesdev.org/wiki/APU_registers>) and "FDS audio"
//! (<https://www.nesdev.org/wiki/FDS_audio>).
//!
//! Addressing: one bank, so only port 0. For 0x00-0x1F the VGM command
//! carries the low byte of the CPU address ($4000 + addr). The FDS
//! expansion is folded in above that: 0x20-0x3E maps to $4080-$409E
//! ($4080 + addr - 0x20), 0x3F to $4023, and 0x40-0x7F to the $4040-$407F
//! wave table. Registers the two pulse channels share use one role-named
//! doc; the address column tells the channels apart.

use super::{RegisterDoc, bf};

const PULSE_DUTY_ENVELOPE: RegisterDoc = RegisterDoc {
    name: "Pulse: duty / envelope",
    fields: &[
        bf("Duty", 0xC0),
        bf("Length counter halt / envelope loop", 0x20),
        bf("Constant volume", 0x10),
        bf("Volume / envelope period", 0x0F),
    ],
};
const PULSE_SWEEP: RegisterDoc = RegisterDoc {
    name: "Pulse: sweep",
    fields: &[
        bf("Sweep enable", 0x80),
        bf("Sweep period", 0x70),
        bf("Negate", 0x08),
        bf("Shift count", 0x07),
    ],
};
const PULSE_TIMER_LOW: RegisterDoc = RegisterDoc {
    name: "Pulse: timer (low 8 bits)",
    fields: &[bf("Timer (low 8 bits)", 0xFF)],
};
const PULSE_LENGTH_TIMER_HIGH: RegisterDoc = RegisterDoc {
    name: "Pulse: length load / timer (high 3 bits)",
    fields: &[
        bf("Length counter load", 0xF8),
        bf("Timer (high 3 bits)", 0x07),
    ],
};
const TRIANGLE_LINEAR: RegisterDoc = RegisterDoc {
    name: "Triangle: linear counter",
    fields: &[
        bf("Control / length counter halt", 0x80),
        bf("Linear counter load", 0x7F),
    ],
};
const TRIANGLE_TIMER_LOW: RegisterDoc = RegisterDoc {
    name: "Triangle: timer (low 8 bits)",
    fields: &[bf("Timer (low 8 bits)", 0xFF)],
};
const TRIANGLE_LENGTH_TIMER_HIGH: RegisterDoc = RegisterDoc {
    name: "Triangle: length load / timer (high 3 bits)",
    fields: &[
        bf("Length counter load", 0xF8),
        bf("Timer (high 3 bits)", 0x07),
    ],
};
const NOISE_ENVELOPE: RegisterDoc = RegisterDoc {
    name: "Noise: envelope",
    fields: &[
        bf("Length counter halt / envelope loop", 0x20),
        bf("Constant volume", 0x10),
        bf("Volume / envelope period", 0x0F),
    ],
};
const NOISE_MODE_PERIOD: RegisterDoc = RegisterDoc {
    name: "Noise: mode / period",
    fields: &[bf("Mode (short loop)", 0x80), bf("Noise period", 0x0F)],
};
const NOISE_LENGTH: RegisterDoc = RegisterDoc {
    name: "Noise: length load",
    fields: &[bf("Length counter load", 0xF8)],
};
const DMC_CONTROL: RegisterDoc = RegisterDoc {
    name: "DMC: IRQ / loop / frequency",
    fields: &[
        bf("IRQ enable", 0x80),
        bf("Loop", 0x40),
        bf("Rate index", 0x0F),
    ],
};
const DMC_DIRECT_LOAD: RegisterDoc = RegisterDoc {
    name: "DMC: direct load",
    fields: &[bf("DAC level", 0x7F)],
};
const DMC_SAMPLE_ADDRESS: RegisterDoc = RegisterDoc {
    name: "DMC: sample address",
    fields: &[bf("Sample address (64-byte units from $C000)", 0xFF)],
};
const DMC_SAMPLE_LENGTH: RegisterDoc = RegisterDoc {
    name: "DMC: sample length",
    fields: &[bf("Sample length (16-byte units)", 0xFF)],
};
const STATUS: RegisterDoc = RegisterDoc {
    name: "Status / channel enable",
    fields: &[
        bf("DMC enable", 0x10),
        bf("Noise enable", 0x08),
        bf("Triangle enable", 0x04),
        bf("Pulse 2 enable", 0x02),
        bf("Pulse 1 enable", 0x01),
    ],
};
const FRAME_COUNTER: RegisterDoc = RegisterDoc {
    name: "Frame counter",
    fields: &[bf("5-step mode", 0x80), bf("IRQ inhibit", 0x40)],
};

// The FDS expansion's registers ($4080-$408A, $4023, and the wave table).
const FDS_VOLUME_ENVELOPE: RegisterDoc = RegisterDoc {
    name: "FDS: volume envelope",
    fields: &[
        bf("Envelope disable (direct volume)", 0x80),
        bf("Envelope direction (increase)", 0x40),
        bf("Speed / gain", 0x3F),
    ],
};
const FDS_FREQ_LOW: RegisterDoc = RegisterDoc {
    name: "FDS: frequency (low 8 bits)",
    fields: &[bf("Frequency (low 8 bits)", 0xFF)],
};
const FDS_FREQ_HIGH: RegisterDoc = RegisterDoc {
    name: "FDS: halt / frequency (high 4 bits)",
    fields: &[
        bf("Halt waveform", 0x80),
        bf("Disable envelopes", 0x40),
        bf("Frequency (high 4 bits)", 0x0F),
    ],
};
const FDS_MOD_ENVELOPE: RegisterDoc = RegisterDoc {
    name: "FDS: modulator envelope",
    fields: &[
        bf("Envelope disable (direct gain)", 0x80),
        bf("Envelope direction (increase)", 0x40),
        bf("Speed / gain", 0x3F),
    ],
};
const FDS_MOD_FREQ_LOW: RegisterDoc = RegisterDoc {
    name: "FDS: mod frequency (low 8 bits)",
    fields: &[bf("Mod frequency (low 8 bits)", 0xFF)],
};
const FDS_MOD_FREQ_HIGH: RegisterDoc = RegisterDoc {
    name: "FDS: halt modulator / mod frequency (high 4 bits)",
    fields: &[
        bf("Halt modulator", 0x80),
        bf("Mod frequency (high 4 bits)", 0x0F),
    ],
};
const FDS_MOD_TABLE: RegisterDoc = RegisterDoc {
    name: "FDS: modulation table write",
    fields: &[bf("Modulation table entry", 0x07)],
};
const FDS_WAVE_WRITE: RegisterDoc = RegisterDoc {
    name: "FDS: wave write / master volume",
    fields: &[bf("Wave write enable", 0x80), bf("Master volume", 0x03)],
};
const FDS_ENVELOPE_SPEED: RegisterDoc = RegisterDoc {
    name: "FDS: envelope speed",
    fields: &[bf("Envelope speed", 0xFF)],
};
const FDS_IO_ENABLE: RegisterDoc = RegisterDoc {
    name: "FDS: I/O enable",
    fields: &[bf("Sound I/O enable", 0x02), bf("Disk I/O enable", 0x01)],
};
const FDS_WAVE_TABLE: RegisterDoc = RegisterDoc {
    name: "FDS: wave table sample",
    fields: &[bf("Sample (6 bits)", 0x3F)],
};

/// The documentation for a write to `(port, addr)`.
pub(super) const fn doc(port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    if port != 0 {
        return None;
    }
    Some(match addr {
        0x00 | 0x04 => &PULSE_DUTY_ENVELOPE,
        0x01 | 0x05 => &PULSE_SWEEP,
        0x02 | 0x06 => &PULSE_TIMER_LOW,
        0x03 | 0x07 => &PULSE_LENGTH_TIMER_HIGH,
        0x08 => &TRIANGLE_LINEAR,
        0x0A => &TRIANGLE_TIMER_LOW,
        0x0B => &TRIANGLE_LENGTH_TIMER_HIGH,
        0x0C => &NOISE_ENVELOPE,
        0x0E => &NOISE_MODE_PERIOD,
        0x0F => &NOISE_LENGTH,
        0x10 => &DMC_CONTROL,
        0x11 => &DMC_DIRECT_LOAD,
        0x12 => &DMC_SAMPLE_ADDRESS,
        0x13 => &DMC_SAMPLE_LENGTH,
        0x15 => &STATUS,
        0x17 => &FRAME_COUNTER,
        // The FDS expansion: 0x20-0x3E is $4080 + (addr - 0x20).
        0x20 => &FDS_VOLUME_ENVELOPE,
        0x22 => &FDS_FREQ_LOW,
        0x23 => &FDS_FREQ_HIGH,
        0x24 => &FDS_MOD_ENVELOPE,
        0x26 => &FDS_MOD_FREQ_LOW,
        0x27 => &FDS_MOD_FREQ_HIGH,
        0x28 => &FDS_MOD_TABLE,
        0x29 => &FDS_WAVE_WRITE,
        0x2A => &FDS_ENVELOPE_SPEED,
        0x3F => &FDS_IO_ENABLE,
        0x40..=0x7F => &FDS_WAVE_TABLE,
        _ => return None,
    })
}

/// The registers a find dropdown offers.
pub(super) const NOTABLE: &[(u8, u16, &str)] = &[
    (0, 0x15, "Status / channel enable"),
    (0, 0x17, "Frame counter"),
    (0, 0x10, "DMC control"),
];
