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
//! Implemented from the interface facts in `docs/retrowave-2026-07/PLAN.md` §1,
//! not from the AGPL-3.0 reference implementation: this project excludes
//! GPL-3-only code from every shipped binary, so that document — not the
//! reference sources — is the specification this code follows. See §2 there and
//! `licenses/README.md`.

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
/// [`CoreMaker::Routed`], not a constructor: choosing hardware swaps the whole
/// audio *service* (`SwitchingAudioService`), because a board that mixes its own
/// sound is not a chip the engine can pull samples from.
///
/// Registered after the built-ins so the emulator stays the default, and only
/// from this crate, so the web build (no serial ports) never offers a board it
/// could not reach.
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
            channel_pan: false,
            level: dro_synth::LEVEL_UNITY,
            make: dro_synth::CoreMaker::Routed,
        });
    }
}

pub use device::{Device, Error, PortInfo, SerialIo, UsbInfo, default_port, enumerate};
pub use protocol::{Bank, CmdBuffer};
