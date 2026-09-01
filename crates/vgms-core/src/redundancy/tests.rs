//! What each rule shape promises, one shape at a time.

use super::*;
use crate::vgm::stream::END_OF_DATA;

fn stream(bytes: Vec<u8>) -> VgmStream {
    let mut bytes = bytes;
    bytes.push(END_OF_DATA);
    VgmStream::parse(bytes, 0x171).expect("a walkable stream")
}

/// A YM3812 write: every OPL register is a pure latch.
fn opl(addr: u8, data: u8) -> [u8; 3] {
    [0x5A, addr, data]
}

fn flat(commands: &[&[u8]]) -> Vec<u8> {
    commands.iter().flat_map(|c| c.iter().copied()).collect()
}

// -- the ordinary latch ------------------------------------------------------

#[test]
fn a_repeated_write_to_a_latch_is_redundant() {
    let s = stream(flat(&[
        &opl(0x20, 0x01), // 0
        &[0x62],          // 1
        &opl(0x20, 0x01), // 2: the same value again
        &opl(0x20, 0x02), // 3: a different value
        &opl(0x20, 0x02), // 4: and its repeat
    ]));
    assert_eq!(redundant_indices(&s, None), [2, 4]);
}

/// Registers that trigger on write rather than latching are never dropped,
/// even on a chip that has rules. The YM2413's `0x20`-`0x28` carry the key bit,
/// so a value-identical repeat re-attacks and is kept; an ordinary latch
/// (`0x30`, instrument + volume) is dropped on repeat.
#[test]
fn a_trigger_register_is_never_dropped() {
    let s = stream(vec![
        0x51, 0x20, 0x30, // 0: YM2413 block + key
        0x51, 0x20, 0x30, // 1: the same value again -- a trigger, kept
        0x51, 0x30, 0x0F, // 2: an ordinary latch
        0x51, 0x30, 0x0F, // 3: its repeat -- dropped
    ]);
    assert_eq!(redundant_indices(&s, None), [3], "only the latch repeat");
}

/// Everything is forgotten at the loop point, so the loop body carries its own
/// state and sounds the same on the second pass.
#[test]
fn the_loop_point_forgets_every_cell() {
    let s = stream(flat(&[
        &opl(0x20, 0x01), // 0
        &opl(0x20, 0x01), // 1: redundant
        &opl(0x20, 0x01), // 2: the loop point -- kept
        &opl(0x20, 0x01), // 3: redundant again
    ]));
    assert_eq!(redundant_indices(&s, Some(2)), [1, 3]);
}

/// Two instances of a chip are two sets of registers.
#[test]
fn a_second_instance_holds_its_own_values() {
    let s = stream(vec![
        0x5A, 0x20, 0x01, // 0: chip 1
        0xAA, 0x20, 0x01, // 1: chip 2 -- not a repeat of chip 1
        0xAA, 0x20, 0x01, // 2: now it is
    ]);
    assert_eq!(redundant_indices(&s, None), [2]);
}

/// Two ports of one chip are two sets of registers: OPL3's second bank shares
/// its register numbers with the first.
#[test]
fn a_second_port_holds_its_own_values() {
    let s = stream(vec![
        0x5E, 0x20, 0x01, // 0: YMF262 port 0
        0x5F, 0x20, 0x01, // 1: port 1 -- a different cell
        0x5F, 0x20, 0x01, // 2: now it repeats
    ]);
    assert_eq!(redundant_indices(&s, None), [2]);
}

// -- every chip is classified ------------------------------------------------

/// The routing decision the pipeline makes rests on this: there is no chip the
/// built-in optimiser has to hand to an external tool for want of a rule.
#[test]
fn every_chip_the_format_defines_has_rules() {
    for chip in ChipKind::all() {
        assert!(has_latch_rules(chip), "{} has no rules", chip.name());
    }
}

// -- the OPN commit latch ----------------------------------------------------

