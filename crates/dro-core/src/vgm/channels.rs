//! What channels each chip has: counts and names, from the chips' own
//! datasheets.
//!
//! This is the canonical channel order for the whole app. A mute mask's bit
//! `i` means entry `i` of [`channels_of`], a pan array's entry `i` likewise,
//! and a split names its files after these entries -- so every consumer agrees
//! about which voice "channel 3" is. Where an emulator numbers its own mute
//! bits differently, its provider crate remaps to this order; the order here
//! never bends to an implementation.
//!
//! Counts are datasheet facts (voice/slot/oscillator counts), cross-checked
//! against the widths libvgm's per-core mute masks accept -- checked, not
//! copied; no emulator tables live here. Chips outside the documented set get
//! honest counts with generic `Ch N` names, upgraded chip by chip as their
//! documentation lands.
//!
//! Two deliberate simplifications, both noted at their rows: the YM2610's
//! table lists six FM channels even though the non-B part only bonds out
//! four (the B variant and every emulator use the six-channel layout), and
//! the YMF278B row covers its 24 wavetable channels only -- the OPL4's FM
//! half is the linked OPL3, which has its own table.

use super::header::ChipKind;

/// One channel of a chip: a display name, and the short form filenames and
/// tight UI use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelInfo {
    pub name: &'static str,
    pub short: &'static str,
}

macro_rules! channels {
    ($( $name:literal / $short:literal ),+ $(,)?) => {
        &[ $( ChannelInfo { name: $name, short: $short } ),+ ]
    };
}

/// Generic numbered channels, sliced per chip for the kinds whose channels
/// have no distinct roles (or no documentation yet).
const GENERIC: &[ChannelInfo] = channels![
    "Ch 1" / "01", "Ch 2" / "02", "Ch 3" / "03", "Ch 4" / "04",
    "Ch 5" / "05", "Ch 6" / "06", "Ch 7" / "07", "Ch 8" / "08",
    "Ch 9" / "09", "Ch 10" / "10", "Ch 11" / "11", "Ch 12" / "12",
    "Ch 13" / "13", "Ch 14" / "14", "Ch 15" / "15", "Ch 16" / "16",
    "Ch 17" / "17", "Ch 18" / "18", "Ch 19" / "19", "Ch 20" / "20",
    "Ch 21" / "21", "Ch 22" / "22", "Ch 23" / "23", "Ch 24" / "24",
    "Ch 25" / "25", "Ch 26" / "26", "Ch 27" / "27", "Ch 28" / "28",
    "Ch 29" / "29", "Ch 30" / "30", "Ch 31" / "31", "Ch 32" / "32",
];

