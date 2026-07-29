//! YM2151 (OPM) register documentation.
//!
//! Sources: the Yamaha YM2151 application manual (long-public, widely
//! mirrored in arcade programming references) and the community's canonical
//! OPM register map as summarised on the VGMRips wiki. Bit assignments are
//! datasheet facts.
//!
//! Addressing: a single port of 256 registers. Channel registers repeat per
//! channel (`addr & 7` selects the channel), operator registers per operator
//! and channel (`addr & 0x18` the operator, `addr & 7` the channel); their
//! docs name the role, not the channel, exactly as the OPL tables do.

use super::{RegisterDoc, bf};

const TEST_LFO_RESET: RegisterDoc = RegisterDoc {
    name: "Test / LFO reset",
    fields: &[bf("LFO reset", 0x02)],
};
const KEY_ON: RegisterDoc = RegisterDoc {
    name: "Key on (operator mask + channel)",
    fields: &[bf("Operator on/off mask", 0x78), bf("Channel", 0x07)],
};
const NOISE: RegisterDoc = RegisterDoc {
    name: "Noise enable / frequency",
    fields: &[bf("Noise enable", 0x80), bf("Noise frequency", 0x1F)],
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
const CSM_TIMER: RegisterDoc = RegisterDoc {
    name: "CSM / timer control",
    fields: &[
        bf("CSM mode", 0x80),
        bf("Timer B IRQ reset", 0x20),
        bf("Timer A IRQ reset", 0x10),
        bf("Timer B IRQ enable", 0x08),
        bf("Timer A IRQ enable", 0x04),
        bf("Timer B load", 0x02),
        bf("Timer A load", 0x01),
    ],
};
const LFO_FREQUENCY: RegisterDoc = RegisterDoc {
    name: "LFO frequency",
    fields: &[bf("LFO frequency", 0xFF)],
};
const LFO_DEPTH: RegisterDoc = RegisterDoc {
    name: "LFO modulation depth (PMD/AMD)",
    fields: &[
        bf("Depth select (1 = PMD, 0 = AMD)", 0x80),
        bf("Modulation depth", 0x7F),
    ],
};
const CT_LFO_WAVEFORM: RegisterDoc = RegisterDoc {
    name: "CT outputs / LFO waveform",
    fields: &[
        bf("CT2 output", 0x80),
        bf("CT1 output", 0x40),
        bf("LFO waveform", 0x03),
    ],
};
const RL_FB_CONNECT: RegisterDoc = RegisterDoc {
    name: "Channel: stereo / feedback / connection",
    fields: &[
        bf("Right output", 0x80),
        bf("Left output", 0x40),
        bf("Feedback", 0x38),
        bf("Connection (algorithm)", 0x07),
    ],
};
const KEY_CODE: RegisterDoc = RegisterDoc {
    name: "Channel: key code",
    fields: &[bf("Octave", 0x70), bf("Note", 0x0F)],
};
const KEY_FRACTION: RegisterDoc = RegisterDoc {
    name: "Channel: key fraction",
    fields: &[bf("Key fraction", 0xFC)],
};
const PMS_AMS: RegisterDoc = RegisterDoc {
    name: "Channel: PM / AM sensitivity",
    fields: &[bf("PM sensitivity", 0x70), bf("AM sensitivity", 0x03)],
};
const DT1_MUL: RegisterDoc = RegisterDoc {
    name: "Operator: detune / multiple",
    fields: &[bf("Detune (DT1)", 0x70), bf("Frequency multiple", 0x0F)],
};
const TOTAL_LEVEL: RegisterDoc = RegisterDoc {
    name: "Operator: total level",
    fields: &[bf("Total level (attenuation)", 0x7F)],
};
const KS_AR: RegisterDoc = RegisterDoc {
    name: "Operator: key scale / attack rate",
    fields: &[bf("Key scale", 0xC0), bf("Attack rate", 0x1F)],
};
const AMSEN_D1R: RegisterDoc = RegisterDoc {
    name: "Operator: AM enable / decay rate",
    fields: &[bf("AM enable", 0x80), bf("Decay rate (D1R)", 0x1F)],
};
const DT2_D2R: RegisterDoc = RegisterDoc {
    name: "Operator: coarse detune / sustain rate",
    fields: &[bf("Detune (DT2)", 0xC0), bf("Sustain rate (D2R)", 0x1F)],
};
const D1L_RR: RegisterDoc = RegisterDoc {
    name: "Operator: sustain level / release rate",
    fields: &[bf("Sustain level (D1L)", 0xF0), bf("Release rate", 0x0F)],
};

/// The documentation for a write to `(port, addr)`.
pub(super) const fn doc(port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    if port != 0 {
        return None;
    }
    Some(match addr {
        0x01 => &TEST_LFO_RESET,
        0x08 => &KEY_ON,
        0x0F => &NOISE,
        0x10 => &TIMER_A_HIGH,
        0x11 => &TIMER_A_LOW,
        0x12 => &TIMER_B,
        0x14 => &CSM_TIMER,
        0x18 => &LFO_FREQUENCY,
        0x19 => &LFO_DEPTH,
        0x1B => &CT_LFO_WAVEFORM,
        0x20..=0x27 => &RL_FB_CONNECT,
        0x28..=0x2F => &KEY_CODE,
        0x30..=0x37 => &KEY_FRACTION,
        0x38..=0x3F => &PMS_AMS,
        0x40..=0x5F => &DT1_MUL,
        0x60..=0x7F => &TOTAL_LEVEL,
        0x80..=0x9F => &KS_AR,
        0xA0..=0xBF => &AMSEN_D1R,
        0xC0..=0xDF => &DT2_D2R,
        0xE0..=0xFF => &D1L_RR,
        _ => return None,
    })
}

/// The registers a find dropdown offers.
pub(super) const NOTABLE: &[(u8, u16, &str)] = &[
    (0, 0x08, "Key on"),
    (0, 0x0F, "Noise enable / frequency"),
    (0, 0x14, "CSM / timer control"),
    (0, 0x18, "LFO frequency"),
];
