// SPDX-License-Identifier: GPL-2.0-or-later
//! Everything behind the `drotrim` executable: its `play`, `render`, `split` and
//! `optimize` subcommands, and the platform services the GUI runs on.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use vgms_core::io::read_song;
use vgms_core::{ChipKind, Song, VgmFile};

pub mod cli;
pub mod config;
pub mod corpus;
pub mod pack_zip;
pub mod parity;
pub mod services;

pub use config::load_config;
pub use pack_zip::{PackZipOutput, build_pack_zip};

/// Builds the core registry this binary offers and installs it process-wide.
///
/// **Call this first, before anything reads a core.** The registry answers "can
/// this file be played", "what does Settings list" and "who is credited in the
/// About box"; before installation only the built-in cores exist. Both the GUI
/// and the subcommands go through here.
///
/// Registration order is priority order, so the built-ins come first and the
/// emulator stays OPL's default (a first run must not go hunting for a serial
/// port). `vgms-retrowave` is native-only, which is why this lives in the app
/// rather than `vgms-synth`: the web build never calls it.
pub fn install_cores() {
    let mut registry = vgms_synth::CoreRegistry::with_builtins();
    // libvgm first: the source of truth and the default for every chip it
    // serves. It carries no OPL rows, so the built-ins' Nuked-OPL3 keeps that
    // family's default untouched.
    vgms_cores_libvgm::register(&mut registry);
    // Behind it, the Nuked integrations stay as picker options: CQM is
    // Creative's clone of a YMF262 beside the faithful one, OPN2/OPM flavours
    // behind libvgm's rows.
    vgms_cores_nuked::register(&mut registry);
    // The LLE die and Nuked-OPLL/PSG stay as options.
    vgms_cores_gpl::register(&mut registry);
    vgms_retrowave::register(&mut registry);
    // Three exceptions: Nuked stays the default for these chips, libvgm the
    // picker alternative. Promotion rather than registration order because the
    // OPLL shares a crate with Nuked-PSG and the LLE, which must stay behind
    // libvgm.
    registry.promote(ChipKind::Ym2612, "ym2612.nuked");
    registry.promote(ChipKind::Ym2151, "ym2151.nuked");
    registry.promote(ChipKind::Ym2413, "ym2413.nuked");
    if vgms_synth::install(registry).is_err() {
        // Only reachable if startup ran twice in one process. The installed
        // registry is already correct, so this is a note, not a failure.
        log::debug!("the core registry was already installed");
    }
}

/// Reads and parses the song at `path`, naming it after the file (falling back to
/// `input.dro`) so format detection follows the file's extension. Every
/// subcommand opens its one input exactly this way.
///
/// # Errors
/// If the file cannot be read, or is not a song `vgms_core` can parse.
pub fn read_song_from_path(path: &Path) -> Result<Song> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("input.dro");
    Ok(read_song(name, &bytes)?)
}

/// A song a subcommand opened: the OPL stream when there is one (a DRO, or an
/// OPL VGM through its projection), else the whole VGM.
///
/// The two arms go to different engines, exactly as the GUI's
/// [`vgms_synth::AudioSource`] splits them; this is that split made at the
/// reading step, where the CLI can still print format-appropriate detail.
#[derive(Debug)]
pub enum LoadedSong {
    Opl(Song),
    Vgm(Box<VgmFile>),
}

impl LoadedSong {
    /// Hands the song to an audio backend.
    #[must_use]
    pub fn audio_source(self) -> vgms_synth::AudioSource {
        match self {
            Self::Opl(song) => vgms_synth::AudioSource::Opl(Arc::new(song)),
            Self::Vgm(file) => vgms_synth::AudioSource::Vgm(Arc::new(*file)),
        }
    }

    /// The song's length, for progress lines.
    #[must_use]
    pub fn total_ms(&self) -> u32 {
        match self {
            Self::Opl(song) => song.total_delay_ms(),
            Self::Vgm(file) => file.total_ms(),
        }
    }

    /// The banner a subcommand prints on opening, mirroring
    /// [`Song::pretty_string`] for the generic arm.
    #[must_use]
    pub fn pretty_string(&self) -> String {
        match self {
            Self::Opl(song) => song.pretty_string(),
            Self::Vgm(file) => format!(
                "Song: {}\nFormat: VGM v{}\nChips: {}\nLength (ms): {}",
                file.name,
                file.header.version_string(),
                file.chip_list(),
                file.total_ms(),
            ),
        }
    }

    /// The chips the file clocks, deduplicated -- what
    /// [`vgms_synth::playability`] wants to hear about.
    #[must_use]
    pub fn chips(&self) -> Vec<ChipKind> {
        match self {
            Self::Opl(_) => Vec::new(),
            Self::Vgm(file) => {
                let mut kinds: Vec<ChipKind> =
                    file.header.chips().iter().map(|chip| chip.kind).collect();
                kinds.dedup();
                kinds
            }
        }
    }
}

