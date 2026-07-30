//! Songs for unit tests, built through `vgms-core`'s public constructors
//! (`vgms-core`'s own fixtures are `pub(crate)` to it).

use vgms_core::vgm::io::synthesise_header;
use vgms_core::{DroDataV1, DroDataV2, OplType, Song, VgmData, VgmMeta};

/// A 300 ms OPL2 tone: instruments, key-on, 200 ms of sound, key-off, 100 ms of
/// silence. Same stream as `vgms-synth`'s waveform test song.
pub(crate) fn tone_song() -> Song {
    Song::dro_v1(
        "tone.dro".to_owned(),
        DroDataV1::new(vec![
            0x20, 0x01, 0x40, 0x10, 0x60, 0xF0, 0x80, 0x7F, // modulator (fast release)
            0x23, 0x01, 0x43, 0x00, 0x63, 0xF0, 0x83, 0x7F, // carrier (fast release)
            0xA0, 0x98, 0xB0, 0x31, // frequency, key on
            0x00, 0xC7, // 200 ms of tone
            0xB0, 0x11, // key off
            0x00, 0x63, // 100 ms of silence
        ])
        .unwrap(),
        300,
        OplType::Opl2,
    )
}

/// The tone song as a dual-OPL2 file, so the strip's fixed hard-L/R Original
/// panning image can be exercised. The instruction stream is identical to
/// [`tone_song`]; only the declared chip type differs.
pub(crate) fn dual_tone_song() -> Song {
    Song::dro_v1(
        "dual.dro".to_owned(),
        DroDataV1::new(vec![
            0x20, 0x01, 0x40, 0x10, 0x60, 0xF0, 0x80, 0x7F, // modulator (fast release)
            0x23, 0x01, 0x43, 0x00, 0x63, 0xF0, 0x83, 0x7F, // carrier (fast release)
            0xA0, 0x98, 0xB0, 0x31, // frequency, key on
            0x00, 0xC7, // 200 ms of tone
            0xB0, 0x11, // key off
            0x00, 0x63, // 100 ms of silence
        ])
        .unwrap(),
        300,
        OplType::DualOpl2,
    )
}

/// Six 50 ms tone bursts, each followed by 50 ms of silence -- 600 ms in all,
/// with a delay every other instruction.
///
/// [`tone_song`]'s delays sit only at its very end, so every one of its first ten
/// instructions shares a timestamp of zero and any marked region among them
/// collapses onto the left edge. This one carries time across its whole width,
/// which is what a range drawn over the waveform needs to be visible at all.
pub(crate) fn paced_song() -> Song {
    let mut data = vec![
        0x20, 0x01, 0x40, 0x10, 0x60, 0xF0, 0x80, 0x7F, // modulator (fast release)
        0x23, 0x01, 0x43, 0x00, 0x63, 0xF0, 0x83, 0x7F, // carrier (fast release)
        0xA0, 0x98, // frequency
    ];
    for _ in 0..6 {
        data.extend_from_slice(&[
            0xB0, 0x31, // key on
            0x00, 0x31, // 50 ms of tone
            0xB0, 0x11, // key off
            0x00, 0x31, // 50 ms of silence
        ]);
    }
    Song::dro_v1(
        "paced.dro".to_owned(),
        DroDataV1::new(data).unwrap(),
        600,
        OplType::Opl2,
    )
}

/// The `vgms-core` v2 fixture rebuilt via public constructors: five register
/// writes, a short delay (177 ms), a long delay (49408 ms), then the same
/// fourteen instructions again. Total delay 99170 ms.
pub(crate) fn dro_song_v2() -> Song {
    let mut data: Vec<u8> = (0..10).collect();
    data.extend_from_slice(&[0xFE, 0xB0, 0xFF, 0xC0]);
    data.extend_from_within(..);
    Song::dro_v2(
        "test.dro".to_owned(),
        DroDataV2::new(
            data,
            vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0],
            0xFE,
            0xFF,
        )
        .unwrap(),
        99_170,
        OplType::Opl3,
    )
}

/// A small OPL2 VGM carrying redundant register writes between its delays, so
/// the optimiser has both writes to strip (indices 4 and 5 repeat the values set
/// at 1 and 2) and the delays they separate to merge. Named `*.vgm` so it
/// round-trips through the VGM writer when a test opens it.
pub(crate) fn redundant_vgm_song() -> Song {
    let bytes = vec![
        0x5A, 0x20, 0x01, // 0: write
        0x5A, 0x40, 0x10, // 1: write (operator level)
        0x5A, 0xB0, 0x31, // 2: key on
        0x61, 0x64, 0x00, // 3: wait 100
        0x5A, 0x40, 0x10, // 4: redundant -- same operator level
        0x5A, 0xB0, 0x31, // 5: redundant -- key already on
        0x61, 0xC8, 0x00, // 6: wait 200
        0x5A, 0xB0, 0x11, // 7: key off
        0x61, 0x64, 0x00, // 8: wait 100
    ];
    Song::vgm(
        "redundant.vgm".to_owned(),
        0x151,
        VgmData::new(bytes).unwrap(),
        OplType::Opl2,
        VgmMeta::new(synthesise_header()),
    )
}

