//! OPN family (YM2203, YM2608, YM2610) register documentation.
//!
//! Sources: the Yamaha YM2608 (OPNA) application manual (long-public), the
//! NeoGeo Development Wiki / VGMRips wiki YM2610 register map, and the GI
//! AY-3-8910 datasheet for the SSG section, whose layout all three chips
//! inherit. Bit assignments are datasheet facts.
//!
//! Addressing: the YM2203 has a single port; the YM2608 and YM2610 add port
//! 1, carrying FM channels 4-6 beside the YM2608's ADPCM-B and the YM2610's
//! ADPCM-A. The FM section is the YM2612's layout exactly -- same operator
//! ranges, same `addr & 3 == 3` holes -- and its docs name the role, not the
//! channel. The differences are gated per chip in [`doc`]: LFO and stereo on
//! the OPNA generation only, prescaler on the 2203/2608 only, rhythm on the
//! 2608, ADPCM per chip.

use super::{RegisterDoc, bf};
use crate::vgm::ChipKind;

// The FM section all three chips share (the YM2612's layout).
const TEST: RegisterDoc = RegisterDoc {
    name: "Test",
    fields: &[bf("Test", 0xFF)],
};
const LFO: RegisterDoc = RegisterDoc {
    name: "LFO enable / frequency",
    fields: &[bf("LFO enable", 0x08), bf("LFO frequency", 0x07)],
};
const TIMER_A_HIGH: RegisterDoc = RegisterDoc {
    name: "Timer A period (high 8 bits)",
    fields: &[bf("Timer A period (high)", 0xFF)],
};
const TIMER_A_LOW: RegisterDoc = RegisterDoc {
    name: "Timer A period (low 2 bits)",
    fields: &[bf("Timer A period (low)", 0x03)],
};
const TIMER_B: RegisterDoc = RegisterDoc {
    name: "Timer B period",
    fields: &[bf("Timer B period", 0xFF)],
};
const MODE_TIMER: RegisterDoc = RegisterDoc {
    name: "Ch 3 mode / timer control",
    fields: &[
        bf("Ch 3 mode (normal / special)", 0xC0),
        bf("Timer B reset", 0x20),
        bf("Timer A reset", 0x10),
        bf("Timer B enable", 0x08),
        bf("Timer A enable", 0x04),
        bf("Timer B load", 0x02),
        bf("Timer A load", 0x01),
    ],
};
const KEY_ON_OFF: RegisterDoc = RegisterDoc {
    name: "Key on/off (operator mask + channel)",
    fields: &[bf("Operator on/off mask", 0xF0), bf("Channel", 0x07)],
};
// Writing the address alone switches the clock ratio; the data byte is
// ignored by the hardware.
const PRESCALER_6: RegisterDoc = RegisterDoc {
    name: "Prescaler: FM 1/6, SSG 1/4",
    fields: &[bf("Prescaler select (value ignored)", 0xFF)],
};
const PRESCALER_3: RegisterDoc = RegisterDoc {
    name: "Prescaler: FM 1/3, SSG 1/2 (after 0x2D)",
    fields: &[bf("Prescaler select (value ignored)", 0xFF)],
};
const PRESCALER_2: RegisterDoc = RegisterDoc {
    name: "Prescaler: FM 1/2, SSG 1/1",
    fields: &[bf("Prescaler select (value ignored)", 0xFF)],
};
const DT_MULTI: RegisterDoc = RegisterDoc {
    name: "Operator: detune / multiple",
    fields: &[bf("Detune", 0x70), bf("Frequency multiple", 0x0F)],
};
const TOTAL_LEVEL: RegisterDoc = RegisterDoc {
    name: "Operator: total level",
    fields: &[bf("Total level (attenuation)", 0x7F)],
};
const KS_AR: RegisterDoc = RegisterDoc {
    name: "Operator: key scale / attack rate",
    fields: &[bf("Key scale", 0xC0), bf("Attack rate", 0x1F)],
};
const AM_DR: RegisterDoc = RegisterDoc {
    name: "Operator: AM enable / decay rate",
    fields: &[bf("AM enable", 0x80), bf("Decay rate", 0x1F)],
};
const SR: RegisterDoc = RegisterDoc {
    name: "Operator: sustain rate",
    fields: &[bf("Sustain rate", 0x1F)],
};
const SL_RR: RegisterDoc = RegisterDoc {
    name: "Operator: sustain level / release rate",
    fields: &[bf("Sustain level", 0xF0), bf("Release rate", 0x0F)],
};
const SSG_EG: RegisterDoc = RegisterDoc {
    name: "Operator: SSG-EG envelope",
    fields: &[bf("SSG-EG mode", 0x0F)],
};
const FREQ_LOW: RegisterDoc = RegisterDoc {
    name: "Channel: frequency (low 8 bits)",
    fields: &[bf("Frequency (low 8 bits)", 0xFF)],
};
const FREQ_HIGH: RegisterDoc = RegisterDoc {
    name: "Channel: block / frequency (high 3 bits)",
    fields: &[
        bf("Block (octave)", 0x38),
        bf("Frequency (high 3 bits)", 0x07),
    ],
};
const CH3_FREQ_LOW: RegisterDoc = RegisterDoc {
    name: "Ch 3 special mode: operator frequency (low 8 bits)",
    fields: &[bf("Frequency (low 8 bits)", 0xFF)],
};
const CH3_FREQ_HIGH: RegisterDoc = RegisterDoc {
    name: "Ch 3 special mode: operator block / frequency",
    fields: &[
        bf("Block (octave)", 0x38),
        bf("Frequency (high 3 bits)", 0x07),
    ],
};
const FB_ALGO: RegisterDoc = RegisterDoc {
    name: "Channel: feedback / algorithm",
    fields: &[bf("Feedback", 0x38), bf("Algorithm", 0x07)],
};
const LR_AMS_PMS: RegisterDoc = RegisterDoc {
    name: "Channel: stereo / LFO sensitivity",
    fields: &[
        bf("Left output", 0x80),
        bf("Right output", 0x40),
        bf("AM sensitivity", 0x30),
        bf("PM sensitivity", 0x07),
    ],
};

