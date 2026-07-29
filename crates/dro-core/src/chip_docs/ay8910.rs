//! AY-3-8910 (PSG) register documentation.
//!
//! Sources: the General Instrument AY-3-8910/8912 Programmable Sound
//! Generator data manual, as long circulated in the community, and the MSX
//! Assembly Page's PSG documentation. Bit assignments are datasheet facts.
//!
//! Addressing: sixteen registers R0-R15, one bank, so only port 0. The R7
//! mixer bits are active low -- writing 0 enables the source -- which the
//! Description column leaves to the register name rather than negating every
//! field.

use super::{RegisterDoc, bf};

const TONE_A_FINE: RegisterDoc = RegisterDoc {
    name: "Tone period A (fine)",
    fields: &[bf("Tone period (low 8 bits)", 0xFF)],
};
const TONE_A_COARSE: RegisterDoc = RegisterDoc {
    name: "Tone period A (coarse)",
    fields: &[bf("Tone period (high 4 bits)", 0x0F)],
};
const TONE_B_FINE: RegisterDoc = RegisterDoc {
    name: "Tone period B (fine)",
    fields: &[bf("Tone period (low 8 bits)", 0xFF)],
};
const TONE_B_COARSE: RegisterDoc = RegisterDoc {
    name: "Tone period B (coarse)",
    fields: &[bf("Tone period (high 4 bits)", 0x0F)],
};
const TONE_C_FINE: RegisterDoc = RegisterDoc {
    name: "Tone period C (fine)",
    fields: &[bf("Tone period (low 8 bits)", 0xFF)],
};
const TONE_C_COARSE: RegisterDoc = RegisterDoc {
    name: "Tone period C (coarse)",
    fields: &[bf("Tone period (high 4 bits)", 0x0F)],
};
const NOISE_PERIOD: RegisterDoc = RegisterDoc {
    name: "Noise period",
    fields: &[bf("Noise period", 0x1F)],
};
const MIXER: RegisterDoc = RegisterDoc {
    name: "Mixer / enable (active low)",
    fields: &[
        bf("IO port B direction", 0x80),
        bf("IO port A direction", 0x40),
        bf("Noise C enable", 0x20),
        bf("Noise B enable", 0x10),
        bf("Noise A enable", 0x08),
        bf("Tone C enable", 0x04),
        bf("Tone B enable", 0x02),
        bf("Tone A enable", 0x01),
    ],
};
const AMPLITUDE_A: RegisterDoc = RegisterDoc {
    name: "Amplitude A",
    fields: &[bf("Envelope mode", 0x10), bf("Level", 0x0F)],
};
const AMPLITUDE_B: RegisterDoc = RegisterDoc {
    name: "Amplitude B",
    fields: &[bf("Envelope mode", 0x10), bf("Level", 0x0F)],
};
const AMPLITUDE_C: RegisterDoc = RegisterDoc {
    name: "Amplitude C",
    fields: &[bf("Envelope mode", 0x10), bf("Level", 0x0F)],
};
const ENVELOPE_FINE: RegisterDoc = RegisterDoc {
    name: "Envelope period (fine)",
    fields: &[bf("Envelope period (low 8 bits)", 0xFF)],
};
const ENVELOPE_COARSE: RegisterDoc = RegisterDoc {
    name: "Envelope period (coarse)",
    fields: &[bf("Envelope period (high 8 bits)", 0xFF)],
};
const ENVELOPE_SHAPE: RegisterDoc = RegisterDoc {
    name: "Envelope shape",
    fields: &[
        bf("Continue", 0x08),
        bf("Attack", 0x04),
        bf("Alternate", 0x02),
        bf("Hold", 0x01),
    ],
};
const IO_PORT_A: RegisterDoc = RegisterDoc {
    name: "IO port A data",
    fields: &[bf("Port data", 0xFF)],
};
const IO_PORT_B: RegisterDoc = RegisterDoc {
    name: "IO port B data",
    fields: &[bf("Port data", 0xFF)],
};

/// The documentation for a write to `(port, addr)`.
pub(super) const fn doc(port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    if port != 0 {
        return None;
    }
    Some(match addr {
        0x00 => &TONE_A_FINE,
        0x01 => &TONE_A_COARSE,
        0x02 => &TONE_B_FINE,
        0x03 => &TONE_B_COARSE,
        0x04 => &TONE_C_FINE,
        0x05 => &TONE_C_COARSE,
        0x06 => &NOISE_PERIOD,
        0x07 => &MIXER,
        0x08 => &AMPLITUDE_A,
        0x09 => &AMPLITUDE_B,
        0x0A => &AMPLITUDE_C,
        0x0B => &ENVELOPE_FINE,
        0x0C => &ENVELOPE_COARSE,
        0x0D => &ENVELOPE_SHAPE,
        0x0E => &IO_PORT_A,
        0x0F => &IO_PORT_B,
        _ => return None,
    })
}

/// The registers a find dropdown offers.
pub(super) const NOTABLE: &[(u8, u16, &str)] =
    &[(0, 0x07, "Mixer / enable"), (0, 0x0D, "Envelope shape")];
