//! YM2413 (OPLL) register documentation.
//!
//! Sources: the Yamaha YM2413 application manual, as long circulated in the
//! community (the smspower.org transcription of the "YM2413 Application
//! Manual", and the MSX Assembly Page's MSX-Music documentation). Bit
//! assignments are datasheet facts.
//!
//! Addressing: a single register bank, so only port 0. Registers 0x00-0x07
//! edit the one user patch (its modulator and carrier operators); everything
//! from 0x10 up repeats per channel (nine channels, `addr & 0x0F` selecting
//! one), and those docs name the role, not the channel, exactly as the OPL
//! tables do.

use super::{RegisterDoc, bf};

const MOD_AM_VIB_EG: RegisterDoc = RegisterDoc {
    name: "User patch modulator: AM / vibrato / EG type / KSR / multiple",
    fields: &[
        bf("AM (tremolo)", 0x80),
        bf("Vibrato", 0x40),
        bf("EG type (sustained)", 0x20),
        bf("KSR (envelope scaling)", 0x10),
        bf("Frequency multiple", 0x0F),
    ],
};
const CAR_AM_VIB_EG: RegisterDoc = RegisterDoc {
    name: "User patch carrier: AM / vibrato / EG type / KSR / multiple",
    fields: &[
        bf("AM (tremolo)", 0x80),
        bf("Vibrato", 0x40),
        bf("EG type (sustained)", 0x20),
        bf("KSR (envelope scaling)", 0x10),
        bf("Frequency multiple", 0x0F),
    ],
};
const MOD_KSL_TL: RegisterDoc = RegisterDoc {
    name: "User patch modulator: key scale level / total level",
    fields: &[
        bf("Key scale level", 0xC0),
        bf("Total level (attenuation)", 0x3F),
    ],
};
const KSL_WAVE_FB: RegisterDoc = RegisterDoc {
    name: "User patch: carrier key scale level / waveforms / feedback",
    fields: &[
        bf("Key scale level (carrier)", 0xC0),
        bf("Carrier waveform (half-sine)", 0x10),
        bf("Modulator waveform (half-sine)", 0x08),
        bf("Feedback (modulator)", 0x07),
    ],
};
const MOD_AR_DR: RegisterDoc = RegisterDoc {
    name: "User patch modulator: attack rate / decay rate",
    fields: &[bf("Attack rate", 0xF0), bf("Decay rate", 0x0F)],
};
const CAR_AR_DR: RegisterDoc = RegisterDoc {
    name: "User patch carrier: attack rate / decay rate",
    fields: &[bf("Attack rate", 0xF0), bf("Decay rate", 0x0F)],
};
const MOD_SL_RR: RegisterDoc = RegisterDoc {
    name: "User patch modulator: sustain level / release rate",
    fields: &[bf("Sustain level", 0xF0), bf("Release rate", 0x0F)],
};
const CAR_SL_RR: RegisterDoc = RegisterDoc {
    name: "User patch carrier: sustain level / release rate",
    fields: &[bf("Sustain level", 0xF0), bf("Release rate", 0x0F)],
};
const RHYTHM: RegisterDoc = RegisterDoc {
    name: "Rhythm mode / rhythm key on",
    fields: &[
        bf("Rhythm mode", 0x20),
        bf("Bass drum key on", 0x10),
        bf("Snare drum key on", 0x08),
        bf("Tom-tom key on", 0x04),
        bf("Top cymbal key on", 0x02),
        bf("High hat key on", 0x01),
    ],
};
const TEST: RegisterDoc = RegisterDoc {
    name: "Test",
    fields: &[bf("Test", 0x0F)],
};
const FNUM_LOW: RegisterDoc = RegisterDoc {
    name: "Channel: F-number (low 8 bits)",
    fields: &[bf("F-number (low 8 bits)", 0xFF)],
};
const KEY_BLOCK: RegisterDoc = RegisterDoc {
    name: "Channel: sustain / key on / block / F-number (high bit)",
    fields: &[
        bf("Sustain on", 0x20),
        bf("Key on", 0x10),
        bf("Block (octave)", 0x0E),
        bf("F-number (high bit)", 0x01),
    ],
};
const INST_VOL: RegisterDoc = RegisterDoc {
    name: "Channel: instrument / volume",
    fields: &[bf("Instrument", 0xF0), bf("Volume (attenuation)", 0x0F)],
};

/// The documentation for a write to `(port, addr)`.
pub(super) const fn doc(port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    if port != 0 {
        return None;
    }
    Some(match addr {
        0x00 => &MOD_AM_VIB_EG,
        0x01 => &CAR_AM_VIB_EG,
        0x02 => &MOD_KSL_TL,
        0x03 => &KSL_WAVE_FB,
        0x04 => &MOD_AR_DR,
        0x05 => &CAR_AR_DR,
        0x06 => &MOD_SL_RR,
        0x07 => &CAR_SL_RR,
        0x0E => &RHYTHM,
        0x0F => &TEST,
        0x10..=0x18 => &FNUM_LOW,
        0x20..=0x28 => &KEY_BLOCK,
        0x30..=0x38 => &INST_VOL,
        _ => return None,
    })
}

/// The registers a find dropdown offers.
pub(super) const NOTABLE: &[(u8, u16, &str)] = &[
    (0, 0x0E, "Rhythm control"),
    (0, 0x20, "Key on / block (first channel)"),
    (0, 0x30, "Instrument / volume (first channel)"),
];