/// `0xA4` latches the block and F-number high bits; the `0xA0` that follows
/// commits the pair. A low-byte write is therefore never redundant, however
/// familiar its value -- dropping one after a re-latch leaves the channel at
/// the old pitch, which is what corrupted 25 of 500 corpus files.
#[test]
fn an_opn_low_byte_write_is_never_dropped() {
    let s = stream(vec![
        0x52, 0xA4, 0x22, // 0: latch high
        0x52, 0xA0, 0x69, // 1: commit
        0x52, 0xA4, 0x1A, // 2: a different high byte
        0x52, 0xA0, 0x69, // 3: the same low byte -- but a different note
    ]);
    assert!(
        redundant_indices(&s, None).is_empty(),
        "no OPN frequency write may be dropped here"
    );
}

/// The latch itself dedupes, but only against the last write to the whole
/// group: `0xA4` and `0xA5` share one latch on the chip, so an `0xA5` in
/// between makes the second `0xA4` meaningful again.
#[test]
fn an_opn_latch_dedupes_only_against_the_last_write_to_its_group() {
    let s = stream(vec![
        0x52, 0xA4, 0x22, // 0
        0x52, 0xA4, 0x22, // 1: the same latch, the same value -- redundant
        0x52, 0xA5, 0x22, // 2: the same value, a different address -- kept
        0x52, 0xA4, 0x22, // 3: the group moved since -- kept
    ]);
    assert_eq!(redundant_indices(&s, None), [1]);
}

/// `0xAC`-`0xAE` are the second latch, not the first: a write to one does not
/// invalidate the other.
#[test]
fn the_two_opn_latch_groups_are_separate() {
    let s = stream(vec![
        0x52, 0xA4, 0x22, // 0
        0x52, 0xAC, 0x22, // 1: the other group
        0x52, 0xA4, 0x22, // 2: still the last write to its own group
    ]);
    assert_eq!(redundant_indices(&s, None), [2]);
}

/// But the group is one latch for the whole chip, ports included: `0x53` is the
/// YM2612's port 1, and its `0xA4` overwrites what port 0 latched.
#[test]
fn the_opn_latch_group_spans_both_ports() {
    let s = stream(vec![
        0x52, 0xA4, 0x22, // 0: port 0
        0x53, 0xA4, 0x22, // 1: port 1 -- the same latch, a different address
        0x52, 0xA4, 0x22, // 2: port 0 again, and the latch moved since
    ]);
    assert!(
        redundant_indices(&s, None).is_empty(),
        "the chip has one F-number latch, not one per port"
    );
}

