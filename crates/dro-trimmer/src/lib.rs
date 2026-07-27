// SPDX-License-Identifier: GPL-2.0-or-later
//! Everything behind the `drotrim` executable: its `play`, `render`, `split` and
//! `optimize` subcommands, and the platform services the GUI runs on.

use std::path::Path;

use anyhow::{Context, Result};
use dro_core::Song;
use dro_core::io::read_song;

pub mod cli;
pub mod config;
pub mod corpus;
pub mod pack_zip;
pub mod services;

pub use config::load_config;
pub use pack_zip::{PackZipOutput, build_pack_zip};

/// Builds the core registry this binary offers and installs it process-wide.
///
/// **Call this first, before anything reads a core.** The registry answers "can
/// this file be played", "what does Settings list" and "who is credited in the
/// About box"; a path that runs before installation silently gets the built-in
/// cores only, which on this target means no RetroWave board and a Settings
/// dialog missing a row. Both the GUI and the subcommands go through here.
///
/// Registration order is priority order, so the built-ins come first and the
/// emulator stays OPL's default -- a first run must not go hunting for a serial
/// port. `dro-retrowave` is native-only, which is precisely why this lives in
/// the app rather than in `dro-synth`: the web build never calls it, so its
/// Settings dialog stops offering hardware it could never reach.
pub fn install_cores() {
    let mut registry = dro_synth::CoreRegistry::with_builtins();
    // After the built-ins: Nuked-OPL3 is the faithful YMF262 and CQM is
    // Creative's clone of one, so CQM is an authenticity flavour beside it
    // rather than a better default.
    dro_cores_nuked::register(&mut registry);
    dro_retrowave::register(&mut registry);
    if dro_synth::install(registry).is_err() {
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
/// If the file cannot be read, or is not a song `dro_core` can parse.
pub fn read_song_from_path(path: &Path) -> Result<Song> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("input.dro");
    Ok(read_song(name, &bytes)?)
}

#[cfg(test)]
mod core_registry_tests {
    use dro_core::vgm::ChipKind;

    /// `dro-ui` declares stand-in provider entries for its dialog tests, since
    /// it compiles to wasm and can link neither the native-only board crate nor
    /// the C-toolchain one. Those declarations drifting from these would mean
    /// the snapshot documents a picker the app does not actually show -- the
    /// exact failure a snapshot exists to prevent, made invisible.
    #[test]
    fn the_test_registry_matches_the_apps() {
        let mut registry = dro_synth::CoreRegistry::with_builtins();
        dro_cores_nuked::register(&mut registry);
        dro_retrowave::register(&mut registry);

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

    /// The whole point of the registry reaching playback: a core the user picks
    /// is a core that gets built. `Routed` entries (the board) are the app's to
    /// interpret and correctly build nothing here.
    #[test]
    fn a_picked_opl_core_is_the_one_that_gets_built() {
        let mut registry = dro_synth::CoreRegistry::with_builtins();
        dro_cores_nuked::register(&mut registry);
        dro_retrowave::register(&mut registry);

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
