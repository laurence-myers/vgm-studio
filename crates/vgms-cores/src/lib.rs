// SPDX-License-Identifier: GPL-2.0-or-later
//! The core providers every VGM Studio build shares, registered in one place.
//!
//! The native app and the wasm worklet each build a [`CoreRegistry`] and install
//! it. They must register the *same* providers in the *same* priority order, or
//! a file would play differently depending on where it was opened. That common
//! list used to be copied into both; here it is written once. The single
//! native-only provider, `vgms-retrowave`, is added by the app on top of this.

use vgms_core::ChipKind;
use vgms_synth::CoreRegistry;

/// Registers the providers common to every build into `registry`, in priority
/// order.
///
/// Deliberately does **not** install it: [`vgms_synth::install`] is a
/// process-global one-shot, so a test that wants to inspect or compare the
/// roster needs to build one *without* installing. Both production installers
/// (the app's `install_cores` and the worklet's `install_web_cores`) call this,
/// so the roster this asserts is the roster both ship.
///
/// Order is priority order:
///
/// - **libvgm first** -- the source of truth and the default for every chip it
///   serves. It carries no pure-FM OPL rows, so the built-ins' Nuked-OPL3 stays
///   that family's default.
/// - **Nuked and the GPL die-sims behind it**, as picker options: CQM (Creative's
///   YMF262 clone), the OPN2/OPM Nuked flavours, the LLE die and Nuked-OPLL/PSG.
/// - **Promotions** where the right default is not the registration order. Nuked
///   stays the default over libvgm for four chips (a promotion rather than
///   registration order, because the Nuked OPLL shares its crate with the PSG
///   and the LLE, which must stay *behind* libvgm); and the libvgm Y8950 goes
///   *ahead of the built-in OPL row*, because only its MAME fmopl core has the
///   chip's ADPCM-B half -- on the adapter tier the sample half of every Y8950
///   rip was silent.
pub fn register_common_cores(registry: &mut CoreRegistry) {
    vgms_cores_libvgm::register(registry);
    vgms_cores_nuked::register(registry);
    vgms_cores_gpl::register(registry);
    registry.promote(ChipKind::Ym2612, "ym2612.nuked");
    registry.promote(ChipKind::Ym2151, "ym2151.nuked");
    registry.promote(ChipKind::Ym2413, "ym2413.nuked");
    // Nuked-PSG leads the SN76489 too. libvgm registers first and would
    // otherwise be the default, but the Nuked die trace is the more faithful
    // SN76489 and runs far above realtime, so it is worth the promotion --
    // the same reasoning as the three Nuked FM cores above.
    registry.promote(ChipKind::Sn76489, "sn76489.nuked-psg");
    // The Y8950 goes the other way: the built-in OPL row registered before any
    // provider could, but Nuked-OPL3 has no ADPCM-B unit and libvgm's MAME
    // fmopl does, so the libvgm row leads. The family stays on Nuked -- this
    // moves one chip, not the OPL selector.
    registry.promote(ChipKind::Y8950, "opl3.libvgm-y8950");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_common_roster_registers_libvgm_and_keeps_nuked_ahead_of_it() {
        let mut registry = CoreRegistry::with_builtins();
        // Before: no libvgm provider, so a chip only it serves has no core.
        assert!(
            !registry.has_core(ChipKind::SegaPcm),
            "the built-ins alone should not serve SegaPCM"
        );

        register_common_cores(&mut registry);

        // libvgm registered: a chip only it serves now has a core, proving the
        // provider ran and not merely that the function returned.
        assert!(
            registry.has_core(ChipKind::SegaPcm),
            "libvgm should serve SegaPCM after registration"
        );

        // The deliberate promotions: Nuked stays the default over libvgm for
        // four chips, and the libvgm Y8950 (the one core with the ADPCM-B
        // half) leads the built-in OPL row.
        for (chip, id) in [
            (ChipKind::Ym2612, "ym2612.nuked"),
            (ChipKind::Ym2151, "ym2151.nuked"),
            (ChipKind::Ym2413, "ym2413.nuked"),
            (ChipKind::Sn76489, "sn76489.nuked-psg"),
            (ChipKind::Y8950, "opl3.libvgm-y8950"),
        ] {
            assert_eq!(
                registry.default_for(chip).map(|info| info.id),
                Some(id),
                "{} should default to {id}",
                chip.name()
            );
        }
        // The Y8950 promotion moves one chip, not the family: the other OPL
        // chips keep the built-in Nuked-OPL3 default.
        assert_eq!(
            registry.default_for(ChipKind::Ym3812).map(|info| info.id),
            Some("opl3.nuked"),
            "the YM3812 stays on the OPL family default"
        );
    }

    #[test]
    fn the_roster_is_the_same_however_many_times_it_is_built() {
        // The property the extraction buys: two builds (the app's and the
        // worklet's) that both call this cannot disagree. `install` could not be
        // used to check that -- it installs once per process -- but this can.
        let ids = |()| {
            let mut registry = CoreRegistry::with_builtins();
            register_common_cores(&mut registry);
            registry
                .all()
                .map(|info| (info.chip, info.id))
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(()), ids(()));
    }
}
