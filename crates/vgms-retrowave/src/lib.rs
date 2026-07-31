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

// The pure protocol layer -- everything a Web Serial transport would sit under --
// builds on every target. `device` (the `serialport` open/enumerate) and `player`
// (the OS-thread pump: `std::thread`, `Instant`, `rtrb`) are native-only, and
// `test_tone` takes `&mut Device`, so all three are gated off wasm (wt-9). The web
// build offers no board, so nothing above needs them there.
pub mod chip;
pub mod commands;
pub mod protocol;

#[cfg(not(target_arch = "wasm32"))]
pub mod device;
#[cfg(not(target_arch = "wasm32"))]
pub mod player;
#[cfg(not(target_arch = "wasm32"))]
pub mod test_tone;

pub use chip::SerialOpl3Chip;
#[cfg(not(target_arch = "wasm32"))]
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
pub fn register(registry: &mut vgms_synth::CoreRegistry) {
    for chip in vgms_synth::registry::OPL_CHIPS {
        registry.register(vgms_synth::CoreInfo {
            id: "opl3.retrowave",
            chip,
            label: "RetroWave OPL3 (hardware)",
            authors: "SudoMaker (the board); this project (the protocol)",
            license: "GPL-2.0-or-later",
            upstream: "https://github.com/SudoMaker/RetroWave",
            realtime: true,
            channel_pan: false,
            // An OPL device: muting is register-gated on the shadow chip and
            // reaches the board, so it works.
            channel_mute: true,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Routed,
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use device::{Device, Error, PortInfo, SerialIo, UsbInfo, default_port, enumerate};
pub use protocol::{Bank, CmdBuffer};
