//! Game Boy (DMG) APU register documentation.
//!
//! Sources: the gbdev Pan Docs "Audio Registers" chapter
//! (<https://gbdev.io/pandocs/Audio_Registers.html>).
//!
//! Addressing: one bank, so only port 0. The VGM command carries the offset
//! from 0xFF10, so NR10 is 0x00, NR52 is 0x16, and wave RAM (0xFF30-0xFF3F)
//! is 0x20-0x2F. Registers the two pulse channels share (and the noise
//! channel's envelope) use one role-named doc; the address column tells the
//! channels apart.

use super::{RegisterDoc, bf};

const SWEEP: RegisterDoc = RegisterDoc {
    name: "Pulse 1: sweep (NR10)",
    fields: &[
        bf("Sweep pace", 0x70),
        bf("Sweep direction", 0x08),
        bf("Sweep step", 0x07),
    ],
};
const PULSE_DUTY_LENGTH: RegisterDoc = RegisterDoc {
    name: "Pulse: duty / length",
    fields: &[bf("Wave duty", 0xC0), bf("Initial length timer", 0x3F)],
};
const ENVELOPE: RegisterDoc = RegisterDoc {
    name: "Volume & envelope",
    fields: &[
        bf("Initial volume", 0xF0),
        bf("Envelope direction", 0x08),
        bf("Envelope pace", 0x07),
    ],
};
const PERIOD_LOW: RegisterDoc = RegisterDoc {
    name: "Period (low 8 bits)",
    fields: &[bf("Period (low 8 bits)", 0xFF)],
};
const TRIGGER_PERIOD_HIGH: RegisterDoc = RegisterDoc {
    name: "Trigger / length enable / period (high 3 bits)",
    fields: &[
        bf("Trigger", 0x80),
        bf("Length enable", 0x40),
        bf("Period (high 3 bits)", 0x07),
    ],
};
const WAVE_DAC: RegisterDoc = RegisterDoc {
    name: "Wave: DAC enable (NR30)",
    fields: &[bf("DAC enable", 0x80)],
};
const WAVE_LENGTH: RegisterDoc = RegisterDoc {
    name: "Wave: length (NR31)",
    fields: &[bf("Initial length timer", 0xFF)],
};
const WAVE_LEVEL: RegisterDoc = RegisterDoc {
    name: "Wave: output level (NR32)",
    fields: &[bf("Output level", 0x60)],
};
const NOISE_LENGTH: RegisterDoc = RegisterDoc {
    name: "Noise: length (NR41)",
    fields: &[bf("Initial length timer", 0x3F)],
};
const NOISE_FREQUENCY: RegisterDoc = RegisterDoc {
    name: "Noise: frequency / LFSR width (NR43)",
    fields: &[
        bf("Clock shift", 0xF0),
        bf("LFSR width (7-bit)", 0x08),
        bf("Clock divider", 0x07),
    ],
};
const NOISE_TRIGGER: RegisterDoc = RegisterDoc {
    name: "Noise: trigger / length enable (NR44)",
    fields: &[bf("Trigger", 0x80), bf("Length enable", 0x40)],
};
const MASTER_VOLUME: RegisterDoc = RegisterDoc {
    name: "Master volume / VIN panning (NR50)",
    fields: &[
        bf("VIN left enable", 0x80),
        bf("Left volume", 0x70),
        bf("VIN right enable", 0x08),
        bf("Right volume", 0x07),
    ],
};
const PANNING: RegisterDoc = RegisterDoc {
    name: "Sound panning (NR51)",
    fields: &[
        bf("Ch 4 left", 0x80),
        bf("Ch 3 left", 0x40),
        bf("Ch 2 left", 0x20),
        bf("Ch 1 left", 0x10),
        bf("Ch 4 right", 0x08),
        bf("Ch 3 right", 0x04),
        bf("Ch 2 right", 0x02),
        bf("Ch 1 right", 0x01),
    ],
};
const MASTER_CONTROL: RegisterDoc = RegisterDoc {
    name: "Audio on/off / channel status (NR52)",
    fields: &[
        bf("Audio on/off", 0x80),
        bf("Channel on status (read-only)", 0x0F),
    ],
};
const WAVE_RAM: RegisterDoc = RegisterDoc {
    name: "Wave RAM sample pair",
    fields: &[
        bf("First sample (high nibble)", 0xF0),
        bf("Second sample (low nibble)", 0x0F),
    ],
};

/// The documentation for a write to `(port, addr)`.
pub(super) const fn doc(port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    if port != 0 {
        return None;
    }
    Some(match addr {
        0x00 => &SWEEP,
        0x01 | 0x06 => &PULSE_DUTY_LENGTH,
        0x02 | 0x07 | 0x11 => &ENVELOPE,
        0x03 | 0x08 | 0x0D => &PERIOD_LOW,
        0x04 | 0x09 | 0x0E => &TRIGGER_PERIOD_HIGH,
        0x0A => &WAVE_DAC,
        0x0B => &WAVE_LENGTH,
        0x0C => &WAVE_LEVEL,
        0x10 => &NOISE_LENGTH,
        0x12 => &NOISE_FREQUENCY,
        0x13 => &NOISE_TRIGGER,
        0x14 => &MASTER_VOLUME,
        0x15 => &PANNING,
        0x16 => &MASTER_CONTROL,
        0x20..=0x2F => &WAVE_RAM,
        _ => return None,
    })
}

/// The registers a find dropdown offers.
pub(super) const NOTABLE: &[(u8, u16, &str)] = &[
    (0, 0x16, "Audio on/off (NR52)"),
    (0, 0x15, "Sound panning (NR51)"),
    (0, 0x04, "Pulse 1 trigger (NR14)"),
    (0, 0x09, "Pulse 2 trigger (NR24)"),
    (0, 0x0E, "Wave trigger (NR34)"),
    (0, 0x13, "Noise trigger (NR44)"),
];
