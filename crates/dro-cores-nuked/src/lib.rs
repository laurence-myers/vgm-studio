// SPDX-License-Identifier: LGPL-2.1-or-later
//! Nuke.YKT's cores, compiled from pinned upstream submodules.
//!
//! **Why this crate exists at all**: `dro-synth` is `MIT OR Apache-2.0` so it
//! can be reused without copyleft, and these cores are LGPL-2.1-or-later. They
//! are not less welcome for it -- they are the most accurate emulation of these
//! chips there is, and the *application* is GPL precisely so it can link them.
//! They just do not belong in the reusable half. This crate is the other side
//! of that line: a leaf only the app depends on, registering into `dro-synth`'s
//! registry through the provider convention.
//!
//! **Why a submodule and not a port.** These upstreams are alive. A hand-port
//! is a fork that has to be re-done every time Nuke.YKT fixes something, and
//! the fix is exactly what a user of an accuracy core wants. Compiled as they
//! stand, an upgrade is `git -C vendor/upstream/<x> pull`, a pin bump and a
//! corpus re-run. Nothing in `vendor/upstream/` is ever edited; what the build
//! needs and the upstream does not provide lives in `shim/`.
//!
//! See `crates/dro-synth/PROVENANCE.md` for the per-core record and
//! `licenses/README.md` for the split.

mod cqm;
mod ffi;
mod opaque;
mod opm;
mod opn;
mod opn2;

pub use cqm::CqmOpl3;
pub use opm::Ym2151;
pub use opn::{OpnCore, OpnKind};
pub use opn2::Ym2612;

/// Adds every core here to the registry.
///
/// The provider convention: this crate depends on `dro-synth` for the traits
/// and the registry, and `dro-synth` names no provider. Registration order is
/// priority order, so calling this *before* the built-ins would make CQM the
/// default OPL core -- the app calls it after, because Nuked-OPL3 is the
/// faithful YMF262 and CQM is Creative's clone of one. A flavour, not a
/// replacement.
pub fn register(registry: &mut dro_synth::CoreRegistry) {
    for chip in dro_synth::registry::OPL_CHIPS {
        registry.register(dro_synth::CoreInfo {
            id: cqm::CORE_ID,
            chip,
            label: "Nuked-CQM (Creative CQM)",
            authors: "Nuke.YKT",
            license: "LGPL-2.1-or-later",
            upstream: "https://github.com/nukeykt/Nuked-CQM",
            realtime: true,
            make: dro_synth::CoreMaker::Opl(|rate| Box::new(CqmOpl3::new(rate))),
        });
    }
    for chip in opn2::CHIPS {
        registry.register(dro_synth::CoreInfo {
            id: opn2::YM2612_CORE_ID,
            chip,
            label: "Nuked-OPN2 (YM2612 / YM3438)",
            authors: "Nuke.YKT",
            license: "LGPL-2.1-or-later",
            upstream: "https://github.com/nukeykt/Nuked-OPN2",
            realtime: true,
            // A `ChipCore`, not an `OplChip`: this one plays through
            // `VgmEngine`, the generic path with no register policy.
            make: dro_synth::CoreMaker::Generic(|| Box::new(Ym2612::new())),
        });
    }
    for chip in opm::CHIPS {
        registry.register(dro_synth::CoreInfo {
            id: opm::CORE_ID,
            chip,
            label: "Nuked-OPM (YM2151 / YM2164)",
            authors: "Nuke.YKT",
            license: "LGPL-2.1-or-later",
            upstream: "https://github.com/nukeykt/Nuked-OPM",
            realtime: true,
            make: dro_synth::CoreMaker::Generic(|| Box::new(Ym2151::new())),
        });
    }
    opn::register(registry);
}

#[cfg(test)]
mod tests {
    use dro_core::vgm::ChipKind;

    #[test]
    fn cqm_joins_the_opl_row_without_taking_it_over() {
        let mut registry = dro_synth::CoreRegistry::with_builtins();
        super::register(&mut registry);

        let cores: Vec<&str> = registry
            .for_chip(ChipKind::Ymf262)
            .map(|info| info.id)
            .collect();
        assert!(cores.contains(&super::cqm::CORE_ID), "{cores:?}");
        assert_ne!(
            cores.first(),
            Some(&super::cqm::CORE_ID),
            "Nuked-OPL3 stays the default: CQM is Creative's clone of a YMF262, \
             an authenticity flavour rather than a more faithful one"
        );

        // Every OPL chip gets it, since one core plays all four.
        for chip in dro_synth::registry::OPL_CHIPS {
            assert!(
                registry.find(chip, super::cqm::CORE_ID).is_some(),
                "{} is missing CQM",
                chip.name()
            );
        }
    }
}
