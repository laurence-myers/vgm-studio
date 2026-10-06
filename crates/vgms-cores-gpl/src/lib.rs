// SPDX-License-Identifier: GPL-2.0-or-later
//! GPL-licensed emulator cores, compiled from pinned upstream submodules.
//!
//! The third tier of the licence split: the most faithful emulation of these
//! chips is GPL-2, so it lives in a leaf crate only the application depends on,
//! kept out of `vgms-synth` (`MIT OR Apache-2.0`) and `vgms-cores-nuked`
//! (LGPL-2.1-or-later). Submodules under `vendor/upstream/` are pinned and
//! compiled unmodified, with glue in `shim/` and no upstream struct mirrored.
//! See `crates/vgms-synth/PROVENANCE.md` and `licenses/README.md`.
//!
//! Each core is a feature: `opl2-lite`, `opll`, `psg`, and the die
//! simulations `ym2151-lle`, `ym2203-lle`, `ym2608-lle`, `ym2612-lle`,
//! `ymf276-lle`, `ym3812-lle` and `ymf262-lle` (`lle` for all seven). The
//! default builds every core. A consumer that wants fewer takes
//! `default-features = false` and names them; a build with none is an error.

#[cfg(not(any(
    feature = "opl2-lite",
    feature = "opll",
    feature = "psg",
    feature = "ym2151-lle",
    feature = "ym2203-lle",
    feature = "ym2608-lle",
    feature = "ym2612-lle",
    feature = "ymf276-lle",
    feature = "ym3812-lle",
    feature = "ymf262-lle",
)))]
compile_error!(
    "vgms-cores-gpl builds no core: enable at least one core feature (opl2-lite, opll, psg, ym2151-lle, \
     ym2203-lle, ym2608-lle, ym2612-lle, ymf276-lle, ym3812-lle, ymf262-lle, or lle for all the die \
     simulations), or keep the default features"
);

mod ffi;
#[cfg(feature = "ym3812-lle")]
mod lle_opl2;
#[cfg(feature = "ymf262-lle")]
mod lle_opl3;
#[cfg(feature = "ym2151-lle")]
mod lle_opm;
#[cfg(feature = "ym2203-lle")]
mod lle_opn;
#[cfg(feature = "ym2612-lle")]
mod lle_opn2;
#[cfg(feature = "ymf276-lle")]
mod lle_opn2l;
#[cfg(feature = "ym2608-lle")]
mod lle_opna;
#[cfg(feature = "opl2-lite")]
mod opl2_lite;
#[cfg(feature = "opll")]
mod opll;
#[cfg(feature = "psg")]
mod psg;

#[cfg(feature = "ym3812-lle")]
pub use lle_opl2::Ym3812Lle;
#[cfg(feature = "ymf262-lle")]
pub use lle_opl3::Ymf262Lle;
#[cfg(feature = "ym2151-lle")]
pub use lle_opm::Ym2151Lle;
#[cfg(feature = "ym2203-lle")]
pub use lle_opn::Ym2203Lle;
#[cfg(feature = "ym2612-lle")]
pub use lle_opn2::Ym2612Lle;
#[cfg(feature = "ymf276-lle")]
pub use lle_opn2l::Ymf276Lle;
#[cfg(feature = "ym2608-lle")]
pub use lle_opna::Ym2608Lle;
#[cfg(feature = "opl2-lite")]
pub use opl2_lite::Opl2Lite;
#[cfg(feature = "opll")]
pub use opll::Ym2413;
#[cfg(feature = "psg")]
pub use psg::Sn76489Nuked;

/// Adds every core that the enabled features build to the registry.
///
/// Registration order is priority order, so a core that should be a picker
/// *alternative* rather than the default (Nuked-PSG, behind the clean-room
/// SN76489) relies on the builtins registering first.
pub fn register(registry: &mut vgms_synth::CoreRegistry) {
    #[cfg(feature = "opll")]
    register_opll(registry);
    #[cfg(feature = "psg")]
    register_psg(registry);
    #[cfg(any(
        feature = "ym2151-lle",
        feature = "ym2612-lle",
        feature = "ym2608-lle",
        feature = "ym2203-lle",
        feature = "ymf276-lle",
    ))]
    register_lle_non_opl(registry);
    // Realtime before the die tier, so the OPL2 picker reads fast-to-slow.
    #[cfg(feature = "opl2-lite")]
    register_opl2_lite(registry);
    #[cfg(any(feature = "ym3812-lle", feature = "ymf262-lle"))]
    register_lle_opl(registry);
}

