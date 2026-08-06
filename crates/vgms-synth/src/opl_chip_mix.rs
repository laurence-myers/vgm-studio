//! Translating the OPL device's [`Muting`]/[`Panning`] vocabulary into the
//! multichip [`ChipMuting`]/[`ChipPanning`] the generic engine speaks (ou-2).
//!
//! The OPL panel describes an 18-channel, two-bank OPL device; the generic
//! playback path ([`VgmEngine`](crate::vgm_engine)) mutes and pans through
//! [`channels_of`](vgms_core::vgm::channels_of)'s per-chip roster. When an OPL
//! document routes to that engine, the panel's `Muting`/`Panning` must become
//! the roster's `ChipMuting`/`ChipPanning`. Three things move:
//!
//! * **Polarity.** A `Muting` bit set means *audible*; a `ChipMuting` bit set
//!   means *muted*. Every channel flips.
//! * **Percussion.** `Muting` gates the `0xBD` rhythm register with a per-bank
//!   AND-mask; the roster lists the five drums (Bass Drum, Snare, Tom-Tom,
//!   Cymbal, Hi-Hat) as their own channels. A drum is muted when its `0xBD` bit
//!   is cleared in the mask.
//! * **Topology.** [`OplType::Opl3`] is one [`Ymf262`](ChipKind::Ymf262) (23
//!   channels: 18 melodic + 5 drums); [`OplType::Opl2`] one
//!   [`Ym3812`](ChipKind::Ym3812) (14: 9 + 5); [`OplType::DualOpl2`] *two*
//!   `Ym3812`, the panel's low bank instance 0 and its high bank instance 1 --
//!   the same split [`dro_to_vgm`](vgms_core::convert::dro_to_vgm) encodes into
//!   the projected VGM's header.
//!
//! Panning rides the OPL core's stereo-ext panpots either way, so the byte
//! image the panel emits (`0x00` left .. `0x80` centre .. `0xFF` right) maps to
//! a `chip_mix` position by `(byte - 0x80) * 2` -- the exact inverse of
//! [`OplCoreAdapter`](crate::opl_adapter)'s panpot conversion, so the round trip
//! reproduces the panel's intended panpot.

use vgms_core::vgm::ChipKind;
use vgms_core::{Bank, OplType};

use crate::chip_mix::{ChipMuting, ChipPanning, PAN_CENTER};
use crate::engine::{Muting, Panning};

/// How many melodic channels one OPL bank carries (`0xB0`..=`0xB8`).
const BANK_CHANNELS: usize = 9;

/// The five rhythm voices' `0xBD` bits, in the roster's drum order
/// (Bass Drum, Snare Drum, Tom-Tom, Cymbal, Hi-Hat) -- so drum `d` is muted when
/// bit `DRUM_BITS[d]` is cleared in the percussion mask.
const DRUM_BITS: [u8; 5] = [0x10, 0x08, 0x04, 0x02, 0x01];

/// The [`ChipKind`] one `Ym3812`/`Ymf262` instance of an [`OplType`] projects to
/// -- the key its [`ChipMuting`]/[`ChipPanning`] entry must carry to reach the
/// projected VGM's voice.
#[must_use]
fn opl_chip_kind(opl_type: OplType) -> ChipKind {
    match opl_type {
        OplType::Opl3 => ChipKind::Ymf262,
        OplType::Opl2 | OplType::DualOpl2 => ChipKind::Ym3812,
    }
}

/// The `chip_mix` pan position for an OPL panpot byte: `0x00` hard left, `0x80`
/// centre, `0xFF` hard right, on the `-0x100..=0x100` scale. The inverse of
/// [`OplCoreAdapter`](crate::opl_adapter)'s `to_panpot`.
#[must_use]
fn pan_of(byte: u8) -> i16 {
    (i16::from(byte) - 0x80) * 2
}

/// The mute mask for one OPL chip instance: its `melodic` channels drawn from
/// `bank`, then the five drums from that bank's percussion mask. Bit `i` set
/// means channel `i` of the roster is muted.
#[must_use]
fn instance_mask(muting: &Muting, bank: Bank, melodic: usize) -> u32 {
    let mut mask = 0u32;
    for channel in 0..melodic {
        if !muting.is_channel_audible(bank, 0xB0 + channel as u8) {
            mask |= 1 << channel;
        }
    }
    let percussion = muting.percussion_raw()[usize::from(bank.index())];
    for (drum, &bit) in DRUM_BITS.iter().enumerate() {
        if percussion & bit == 0 {
            mask |= 1 << (melodic + drum);
        }
    }
    mask
}

