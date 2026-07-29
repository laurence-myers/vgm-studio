//! OPL family (YM3526, YM3812, Y8950, YMF262) register documentation for the
//! *multichip* table.
//!
//! An OPL-only VGM never reaches this module -- it opens through the OPL
//! projection and uses [`regdata`](crate::regdata) as it always has. What
//! does reach it is the mixed file: an OPL beside a PCM chip, or an OPL VGM
//! carrying data blocks the projection refuses. For those rows this module
//! restates `regdata`'s tables in [`RegisterDoc`] shape (same sources:
//! shipbrook.com, shikadi.net) -- and a test pins every entry against
//! `regdata`, so the two cannot drift apart.
//!
//! Addressing: `port` is the OPL3's register bank (the projection's "high
//! bank" is port 1 here); the Y8950's ADPCM registers (0x07-0x15) are its
//! own and take precedence over the OPL tables where they overlap.

use super::{RegisterDoc, bf};
use crate::regdata::{self, RegisterKind};
use crate::vgm::ChipKind;

const TEST_LSI: RegisterDoc = RegisterDoc {
    name: "Test LSI Register / Waveform Select Enable",
    fields: &[
        bf("Waveform Select Enable", 0b0010_0000),
        bf("Test LSI Register", 0b0001_1111),
    ],
};
const TIMER_1_COUNT: RegisterDoc = RegisterDoc {
    name: "Timer 1 Count",
    fields: &[bf("Timer 1 Count", 0b1111_1111)],
};
const TIMER_2_COUNT: RegisterDoc = RegisterDoc {
    name: "Timer 2 Count",
    fields: &[bf("Timer 2 Count", 0b1111_1111)],
};
const TIMER_CONTROL: RegisterDoc = RegisterDoc {
    name: "1: Timer Control Flags (IRQ Reset / Mask / Start)   2: Four-Operator Enable",
    fields: &[
        bf("IRQ Reset", 0b1000_0000),
        bf("Timer 1 Mask", 0b0100_0000),
        bf("Timer 2 Mask", 0b0010_0000),
        bf("Timer 1 Start", 0b0000_0010),
        bf("Timer 2 Start", 0b0000_0001),
    ],
};
const FOUR_OPERATOR_ENABLE: RegisterDoc = RegisterDoc {
    name: "Four-Operator Enable",
    fields: &[
        bf("4-Operator enable for ch. 11 & 14", 0b0010_0000),
        bf("4-Operator enable for ch. 10 & 13", 0b0001_0000),
        bf("4-Operator enable for ch. 9 & 12", 0b0000_1000),
        bf("4-Operator enable for ch. 2 & 5", 0b0000_0100),
        bf("4-Operator enable for ch. 1 & 4", 0b0000_0010),
        bf("4-Operator enable for ch. 0 & 3", 0b0000_0001),
    ],
};
const OPL3_MODE_ENABLE: RegisterDoc = RegisterDoc {
    name: "OPL3 Mode Enable",
    fields: &[bf("OPL3 Mode Enable", 0b0000_0001)],
};
const SPEECH_SYNTHESIS: RegisterDoc = RegisterDoc {
    name: "Speech synthesis mode / Keyboard split note select (CSW / NOTE-SEL)",
    fields: &[
        bf("CSW (Speech synthesis mode)", 0b1000_0000),
        bf("Keyboard split", 0b0100_0000),
    ],
};
const OPERATOR_TVSKM: RegisterDoc = RegisterDoc {
    name: "Tremolo / Vibrato / Sustain / KSR / Frequency Multiplication Factor",
    fields: &[
        bf("Tremolo", 0b1000_0000),
        bf("Vibrato", 0b0100_0000),
        bf("Sustain", 0b0010_0000),
        bf("KSR (envelope scaling)", 0b0001_0000),
        bf("Frequency Multiplication Factor", 0b0000_1111),
    ],
};
const OPERATOR_KSL_LEVEL: RegisterDoc = RegisterDoc {
    name: "Key Scale Level / Output Level",
    fields: &[
        bf("Key Scale Level", 0b1100_0000),
        bf("Output Level", 0b0011_1111),
    ],
};
const OPERATOR_ATTACK_DECAY: RegisterDoc = RegisterDoc {
    name: "Attack Rate / Decay Rate",
    fields: &[
        bf("Attack Rate", 0b1111_0000),
        bf("Decay Rate", 0b0000_1111),
    ],
};
const OPERATOR_SUSTAIN_RELEASE: RegisterDoc = RegisterDoc {
    name: "Sustain Level / Release Rate",
    fields: &[
        bf("Sustain Level", 0b1111_0000),
        bf("Release Rate", 0b0000_1111),
    ],
};
const CHANNEL_FREQUENCY_LOW: RegisterDoc = RegisterDoc {
    name: "Frequency Number (low 8 bits)",
    fields: &[bf("Frequency Number (low 8 bits)", 0b1111_1111)],
};
const CHANNEL_KEY_ON: RegisterDoc = RegisterDoc {
    name: "Key On / Octave / Frequency (high 2 bits)",
    fields: &[
        bf("Key On", 0b0010_0000),
        bf("Octave", 0b0001_1100),
        bf("Frequency (high 2 bits)", 0b0000_0011),
    ],
};
const PERCUSSION_CONTROL: RegisterDoc = RegisterDoc {
    name: "AM depth / Vibrato depth / Percussion control",
    fields: &[
        bf("Tremolo depth", 0b1000_0000),
        bf("Vibrato depth", 0b0100_0000),
        bf("Percussion mode", 0b0010_0000),
        bf("BD", 0b0001_0000),
        bf("SD", 0b0000_1000),
        bf("TT", 0b0000_0100),
        bf("CY", 0b0000_0010),
        bf("HH", 0b0000_0001),
    ],
};
const CHANNEL_FEEDBACK_PAN: RegisterDoc = RegisterDoc {
    name: "Feedback strength / Panning / Synthesis type",
    fields: &[
        bf("Pan right", 0b0010_0000),
        bf("Pan left", 0b0001_0000),
        bf("Feedback", 0b0000_1110),
        bf("Synthesis type", 0b0000_0001),
    ],
};
const OPERATOR_WAVEFORM: RegisterDoc = RegisterDoc {
    name: "Waveform Select",
    fields: &[bf("Waveform Select", 0b0000_0111)],
};

