//! Static OPL register descriptions.
//!
//! Data taken from:
//! - <http://www.shipbrook.com/jeff/sb.html>
//! - <http://www.gamedev.net/reference/articles/article447.asp>
//! - <http://www.shikadi.net/moddingwiki/OPL_chip>
//!
//! Both tables are keyed by a [`RegisterKind`], so they cannot drift and no
//! string hashing happens on the table-painting path.

/// A named field within a register's value byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterBitmask {
    pub description: &'static str,
    pub mask: u8,
}

const fn bm(description: &'static str, mask: u8) -> RegisterBitmask {
    RegisterBitmask { description, mask }
}

/// The distinct kinds of OPL register the app knows how to describe.
///
/// Registers 0x104 and 0x105 only exist in the high bank; every other kind is
/// addressed by an 8-bit register number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegisterKind {
    TestLsi,
    Timer1Count,
    Timer2Count,
    TimerControlOrFourOperator,
    FourOperatorEnable,
    Opl3ModeEnable,
    SpeechSynthesis,
    OperatorTremoloVibratoSustainKsrMultiplier,
    OperatorKeyScaleLevelOutputLevel,
    OperatorAttackDecay,
    OperatorSustainRelease,
    ChannelFrequencyLow,
    ChannelKeyOnOctaveFrequencyHigh,
    PercussionControl,
    ChannelFeedbackPanningSynthesis,
    OperatorWaveformSelect,
}

impl RegisterKind {
    /// The human-readable description.
    #[must_use]
    pub const fn description(self) -> &'static str {
        use RegisterKind::*;
        match self {
            TestLsi => "Test LSI Register / Waveform Select Enable",
            Timer1Count => "Timer 1 Count",
            Timer2Count => "Timer 2 Count",
            TimerControlOrFourOperator => {
                "1: Timer Control Flags (IRQ Reset / Mask / Start)   2: Four-Operator Enable"
            }
            FourOperatorEnable => "Four-Operator Enable",
            Opl3ModeEnable => "OPL3 Mode Enable",
            SpeechSynthesis => {
                "Speech synthesis mode / Keyboard split note select (CSW / NOTE-SEL)"
            }
            OperatorTremoloVibratoSustainKsrMultiplier => {
                "Tremolo / Vibrato / Sustain / KSR / Frequency Multiplication Factor"
            }
            OperatorKeyScaleLevelOutputLevel => "Key Scale Level / Output Level",
            OperatorAttackDecay => "Attack Rate / Decay Rate",
            OperatorSustainRelease => "Sustain Level / Release Rate",
            ChannelFrequencyLow => "Frequency Number (low 8 bits)",
            ChannelKeyOnOctaveFrequencyHigh => "Key On / Octave / Frequency (high 2 bits)",
            PercussionControl => "AM depth / Vibrato depth / Percussion control",
            ChannelFeedbackPanningSynthesis => "Feedback strength / Panning / Synthesis type",
            OperatorWaveformSelect => "Waveform Select",
        }
    }

    /// The value byte's fields, most significant first.
    ///
    /// Used by the detailed register analyser to report which fields a write
    /// actually changed.
    #[must_use]
    pub const fn bitmasks(self) -> &'static [RegisterBitmask] {
        use RegisterKind::*;
        match self {
            TestLsi => TEST_LSI,
            Timer1Count => TIMER_1_COUNT,
            Timer2Count => TIMER_2_COUNT,
            TimerControlOrFourOperator => TIMER_CONTROL,
            FourOperatorEnable => FOUR_OPERATOR_ENABLE,
            Opl3ModeEnable => OPL3_MODE_ENABLE,
            SpeechSynthesis => SPEECH_SYNTHESIS,
            OperatorTremoloVibratoSustainKsrMultiplier => OPERATOR_TVSKM,
            OperatorKeyScaleLevelOutputLevel => OPERATOR_KSL_LEVEL,
            OperatorAttackDecay => OPERATOR_ATTACK_DECAY,
            OperatorSustainRelease => OPERATOR_SUSTAIN_RELEASE,
            ChannelFrequencyLow => CHANNEL_FREQUENCY_LOW,
            ChannelKeyOnOctaveFrequencyHigh => CHANNEL_KEY_ON,
            PercussionControl => PERCUSSION_CONTROL,
            ChannelFeedbackPanningSynthesis => CHANNEL_FEEDBACK_PAN,
            OperatorWaveformSelect => OPERATOR_WAVEFORM,
        }
    }
}

// A `&[bm(..)]` literal is not const-promoted -- const promotion does not see
// through function calls -- so each table needs a name.

