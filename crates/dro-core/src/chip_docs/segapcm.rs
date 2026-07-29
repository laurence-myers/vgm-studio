//! Sega PCM (315-5218) register documentation.
//!
//! Sources: the VGMRips wiki's "SegaPCM" page and Sega Retro's 315-5218
//! hardware page. The chip is a bare register file and public documentation
//! covers it unevenly, so bytes whose role those pages do not pin down
//! return `None` here rather than a guess.
//!
//! Addressing: sixteen channels, each owning an 8-byte slot -- `addr & 0x78`
//! selects the channel, `addr & 0x07` the byte within the slot -- with bit 7
//! flipping to the control mirror that carries the playback address and the
//! bank/loop/key byte. Docs are therefore keyed on `addr & 0x87` and name
//! the role, never the channel, exactly as the OPL tables do.

use super::{RegisterDoc, bf};

const VOLUME_LEFT: RegisterDoc = RegisterDoc {
    name: "Channel volume (left)",
    fields: &[bf("Volume (left)", 0xFF)],
};
const VOLUME_RIGHT: RegisterDoc = RegisterDoc {
    name: "Channel volume (right)",
    fields: &[bf("Volume (right)", 0xFF)],
};
const LOOP_LOW: RegisterDoc = RegisterDoc {
    name: "Channel loop address (low)",
    fields: &[bf("Loop address (low)", 0xFF)],
};
const LOOP_HIGH: RegisterDoc = RegisterDoc {
    name: "Channel loop address (high)",
    fields: &[bf("Loop address (high)", 0xFF)],
};
const END_HIGH: RegisterDoc = RegisterDoc {
    name: "Channel end address (high byte)",
    fields: &[bf("End address (high byte)", 0xFF)],
};
const DELTA: RegisterDoc = RegisterDoc {
    name: "Channel address delta (pitch)",
    fields: &[bf("Address delta (pitch)", 0xFF)],
};
const CURRENT_LOW: RegisterDoc = RegisterDoc {
    name: "Channel current address (low)",
    fields: &[bf("Current address (low)", 0xFF)],
};
const CURRENT_HIGH: RegisterDoc = RegisterDoc {
    name: "Channel current address (high)",
    fields: &[bf("Current address (high)", 0xFF)],
};
const CONTROL: RegisterDoc = RegisterDoc {
    name: "Channel control: bank / loop / key",
    fields: &[
        bf("Bank select", 0xFC),
        bf("Loop enable", 0x02),
        bf("Key / mute (1 = off)", 0x01),
    ],
};

/// The documentation for a write to `(port, addr)`.
pub(super) const fn doc(port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    // One port, one 256-byte register file; offsets beyond it are
    // undocumented rather than mirrored here.
    if port != 0 || addr > 0xFF {
        return None;
    }
    Some(match addr & 0x87 {
        0x02 => &VOLUME_LEFT,
        0x03 => &VOLUME_RIGHT,
        0x04 => &LOOP_LOW,
        0x05 => &LOOP_HIGH,
        0x06 => &END_HIGH,
        0x07 => &DELTA,
        0x84 => &CURRENT_LOW,
        0x85 => &CURRENT_HIGH,
        0x86 => &CONTROL,
        // Slot bytes 0x00/0x01 and the rest of the control mirror are not
        // pinned down by the public documentation.
        _ => return None,
    })
}

/// The registers a find dropdown offers: none. Every interesting byte is
/// channel-relative, so no single address is worth listing; the dialog's
/// free hex entry covers the register file instead.
pub(super) const NOTABLE: &[(u8, u16, &str)] = &[];
