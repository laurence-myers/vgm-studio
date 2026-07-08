//! The instruction type, decoded on access from the raw byte array.
//!
//! The Python `DROInstruction` was a heap object rebuilt on every `data[i]`, and
//! `data[i]` is on the path of every table-row paint, every analyser pass and the
//! seeker. Here it is a `Copy` enum: decoding allocates nothing.

use core::fmt;
use core::str::FromStr;

/// Which of the two OPL register banks an instruction addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bank {
    Low,
    High,
}

impl Bank {
    /// `0` for the low bank, `1` for the high bank.
    #[must_use]
    pub const fn index(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::High => 1,
        }
    }

    /// `"low"` or `"high"`, as the Python `("low", "high")[value]` produced.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
        }
    }

    /// The bank selected by bit 0 of `value`. Any non-zero value selects `High`.
    #[must_use]
    pub const fn from_bit(value: u8) -> Self {
        if value & 1 == 0 {
            Self::Low
        } else {
            Self::High
        }
    }

    /// The offset to OR into a register number to address this bank: `0x000` or `0x100`.
    #[must_use]
    pub const fn register_offset(self) -> u16 {
        (self.index() as u16) << 8
    }
}

impl fmt::Display for Bank {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Whether a delay was encoded with the one-byte or two-byte opcode.
///
/// The Python original carried the raw delay opcode in `DROInstruction.command`
/// and asked the enclosing `DROData` whether it was short or long. Since a
/// `DELAY_MS` instruction is only ever produced *because* the opcode matched one
/// of the two delay codes, the third `"???"` branch was unreachable; encoding the
/// answer here removes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DelayKind {
    Short,
    Long,
}

impl DelayKind {
    /// `"DLYS"` or `"DLYL"`, the tokens shown in the register column.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Short => "DLYS",
            Self::Long => "DLYL",
        }
    }

    /// `"Delay (short)"` or `"Delay (long)"`.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Short => "Delay (short)",
            Self::Long => "Delay (long)",
        }
    }
}

/// One decoded DRO instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DroInstruction {
    /// Write `value` to `reg`.
    ///
    /// `bank` is `Some` only for DRO v2, which encodes the bank in the high bit
    /// of every register code. DRO v1 tracks the bank with [`Self::BankSwitch`]
    /// instructions instead, so its register writes carry `None`.
    Register {
        reg: u8,
        value: u8,
        bank: Option<Bank>,
    },
    /// Select a register bank (DRO v1 only).
    BankSwitch(Bank),
    /// Wait `ms` milliseconds.
    DelayMs { kind: DelayKind, ms: u32 },
}

impl DroInstruction {
    /// The delay in milliseconds, or `0` for anything that is not a delay.
    #[must_use]
    pub const fn delay_ms(self) -> u32 {
        match self {
            Self::DelayMs { ms, .. } => ms,
            _ => 0,
        }
    }

    /// The bank this instruction *selects*, if any.
    ///
    /// A v2 register write selects its own bank; a v1 bank switch selects one
    /// explicitly. Delays never do.
    #[must_use]
    pub const fn selected_bank(self) -> Option<Bank> {
        match self {
            Self::Register { bank, .. } => bank,
            Self::BankSwitch(bank) => Some(bank),
            Self::DelayMs { .. } => None,
        }
    }
}

/// What `Find Register` is looking for.
///
/// Replaces the Python's magic strings (`"DLYS"`, `"DLYL"`, `"DALL"`, `"BANK"`,
/// or a hex register number) and the per-step comparison lambda they selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindTarget {
    Register(u8),
    ShortDelay,
    LongDelay,
    AnyDelay,
    BankSwitch,
}

impl FindTarget {
    #[must_use]
    pub fn matches(self, instruction: DroInstruction) -> bool {
        match (self, instruction) {
            (Self::Register(wanted), DroInstruction::Register { reg, .. }) => reg == wanted,
            (
                Self::ShortDelay,
                DroInstruction::DelayMs {
                    kind: DelayKind::Short,
                    ..
                },
            ) => true,
            (
                Self::LongDelay,
                DroInstruction::DelayMs {
                    kind: DelayKind::Long,
                    ..
                },
            ) => true,
            (Self::AnyDelay, DroInstruction::DelayMs { .. }) => true,
            (Self::BankSwitch, DroInstruction::BankSwitch(_)) => true,
            _ => false,
        }
    }
}

