//! The chip cores themselves.
//!
//! One module per chip, each an implementation of
//! [`ChipCore`](crate::chip::ChipCore) and nothing else: no routing, no banks,
//! no timing. [`core_for`](crate::chip::core_for) is what decides which of them
//! a file gets.
//!
//! Every core here is written from documented behaviour rather than ported, the
//! same Route B the optimiser, the splitter and the block decompressor took, so
//! the project stays LGPL-2.1-or-later. Vendoring an existing emulator is
//! allowed for by the plan (`docs/vgm-multichip-2026-07/HANDOVER.md` §7) and is
//! a licensing decision to make deliberately, not a shortcut to take quietly.

pub mod sn76489;

pub use sn76489::Sn76489;