/// An OPL2 VGM standing in for a whole sound-test session logged in one file:
/// three short "songs" parted by two one-second (44100-sample) silent gaps, well
/// over the 0.75 s default split threshold. Each song sets a distinct register
/// before its note, so a later piece's state-replay prelude has earlier writes to
/// restore. Named `*.vgm` so it round-trips through the VGM writer.
pub(crate) fn multi_song_capture() -> Song {
    // 44100 samples = 0xAC44 (one second); 4410 = 0x113A (a tenth).
    let gap = [0x61, 0x44, 0xAC];
    let short = [0x61, 0x3A, 0x11];
    let mut bytes = Vec::new();
    // song 1
    bytes.extend_from_slice(&[0x5A, 0x20, 0x01, 0x5A, 0x40, 0x10, 0x5A, 0xB0, 0x31]);
    bytes.extend_from_slice(&short);
    bytes.extend_from_slice(&[0x5A, 0xB0, 0x11]);
    bytes.extend_from_slice(&gap);
    // song 2
    bytes.extend_from_slice(&[0x5A, 0x21, 0x02, 0x5A, 0xB1, 0x32]);
    bytes.extend_from_slice(&short);
    bytes.extend_from_slice(&[0x5A, 0xB1, 0x12]);
    bytes.extend_from_slice(&gap);
    // song 3
    bytes.extend_from_slice(&[0x5A, 0x22, 0x03, 0x5A, 0xB2, 0x33]);
    bytes.extend_from_slice(&short);
    bytes.extend_from_slice(&[0x5A, 0xB2, 0x13]);

    Song::vgm(
        "capture.vgm".to_owned(),
        0x151,
        VgmData::new(bytes).unwrap(),
        OplType::Opl2,
        VgmMeta::new(synthesise_header()),
    )
}

/// A DRO v2 stand-in for a sound-test session: three short songs parted by two
/// ~1024 ms silent gaps (over the 0.75 s = 750 ms default split threshold). Each
/// song sets a distinct register before its note, so a later piece's state-replay
/// prelude has earlier writes to restore. The DRO counterpart to
/// [`multi_song_capture`].
pub(crate) fn multi_song_capture_dro() -> Song {
    // Codemap slots: 0x20, 0x40, 0xB0, 0x21, 0xB1, 0x22, 0xB2.
    let codemap = vec![0x20, 0x40, 0xB0, 0x21, 0xB1, 0x22, 0xB2];
    let (short, long) = (0xFE, 0xFF);
    // A 100 ms short delay is `[short, 99]`; a 1024 ms long delay is `[long, 3]`
    // -> (3 + 1) << 8.
    let mut data = Vec::new();
    data.extend_from_slice(&[0, 0x01, 1, 0x10, short, 99, 2, 0x31]); // song 1
    data.extend_from_slice(&[long, 3]); // gap
    data.extend_from_slice(&[3, 0x02, short, 99, 4, 0x32]); // song 2
    data.extend_from_slice(&[long, 3]); // gap
    data.extend_from_slice(&[5, 0x03, short, 99, 6, 0x33]); // song 3

    Song::dro_v2(
        "capture.dro".to_owned(),
        DroDataV2::new(data, codemap, short, long).unwrap(),
        2348, // 3 x 100 ms + 2 x 1024 ms
        OplType::Opl2,
    )
}

/// An OPL2 VGM whose four-write body plays twice in a row after a short intro,
/// so the loop finder has a clean repeat to discover. The body's registers are
/// distinct, so only the whole-body repeat matches: the search reports one loop
/// running from the body's first write (instruction 3) to the repeat's (9).
pub(crate) fn looping_vgm() -> Song {
    fn write(bytes: &mut Vec<u8>, reg: u8, value: u8) {
        bytes.extend_from_slice(&[0x5A, reg, value]);
    }
    let mut bytes = Vec::new();
    // intro: two writes and a half-second delay (22050 samples)
    write(&mut bytes, 0x20, 0x01);
    write(&mut bytes, 0x40, 0x10);
    bytes.extend_from_slice(&[0x61, 0x22, 0x56]);
    // body, twice: distinct writes parted by quarter-second delays (11025 samples),
    // so one loop body spans half a second.
    for _ in 0..2 {
        write(&mut bytes, 0xA0, 0x11);
        bytes.extend_from_slice(&[0x61, 0x11, 0x2B]);
        write(&mut bytes, 0xB0, 0x22);
        write(&mut bytes, 0xA3, 0x33);
        bytes.extend_from_slice(&[0x61, 0x11, 0x2B]);
        write(&mut bytes, 0xC0, 0x44);
    }
    Song::vgm(
        "looping.vgm".to_owned(),
        0x151,
        VgmData::new(bytes).unwrap(),
        OplType::Opl2,
        VgmMeta::new(synthesise_header()),
    )
}

/// A DRO v2 song whose first instruction is a delay and whose header length
/// disagrees with the summed delays -- both load-time warnings at once.
pub(crate) fn bogus_leading_delay_song() -> Song {
    Song::dro_v2(
        "bogus.dro".to_owned(),
        DroDataV2::new(
            vec![
                0xFE, 0x63, // 100 ms short delay -- the bogus leader
                0x00, 0x01, // register write
                0xFE, 0xC7, // 200 ms short delay
            ],
            vec![0x20],
            0xFE,
            0xFF,
        )
        .unwrap(),
        999, // header lies: the real total is 300 (200 after the auto-trim)
        OplType::Opl3,
    )
}
