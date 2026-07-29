//! YM2612 (OPN2) register documentation.
//!
//! Sources: Yamaha YM2612 application notes as circulated in the long-public
//! Sega Genesis Software Manual excerpts, and the community's canonical
//! register map (<https://www.smspower.org/maxim/Documents/YM2612> and the
//! Sega Retro "YM2612" hardware page). Bit assignments are datasheet facts.
//!
//! Addressing: port 0 carries channels 1-3 and the global registers, port 1
//! carries channels 4-6. Operator registers repeat per channel (`addr & 3`
//! selects the channel, `addr & 0x0C` the operator); their docs name the
//! role, not the channel, exactly as the OPL tables do.

use super::{RegisterDoc, bf};

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
const DAC_DATA: RegisterDoc = RegisterDoc {
    name: "DAC data",
    fields: &[bf("DAC sample", 0xFF)],
};
const DAC_ENABLE: RegisterDoc = RegisterDoc {
    name: "DAC enable",
    fields: &[bf("DAC enable (replaces FM 6)", 0x80)],
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

/// The documentation for a write to `(port, addr)`.
pub(super) const fn doc(port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    if port > 1 {
        return None;
    }
    // Global registers live on port 0 only.
    if port == 0 {
        match addr {
            0x22 => return Some(&LFO),
            0x24 => return Some(&TIMER_A_HIGH),
            0x25 => return Some(&TIMER_A_LOW),
            0x26 => return Some(&TIMER_B),
            0x27 => return Some(&MODE_TIMER),
            0x28 => return Some(&KEY_ON_OFF),
            0x2A => return Some(&DAC_DATA),
            0x2B => return Some(&DAC_ENABLE),
            _ => {}
        }
    }
    // Per-channel registers, identical on both ports. `addr & 3 == 3` is a
    // hole: each block covers channels 1-3 at offsets 0-2.
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
        0xB4..=0xB6 => &LR_AMS_PMS,
        _ => return None,
    })
}

/// The registers a find dropdown offers.
pub(super) const NOTABLE: &[(u8, u16, &str)] = &[
    (0, 0x28, "Key on/off"),
    (0, 0x2A, "DAC data"),
    (0, 0x2B, "DAC enable"),
    (0, 0x22, "LFO enable / frequency"),
    (0, 0x27, "Ch 3 mode / timer control"),
];
