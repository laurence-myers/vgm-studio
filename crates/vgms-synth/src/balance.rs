//! VGMPlay's cross-chip balance, expressed as a per-voice *ratio*.
//!
//! The reference player mixes a multi-chip file in three volume stages:
//!
//! 1. **Per chip**, `GetChipVolume`: a fixed table (`_CHIP_VOLUME`), halved
//!    per instance for a dual-chip declaration (except the T6W28, two half
//!    chips that make one), overridden by the v1.70 extra header's per-chip
//!    volumes, then two per-core patches of its own (K051649 x8/5, C140
//!    x2/3).
//! 2. **Per file**, `EstimateOverallVolume` / `NormalizeOverallVolume`: the
//!    chip volumes are summed -- weighted by a second table,
//!    `_PB_VOL_AMNT` -- and every chip volume is then doubled while the sum
//!    sits at or under 0x180 and halved while it sits over 0x300. A
//!    power-of-two loudness normalisation, decided entirely by the declared
//!    chip set.
//! 3. The header's volume modifier, on the master -- which this app applies
//!    as the volume lever's starting position, not here.
//!
//! Our per-core [`CoreInfo::level`](crate::CoreInfo::level) calibrations were
//! each measured against the reference playing a **single-chip file**, so
//! stages 1 and 2 for that one-chip set are already inside every calibrated
//! level. What a multi-chip file needs is only the *difference* between the
//! stages for its set and the stages for each chip alone:
//!
//! ```text
//! ratio(chip, set) = V_eff(chip in set) * N(set) / (V(chip) * N({chip}))
//! ```
//!
//! which is exactly `1.0` for every single-instance single-chip file -- the
//! calibrations, and the parity measurements behind them, do not move. For
//! Black Knight 2000's YM2612+YM2151+PWM set it is 1/2 for all three: the
//! sum of three chips normalises the file down where each chip alone was
//! normalised up, which is why our un-normalised mix clipped where the
//! reference did not.
//!
//! Applied per voice *inside* the engine (not on the summed mix) so the
//! headroom exists before the sum is clamped to 16 bits.

use vgms_core::vgm::{ChipKind, ChipUse, ExtraHeader};

/// Unity in 8.8 fixed point, the scale [`voice_gain`] returns.
pub(crate) const GAIN_UNITY: u32 = 0x100;

/// VGMPlay's `_CHIP_VOLUME`, indexed by [`ChipKind::id`] -- the default
/// loudness of each chip relative to full scale (8.8, 0x100 = 1.0).
const CHIP_VOLUME: [u16; 0x2A] = [
    0x80, 0x200, 0x100, 0x100, 0x180, 0xB0, 0x100, 0x80, // SN76489..YM2608
    0x80, 0x100, 0x100, 0x100, 0x100, 0x100, 0x100, 0x98, // YM2610..YMZ280B
    0x80, 0xE0, 0x100, 0xC0, 0x100, 0x40, 0x11E, 0x1C0, // RF5C164..OKIM6258
    0x100, 0xA0, 0x100, 0x100, 0x100, 0x100, 0x100, 0x100, // OKIM6295..QSound
    0x20, 0x100, 0x100, 0x100, 0x40, 0x20, 0x100, 0x40, // SCSP..C352
    0x280, 0x100, // GA20, Mikey
];

/// VGMPlay's `_PB_VOL_AMNT` -- the weight each chip's volume carries in the
/// overall-loudness estimate (8.8).
const PB_VOL: [u16; 0x2A] = [
    0x100, 0x80, 0x100, 0x100, 0x100, 0x100, 0x100, 0x100, // SN76489..YM2608
    0x100, 0x200, 0x200, 0x200, 0x200, 0x100, 0x100, 0x1AF, // YM2610..YMZ280B
    0x200, 0x100, 0x200, 0x400, 0x200, 0x400, 0x100, 0x200, // RF5C164..OKIM6258
    0x200, 0x100, 0x100, 0x100, 0x180, 0x100, 0x100, 0x100, // OKIM6295..QSound
    0x800, 0x100, 0x100, 0x100, 0x800, 0x1000, 0x100, 0x800, // SCSP..C352
    0x100, 0x200, // GA20, Mikey
];

