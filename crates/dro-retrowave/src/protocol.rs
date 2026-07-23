//! The RetroWave wire format: SPI transactions tunnelled over a CDC serial port.
//!
//! Two layers, both pure:
//!
//! 1. [`pack`] frames one SPI transaction — a chip-select-on byte, the payload
//!    re-packed 7 bits to a byte, a chip-select-off byte.
//! 2. [`CmdBuffer`] turns OPL3 register writes into those payloads, coalescing
//!    consecutive writes into one transaction.
//!
//! Written from the interface facts recorded in `docs/retrowave-2026-07/PLAN.md`
//! §1, never from the (AGPL) reference implementation; see §2 of that document.

/// Control byte: assert SPI chip select. Opens a transaction.
const CS_ON: u8 = 0x00;

/// Control byte: release SPI chip select. Closes a transaction, and clears the
/// device's bit accumulator — this is the protocol's only resynchronisation
/// point, which is why even an empty transaction is worth sending at init.
const CS_OFF: u8 = 0x02;

/// Set on every packed data byte, distinguishing it from a control byte.
const DATA_FLAG: u8 = 0x01;

/// SPI address of the OPL3 board's I/O expander, shifted into its command byte.
pub(crate) const BOARD_OPL3: u8 = 0x21 << 1;

/// The expander's GPIOA register: where every OPL3 bus transaction starts.
pub(crate) const REG_GPIOA: u8 = 0x12;

/// Payload size at which [`CmdBuffer`] seals a transaction on its own.
///
/// Coalescing is unbounded in principle, but a stall in the consuming pump would
/// otherwise let the payload grow without limit. 8 KiB is ~1360 register writes:
/// far more than any quantum produces, so in practice this never fires.
const SEAL_THRESHOLD: usize = 8192;

/// The number of wire bytes [`pack`] produces for a `len`-byte payload.
#[must_use]
pub fn packed_len(len: usize) -> usize {
    // Every 7 payload bits become one wire byte, plus the two control bytes.
    (len * 8).div_ceil(7) + 2
}

/// Frames one SPI transaction, appending the wire bytes to `out`.
///
/// The payload is treated as an MSB-first bit stream: each wire byte carries the
/// next 7 bits in its high bits and the data flag in bit 0. A trailing partial
/// group is zero-padded (the device discards it — the transaction's byte count is
/// implied by chip select, not by the padding).
pub fn pack(payload: &[u8], out: &mut Vec<u8>) {
    out.reserve(packed_len(payload.len()));
    out.push(CS_ON);

    let total_bits = payload.len() * 8;
    let mut bit = 0;
    while bit < total_bits {
        let mut group = 0u8;
        for offset in 0..7 {
            let index = bit + offset;
            let set = index < total_bits && (payload[index / 8] >> (7 - index % 8)) & 1 == 1;
            group = (group << 1) | u8::from(set);
        }
        out.push((group << 1) | DATA_FLAG);
        bit += 7;
    }

    out.push(CS_OFF);
}

/// Which of the YMF262's two register arrays a write addresses.
///
/// Bank 1 is the OPL3 extension: it only accepts writes while the NEW bit
/// (register `0x105`) is set, and it is where a dual-OPL2 song's second chip
/// lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bank {
    Zero,
    One,
}

impl Bank {
    /// The address-latch and data-latch command bytes for this bank.
    const fn latches(self) -> (u8, u8) {
        match self {
            Bank::Zero => (0xE1, 0xE3),
            Bank::One => (0xE5, 0xE7),
        }
    }
}

/// Accumulates OPL3 register writes into packed, ready-to-send wire bytes.
///
/// Consecutive writes share one SPI transaction: the two-byte board/expander
/// header is sent once, then a six-byte group per register write. Sealing packs
/// the accumulated payload into [`wire`](Self::wire), which the caller writes to
/// the device and then [`clears`](Self::clear_wire).
///
/// Both buffers are reused across cycles, so steady-state playback allocates
/// nothing.
#[derive(Debug, Default)]
pub struct CmdBuffer {
    /// The unpacked SPI payload being built, empty or starting with the header.
    payload: Vec<u8>,
    /// Packed bytes from sealed transactions, awaiting the device.
    wire: Vec<u8>,
}

