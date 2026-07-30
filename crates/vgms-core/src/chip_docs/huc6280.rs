//! HuC6280 PSG register documentation.
//!
//! Sources: PC Engine hardware notes and Hudson devkit documentation, as
//! mirrored on the Archaic Pixels wiki and the MagicEngine developer docs.
//!
//! Addressing: ten registers, 0x00-0x09. Register 0x00 selects one of the
//! six channels; 0x02-0x07 then act on the selected channel, so their docs
//! name the role, not the channel. Noise (0x07) exists on channels 5-6 only;
//! the LFO pair (0x08-0x09) feeds channel 2's waveform into channel 1's
//! frequency.

use super::{RegisterDoc, bf};

const CHANNEL_SELECT: RegisterDoc = RegisterDoc {
    name: "Channel select",
    fields: &[bf("Channel select", 0x07)],
};
const MAIN_AMPLITUDE: RegisterDoc = RegisterDoc {
    name: "Main amplitude (left / right)",
    fields: &[
        bf("Main amplitude (left)", 0xF0),
        bf("Main amplitude (right)", 0x0F),
    ],
};
const FREQ_LOW: RegisterDoc = RegisterDoc {
    name: "Channel: frequency (low 8 bits)",
    fields: &[bf("Frequency (low 8 bits)", 0xFF)],
};
const FREQ_HIGH: RegisterDoc = RegisterDoc {
    name: "Channel: frequency (high 4 bits)",
    fields: &[bf("Frequency (high 4 bits)", 0x0F)],
};
const CHANNEL_CONTROL: RegisterDoc = RegisterDoc {
    name: "Channel: control",
    fields: &[
        bf("Channel enable", 0x80),
        bf("DDA (direct D/A) mode", 0x40),
        bf("Channel volume", 0x1F),
    ],
};
const CHANNEL_BALANCE: RegisterDoc = RegisterDoc {
    name: "Channel: balance",
    fields: &[bf("Balance (left)", 0xF0), bf("Balance (right)", 0x0F)],
};
const WAVEFORM_DATA: RegisterDoc = RegisterDoc {
    name: "Channel: waveform data",
    fields: &[bf("Waveform sample", 0x1F)],
};
const NOISE_CONTROL: RegisterDoc = RegisterDoc {
    name: "Channel: noise control (channels 5-6)",
    fields: &[bf("Noise enable", 0x80), bf("Noise frequency", 0x1F)],
};
const LFO_FREQUENCY: RegisterDoc = RegisterDoc {
    name: "LFO frequency",
    fields: &[bf("LFO frequency", 0xFF)],
};
const LFO_CONTROL: RegisterDoc = RegisterDoc {
    name: "LFO control",
    fields: &[bf("LFO trigger (reset)", 0x80), bf("LFO mode", 0x03)],
};

/// The documentation for a write to `(port, addr)`.
pub(super) const fn doc(port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    if port != 0 {
        return None;
    }
    Some(match addr {
        0x00 => &CHANNEL_SELECT,
        0x01 => &MAIN_AMPLITUDE,
        0x02 => &FREQ_LOW,
        0x03 => &FREQ_HIGH,
        0x04 => &CHANNEL_CONTROL,
        0x05 => &CHANNEL_BALANCE,
        0x06 => &WAVEFORM_DATA,
        0x07 => &NOISE_CONTROL,
        0x08 => &LFO_FREQUENCY,
        0x09 => &LFO_CONTROL,
        _ => return None,
    })
}

/// The registers a find dropdown offers.
pub(super) const NOTABLE: &[(u8, u16, &str)] = &[
    (0, 0x00, "Channel select"),
    (0, 0x04, "Channel control"),
    (0, 0x07, "Noise control"),
];