/// The generic-engine mutes an OPL [`Muting`] describes for an [`OplType`]
/// document.
///
/// [`OplType::Opl3`] fills one [`Ymf262`](ChipKind::Ymf262) whose 18 melodic
/// channels span both banks (0..8 low, 9..17 high) with the five drums from the
/// low bank; [`OplType::Opl2`] one [`Ym3812`](ChipKind::Ym3812) from the low
/// bank alone; [`OplType::DualOpl2`] two `Ym3812`, the low bank as instance 0
/// and the high bank as instance 1.
#[must_use]
pub fn opl_chip_muting(muting: &Muting, opl_type: OplType) -> ChipMuting {
    let mut out = ChipMuting::new();
    match opl_type {
        OplType::Opl3 => {
            let mut mask = 0u32;
            for i in 0..18 {
                let bank = if i < BANK_CHANNELS {
                    Bank::Low
                } else {
                    Bank::High
                };
                let channel = 0xB0 + (i % BANK_CHANNELS) as u8;
                if !muting.is_channel_audible(bank, channel) {
                    mask |= 1 << i;
                }
            }
            let percussion = muting.percussion_raw()[usize::from(Bank::Low.index())];
            for (drum, &bit) in DRUM_BITS.iter().enumerate() {
                if percussion & bit == 0 {
                    mask |= 1 << (18 + drum);
                }
            }
            out.set(ChipKind::Ymf262, 0, mask);
        }
        OplType::Opl2 => {
            out.set(
                ChipKind::Ym3812,
                0,
                instance_mask(muting, Bank::Low, BANK_CHANNELS),
            );
        }
        OplType::DualOpl2 => {
            out.set(
                ChipKind::Ym3812,
                0,
                instance_mask(muting, Bank::Low, BANK_CHANNELS),
            );
            out.set(
                ChipKind::Ym3812,
                1,
                instance_mask(muting, Bank::High, BANK_CHANNELS),
            );
        }
    }
    out
}

/// A roster-length pan array for one OPL chip instance: `melodic` positions from
/// `image` (the panel's byte-per-channel `Custom` image, sliced from `offset`),
/// then five centred drum slots. The array is the full roster width so
/// [`OplCoreAdapter::set_channel_pans`](crate::opl_adapter) reads its melodic
/// prefix correctly (it takes `len - 5` melodic entries).
#[must_use]
fn instance_pans(image: &[u8; 18], offset: usize, melodic: usize) -> Vec<i16> {
    let mut pans = vec![PAN_CENTER; melodic + DRUM_BITS.len()];
    for (channel, slot) in pans.iter_mut().take(melodic).enumerate() {
        *slot = pan_of(image[offset + channel]);
    }
    pans
}

/// The generic-engine pans an OPL [`Panning`] describes for an [`OplType`]
/// document.
///
/// [`Panning::Original`] leaves every chip on its own stereo image (an empty
/// [`ChipPanning`]); [`Panning::Custom`] fans the byte image out over the same
/// instances [`opl_chip_muting`] uses, drums centred. The pan positions match
/// the panpots the OPL core would apply, so routing through the generic engine
/// reproduces the OPL panel's image.
#[must_use]
pub fn opl_chip_panning(panning: &Panning, opl_type: OplType) -> ChipPanning {
    let mut out = ChipPanning::new();
    let Panning::Custom(image) = panning else {
        return out;
    };
    match opl_type {
        OplType::Opl3 => {
            out.set(ChipKind::Ymf262, 0, instance_pans(image, 0, 18));
        }
        OplType::Opl2 => {
            out.set(ChipKind::Ym3812, 0, instance_pans(image, 0, BANK_CHANNELS));
        }
        OplType::DualOpl2 => {
            out.set(ChipKind::Ym3812, 0, instance_pans(image, 0, BANK_CHANNELS));
            out.set(
                ChipKind::Ym3812,
                1,
                instance_pans(image, BANK_CHANNELS, BANK_CHANNELS),
            );
        }
    }
    out
}