// The SSG section: AY-3-8910-compatible registers 0x00-0x0F on port 0, on
// all three chips. Role-shared docs: R0/R2/R4 are the same register for
// three different voices.
const SSG_TONE_FINE: RegisterDoc = RegisterDoc {
    name: "SSG: tone period (fine)",
    fields: &[bf("Tone period (fine 8 bits)", 0xFF)],
};
const SSG_TONE_COARSE: RegisterDoc = RegisterDoc {
    name: "SSG: tone period (coarse)",
    fields: &[bf("Tone period (coarse 4 bits)", 0x0F)],
};
const SSG_NOISE: RegisterDoc = RegisterDoc {
    name: "SSG: noise period",
    fields: &[bf("Noise period", 0x1F)],
};
const SSG_MIXER: RegisterDoc = RegisterDoc {
    name: "SSG: mixer (enables, active low)",
    fields: &[
        bf("I/O port B direction", 0x80),
        bf("I/O port A direction", 0x40),
        bf("Noise C off", 0x20),
        bf("Noise B off", 0x10),
        bf("Noise A off", 0x08),
        bf("Tone C off", 0x04),
        bf("Tone B off", 0x02),
        bf("Tone A off", 0x01),
    ],
};
const SSG_AMPLITUDE: RegisterDoc = RegisterDoc {
    name: "SSG: amplitude",
    fields: &[bf("Envelope mode", 0x10), bf("Amplitude", 0x0F)],
};
const SSG_ENVELOPE_FINE: RegisterDoc = RegisterDoc {
    name: "SSG: envelope period (fine)",
    fields: &[bf("Envelope period (fine 8 bits)", 0xFF)],
};
const SSG_ENVELOPE_COARSE: RegisterDoc = RegisterDoc {
    name: "SSG: envelope period (coarse)",
    fields: &[bf("Envelope period (coarse 8 bits)", 0xFF)],
};
const SSG_ENVELOPE_SHAPE: RegisterDoc = RegisterDoc {
    name: "SSG: envelope shape",
    fields: &[
        bf("Continue", 0x08),
        bf("Attack", 0x04),
        bf("Alternate", 0x02),
        bf("Hold", 0x01),
    ],
};
const SSG_IO: RegisterDoc = RegisterDoc {
    name: "SSG: I/O port data",
    fields: &[bf("Port data", 0xFF)],
};

