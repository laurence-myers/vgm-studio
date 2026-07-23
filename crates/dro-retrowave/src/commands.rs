//! The fixed command sequences: expander init, chip reset, and the mute sweep.
//!
//! All pure builders over a [`CmdBuffer`], so the device layer is thin and every
//! byte is testable without hardware. From `docs/retrowave-2026-07/PLAN.md` §1.3.

use crate::protocol::{BOARD_OPL3, Bank, CmdBuffer, REG_GPIOA};

/// Highest register the mute sweep touches.
const SWEEP_END: u8 = 0xF5;

/// The sweep deliberately starts here rather than at `0x00`.
///
/// A blind sweep from zero would write bank 1's registers `0x04` and `0x05` —
/// `0x105` being the NEW bit that makes bank 1 writable at all. Clearing it
/// partway through would make the chip ignore every later bank-1 write in the
/// same sweep, leaving stale state (and stuck notes) behind on channels 9-17.
const SWEEP_START: u8 = 0x20;

/// Whether `reg` is a total-level register, where silence is maximum attenuation
/// rather than zero.
#[must_use]
pub const fn is_total_level(reg: u8) -> bool {
    0x40 <= reg && reg <= 0x55
}

/// The value that silences `reg`.
const fn silent_value(reg: u8) -> u8 {
    if is_total_level(reg) { 0xFF } else { 0x00 }
}

/// Queues the expander initialisation, run once per opened port.
///
/// The leading empty transaction is not a no-op: its trailing chip-select-off
/// clears the device's bit accumulator, so a session that died mid-transaction
/// cannot desynchronise this one. The three per-address sequences then configure
/// every expander that might be present (only `0x21`, the OPL3, answers here) to
/// drive all its pins as outputs, idle high.
pub fn queue_io_init(buf: &mut CmdBuffer) {
    buf.push_transaction(&[0x00]);

    for addr in 0x20..=0x27u8 {
        let cmd = addr << 1;
        // IOCON: hardware addressing plus sequential operation.
        buf.push_transaction(&[cmd, 0x0A, 0x28, 0x28]);
        // IODIRA/B: every pin an output.
        buf.push_transaction(&[cmd, 0x00, 0x00, 0x00]);
        // GPIOA/B: idle the bus high.
        buf.push_transaction(&[cmd, REG_GPIOA, 0xFF, 0xFF]);
    }
}

/// Queues a hard reset of the YMF262 by pulsing its IC line.
///
/// The chip needs time to settle afterwards ([`RESET_SETTLE`]). Paid only when a
/// port is opened or closed for good — seeks reconstruct state through the
/// register diff instead, which is what keeps them inaudible.
pub fn queue_chip_reset(buf: &mut CmdBuffer) {
    buf.push_transaction(&[BOARD_OPL3, REG_GPIOA, 0xFE, 0x00]);
    buf.push_transaction(&[BOARD_OPL3, REG_GPIOA, 0xFF, 0x00]);
}

/// How long the YMF262 needs after [`queue_chip_reset`] before it accepts writes.
pub const RESET_SETTLE: core::time::Duration = core::time::Duration::from_millis(200);

/// Queues writes that silence the chip without knowing its current state.
///
/// Clobbers the register file by design, so a caller that models the hardware
/// must record what this wrote — see `SerialOpl3Chip::mute_sweep`, which stamps
/// its hardware model as it sweeps. Callers with no model (the connect sequence)
/// have nothing to keep in step.
///
/// Calls `record` for every register written, in the order written.
pub fn queue_mute_sweep(buf: &mut CmdBuffer, mut record: impl FnMut(Bank, u8, u8)) {
    for reg in SWEEP_START..=SWEEP_END {
        let value = silent_value(reg);
        for bank in [Bank::Zero, Bank::One] {
            buf.push_write(bank, reg, value);
            record(bank, reg, value);
        }
    }
}

