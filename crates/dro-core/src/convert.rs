//! Format conversions: DRO -> VGM, DRO v2 -> v1, and filtering a VGM's register
//! writes ([`filter_vgm`], which shares the VGM emitter with the DRO conversion).
//!
//! Ported from `VGMSong.from_song` and `dro2to1.py`.

use crate::error::{Error, Result};
use crate::song::dro_data::v1_opcode;
use crate::song::{Bank, DelayKind, DroDataV1, DroInstruction, OplType, Song, SongData};
use crate::util::VGM_SAMPLE_RATE;
use crate::vgm::data::command;
use crate::vgm::io::{CONVERSION_VERSION, synthesise_header};
use crate::vgm::{VgmData, VgmMeta};

/// The longest wait a single `0x61` command can express.
const MAX_WAIT_SAMPLES: u64 = 0xFFFF;

/// Milliseconds to samples, rounding the *running total* half up.
///
/// Recovered from `tests/lsl3_score_up.vgm`, which `dro2vgm` produced from the DRO
/// fixture: two identical 16 ms delays in it become 706 and 705 samples, which no
/// per-delay rounding can produce. Seeding the carry at half a millisecond makes
/// each emitted count `round(cumulative_ms * 44.1)` minus its predecessor.
///
/// The Python got this triply wrong: delays of 15 ms or less were emitted as
/// `0x70 | ms`, which waits `ms + 1` *samples*; the running total counted those
/// milliseconds as samples; and a wait longer than 65535 samples dropped the
/// repeated `0x61` opcode, corrupting the stream.
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
/// Shared by [`dro_to_vgm`], whose millisecond delays arrive through a
/// [`SampleClock`], and [`filter_vgm`], whose delays are already in samples.
#[derive(Debug)]
struct VgmStream {
    opl_type: OplType,
    out: Vec<u8>,
    /// Commands emitted so far. A VGM instruction index, which is what
    /// [`filter_vgm`] remaps loop points onto.
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
        let mut remaining = samples;
        loop {
            let chunk = remaining.min(MAX_WAIT_SAMPLES);
            let chunk16 = u16::try_from(chunk).expect("clamped to 0xFFFF");
            self.out.push(command::WAIT);
            self.out.extend_from_slice(&chunk16.to_le_bytes());
            self.count += 1;
            remaining -= chunk;
            if remaining == 0 {
                break;
            }
        }
    }

    /// Indexes the finished stream.
    ///
    /// # Errors
    /// Never in practice -- every command written here is one the indexer knows.
    fn finish(self) -> Result<VgmData> {
        VgmData::new(self.out)
    }
}

/// Converts a DRO song to a VGM song.
///
/// # Errors
/// If `song` is already a VGM.
pub fn dro_to_vgm(song: &Song) -> Result<Song> {
    if song.is_vgm() {
        return Err(Error::file("Tried to convert a VGM song to VGM"));
    }

    let mut clock = SampleClock::new();
    let mut bank = Bank::Low;
    let mut stream = VgmStream::with_capacity(song.opl_type, song.len() * 3);

    for instruction in song.data().iter() {
        match instruction {
            DroInstruction::BankSwitch(selected) => bank = selected, // DRO v1
            DroInstruction::Register {
                reg,
                value,
                bank: own,
            } => {
                // DRO v2 carries the bank on the write; v1 tracks it separately.
                stream.write(own.unwrap_or(bank), reg, value);
            }
            DroInstruction::DelayMs { ms, .. } => stream.wait(clock.samples_for_ms(ms)),
            DroInstruction::DelaySamples { .. } => {
                unreachable!("a DRO song has no sample delays")
            }
        }
    }

    let mut header = synthesise_header();
    crate::vgm::io::put_chip_clocks(&mut header, song.opl_type)?;

    Ok(Song::vgm(
        replace_extension(&song.name, "vgm"),
        CONVERSION_VERSION,
        stream.finish()?,
        song.opl_type,
        VgmMeta::new(header),
    ))
}