const fn ssg(addr: u16) -> Option<&'static RegisterDoc> {
    Some(match addr {
        0x00 | 0x02 | 0x04 => &SSG_TONE_FINE,
        0x01 | 0x03 | 0x05 => &SSG_TONE_COARSE,
        0x06 => &SSG_NOISE,
        0x07 => &SSG_MIXER,
        0x08..=0x0A => &SSG_AMPLITUDE,
        0x0B => &SSG_ENVELOPE_FINE,
        0x0C => &SSG_ENVELOPE_COARSE,
        0x0D => &SSG_ENVELOPE_SHAPE,
        0x0E | 0x0F => &SSG_IO,
        _ => return None,
    })
}

// The YM2608's rhythm section: six fixed ADPCM voices on port 0.
const RHYTHM_KEY: RegisterDoc = RegisterDoc {
    name: "Rhythm: key on / dump",
    fields: &[
        bf("Dump (key off)", 0x80),
        bf("Rim shot", 0x20),
        bf("Tom", 0x10),
        bf("Hi-hat", 0x08),
        bf("Top cymbal", 0x04),
        bf("Snare drum", 0x02),
        bf("Bass drum", 0x01),
    ],
};
const RHYTHM_TOTAL_LEVEL: RegisterDoc = RegisterDoc {
    name: "Rhythm: total level",
    fields: &[bf("Rhythm total level", 0x3F)],
};
const RHYTHM_PAN_LEVEL: RegisterDoc = RegisterDoc {
    name: "Rhythm: pan / instrument level",
    fields: &[
        bf("Left output", 0x80),
        bf("Right output", 0x40),
        bf("Instrument level", 0x1F),
    ],
};

const fn rhythm(addr: u16) -> Option<&'static RegisterDoc> {
    Some(match addr {
        0x10 => &RHYTHM_KEY,
        0x11 => &RHYTHM_TOTAL_LEVEL,
        0x18..=0x1D => &RHYTHM_PAN_LEVEL,
        _ => return None,
    })
}

// ADPCM-B (delta-T): port 1 registers 0x00-0x0D on the YM2608; the YM2610
// carries the same roles on port 0 at 0x10+, minus the prescale and CPU-data
// registers it has no pins for, plus the flag-control byte at 0x1C.
const ADPCM_B_CONTROL_1: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: control 1 (start / record / repeat)",
    fields: &[
        bf("Start", 0x80),
        bf("Record", 0x40),
        bf("External memory", 0x20),
        bf("Repeat", 0x10),
        bf("Reset", 0x01),
    ],
};
const ADPCM_B_CONTROL_2: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: control 2 (pan / memory type)",
    fields: &[
        bf("Left output", 0x80),
        bf("Right output", 0x40),
        bf("Memory type", 0x03),
    ],
};
const ADPCM_B_START_LOW: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: start address (low)",
    fields: &[bf("Start address (low)", 0xFF)],
};
const ADPCM_B_START_HIGH: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: start address (high)",
    fields: &[bf("Start address (high)", 0xFF)],
};
const ADPCM_B_STOP_LOW: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: stop address (low)",
    fields: &[bf("Stop address (low)", 0xFF)],
};
const ADPCM_B_STOP_HIGH: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: stop address (high)",
    fields: &[bf("Stop address (high)", 0xFF)],
};
const ADPCM_B_PRESCALE_LOW: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: prescale (low)",
    fields: &[bf("Prescale (low)", 0xFF)],
};
const ADPCM_B_PRESCALE_HIGH: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: prescale (high)",
    fields: &[bf("Prescale (high)", 0x07)],
};
const ADPCM_B_DATA: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: data",
    fields: &[bf("ADPCM data", 0xFF)],
};
const ADPCM_B_DELTA_LOW: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: delta-N (low)",
    fields: &[bf("Delta-N (low)", 0xFF)],
};
const ADPCM_B_DELTA_HIGH: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: delta-N (high)",
    fields: &[bf("Delta-N (high)", 0xFF)],
};
const ADPCM_B_LEVEL: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: output level",
    fields: &[bf("Output level", 0xFF)],
};
const ADPCM_B_LIMIT_LOW: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: limit address (low)",
    fields: &[bf("Limit address (low)", 0xFF)],
};
const ADPCM_B_LIMIT_HIGH: RegisterDoc = RegisterDoc {
    name: "ADPCM-B: limit address (high)",
    fields: &[bf("Limit address (high)", 0xFF)],
};
const ADPCM_FLAG: RegisterDoc = RegisterDoc {
    name: "ADPCM: end-of-sample flag control",
    fields: &[
        bf("ADPCM-B flag reset / mask", 0x80),
        bf("ADPCM-A flag reset / mask", 0x3F),
    ],
};

