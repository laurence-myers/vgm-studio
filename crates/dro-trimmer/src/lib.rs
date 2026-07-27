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

    /// `dro-ui` declares a stand-in RetroWave entry for its dialog tests, since
    /// it compiles to wasm and cannot link this native-only crate. The two
    /// declarations drifting apart would mean the snapshot documents a picker
    /// the app does not actually show -- the exact failure a snapshot exists to
    /// prevent, made invisible.
    #[test]
    fn the_test_registry_matches_the_apps() {
        let mut registry = dro_synth::CoreRegistry::with_builtins();
        dro_retrowave::register(&mut registry);

        let hardware = registry
            .for_chip(ChipKind::Ymf262)
            .find(|info| info.id == "opl3.retrowave")
            .expect("the board is offered for OPL");
        assert_eq!(hardware.label, "RetroWave OPL3 (hardware)");
        assert_eq!(hardware.license, "GPL-2.0-or-later");

        // And it is an alternative, not the default: a first run must not go
        // hunting for a serial port.
        assert_ne!(
            registry.default_for(ChipKind::Ymf262).map(|info| info.id),
            Some("opl3.retrowave"),
            "the emulator stays the default"
        );
    }
}
