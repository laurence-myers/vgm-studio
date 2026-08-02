//! An [`OplChip`] that writes to real silicon instead of emulating it.
//!
//! A real YMF262 sounds continuously and has no undo, so this chip does not
//! simply forward writes. It keeps two register files:
//!
//! * `shadow` — where the song *wants* the chip to be. Every write records here.
//! * `hw` — what the hardware actually holds, as far as we know.
//!
//! Playback writes go to both, and to the wire. A seek's replay goes only to the
//! shadow, and [`SerialOpl3Chip::materialize`] emits the difference — collapsing
//! the seek's hundreds of thousands of replayed writes to at most one per
//! register, so the chip does not audibly race through the song's history.

use vgms_core::OplType;
use vgms_synth::opl::OplChip;

use crate::{
    commands,
    protocol::{Bank, CmdBuffer},
};

/// Channel key-on registers. Written last when reconstructing state, so a note
/// never sounds before its frequency and envelope are in place.
const KEY_REGISTERS: core::ops::RangeInclusive<u8> = 0xB0..=0xB8;

/// The rhythm-mode register: bits 0-4 key the percussion voices.
const RHYTHM_REGISTER: u8 = 0xBD;

/// Bit 5 of `0xB0..=0xB8` — key on.
const KEY_ON: u8 = 0x20;

/// OPL3 mode, within bank 1: register `0x105`.
const NEW_REGISTER: u8 = 0x05;

/// Both speaker-enable bits of `0xC0..=0xC8`.
const BOTH_SPEAKERS: u8 = 0x30;

/// Whether `reg` is one this chip never puts on the wire.
///
/// `0xD0..=0xD8` are the emulator's per-channel panpots, an extension the engine
/// uses for the GUI's panning sliders. A real YMF262 has no such registers.
const fn is_emulator_only(reg: u8) -> bool {
    0xD0 <= reg && reg <= 0xD8
}

const fn is_key_register(reg: u8) -> bool {
    (0xB0 <= reg && reg <= 0xB8) || reg == RHYTHM_REGISTER
}

/// Drives a real YMF262 over a RetroWave board.
///
/// Produces silence from [`OplChip::generate_samples`]: the audio leaves through
/// the board's own output, never through this program.
#[derive(Debug)]
pub struct SerialOpl3Chip {
    /// The state the song is asking for, indexed `[bank][register]`.
    shadow: [[u8; 256]; 2],
    /// The state the hardware is believed to hold. `None` is "never written", so
    /// a fresh chip reconstructs its whole register file rather than trusting
    /// whatever the last song left behind.
    hw: [[Option<u8>; 256]; 2],
    buf: CmdBuffer,
    /// Whether to fix up an OPL2-era song for a real OPL3. See [`Self::translate`].
    opl2_compat: bool,
}

impl SerialOpl3Chip {
    /// A chip for a song of `opl_type`.
    ///
    /// The type matters because a real YMF262 is less forgiving than the
    /// emulator: see [`Self::translate`].
    #[must_use]
    pub fn new(opl_type: OplType) -> Self {
        Self {
            shadow: [[0; 256]; 2],
            hw: [[None; 256]; 2],
            buf: CmdBuffer::new(),
            opl2_compat: matches!(opl_type, OplType::Opl2 | OplType::DualOpl2),
        }
    }

    /// Rewrites a value for real hardware.
    ///
    /// Two fixes, both cases where the emulator is more forgiving than silicon:
    ///
    /// * `0x105` — the engine sets bit 1 to drive the emulator's stereo-ext
    ///   panpots. On a YMF262 that bit is not the panpot enable, and writing it
    ///   would clear OPL3 mode outright. Only bit 0, NEW, survives.
    /// * OPL2-family songs never write `0x105` at all, and a YMF262 with NEW
    ///   clear ignores its second register array entirely — so a dual-OPL2 song's
    ///   second chip would fall silent. NEW is therefore forced on; but with NEW
    ///   set the chip also honours the `0xC0` speaker bits, which OPL2 data does
    ///   not carry, so every channel would route to no speaker. Both are enabled.
    fn translate(&self, bank: usize, reg: u8, value: u8) -> u8 {
        if bank == 1 && reg == NEW_REGISTER {
            (value & 0x01) | u8::from(self.opl2_compat)
        } else if self.opl2_compat && (0xC0..=0xC8).contains(&reg) {
            value | BOTH_SPEAKERS
        } else {
            value
        }
    }