/// Names the chips that would render silence for want of a core, erroring when
/// that is all of them.
///
/// `all_silent` finishes the refusal: "no chip in this file has a core (...),
/// so {all_silent}". A partial gap is a warning line instead -- the song still
/// plays what it can, exactly as the GUI's transport does.
///
/// # Errors
/// If no chip in `chips` has a core.
pub fn warn_missing_cores(chips: &[ChipKind], all_silent: &str) -> Result<()> {
    match vgms_synth::playability(chips) {
        vgms_synth::Playability::None => anyhow::bail!(
            "no chip in this file has a core ({}), so {all_silent}",
            chip_names(chips)
        ),
        vgms_synth::Playability::Partial(missing) => {
            println!(
                "No core for {} -- those chips will be silent.",
                chip_names(&missing)
            );
            Ok(())
        }
        vgms_synth::Playability::Full => Ok(()),
    }
}

fn chip_names(chips: &[ChipKind]) -> String {
    chips
        .iter()
        .map(|kind| kind.name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reads the song at `path` whatever its chips, keeping the OPL arm exactly
/// what [`read_song_from_path`] produces.
///
/// A VGM goes through the multichip reader; when its every command projects to
/// OPL it comes back as a [`Song`], instruction-for-instruction what the OPL
/// reader made of it (the projection corpus pins that parity), so the OPL
/// subcommand paths behave as they always have. Anything else comes back whole.
///
/// # Errors
/// If the file cannot be read, or is not a song `vgms_core` can parse.
pub fn read_any_song_from_path(path: &Path) -> Result<LoadedSong> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("input.dro");
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".vgm") || lower.ends_with(".vgz") {
        let file = vgms_core::vgm::file::read(name, &bytes)?;
        return Ok(match file.to_song() {
            Some(song) => LoadedSong::Opl(song),
            None => LoadedSong::Vgm(Box::new(file)),
        });
    }
    Ok(LoadedSong::Opl(read_song(name, &bytes)?))
}

#[cfg(test)]
mod loaded_song_tests {
    use super::*;

    const OPL_VGM: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

    /// A distinct temp path per test, namespaced by the process so parallel
    /// runs of the binary cannot collide.
    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("drotrim-load-{}-{name}", std::process::id()))
    }

    fn put(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// A minimal Mega Drive rip -- SN76489 and YM2612 clocked, commands the
    /// OPL reader cannot size. Offsets are the VGM spec's.
    fn mega_drive_bytes() -> Vec<u8> {
        let mut out = vec![0u8; 0x80];
        out[..4].copy_from_slice(b"Vgm ");
        put(&mut out, 0x08, 0x151); // version
        put(&mut out, 0x34, 0x80 - 0x34); // data offset
        put(&mut out, 0x0C, 3_579_545); // SN76489 clock
        put(&mut out, 0x2C, 7_670_454); // YM2612 clock
        put(&mut out, 0x18, 10_000); // total samples
        out.extend_from_slice(&[
            0x52, 0x28, 0xF0, // YM2612 key on
            0x50, 0x9F, // SN76489 volume
            0x61, 0x10, 0x27, // delay 10000
            0x66, // end of data
        ]);
        let eof = (out.len() - 4) as u32;
        put(&mut out, 0x04, eof);
        out
    }

    #[test]
    fn a_multichip_vgm_loads_whole() {
        let path = temp_path("md.vgm");
        std::fs::write(&path, mega_drive_bytes()).unwrap();
        let loaded = read_any_song_from_path(&path).unwrap();
        let LoadedSong::Vgm(file) = &loaded else {
            panic!("a Mega Drive rip is not an OPL song: {loaded:?}");
        };
        assert_eq!(file.chip_list(), "SN76489, YM2612");
        assert_eq!(
            loaded.chips(),
            vec![ChipKind::Sn76489, ChipKind::Ym2612],
            "the chips playability is asked about"
        );
        assert_eq!(loaded.total_ms(), 227);
        assert!(loaded.pretty_string().contains("SN76489, YM2612"));
        std::fs::remove_file(&path).ok();
    }

    /// The OPL arm must be what `read_song_from_path` always produced -- the
    /// projection corpus pins the deep parity; this pins the routing.
    #[test]
    fn an_opl_vgm_still_loads_as_the_song_it_always_was() {
        let path = temp_path("opl.vgm");
        std::fs::write(&path, OPL_VGM).unwrap();
        let old = read_song_from_path(&path).unwrap();
        let LoadedSong::Opl(new) = read_any_song_from_path(&path).unwrap() else {
            panic!("an OPL VGM should load through its projection");
        };
        assert_eq!(new.opl_type, old.opl_type);
        assert_eq!(new.len(), old.len());
        for index in 0..old.len() {
            assert_eq!(
                new.instruction(index),
                old.instruction(index),
                "row {index}"
            );
        }
        std::fs::remove_file(&path).ok();
    }
}

#[cfg(test)]
mod core_registry_tests {
    use vgms_core::vgm::ChipKind;