/// Mutes, on `bank`, the melodic channels and drums a roster `mask` marks --
/// the inverse of [`instance_mask`] for one `Ym3812` bank (`melodic` melodic
/// channels, then the five drums).
fn mute_bank_from_mask(muting: &mut Muting, mask: u32, bank: Bank, melodic: usize) {
    for channel in 0..melodic {
        if mask & (1 << channel) != 0 {
            muting.mute_channel(bank, 0xB0 + channel as u8);
        }
    }
    // Start every drum audible (as `Muting::all` leaves them) and clear the bit
    // of each muted one; only write the mask back if a drum is actually muted,
    // so an untouched bank keeps its full `0xFF`.
    let mut percussion = 0xFFu8;
    for (drum, &bit) in DRUM_BITS.iter().enumerate() {
        if mask & (1 << (melodic + drum)) != 0 {
            percussion &= !bit;
        }
    }
    if percussion != 0xFF {
        muting.set_percussion(bank, percussion);
    }
}

/// The OPL [`Muting`] a generic-engine [`ChipMuting`] describes -- the inverse
/// of [`opl_chip_muting`].
///
/// A DRO now drives the same per-chip [`GenericChannelPanel`] a VGM does, so its
/// mixer speaks [`ChipMuting`] keyed by the projection chip; this turns that back
/// into the OPL `Muting` vocabulary the DRO's audio, render and split paths still
/// consume. The round trip is lossless for the states a panel produces:
/// `opl_chip_muting(opl_muting_from_chip(c, t), t) == c`.
///
/// [`GenericChannelPanel`]: https://docs.rs/vgms-ui
#[must_use]
pub fn opl_muting_from_chip(chip: &ChipMuting, opl_type: OplType) -> Muting {
    let mut muting = Muting::all();
    match opl_type {
        OplType::Opl3 => {
            let mask = chip.mask_for(ChipKind::Ymf262, 0);
            for i in 0..18 {
                if mask & (1 << i) != 0 {
                    let bank = if i < BANK_CHANNELS {
                        Bank::Low
                    } else {
                        Bank::High
                    };
                    muting.mute_channel(bank, 0xB0 + (i % BANK_CHANNELS) as u8);
                }
            }
            // The five drums live on the low bank for an OPL3 document.
            let mut percussion = 0xFFu8;
            for (drum, &bit) in DRUM_BITS.iter().enumerate() {
                if mask & (1 << (18 + drum)) != 0 {
                    percussion &= !bit;
                }
            }
            if percussion != 0xFF {
                muting.set_percussion(Bank::Low, percussion);
            }
        }
        OplType::Opl2 => {
            mute_bank_from_mask(
                &mut muting,
                chip.mask_for(ChipKind::Ym3812, 0),
                Bank::Low,
                BANK_CHANNELS,
            );
        }
        OplType::DualOpl2 => {
            mute_bank_from_mask(
                &mut muting,
                chip.mask_for(ChipKind::Ym3812, 0),
                Bank::Low,
                BANK_CHANNELS,
            );
            mute_bank_from_mask(
                &mut muting,
                chip.mask_for(ChipKind::Ym3812, 1),
                Bank::High,
                BANK_CHANNELS,
            );
        }
    }
    muting
}

