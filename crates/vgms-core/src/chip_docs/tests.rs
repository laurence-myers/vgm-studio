use std::borrow::Cow;

use super::*;
use crate::regdata;
use crate::vgm::stream::END_OF_DATA;

/// The OPL restatement cannot drift from `regdata`: every kind's name and
/// every field's description and mask must match, both ways.
#[test]
fn the_opl_docs_mirror_regdata_exactly() {
    for reg in 0u16..=0x1FF {
        let Some(kind) = regdata::register_kind(reg) else {
            continue;
        };
        let doc = opl::doc_for_kind(kind);
        assert_eq!(doc.name, kind.description(), "{kind:?} name");
        let masks = kind.bitmasks();
        assert_eq!(doc.fields.len(), masks.len(), "{kind:?} field count");
        for (field, mask) in doc.fields.iter().zip(masks) {
            assert_eq!(field.description, mask.description, "{kind:?}");
            assert_eq!(field.mask, u16::from(mask.mask), "{kind:?}");
        }
    }
}

/// The lookup honours the OPL bank precedence and the chip's generation: the
/// OPL3-only registers exist only for the YMF262.
#[test]
fn opl_lookup_respects_bank_and_generation() {
    use crate::vgm::ChipKind as K;
    assert_eq!(
        register_doc(K::Ymf262, 1, 0x05).unwrap().name,
        "OPL3 Mode Enable"
    );
    assert_eq!(
        register_doc(K::Ymf262, 1, 0xB0).unwrap().name,
        "Key On / Octave / Frequency (high 2 bits)"
    );
    assert!(register_doc(K::Ym3812, 1, 0x05).is_none(), "no high bank");
    assert_eq!(
        register_doc(K::Y8950, 0, 0x0F).unwrap().name,
        "ADPCM: data",
        "the Y8950's ADPCM section takes precedence"
    );
}

/// A few documented cells across the roster, pinned so the dispatch cannot
/// silently lose a chip.
#[test]
fn documented_chips_answer() {
    use crate::vgm::ChipKind as K;
    for (chip, port, addr) in [
        (K::Ym2612, 0, 0x28),
        (K::Ym2612, 1, 0xB4),
        (K::Ym2413, 0, 0x0E),
        (K::Ym2151, 0, 0x08),
        (K::Ym2203, 0, 0x28),
        (K::Ym2608, 0, 0x10),
        (K::Ym2610, 1, 0x00),
        (K::Ay8910, 0, 0x07),
        (K::GameBoyDmg, 0, 0x16),
        (K::NesApu, 0, 0x15),
        (K::HuC6280, 0, 0x04),
    ] {
        assert!(
            register_doc(chip, port, addr).is_some(),
            "{chip:?} port {port} addr {addr:#04X}"
        );
    }
    // Undocumented chips stay undocumented -- the fallback wording is theirs.
    assert!(register_doc(K::Pokey, 0, 0x00).is_none());
    assert!(register_doc(K::Scsp, 0, 0x00).is_none());
}

/// The find dropdown's lists: present for the documented chips (the SN76489
/// has no addresses to list, the Sega PCM's are channel-relative).
#[test]
fn notable_lists_exist_for_documented_chips() {
    use crate::vgm::ChipKind as K;
    for chip in [
        K::Ym2612,
        K::Ym2413,
        K::Ym2151,
        K::Ym2203,
        K::Ym2608,
        K::Ym2610,
        K::Ay8910,
        K::GameBoyDmg,
        K::NesApu,
        K::HuC6280,
        // Every OPL chip, not just OPL3: the shared NOTABLE list must be filtered
        // so each two-operator chip lists only registers it actually documents.
        K::Ym3812,
        K::Ym3526,
        K::Y8950,
        K::Ymf262,
    ] {
        let notable = documented_registers(chip);
        assert!(!notable.is_empty(), "{chip:?}");
        for &(port, addr, name) in &notable {
            assert!(
                register_doc(chip, port, addr).is_some(),
                "{chip:?}'s notable {name:?} has no doc"
            );
        }
    }
    assert!(documented_registers(K::Sn76489).is_empty());
    assert!(documented_registers(K::Pokey).is_empty());
}

/// Builds a stream from commands + the end marker.
fn stream(bytes: &[u8]) -> VgmStream {
    let mut data = bytes.to_vec();
    data.push(END_OF_DATA);
    VgmStream::parse(data, 0x171).unwrap()
}