    /// `vgms-ui` declares stand-in provider entries for its dialog tests, since
    /// it compiles to wasm and can link neither the native-only board crate nor
    /// the C-toolchain one. Those declarations drifting from these would mean
    /// the snapshot documents a picker the app does not actually show -- the
    /// exact failure a snapshot exists to prevent, made invisible.
    #[test]
    fn the_test_registry_matches_the_apps() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        vgms_cores_nuked::register(&mut registry);
        vgms_retrowave::register(&mut registry);

        for (id, label, license) in [
            ("opl3.cqm", "Nuked-CQM (Creative CQM)", "LGPL-2.1-or-later"),
            (
                "opl3.retrowave",
                "RetroWave OPL3 (hardware)",
                "GPL-2.0-or-later",
            ),
        ] {
            let info = registry
                .for_chip(ChipKind::Ymf262)
                .find(|info| info.id == id)
                .unwrap_or_else(|| panic!("{id} is offered for OPL"));
            assert_eq!(info.label, label);
            assert_eq!(info.license, license);
        }

        // Both are alternatives, not defaults: a first run must not go hunting
        // for a serial port, and the faithful YMF262 stays ahead of Creative's
        // clone of one.
        assert_eq!(
            registry.default_for(ChipKind::Ymf262).map(|info| info.id),
            Some("opl3.nuked"),
            "Nuked-OPL3 stays the default OPL core"
        );
    }

    /// The registry as the whole app sees it, checked for the things that are
    /// invisible one core at a time.
    ///
    /// Ids must be unique across the registry, because config stores one per
    /// slot and the About box lists them side by side; each must be prefixed by
    /// its slot, because that is what makes composing a config value with a
    /// slot unambiguous; and each must carry the authors and licence its notice
    /// requires. A core added without one of those looks fine in isolation and
    /// is wrong in the aggregate.
    #[test]
    fn every_registered_core_is_complete_and_uniquely_named() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        vgms_cores_libvgm::register(&mut registry);
        vgms_cores_nuked::register(&mut registry);
        vgms_cores_gpl::register(&mut registry);
        vgms_retrowave::register(&mut registry);

        let mut seen: std::collections::HashMap<&str, ChipKind> = std::collections::HashMap::new();
        for info in registry.all() {
            let prefix = format!("{}.", vgms_synth::registry::slot_slug(info.chip));
            assert!(
                info.id.starts_with(&prefix),
                "{} should start with {prefix}",
                info.id
            );
            assert!(!info.label.is_empty(), "{}: no label", info.id);
            assert!(!info.authors.is_empty(), "{}: no authors", info.id);
            assert!(!info.license.is_empty(), "{}: no licence", info.id);
            // One id may serve several chips (the OPL family shares a core), so
            // the check is that an id never means two *different* things.
            if let Some(&first) = seen.get(info.id) {
                assert_eq!(
                    vgms_synth::registry::slot_slug(first),
                    vgms_synth::registry::slot_slug(info.chip),
                    "{} names two different slots",
                    info.id
                );
            } else {
                seen.insert(info.id, info.chip);
            }
        }
        assert!(seen.len() > 10, "only {} distinct cores?", seen.len());
    }

    /// Every chip the spec defines is either playable or knowably not -- there
    /// is no third state. The tally is what the Settings dialog shows, so a
    /// chip falling out of both halves would be silently unmentioned.
    #[test]
    fn every_chip_in_the_table_is_accounted_for() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        vgms_cores_libvgm::register(&mut registry);
        vgms_cores_nuked::register(&mut registry);
        vgms_cores_gpl::register(&mut registry);
        vgms_retrowave::register(&mut registry);

        let (mut cored, mut silent) = (0usize, 0usize);
        for chip in ChipKind::all() {
            if registry.has_core(chip) {
                cored += 1;
            } else {
                silent += 1;
            }
        }
        assert_eq!(
            cored + silent,
            ChipKind::all().count(),
            "a chip is in neither half"
        );
        // The OPL family is four chips behind one core, so this counts higher
        // than the number of *cores*.
        assert!(cored >= 16, "only {cored} chips have a core");
    }

    /// The whole point of the registry reaching playback: a core the user picks
    /// is a core that gets built. `Routed` entries (the board) are the app's to
    /// interpret and correctly build nothing here.
    #[test]
    fn a_picked_opl_core_is_the_one_that_gets_built() {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        vgms_cores_nuked::register(&mut registry);
        vgms_retrowave::register(&mut registry);

        assert!(registry.build_opl(Some("cqm"), 49_716).is_some());
        assert!(registry.build_opl(Some("nuked"), 49_716).is_some());
        assert!(
            registry.build_opl(Some("retrowave"), 49_716).is_none(),
            "the board is a whole audio service, not a chip the engine pulls from"
        );
        // An unknown name falls back to the default, which does build.
        assert!(registry.build_opl(Some("nonesuch"), 49_716).is_some());
    }
}
