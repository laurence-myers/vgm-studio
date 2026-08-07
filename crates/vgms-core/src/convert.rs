//! Format conversions: DRO -> VGM and DRO v2 -> v1.

use crate::error::{Error, Result};
use crate::song::dro_data::v1_opcode;
use crate::song::{
    Bank, DRO_FILE_V1, DelayKind, DroDataV1, DroSong, DroSongData, Instruction, OplType,
};
use crate::util::VGM_SAMPLE_RATE;
use crate::vgm::VgmFile;
use crate::vgm::data::command;
use crate::vgm::header::offset;
use crate::vgm::io::{put_chip_clocks, synthesise_header};

/// The longest wait a single `0x61` command can express. Only the test that a
/// long delay spans several commands names it now; the chunking itself lives in
/// [`vgm::data::append_wait`](crate::vgm::data).
#[cfg(test)]
const MAX_WAIT_SAMPLES: u64 = 0xFFFF;

/// Milliseconds to samples, rounding the *running total* half up.
///
/// Two identical 16 ms delays in the `dro2vgm` reference become 706 and 705
/// samples, which no per-delay rounding can produce. Seeding the carry at half a
/// millisecond makes each emitted count `round(cumulative_ms * 44.1)` minus its
/// predecessor.
#[derive(Debug)]
struct SampleClock {
    carry: u64,
}

impl SampleClock {
    const fn new() -> Self {
        Self { carry: 500 }
    }

    fn samples_for_ms(&mut self, ms: u32) -> u64 {
        let numerator = u64::from(ms) * u64::from(VGM_SAMPLE_RATE) + self.carry;
        self.carry = numerator % 1000;
        numerator / 1000
    }
}

/// Accumulates a VGM command stream: register writes for one chip, and waits
/// chunked to fit `0x61`'s 16-bit operand.
///
/// Used by [`dro_to_vgm`], whose millisecond delays arrive through a
/// [`SampleClock`].
#[derive(Debug)]
struct VgmStream {
    opl_type: OplType,
    out: Vec<u8>,
    /// Commands emitted so far, a VGM instruction count.
    count: usize,
}

impl VgmStream {
    fn with_capacity(opl_type: OplType, bytes: usize) -> Self {
        Self {
            opl_type,
            out: Vec::with_capacity(bytes),
            count: 0,
        }
    }

    fn write(&mut self, bank: Bank, reg: u8, value: u8) {
        self.out
            .extend_from_slice(&[write_command(self.opl_type, bank), reg, value]);
        self.count += 1;
    }

    /// Emits `samples` as one or more `0x61` waits. A zero-sample wait still
    /// emits one command, so a delay never vanishes from the stream.
    fn wait(&mut self, samples: u64) {
        if samples == 0 {
            self.out.extend_from_slice(&[command::WAIT, 0, 0]);
            self.count += 1;
        } else {
            self.count += crate::vgm::data::append_wait(&mut self.out, samples);
        }
    }

    /// The raw command bytes, for a caller that assembles the VGM container
    /// itself ([`dro_to_vgm`]) rather than wrapping them in a `DroSong`.
    fn into_bytes(self) -> Vec<u8> {
        self.out
    }
}

/// Writes a little-endian `u32` at `offset` into a header buffer.
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Converts a DRO song to a playable [`VgmFile`].
///
/// The DRO's register writes and millisecond delays become a v1.51 VGM command
/// stream (delays in samples, chunked to fit `0x61`); the container is assembled
/// here -- a synthesised header with the OPL chip clocks, total-sample count and
/// EOF patched in, then the stream and an end marker -- and read straight back
/// through [`vgm::file::read`](crate::vgm::file::read), so the result is a real
/// `VgmFile` rather than a VGM-flavoured `DroSong`. That assembled byte image is
/// exactly what `dro2vgm` emits (pinned byte-for-byte against the fixture), which
/// is why the conversion carries no `DroSong::vgm` intermediate.
///
/// # Errors
/// If the synthesised header cannot hold the OPL clocks, or the assembled bytes
/// do not read back. (A `DroSong` is always a DRO, so there is no "already a
/// VGM" case.)
pub fn dro_to_vgm(song: &DroSong) -> Result<VgmFile> {
    // Byte-exact to `dro2vgm`: no synthetic prime (see [`dro_to_vgm_inner`]).
    dro_to_vgm_inner(song, false)
}

