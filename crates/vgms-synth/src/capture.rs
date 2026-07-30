//! Capture: re-record a (muted) playthrough as a new song file.
//!
//! This is what `drotrim split --song` uses to split a song into one file per
//! channel. It renders no audio: it walks the instruction stream, applies the
//! same [`Muting`] gate the player does, and re-emits the surviving register
//! writes and delays. A DRO becomes a DRO v2, its delays re-encoded into v2
//! short/long opcodes; a VGM becomes a VGM (see [`capture`]). Either way the
//! total timing is preserved.

use std::collections::HashMap;

use vgms_core::{Bank, DroDataV2, DroInstruction, Error, OplType, Result, Song, SongData};

use crate::engine::Muting;

/// The registers a DRO v1 capture zeroes before playback, so an isolated channel
/// starts from a known chip state: 122 registers -- five singletons, five
/// operator banks, three channel banks.
fn registers_to_init() -> Vec<u8> {
    let mut registers = vec![0x01u8, 0x04, 0x05, 0x08, 0xBD];
    for i in 0..24u8 {
        // Six of every eight operator offsets address a real operator.
        if (i & 7) < 6 {
            for base in [0x20u8, 0x40, 0x60, 0x80, 0xE0] {
                registers.push(base + i);
            }
        }
    }
    for i in 0..9u8 {
        for base in [0xA0u8, 0xB0, 0xC0] {
            registers.push(base + i);
        }
    }
    registers
}

/// Builds a DRO v2 stream from a sequence of register writes and delays.
struct DroCapture {
    /// Register number to its codemap code.
    code_of: HashMap<u8, u8>,
    /// The next code to hand an unfamiliar register. Starts past the delay codes.
    next_unknown: u8,
    short_delay_code: u8,
    long_delay_code: u8,
    data: Vec<u8>,
    length_ms: u32,
    bank: Bank,
}

impl DroCapture {
    fn new() -> Self {
        let init = registers_to_init();
        let short_delay_code = u8::try_from(init.len()).expect("122 fits in u8"); // 122
        let long_delay_code = short_delay_code + 1; // 123
        let code_of = init
            .iter()
            .enumerate()
            .map(|(code, &register)| (register, u8::try_from(code).expect("< 122")))
            .collect();
        Self {
            code_of,
            next_unknown: long_delay_code + 1, // 124
            short_delay_code,
            long_delay_code,
            data: Vec::new(),
            length_ms: 0,
            bank: Bank::Low,
        }
    }

    /// Records a register write on the current bank.
    ///
    /// # Errors
    /// If more than 128 distinct registers are seen -- the codemap's limit.
    fn write(&mut self, reg: u8, value: u8) -> Result<()> {
        let code = match self.code_of.get(&reg) {
            Some(&code) => code,
            None => {
                // Codes are 7-bit (bit 7 is the bank), so 127 is the last usable.
                if self.next_unknown > 127 {
                    return Err(Error::file(
                        "Too many distinct registers to capture as a DRO file (128 code limit). \
                         Delete instructions that write unusual registers and try again."
                            .to_owned(),
                    ));
                }
                let code = self.next_unknown;
                self.next_unknown += 1;
                self.code_of.insert(reg, code);
                code
            }
        };
        self.data.push(code | (self.bank.index() << 7));
        self.data.push(value);
        Ok(())
    }

    /// Records a delay, re-encoding it as DRO v2 short (`+1` ms) and long
    /// (`(+1) << 8` ms) opcodes.
    fn render(&mut self, ms: u32) {
        if ms == 0 {
            return;
        }
        self.length_ms = self.length_ms.saturating_add(ms);
        let mut long_units = ms / 256;
        while long_units > 0 {
            let chunk = long_units.min(255);
            self.data.push(self.long_delay_code);
            self.data
                .push(u8::try_from(chunk - 1).expect("chunk <= 255"));
            long_units -= chunk;
        }
        let short = ms % 256;
        if short > 0 {
            self.data.push(self.short_delay_code);
            self.data
                .push(u8::try_from(short - 1).expect("short <= 255"));
        }
    }

    /// Zeroes the init registers (DRO v1 sources only, so an isolated channel
    /// begins from silence). High bank too for Dual OPL2 / OPL3.
    ///
    /// # Errors
    /// Propagated from [`Self::write`].
    fn initialise_registers(&mut self, opl_type: OplType) -> Result<()> {
        let init = registers_to_init();
        self.bank = Bank::Low;
        for &register in &init {
            if register == 0x05 {
                continue; // register 5 only exists in the high bank
            }
            self.write(register, 0)?;
        }
        if opl_type != OplType::Opl2 {
            self.bank = Bank::High;
            for &register in &init {
                self.write(register, 0)?;
            }
            self.bank = Bank::Low;
        }
        Ok(())
    }