/// The value a register holds after a chip reset, for the diffing chip's benefit.
///
/// Matches [`queue_mute_sweep`]: silence means maximum attenuation on the
/// total-level registers and zero everywhere else.
#[must_use]
pub const fn reset_value(reg: u8) -> u8 {
    silent_value(reg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{pack, packed_len};

    /// Counts transactions by their chip-select-on bytes.
    fn transaction_count(wire: &[u8]) -> usize {
        wire.iter().filter(|&&b| b == 0x00).count()
    }

    #[test]
    fn init_opens_with_the_resynchronising_empty_transaction() {
        let mut buf = CmdBuffer::new();
        queue_io_init(&mut buf);
        buf.seal();
        assert_eq!(&buf.wire()[..4], [0x00, 0x01, 0x01, 0x02]);
    }

    #[test]
    fn init_configures_all_eight_expander_addresses() {
        let mut buf = CmdBuffer::new();
        queue_io_init(&mut buf);
        buf.seal();
        // The empty transaction plus three per address.
        assert_eq!(transaction_count(buf.wire()), 1 + 8 * 3);
    }

    #[test]
    fn init_sends_the_documented_sequences_for_the_opl3_board() {
        let mut buf = CmdBuffer::new();
        queue_io_init(&mut buf);
        buf.seal();

        for payload in [
            [BOARD_OPL3, 0x0A, 0x28, 0x28],
            [BOARD_OPL3, 0x00, 0x00, 0x00],
            [BOARD_OPL3, REG_GPIOA, 0xFF, 0xFF],
        ] {
            let mut expected = Vec::new();
            pack(&payload, &mut expected);
            assert!(
                buf.wire()
                    .windows(expected.len())
                    .any(|window| window == expected),
                "missing init transaction {payload:02X?}"
            );
        }
    }

    #[test]
    fn reset_pulses_the_ic_line_low_then_high() {
        let mut buf = CmdBuffer::new();
        queue_chip_reset(&mut buf);
        buf.seal();

        let mut expected = Vec::new();
        pack(&[BOARD_OPL3, REG_GPIOA, 0xFE, 0x00], &mut expected);
        pack(&[BOARD_OPL3, REG_GPIOA, 0xFF, 0x00], &mut expected);
        assert_eq!(buf.wire(), expected);
    }

    #[test]
    fn the_mute_sweep_covers_both_banks_from_0x20_to_0xf5() {
        let mut buf = CmdBuffer::new();
        let mut written = Vec::new();
        queue_mute_sweep(&mut buf, |bank, reg, value| {
            written.push((bank, reg, value))
        });

        let expected_count = (0x20..=0xF5u8).count() * 2;
        assert_eq!(written.len(), expected_count);
        assert!(
            written
                .iter()
                .all(|&(_, reg, _)| (0x20..=0xF5).contains(&reg))
        );
        for bank in [Bank::Zero, Bank::One] {
            assert!(written.contains(&(bank, 0x20, 0x00)));
            assert!(written.contains(&(bank, 0xF5, 0x00)));
        }
    }

    /// The whole reason the sweep starts at `0x20`: touching bank 1's `0x05`
    /// would clear NEW and deafen the rest of its own sweep.
    #[test]
    fn the_mute_sweep_never_touches_the_new_or_connection_registers() {
        let mut buf = CmdBuffer::new();
        let mut written = Vec::new();
        queue_mute_sweep(&mut buf, |bank, reg, value| {
            written.push((bank, reg, value))
        });

        assert!(
            !written.iter().any(|&(_, reg, _)| reg < 0x20),
            "the sweep must not write below 0x20"
        );
        assert!(!written.contains(&(Bank::One, 0x05, 0x00)));
        assert!(!written.contains(&(Bank::One, 0x04, 0x00)));
    }

    #[test]
    fn the_mute_sweep_maxes_out_the_total_level_registers() {
        let mut buf = CmdBuffer::new();
        let mut written = Vec::new();
        queue_mute_sweep(&mut buf, |bank, reg, value| {
            written.push((bank, reg, value))
        });

        for &(_, reg, value) in &written {
            let expected = if (0x40..=0x55).contains(&reg) {
                0xFF
            } else {
                0x00
            };
            assert_eq!(
                value, expected,
                "wrong silent value for register {reg:#04X}"
            );
        }
    }

    #[test]
    fn the_mute_sweep_coalesces_into_few_transactions() {
        let mut buf = CmdBuffer::new();
        queue_mute_sweep(&mut buf, |_, _, _| {});
        buf.seal();

        // 428 writes * 6 bytes is over the seal threshold, so it splits — but
        // into a handful of transactions, not one per write.
        let count = transaction_count(buf.wire());
        assert!(
            (1..=4).contains(&count),
            "unexpected transaction count {count}"
        );
        assert!(buf.wire().len() < packed_len(428 * 6 + 8) + 64);
    }

    #[test]
    fn reset_value_agrees_with_the_sweep() {
        assert_eq!(reset_value(0x40), 0xFF);
        assert_eq!(reset_value(0x55), 0xFF);
        assert_eq!(reset_value(0x3F), 0x00);
        assert_eq!(reset_value(0x56), 0x00);
        assert!(is_total_level(0x4A));
        assert!(!is_total_level(0xB0));
    }
}
