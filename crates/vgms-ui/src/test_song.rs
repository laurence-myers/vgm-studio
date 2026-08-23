//! Songs for unit tests, built through `vgms-core`'s public constructors
//! (`vgms-core`'s own fixtures are `pub(crate)` to it).

use vgms_core::vgm::io::synthesise_header;
use vgms_core::{DroDataV1, DroDataV2, DroSong, OplType, VgmFile};

/// Wraps an OPL2 command stream in a real VGM container: a synthesised v1.51
/// header with the YM3812 clock (offset 0x50, spec-stable), the stream, an end
/// marker, then canonicalised through the reader/writer. VGM documents are held
/// as [`VgmFile`]s now, so the OPL test fixtures assemble bytes rather than a
/// VGM-flavoured `DroSong`.
fn assemble_opl2_vgm(name: &str, stream: &[u8]) -> Vec<u8> {
    let mut bytes = synthesise_header();
    bytes[0x50..0x54].copy_from_slice(&3_579_545u32.to_le_bytes());
    bytes.extend_from_slice(stream);
    bytes.push(0x66); // end marker
    let eof = (bytes.len() - 0x04) as u32;
    bytes[0x04..0x08].copy_from_slice(&eof.to_le_bytes());
    let file = vgms_core::vgm::file::read(name, &bytes).expect("a walkable OPL VGM");
    vgms_core::vgm::file::write(&file).expect("the OPL VGM writes back")
}