/// The shared DRO -> VGM conversion, optionally priming the DRO v1
/// waveform-select-enable register the way [`DroEngine`](../../vgms_synth) does on
/// reset.
///
/// `prime_wse` is `false` for [`dro_to_vgm`] (which must stay byte-for-byte with
/// `dro2vgm`) and `true` only for a v1 source through the playback/split
/// projection ([`opl_song_to_vgm_file`]).
///
/// # Errors
/// If the synthesised header cannot hold the OPL clocks, or the assembled bytes
/// do not read back.
fn dro_to_vgm_inner(song: &DroSong, prime_wse: bool) -> Result<VgmFile> {
    let mut clock = SampleClock::new();
    let mut bank = Bank::Low;
    let mut stream = VgmStream::with_capacity(song.opl_type, song.len() * 3);
    // The header's `total # samples` field; accumulated here because the stream
    // holds only the (chunked) wait commands, not their sum.
    let mut total_samples = 0u64;

    // DRO v1 (OPL2) captures assume waveform-select is already enabled
    // (`0x01 = 0x20`): DOSBox's chip had the bit set before recording began, so
    // the write is not in the file. `DroEngine::reset_chip` primes it for the
    // offline render; the playback projection primes it here, or a v1 file's
    // non-sine timbres collapse to sine. A zero-delay low-bank write, so the
    // timing and total-sample count are unchanged.
    if prime_wse {
        stream.write(Bank::Low, 0x01, 0x20);
    }

    for instruction in song.data().iter() {
        match instruction {
            Instruction::BankSwitch(selected) => bank = selected, // DRO v1
            Instruction::Register {
                reg,
                value,
                bank: own,
            } => {
                // DRO v2 carries the bank on the write; v1 tracks it separately.
                stream.write(own.unwrap_or(bank), reg, value);
            }
            Instruction::DelayMs { ms, .. } => {
                let samples = clock.samples_for_ms(ms);
                total_samples = total_samples.saturating_add(samples);
                stream.wait(samples);
            }
            Instruction::DelaySamples { .. } => {
                unreachable!("a DRO song has no sample delays")
            }
        }
    }

    // Assemble the VGM container. A converted file has no loop and no GD3, so the
    // synthesised header's zeroed loop/GD3/modifier fields are already correct;
    // only the chip clocks, total samples and EOF need patching -- the same
    // fields `vgm::io::write` would patch for this file.
    let mut header = synthesise_header();
    put_chip_clocks(&mut header, song.opl_type)?;
    let data = stream.into_bytes();
    let end_marker = 1;
    let eof = header.len() + data.len() + end_marker;
    put_u32(&mut header, offset::EOF, (eof - offset::EOF) as u32);
    put_u32(
        &mut header,
        offset::TOTAL_SAMPLES,
        u32::try_from(total_samples).unwrap_or(u32::MAX),
    );

    let mut out = header;
    out.extend_from_slice(&data);
    out.push(command::END);
    crate::vgm::file::read(&replace_extension(&song.name, "vgm"), &out)
}

/// Projects an OPL document (a DRO) to a playable [`VgmFile`], for routing OPL
/// playback through the multichip [`VgmEngine`](../../vgms_synth/vgm_engine)
/// (ou-2).
///
/// The register writes and millisecond delays become a real `VgmFile` whose
/// header the generic engine builds voices from -- the same round trip
/// [`Editor::convert_to_vgm`](../../vgms_ui) makes, done at play time rather than
/// on a user's explicit convert.
///
/// Unlike that explicit convert ([`dro_to_vgm`]), this primes the DRO v1
/// waveform-select-enable register (`0x01 = 0x20`): a v1 (OPL2) capture assumes
/// the bit was set before it ran, so without the prime its non-sine timbres
/// collapse to sine on playback -- the projection must match what the offline
/// render ([`DroEngine::reset_chip`](../../vgms_synth)) does. A v2 capture records
/// the write itself and is not primed.
///
/// # Errors
/// If the song will not convert, or the assembled VGM does not read back.
pub fn opl_song_to_vgm_file(song: &DroSong) -> Result<VgmFile> {
    dro_to_vgm_inner(song, song.file_version == DRO_FILE_V1)
}