/// Rewrites a VGM's register writes through `gate`, keeping everything else.
///
/// Each write is offered to `gate` as `(bank, register, value)`: `None` drops it,
/// `Some(value)` writes that value instead. That is deliberately the shape of
/// `dro-synth`'s playback muting gate, which is what splitting a VGM into one
/// file per channel passes in. Delays are preserved sample for sample, so the
/// result lines up with the original however much is muted out.
///
/// The version, header and GD3 tag come from `song`, so the output is the
/// original file minus the muted voices. Loop points are instruction indices, so
/// they are remapped onto the commands that survive: a dropped write takes no
/// time, so a loop point that lands on one moves to the next surviving command
/// and still restarts the same moment of music.
///
/// Waits are re-encoded canonically as `0x61`: a `0x62`, `0x63` or `0x7n` in the
/// source becomes an equivalent `0x61`. The timing is identical; the bytes are
/// not.
///
/// # Errors
/// If `song` is not a VGM.
pub fn filter_vgm(
    song: &Song,
    mut gate: impl FnMut(Bank, u8, u8) -> Option<u8>,
    name: String,
) -> Result<Song> {
    let Some(meta) = song.vgm_meta() else {
        return Err(Error::file(
            "Only a VGM song can be filtered into a VGM".to_owned(),
        ));
    };

    let mut stream = VgmStream::with_capacity(song.opl_type, song.len() * 3);
    let mut loop_point = None;
    let mut loop_end = None;

    for (index, instruction) in song.data().iter().enumerate() {
        // Resolve the boundaries *before* emitting: a loop point on a delay long
        // enough to need several `0x61`s must land on the first of them.
        if meta.loop_point == Some(index) {
            loop_point = Some(stream.count);
        }
        if meta.loop_end == Some(index) {
            loop_end = Some(stream.count);
        }
        match instruction {
            DroInstruction::Register { reg, value, bank } => {
                let bank = bank.unwrap_or(Bank::Low);
                if let Some(gated) = gate(bank, reg, value) {
                    stream.write(bank, reg, gated);
                }
            }
            DroInstruction::DelaySamples { samples, .. } => stream.wait(u64::from(samples)),
            DroInstruction::BankSwitch(_) | DroInstruction::DelayMs { .. } => {
                unreachable!("a VGM song has neither bank switches nor millisecond delays")
            }
        }
    }
    // A loop that runs to the very end carries an index of `len`, which the walk
    // above never reaches.
    if meta.loop_point == Some(song.len()) {
        loop_point = Some(stream.count);
    }
    if meta.loop_end == Some(song.len()) {
        loop_end = Some(stream.count);
    }

    let mut filtered = meta.clone();
    // Muting out every command between the loop points leaves a region of no
    // duration, which nothing can loop. Such a loop was already degenerate in the
    // source (only a run of register writes, no delay); drop it rather than write
    // a loop end that is no longer past its start.
    if let (Some(start), Some(end)) = (loop_point, loop_end)
        && end <= start
    {
        loop_point = None;
        loop_end = None;
    }
    filtered.loop_point = loop_point;
    filtered.loop_end = loop_end;

    Ok(Song::vgm(
        name,
        song.file_version,
        stream.finish()?,
        song.opl_type,
        filtered,
    ))
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
/// the bank changes. As the Python noted, v2 files usually alternate banks, so the
/// result is bank-switch heavy; grouping the writes would be a better conversion.
///
/// # Errors
/// If `song` is not a DRO v2 song, or a delay does not fit v1's encoding.
pub fn dro2_to_dro1(song: &Song) -> Result<Song> {
    let SongData::V2(_) = song.data() else {
        return Err(Error::file(
            "Only DRO v2 files can be converted to DRO v1".to_owned(),
        ));
    };

    let mut out: Vec<u8> = Vec::with_capacity(song.len() * 2);
    let mut bank = Bank::Low; // DRO v1 chips start on the low bank

    for instruction in song.data().iter() {
        match instruction {
            DroInstruction::DelayMs {
                kind: DelayKind::Short,
                ms,
            } => {
                let value = u8::try_from(ms - 1).map_err(|_| {
                    Error::file(format!("Short delay of {ms} ms does not fit DRO v1"))
                })?;
                out.extend_from_slice(&[v1_opcode::SHORT_DELAY, value]);
            }
            DroInstruction::DelayMs {
                kind: DelayKind::Long,
                ms,
            } => {
                let value = u16::try_from(ms - 1).map_err(|_| {
                    Error::file(format!("Long delay of {ms} ms does not fit DRO v1"))
                })?;
                out.push(v1_opcode::LONG_DELAY);
                out.extend_from_slice(&value.to_le_bytes());
            }
            DroInstruction::Register {
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
            DroInstruction::BankSwitch(_) | DroInstruction::DelaySamples { .. } => {
                unreachable!("a DRO v2 song has neither bank switches nor sample delays")
            }
        }
    }

    Ok(Song::dro_v1(
        song.name.clone(),
        DroDataV1::new(out)?,
        song.ms_length,
        song.opl_type,
    ))
}

/// Swaps a three- or four-character extension for `extension`, or appends one.
///
/// The Python's `re.sub(r"\..{3,4}$", ".vgm", name)` left a name with no extension
/// untouched, so `capture` converted to a VGM still called `capture`.
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
    use crate::vgm::io as vgm_io;

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
        assert_eq!(vgm.total_delay_samples(), 118_320);
        assert_eq!(vgm.len(), 299);

        let written = vgm_io::write(&vgm).unwrap();
        assert_eq!(written.len(), VGM_FIXTURE.len());
        assert_eq!(written, VGM_FIXTURE);
    }

    /// Python's converter reports 118125 samples for the same input.
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
        // 65535 samples is ~1486 ms. Python emitted `61 FF FF <lo> <hi>`, leaving
        // the second pair to be decoded as commands.
        let mut clock = SampleClock::new();
        let samples = clock.samples_for_ms(2000);
        assert!(samples > MAX_WAIT_SAMPLES);

        let song = build_dro_v1(&[0x01, 0xCF, 0x07]); // long delay: 0x07CF + 1 = 2000 ms
        let vgm = dro_to_vgm(&song).unwrap();
        assert_eq!(vgm.len(), 2, "one wait cannot express 88200 samples");
        assert_eq!(vgm.total_delay_samples() as u64, samples);
        assert_eq!(vgm.data().raw()[0], command::WAIT);
        assert_eq!(vgm.data().raw()[3], command::WAIT, "the opcode repeats");

        // ... and it survives a round trip through the writer and reader.
        let written = vgm_io::write(&vgm).unwrap();
        let reread = vgm_io::read("t.vgm", &written).unwrap();
        assert_eq!(reread.total_delay_samples() as u64, samples);
    }

    fn build_dro_v1(data: &[u8]) -> Song {
        Song::dro_v1(
            "t.dro".to_owned(),
            DroDataV1::new(data.to_vec()).unwrap(),
            0,
            OplType::Opl2,
        )
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
        let song = Song::dro_v1(
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
        assert_eq!(
            vgm.data().raw(),
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
            ]
        );
        assert_eq!(vgm.len(), 3, "bank switches leave no VGM command behind");
    }

    #[test]
    fn converting_a_vgm_is_rejected() {
        let vgm = vgm_io::read("f.vgm", VGM_FIXTURE).unwrap();
        assert!(dro_to_vgm(&vgm).is_err());
    }

    #[test]
    fn extension_replacement() {
        assert_eq!(replace_extension("song.dro", "vgm"), "song.vgm");
        assert_eq!(replace_extension("song.DRO", "vgm"), "song.vgm");
        assert_eq!(replace_extension("song.vgz", "vgm"), "song.vgm");
        assert_eq!(replace_extension("a/b.c/song.dro", "vgm"), "a/b.c/song.vgm");
        // Python left this one alone, producing a VGM still called `capture`.
        assert_eq!(replace_extension("capture", "vgm"), "capture.vgm");
        assert_eq!(replace_extension("song.a", "vgm"), "song.a.vgm");
    }

    // -- dro2to1 -----------------------------------------------------------

    #[test]
    fn dro2_to_dro1_converts_the_fixture() {
        let v2 = dro::read("f.dro", DRO_V2_FIXTURE).unwrap();
        let v1 = dro2_to_dro1(&v2).unwrap();

        assert_eq!(v1.file_version, crate::song::DRO_FILE_V1);
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
                    DroInstruction::Register {
                        reg: a,
                        value: b,
                        bank: None,
                    },
                    DroInstruction::Register {
                        reg: c,
                        value: d,
                        bank: Some(Bank::Low),
                    },
                ) => assert_eq!((a, b), (c, d), "instruction {index}"),
                (a, b) => assert_eq!(a, b, "instruction {index}"),
            }
        }
    }

    /// The v1 output must be readable -- which the Python's own v1 reader cannot do.
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
        let v2 = Song::dro_v2("t.dro".to_owned(), data, 257, OplType::Opl3);
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

    // -- filter_vgm --------------------------------------------------------

    /// Passes everything through, so only the encoding may change.
    fn keep_all(_: Bank, _: u8, value: u8) -> Option<u8> {
        Some(value)
    }

    #[test]
    fn filtering_a_vgm_with_an_open_gate_preserves_the_song() {
        let vgm = vgm_io::read("f.vgm", VGM_FIXTURE).unwrap();
        let filtered = filter_vgm(&vgm, keep_all, "out.vgm".to_owned()).unwrap();

        assert_eq!(filtered.name, "out.vgm");
        assert_eq!(filtered.file_version, vgm.file_version);
        assert_eq!(filtered.opl_type, vgm.opl_type);
        assert_eq!(filtered.total_delay_samples(), vgm.total_delay_samples());
        assert_eq!(filtered.len(), vgm.len());
        // The header and tag come along, so this is still the same file.
        let (before, after) = (vgm.vgm_meta().unwrap(), filtered.vgm_meta().unwrap());
        assert_eq!(after.header(), before.header());
        assert_eq!(after.tag, before.tag);

        // ... and it survives the writer and reader.
        let written = vgm_io::write(&filtered).unwrap();
        let reread = vgm_io::read("out.vgm", &written).unwrap();
        assert_eq!(reread.total_delay_samples(), vgm.total_delay_samples());
    }

    #[test]
    fn a_closed_gate_drops_every_write_but_keeps_the_timing() {
        let vgm = vgm_io::read("f.vgm", VGM_FIXTURE).unwrap();
        let filtered = filter_vgm(&vgm, |_, _, _| None, "out.vgm".to_owned()).unwrap();

        let delays = vgm
            .data()
            .iter()
            .filter(|i| matches!(i, DroInstruction::DelaySamples { .. }))
            .count();
        assert_eq!(filtered.len(), delays, "only the delays should remain");
        assert_eq!(filtered.total_delay_samples(), vgm.total_delay_samples());
    }

    #[test]
    fn a_rewriting_gate_replaces_the_value() {
        let vgm = vgm_io::read("f.vgm", VGM_FIXTURE).unwrap();
        let filtered = filter_vgm(&vgm, |_, _, _| Some(0x7F), "out.vgm".to_owned()).unwrap();
        assert!(
            filtered
                .data()
                .iter()
                .filter_map(|i| match i {
                    DroInstruction::Register { value, .. } => Some(value),
                    _ => None,
                })
                .all(|value| value == 0x7F)
        );
    }

    /// A source whose waits use every encoding: they all come back as `0x61`,
    /// with the sample totals untouched.
    #[test]
    fn waits_are_re_encoded_canonically() {
        let data = VgmData::new(vec![
            command::WAIT_60TH,              // 735 samples
            command::WAIT_50TH,              // 882 samples
            command::SHORT_WAIT_BASE | 0x0F, // 16 samples
            command::WAIT,
            0x10,
            0x00, // 16 samples
        ])
        .unwrap();
        let song = Song::vgm(
            "t.vgm".to_owned(),
            0x151,
            data,
            OplType::Opl2,
            VgmMeta::new(synthesise_header()),
        );

        let filtered = filter_vgm(&song, keep_all, "out.vgm".to_owned()).unwrap();
        assert_eq!(filtered.total_delay_samples(), song.total_delay_samples());
        assert_eq!(filtered.len(), song.len());
        assert!(
            filtered
                .data()
                .raw()
                .chunks_exact(3)
                .all(|command| command[0] == command::WAIT),
            "every wait should be a 0x61: {:02X?}",
            filtered.data().raw()
        );
    }

    /// A wait too long for one `0x61` becomes several -- and a loop point sitting
    /// on it must resolve to the first of them, not the last.
    #[test]
    fn a_loop_point_on_a_multi_chunk_wait_lands_on_its_first_command() {
        let mut bytes = vec![command::YM3812, 0x20, 0x01];
        bytes.extend_from_slice(&[command::WAIT, 0xFF, 0xFF]); // 65535 samples
        bytes.extend_from_slice(&[command::YM3812, 0x21, 0x02]);
        let mut song = Song::vgm(
            "t.vgm".to_owned(),
            0x151,
            VgmData::new(bytes).unwrap(),
            OplType::Opl2,
            VgmMeta::new(synthesise_header()),
        );
        song.vgm_meta_mut().unwrap().loop_point = Some(1); // the wait

        // Drop the first write, so indices shift by one.
        let filtered = filter_vgm(
            &song,
            |_, reg, value| (reg != 0x20).then_some(value),
            "out.vgm".to_owned(),
        )
        .unwrap();
        assert_eq!(
            filtered.vgm_meta().unwrap().loop_point,
            Some(0),
            "the loop should follow the wait to its new index"
        );
        assert_eq!(filtered.total_delay_samples(), song.total_delay_samples());
    }

    #[test]
    fn loop_points_slide_past_dropped_writes() {
        let bytes = vec![
            command::YM3812,
            0x20,
            0x01, // 0: dropped
            command::YM3812,
            0x21,
            0x02, // 1: kept
            command::WAIT,
            0x64,
            0x00, // 2: 100 samples
            command::YM3812,
            0x20,
            0x03, // 3: dropped
            command::WAIT,
            0x64,
            0x00, // 4: 100 samples
        ];
        let mut song = Song::vgm(
            "t.vgm".to_owned(),
            0x151,
            VgmData::new(bytes).unwrap(),
            OplType::Opl2,
            VgmMeta::new(synthesise_header()),
        );
        {
            let meta = song.vgm_meta_mut().unwrap();
            meta.loop_point = Some(2); // the first wait
            meta.loop_end = Some(4); // exclusive: up to the second wait
        }

        let filtered = filter_vgm(
            &song,
            |_, reg, value| (reg != 0x20).then_some(value),
            "out.vgm".to_owned(),
        )
        .unwrap();

        // Surviving stream: write(0x21), wait, wait -> the loop region is [1, 2).
        let meta = filtered.vgm_meta().unwrap();
        assert_eq!(meta.loop_point, Some(1));
        assert_eq!(meta.loop_end, Some(2));
        // The loop still covers the same music: one 100-sample wait.
        assert_eq!(filtered.loop_num_samples(), song.loop_num_samples());
    }

    /// Muting out a loop region that held only register writes leaves nothing to
    /// loop, so the loop goes rather than being written back inverted.
    #[test]
    fn a_loop_region_emptied_by_the_gate_is_dropped() {
        let bytes = vec![
            command::WAIT,
            0x64,
            0x00, // 0
            command::YM3812,
            0x20,
            0x01, // 1: the whole loop region, dropped
            command::WAIT,
            0x64,
            0x00, // 2
        ];
        let mut song = Song::vgm(
            "t.vgm".to_owned(),
            0x151,
            VgmData::new(bytes).unwrap(),
            OplType::Opl2,
            VgmMeta::new(synthesise_header()),
        );
        {
            let meta = song.vgm_meta_mut().unwrap();
            meta.loop_point = Some(1);
            meta.loop_end = Some(2);
        }

        let filtered = filter_vgm(&song, |_, _, _| None, "out.vgm".to_owned()).unwrap();
        let meta = filtered.vgm_meta().unwrap();
        assert_eq!(meta.loop_point, None);
        assert_eq!(meta.loop_end, None);
    }

    #[test]
    fn filtering_a_dro_is_rejected() {
        let dro = build_dro_v1(&[0x20, 0x01]);
        assert!(filter_vgm(&dro, keep_all, "out.vgm".to_owned()).is_err());
    }

    #[test]
    fn dro2_to_dro1_rejects_a_v1_song_or_a_vgm() {
        let v1 = build_dro_v1(&[0x20, 0x01]);
        assert!(dro2_to_dro1(&v1).is_err());
        let vgm = vgm_io::read("f.vgm", VGM_FIXTURE).unwrap();
        assert!(dro2_to_dro1(&vgm).is_err());
    }
}