const fn adpcm_b_2608(addr: u16) -> Option<&'static RegisterDoc> {
    Some(match addr {
        0x00 => &ADPCM_B_CONTROL_1,
        0x01 => &ADPCM_B_CONTROL_2,
        0x02 => &ADPCM_B_START_LOW,
        0x03 => &ADPCM_B_START_HIGH,
        0x04 => &ADPCM_B_STOP_LOW,
        0x05 => &ADPCM_B_STOP_HIGH,
        0x06 => &ADPCM_B_PRESCALE_LOW,
        0x07 => &ADPCM_B_PRESCALE_HIGH,
        0x08 => &ADPCM_B_DATA,
        0x09 => &ADPCM_B_DELTA_LOW,
        0x0A => &ADPCM_B_DELTA_HIGH,
        0x0B => &ADPCM_B_LEVEL,
        0x0C => &ADPCM_B_LIMIT_LOW,
        0x0D => &ADPCM_B_LIMIT_HIGH,
        _ => return None,
    })
}

const fn adpcm_b_2610(addr: u16) -> Option<&'static RegisterDoc> {
    Some(match addr {
        0x10 => &ADPCM_B_CONTROL_1,
        0x11 => &ADPCM_B_CONTROL_2,
        0x12 => &ADPCM_B_START_LOW,
        0x13 => &ADPCM_B_START_HIGH,
        0x14 => &ADPCM_B_STOP_LOW,
        0x15 => &ADPCM_B_STOP_HIGH,
        0x19 => &ADPCM_B_DELTA_LOW,
        0x1A => &ADPCM_B_DELTA_HIGH,
        0x1B => &ADPCM_B_LEVEL,
        0x1C => &ADPCM_FLAG,
        _ => return None,
    })
}

// The YM2610's ADPCM-A section: six sample voices on port 1. The address
// ranges repeat per channel (`addr & 7` selects the channel); their docs
// name the role, not the channel.
const ADPCM_A_KEY: RegisterDoc = RegisterDoc {
    name: "ADPCM-A: dump / key on",
    fields: &[bf("Dump (key off)", 0x80), bf("Channel key mask", 0x3F)],
};
const ADPCM_A_LEVEL: RegisterDoc = RegisterDoc {
    name: "ADPCM-A: shared total level",
    fields: &[bf("Total level", 0x3F)],
};
const ADPCM_A_PAN_LEVEL: RegisterDoc = RegisterDoc {
    name: "ADPCM-A: pan / instrument level",
    fields: &[
        bf("Left output", 0x80),
        bf("Right output", 0x40),
        bf("Instrument level", 0x1F),
    ],
};
const ADPCM_A_START_LOW: RegisterDoc = RegisterDoc {
    name: "ADPCM-A: start address (low)",
    fields: &[bf("Start address (low)", 0xFF)],
};
const ADPCM_A_START_HIGH: RegisterDoc = RegisterDoc {
    name: "ADPCM-A: start address (high)",
    fields: &[bf("Start address (high)", 0xFF)],
};
const ADPCM_A_END_LOW: RegisterDoc = RegisterDoc {
    name: "ADPCM-A: end address (low)",
    fields: &[bf("End address (low)", 0xFF)],
};
const ADPCM_A_END_HIGH: RegisterDoc = RegisterDoc {
    name: "ADPCM-A: end address (high)",
    fields: &[bf("End address (high)", 0xFF)],
};

const fn adpcm_a(addr: u16) -> Option<&'static RegisterDoc> {
    Some(match addr {
        0x00 => &ADPCM_A_KEY,
        0x01 => &ADPCM_A_LEVEL,
        0x08..=0x0D => &ADPCM_A_PAN_LEVEL,
        0x10..=0x15 => &ADPCM_A_START_LOW,
        0x18..=0x1D => &ADPCM_A_START_HIGH,
        0x20..=0x25 => &ADPCM_A_END_LOW,
        0x28..=0x2D => &ADPCM_A_END_HIGH,
        _ => return None,
    })
}