#[cfg(feature = "opll")]
fn register_opll(registry: &mut vgms_synth::CoreRegistry) {
    for chip in opll::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: opll::CORE_ID,
            chip,
            label: "Nuked-OPLL",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/Nuked-OPLL",
            tier: vgms_synth::CoreTier::Cycle,
            exact: true,
            realtime: true,
            channel_pan: false,
            // Muted in the binding's own render gate, cycle by cycle -- the
            // same idea as libvgm's copy of this core.
            channel_mute: true,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym2413::new())),
        });
    }
}

#[cfg(feature = "psg")]
fn register_psg(registry: &mut vgms_synth::CoreRegistry) {
    for chip in psg::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: psg::CORE_ID,
            chip,
            label: "Nuked-PSG (Sega VDP)",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/Nuked-PSG",
            // The die trace of the one part inside the Sega VDPs -- a flavour
            // of the SN76489, not the discrete chip itself.
            tier: vgms_synth::CoreTier::Cycle,
            exact: false,
            realtime: true,
            channel_pan: false,
            // Generic ChipCores with no channel-mute impl (the trait no-op).
            channel_mute: false,
            // The chip's default row, so the 2026-08-12 sweep's SN76489
            // measurement is THIS core's: lvl 0.247 against the reference
            // (n=12; the noise/HF band keeps corr at 0.36, but the RMS ratio
            // agrees with the staging derivation within 1.2%). 256/0.247:
            level: 1036,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Sn76489Nuked::new())),
        });
    }
}

/// The die simulations of the chips outside the OPL family.
#[cfg(any(
    feature = "ym2151-lle",
    feature = "ym2612-lle",
    feature = "ym2608-lle",
    feature = "ym2203-lle",
    feature = "ymf276-lle",
))]
fn register_lle_non_opl(registry: &mut vgms_synth::CoreRegistry) {
    #[cfg(feature = "ym2151-lle")]
    for chip in lle_opm::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opm::CORE_ID,
            chip,
            label: "YM2151-LLE",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YM2151-LLE",
            tier: vgms_synth::CoreTier::DieSim,
            exact: true,
            realtime: false,
            channel_pan: false,
            // Generic ChipCores with no channel-mute impl (the trait no-op).
            channel_mute: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym2151Lle::new())),
        });
    }
    #[cfg(feature = "ym2612-lle")]
    for chip in lle_opn2::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opn2::CORE_ID,
            chip,
            label: "YM2612-LLE",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YM2608-LLE",
            tier: vgms_synth::CoreTier::DieSim,
            exact: true,
            realtime: false,
            channel_pan: false,
            // Generic ChipCores with no channel-mute impl (the trait no-op).
            channel_mute: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym2612Lle::new())),
        });
    }
    #[cfg(feature = "ym2608-lle")]
    for chip in lle_opna::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opna::CORE_ID,
            chip,
            label: "YM2608-LLE",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YM2608-LLE",
            tier: vgms_synth::CoreTier::DieSim,
            exact: true,
            realtime: false,
            channel_pan: false,
            // Generic ChipCores with no channel-mute impl (the trait no-op).
            channel_mute: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym2608Lle::new())),
        });
    }
    #[cfg(feature = "ym2203-lle")]
    for chip in lle_opn::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opn::CORE_ID,
            chip,
            label: "YM2203-LLE",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YM2203-LLE",
            tier: vgms_synth::CoreTier::DieSim,
            exact: true,
            realtime: false,
            channel_pan: false,
            // Generic ChipCores with no channel-mute impl (the trait no-op).
            channel_mute: false,
            // The die agrees with libvgm's MAME core on scale (measured 1.01
            // against it, n=12) -- and that shared scale is half the reference
            // player's, found when the libvgm row was calibrated to lvl 0.508
            // (see `make_ym2203` in vgms-cores-libvgm). The same correction
            // keeps the core swap level-neutral: 256 / (1.01 * 0.508) = 499.
            level: 499,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym2203Lle::new())),
        });
    }
    #[cfg(feature = "ymf276-lle")]
    for chip in lle_opn2l::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opn2l::CORE_ID,
            chip,
            label: "YMF276-LLE",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YMF276-LLE",
            // The 276 die standing in for a YM2612/YM3438 -- the clean serial
            // sibling, not the chip the song clocks.
            tier: vgms_synth::CoreTier::DieSim,
            exact: false,
            realtime: false,
            channel_pan: false,
            // Generic ChipCores with no channel-mute impl (the trait no-op).
            channel_mute: false,
            // The 276's serial DAC comes out ~2.3x hot against the YM2612
            // default: measured lvl 2.29 (n=12) over single-chip OPN2 corpus
            // files, so 256/2.29 = 112 pulls its median to the reference. The
            // per-file ratio scatters (1.49x, [2.03..3.02]) -- this is a
            // stand-in die, not the exact 2612 -- so this fixes the gross
            // loudness, not the last few percent. (The exact YM2612-LLE die
            // needs no correction: it measured 1.00.)
            level: 112,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ymf276Lle::new())),
        });
    }
}