    /// Splits the engine's bank-encoded register address.
    const fn split(reg: u16) -> (usize, u8) {
        (((reg >> 8) & 1) as usize, (reg & 0xFF) as u8)
    }

    /// Queues a write and records it as the hardware's state.
    ///
    /// Takes an already-translated value: this is the wire, not the song.
    fn emit(&mut self, bank: usize, reg: u8, wire_value: u8) {
        let bank_id = if bank == 1 { Bank::One } else { Bank::Zero };
        self.buf.push_write(bank_id, reg, wire_value);
        self.hw[bank][usize::from(reg)] = Some(wire_value);
    }

    /// Whether the hardware disagrees with what the song wants.
    fn differs(&self, bank: usize, reg: u8) -> bool {
        if is_emulator_only(reg) {
            return false;
        }
        let target = self.translate(bank, reg, self.shadow[bank][usize::from(reg)]);
        self.hw[bank][usize::from(reg)] != Some(target)
    }

    fn emit_target(&mut self, bank: usize, reg: u8) {
        let target = self.translate(bank, reg, self.shadow[bank][usize::from(reg)]);
        self.emit(bank, reg, target);
    }

    /// Brings the hardware up to the state the song asks for, in one burst.
    ///
    /// Call after a seek, or on resuming from a pause. Emits at most one write
    /// per register, so cost is bounded by the register file however far the song
    /// has been replayed.
    ///
    /// Ordering is load-bearing:
    ///
    /// 1. NEW goes on first if any bank-1 register needs writing — the chip
    ///    ignores that whole array while NEW is clear, even for writes meant to
    ///    silence it.
    /// 2. Then everything that is not a key bit.
    /// 3. Then the key bits, so notes start with their pitch already set.
    /// 4. NEW goes off last if that is where the song wants it, by which point
    ///    bank 1 has been written.
    pub fn materialize(&mut self) {
        // Any bank-1 register needing a write forces NEW on first -- key
        // registers included, since the chip ignores the whole array while NEW
        // is clear, even a key-on. (The earlier `!is_key_register` term left a
        // note living only in bank 1 deaf after a seek.)
        let needs_bank_one =
            (0..=u8::MAX).any(|reg| reg != NEW_REGISTER && self.differs(1, reg));
        if needs_bank_one && self.hw[1][usize::from(NEW_REGISTER)] != Some(0x01) {
            self.emit(1, NEW_REGISTER, 0x01);
        }

        for bank in [1, 0] {
            for reg in 0..=u8::MAX {
                if is_key_register(reg) || (bank == 1 && reg == NEW_REGISTER) {
                    continue;
                }
                if self.differs(bank, reg) {
                    self.emit_target(bank, reg);
                }
            }
        }

        for bank in [1, 0] {
            for reg in KEY_REGISTERS.chain(core::iter::once(RHYTHM_REGISTER)) {
                if self.differs(bank, reg) {
                    self.emit_target(bank, reg);
                }
            }
        }

        let target_new = self.translate(1, NEW_REGISTER, self.shadow[1][usize::from(NEW_REGISTER)]);
        if self.hw[1][usize::from(NEW_REGISTER)] != Some(target_new) {
            self.emit(1, NEW_REGISTER, target_new);
        }
    }

    /// Releases every sounding note, without changing what the song wants.
    ///
    /// For pausing: the hardware goes quiet, but the shadow still describes the
    /// song's state, so [`materialize`](Self::materialize) re-arms exactly these
    /// bits on resume.
    pub fn release_all_notes(&mut self) {
        for bank in [0, 1] {
            for reg in KEY_REGISTERS {
                let current = self.hw[bank][usize::from(reg)].unwrap_or(0);
                if current & KEY_ON != 0 {
                    self.emit(bank, reg, current & !KEY_ON);
                }
            }
            // Rhythm voices are keyed by the low five bits of 0xBD instead.
            let current = self.hw[bank][usize::from(RHYTHM_REGISTER)].unwrap_or(0);
            if current & 0x1F != 0 {
                self.emit(bank, RHYTHM_REGISTER, current & 0xE0);
            }
        }
    }

    /// Silences the chip by sweeping its registers, recording what it wrote.
    ///
    /// The recording is the point: a sweep that bypassed this model would leave
    /// `hw` describing state the chip no longer holds, and the next
    /// [`materialize`](Self::materialize) would skip every register whose target
    /// happens to equal the pre-sweep value — leaving those channels silent.
    pub fn mute_sweep(&mut self) {
        let Self { buf, hw, .. } = self;
        commands::queue_mute_sweep(buf, |bank, reg, value| {
            let index = usize::from(bank == Bank::One);
            hw[index][usize::from(reg)] = Some(value);
        });
    }

