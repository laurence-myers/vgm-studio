//! SN76489 (and T6W28) write decoding.
//!
//! Sources: Texas Instruments SN76489AN datasheet; the SMS Power! "SN76489"
//! hardware notes (<https://www.smspower.org/Development/SN76489>).
//!
//! The PSG has no address bus worth the name: a write with bit 7 set latches
//! one of eight registers (two per channel: frequency/control and
//! attenuation) and carries its low nibble; a write with bit 7 clear extends
//! the latched register with six more bits. So this module describes bytes
//! rather than looking addresses up, and the analyser keeps the latch.

/// The description for a latch byte naming register `register` (0..8, from
/// the byte's bits 6-4).
#[must_use]
pub(super) const fn latch_description(register: u8) -> &'static str {
    match register {
        0 => "Tone 1 frequency (latch + low 4 bits)",
        1 => "Tone 1 attenuation",
        2 => "Tone 2 frequency (latch + low 4 bits)",
        3 => "Tone 2 attenuation",
        4 => "Tone 3 frequency (latch + low 4 bits)",
        5 => "Tone 3 attenuation",
        6 => "Noise control (mode / shift rate)",
        _ => "Noise attenuation",
    }
}

/// The description for a data byte extending `register`, or one that arrived
/// before any latch.
#[must_use]
pub(super) const fn data_description(register: Option<u8>) -> &'static str {
    match register {
        Some(0) => "Tone 1 frequency (high 6 bits)",
        Some(1) => "Tone 1 attenuation (data)",
        Some(2) => "Tone 2 frequency (high 6 bits)",
        Some(3) => "Tone 2 attenuation (data)",
        Some(4) => "Tone 3 frequency (high 6 bits)",
        Some(5) => "Tone 3 attenuation (data)",
        Some(6) => "Noise control (data)",
        Some(_) => "Noise attenuation (data)",
        None => "Data byte (no register latched)",
    }
}

/// The Game Gear's stereo extension: one enable bit per channel and side.
pub(super) const GG_STEREO: &str = "Game Gear stereo enables (L/R per channel)";