/// Upstream's rounding 8.8 multiply (`MulFixed8x8`).
const fn mul_8x8(a: u32, b: u32) -> u32 {
    (a * b + 0x80) >> 8
}

/// `GetChipVolume` for one instance of a declared chip: the table value,
/// halved per instance of a dual declaration (T6W28 excepted), an extra-header
/// override if the file carries one, then upstream's two per-core patches.
///
/// `linked` asks for the chip's linked child rather than the chip itself
/// (upstream's `GetChipVolume(.., isLinked)`): the reference halves only the
/// YM2203's SSG, matches the paired extra-header entry rather than the plain
/// one, and skips the per-core patches (which fold `isLinked` into the type
/// before the patch check). Every other stage is shared.
fn chip_volume(chip: &ChipUse, instance: u8, extra: Option<&ExtraHeader>, linked: bool) -> u32 {
    let id = chip.kind.id() as usize;
    let mut volume = u32::from(CHIP_VOLUME[id]);
    // The reference draws the linked base from the parent's own table value and
    // halves only chip type 0x06 (the YM2203's SSG); the other linked parents
    // (YM2608/YM2610/YMF278B) take the table value as-is.
    if linked && chip.kind == ChipKind::Ym2203 {
        volume /= 2;
    }
    if chip.dual && !chip.is_t6w28() {
        volume /= 2;
    }
    if let Some(extra) = extra
        && let Some(entry) = extra.volumes.iter().find(|entry| {
            usize::from(entry.chip_id) == id
                && entry.paired == linked
                && entry.second_instance == (instance == 1)
        })
    {
        // `ExtraVolume::volume` arrives with the relative bit already
        // stripped by the header parser.
        if entry.relative {
            volume = mul_8x8(volume, u32::from(entry.volume));
        } else {
            volume = u32::from(entry.volume);
        }
    }
    // The per-core patches are the parent chip's own; a linked child never hits
    // them (none of the linked parents is a K051649 or C140).
    if linked {
        return volume;
    }
    match chip.kind {
        ChipKind::K051649 => volume * 8 / 5,
        ChipKind::C140 => (volume * 2 + 1) / 3,
        _ => volume,
    }
}

/// One chip instance's weight in `EstimateOverallVolume`: upstream sums
/// `MulFixed8x8(volumeL + volumeR, PB) / 2` per started device.
fn estimate_contribution(chip: &ChipUse, instance: u8, extra: Option<&ExtraHeader>) -> u32 {
    let volume = chip_volume(chip, instance, extra, false);
    mul_8x8(volume * 2, u32::from(PB_VOL[chip.kind.id() as usize])) / 2
}

/// The linked child a declared chip starts beside itself -- an OPN's SSG, the
/// OPL4's FM half -- as the reference's estimate counts it: every started
/// device enters `EstimateOverallVolume`, links included, at
/// `GetChipVolume(.., isLinked)` (the SSG at half the FM, the OPL4's OPL3 at
/// parity). Both children's PB weight is 0x100, so the weighted contribution
/// is the volume itself. Without this, a YM2203 + OKIM6295 set estimated
/// 0x300 (no shift) where the reference reads 0x380 (halve everything) -- the
/// whole mix +6 dB.
///
/// A v1.70 extra-header volume entry with the paired bit (the parent's id
/// with bit 7) overrides the linked half, absolutely or relatively, as the
/// reference folds `isLinked` into the matched type. (The override reaches
/// the *estimate* here; the linked child's audible gain inside the core
/// binding stays its constant -- the rare files carrying such entries are the
/// remaining gap, recorded in the audit.)
fn linked_contribution(chip: &ChipUse, instance: u8, extra: Option<&ExtraHeader>) -> u32 {
    // The reference calls `GetChipVolume(.., isLinked=1)` only for a chip whose
    // `linkDevIDs` is non-empty -- the OPN family's SSG and the OPL4's FM half.
    // Everything else has no linked child and contributes nothing.
    if !matches!(
        chip.kind,
        ChipKind::Ym2203 | ChipKind::Ym2608 | ChipKind::Ym2610 | ChipKind::Ymf278b
    ) {
        return 0;
    }
    // The shared dual-halving and extra-header override now live in
    // `chip_volume`; the linked base is the parent's own table value (the SSG
    // at half the FM for the YM2203, at parity for the rest).
    chip_volume(chip, instance, extra, true)
}