const TEST_LSI: &[RegisterBitmask] = &[
    bm("Waveform Select Enable", 0b0010_0000),
    bm("Test LSI Register", 0b0001_1111),
];
const TIMER_1_COUNT: &[RegisterBitmask] = &[bm("Timer 1 Count", 0b1111_1111)];
const TIMER_2_COUNT: &[RegisterBitmask] = &[bm("Timer 2 Count", 0b1111_1111)];
// TODO: registers 004 and 104 need revising.
const TIMER_CONTROL: &[RegisterBitmask] = &[
    bm("IRQ Reset", 0b1000_0000),
    bm("Timer 1 Mask", 0b0100_0000),
    bm("Timer 2 Mask", 0b0010_0000),
    bm("Timer 1 Start", 0b0000_0010),
    bm("Timer 2 Start", 0b0000_0001),
];
const FOUR_OPERATOR_ENABLE: &[RegisterBitmask] = &[
    bm("4-Operator enable for ch. 11 & 14", 0b0010_0000),
    bm("4-Operator enable for ch. 10 & 13", 0b0001_0000),
    bm("4-Operator enable for ch. 9 & 12", 0b0000_1000),
    bm("4-Operator enable for ch. 2 & 5", 0b0000_0100),
    bm("4-Operator enable for ch. 1 & 4", 0b0000_0010),
    bm("4-Operator enable for ch. 0 & 3", 0b0000_0001),
];
const OPL3_MODE_ENABLE: &[RegisterBitmask] = &[bm("OPL3 Mode Enable", 0b0000_0001)];
const SPEECH_SYNTHESIS: &[RegisterBitmask] = &[
    bm("CSW (Speech synthesis mode)", 0b1000_0000),
    bm("Keyboard split", 0b0100_0000),
];
const OPERATOR_TVSKM: &[RegisterBitmask] = &[
    bm("Tremolo", 0b1000_0000),
    bm("Vibrato", 0b0100_0000),
    bm("Sustain", 0b0010_0000),
    bm("KSR (envelope scaling)", 0b0001_0000),
    bm("Frequency Multiplication Factor", 0b0000_1111),
];
const OPERATOR_KSL_LEVEL: &[RegisterBitmask] = &[
    bm("Key Scale Level", 0b1100_0000),
    bm("Output Level", 0b0011_1111),
];
const OPERATOR_ATTACK_DECAY: &[RegisterBitmask] = &[
    bm("Attack Rate", 0b1111_0000),
    bm("Decay Rate", 0b0000_1111),
];
const OPERATOR_SUSTAIN_RELEASE: &[RegisterBitmask] = &[
    bm("Sustain Level", 0b1111_0000),
    bm("Release Rate", 0b0000_1111),
];
const CHANNEL_FREQUENCY_LOW: &[RegisterBitmask] =
    &[bm("Frequency Number (low 8 bits)", 0b1111_1111)];
const CHANNEL_KEY_ON: &[RegisterBitmask] = &[
    bm("Key On", 0b0010_0000),
    bm("Octave", 0b0001_1100),
    bm("Frequency (high 2 bits)", 0b0000_0011),
];
const PERCUSSION_CONTROL: &[RegisterBitmask] = &[
    bm("Tremolo depth", 0b1000_0000),
    bm("Vibrato depth", 0b0100_0000),
    bm("Percussion mode", 0b0010_0000),
    bm("BD", 0b0001_0000),
    bm("SD", 0b0000_1000),
    bm("TT", 0b0000_0100),
    bm("CY", 0b0000_0010),
    bm("HH", 0b0000_0001),
];
const CHANNEL_FEEDBACK_PAN: &[RegisterBitmask] = &[
    bm("Pan right", 0b0010_0000),
    bm("Pan left", 0b0001_0000),
    bm("Feedback", 0b0000_1110),
    bm("Synthesis type", 0b0000_0001),
];
const OPERATOR_WAVEFORM: &[RegisterBitmask] = &[bm("Waveform Select", 0b0000_0111)];

/// The register 0xBD, which controls percussion mode and the five percussion voices.
pub const PERCUSSION_REGISTER: u8 = 0xBD;

