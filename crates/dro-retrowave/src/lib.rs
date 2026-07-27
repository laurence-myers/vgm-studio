// SPDX-License-Identifier: GPL-2.0-or-later
//! Playback through RetroWave OPL3 hardware: a real YMF262 on the far side of a
//! USB CDC serial port.
//!
//! The same crate drives both board generations — the original RetroWave OPL3 and
//! the OPL3 Express speak an identical protocol.
//!
//! Native only: the web build has no serial ports. Songs must be OPL-family
//! (every DRO, and VGM files whose chip data is OPL2, dual-OPL2, or OPL3); a
//! single YMF262 cannot voice anything else.
//!
//! Implemented from the interface facts in `docs/retrowave-2026-07/PLAN.md` §1.
//! The reference implementation is AGPL-3.0, which this project excludes from
//! every shipped binary along with all other GPL-3-only code, so that document
//! — not the reference sources — is the specification this code follows. See
//! §2 there, and `licenses/README.md` for why the exclusion outlived the move
//! from LGPL-2.1 to GPL-2.0-or-later: a v3-only obligation would lock out the
//! GPL-2 emulator cores the app exists to link.

pub mod chip;
pub mod commands;
pub mod device;
pub mod player;
pub mod protocol;
pub mod test_tone;

pub use chip::SerialOpl3Chip;
pub use player::RetroWaveAudio;

/// Adds this board to the core registry, so Settings offers it for OPL.
///
/// The provider convention: a crate holding cores depends on `dro-synth` for
/// the registry and exports one of these; `dro-synth` names no provider. Here
/// that direction earns something concrete -- this crate needs serial ports,
/// so the web build never calls this and its Settings dialog stops offering a
/// board it could never reach.
///
/// [`CoreMaker::Routed`], not a constructor: choosing hardware swaps the whole
/// audio *service* (`SwitchingAudioService`), because a board that mixes its
/// own sound is not a chip the engine can pull samples from. The app routes on
/// the id; this entry exists to be listed and chosen.
///
/// Registered after the built-ins on purpose, so the emulator stays the
/// default and a first run does not go looking for a serial port.
pub fn register(registry: &mut dro_synth::CoreRegistry) {
    for chip in dro_synth::registry::OPL_CHIPS {
        registry.register(dro_synth::CoreInfo {
            id: "opl3.retrowave",
            chip,
            label: "RetroWave OPL3 (hardware)",
            authors: "SudoMaker (the board); this project (the protocol)",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/SudoMaker/RetroWave",
            realtime: true,
            make: dro_synth::CoreMaker::Routed,
        });
    }
}

pub use device::{Device, Error, PortInfo, SerialIo, UsbInfo, default_port, enumerate};
pub use protocol::{Bank, CmdBuffer};
