// SPDX-License-Identifier: GPL-2.0-or-later
//! GPL-licensed emulator cores, compiled from pinned upstream submodules.
//!
//! The third tier of the licence split: the most faithful emulation of these
//! chips is GPL-2, so it lives in a leaf crate only the application depends on,
//! kept out of `vgms-synth` (`MIT OR Apache-2.0`) and `vgms-cores-nuked`
//! (LGPL-2.1-or-later). Submodules under `vendor/upstream/` are pinned and
//! compiled unmodified, with glue in `shim/` and no upstream struct mirrored.
//! See `crates/vgms-synth/PROVENANCE.md` and `licenses/README.md`.

mod ffi;
mod lle_opl2;
mod lle_opl3;
mod lle_opm;
mod lle_opn2;
mod lle_opna;
mod opll;
mod psg;

pub use lle_opl2::Ym3812Lle;
pub use lle_opl3::Ymf262Lle;
pub use lle_opm::Ym2151Lle;
pub use lle_opn2::Ym2612Lle;
pub use lle_opna::Ym2608Lle;
pub use opll::Ym2413;
pub use psg::Sn76489Nuked;

/// Adds every core here to the registry.
///
/// Registration order is priority order, so a core that should be a picker
/// *alternative* rather than the default (Nuked-PSG, behind the clean-room
/// SN76489) relies on the builtins registering first.
pub fn register(registry: &mut vgms_synth::CoreRegistry) {
    for chip in opll::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: opll::CORE_ID,
            chip,
            label: "Nuked-OPLL",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/Nuked-OPLL",
            realtime: true,
            channel_pan: false,
            // Muted in the binding's own render gate, cycle by cycle -- the
            // same idea as libvgm's copy of this core.
            channel_mute: true,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym2413::new())),
        });
    }
    for chip in psg::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: psg::CORE_ID,
            chip,
            label: "Nuked-PSG (Sega VDP)",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/Nuked-PSG",
            realtime: true,
            channel_pan: false,
            // Generic ChipCores with no channel-mute impl (the trait no-op).
            channel_mute: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Sn76489Nuked::new())),
        });
    }
    for chip in lle_opm::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opm::CORE_ID,
            chip,
            label: "YM2151-LLE (die sim, below realtime)",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YM2151-LLE",
            realtime: false,
            channel_pan: false,
            // Generic ChipCores with no channel-mute impl (the trait no-op).
            channel_mute: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym2151Lle::new())),
        });
    }
    for chip in lle_opn2::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opn2::CORE_ID,
            chip,
            label: "YM2612-LLE (die sim, below realtime)",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YM2608-LLE",
            realtime: false,
            channel_pan: false,
            // Generic ChipCores with no channel-mute impl (the trait no-op).
            channel_mute: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym2612Lle::new())),
        });
    }
    for chip in lle_opna::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opna::CORE_ID,
            chip,
            label: "YM2608-LLE (die sim, below realtime)",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YM2608-LLE",
            realtime: false,
            channel_pan: false,
            // Generic ChipCores with no channel-mute impl (the trait no-op).
            channel_mute: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym2608Lle::new())),
        });
    }
    for chip in lle_opl2::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opl2::CORE_ID,
            chip,
            label: "YM3812-LLE (die sim, below realtime)",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YM3812-LLE",
            realtime: false,
            // A real OPL2 die has no stereo-ext registers to pan with.
            channel_pan: false,
            // `false` engages the OPL write gate: `CoreInfo::build` wraps this
            // row in a `GatedCore`, so per-channel muting works exactly as it
            // does on the other OPL cores.
            channel_mute: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym3812Lle::new())),
        });
    }
    for chip in lle_opl3::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opl3::CORE_ID,
            chip,
            label: "YMF262-LLE (die sim, below realtime)",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YMF262-LLE",
            realtime: false,
            // The stereo-ext panpots are Nuked-OPL3's extension; the real die
            // has no such registers.
            channel_pan: false,
            // As the OPL2 die: `false` engages the OPL write gate.
            channel_mute: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ymf262Lle::new())),
        });
    }
}

#[cfg(test)]
mod tests {
    use vgms_core::vgm::ChipKind;

    /// The point of the crate: a GPL core reaches the registry through the same
    /// provider convention the LGPL and permissive ones use, with its licence
    /// carried along so the Settings picker and the About box can show it.
    #[test]
    fn the_gpl_core_registers_with_its_licence_attached() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        super::register(&mut registry);

        let info = registry
            .for_chip(ChipKind::Ym2413)
            .find(|info| info.id == super::opll::CORE_ID)
            .expect("the YM2413 is offered");
        assert_eq!(info.license, "GPL-2.0-or-later");
        assert!(info.upstream.starts_with("https://"));
        assert!(registry.can_build(ChipKind::Ym2413));
    }

    /// The OPL2 die is a picker *alternative* in the shared OPL slot: the
    /// built-in Nuked-OPL3 registers first and stays the family default, and
    /// this row is offered for the OPL2-generation chips only -- an OPL3 song
    /// needs the second register bank the die lacks, so the YMF262 must not
    /// list it.
    #[test]
    fn the_opl2_die_is_offered_for_its_generation_only() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        super::register(&mut registry);

        for chip in [ChipKind::Ym3812, ChipKind::Ym3526, ChipKind::Y8950] {
            assert!(
                registry
                    .for_chip(chip)
                    .any(|info| info.id == super::lle_opl2::CORE_ID),
                "{} should offer the OPL2 die",
                chip.name()
            );
            assert_ne!(
                registry.default_for(chip).map(|info| info.id),
                Some(super::lle_opl2::CORE_ID),
                "{} must not default to a below-realtime die",
                chip.name()
            );
        }
        assert!(
            !registry
                .for_chip(ChipKind::Ymf262)
                .any(|info| info.id == super::lle_opl2::CORE_ID),
            "the YMF262 has banked registers the OPL2 die cannot address"
        );
    }

    /// The OPL3 die serves the whole family (an OPL2 song on OPL3 silicon is
    /// the SB16 experience), as an alternative behind the modelled default.
    #[test]
    fn the_opl3_die_is_offered_for_the_whole_family_behind_the_default() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        super::register(&mut registry);

        for chip in [
            ChipKind::Ymf262,
            ChipKind::Ym3812,
            ChipKind::Ym3526,
            ChipKind::Y8950,
        ] {
            assert!(
                registry
                    .for_chip(chip)
                    .any(|info| info.id == super::lle_opl3::CORE_ID),
                "{} should offer the OPL3 die",
                chip.name()
            );
            assert_ne!(
                registry.default_for(chip).map(|info| info.id),
                Some(super::lle_opl3::CORE_ID),
                "{} must not default to a below-realtime die",
                chip.name()
            );
        }
    }

    /// Nuked-PSG is a picker *alternative*: in the app, libvgm registers first
    /// and takes the SN76489's default; the die trace is of one specific part
    /// (the Sega VDP's) and stays a flavour beside it. This crate asserts only
    /// its own half of that: the row is on the list.
    #[test]
    fn nuked_psg_is_offered() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        super::register(&mut registry);

        assert!(
            registry
                .for_chip(ChipKind::Sn76489)
                .any(|info| info.id == super::psg::CORE_ID),
            "Nuked-PSG is on the list"
        );
    }
}
