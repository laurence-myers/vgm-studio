//! The chip cores themselves.
//!
//! One module per chip, each an implementation of
//! [`ChipCore`](crate::chip::ChipCore) and nothing else: no routing, no banks,
//! no timing. [`core_for`](crate::chip::core_for) is what decides which of them
//! a file gets.
//!
//! **Everything in this module is permissively licensed, and that is a hard
//! rule, not a preference.** `dro-synth` is `MIT OR Apache-2.0` so it can be
//! reused without copyleft obligations, so a core lands here only if it was
//! written from documented behaviour (the Route B the optimiser, the splitter
//! and the block decompressor took) or ported from an MIT/BSD/ISC/zlib source
//! with the upstream notice kept verbatim.
//!
//! Copyleft cores — the Nuked family, the GPL-2 and LLE tiers — are not
//! excluded from the *program*, only from this crate: they live in provider
//! crates (`dro-cores-nuked`, `dro-cores-gpl`) that the application depends on
//! and register into the same registry at startup. See `licenses/README.md`
//! for the split, `PROVENANCE.md` for the per-core record, and
//! `docs/vgm-multichip-2026-07/CORES-PLAN.md` for the programme.

pub mod ay8910;
pub mod c140;
pub mod gb_dmg;
pub mod huc6280;
pub mod k051649;
pub mod k054539;
pub mod nes_apu;
pub mod okim;
pub mod rf5c68;
pub mod sn76489;

pub use ay8910::Ay8910;
pub use c140::C140;
pub use gb_dmg::GbDmg;
pub use huc6280::HuC6280;
pub use k051649::K051649;
pub use k054539::K054539;
pub use nes_apu::NesApu;
pub use okim::{Okim6258, Okim6295};
pub use rf5c68::Rf5c68;
pub use sn76489::Sn76489;