/// The `regdata` kind's `RegisterDoc` restatement. Every kind has one; the
/// pinning test walks this pairing.
pub(super) const fn doc_for_kind(kind: RegisterKind) -> &'static RegisterDoc {
    use RegisterKind::*;
    match kind {
        TestLsi => &TEST_LSI,
        Timer1Count => &TIMER_1_COUNT,
        Timer2Count => &TIMER_2_COUNT,
        TimerControlOrFourOperator => &TIMER_CONTROL,
        FourOperatorEnable => &FOUR_OPERATOR_ENABLE,
        Opl3ModeEnable => &OPL3_MODE_ENABLE,
        SpeechSynthesis => &SPEECH_SYNTHESIS,
        OperatorTremoloVibratoSustainKsrMultiplier => &OPERATOR_TVSKM,
        OperatorKeyScaleLevelOutputLevel => &OPERATOR_KSL_LEVEL,
        OperatorAttackDecay => &OPERATOR_ATTACK_DECAY,
        OperatorSustainRelease => &OPERATOR_SUSTAIN_RELEASE,
        ChannelFrequencyLow => &CHANNEL_FREQUENCY_LOW,
        ChannelKeyOnOctaveFrequencyHigh => &CHANNEL_KEY_ON,
        PercussionControl => &PERCUSSION_CONTROL,
        ChannelFeedbackPanningSynthesis => &CHANNEL_FEEDBACK_PAN,
        OperatorWaveformSelect => &OPERATOR_WAVEFORM,
    }
}