/// The analyser's first-write / changed-bits / no-change wording matches the
/// OPL analyser's rules.
#[test]
fn the_analyzer_diffs_fields_like_the_opl_one() {
    let stream = stream(&[
        0x52, 0x28, 0xF0, // key on, ch 0, all operators
        0x52, 0x28, 0xF0, // the same value again
        0x52, 0x28, 0xF1, // only the channel bits change
    ]);
    let mut analyzer = ChipAnalyzer::new();
    assert_eq!(
        analyzer.row(&stream, 0).unwrap(),
        "Operator on/off mask / Channel",
        "a first write counts every field"
    );
    assert_eq!(analyzer.row(&stream, 1).unwrap(), "(no changes)");
    assert_eq!(analyzer.row(&stream, 2).unwrap(), "Channel");
}

/// Dual chips do not share state: the same write to instance 2 is a first
/// write for instance 2.
#[test]
fn instances_keep_separate_state() {
    let stream = stream(&[
        0x52, 0xB0, 0x3A, // instance 1: feedback / algorithm
        0xA2, 0xB0, 0x3A, // instance 2, same register, same value
    ]);
    let mut analyzer = ChipAnalyzer::new();
    assert_eq!(analyzer.row(&stream, 0).unwrap(), "Feedback / Algorithm");
    assert_eq!(
        analyzer.row(&stream, 1).unwrap(),
        "Feedback / Algorithm",
        "instance 2 has never seen this register"
    );
}

/// The SN76489's latch travels in the data: a latch byte names its register,
/// a data byte names whatever was latched, per instance.
#[test]
fn sn76489_latch_decoding() {
    let stream = stream(&[
        0x50, 0x8E, // latch tone 1 frequency, low nibble
        0x50, 0x0F, // data byte extending it
        0x50, 0x9F, // tone 1 attenuation
        0x30, 0x0F, // second chip: data byte with no latch yet
        0x4F, 0x03, // GG stereo
    ]);
    let mut analyzer = ChipAnalyzer::new();
    assert_eq!(
        analyzer.row(&stream, 0).unwrap(),
        "Tone 1 frequency (latch + low 4 bits)"
    );
    assert_eq!(
        analyzer.row(&stream, 1).unwrap(),
        "Tone 1 frequency (high 6 bits)"
    );
    assert_eq!(analyzer.row(&stream, 2).unwrap(), "Tone 1 attenuation");
    assert_eq!(
        analyzer.row(&stream, 3).unwrap(),
        "Data byte (no register latched)",
        "the second chip's latch is its own"
    );
    assert_eq!(
        analyzer.row(&stream, 4).unwrap(),
        "Game Gear stereo enables (L/R per channel)"
    );
}

/// Undocumented commands advance the replay without a description, and a
/// backwards query replays from the start.
#[test]
fn fallback_rows_and_backwards_queries() {
    let stream = stream(&[
        0xBB, 0x00, 0x11, // Pokey: undocumented
        0x61, 0x10, 0x27, // a delay
        0x52, 0x28, 0xF0, // documented
        0x52, 0x28, 0xF0, // the same value
    ]);
    let mut analyzer = ChipAnalyzer::new();
    assert_eq!(analyzer.row(&stream, 0), None, "undocumented chip");
    assert_eq!(analyzer.row(&stream, 1), None, "a delay is not a write");
    assert!(analyzer.row(&stream, 2).is_some());
    assert_eq!(analyzer.row(&stream, 3).unwrap(), "(no changes)");
    // Backwards: the replay restarts, so row 2 is a first write again.
    assert_eq!(
        analyzer.row(&stream, 2).unwrap(),
        "Operator on/off mask / Channel"
    );
    assert_eq!(analyzer.row(&stream, 4), None, "out of range");
}

/// The single-changed-field fast path borrows; the multi-field path allocates.
#[test]
fn borrowing_matches_the_field_count() {
    let stream = stream(&[
        0x52, 0x28, 0x01, // first write: two fields -> owned
        0x52, 0x28, 0x02, // channel bits only -> borrowed
    ]);
    let mut analyzer = ChipAnalyzer::new();
    assert!(matches!(analyzer.row(&stream, 0).unwrap(), Cow::Owned(_)));
    assert!(matches!(
        analyzer.row(&stream, 1).unwrap(),
        Cow::Borrowed(_)
    ));
}
