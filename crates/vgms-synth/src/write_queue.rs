//! Handing registers to a cycle-accurate core at a rate it can actually take
//! them.
//!
//! Lives here rather than in a provider crate because it is *our* glue -- no
//! upstream code, just a description of a constraint several upstreams share --
//! and because both the LGPL and GPL provider crates need it. Nothing in it is
//! specific to any one core, which is the point: the shape is shared and the
//! numbers are not.
//!
//! Every Nuke.YKT core latches a register write and applies it only when
//! the chip's rotation reaches that register's *slot*. Push a run of writes
//! straight through and every one is accepted, nothing errors, and the ones
//! that miss their slot simply never land -- which sounds like a note that
//! never starts, not like a fault.
//!
//! **How much room each half of the handover needs is per core, and measured.**
//! The YM2612 wants the address, its value on the next cycle, and then the rest
//! of its 24-cycle turn. The YM2151 needs a whole 32-cycle rotation on *each*
//! side of the pair: spacing its writes 1, 2, 3 or 6 cycles apart produces total
//! silence, while 4 gives full amplitude, 8 a quarter and 16 a half -- a
//! sequence that is not monotonic because those numbers are phases, not
//! durations. A spacing that happens to work for one patch is therefore no
//! evidence at all.
//!
//! So the shape is shared and the numbers are not. A new core in this family
//! gets its own [`WriteQueue::new`] figures, found by measuring, and a
//! burst-of-writes test to pin them.

use std::collections::VecDeque;

/// Where one register is in its handover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Nothing in flight; the next queued register can start.
    Idle,
    /// The address is on the bus. Carries where its value goes and the cycles
    /// owed before it may follow -- there is never a second register in flight,
    /// so the pair travels together rather than being looked up later.
    AddressSettling { left: u32, port: u32, value: u8 },
    /// The value is on the bus; these cycles are owed before another address
    /// may disturb it.
    ValueSettling(u32),
}

/// Registers waiting for their turn on a chip.
#[derive(Debug)]
pub struct WriteQueue {
    queue: VecDeque<(u32, u8, u8)>,
    phase: Phase,
    /// Cycles between the address and its value.
    address_settle: u32,
    /// Cycles after the value before the next address.
    value_settle: u32,
}

impl WriteQueue {
    /// A queue pacing writes for a core that needs `address_settle` cycles
    /// between an address and its value, and `value_settle` after the value.
    pub fn new(address_settle: u32, value_settle: u32) -> Self {
        Self {
            queue: VecDeque::new(),
            phase: Phase::Idle,
            address_settle,
            value_settle,
        }
    }

    /// Queues one register write. `port` is the chip's own address/data pair
    /// base -- 0 and 1, or 2 and 3 for a second bank.
    pub fn push(&mut self, port: u32, address: u8, value: u8) {
        self.queue.push_back((port, address, value));
    }

    /// Forgets everything pending. A seek must not deliver writes the song made
    /// before it.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.phase = Phase::Idle;
    }

    /// How many registers are still waiting.
    ///
    /// Not test-only: a provider crate's own tests need it, and `#[cfg(test)]`
    /// does not cross a crate boundary.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Moves the handover on by one internal cycle, calling `write` when a byte
    /// is due.
    pub fn advance(&mut self, mut write: impl FnMut(u32, u8)) {
        self.phase = match self.phase {
            Phase::Idle => match self.queue.pop_front() {
                Some((port, address, value)) => {
                    // Ports come in address/data pairs: 0 and 1, or 2 and 3 for
                    // a second bank.
                    write(port & !1, address);
                    Phase::AddressSettling {
                        left: self.address_settle,
                        port,
                        value,
                    }
                }
                None => Phase::Idle,
            },
            Phase::AddressSettling {
                left: 0,
                port,
                value,
            } => {
                // The value goes on the *next* cycle after the address at the
                // earliest, which is what `address_settle == 0` means.
                write(port | 1, value);
                Phase::ValueSettling(self.value_settle)
            }
            Phase::AddressSettling { left, port, value } => Phase::AddressSettling {
                left: left - 1,
                port,
                value,
            },
            Phase::ValueSettling(0) => Phase::Idle,
            Phase::ValueSettling(left) => Phase::ValueSettling(left - 1),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives a queue for `cycles` and returns everything it wrote.
    fn run(queue: &mut WriteQueue, cycles: u32) -> Vec<(u32, u8)> {
        let mut written = Vec::new();
        for _ in 0..cycles {
            queue.advance(|port, byte| written.push((port, byte)));
        }
        written
    }

    /// The YM2612's pacing: address, value on the next cycle, then the rest of
    /// a 24-cycle rotation -- one register per output sample.
    #[test]
    fn back_to_back_settling_gives_one_register_a_rotation() {
        let mut queue = WriteQueue::new(0, 21);
        queue.push(0, 0x28, 0xF0);
        queue.push(0, 0x30, 0x01);

        let first = run(&mut queue, 2);
        assert_eq!(first, [(0, 0x28), (1, 0xF0)], "address then value");
        // The next address must wait out the settle: 22 quiet cycles, so the
        // whole register costs the chip's 24-cycle rotation.
        assert!(
            run(&mut queue, 22).is_empty(),
            "the value needs its rotation"
        );
        assert_eq!(run(&mut queue, 2), [(0, 0x30), (1, 0x01)]);
    }

    /// The YM2151's pacing: a whole rotation each side of the pair.
    #[test]
    fn a_settle_on_both_sides_spaces_the_pair_too() {
        let mut queue = WriteQueue::new(32, 32);
        queue.push(0, 0x20, 0xC7);

        assert_eq!(run(&mut queue, 1), [(0, 0x20)], "the address goes first");
        assert!(
            run(&mut queue, 32).is_empty(),
            "the address needs its rotation"
        );
        assert_eq!(run(&mut queue, 1), [(1, 0xC7)], "then the value");
    }

    /// A second bank addresses its own port pair.
    #[test]
    fn a_second_bank_keeps_its_own_port_pair() {
        let mut queue = WriteQueue::new(0, 0);
        queue.push(2, 0x30, 0x01);
        assert_eq!(run(&mut queue, 2), [(2, 0x30), (3, 0x01)]);
    }

    /// A run must be delayed, never dropped -- and a seek must drop it all.
    #[test]
    fn a_run_finishes_and_a_clear_discards_it() {
        let mut queue = WriteQueue::new(0, 21);
        for register in 0x30..0x40u8 {
            queue.push(0, register, 0x01);
        }
        assert_eq!(queue.pending(), 16);
        let written = run(&mut queue, 24 * 16);
        assert_eq!(written.len(), 32, "sixteen registers, two bytes each");
        assert_eq!(queue.pending(), 0);

        for register in 0x30..0x40u8 {
            queue.push(0, register, 0x01);
        }
        queue.clear();
        assert_eq!(queue.pending(), 0);
        assert!(
            run(&mut queue, 100).is_empty(),
            "a clear must discard the run"
        );
    }
}
