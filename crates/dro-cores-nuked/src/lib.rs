// SPDX-License-Identifier: LGPL-2.1-or-later
//! Nuke.YKT's cores, compiled from pinned upstream submodules.
//!
//! These cores are LGPL-2.1-or-later, so they live in a leaf crate only the app
//! depends on rather than in `dro-synth` (`MIT OR Apache-2.0`, reusable without
//! copyleft), registering through the provider convention.
//!
//! Kept as pinned submodules rather than hand-ports so an upgrade is a pin bump
//! and a corpus re-run, not a re-port every time Nuke.YKT fixes something.
//! Nothing in `vendor/upstream/` is edited; the build's extras live in `shim/`.
//!
//! See `crates/dro-synth/PROVENANCE.md` and `licenses/README.md`.

mod cqm;
mod ffi;
mod opaque;
mod opm;
mod opn2;

pub use cqm::CqmOpl3;
pub use opm::Ym2151;
pub use opn2::Ym2612;

/// Adds every core here to the registry.
///
/// Registration order is priority order, so calling this *before* the built-ins
/// would make CQM the default OPL core; the app calls it after, because
/// Nuked-OPL3 is the faithful YMF262 and CQM is Creative's clone -- a flavour,
/// not a replacement.
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
            channel_pan: false,
            level: dro_synth::LEVEL_UNITY,
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
            channel_pan: false,
            level: dro_synth::LEVEL_UNITY,
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
            channel_pan: false,
            level: dro_synth::LEVEL_UNITY,
            make: dro_synth::CoreMaker::Generic(|| Box::new(Ym2151::new())),
        });
    }
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