/// A 300 ms OPL2 tone: instruments, key-on, 200 ms of sound, key-off, 100 ms of
/// silence. Same stream as `vgms-synth`'s waveform test song.
pub(crate) fn tone_song() -> DroSong {
    DroSong::dro_v1(
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
pub(crate) fn dual_tone_song() -> DroSong {
    DroSong::dro_v1(
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
pub(crate) fn paced_song() -> DroSong {
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
    DroSong::dro_v1(
        "paced.dro".to_owned(),
        DroDataV1::new(data).unwrap(),
        600,
        OplType::Opl2,
    )
}

/// The `vgms-core` v2 fixture rebuilt via public constructors: five register
/// writes, a short delay (177 ms), a long delay (49408 ms), then the same
/// fourteen instructions again. Total delay 99170 ms.
pub(crate) fn dro_song_v2() -> DroSong {
    let mut data: Vec<u8> = (0..10).collect();
    data.extend_from_slice(&[0xFE, 0xB0, 0xFF, 0xC0]);
    data.extend_from_within(..);
    DroSong::dro_v2(
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
/// at 1 and 2) and the delays they separate to merge.
pub(crate) fn redundant_vgm_bytes() -> Vec<u8> {
    assemble_opl2_vgm(
        "redundant.vgm",
        &[
            0x5A, 0x20, 0x01, // write
            0x5A, 0x40, 0x10, // write (operator level)
            0x5A, 0xB0, 0x31, // key on
            0x61, 0x64, 0x00, // wait 100
            0x5A, 0x40, 0x10, // redundant -- same operator level
            0x5A, 0xB0, 0x31, // redundant -- key already on
            0x61, 0xC8, 0x00, // wait 200
            0x5A, 0xB0, 0x11, // key off
            0x61, 0x64, 0x00, // wait 100
        ],
    )
}

/// [`redundant_vgm_bytes`] read as a [`VgmFile`], the way the editor holds it.
pub(crate) fn redundant_vgm_file() -> VgmFile {
    vgms_core::vgm::file::read("redundant.vgm", &redundant_vgm_bytes()).unwrap()
}

/// An OPL2 VGM with a run of six consecutive waits between register writes, so
/// the instruction table's folding has a run to collapse.
pub(crate) fn folding_vgm() -> VgmFile {
    let bytes = assemble_opl2_vgm(
        "fold.vgm",
        &[
            0x5A, 0x20, 0x01, // write (index 0)
            0x5A, 0x40, 0x10, // write (index 1)
            0x70, 0x70, 0x70, 0x70, 0x70, 0x70, // six waits (indices 2..=7)
            0x5A, 0xB0, 0x31, // write (index 8)
        ],
    );
    vgms_core::vgm::file::read("fold.vgm", &bytes).unwrap()
}

/// An OPL2 VGM standing in for a whole sound-test session logged in one file:
/// three short "songs" parted by two one-second (44100-sample) silent gaps, well
/// over the 0.75 s default split threshold. Each song sets a distinct register
/// before its note, so a later piece's state-replay prelude has earlier writes to
/// restore. Named `*.vgm` so it round-trips through the VGM writer.
pub(crate) fn multi_song_capture_bytes() -> Vec<u8> {
    // 44100 samples = 0xAC44 (one second); 4410 = 0x113A (a tenth).
    let gap = [0x61, 0x44, 0xAC];
    let short = [0x61, 0x3A, 0x11];
    let mut stream = Vec::new();
    // song 1
    stream.extend_from_slice(&[0x5A, 0x20, 0x01, 0x5A, 0x40, 0x10, 0x5A, 0xB0, 0x31]);
    stream.extend_from_slice(&short);
    stream.extend_from_slice(&[0x5A, 0xB0, 0x11]);
    stream.extend_from_slice(&gap);
    // song 2
    stream.extend_from_slice(&[0x5A, 0x21, 0x02, 0x5A, 0xB1, 0x32]);
    stream.extend_from_slice(&short);
    stream.extend_from_slice(&[0x5A, 0xB1, 0x12]);
    stream.extend_from_slice(&gap);
    // song 3
    stream.extend_from_slice(&[0x5A, 0x22, 0x03, 0x5A, 0xB2, 0x33]);
    stream.extend_from_slice(&short);
    stream.extend_from_slice(&[0x5A, 0xB2, 0x13]);

    assemble_opl2_vgm("capture.vgm", &stream)
}

/// [`multi_song_capture_bytes`] read as a [`VgmFile`], the way the editor holds it.
pub(crate) fn multi_song_capture() -> VgmFile {
    vgms_core::vgm::file::read("capture.vgm", &multi_song_capture_bytes()).unwrap()
}

/// A DRO v2 stand-in for a sound-test session: three short songs parted by two
/// ~1024 ms silent gaps (over the 0.75 s = 750 ms default split threshold). Each
/// song sets a distinct register before its note, so a later piece's state-replay
/// prelude has earlier writes to restore. The DRO counterpart to
/// [`multi_song_capture`].
pub(crate) fn multi_song_capture_dro() -> DroSong {
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

    DroSong::dro_v2(
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
pub(crate) fn looping_vgm_bytes() -> Vec<u8> {
    fn write(bytes: &mut Vec<u8>, reg: u8, value: u8) {
        bytes.extend_from_slice(&[0x5A, reg, value]);
    }
    let mut stream = Vec::new();
    // intro: two writes and a half-second delay (22050 samples)
    write(&mut stream, 0x20, 0x01);
    write(&mut stream, 0x40, 0x10);
    stream.extend_from_slice(&[0x61, 0x22, 0x56]);
    // body, twice: distinct writes parted by quarter-second delays (11025 samples),
    // so one loop body spans half a second.
    for _ in 0..2 {
        write(&mut stream, 0xA0, 0x11);
        stream.extend_from_slice(&[0x61, 0x11, 0x2B]);
        write(&mut stream, 0xB0, 0x22);
        write(&mut stream, 0xA3, 0x33);
        stream.extend_from_slice(&[0x61, 0x11, 0x2B]);
        write(&mut stream, 0xC0, 0x44);
    }
    assemble_opl2_vgm("looping.vgm", &stream)
}

/// [`looping_vgm_bytes`] read as a [`VgmFile`], the way the editor holds it.
pub(crate) fn looping_vgm() -> VgmFile {
    vgms_core::vgm::file::read("looping.vgm", &looping_vgm_bytes()).unwrap()
}

/// A DRO v2 song whose first instruction is a delay and whose header length
/// disagrees with the summed delays -- both load-time warnings at once.
pub(crate) fn bogus_leading_delay_song() -> DroSong {
    DroSong::dro_v2(
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
