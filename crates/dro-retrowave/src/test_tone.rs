//! A chord the hardware can play without a song loaded.
//!
//! The bring-up test: it exercises the framing, both register banks, and the
//! analogue output in one go, so a silent result narrows the fault down fast.

use crate::{
    device::{Device, Error},
    protocol::Bank,
};

/// Operator offsets for channels 0, 1 and 2 — modulator and carrier.
const OPERATORS: [(u8, u8); 3] = [(0x00, 0x03), (0x01, 0x04), (0x02, 0x05)];

/// A major triad: F-numbers for C4, E4, G4 at block 4.
///
/// `fnum = freq * 2^(20 - block) / 49716`, which at block 4 is `freq * 1.318`.
const CHORD: [u16; 3] = [345, 434, 517];

const BLOCK: u8 = 4;

/// Sets up three channels in `bank` and keys them on.
///
/// The voice is deliberately plain: a single-multiple carrier under a
/// single-multiple modulator, fast attack, moderate sustain.
pub fn key_on_chord(device: &mut Device, bank: Bank) -> Result<(), Error> {
    for (index, &(modulator, carrier)) in OPERATORS.iter().enumerate() {
        for (operator, level) in [(modulator, 0x10), (carrier, 0x00)] {
            // Multiple = 1, no tremolo/vibrato/sustain flags.
            device.write_reg(bank, 0x20 + operator, 0x01)?;
            // Key-scale level 0, total level as given (0 is loudest).
            device.write_reg(bank, 0x40 + operator, level)?;
            // Fast attack, moderate decay.
            device.write_reg(bank, 0x60 + operator, 0xF0)?;
            // Middling sustain, quick release.
            device.write_reg(bank, 0x80 + operator, 0x77)?;
        }

        let channel = index as u8;
        // Both speakers (the CHA/CHB bits OPL3 mode requires), no feedback,
        // frequency modulation rather than additive synthesis.
        device.write_reg(bank, 0xC0 + channel, 0x30)?;

        let fnum = CHORD[index];
        device.write_reg(bank, 0xA0 + channel, (fnum & 0xFF) as u8)?;
        // Key on, block, and the F-number's top two bits.
        let key_on = 0x20 | (BLOCK << 2) | ((fnum >> 8) as u8 & 0x03);
        device.write_reg(bank, 0xB0 + channel, key_on)?;
    }
    Ok(())
}

/// Releases the three channels [`key_on_chord`] started.
pub fn key_off_chord(device: &mut Device, bank: Bank) -> Result<(), Error> {
    for channel in 0..3u8 {
        device.write_reg(bank, 0xB0 + channel, 0x00)?;
    }
    Ok(())
}

/// Turns on OPL3 mode, so bank 1 accepts writes and the stereo bits apply.
pub fn enable_opl3(device: &mut Device) -> Result<(), Error> {
    device.write_reg(Bank::One, 0x05, 0x01)
}