/// The [`ChipKind`] an [`OplType`] document's voices carry, exposed for the
/// callers that must name the same instance when they push a fresh mute/pan.
#[must_use]
pub fn opl_projection_kind(opl_type: OplType) -> ChipKind {
    opl_chip_kind(opl_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chip_mix::{PAN_LEFT, PAN_RIGHT};

    /// Nothing muted, own stereo image: the neutral state a fresh panel is in
    /// must translate to neutral mutes and pans, whatever the OPL type.
    #[test]
    fn a_neutral_panel_translates_to_neutral() {
        for opl_type in OplType::ALL {
            let muting = opl_chip_muting(&Muting::all(), opl_type);
            assert!(muting.is_neutral(), "{opl_type:?} mutes not neutral");
            let panning = opl_chip_panning(&Panning::Original, opl_type);
            assert!(panning.is_neutral(), "{opl_type:?} pans not neutral");
        }
    }

    /// The polarity flip: an audible channel is an unset `ChipMuting` bit, a
    /// muted one a set bit. Muting low-bank channel 0 (`0xB0`) sets bit 0.
    #[test]
    fn muting_a_channel_sets_its_roster_bit_opl3() {
        let mut muting = Muting::all();
        muting.mute_channel(Bank::Low, 0xB0);
        let chip = opl_chip_muting(&muting, OplType::Opl3);
        assert_eq!(chip.mask_for(ChipKind::Ymf262, 0), 0b1);
    }

    /// The high bank is the top nine melodic bits of an OPL3 roster: muting
    /// high-bank channel 3 (`0xB3`) sets bit `9 + 3`.
    #[test]
    fn the_high_bank_is_the_upper_nine_channels_opl3() {
        let mut muting = Muting::all();
        muting.mute_channel(Bank::High, 0xB3);
        let chip = opl_chip_muting(&muting, OplType::Opl3);
        assert_eq!(chip.mask_for(ChipKind::Ymf262, 0), 1 << (9 + 3));
    }

    /// Silencing the low-bank drums sets all five drum bits (roster indices
    /// 18..=22), and nothing else.
    #[test]
    fn muting_drums_sets_the_five_drum_bits_opl3() {
        let mut muting = Muting::all();
        muting.set_percussion(Bank::Low, 0xE0); // keep control bits, drop the drums
        let chip = opl_chip_muting(&muting, OplType::Opl3);
        assert_eq!(chip.mask_for(ChipKind::Ymf262, 0), 0b1_1111 << 18);
    }

    /// One drum at a time: clearing only the Snare Drum bit (`0x08`) mutes the
    /// second drum in the roster (index 19), leaving the rest audible.
    #[test]
    fn a_single_drum_bit_maps_to_one_roster_drum() {
        let mut muting = Muting::all();
        muting.set_percussion(Bank::Low, 0xFF ^ 0x08); // all pass but the snare bit
        let chip = opl_chip_muting(&muting, OplType::Opl3);
        assert_eq!(chip.mask_for(ChipKind::Ymf262, 0), 1 << 19);
    }

    /// An OPL2 document is one 14-channel Ym3812 from the low bank; the high
    /// bank is not its chip and must not leak in.
    #[test]
    fn opl2_is_one_low_bank_ym3812() {
        let mut muting = Muting::all();
        muting.mute_channel(Bank::Low, 0xB2);
        muting.set_percussion(Bank::Low, 0xE0);
        muting.mute_channel(Bank::High, 0xB0); // not this chip's bank
        let chip = opl_chip_muting(&muting, OplType::Opl2);
        // Channel 2 muted, five drums (indices 9..=13) muted; high bank ignored.
        assert_eq!(
            chip.mask_for(ChipKind::Ym3812, 0),
            (1 << 2) | (0b1_1111 << 9)
        );
        assert_eq!(chip.mask_for(ChipKind::Ym3812, 1), 0, "no second instance");
    }

    /// A dual OPL2 is two Ym3812: the panel's low bank is instance 0, its high
    /// bank instance 1, each with its own drums.
    #[test]
    fn dual_opl2_splits_the_banks_into_two_instances() {
        let mut muting = Muting::all();
        muting.mute_channel(Bank::Low, 0xB0); // instance 0, channel 0
        muting.mute_channel(Bank::High, 0xB4); // instance 1, channel 4
        muting.set_percussion(Bank::High, 0xE0); // instance 1's drums
        let chip = opl_chip_muting(&muting, OplType::DualOpl2);
        assert_eq!(chip.mask_for(ChipKind::Ym3812, 0), 0b1);
        assert_eq!(
            chip.mask_for(ChipKind::Ym3812, 1),
            (1 << 4) | (0b1_1111 << 9)
        );
    }

    /// `Original` leaves every chip on its own image -- no pan entry at all.
    #[test]
    fn original_panning_is_no_entry() {
        for opl_type in OplType::ALL {
            assert!(opl_chip_panning(&Panning::Original, opl_type).is_neutral());
        }
    }

    /// A custom image maps byte-per-channel to a roster-length array: the 18
    /// melodic channels take their pan, the five drums stay centred, and the
    /// extremes hit the hard positions.
    #[test]
    fn custom_panning_maps_bytes_to_positions_opl3() {
        let mut image = [0x80u8; 18]; // all centred
        image[0] = 0x00; // hard left
        image[1] = 0xFF; // hard right
        image[17] = 0xC0; // half right (high bank, last channel)
        let panning = opl_chip_panning(&Panning::Custom(image), OplType::Opl3);
        let pans = panning.pans_for(ChipKind::Ymf262, 0).expect("an entry");
        assert_eq!(pans.len(), 23, "full roster width");
        assert_eq!(pans[0], PAN_LEFT);
        assert_eq!(pans[1], PAN_RIGHT - 2, "0xFF is one step shy of +0x100");
        assert_eq!(pans[2], PAN_CENTER);
        assert_eq!(pans[17], 0x80);
        assert!(
            pans[18..].iter().all(|&p| p == PAN_CENTER),
            "drums centred: {:?}",
            &pans[18..]
        );
    }

    /// A dual OPL2's pan image splits the same way its mutes do: bytes 0..8 to
    /// instance 0, bytes 9..17 to instance 1.
    #[test]
    fn custom_panning_splits_across_dual_opl2() {
        let mut image = [0x80u8; 18];
        image[0] = 0x00; // instance 0, channel 0
        image[9] = 0xFF; // instance 1, channel 0
        let panning = opl_chip_panning(&Panning::Custom(image), OplType::DualOpl2);
        let first = panning.pans_for(ChipKind::Ym3812, 0).expect("instance 0");
        let second = panning.pans_for(ChipKind::Ym3812, 1).expect("instance 1");
        assert_eq!(first.len(), 14);
        assert_eq!(first[0], PAN_LEFT);
        assert_eq!(second[0], PAN_RIGHT - 2);
    }

    /// The reverse mute translation is the exact inverse of the forward one for
    /// every state a panel produces, so a DRO's `ChipMuting` survives the trip
    /// out to the OPL vocabulary and back into the engine unchanged.
    #[test]
    fn chip_muting_round_trips_through_the_opl_vocabulary() {
        // Compare per (kind, instance) mask rather than structurally: the forward
        // translation always emits a (possibly-zero) entry per instance, so an
        // all-audible state that starts with no entries is still equivalent.
        let check = |opl_type: OplType, build: &dyn Fn(&mut ChipMuting)| {
            let mut chip = ChipMuting::new();
            build(&mut chip);
            let back = opl_chip_muting(&opl_muting_from_chip(&chip, opl_type), opl_type);
            let instances = match opl_type {
                OplType::Opl3 => vec![(ChipKind::Ymf262, 0)],
                OplType::Opl2 => vec![(ChipKind::Ym3812, 0)],
                OplType::DualOpl2 => vec![(ChipKind::Ym3812, 0), (ChipKind::Ym3812, 1)],
            };
            for (kind, instance) in instances {
                assert_eq!(
                    back.mask_for(kind, instance),
                    chip.mask_for(kind, instance),
                    "{opl_type:?} {kind:?}#{instance} did not round-trip"
                );
            }
        };

        // Nothing muted.
        for opl_type in OplType::ALL {
            check(opl_type, &|_| {});
        }
        // A melodic channel, a single drum, and both together.
        check(OplType::Opl2, &|c| c.set(ChipKind::Ym3812, 0, 1 << 2));
        check(OplType::Opl2, &|c| c.set(ChipKind::Ym3812, 0, 1 << 11)); // Tom-Tom
        check(OplType::Opl2, &|c| {
            c.set(ChipKind::Ym3812, 0, (1 << 0) | (1 << 9) | (1 << 13));
        });
        // OPL3: a high-bank channel and the full drum set.
        check(OplType::Opl3, &|c| {
            c.set(ChipKind::Ymf262, 0, (1 << 12) | (0b1_1111 << 18));
        });
        // Dual OPL2: each instance muted independently.
        check(OplType::DualOpl2, &|c| {
            c.set(ChipKind::Ym3812, 0, (1 << 1) | (1 << 9));
            c.set(ChipKind::Ym3812, 1, (1 << 4) | (0b1_1111 << 9));
        });
        // A whole chip muted (the lamp): every channel and drum bit set.
        check(OplType::Opl3, &|c| {
            c.set(ChipKind::Ymf262, 0, (1 << 23) - 1)
        });
    }

    /// The pan mapping is the exact inverse of the adapter's panpot conversion,
    /// so a byte survives the round trip: `to_panpot(pan_of(b)) == b`.
    #[test]
    fn pan_round_trips_through_the_panpot_conversion() {
        for byte in 0u8..=0xFF {
            let pan = pan_of(byte);
            // The adapter's to_panpot: (0x80 + pan/2).clamp(0, 0xFF).
            let panpot = (0x80 + i32::from(pan) / 2).clamp(0, 0xFF) as u8;
            assert_eq!(panpot, byte, "byte {byte:#04X} did not round-trip");
        }
    }
}