// The Y8950's ADPCM speech section, which the OPL map does not cover.
// Source: Yamaha Y8950 (MSX-Audio) application manual register map, as on
// the long-public MSX Assembly Page hardware documentation.
const ADPCM_CONTROL_1: RegisterDoc = RegisterDoc {
    name: "ADPCM: start / record / memory / repeat",
    fields: &[
        bf("Start", 0b1000_0000),
        bf("Record", 0b0100_0000),
        bf("Memory data (RAM/ROM)", 0b0010_0000),
        bf("Repeat", 0b0001_0000),
        bf("Speaker off", 0b0000_0001),
    ],
};
const ADPCM_CONTROL_2: RegisterDoc = RegisterDoc {
    name: "ADPCM: memory type / RAM size",
    fields: &[bf("Memory type", 0b1100_0000), bf("RAM type", 0b0000_0011)],
};
const ADPCM_START_LOW: RegisterDoc = RegisterDoc {
    name: "ADPCM: start address (low)",
    fields: &[bf("Start address (low)", 0xFF)],
};
const ADPCM_START_HIGH: RegisterDoc = RegisterDoc {
    name: "ADPCM: start address (high)",
    fields: &[bf("Start address (high)", 0xFF)],
};
const ADPCM_STOP_LOW: RegisterDoc = RegisterDoc {
    name: "ADPCM: stop address (low)",
    fields: &[bf("Stop address (low)", 0xFF)],
};
const ADPCM_STOP_HIGH: RegisterDoc = RegisterDoc {
    name: "ADPCM: stop address (high)",
    fields: &[bf("Stop address (high)", 0xFF)],
};
const ADPCM_DATA: RegisterDoc = RegisterDoc {
    name: "ADPCM: data",
    fields: &[bf("ADPCM data", 0xFF)],
};
const ADPCM_DELTA_LOW: RegisterDoc = RegisterDoc {
    name: "ADPCM: delta-N (low)",
    fields: &[bf("Delta-N (low)", 0xFF)],
};
const ADPCM_DELTA_HIGH: RegisterDoc = RegisterDoc {
    name: "ADPCM: delta-N (high)",
    fields: &[bf("Delta-N (high)", 0xFF)],
};
const ADPCM_LEVEL: RegisterDoc = RegisterDoc {
    name: "ADPCM: output level",
    fields: &[bf("Output level", 0xFF)],
};

const fn y8950_adpcm(addr: u16) -> Option<&'static RegisterDoc> {
    Some(match addr {
        0x07 => &ADPCM_CONTROL_1,
        0x08 => &ADPCM_CONTROL_2,
        0x09 => &ADPCM_START_LOW,
        0x0A => &ADPCM_START_HIGH,
        0x0B => &ADPCM_STOP_LOW,
        0x0C => &ADPCM_STOP_HIGH,
        0x0F => &ADPCM_DATA,
        0x10 => &ADPCM_DELTA_LOW,
        0x11 => &ADPCM_DELTA_HIGH,
        0x12 => &ADPCM_LEVEL,
        _ => return None,
    })
}

/// The documentation for a write to `(chip, port, addr)`.
pub(super) fn doc(chip: ChipKind, port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    if port > 1 || (port == 1 && chip != ChipKind::Ymf262) {
        return None;
    }
    if chip == ChipKind::Y8950
        && let Some(doc) = y8950_adpcm(addr)
    {
        return Some(doc);
    }
    // The same hardware-correct precedence as the OPL analyser: a high-bank
    // write resolves to the high-bank register if one exists.
    let key = (u16::from(port) << 8) | addr;
    let kind = regdata::register_kind(key).or_else(|| regdata::register_kind(addr))?;
    // The two-operator chips have no OPL3-only registers.
    if chip != ChipKind::Ymf262
        && matches!(
            kind,
            RegisterKind::FourOperatorEnable | RegisterKind::Opl3ModeEnable
        )
    {
        return None;
    }
    Some(doc_for_kind(kind))
}

/// The registers a find dropdown offers.
pub(super) const NOTABLE: &[(u8, u16, &str)] = &[
    (0, 0xBD, "Percussion control"),
    (0, 0x01, "Test LSI / Waveform Select Enable"),
    (0, 0x04, "Timer control"),
    (1, 0x04, "Four-Operator Enable"),
    (1, 0x05, "OPL3 Mode Enable"),
];