    /// Packs anything queued, ready for [`wire`](Self::wire).
    pub fn seal(&mut self) {
        self.buf.seal();
    }

    /// The bytes waiting to go to the device. [`seal`](Self::seal) first.
    #[must_use]
    pub fn wire(&self) -> &[u8] {
        self.buf.wire()
    }

    /// Drops the sent bytes, keeping the allocation.
    pub fn clear_wire(&mut self) {
        self.buf.clear_wire();
    }
}

impl OplChip for SerialOpl3Chip {
    /// Forgets what the song asked for, without touching the hardware.
    ///
    /// The engine resets the chip at the top of every seek. Pulsing the YMF262's
    /// reset line that often would be both slow and audible, so this only clears
    /// the shadow; the difference against `hw` is settled by
    /// [`materialize`](SerialOpl3Chip::materialize) once the replay is done.
    fn reset(&mut self, _sample_rate: u32) {
        self.shadow = [[0; 256]; 2];
    }

    /// Records a write without sending it — the engine's seek/replay path.
    fn write_reg(&mut self, reg: u16, value: u8) {
        let (bank, reg) = Self::split(reg);
        self.shadow[bank][usize::from(reg)] = value;
    }

    /// Records a write and sends it — the engine's playback path.
    fn write_reg_buffered(&mut self, reg: u16, value: u8) {
        let (bank, reg) = Self::split(reg);
        self.shadow[bank][usize::from(reg)] = value;
        if is_emulator_only(reg) {
            return;
        }
        let wire_value = self.translate(bank, reg, value);
        self.emit(bank, reg, wire_value);
    }