/// Looks up the kind of a register, addressed as `bank << 8 | reg`.
///
/// Returns `None` for registers with no entry; callers render those as
/// `"(unknown)"`.
#[must_use]
pub const fn register_kind(reg: u16) -> Option<RegisterKind> {
    use RegisterKind::*;
    Some(match reg {
        0x01 => TestLsi,
        0x02 => Timer1Count,
        0x03 => Timer2Count,
        0x04 => TimerControlOrFourOperator,
        0x08 => SpeechSynthesis,
        0x20..=0x35 => OperatorTremoloVibratoSustainKsrMultiplier,
        0x40..=0x55 => OperatorKeyScaleLevelOutputLevel,
        0x60..=0x75 => OperatorAttackDecay,
        0x80..=0x95 => OperatorSustainRelease,
        0xA0..=0xA8 => ChannelFrequencyLow,
        0xB0..=0xB8 => ChannelKeyOnOctaveFrequencyHigh,
        0xBD => PercussionControl,
        0xC0..=0xC8 => ChannelFeedbackPanningSynthesis,
        0xE0..=0xF5 => OperatorWaveformSelect,
        // High-bank-only registers. 0x105 selects OPL3 mode; 0x104 enables
        // four-operator channels. Both are written to port base+3.
        0x104 => FourOperatorEnable,
        0x105 => Opl3ModeEnable,
        _ => return None,
    })
}

/// The description for a register, or `None` if it is not a register we know.
#[must_use]
pub const fn register_description(reg: u16) -> Option<&'static str> {
    match register_kind(reg) {
        Some(kind) => Some(kind.description()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_registers_match_the_python_table() {
        assert_eq!(
            register_description(0x01),
            Some("Test LSI Register / Waveform Select Enable")
        );
        assert_eq!(register_description(0x02), Some("Timer 1 Count"));
        assert_eq!(register_description(0x03), Some("Timer 2 Count"));
        assert_eq!(
            register_description(0x04),
            Some("1: Timer Control Flags (IRQ Reset / Mask / Start)   2: Four-Operator Enable")
        );
        assert_eq!(register_description(0x104), Some("Four-Operator Enable"));
        assert_eq!(register_description(0x105), Some("OPL3 Mode Enable"));
        assert_eq!(
            register_description(0x30),
            Some("Tremolo / Vibrato / Sustain / KSR / Frequency Multiplication Factor")
        );
        assert_eq!(
            register_description(0x50),
            Some("Key Scale Level / Output Level")
        );
        assert_eq!(
            register_description(0xBD),
            Some("AM depth / Vibrato depth / Percussion control")
        );
        assert_eq!(register_description(0xF5), Some("Waveform Select"));
    }

    #[test]
    fn gaps_in_the_python_table_stay_gaps() {
        // Every register with no description entry.
        for reg in [
            0x00u16, 0x05, 0x06, 0x07, 0x09, 0x1F, 0x36, 0x3F, 0x56, 0x5F, 0x76, 0x7F, 0x96, 0x9F,
            0xA9, 0xAF, 0xB9, 0xBC, 0xBE, 0xBF, 0xC9, 0xCF, 0xDF, 0xF6, 0xFF, 0x100, 0x103, 0x106,
            0x120, 0x1BD,
        ] {
            assert_eq!(register_description(reg), None, "reg {reg:#05X}");
        }
    }

    #[test]
    fn the_python_table_had_exactly_this_many_keys() {
        // 1 + 1 + 1 + 1 + 1 + 1 + 1 (singletons: 01,02,03,04,104,105,08)
        // + 22 + 22 + 22 + 22 (operator banks) + 9 + 9 + 1 + 9 + 22
        let count = (0u16..=0x1FF)
            .filter(|&r| register_kind(r).is_some())
            .count();
        assert_eq!(count, 7 + 22 * 4 + 9 * 3 + 1 + 22);
        assert_eq!(count, 145);
    }

    #[test]
    fn percussion_register_has_five_voice_bits() {
        let kind = register_kind(u16::from(PERCUSSION_REGISTER)).unwrap();
        assert_eq!(kind, RegisterKind::PercussionControl);
        let voices: Vec<&str> = kind.bitmasks()[3..].iter().map(|b| b.description).collect();
        assert_eq!(voices, ["BD", "SD", "TT", "CY", "HH"]);
    }

    #[test]
    fn every_kind_has_at_least_one_bitmask_and_no_mask_is_zero() {
        for reg in 0u16..=0x1FF {
            let Some(kind) = register_kind(reg) else {
                continue;
            };
            let masks = kind.bitmasks();
            assert!(!masks.is_empty(), "{kind:?} has no bitmasks");
            assert!(
                masks.iter().all(|b| b.mask != 0),
                "{kind:?} has a zero mask"
            );
        }
    }

    #[test]
    fn bitmasks_within_a_kind_do_not_overlap() {
        for reg in 0u16..=0x1FF {
            let Some(kind) = register_kind(reg) else {
                continue;
            };
            let mut seen = 0u8;
            for mask in kind.bitmasks() {
                assert_eq!(seen & mask.mask, 0, "{kind:?} has overlapping bitmasks");
                seen |= mask.mask;
            }
        }
    }
}