/// The DAC port is a sample, not a register.
#[test]
fn the_ym2612_dac_port_keeps_every_write() {
    let s = stream(vec![
        0x52, 0x2A, 0x80, // 0
        0x52, 0x2A, 0x80, // 1: the same byte, another sample
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

// -- the SN76489's two-byte protocol -----------------------------------------

/// A volume write repeats often -- a driver silencing a channel every frame --
/// and is a plain latch, so long as no continuation byte depends on the
/// register select it carries.
#[test]
fn a_repeated_sn76489_volume_latch_is_redundant() {
    let s = stream(vec![
        0x50, 0x9F, // 0: channel 0 volume off
        0x50, 0x9F, // 1: again
        0x50, 0xBF, // 2: channel 1 volume off
        0x50, 0x9F, // 3: channel 0 again -- its own cell, still 0xF
    ]);
    assert_eq!(redundant_indices(&s, None), [1, 3]);
}

/// A latch byte followed by a continuation byte carries the register select
/// that continuation needs, so it is kept even when its own nibble repeats.
#[test]
fn an_sn76489_latch_a_continuation_depends_on_is_kept() {
    let s = stream(vec![
        0x50, 0x80, // 0: tone 0, low nibble 0
        0x50, 0x3F, // 1: its high bits
        0x50, 0x80, // 2: the same nibble -- but the next byte continues it
        0x50, 0x3F, // 3: the same high bits, on a register that already holds
    ]);
    assert_eq!(
        redundant_indices(&s, None),
        [3],
        "the continuation dedupes, the latch it depends on does not"
    );
}

/// A latch that is kept still changes the register, so it must be recorded --
/// otherwise the next repeat is judged against a value the chip no longer
/// holds and a real frequency change is dropped.
#[test]
fn a_kept_sn76489_latch_still_updates_the_shadow_register() {
    let s = stream(vec![
        0x50, 0x8F, // 0: tone 0 low nibble F
        0x50, 0x80, // 1: nibble 0 -- kept, the next byte continues it
        0x50, 0x3F, // 2: its high bits
        0x50, 0x8F, // 3: nibble F again. The chip holds 0, so this is real.
    ]);
    assert!(
        redundant_indices(&s, None).is_empty(),
        "a kept latch must not leave the shadow register holding the old nibble"
    );
}

/// The noise register reseeds the shift register on every write.
#[test]
fn the_sn76489_noise_register_keeps_every_write() {
    let s = stream(vec![
        0x50, 0xE7, // 0
        0x50, 0xE7, // 1: the same byte, another reseed
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

/// The Game Gear stereo latch shares the chip but not its register select.
#[test]
fn the_game_gear_stereo_latch_is_its_own_cell() {
    let s = stream(vec![
        0x4F, 0xFF, // 0
        0x4F, 0xFF, // 1: redundant
        0x50, 0x9F, // 2
        0x4F, 0xFF, // 3: still redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [1, 3]);
}

// -- indirection -------------------------------------------------------------

/// The RF5C68's `0x00`-`0x06` address whichever channel `0x07` last selected,
/// so the same address and the same byte are two different writes when the
/// selection moved between them.
#[test]
fn rf5c68_registers_follow_the_selected_channel() {
    let s = stream(vec![
        0xB0, 0x07, 0xC0, // 0: select channel 0
        0xB0, 0x01, 0xFF, // 1: its pan
        0xB0, 0x07, 0xC1, // 2: select channel 1
        0xB0, 0x01, 0xFF, // 3: a different channel's pan -- kept
        0xB0, 0x01, 0xFF, // 4: now it repeats
        0xB0, 0x07, 0xC0, // 5: back to channel 0
        0xB0, 0x01, 0xFF, // 6: channel 0 still holds 0xFF -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [4, 6]);
}

/// Wave RAM is poked through a window whose bank a register moves, so nothing
/// is dropped from it.
#[test]
fn rf5c68_wave_ram_keeps_every_write() {
    let s = stream(vec![
        0xC1, 0x00, 0x00, 0x40, // 0
        0xC1, 0x00, 0x00, 0x40, // 1: the same byte at the same offset
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

/// The HuC6280's waveform port advances an index with every write.
#[test]
fn the_huc6280_waveform_port_keeps_every_write() {
    let s = stream(vec![
        0xB9, 0x00, 0x00, // 0: select channel 0
        0xB9, 0x06, 0x10, // 1: a wave sample
        0xB9, 0x06, 0x10, // 2: the same sample again, one place further on
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

/// Its per-channel registers follow the channel select, as the RF5C's do.
#[test]
fn huc6280_registers_follow_the_selected_channel() {
    let s = stream(vec![
        0xB9, 0x00, 0x00, // 0: select channel 0
        0xB9, 0x04, 0x9F, // 1: its control byte
        0xB9, 0x00, 0x01, // 2: select channel 1
        0xB9, 0x04, 0x9F, // 3: another channel -- kept
        0xB9, 0x00, 0x01, // 4: the same selection again -- redundant
        0xB9, 0x04, 0x9F, // 5: and now the control byte repeats
    ]);
    assert_eq!(redundant_indices(&s, None), [4, 5]);
}

// -- forgetting --------------------------------------------------------------

/// Every Game Boy register does something on write that the value it already
/// holds does not excuse -- the mixer pair included, which forces a sample
/// update on all four channels. Nothing is dropped from it.
#[test]
fn the_game_boy_keeps_every_write() {
    let s = stream(vec![
        0xB3, 0x04, 0x87, // 0: NR14, a trigger
        0xB3, 0x04, 0x87, // 1: again -- a second note
        0xB3, 0x02, 0xF0, // 2: NR12, the envelope glitch register
        0xB3, 0x02, 0xF0, // 3
        0xB3, 0x15, 0xFF, // 4: NR51, panning
        0xB3, 0x15, 0xFF, // 5
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

// -- a sample of the per-chip judgements -------------------------------------

/// The SAA1099's envelope registers reload their generator. `vgm_cmp` judges
/// this chip with the YM2413's rules by way of a missing `break`, and drops
/// these; the built-in does not.
#[test]
fn the_saa1099_envelope_registers_keep_every_write() {
    let s = stream(vec![
        0xBD, 0x18, 0x80, // 0
        0xBD, 0x18, 0x80, // 1: reloads the envelope again
        0xBD, 0x00, 0x0F, // 2: an amplitude register
        0xBD, 0x00, 0x0F, // 3: a plain latch -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [3]);
}

/// The NES APU's `$4003` reloads the length counter and restarts the envelope.
#[test]
fn the_nes_length_counter_registers_keep_every_write() {
    let s = stream(vec![
        0xB4, 0x03, 0x08, // 0
        0xB4, 0x03, 0x08, // 1: another note
        0xB4, 0x00, 0x9F, // 2: the duty/volume register
        0xB4, 0x00, 0x9F, // 3: a plain latch -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [3]);
}

/// The SSG's envelope-shape register restarts the envelope; the tone registers
/// beside it do not.
#[test]
fn the_ssg_envelope_shape_keeps_every_write() {
    let s = stream(vec![
        0xA0, 0x0D, 0x0E, // 0
        0xA0, 0x0D, 0x0E, // 1: restarts it again
        0xA0, 0x00, 0x40, // 2: channel A period
        0xA0, 0x00, 0x40, // 3: a plain latch -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [3]);
}

/// SegaPCM's play cursor and channel-enable byte move under the driver's feet.
#[test]
fn the_segapcm_play_cursor_keeps_every_write() {
    let s = stream(vec![
        0xC0, 0x86, 0x00, 0x01, // 0: channel 0 flags
        0xC0, 0x86, 0x00, 0x01, // 1: the chip may have changed it since
        0xC0, 0x82, 0x00, 0x40, // 2: its volume
        0xC0, 0x82, 0x00, 0x40, // 3: a plain latch -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [3]);
}

/// The OKIM6295's command port is a two-write protocol whose second write means
/// what the first said.
#[test]
fn the_okim6295_command_port_keeps_every_write() {
    let s = stream(vec![
        0xB8, 0x00, 0x81, // 0: play sample 1
        0xB8, 0x00, 0x81, // 1: again
        0xB8, 0x0C, 0x01, // 2: the clock divider
        0xB8, 0x0C, 0x01, // 3: a plain latch -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [3]);
}

/// The MultiPCM writes through a slot select and a register select. Its
/// KEYONOFF register restarts the envelope every time it is written, and its
/// sample select reloads the LFO pair from the sample's own header, so neither
/// is a latch -- but the panpot beside them is.
#[test]
fn the_multipcm_writes_through_its_selects() {
    let s = stream(vec![
        0xB5, 0x01, 0x00, // 0: select slot 0
        0xB5, 0x02, 0x00, // 1: select register 0, the panpot
        0xB5, 0x00, 0x40, // 2: write it
        0xB5, 0x01, 0x01, // 3: select slot 1
        0xB5, 0x00, 0x40, // 4: another slot's panpot -- kept
        0xB5, 0x00, 0x40, // 5: now it repeats
        0xB5, 0x02, 0x01, // 6: select the sample register
        0xB5, 0x00, 0x10, // 7: load a sample
        0xB5, 0x00, 0x10, // 8: again -- it would reload the LFO pair
        0xB5, 0x02, 0x04, // 9: select KEYONOFF
        0xB5, 0x00, 0x80, // 10: key on
        0xB5, 0x00, 0x80, // 11: key on again -- a second note
    ]);
    assert_eq!(redundant_indices(&s, None), [5]);
}

/// The ES5505's paged register file: a page change makes the same address a
/// different register.
#[test]
fn es5505_registers_follow_the_page_select() {
    let s = stream(vec![
        0xBE, 0x0D, 0x00, // 0: page 0
        0xBE, 0x01, 0x20, // 1: a register on it
        0xBE, 0x0D, 0x20, // 2: another page
        0xBE, 0x01, 0x20, // 3: the same address, a different register -- kept
        0xBE, 0x01, 0x20, // 4: now it repeats
        0xBE, 0x0D, 0x00, // 5: back to page 0
        0xBE, 0x01, 0x20, // 6: page 0 still holds it -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [4, 6]);
}

// -- what carries no state ---------------------------------------------------

#[test]
fn waits_and_data_blocks_are_not_writes() {
    let s = stream(vec![
        0x61, 0x10, 0x00, // 0: a wait
        0x62, // 1
        0x70, // 2
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}