/// The per-channel FM registers, identical on both ports and on all three
/// chips -- except the stereo/sensitivity register, which the mono,
/// LFO-less YM2203 lacks.
fn fm(chip: ChipKind, addr: u16) -> Option<&'static RegisterDoc> {
    // `addr & 3 == 3` is a hole: each block covers channels 1-3 at offsets 0-2.
    if addr & 3 == 3 {
        return None;
    }
    Some(match addr {
        0x30..=0x3E => &DT_MULTI,
        0x40..=0x4E => &TOTAL_LEVEL,
        0x50..=0x5E => &KS_AR,
        0x60..=0x6E => &AM_DR,
        0x70..=0x7E => &SR,
        0x80..=0x8E => &SL_RR,
        0x90..=0x9E => &SSG_EG,
        0xA0..=0xA2 => &FREQ_LOW,
        0xA4..=0xA6 => &FREQ_HIGH,
        0xA8..=0xAA => &CH3_FREQ_LOW,
        0xAC..=0xAE => &CH3_FREQ_HIGH,
        0xB0..=0xB2 => &FB_ALGO,
        0xB4..=0xB6 if chip != ChipKind::Ym2203 => &LR_AMS_PMS,
        _ => return None,
    })
}

/// The documentation for a write to `(chip, port, addr)`.
pub(super) fn doc(chip: ChipKind, port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    if port > 1 || (port == 1 && chip == ChipKind::Ym2203) {
        return None;
    }
    if port == 0 {
        // The SSG section, identical on all three chips.
        if addr <= 0x0F {
            return ssg(addr);
        }
        // 0x10-0x1D is the 2608's rhythm section; on the 2610 the same
        // range is its relocated ADPCM-B; on the 2203 it is empty.
        if chip == ChipKind::Ym2608
            && let Some(doc) = rhythm(addr)
        {
            return Some(doc);
        }
        if chip == ChipKind::Ym2610
            && let Some(doc) = adpcm_b_2610(addr)
        {
            return Some(doc);
        }
        // Global FM registers live on port 0 only.
        match addr {
            0x21 => return Some(&TEST),
            // The 2203 has no LFO.
            0x22 if chip != ChipKind::Ym2203 => return Some(&LFO),
            0x24 => return Some(&TIMER_A_HIGH),
            0x25 => return Some(&TIMER_A_LOW),
            0x26 => return Some(&TIMER_B),
            0x27 => return Some(&MODE_TIMER),
            0x28 => return Some(&KEY_ON_OFF),
            // The 2610's clock dividers are fixed: no prescaler registers.
            0x2D if chip != ChipKind::Ym2610 => return Some(&PRESCALER_6),
            0x2E if chip != ChipKind::Ym2610 => return Some(&PRESCALER_3),
            0x2F if chip != ChipKind::Ym2610 => return Some(&PRESCALER_2),
            _ => {}
        }
    } else {
        // Port 1: the 2608's ADPCM-B or the 2610's ADPCM-A, then FM 4-6.
        if chip == ChipKind::Ym2608
            && let Some(doc) = adpcm_b_2608(addr)
        {
            return Some(doc);
        }
        if chip == ChipKind::Ym2610
            && let Some(doc) = adpcm_a(addr)
        {
            return Some(doc);
        }
    }
    fm(chip, addr)
}

/// The registers a find dropdown offers for the YM2203.
pub(super) const NOTABLE_2203: &[(u8, u16, &str)] = &[
    (0, 0x28, "Key on/off"),
    (0, 0x07, "SSG mixer"),
    (0, 0x27, "Ch 3 mode / timer control"),
];

/// The registers a find dropdown offers for the YM2608 -- and, shared, for
/// the YM2610, where (0, 0x10) resolves to ADPCM-B control 1 and (1, 0x00)
/// to the ADPCM-A key register: different names, same hunt-worthy addresses.
pub(super) const NOTABLE_2608: &[(u8, u16, &str)] = &[
    (0, 0x28, "Key on/off"),
    (0, 0x10, "Rhythm key on / dump"),
    (0, 0x07, "SSG mixer"),
    (1, 0x00, "ADPCM-B control"),
];
