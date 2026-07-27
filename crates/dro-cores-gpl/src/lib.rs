// SPDX-License-Identifier: GPL-2.0-or-later
//! GPL-licensed emulator cores, compiled from pinned upstream submodules.
//!
//! **The third tier of the licence split**, and the reason it exists: some of
//! the most faithful emulation of these chips is GPL-2, the application is
//! GPL-2.0-or-later precisely so it can link that, and neither `dro-synth`
//! (`MIT OR Apache-2.0`) nor `dro-cores-nuked` (LGPL-2.1-or-later) may carry it
//! without becoming something else. So it lives here, in a leaf crate only the
//! application depends on.
//!
//! Everything else is as `dro-cores-nuked`: submodules under
//! `vendor/upstream/` pinned to a commit and compiled **unmodified**, glue in
//! `shim/`, and no upstream struct mirrored on the Rust side. See
//! `crates/dro-synth/PROVENANCE.md` for the per-core record and
//! `licenses/README.md` for the split.

mod ffi;
mod opll;
mod psg;

pub use opll::Ym2413;
pub use psg::Sn76489Nuked;

/// Adds every core here to the registry.
///
/// The provider convention: this crate depends on `dro-synth` for the traits
/// and the registry, and `dro-synth` names no provider. Registration order is
/// priority order, so a core that should be a picker *alternative* rather
/// than the default -- Nuked-PSG, behind the clean-room SN76489 -- relies on
/// the builtins registering first.
pub fn register(registry: &mut dro_synth::CoreRegistry) {
    for chip in opll::CHIPS {
        registry.register(dro_synth::CoreInfo {
            id: opll::CORE_ID,
            chip,
            label: "Nuked-OPLL",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/Nuked-OPLL",
            realtime: true,
            make: dro_synth::CoreMaker::Generic(|| Box::new(Ym2413::new())),
        });
    }
    for chip in psg::CHIPS {
        registry.register(dro_synth::CoreInfo {
            id: psg::CORE_ID,
            chip,
            label: "Nuked-PSG (Sega VDP)",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/Nuked-PSG",
            realtime: true,
            make: dro_synth::CoreMaker::Generic(|| Box::new(Sn76489Nuked::new())),
        });
    }
}

#[cfg(test)]
mod tests {
    use dro_core::vgm::ChipKind;

    /// The point of the crate: a GPL core reaches the registry through the same
    /// provider convention the LGPL and permissive ones use, with its licence
    /// carried along so the Settings picker and the About box can show it.
    #[test]
    fn the_gpl_core_registers_with_its_licence_attached() {
        let mut registry = dro_synth::CoreRegistry::with_builtins();
        super::register(&mut registry);

        let info = registry
            .for_chip(ChipKind::Ym2413)
            .find(|info| info.id == super::opll::CORE_ID)
            .expect("the YM2413 is offered");
        assert_eq!(info.license, "GPL-2.0-or-later");
        assert!(info.upstream.starts_with("https://"));
        assert!(registry.can_build(ChipKind::Ym2413));
    }

    /// Nuked-PSG is a picker *alternative*: the clean-room SN76489 registers
    /// first and stays the default, because the die trace is of one specific
    /// part -- the Sega VDP's -- and the clean-room core is the one that
    /// honours the header's feedback/width fields for everything else.
    #[test]
    fn nuked_psg_is_offered_but_not_the_default() {
        let mut registry = dro_synth::CoreRegistry::with_builtins();
        super::register(&mut registry);

        let default = registry
            .default_for(ChipKind::Sn76489)
            .expect("a default exists");
        assert_ne!(default.id, super::psg::CORE_ID, "the clean room leads");
        assert!(
            registry
                .for_chip(ChipKind::Sn76489)
                .any(|info| info.id == super::psg::CORE_ID),
            "and Nuked-PSG is on the list"
        );
    }
}