/// The channels of one `kind` chip, in the app's canonical order.
///
/// `variant` is the header clock's bit 31 -- it changes the answer only where
/// the variant really has different voices (the NES APU's FDS expansion).
/// Never empty, and never longer than 32, so a `u32` mute mask always covers
/// a whole chip.
#[must_use]
pub fn channels_of(kind: ChipKind, variant: bool) -> &'static [ChannelInfo] {
    use ChipKind as K;
    match kind {
        K::Sn76489 => channels![
            "Tone 1" / "T1", "Tone 2" / "T2", "Tone 3" / "T3", "Noise" / "N",
        ],
        K::Ym2413 => channels![
            "FM 1" / "01", "FM 2" / "02", "FM 3" / "03", "FM 4" / "04",
            "FM 5" / "05", "FM 6" / "06", "FM 7" / "07", "FM 8" / "08",
            "FM 9" / "09",
            "Bass Drum" / "BD", "Snare Drum" / "SD", "Tom-Tom" / "TT",
            "Cymbal" / "CY", "Hi-Hat" / "HH",
        ],
        K::Ym2612 => channels![
            "FM 1" / "01", "FM 2" / "02", "FM 3" / "03", "FM 4" / "04",
            "FM 5" / "05", "FM 6" / "06", "DAC" / "DA",
        ],
        K::Ym2151 => channels![
            "FM 1" / "01", "FM 2" / "02", "FM 3" / "03", "FM 4" / "04",
            "FM 5" / "05", "FM 6" / "06", "FM 7" / "07", "FM 8" / "08",
        ],
        K::SegaPcm => &GENERIC[..16],
        K::Rf5c68 | K::Rf5c164 => &GENERIC[..8],
        K::Ym2203 => channels![
            "FM 1" / "01", "FM 2" / "02", "FM 3" / "03",
            "SSG A" / "A", "SSG B" / "B", "SSG C" / "C",
        ],
        // The YM2608's ADPCM-A part plays six fixed rhythm samples.
        K::Ym2608 => channels![
            "FM 1" / "01", "FM 2" / "02", "FM 3" / "03", "FM 4" / "04",
            "FM 5" / "05", "FM 6" / "06",
            "Bass Drum" / "BD", "Snare Drum" / "SD", "Top Cymbal" / "TC",
            "Hi-Hat" / "HH", "Tom" / "TM", "Rim Shot" / "RS",
            "ADPCM" / "AD",
            "SSG A" / "A", "SSG B" / "B", "SSG C" / "C",
        ],
        // Six FM channels as on the B variant; the plain YM2610 bonds out
        // only four of them (1, 2, 4, 5), but the layout is the same.
        K::Ym2610 => channels![
            "FM 1" / "01", "FM 2" / "02", "FM 3" / "03", "FM 4" / "04",
            "FM 5" / "05", "FM 6" / "06",
            "ADPCM-A 1" / "A1", "ADPCM-A 2" / "A2", "ADPCM-A 3" / "A3",
            "ADPCM-A 4" / "A4", "ADPCM-A 5" / "A5", "ADPCM-A 6" / "A6",
            "ADPCM-B" / "AB",
            "SSG A" / "A", "SSG B" / "B", "SSG C" / "C",
        ],
        K::Ym3812 | K::Ym3526 => channels![
            "FM 1" / "01", "FM 2" / "02", "FM 3" / "03", "FM 4" / "04",
            "FM 5" / "05", "FM 6" / "06", "FM 7" / "07", "FM 8" / "08",
            "FM 9" / "09",
            "Bass Drum" / "BD", "Snare Drum" / "SD", "Tom-Tom" / "TT",
            "Cymbal" / "CY", "Hi-Hat" / "HH",
        ],
        K::Y8950 => channels![
            "FM 1" / "01", "FM 2" / "02", "FM 3" / "03", "FM 4" / "04",
            "FM 5" / "05", "FM 6" / "06", "FM 7" / "07", "FM 8" / "08",
            "FM 9" / "09",
            "Bass Drum" / "BD", "Snare Drum" / "SD", "Tom-Tom" / "TT",
            "Cymbal" / "CY", "Hi-Hat" / "HH",
            "ADPCM" / "AD",
        ],
        K::Ymf262 => channels![
            "FM 1" / "01", "FM 2" / "02", "FM 3" / "03", "FM 4" / "04",
            "FM 5" / "05", "FM 6" / "06", "FM 7" / "07", "FM 8" / "08",
            "FM 9" / "09", "FM 10" / "10", "FM 11" / "11", "FM 12" / "12",
            "FM 13" / "13", "FM 14" / "14", "FM 15" / "15", "FM 16" / "16",
            "FM 17" / "17", "FM 18" / "18",
            "Bass Drum" / "BD", "Snare Drum" / "SD", "Tom-Tom" / "TT",
            "Cymbal" / "CY", "Hi-Hat" / "HH",
        ],
        // The wavetable half only; the OPL4's FM half is the linked OPL3.
        K::Ymf278b => &GENERIC[..24],
        K::Ymf271 => &GENERIC[..12],
        K::Ymz280b => &GENERIC[..8],
        K::Pwm => channels!["PWM" / "PW"],
        K::Ay8910 => channels!["A" / "A", "B" / "B", "C" / "C"],
        K::GameBoyDmg => channels![
            "Pulse 1" / "P1", "Pulse 2" / "P2", "Wave" / "WV", "Noise" / "N",
        ],
        K::NesApu if variant => channels![
            "Pulse 1" / "P1", "Pulse 2" / "P2", "Triangle" / "TR",
            "Noise" / "N", "DMC" / "DM", "FDS" / "FD",
        ],
        K::NesApu => channels![
            "Pulse 1" / "P1", "Pulse 2" / "P2", "Triangle" / "TR",
            "Noise" / "N", "DMC" / "DM",
        ],
        K::MultiPcm => &GENERIC[..28],
        K::Upd7759 | K::Okim6258 => channels!["ADPCM" / "AD"],
        K::Okim6295 | K::K053260 | K::Pokey | K::Ga20 | K::Mikey | K::WonderSwan => &GENERIC[..4],
        K::K051649 => &GENERIC[..5],
        K::K054539 => &GENERIC[..8],
        K::HuC6280 => channels![
            "Wave 1" / "01", "Wave 2" / "02", "Wave 3" / "03",
            "Wave 4" / "04", "Wave 5" / "05", "Wave 6" / "06",
        ],
        K::C140 => &GENERIC[..24],
        K::QSound => channels![
            "PCM 1" / "01", "PCM 2" / "02", "PCM 3" / "03", "PCM 4" / "04",
            "PCM 5" / "05", "PCM 6" / "06", "PCM 7" / "07", "PCM 8" / "08",
            "PCM 9" / "09", "PCM 10" / "10", "PCM 11" / "11", "PCM 12" / "12",
            "PCM 13" / "13", "PCM 14" / "14", "PCM 15" / "15", "PCM 16" / "16",
            "ADPCM 1" / "A1", "ADPCM 2" / "A2", "ADPCM 3" / "A3",
        ],
        K::Scsp | K::Es5503 | K::Es5505 | K::C352 => &GENERIC[..32],
        K::Vsu => channels![
            "Wave 1" / "01", "Wave 2" / "02", "Wave 3" / "03",
            "Wave 4" / "04", "Wave 5" / "05", "Noise" / "N",
        ],
        K::Saa1099 => &GENERIC[..6],
        K::X1010 => &GENERIC[..16],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract a `u32` mute mask depends on: every chip has channels, and
    /// never more than the mask has bits.
    #[test]
    fn every_chip_fits_a_mute_mask() {
        for kind in ChipKind::all() {
            for variant in [false, true] {
                let channels = channels_of(kind, variant);
                assert!(!channels.is_empty(), "{kind:?} has no channels");
                assert!(
                    channels.len() <= 32,
                    "{kind:?} has {} channels, more than a u32 mask",
                    channels.len()
                );
                for channel in channels {
                    assert!(!channel.name.is_empty());
                    assert!(!channel.short.is_empty());
                }
            }
        }
    }

    /// The documented chips' rosters, pinned -- a reorder here silently remaps
    /// every consumer's mute bits.
    #[test]
    fn the_documented_chips_are_pinned() {
        let names = |kind, variant| -> Vec<&'static str> {
            channels_of(kind, variant)
                .iter()
                .map(|channel| channel.name)
                .collect()
        };
        assert_eq!(
            names(ChipKind::Sn76489, false),
            ["Tone 1", "Tone 2", "Tone 3", "Noise"]
        );
        assert_eq!(names(ChipKind::Ym2612, false).len(), 7);
        assert_eq!(names(ChipKind::Ym2612, false)[6], "DAC");
        assert_eq!(names(ChipKind::Ym2413, false).len(), 14);
        assert_eq!(names(ChipKind::Ym2413, false)[9], "Bass Drum");
        assert_eq!(names(ChipKind::Ym2151, false).len(), 8);
        assert_eq!(
            names(ChipKind::Ym2203, false),
            ["FM 1", "FM 2", "FM 3", "SSG A", "SSG B", "SSG C"]
        );
        assert_eq!(names(ChipKind::Ym2608, false).len(), 16);
        assert_eq!(names(ChipKind::Ym2610, false).len(), 16);
        assert_eq!(names(ChipKind::Ay8910, false), ["A", "B", "C"]);
        assert_eq!(
            names(ChipKind::GameBoyDmg, false),
            ["Pulse 1", "Pulse 2", "Wave", "Noise"]
        );
        assert_eq!(names(ChipKind::HuC6280, false).len(), 6);
        assert_eq!(names(ChipKind::SegaPcm, false).len(), 16);
    }

    /// The one variant that changes the roster: the FDS expansion adds a
    /// channel to the NES APU.
    #[test]
    fn the_fds_variant_adds_a_channel() {
        assert_eq!(channels_of(ChipKind::NesApu, false).len(), 5);
        let with_fds = channels_of(ChipKind::NesApu, true);
        assert_eq!(with_fds.len(), 6);
        assert_eq!(with_fds[5].name, "FDS");
    }

    /// Widths the emulator side must be able to serve, pinned where libvgm's
    /// own mute masks were consulted -- a disagreement here is a remap bug
    /// waiting in the provider.
    #[test]
    fn cross_checked_counts_hold() {
        for (kind, count) in [
            (ChipKind::Ymf262, 23),
            (ChipKind::Y8950, 15),
            (ChipKind::Ym3812, 14),
            (ChipKind::MultiPcm, 28),
            (ChipKind::Ymf271, 12),
            (ChipKind::QSound, 19),
            (ChipKind::C140, 24),
            (ChipKind::C352, 32),
            (ChipKind::X1010, 16),
            (ChipKind::Scsp, 32),
            (ChipKind::Vsu, 6),
        ] {
            assert_eq!(channels_of(kind, false).len(), count, "{kind:?}");
        }
    }
}