/// The estimate weight of the chips past our roster -- the reference's tail
/// rows (extra-header ids `0x2A`-`0x2F`: K007232, K005289, MSM5205, MSM5232,
/// BSMT2000, ICS2115). We cannot play them, but their declared presence still
/// weights the reference's whole-mix normalisation: a YM2151 + K007232 rip
/// estimates 0x200 there (no shift), and ignoring the K007232 normalised the
/// YM2151 up to its solo level, +6 dB over the reference. Each value is
/// `volume x PB >> 8` from the reference's `_CHIP_VOLUME`/`_PB_VOL_AMNT` tail
/// entries.
pub(crate) fn tail_contribution(tail_ids: &[u8]) -> u32 {
    // Premultiplied `volume x PB >> 8` for the reference ids 0x2A-0x2F (=
    // CHIP_COUNT..CHIP_COUNT+6, its un-cored tail chips). Promoting any of them
    // to a real `ChipKind` means deleting its row here; the header's
    // `read_tail_chip_ids` `from_id` filter then stops it being double-counted.
    const TAIL: [u32; 6] = [0x100, 0x100, 0x200, 0x100, 0x200, 0x200];
    tail_ids
        .iter()
        .filter_map(|&id| TAIL.get(usize::from(id.wrapping_sub(0x2A))))
        .sum()
}

/// `NormalizeOverallVolume`'s factor for an estimate, as a power-of-two
/// exponent: `+n` doubles every chip volume `n` times, `-n` halves.
fn normalization_shift(mut estimate: u32) -> i32 {
    if estimate == 0 {
        return 0;
    }
    let mut shift = 0;
    if estimate <= 0x180 {
        while estimate <= 0x180 {
            shift += 1;
            estimate *= 2;
        }
    } else {
        while estimate > 0x300 {
            shift -= 1;
            estimate /= 2;
        }
    }
    shift
}

/// The reference's whole-file `EstimateOverallVolume`: every declared chip
/// instance's weight, its linked child's, and the un-cored tail chips'.
///
/// `chips` is the file's full declared set -- including chips this build has no
/// core for, because the reference's normalisation counts them too. A pure
/// function of the header, so a caller building many voices computes it once.
pub(crate) fn mix_estimate(chips: &[ChipUse], extra: Option<&ExtraHeader>, tail_ids: &[u8]) -> u32 {
    chips
        .iter()
        .flat_map(|chip| {
            let instances = if chip.dual && !chip.is_t6w28() { 2 } else { 1 };
            (0..instances).map(move |at| {
                estimate_contribution(chip, at, extra) + linked_contribution(chip, at, extra)
            })
        })
        .sum::<u32>()
        + tail_contribution(tail_ids)
}