/// The VGM opcode that writes an OPL register on the given chip and bank.
const fn write_command(opl_type: OplType, bank: Bank) -> u8 {
    match (opl_type, bank) {
        // A single OPL2 has one bank; the high bank cannot be addressed.
        (OplType::Opl2, _) | (OplType::DualOpl2, Bank::Low) => command::YM3812,
        (OplType::DualOpl2, Bank::High) => command::YM3812_CHIP_2,
        (OplType::Opl3, Bank::Low) => command::YMF262_PORT_0,
        (OplType::Opl3, Bank::High) => command::YMF262_PORT_1,
    }
}

/// Converts a DRO v2 song to DRO v1.
///
/// v1 has no per-write bank bit, so a bank switch instruction is emitted whenever
/// the bank changes. v2 files usually alternate banks, so the result is
/// bank-switch heavy; grouping the writes would be a better conversion.
///
/// # Errors
/// If `song` is not a DRO v2 song, or a delay does not fit v1's encoding.
pub fn dro2_to_dro1(song: &DroSong) -> Result<DroSong> {
    let DroSongData::V2(_) = song.data() else {
        return Err(Error::file(
            "Only DRO v2 files can be converted to DRO v1".to_owned(),
        ));
    };

    let mut out: Vec<u8> = Vec::with_capacity(song.len() * 2);
    let mut bank = Bank::Low; // DRO v1 chips start on the low bank

    for instruction in song.data().iter() {
        match instruction {
            Instruction::DelayMs {
                kind: DelayKind::Short,
                ms,
            } => {
                let value = u8::try_from(ms - 1).map_err(|_| {
                    Error::file(format!("Short delay of {ms} ms does not fit DRO v1"))
                })?;
                out.extend_from_slice(&[v1_opcode::SHORT_DELAY, value]);
            }
            Instruction::DelayMs {
                kind: DelayKind::Long,
                ms,
            } => {
                let value = u16::try_from(ms - 1).map_err(|_| {
                    Error::file(format!("Long delay of {ms} ms does not fit DRO v1"))
                })?;
                out.push(v1_opcode::LONG_DELAY);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Instruction::Register {
                reg,
                value,
                bank: own,
            } => {
                let effective = own.unwrap_or(bank);
                if effective != bank {
                    out.push(match effective {
                        Bank::Low => v1_opcode::BANK_LOW,
                        Bank::High => v1_opcode::BANK_HIGH,
                    });
                    bank = effective;
                }
                // Registers that collide with v1's opcodes (0x00..=ESCAPE) are
                // escaped with the ESCAPE opcode.
                if reg <= v1_opcode::ESCAPE {
                    out.extend_from_slice(&[v1_opcode::ESCAPE, reg, value]);
                } else {
                    out.extend_from_slice(&[reg, value]);
                }
            }
            Instruction::BankSwitch(_) | Instruction::DelaySamples { .. } => {
                unreachable!("a DRO v2 song has neither bank switches nor sample delays")
            }
        }
    }

    Ok(DroSong::dro_v1(
        song.name.clone(),
        DroDataV1::new(out)?,
        song.ms_length,
        song.opl_type,
    ))
}

/// The name a converted DRO v1 takes: `song.dro` becomes `song_1.dro`.
///
/// So a Save As after converting suggests the new name rather than offering to
/// overwrite the v2 source.
#[must_use]
pub fn dro1_default_name(name: &str) -> String {
    match name.rfind('.') {
        // Only a plausible extension, matching `replace_extension`'s rule; a dot
        // in the middle of a name is not a suffix to insert before.
        Some(dot) if matches!(name.len() - dot - 1, 3 | 4) => {
            format!("{}_1{}", &name[..dot], &name[dot..])
        }
        _ => format!("{name}_1"),
    }
}

/// Swaps a three- or four-character extension for `extension`, or appends one.
fn replace_extension(name: &str, extension: &str) -> String {
    match name.rfind('.') {
        Some(dot) if matches!(name.len() - dot - 1, 3 | 4) => {
            format!("{}.{extension}", &name[..dot])
        }
        _ => format!("{name}.{extension}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::dro;

    const DRO_V2_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up_dro2.dro");
    const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

    /// The strongest test in the crate.
    ///
    /// `lsl3_score_up.vgm` was produced from `lsl3_score_up_dro2.dro` by `dro2vgm`,
    /// an entirely independent tool. Reproducing it byte for byte exercises the DRO
    /// v2 reader, the instruction decoder, the sample clock, and the VGM writer at
    /// once -- against an oracle none of them has ever seen.
    #[test]
    fn converting_the_dro_fixture_reproduces_the_vgm_fixture_exactly() {
        let dro = dro::read("lsl3_score_up_dro2.dro", DRO_V2_FIXTURE).unwrap();
        let vgm = dro_to_vgm(&dro).unwrap();

        assert_eq!(vgm.name, "lsl3_score_up_dro2.vgm");
        assert_eq!(vgm.header.total_samples(), 118_320);
        assert_eq!(vgm.len(), 299);

        // `file::write` reproduces the container verbatim, so this still pins the
        // conversion byte-for-byte against the independent `dro2vgm` oracle.
        let written = crate::vgm::file::write(&vgm).unwrap();
        assert_eq!(written.len(), VGM_FIXTURE.len());
        assert_eq!(written, VGM_FIXTURE);
    }

    #[test]
    fn the_sample_clock_rounds_the_running_total() {
        let mut clock = SampleClock::new();
        // Two identical 16 ms delays, 100 ms apart, land on different sample counts.
        assert_eq!(clock.samples_for_ms(100), 4410);
        assert_eq!(clock.samples_for_ms(16), 706);
        assert_eq!(clock.samples_for_ms(200), 8820);
        assert_eq!(clock.samples_for_ms(17), 749);
        assert_eq!(clock.samples_for_ms(17), 750);
        assert_eq!(clock.samples_for_ms(16), 706);
        assert_eq!(clock.samples_for_ms(100), 4410);
        assert_eq!(clock.samples_for_ms(1), 44);
        assert_eq!(clock.samples_for_ms(16), 705, "the second 16 ms differs");
    }

    #[test]
    fn the_sample_clock_never_drifts() {
        let mut clock = SampleClock::new();
        let mut total = 0u64;
        for ms in 1..=5000u32 {
            total += clock.samples_for_ms(ms);
            let cumulative_ms = u64::from(ms) * (u64::from(ms) + 1) / 2;
            let expected = (cumulative_ms * 44_100 + 500) / 1000;
            assert_eq!(total, expected, "after a delay of {ms} ms");
        }
    }

    #[test]
    fn a_long_delay_becomes_several_wait_commands() {
        // 65535 samples is ~1486 ms.
        let mut clock = SampleClock::new();
        let samples = clock.samples_for_ms(2000);
        assert!(samples > MAX_WAIT_SAMPLES);

        let song = build_dro_v1(&[0x01, 0xCF, 0x07]); // long delay: 0x07CF + 1 = 2000 ms
        let vgm = dro_to_vgm(&song).unwrap();
        assert_eq!(vgm.len(), 2, "one wait cannot express 88200 samples");
        assert_eq!(u64::from(vgm.header.total_samples()), samples);
        assert_eq!(vgm.body.raw()[0], command::WAIT);
        assert_eq!(vgm.body.raw()[3], command::WAIT, "the opcode repeats");

        // ... and it survives a round trip through the writer and reader.
        let written = crate::vgm::file::write(&vgm).unwrap();
        let reread = crate::vgm::file::read("t.vgm", &written).unwrap();
        assert_eq!(u64::from(reread.header.total_samples()), samples);
    }

    fn build_dro_v1(data: &[u8]) -> DroSong {
        DroSong::dro_v1(
            "t.dro".to_owned(),
            DroDataV1::new(data.to_vec()).unwrap(),
            0,
            OplType::Opl2,
        )
    }

    /// The playback/split projection primes the DRO v1 waveform-select-enable
    /// register (`0x01 = 0x20`) before the song's own writes, matching
    /// `DroEngine::reset_chip`. The peer of the engine's
    /// `the_v1_waveform_select_hack_is_primed_on_reset`.
    #[test]
    fn the_playback_projection_primes_the_v1_waveform_select() {
        let song = build_dro_v1(&[0x20, 0x01]); // one write: reg 0x20 = 0x01
        assert_eq!(song.file_version, DRO_FILE_V1);

        let vgm = opl_song_to_vgm_file(&song).unwrap();
        let raw = vgm.body.raw();
        assert_eq!(
            &raw[0..3],
            &[command::YM3812, 0x01, 0x20],
            "the WSE prime is the first command"
        );
        assert_eq!(
            &raw[3..6],
            &[command::YM3812, 0x20, 0x01],
            "the song's own write follows the prime"
        );
    }

    /// Convert to VGM stays byte-exact to `dro2vgm`: no prime, even for v1.
    #[test]
    fn the_explicit_convert_does_not_prime_the_v1_waveform_select() {
        let song = build_dro_v1(&[0x20, 0x01]);
        let vgm = dro_to_vgm(&song).unwrap();
        assert_eq!(
            &vgm.body.raw()[0..3],
            &[command::YM3812, 0x20, 0x01],
            "the song's own write is first; no prime"
        );
    }

    /// A v2 capture records its own waveform-select if it needs one, so the
    /// projection adds no prime.
    #[test]
    fn a_v2_capture_is_not_primed() {
        use crate::song::DroDataV2;
        // codemap: code 0 -> register 0x20; data: one low-bank write 0x20 = 0x01.
        let data = DroDataV2::new(vec![0x00, 0x01], vec![0x20], 0xFE, 0xFF).unwrap();
        let song = DroSong::dro_v2("t.dro".to_owned(), data, 0, OplType::Opl2);
        assert_ne!(song.file_version, DRO_FILE_V1);

        let vgm = opl_song_to_vgm_file(&song).unwrap();
        assert_eq!(
            &vgm.body.raw()[0..3],
            &[command::YM3812, 0x20, 0x01],
            "no prime is added for a v2 capture"
        );
    }

    #[test]
    fn write_commands_follow_the_chip_and_bank() {
        assert_eq!(write_command(OplType::Opl2, Bank::Low), command::YM3812);
        assert_eq!(write_command(OplType::Opl2, Bank::High), command::YM3812);
        assert_eq!(write_command(OplType::DualOpl2, Bank::Low), command::YM3812);
        assert_eq!(
            write_command(OplType::DualOpl2, Bank::High),
            command::YM3812_CHIP_2
        );
        assert_eq!(
            write_command(OplType::Opl3, Bank::Low),
            command::YMF262_PORT_0
        );
        assert_eq!(
            write_command(OplType::Opl3, Bank::High),
            command::YMF262_PORT_1
        );
    }

    /// DRO v1 bank switches must steer the VGM opcode.
    #[test]
    fn v1_bank_switches_select_the_port() {
        let song = DroSong::dro_v1(
            "t.dro".to_owned(),
            DroDataV1::new(vec![
                0x20, 0x01, // low bank register
                0x03, // switch to high
                0x20, 0x02, // high bank register
                0x02, // switch back to low
                0x20, 0x03,
            ])
            .unwrap(),
            0,
            OplType::Opl3,
        );
        let vgm = dro_to_vgm(&song).unwrap();
        // `body.raw()` is the whole command body, including the trailing end
        // marker the container carries.
        assert_eq!(
            vgm.body.raw(),
            [
                command::YMF262_PORT_0,
                0x20,
                0x01,
                command::YMF262_PORT_1,
                0x20,
                0x02,
                command::YMF262_PORT_0,
                0x20,
                0x03,
                command::END,
            ]
        );
        assert_eq!(vgm.len(), 3, "bank switches leave no VGM command behind");
    }

    #[test]
    fn the_v1_name_suffixes_the_stem() {
        assert_eq!(dro1_default_name("song.dro"), "song_1.dro");
        assert_eq!(dro1_default_name("song.DRO"), "song_1.DRO");
        // A name with no extension still gets the suffix.
        assert_eq!(dro1_default_name("capture"), "capture_1");
        assert_eq!(dro1_default_name("a.b/song.dro"), "a.b/song_1.dro");
    }

    #[test]
    fn extension_replacement() {
        assert_eq!(replace_extension("song.dro", "vgm"), "song.vgm");
        assert_eq!(replace_extension("song.DRO", "vgm"), "song.vgm");
        assert_eq!(replace_extension("song.vgz", "vgm"), "song.vgm");
        assert_eq!(replace_extension("a/b.c/song.dro", "vgm"), "a/b.c/song.vgm");
        assert_eq!(replace_extension("capture", "vgm"), "capture.vgm");
        assert_eq!(replace_extension("song.a", "vgm"), "song.a.vgm");
    }

    // -- dro2to1 -----------------------------------------------------------

    #[test]
    fn dro2_to_dro1_converts_the_fixture() {
        let v2 = dro::read("f.dro", DRO_V2_FIXTURE).unwrap();
        let v1 = dro2_to_dro1(&v2).unwrap();

        assert_eq!(v1.file_version, DRO_FILE_V1);
        assert_eq!(v1.opl_type, v2.opl_type);
        assert_eq!(v1.ms_length, v2.ms_length);
        assert_eq!(v1.total_delay_ms(), v2.total_delay_ms());
        assert_eq!(v1.len(), v2.len(), "an OPL2 capture needs no bank switches");

        // Every instruction survives, modulo the bank the v2 stream carried.
        for index in 0..v2.len() {
            match (
                v1.instruction(index).unwrap(),
                v2.instruction(index).unwrap(),
            ) {
                (
                    Instruction::Register {
                        reg: a,
                        value: b,
                        bank: None,
                    },
                    Instruction::Register {
                        reg: c,
                        value: d,
                        bank: Some(Bank::Low),
                    },
                ) => assert_eq!((a, b), (c, d), "instruction {index}"),
                (a, b) => assert_eq!(a, b, "instruction {index}"),
            }
        }
    }

    /// The v1 output must be readable.
    #[test]
    fn dro2_to_dro1_output_round_trips() {
        let v2 = dro::read("f.dro", DRO_V2_FIXTURE).unwrap();
        let v1 = dro2_to_dro1(&v2).unwrap();
        let written = dro::write(&v1).unwrap();
        let reread = dro::read("f.dro", &written).unwrap();
        assert_eq!(reread.data(), v1.data());
        assert_eq!(reread.total_delay_ms(), 2683);
    }

    #[test]
    fn dro2_to_dro1_escapes_low_registers_and_switches_banks() {
        use crate::song::DroDataV2;
        // codemap: code 0 -> register 0x04 (needs escaping), code 1 -> 0x20.
        let data = DroDataV2::new(
            vec![
                0x00, 0xFF, // low bank, register 0x04 = 0xFF   -> escaped
                0x81, 0x11, // high bank, register 0x20 = 0x11   -> bank switch + write
                0x01, 0x22, // low bank, register 0x20 = 0x22    -> bank switch + write
                0xFE, 0x00, // short delay, 1 ms
                0xFF, 0x00, // long delay, 256 ms
            ],
            vec![0x04, 0x20],
            0xFE,
            0xFF,
        )
        .unwrap();
        let v2 = DroSong::dro_v2("t.dro".to_owned(), data, 257, OplType::Opl3);
        let v1 = dro2_to_dro1(&v2).unwrap();

        assert_eq!(
            v1.data().raw(),
            [
                0x04, 0x04, 0xFF, // escaped register write
                0x03, // switch to the high bank
                0x20, 0x11, //
                0x02, // switch back to the low bank
                0x20, 0x22, //
                0x00, 0x00, // short delay: 0 + 1 = 1 ms
                0x01, 0xFF, 0x00, // long delay: 0x00FF + 1 = 256 ms
            ]
        );
        assert_eq!(v1.total_delay_ms(), 257);
    }

    #[test]
    fn dro2_to_dro1_rejects_a_v1_song() {
        let v1 = build_dro_v1(&[0x20, 0x01]);
        assert!(dro2_to_dro1(&v1).is_err());
    }
}