    /// Finishes the capture into a DRO v2 song.
    ///
    /// # Errors
    /// If the assembled stream is not a valid DRO v2 (it always is by construction).
    fn finish(self, name: String, opl_type: OplType) -> Result<Song> {
        // A dense codemap: `codemap[code] == register`. The delay-code positions
        // (and any gap) are left as placeholders -- `DroDataV2` checks a byte
        // against the delay codes before ever indexing the codemap, so a
        // placeholder there is never read. The dense codemap means an unknown
        // register (code >= 124) can never index past its end.
        let max_code = self.code_of.values().copied().max().unwrap_or(0);
        let mut codemap = vec![0u8; usize::from(max_code) + 1];
        for (&register, &code) in &self.code_of {
            codemap[usize::from(code)] = register;
        }
        let data = DroDataV2::new(
            self.data,
            codemap,
            self.short_delay_code,
            self.long_delay_code,
        )?;
        Ok(Song::dro_v2(name, data, self.length_ms, opl_type))
    }
}

/// Re-records `song` through `muting` as a new song of the same format, named
/// `name`.
///
/// Every surviving register write and every delay is captured; muted channels'
/// key-on writes are dropped and `0xBD` is masked, exactly as during playback.
///
/// A DRO is re-recorded here, as a DRO v2. A VGM's delays are counted in samples,
/// which DRO cannot express, so a VGM is filtered into another VGM instead --
/// keeping its header and GD3 tag -- by [`vgms_core::convert::filter_vgm`], which
/// this hands the same [`Muting::gate`].
///
/// # Errors
/// If the capture exceeds the 128-code codemap limit (DRO only).
pub fn capture(song: &Song, muting: Muting, name: String) -> Result<Song> {
    if song.is_vgm() {
        return vgms_core::convert::filter_vgm(
            song,
            |bank, reg, value| muting.gate(bank, reg, value),
            name,
        );
    }

    let mut capture = DroCapture::new();

    // A DRO v1 source does not reset the chip up front, so zero the registers; a
    // v2 source already begins from a clean state.
    if matches!(song.data(), SongData::V1(_)) {
        capture.initialise_registers(song.opl_type)?;
    }

    let mut bank = Bank::Low;
    for instruction in song.data().iter() {
        match instruction {
            DroInstruction::Register { reg, value, .. } => {
                if let Some(selected) = instruction.selected_bank() {
                    bank = selected;
                }
                capture.bank = bank;
                if let Some(gated) = muting.gate(bank, reg, value) {
                    capture.write(reg, gated)?;
                }
            }
            DroInstruction::BankSwitch(selected) => bank = selected,
            DroInstruction::DelayMs { ms, .. } => capture.render(ms),
            DroInstruction::DelaySamples { .. } => {
                // Unreachable: a sample delay only occurs in a VGM, which was
                // routed to `filter_vgm` above. Reported rather than panicked on.
                return Err(Error::file(
                    "Cannot capture a VGM's sample delays into a DRO file".to_owned(),
                ));
            }
        }
    }

    capture.finish(name, song.opl_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_wav;
    use vgms_core::io::read_song;
    use vgms_core::{DroDataV1, OplType};

    const DRO_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up_dro2.dro");

    fn register_writes(song: &Song) -> usize {
        song.data()
            .iter()
            .filter(|i| matches!(i, DroInstruction::Register { .. }))
            .count()
    }

    #[test]
    fn capturing_with_no_muting_preserves_timing_and_writes() {
        let song = read_song("lsl3_score_up_dro2.dro", DRO_FIXTURE).unwrap();
        let captured = capture(&song, Muting::all(), "out.dro".to_owned()).unwrap();

        // Every register write survives (0xBD is masked with 0xFF, i.e. unchanged),
        // and the re-encoded delays sum to the same length.
        assert_eq!(register_writes(&captured), register_writes(&song));
        assert_eq!(captured.total_delay_ms(), song.total_delay_ms());
    }

    #[test]
    fn the_capture_round_trips_through_the_dro_writer() {
        let song = read_song("lsl3_score_up_dro2.dro", DRO_FIXTURE).unwrap();
        let captured = capture(&song, Muting::all(), "out.dro".to_owned()).unwrap();
        let bytes = vgms_core::io::write_song(&captured).unwrap();
        let reread = read_song("out.dro", &bytes).unwrap();
        assert_eq!(reread.total_delay_ms(), song.total_delay_ms());
        assert_eq!(register_writes(&reread), register_writes(&song));
    }

    #[test]
    fn muting_a_channel_drops_its_writes() {
        let song = read_song("lsl3_score_up_dro2.dro", DRO_FIXTURE).unwrap();
        let full = capture(&song, Muting::all(), "a.dro".to_owned()).unwrap();

        // Isolate a single channel: everything melodic muted but channel 0.
        let mut muting = Muting::silent();
        muting.allow_channel(Bank::Low, 0xB0);
        let isolated = capture(&song, muting, "b.dro".to_owned()).unwrap();

        assert!(
            register_writes(&isolated) < register_writes(&full),
            "isolating a channel should drop other channels' key-on writes"
        );
        // Timing is unchanged -- only writes are dropped, never delays.
        assert_eq!(isolated.total_delay_ms(), full.total_delay_ms());
    }

    // Asserts the OPL render is *not* silent, which only an OPL core can
    // make true. `--no-default-features` has none by design.
    #[cfg(feature = "nuked-opl")]
    #[test]
    fn a_capture_is_playable() {
        let song = read_song("lsl3_score_up_dro2.dro", DRO_FIXTURE).unwrap();
        let captured = capture(&song, Muting::all(), "out.dro".to_owned()).unwrap();
        let wav = render_wav(&captured, 48_000, 16).unwrap();
        let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).unwrap();
        let samples: Vec<i16> = reader.into_samples::<i16>().map(Result::unwrap).collect();
        assert!(samples.iter().any(|&s| s != 0), "captured song was silent");
    }

    // -- VGM capture -------------------------------------------------------

    /// A short OPL2 song on two channels, so muting one is observable.
    fn two_channel_song() -> Song {
        Song::dro_v1(
            "two.dro".to_owned(),
            DroDataV1::new(vec![
                0x20, 0x01, 0xA0, 0x98, 0xB0, 0x31, // channel 0: operator, freq, key on
                0x21, 0x01, 0xA1, 0x98, 0xB1, 0x31, // channel 1
                0x00, 0x63, // 100 ms
            ])
            .unwrap(),
            100,
            OplType::Opl2,
        )
    }

    /// Whether `song` contains a write of `value` to `reg`.
    fn writes(song: &Song, reg: u8, value: u8) -> bool {
        song.data().iter().any(|i| {
            matches!(i, DroInstruction::Register { reg: r, value: v, .. } if r == reg && v == value)
        })
    }

    #[test]
    fn capturing_a_vgm_produces_a_vgm() {
        let vgm = vgms_core::convert::dro_to_vgm(&two_channel_song()).unwrap();
        let captured = capture(&vgm, Muting::all(), "out.vgm".to_owned()).unwrap();

        assert!(captured.is_vgm(), "a VGM capture must stay a VGM");
        assert_eq!(captured.name, "out.vgm");
        assert_eq!(captured.total_delay_samples(), vgm.total_delay_samples());
        assert_eq!(register_writes(&captured), register_writes(&vgm));
    }

    #[test]
    fn muting_a_channel_drops_its_writes_from_a_vgm() {
        let vgm = vgms_core::convert::dro_to_vgm(&two_channel_song()).unwrap();

        // Isolate channel 0: channel 1's key-on must not survive.
        let mut muting = Muting::silent();
        muting.allow_channel(Bank::Low, 0xB0);
        let isolated = capture(&vgm, muting, "b.vgm".to_owned()).unwrap();

        assert!(writes(&isolated, 0xB0, 0x31), "channel 0's key-on survives");
        assert!(!writes(&isolated, 0xB1, 0x31), "channel 1's key-on is gone");
        // Timing is untouched -- only writes are dropped, never delays.
        assert_eq!(isolated.total_delay_samples(), vgm.total_delay_samples());
    }

    #[test]
    fn a_captured_vgm_round_trips_through_the_writer() {
        let vgm = vgms_core::convert::dro_to_vgm(&two_channel_song()).unwrap();
        let captured = capture(&vgm, Muting::all(), "out.vgm".to_owned()).unwrap();

        let bytes = vgms_core::io::write_song(&captured).unwrap();
        assert!(bytes.starts_with(b"Vgm "), "not a VGM file");
        let reread = read_song("out.vgm", &bytes).unwrap();
        assert_eq!(reread.total_delay_samples(), vgm.total_delay_samples());
        assert_eq!(register_writes(&reread), register_writes(&vgm));
    }

    // Asserts the OPL render is *not* silent, which only an OPL core can
    // make true. `--no-default-features` has none by design.
    #[cfg(feature = "nuked-opl")]
    #[test]
    fn a_captured_vgm_is_playable() {
        let vgm = vgms_core::convert::dro_to_vgm(&two_channel_song()).unwrap();
        let captured = capture(&vgm, Muting::all(), "out.vgm".to_owned()).unwrap();
        let wav = render_wav(&captured, 48_000, 16).unwrap();
        let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).unwrap();
        let samples: Vec<i16> = reader.into_samples::<i16>().map(Result::unwrap).collect();
        assert!(samples.iter().any(|&s| s != 0), "captured song was silent");
    }

    #[test]
    fn a_v1_capture_zeroes_the_init_registers_first() {
        // A tiny v1 song: one register write and a delay.
        let song = Song::dro_v1(
            "t.dro".to_owned(),
            DroDataV1::new(vec![0x20, 0x01, 0x00, 0x63]).unwrap(),
            100,
            OplType::Opl2,
        );
        let captured = capture(&song, Muting::all(), "out.dro".to_owned()).unwrap();

        // OPL2 v1 init writes all 122 registers except 0x05 (121), then the song's
        // one write -> 122 register writes, all but the last valued 0.
        assert_eq!(register_writes(&captured), 121 + 1);
        let first = captured.instruction(0).unwrap();
        assert!(matches!(first, DroInstruction::Register { value: 0, .. }));
    }
}