#[cfg(feature = "opl2-lite")]
fn register_opl2_lite(registry: &mut vgms_synth::CoreRegistry) {
    for chip in opl2_lite::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: opl2_lite::CORE_ID,
            chip,
            label: "Nuked-OPL2-Lite (OPL2)",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/Nuked-OPL2-Lite",
            tier: vgms_synth::CoreTier::Behavioural,
            // A model of the YM3812 itself; near for its register-compatible
            // kin, exact only for the 3812.
            exact: chip == vgms_core::vgm::ChipKind::Ym3812,
            realtime: true,
            // A real OPL2 has no stereo-ext registers to pan with, and this
            // models a real OPL2.
            channel_pan: false,
            // `false` engages the OPL write gate, as on every OPL row.
            channel_mute: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Opl(|rate| Box::new(Opl2Lite::new(rate))),
        });
    }
}

/// The OPL2 and OPL3 dies, after the OPL2-Lite.
#[cfg(any(feature = "ym3812-lle", feature = "ymf262-lle"))]
fn register_lle_opl(registry: &mut vgms_synth::CoreRegistry) {
    #[cfg(feature = "ym3812-lle")]
    for chip in lle_opl2::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opl2::CORE_ID,
            chip,
            label: "YM3812-LLE",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YM3812-LLE",
            tier: vgms_synth::CoreTier::DieSim,
            exact: chip == vgms_core::vgm::ChipKind::Ym3812,
            realtime: false,
            // A real OPL2 die has no stereo-ext registers to pan with.
            channel_pan: false,
            // `false` engages the OPL write gate: `CoreInfo::build` wraps this
            // row in a `GatedCore`, so per-channel muting works exactly as it
            // does on the other OPL cores.
            channel_mute: false,
            // The OPL2 die's YM3012 float DAC comes out twice as hot as
            // Nuked-OPL3: measured lvl 2.000 (n=12, tight [1.94..2.04]) over
            // single-chip OPL2 corpus files, so halve it back to the reference
            // balance. (The OPL3 die, YMF262-LLE, needs no such correction --
            // its YAC512 DAC already matches, median lvl ~1.0.)
            level: vgms_synth::LEVEL_UNITY / 2,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(Ym3812Lle::new())),
        });
    }
    #[cfg(feature = "ymf262-lle")]
    for chip in lle_opl3::CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: lle_opl3::CORE_ID,
            chip,
            label: "YMF262-LLE",
            authors: "Nuke.YKT",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/nukeykt/YMF262-LLE",
            tier: vgms_synth::CoreTier::DieSim,
            exact: chip == vgms_core::vgm::ChipKind::Ymf262,
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
    #[cfg(feature = "opll")]
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
    #[cfg(feature = "ym3812-lle")]
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
    #[cfg(feature = "ymf262-lle")]
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

    /// The 2026-08 additions: the YM2203 and YMF276 dies as picker
    /// alternatives behind their chips' defaults, and Nuked-OPL2-Lite as the
    /// OPL2 generation's *realtime* authenticity option -- listed ahead of
    /// the die tier so its picker reads fast-to-slow.
    #[cfg(all(
        feature = "ym2203-lle",
        feature = "ymf276-lle",
        feature = "ym3812-lle",
        feature = "opl2-lite"
    ))]
    #[test]
    fn the_opn_dies_and_the_opl2_lite_are_offered() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        super::register(&mut registry);

        assert!(
            registry
                .for_chip(ChipKind::Ym2203)
                .any(|info| info.id == super::lle_opn::CORE_ID),
            "the YM2203 die is on the list"
        );
        assert!(
            registry
                .for_chip(ChipKind::Ym2612)
                .any(|info| info.id == super::lle_opn2l::CORE_ID),
            "the YMF276 die is on the list"
        );

        let lite = registry
            .for_chip(ChipKind::Ym3812)
            .find(|info| info.id == super::opl2_lite::CORE_ID)
            .expect("Nuked-OPL2-Lite is offered for the OPL2 generation");
        assert!(lite.realtime, "the Lite is the realtime OPL2 option");
        let order: Vec<&str> = registry
            .for_chip(ChipKind::Ym3812)
            .map(|info| info.id)
            .collect();
        let lite_at = order
            .iter()
            .position(|&id| id == super::opl2_lite::CORE_ID)
            .expect("listed");
        let die_at = order
            .iter()
            .position(|&id| id == super::lle_opl2::CORE_ID)
            .expect("listed");
        assert!(lite_at < die_at, "realtime before the die tier: {order:?}");
        assert!(
            !registry
                .for_chip(ChipKind::Ymf262)
                .any(|info| info.id == super::opl2_lite::CORE_ID),
            "an OPL3 song cannot play through an OPL2 model"
        );
    }

    /// Nuked-OPL2-Lite, alone or with the others, serves the OPL2 generation
    /// as an alternative behind the built-in Nuked-OPL3, and not the YMF262.
    #[cfg(feature = "opl2-lite")]
    #[test]
    fn the_opl2_lite_is_offered_for_the_opl2_generation() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        super::register(&mut registry);

        for chip in [ChipKind::Ym3812, ChipKind::Ym3526, ChipKind::Y8950] {
            let lite = registry
                .for_chip(chip)
                .find(|info| info.id == super::opl2_lite::CORE_ID)
                .unwrap_or_else(|| panic!("{} should offer Nuked-OPL2-Lite", chip.name()));
            assert_eq!(lite.license, "GPL-2.0-or-later");
            assert_ne!(
                registry.default_for(chip).map(|info| info.id),
                Some(super::opl2_lite::CORE_ID),
                "{} keeps the built-in default",
                chip.name()
            );
        }
        assert!(
            !registry
                .for_chip(ChipKind::Ymf262)
                .any(|info| info.id == super::opl2_lite::CORE_ID),
            "an OPL3 song cannot play through an OPL2 model"
        );
    }

    /// Each core the features build reaches the registry for its chip, with
    /// its licence -- whatever the set of features.
    #[test]
    // Each push is behind its core's feature, which a `vec![]` cannot say.
    #[allow(clippy::vec_init_then_push)]
    fn every_enabled_core_registers() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        super::register(&mut registry);

        let mut expected: Vec<(&str, ChipKind)> = Vec::new();
        #[cfg(feature = "opl2-lite")]
        expected.push((super::opl2_lite::CORE_ID, ChipKind::Ym3812));
        #[cfg(feature = "opll")]
        expected.push((super::opll::CORE_ID, ChipKind::Ym2413));
        #[cfg(feature = "psg")]
        expected.push((super::psg::CORE_ID, ChipKind::Sn76489));
        #[cfg(feature = "ym2151-lle")]
        expected.push((super::lle_opm::CORE_ID, ChipKind::Ym2151));
        #[cfg(feature = "ym2203-lle")]
        expected.push((super::lle_opn::CORE_ID, ChipKind::Ym2203));
        #[cfg(feature = "ym2608-lle")]
        expected.push((super::lle_opna::CORE_ID, ChipKind::Ym2608));
        #[cfg(feature = "ym2612-lle")]
        expected.push((super::lle_opn2::CORE_ID, ChipKind::Ym2612));
        #[cfg(feature = "ymf276-lle")]
        expected.push((super::lle_opn2l::CORE_ID, ChipKind::Ym2612));
        #[cfg(feature = "ym3812-lle")]
        expected.push((super::lle_opl2::CORE_ID, ChipKind::Ym3812));
        #[cfg(feature = "ymf262-lle")]
        expected.push((super::lle_opl3::CORE_ID, ChipKind::Ymf262));

        assert!(!expected.is_empty(), "a build has at least one core");
        for (id, chip) in expected {
            let info = registry
                .for_chip(chip)
                .find(|info| info.id == id)
                .unwrap_or_else(|| panic!("{id} is not offered for the {}", chip.name()));
            assert_eq!(info.license, "GPL-2.0-or-later", "{id}");
        }
    }

    /// Nuked-PSG is a picker *alternative*: in the app, libvgm registers first
    /// and takes the SN76489's default; the die trace is of one specific part
    /// (the Sega VDP's) and stays a flavour beside it. This crate asserts only
    /// its own half of that: the row is on the list.
    #[cfg(feature = "psg")]
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