/// The gain (8.8, [`GAIN_UNITY`] = 1.0) one voice needs so the whole mix sits
/// where the reference's does, given that the voice's core is already
/// calibrated to the reference's *single-chip* net level.
///
/// `mix_estimate` is the file-wide estimate from [`mix_estimate`], passed in so
/// a multi-voice build does not re-sum it per voice.
pub(crate) fn voice_gain(
    voice: &ChipUse,
    instance: u8,
    extra: Option<&ExtraHeader>,
    mix_estimate: u32,
) -> u32 {
    // The chip alone, single-instance, no overrides: the file every
    // calibration was measured on. The linked term is included on both sides
    // -- the reference's solo-file estimate counts the started SSG or OPL4-FM
    // child too, and the calibration was measured against exactly that render
    // -- so a solo file's ratio stays unity by construction.
    let alone = ChipUse {
        dual: false,
        ..*voice
    };
    let single_estimate =
        estimate_contribution(&alone, 0, None) + linked_contribution(&alone, 0, None);

    let effective = chip_volume(voice, instance, extra, false);
    let single = chip_volume(&alone, 0, None, false);
    if single == 0 {
        return GAIN_UNITY;
    }

    let shift = normalization_shift(mix_estimate) - normalization_shift(single_estimate);
    let numerator = i64::from(effective) << (8 + shift.max(0));
    let denominator = i64::from(single) << (-shift).max(0);
    u32::try_from(numerator / denominator).unwrap_or(GAIN_UNITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The old whole-set `voice_gain` shape, for tests written against it: the
    /// estimate is now hoisted, so this recombines the two.
    fn gain(
        chips: &[ChipUse],
        voice: &ChipUse,
        instance: u8,
        extra: Option<&ExtraHeader>,
        tail: &[u8],
    ) -> u32 {
        voice_gain(voice, instance, extra, mix_estimate(chips, extra, tail))
    }

    fn declared(kind: ChipKind) -> ChipUse {
        ChipUse {
            kind,
            clock: 3_579_545,
            dual: false,
            variant: false,
        }
    }

    /// A single-chip file is the calibration anchor, so its ratio is exactly
    /// unity -- for every chip there is a table row for.
    #[test]
    fn a_single_chip_file_is_left_exactly_alone() {
        for kind in ChipKind::all() {
            let chips = [declared(kind)];
            assert_eq!(
                gain(&chips, &chips[0], 0, None, &[]),
                GAIN_UNITY,
                "{} moved on a single-chip file",
                kind.name()
            );
        }
    }

    /// The Mega Drive pair. The estimates: SN 0x80 and YM2612 0x100, so the
    /// pair (0x180) normalises up once where the SN alone (0x80) normalised
    /// up twice and the YM2612 alone (0x100) once -- the PSG drops to half
    /// against its solo calibration and the FM keeps its own. That 1:2 tilt
    /// *is* VGMPlay's Mega Drive balance (`_CHIP_VOLUME` 0x80 vs 0x100).
    #[test]
    fn the_mega_drive_pair_keeps_the_references_tilt() {
        let chips = [declared(ChipKind::Sn76489), declared(ChipKind::Ym2612)];
        assert_eq!(gain(&chips, &chips[0], 0, None, &[]), 0x80, "SN76489");
        assert_eq!(gain(&chips, &chips[1], 0, None, &[]), 0x100, "YM2612");
    }

    /// Cameltry's Taito B System pair, with the linked SSG counted as the
    /// reference counts every started device. The estimates: YM2203 0x100 +
    /// its SSG 0x80, OKIM6295 0x200 (PB 0x200) -- sum 0x380, over 0x300, so
    /// the set halves once. Alone, the YM2203 (0x180 with its SSG) normalises
    /// up once and the OKI (0x200) does not: in-set the FM sits at a quarter
    /// of its solo calibration and the OKI at half -- upstream's exact
    /// arithmetic for this set. (The pre-M8 code missed the SSG term, read
    /// the sum as 0x300, and played the whole file +6 dB over the reference;
    /// the audit's M8.)
    #[test]
    fn the_cameltry_pair_keeps_the_references_tilt() {
        let chips = [declared(ChipKind::Ym2203), declared(ChipKind::Okim6295)];
        assert_eq!(gain(&chips, &chips[0], 0, None, &[]), 0x40, "YM2203");
        assert_eq!(gain(&chips, &chips[1], 0, None, &[]), 0x80, "OKIM6295");
    }

    /// The M9 half of the estimate: a declared tail chip (past our roster)
    /// still weights the normalisation. YM2151 + K007232: the reference
    /// estimates 0x100 + 0x100 = 0x200 -- no shift -- where ignoring the
    /// K007232 read 0x100 and normalised the YM2151 up to its solo doubling.
    #[test]
    fn a_declared_tail_chip_weights_the_estimate() {
        let chips = [declared(ChipKind::Ym2151)];
        // Alone: estimate 0x100, normalise x2; with the K007232 counted the
        // set sits at 0x200 (no shift), so the YM2151 plays at half its solo
        // calibration -- the reference's tilt.
        assert_eq!(gain(&chips, &chips[0], 0, None, &[]), 0x100);
        assert_eq!(gain(&chips, &chips[0], 0, None, &[0x2A]), 0x80);
        // The six tail contributions, from the reference's tables.
        assert_eq!(tail_contribution(&[0x2A]), 0x100, "K007232");
        assert_eq!(tail_contribution(&[0x2C]), 0x200, "MSM5205: PB 0x200");
        assert_eq!(tail_contribution(&[0x2F]), 0x200, "ICS2115: 0x800 x 0x40");
        assert_eq!(tail_contribution(&[0x30]), 0, "past the table: nothing");
    }

    /// Black Knight 2000's set: three chips summing to 0x2E0 normalise by 1
    /// where each alone normalised by 2 -- every voice at half, which is the
    /// headroom our un-normalised mix was missing when it clipped.
    #[test]
    fn the_black_knight_set_matches_the_reference_arithmetic() {
        let chips = [
            declared(ChipKind::Ym2612),
            declared(ChipKind::Ym2151),
            declared(ChipKind::Pwm),
        ];
        for chip in &chips {
            assert_eq!(
                gain(&chips, chip, 0, None, &[]),
                0x80,
                "{}",
                chip.kind.name()
            );
        }
    }

    /// A dual declaration halves each instance -- upstream's `numChips`
    /// division -- on top of whatever the set's normalisation does.
    #[test]
    fn a_dual_declaration_halves_each_instance() {
        let mut dual = declared(ChipKind::Ym2151);
        dual.dual = true;
        let chips = [dual];
        // Alone: estimate 0x100 -> x2. Dual: two instances at 0x80 each ->
        // estimate 0x100 -> x2. Ratio per instance: (0x80 * 2) / (0x100 * 2).
        assert_eq!(gain(&chips, &chips[0], 0, None, &[]), 0x80);
        assert_eq!(gain(&chips, &chips[0], 1, None, &[]), 0x80);
    }

    /// The T6W28 is two half-chips making one chip: declared dual, not halved.
    #[test]
    fn the_t6w28_is_not_halved() {
        let mut t6w28 = declared(ChipKind::Sn76489);
        t6w28.dual = true;
        t6w28.variant = true;
        let chips = [t6w28];
        assert_eq!(gain(&chips, &chips[0], 0, None, &[]), GAIN_UNITY);
    }

    /// An extra-header volume override lands in the effective volume: an
    /// absolute entry replaces the table's, a relative one scales it.
    ///
    /// Measured on a two-chip set because the whole-mix normalisation is free
    /// to compensate part of an override; what must survive is the *relative*
    /// balance the override asked for -- upstream's arithmetic exactly.
    #[test]
    fn extra_header_volumes_override_the_table() {
        use vgms_core::vgm::ExtraVolume;
        let chips = [declared(ChipKind::Ym2612), declared(ChipKind::Ym2151)];
        // Without an override the pair estimates 0x200 (no shift) and each
        // chip alone 0x100 (one doubling): both voices at half.
        assert_eq!(gain(&chips, &chips[0], 0, None, &[]), 0x80);
        assert_eq!(gain(&chips, &chips[1], 0, None, &[]), 0x80);

        let entry = |relative: bool, volume: u16| ExtraHeader {
            clocks: Vec::new(),
            volumes: vec![ExtraVolume {
                chip_id: ChipKind::Ym2612.id(),
                paired: false,
                second_instance: false,
                relative,
                volume,
            }],
        };
        // YM2612 halved to 0x80 (absolutely or relatively): the estimate
        // falls to 0x180, which normalises up once -- the named chip stays at
        // half while the other doubles to unity. The asked-for 1:2 balance
        // survives; the absolute level is the normaliser's business.
        let absolute = entry(false, 0x80);
        assert_eq!(gain(&chips, &chips[0], 0, Some(&absolute), &[]), 0x80);
        assert_eq!(gain(&chips, &chips[1], 0, Some(&absolute), &[]), 0x100);
        let relative = entry(true, 0x80);
        assert_eq!(gain(&chips, &chips[0], 0, Some(&relative), &[]), 0x80);
        assert_eq!(gain(&chips, &chips[1], 0, Some(&relative), &[]), 0x100);
    }

    /// The estimate counts every declared chip, cored here or not: the
    /// reference it mirrors starts them all. The ES5506 is the one chip this
    /// build has no core for, which is exactly why it is the fixture.
    #[test]
    fn the_estimate_counts_chips_this_build_cannot_play() {
        let chips = [declared(ChipKind::Ym2151), declared(ChipKind::Es5505)];
        // ES5506: V 0x20, PB 0x1000 -> contribution 0x200. YM2151: 0x100.
        // Sum 0x300 -> shift 0. Alone YM2151: 0x100 -> x2.
        assert_eq!(gain(&chips, &chips[0], 0, None, &[]), 0x80);
    }

    /// The unified `chip_volume(.., linked)` reproduces the reference's linked
    /// bases: the YM2203's SSG at half its FM (0x100/2), the rest at parity.
    #[test]
    fn linked_volumes_match_the_reference() {
        assert_eq!(
            chip_volume(&declared(ChipKind::Ym2203), 0, None, true),
            0x80
        );
        assert_eq!(
            chip_volume(&declared(ChipKind::Ym2608), 0, None, true),
            0x80
        );
        assert_eq!(
            chip_volume(&declared(ChipKind::Ym2610), 0, None, true),
            0x80
        );
        assert_eq!(
            chip_volume(&declared(ChipKind::Ymf278b), 0, None, true),
            0x100
        );
        // The main (unlinked) path is unchanged.
        assert_eq!(
            chip_volume(&declared(ChipKind::Ym2612), 0, None, false),
            0x100
        );
        // A chip with no linked child contributes nothing to the linked term.
        assert_eq!(linked_contribution(&declared(ChipKind::Ym2612), 0, None), 0);
    }

    /// The hoisted `mix_estimate` produces the exact whole-mix estimate the
    /// inline sum did, for the sets the voice-gain tests are pinned against.
    #[test]
    fn mix_estimate_matches_the_reference_for_known_sets() {
        // Black Knight: YM2612 0x100 + YM2151 0x100 + PWM 0xE0 = 0x2E0.
        let black_knight = [
            declared(ChipKind::Ym2612),
            declared(ChipKind::Ym2151),
            declared(ChipKind::Pwm),
        ];
        assert_eq!(mix_estimate(&black_knight, None, &[]), 0x2E0);
        // Cameltry: YM2203 0x100 + its SSG 0x80 + OKIM6295 0x200 = 0x380.
        let cameltry = [declared(ChipKind::Ym2203), declared(ChipKind::Okim6295)];
        assert_eq!(mix_estimate(&cameltry, None, &[]), 0x380);
        // A lone YM2151 (0x100) plus a declared K007232 tail (0x100) = 0x200.
        let lone = [declared(ChipKind::Ym2151)];
        assert_eq!(mix_estimate(&lone, None, &[0x2A]), 0x200);
    }

    /// The two per-core patches upstream applies after everything else.
    #[test]
    fn the_k051649_and_c140_patches_apply() {
        // K051649: 0xA0 * 8/5 = 0x100. C140: (0x100*2+1)/3 = 0xAB.
        assert_eq!(
            chip_volume(&declared(ChipKind::K051649), 0, None, false),
            0x100
        );
        assert_eq!(chip_volume(&declared(ChipKind::C140), 0, None, false), 0xAB);
    }
}
