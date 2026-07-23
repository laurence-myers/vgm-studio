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
//! The reference implementation is AGPL-3.0 and this workspace is
//! LGPL-2.1-or-later, so that document — not the reference sources — is the
//! specification this code follows. See §2 there.

pub mod chip;
pub mod commands;
pub mod device;
pub mod player;
pub mod protocol;
pub mod test_tone;

pub use chip::SerialOpl3Chip;
pub use player::RetroWaveAudio;

pub use device::{Device, Error, PortInfo, SerialIo, UsbInfo, default_port, enumerate};
pub use protocol::{Bank, CmdBuffer};