    /// Silence: the sound comes out of the board, not this program.
    fn generate_samples(&mut self, buffer: &mut [i16]) {
        buffer.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip() -> SerialOpl3Chip {
        SerialOpl3Chip::new(OplType::Opl3)
    }

    /// The bytes one register write puts on the wire.
    fn expected_write(bank: Bank, reg: u8, value: u8) -> Vec<u8> {
        let mut buf = CmdBuffer::new();
        buf.push_write(bank, reg, value);
        buf.seal();
        buf.wire().to_vec()
    }

    /// Drains the queued writes, decoded.
    fn queued(chip: &mut SerialOpl3Chip) -> Vec<(Bank, u8, u8)> {
        chip.seal();
        let writes = crate::protocol::decode_writes(chip.wire());
        chip.clear_wire();
        writes
    }

    #[test]
    fn a_playback_write_reaches_the_wire() {
        let mut chip = chip();
        chip.write_reg_buffered(0x20, 0x01);
        chip.seal();
        assert_eq!(chip.wire(), expected_write(Bank::Zero, 0x20, 0x01));
    }

    #[test]
    fn a_high_bank_playback_write_addresses_bank_one() {
        let mut chip = chip();
        chip.write_reg_buffered(0x120, 0x01);
        chip.seal();
        assert_eq!(chip.wire(), expected_write(Bank::One, 0x20, 0x01));
    }

    /// The whole point of the shadow: a seek's replay costs no bandwidth.
    #[test]
    fn a_seek_path_write_emits_nothing() {
        let mut chip = chip();
        for reg in 0..1000u16 {
            chip.write_reg(reg % 0x200, (reg % 256) as u8);
        }
        chip.seal();
        assert!(chip.wire().is_empty(), "the replay path must stay silent");
    }

    #[test]
    fn resetting_emits_nothing_and_forgets_the_song_state() {
        let mut chip = chip();
        chip.write_reg(0x20, 0x55);
        chip.reset(49_716);
        chip.seal();
        assert!(chip.wire().is_empty());
        assert_eq!(chip.shadow[0][0x20], 0);
    }

    /// Nothing is known about a fresh chip's hardware, so everything is written
    /// — which is also how a device left mid-song by the last run gets cleaned up.
    #[test]
    fn a_fresh_chip_materialises_its_whole_register_file() {
        let mut chip = chip();
        chip.materialize();
        let writes = queued(&mut chip);

        let distinct: std::collections::HashSet<(Bank, u8)> =
            writes.iter().map(|&(bank, reg, _)| (bank, reg)).collect();
        // Both banks, minus the panpot range this chip never emits.
        assert_eq!(distinct.len(), 512 - 9 * 2);
        // One extra write: NEW is raised to make bank 1 answer, then put back.
        assert_eq!(writes.len(), distinct.len() + 1);
    }

    #[test]
    fn materialising_twice_sends_nothing_the_second_time() {
        let mut chip = chip();
        chip.materialize();
        let _ = queued(&mut chip);
        chip.materialize();
        assert!(
            queued(&mut chip).is_empty(),
            "a settled chip should be quiet"
        );
    }

    #[test]
    fn a_seek_then_materialise_sends_one_write_per_changed_register() {
        let mut chip = chip();
        chip.materialize();
        let _ = queued(&mut chip);

        // A replay that touches the same register a thousand times.
        for value in 0..1000u16 {
            chip.write_reg(0x40, (value % 64) as u8);
        }
        chip.write_reg(0xA0, 0x59);
        chip.materialize();

        let writes = queued(&mut chip);
        assert_eq!(writes.len(), 2, "expected one write per changed register");
        assert!(writes.contains(&(Bank::Zero, 0x40, (999 % 64) as u8)));
        assert!(writes.contains(&(Bank::Zero, 0xA0, 0x59)));
    }

    #[test]
    fn materialise_writes_key_registers_last() {
        let mut chip = chip();
        chip.materialize();
        let _ = queued(&mut chip);

        chip.write_reg(0xA0, 0x59);
        chip.write_reg(0xB0, 0x31);
        chip.materialize();

        let writes = queued(&mut chip);
        let key = writes.iter().position(|&(_, reg, _)| reg == 0xB0).unwrap();
        let freq = writes.iter().position(|&(_, reg, _)| reg == 0xA0).unwrap();
        assert!(
            freq < key,
            "frequency must be set before the note is keyed on"
        );
    }

    /// Bank 1 is deaf while NEW is clear, so it has to be raised first — even
    /// when the song wants it clear in the end.
    #[test]
    fn materialise_raises_new_before_writing_bank_one_and_clears_it_last() {
        let mut chip = chip();
        chip.materialize();
        let _ = queued(&mut chip);

        chip.write_reg(0x120, 0x01); // a bank-1 register, NEW left clear
        chip.materialize();

        let writes = queued(&mut chip);
        let new_on = writes
            .iter()
            .position(|&w| w == (Bank::One, NEW_REGISTER, 0x01))
            .expect("NEW should be raised");
        let target = writes
            .iter()
            .position(|&w| w == (Bank::One, 0x20, 0x01))
            .expect("the bank-1 write should land");
        let new_off = writes
            .iter()
            .rposition(|&w| w == (Bank::One, NEW_REGISTER, 0x00))
            .expect("NEW should be restored");
        assert!(new_on < target, "NEW must rise before bank-1 writes");
        assert!(target < new_off, "NEW must fall after them");
    }

    /// A note that lives only in bank 1 -- nothing else there differs -- still
    /// needs NEW raised first, or the key-on is ignored (sw-12).
    #[test]
    fn materialise_raises_new_when_only_a_bank_one_key_register_differs() {
        let mut chip = chip();
        chip.materialize();
        let _ = queued(&mut chip);

        chip.write_reg(0x1B0, 0x31); // key-on, bank 1, NEW left clear
        chip.materialize();

        let writes = queued(&mut chip);
        let new_on = writes
            .iter()
            .position(|&w| w == (Bank::One, NEW_REGISTER, 0x01))
            .expect("NEW should be raised so the bank-1 key write is heard");
        let key = writes
            .iter()
            .position(|&(bank, reg, _)| bank == Bank::One && reg == 0xB0)
            .expect("the bank-1 key-on should land");
        assert!(new_on < key, "NEW must rise before the bank-1 key write");
    }

    #[test]
    fn the_panpot_registers_never_reach_the_wire() {
        let mut chip = chip();
        chip.materialize();
        let _ = queued(&mut chip);

        for channel in 0..9u16 {
            chip.write_reg_buffered(0x0D0 + channel, 0x40);
            chip.write_reg_buffered(0x1D0 + channel, 0x40);
        }
        assert!(queued(&mut chip).is_empty(), "panpots are emulator-only");
    }

    /// The engine sets bit 1 of `0x105` for its own panning extension; on real
    /// silicon that would clear OPL3 mode.
    #[test]
    fn the_stereo_extension_bit_is_stripped_from_the_new_register() {
        let mut chip = chip();
        chip.materialize();
        let _ = queued(&mut chip);

        chip.write_reg_buffered(0x105, 0x03); // NEW plus the emulator's flag
        let writes = queued(&mut chip);
        assert_eq!(writes, [(Bank::One, NEW_REGISTER, 0x01)]);
    }

    #[test]
    fn releasing_notes_clears_key_bits_without_disturbing_the_song_state() {
        let mut chip = chip();
        chip.write_reg_buffered(0x0B0, 0x31); // keyed on
        chip.write_reg_buffered(0x0BD, 0x3F); // rhythm mode, all voices keyed
        let _ = queued(&mut chip);

        chip.release_all_notes();
        let writes = queued(&mut chip);
        assert!(
            writes.contains(&(Bank::Zero, 0xB0, 0x11)),
            "key bit cleared"
        );
        assert!(
            writes.contains(&(Bank::Zero, 0xBD, 0x20)),
            "rhythm keys cleared"
        );

        // The song still wants the note on, so resuming re-arms it.
        chip.materialize();
        let resumed = queued(&mut chip);
        assert!(resumed.contains(&(Bank::Zero, 0xB0, 0x31)));
        assert!(resumed.contains(&(Bank::Zero, 0xBD, 0x3F)));
    }

    #[test]
    fn releasing_notes_on_a_silent_chip_sends_nothing() {
        let mut chip = chip();
        chip.materialize();
        let _ = queued(&mut chip);
        chip.release_all_notes();
        assert!(queued(&mut chip).is_empty());
    }

    /// A sweep that did not update the hardware model would make the next
    /// materialise skip registers whose target matches their pre-sweep value.
    #[test]
    fn a_mute_sweep_leaves_the_hardware_model_truthful() {
        let mut chip = chip();
        chip.write_reg_buffered(0x040, 0x3F);
        chip.write_reg_buffered(0x0B0, 0x31);
        let _ = queued(&mut chip);

        chip.mute_sweep();
        let _ = queued(&mut chip);

        chip.materialize();
        let writes = queued(&mut chip);
        assert!(
            writes.contains(&(Bank::Zero, 0x40, 0x3F)),
            "the swept level must be rewritten"
        );
        assert!(
            writes.contains(&(Bank::Zero, 0xB0, 0x31)),
            "the swept note must be rewritten"
        );
    }

    #[test]
    fn generated_samples_are_silence() {
        let mut chip = chip();
        let mut buffer = [1234i16; 64];
        chip.generate_samples(&mut buffer);
        assert!(buffer.iter().all(|&sample| sample == 0));
    }

    mod opl2_compatibility {
        use super::*;

        fn opl2_chip() -> SerialOpl3Chip {
            SerialOpl3Chip::new(OplType::DualOpl2)
        }

        /// A dual-OPL2 song never writes `0x105`, and a YMF262 with NEW clear
        /// ignores bank 1 entirely — the second chip would simply not play.
        #[test]
        fn opl3_mode_is_forced_on_for_opl2_songs() {
            let mut chip = opl2_chip();
            chip.materialize();
            let writes = queued(&mut chip);
            assert!(writes.contains(&(Bank::One, NEW_REGISTER, 0x01)));
            assert!(
                !writes.contains(&(Bank::One, NEW_REGISTER, 0x00)),
                "NEW must never be cleared for an OPL2-family song"
            );
        }

        /// With NEW set, the chip honours speaker bits that OPL2 data lacks.
        #[test]
        fn both_speakers_are_enabled_for_opl2_songs() {
            let mut chip = opl2_chip();
            chip.materialize();
            let _ = queued(&mut chip);

            chip.write_reg_buffered(0x0C0, 0x01); // feedback/connection only
            let writes = queued(&mut chip);
            assert_eq!(writes, [(Bank::Zero, 0xC0, 0x31)]);
        }

        #[test]
        fn an_opl3_song_keeps_its_own_speaker_bits() {
            let mut chip = chip();
            chip.materialize();
            let _ = queued(&mut chip);

            chip.write_reg_buffered(0x0C0, 0x01);
            let writes = queued(&mut chip);
            assert_eq!(writes, [(Bank::Zero, 0xC0, 0x01)]);
        }

        /// Translation happens at the wire, so a settled chip stays quiet rather
        /// than re-emitting the registers it rewrote.
        #[test]
        fn translated_registers_do_not_re_emit_on_every_materialise() {
            let mut chip = opl2_chip();
            chip.materialize();
            let _ = queued(&mut chip);
            chip.write_reg_buffered(0x0C0, 0x01);
            let _ = queued(&mut chip);

            chip.materialize();
            assert!(queued(&mut chip).is_empty());
        }
    }
}