impl CmdBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a write of `value` to `reg` in `bank`.
    ///
    /// `reg` is the register number within the bank (`0x00..=0xFF`), not the
    /// engine's bank-encoded address.
    pub fn push_write(&mut self, bank: Bank, reg: u8, value: u8) {
        if self.payload.is_empty() {
            self.payload.extend_from_slice(&[BOARD_OPL3, REG_GPIOA]);
        }
        let (addr_latch, data_latch) = bank.latches();
        // Address, then data, then a final data byte that completes the write
        // strobe on the expander's port A.
        self.payload
            .extend_from_slice(&[addr_latch, reg, data_latch, value, 0xFB, value]);

        if self.payload.len() >= SEAL_THRESHOLD {
            self.seal();
        }
    }

    /// Queues a raw SPI payload as its own transaction, sealing anything pending.
    ///
    /// For the transactions that are not register writes: the expander init
    /// sequences and the chip reset.
    pub fn push_transaction(&mut self, payload: &[u8]) {
        self.seal();
        pack(payload, &mut self.wire);
    }

    /// Packs any pending payload into [`wire`](Self::wire).
    pub fn seal(&mut self) {
        if !self.payload.is_empty() {
            pack(&self.payload, &mut self.wire);
            self.payload.clear();
        }
    }

    /// The bytes ready to go to the device. Call [`seal`](Self::seal) first.
    #[must_use]
    pub fn wire(&self) -> &[u8] {
        &self.wire
    }

    /// Drops the sealed bytes, keeping the allocation.
    pub fn clear_wire(&mut self) {
        self.wire.clear();
    }

    /// Whether anything at all is queued, sealed or not.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.payload.is_empty() && self.wire.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        pack(payload, &mut out);
        out
    }

    /// The vector published with the protocol description. If this ever fails,
    /// nothing else in this crate is trustworthy.
    #[test]
    fn packs_the_published_golden_vector() {
        assert_eq!(
            packed(&[0xCA, 0xFE, 0xBA, 0xBE]),
            [0x00, 0xCB, 0x7F, 0xAF, 0x57, 0xE1, 0x02]
        );
    }

    /// The empty-transaction case, sent once at init to resynchronise the device.
    #[test]
    fn packs_a_single_zero_byte() {
        assert_eq!(packed(&[0x00]), [0x00, 0x01, 0x01, 0x02]);
    }

    #[test]
    fn packs_an_empty_payload_as_bare_control_bytes() {
        assert_eq!(packed(&[]), [CS_ON, CS_OFF]);
    }

    #[test]
    fn packed_len_matches_what_pack_produces() {
        for len in 0..64 {
            let payload = vec![0xA5; len];
            assert_eq!(
                packed(&payload).len(),
                packed_len(len),
                "length mismatch for a {len}-byte payload"
            );
        }
    }

    #[test]
    fn every_frame_is_delimited_and_flags_its_data_bytes() {
        for len in 0..64 {
            let payload: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let wire = packed(&payload);
            assert_eq!(wire[0], CS_ON);
            assert_eq!(wire[wire.len() - 1], CS_OFF);
            for &byte in &wire[1..wire.len() - 1] {
                assert_eq!(byte & DATA_FLAG, DATA_FLAG, "data byte missing its flag");
            }
        }
    }

    /// Round-trips the packing by decoding it the way the device does: strip the
    /// flag bits, concatenate the 7-bit groups, and read whole bytes back off.
    #[test]
    fn packing_round_trips_through_the_device_side_decode() {
        for len in 1..64 {
            let payload: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(31)).collect();
            let wire = packed(&payload);

            let mut bits = Vec::new();
            for &byte in &wire[1..wire.len() - 1] {
                for shift in (1..8).rev() {
                    bits.push((byte >> shift) & 1);
                }
            }
            let decoded: Vec<u8> = bits
                .chunks_exact(8)
                .map(|chunk| chunk.iter().fold(0u8, |acc, &bit| (acc << 1) | bit))
                .collect();

            assert_eq!(decoded, payload, "round trip failed for {len} bytes");
        }
    }

    #[test]
    fn a_register_write_carries_the_documented_bytes() {
        let mut buf = CmdBuffer::new();
        buf.push_write(Bank::Zero, 0x20, 0x01);
        buf.seal();

        let mut expected = Vec::new();
        pack(
            &[BOARD_OPL3, REG_GPIOA, 0xE1, 0x20, 0xE3, 0x01, 0xFB, 0x01],
            &mut expected,
        );
        assert_eq!(buf.wire(), expected);
    }

    #[test]
    fn a_bank_one_write_uses_the_high_bank_latches() {
        let mut buf = CmdBuffer::new();
        buf.push_write(Bank::One, 0x05, 0x01);
        buf.seal();

        let mut expected = Vec::new();
        pack(
            &[BOARD_OPL3, REG_GPIOA, 0xE5, 0x05, 0xE7, 0x01, 0xFB, 0x01],
            &mut expected,
        );
        assert_eq!(buf.wire(), expected);
    }

    #[test]
    fn consecutive_writes_share_one_transaction() {
        let mut buf = CmdBuffer::new();
        buf.push_write(Bank::Zero, 0x20, 0x01);
        buf.push_write(Bank::Zero, 0x40, 0x3F);
        buf.seal();

        // One header, two six-byte groups, one pair of control bytes.
        assert_eq!(buf.wire().len(), packed_len(2 + 6 + 6));
        assert_eq!(buf.wire().iter().filter(|&&b| b == CS_ON).count(), 1);
    }

    #[test]
    fn a_long_burst_seals_itself_before_growing_without_bound() {
        let mut buf = CmdBuffer::new();
        // Each write adds six payload bytes, so this crosses 8 KiB comfortably.
        for i in 0..2000u32 {
            buf.push_write(Bank::Zero, (i % 256) as u8, 0x00);
        }
        assert!(
            !buf.wire().is_empty(),
            "the buffer should have sealed itself mid-burst"
        );
    }

    #[test]
    fn a_raw_transaction_seals_pending_writes_first() {
        let mut buf = CmdBuffer::new();
        buf.push_write(Bank::Zero, 0x20, 0x01);
        buf.push_transaction(&[BOARD_OPL3, REG_GPIOA, 0xFE, 0x00]);

        // Two transactions, in order: the register write, then the raw one.
        assert_eq!(buf.wire().iter().filter(|&&b| b == CS_ON).count(), 2);
    }
}