/// The string `"0x2A"` (or `"2A"`) was not a token or a hex register number.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a register number or a DLYS/DLYL/DALL/BANK token: {0:?}")]
pub struct ParseFindTargetError(pub String);

impl FromStr for FindTarget {
    type Err = ParseFindTargetError;

    /// Accepts the four tokens, or a register number in hex with an optional
    /// `0x` prefix -- Python's `int(s, 16)` took both forms.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DLYS" => return Ok(Self::ShortDelay),
            "DLYL" => return Ok(Self::LongDelay),
            "DALL" => return Ok(Self::AnyDelay),
            "BANK" => return Ok(Self::BankSwitch),
            _ => {}
        }
        let digits = s
            .strip_prefix("0x")
            .or_else(|| s.strip_prefix("0X"))
            .unwrap_or(s);
        u8::from_str_radix(digits, 16)
            .map(Self::Register)
            .map_err(|_| ParseFindTargetError(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_helpers() {
        assert_eq!(Bank::from_bit(0), Bank::Low);
        assert_eq!(Bank::from_bit(1), Bank::High);
        assert_eq!(Bank::Low.index(), 0);
        assert_eq!(Bank::High.index(), 1);
        assert_eq!(Bank::Low.name(), "low");
        assert_eq!(Bank::High.name(), "high");
        assert_eq!(Bank::Low.register_offset(), 0x000);
        assert_eq!(Bank::High.register_offset(), 0x100);
    }

    #[test]
    fn instruction_is_small_and_copy() {
        // Cheap enough to decode per table-row paint without allocating.
        assert!(size_of::<DroInstruction>() <= 8);
        let inst = DroInstruction::Register {
            reg: 0x20,
            value: 1,
            bank: None,
        };
        let copy = inst;
        assert_eq!(inst, copy);
    }

    #[test]
    fn delay_ms_is_zero_for_non_delays() {
        assert_eq!(DroInstruction::BankSwitch(Bank::High).delay_ms(), 0);
        assert_eq!(
            DroInstruction::Register {
                reg: 1,
                value: 2,
                bank: None
            }
            .delay_ms(),
            0
        );
        assert_eq!(
            DroInstruction::DelayMs {
                kind: DelayKind::Long,
                ms: 49_408
            }
            .delay_ms(),
            49_408
        );
    }

    #[test]
    fn parse_find_target_accepts_tokens_and_hex() {
        assert_eq!("DLYS".parse(), Ok(FindTarget::ShortDelay));
        assert_eq!("DLYL".parse(), Ok(FindTarget::LongDelay));
        assert_eq!("DALL".parse(), Ok(FindTarget::AnyDelay));
        assert_eq!("BANK".parse(), Ok(FindTarget::BankSwitch));
        // Python's `int(s_inst, 16)` accepted both of these.
        assert_eq!("0x50".parse(), Ok(FindTarget::Register(0x50)));
        assert_eq!("50".parse(), Ok(FindTarget::Register(0x50)));
        assert_eq!("bd".parse(), Ok(FindTarget::Register(0xBD)));
        assert!("0x100".parse::<FindTarget>().is_err());
        assert!("nope".parse::<FindTarget>().is_err());
        assert!("".parse::<FindTarget>().is_err());
    }

    #[test]
    fn find_target_matching() {
        let reg = DroInstruction::Register {
            reg: 0x50,
            value: 5,
            bank: Some(Bank::Low),
        };
        let short = DroInstruction::DelayMs {
            kind: DelayKind::Short,
            ms: 177,
        };
        let long = DroInstruction::DelayMs {
            kind: DelayKind::Long,
            ms: 49_408,
        };
        let bank = DroInstruction::BankSwitch(Bank::High);

        assert!(FindTarget::Register(0x50).matches(reg));
        assert!(!FindTarget::Register(0x40).matches(reg));
        assert!(!FindTarget::Register(0x50).matches(short));

        assert!(FindTarget::ShortDelay.matches(short));
        assert!(!FindTarget::ShortDelay.matches(long));
        assert!(FindTarget::LongDelay.matches(long));
        assert!(FindTarget::AnyDelay.matches(short));
        assert!(FindTarget::AnyDelay.matches(long));
        assert!(!FindTarget::AnyDelay.matches(reg));

        assert!(FindTarget::BankSwitch.matches(bank));
        assert!(!FindTarget::BankSwitch.matches(reg));
    }
}
