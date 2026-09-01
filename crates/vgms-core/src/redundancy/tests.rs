//! What each rule shape promises, one shape at a time.
//!
//! Two families of fixture chip, chosen deliberately: the YMZ280B and YM2610
//! carry the *value-dedup* tests, because every core that can render them
//! applies writes immediately; the YM3812, YM2612 and SN76489 carry the
//! *paced-chip* tests, because [`write_timing_audible`] suspends dedup there
//! and leaves only the zero-wait override. Dedup fixtures put a wait between
//! writes wherever adjacency would let the override rule fire first -- each
//! test should exercise one rule, not both.

use super::*;
use crate::vgm::stream::END_OF_DATA;

fn stream(bytes: Vec<u8>) -> VgmStream {
    let mut bytes = bytes;
    bytes.push(END_OF_DATA);
    VgmStream::parse(bytes, 0x171).expect("a walkable stream")
}

// -- the ordinary latch ------------------------------------------------------

#[test]
fn a_repeated_write_to_a_latch_is_redundant() {
    let s = stream(vec![
        0x5D, 0x20, 0x01, // 0: YMZ280B, a pure latch
        0x62, // 1
        0x5D, 0x20, 0x01, // 2: the same value again -- redundant
        0x62, // 3
        0x5D, 0x20, 0x02, // 4: a different value
        0x62, // 5
        0x5D, 0x20, 0x02, // 6: and its repeat
    ]);
    assert_eq!(redundant_indices(&s, None), [2, 6]);
}

/// Registers that trigger on write rather than latching are never dropped,
/// even on a chip that has rules. The AY8910's envelope shape restarts the
/// envelope, so a value-identical repeat is kept; the period register beside
/// it is an ordinary latch.
#[test]
fn a_trigger_register_is_never_dropped() {
    let s = stream(vec![
        0xA0, 0x0D, 0x0E, // 0: envelope shape
        0x62, // 1
        0xA0, 0x0D, 0x0E, // 2: restarts it again -- kept
        0x62, // 3
        0xA0, 0x00, 0x40, // 4: channel A period
        0x62, // 5
        0xA0, 0x00, 0x40, // 6: its repeat -- dropped
    ]);
    assert_eq!(redundant_indices(&s, None), [6], "only the latch repeat");
}

/// Everything is forgotten at the loop point, so the loop body carries its own
/// state and sounds the same on the second pass.
#[test]
fn the_loop_point_forgets_every_cell() {
    let s = stream(vec![
        0x5D, 0x20, 0x01, // 0
        0x62, // 1
        0x5D, 0x20, 0x01, // 2: redundant
        0x62, // 3
        0x5D, 0x20, 0x01, // 4: the loop point -- kept
        0x62, // 5
        0x5D, 0x20, 0x01, // 6: redundant again
    ]);
    assert_eq!(redundant_indices(&s, Some(4)), [2, 6]);
}

/// Two instances of a chip are two sets of registers.
#[test]
fn a_second_instance_holds_its_own_values() {
    let s = stream(vec![
        0x5D, 0x20, 0x01, // 0: chip 1
        0x62, // 1
        0xAD, 0x20, 0x01, // 2: chip 2 -- not a repeat of chip 1
        0x62, // 3
        0xAD, 0x20, 0x01, // 4: now it is
    ]);
    assert_eq!(redundant_indices(&s, None), [4]);
}

/// Two ports of one chip are two sets of registers: the YM2610's second bank
/// shares its register numbers with the first.
#[test]
fn a_second_port_holds_its_own_values() {
    let s = stream(vec![
        0x58, 0x38, 0x01, // 0: YM2610 port 0
        0x62, // 1
        0x59, 0x38, 0x01, // 2: port 1 -- a different cell
        0x62, // 3
        0x59, 0x38, 0x01, // 4: now it repeats
    ]);
    assert_eq!(redundant_indices(&s, None), [4]);
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

// -- write-paced chips: no drops at all ---------------------------------------

/// A chip with a write-paced core in the registry keeps a value-identical
/// repeat however far apart the writes are: on that core the repeat's arrival
/// is itself part of the render, and the owner's rule is that optimisation
/// must be inaudible under every selectable core.
#[test]
fn a_paced_chips_value_repeat_is_kept() {
    let s = stream(vec![
        0x5A, 0x20, 0x01, // 0: YM3812, whose adapter buffers writes
        0x62, // 1
        0x5A, 0x20, 0x01, // 2: the same value again -- kept regardless
    ]);
    assert!(redundant_indices(&s, None).is_empty());
    assert!(write_timing_audible(ChipKind::Ym3812));
}

/// Even a setup-prefix override is kept on a paced chip. The corpus measured
/// why the tempting version fails: an init block's queue backlog outlives the
/// prefix, so dropping a dead write there shifted the song's opening notes on
/// 131 of 193 Mega Drive files.
#[test]
fn a_paced_chip_keeps_even_an_overridden_setup_write() {
    let s = stream(vec![
        0x5A, 0x20, 0x01, // 0: overridden before any time passes -- still kept
        0x5A, 0x20, 0x02, // 1
        0x62, // 2
        0x5A, 0x20, 0x02, // 3: a separated repeat -- kept too
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

// -- the zero-wait override (immediate-core chips, pure stores only) ----------

/// The OPN frequency latch is one chip-wide pure store, so `vgm_cmp`'s
/// dead-latch pattern -- an `0xA4` re-latched before any `0xA0` commits it --
/// falls out of the override rule on the immediate-core YM2610, ports included.
#[test]
fn an_opn_latch_overridden_before_its_commit_is_dropped() {
    let s = stream(vec![
        0x58, 0xA4, 0x22, // 0: latch, never committed
        0x59, 0xA4, 0x1A, // 1: the other port -- the same chip-wide latch
        0x58, 0xA0, 0x69, // 2: the commit -- keeps everything after it
        0x58, 0xA4, 0x22, // 3: latched again...
        0x58, 0xA0, 0x69, // 4: ...and committed, so nothing else drops
    ]);
    assert_eq!(redundant_indices(&s, None), [0]);
}

/// An intervening write on the same chip blocks the override -- it may have
/// observed the earlier value (an OPN commit reads the latch; a select
/// register's value steers the write after it).
#[test]
fn an_intervening_write_on_the_chip_blocks_the_override() {
    let s = stream(vec![
        0x58, 0x30, 0x01, // 0
        0x58, 0x34, 0x09, // 1: another register of the same chip
        0x58, 0x30, 0x02, // 2: no longer adjacent to 0 -- no drop
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

/// Another chip's writes do not block: they cannot observe this chip's state.
#[test]
fn another_chips_write_does_not_block_the_override() {
    let s = stream(vec![
        0x58, 0x30, 0x01, // 0: YM2610
        0xB4, 0x00, 0x9F, // 1: a NES write in between
        0x58, 0x30, 0x02, // 2: still overrides 0
    ]);
    assert_eq!(redundant_indices(&s, None), [0]);
}

/// The override is opt-in per register, and a register without the pure-store
/// blessing never fires it -- here the YM2610's `0x22` (LFO), dedup-only.
#[test]
fn an_unblessed_register_is_never_overridden() {
    let s = stream(vec![
        0x58, 0x22, 0x08, // 0: overridden -- but not a pure store, kept
        0x58, 0x22, 0x09, // 1
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

/// On a dedup chip the two rules compose without double-dropping: a same-value
/// adjacent pair loses the *second* write (dedup), never both. The fixture is a
/// YM2610 operator register -- a pure store on an immediate-core chip, so both
/// rules can reach it.
#[test]
fn dedup_and_override_never_drop_both_writes() {
    let s = stream(vec![
        0x58, 0x30, 0x05, // 0: kept -- something must write the value
        0x58, 0x30, 0x05, // 1: the dedup's drop
        0x58, 0x30,
        0x07, // 2: overrides 0? No -- 0 must survive for 1's sake;
              //    but 1 is dropped, so 2 overrides 0 legitimately
    ]);
    // Walk it: 1 dedups against 0; 2 then lands in 0's cell with zero time
    // passed and only the dropped 1 between, so 0 is dead too. The chip gets
    // exactly one write: value 7, at the same instant. Correct and minimal.
    assert_eq!(redundant_indices(&s, None), [0, 1]);
}

/// A register that is safe to dedup can still be unsafe to override: the OPN's
/// `0x28` key register holds per-channel state selected by the *data*, so an
/// init block's key-off run (`00, 01, 02, ...`) is writes to six different
/// things through one address. The corpus caught the collapse on 132 of 193
/// Mega Drive files; this is that shape, in miniature.
#[test]
fn a_key_off_run_is_never_collapsed() {
    let s = stream(vec![
        0x52, 0x28, 0x00, // 0: key off channel 0
        0x52, 0x28, 0x01, // 1: key off channel 1 -- NOT an override of 0
        0x52, 0x28, 0x02, // 2: key off channel 2
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

// -- the SN76489's two-byte protocol -----------------------------------------

/// The SN76489 has paced cores, so a separated volume repeat -- the classic
/// every-frame driver write -- is kept now.
#[test]
fn a_separated_sn76489_repeat_is_kept() {
    let s = stream(vec![
        0x50, 0x9F, // 0: channel 0 volume off
        0x62, // 1
        0x50, 0x9F, // 2: again -- kept, the chip's cores pace writes
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

/// Even adjacent same-register latches are kept: the SN76489 is a paced chip,
/// so no register write is ever dropped from it.
#[test]
fn an_adjacent_sn76489_latch_is_kept() {
    let s = stream(vec![
        0x50, 0x9F, // 0: channel 0 volume off...
        0x50, 0x90, // 1: ...immediately loud instead -- both kept
        0x50, 0xBF, // 2: channel 1
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

/// A latch byte whose next chip byte is a continuation carries the register
/// select that continuation needs, so it is never override-dropped -- it is
/// classified kept-and-recorded, which also ends the chip's window.
#[test]
fn an_sn76489_latch_a_continuation_depends_on_is_kept() {
    let s = stream(vec![
        0x50, 0x8F, // 0: tone 0, low nibble F
        0x50, 0x80, // 1: low nibble 0 -- would override 0, but a continuation
        //    follows, so it is kept-and-recorded instead
        0x50, 0x3F, // 2: the continuation
        0x50, 0x8F, // 3: pending was cleared at 1 -- kept
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

/// The noise register reseeds the shift register on every write.
#[test]
fn the_sn76489_noise_register_keeps_every_write() {
    let s = stream(vec![
        0x50, 0xE7, // 0
        0x50, 0xE7, // 1: the same byte, another reseed -- and no override
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

/// The Game Gear stereo latch shares the chip's fate: the SN76489 is paced, so
/// even its plainest register keeps every write.
#[test]
fn the_game_gear_stereo_latch_keeps_every_write() {
    let s = stream(vec![
        0x4F, 0xFF, // 0
        0x4F, 0xFF, // 1: adjacent repeat -- kept
        0x62, // 2
        0x4F, 0xFF, // 3: separated repeat -- kept
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

// -- the OPN commit latch (dedup side, on the immediate-core YM2610) ---------

/// `0xA4` latches the block and F-number high bits; the `0xA0` that follows
/// commits the pair. A low-byte write is therefore never redundant, however
/// familiar its value -- dropping one after a re-latch leaves the channel at
/// the old pitch.
#[test]
fn an_opn_low_byte_write_is_never_dropped() {
    let s = stream(vec![
        0x58, 0xA4, 0x22, // 0: latch high
        0x62, // 1
        0x58, 0xA0, 0x69, // 2: commit
        0x62, // 3
        0x58, 0xA4, 0x1A, // 4: a different high byte
        0x62, // 5
        0x58, 0xA0, 0x69, // 6: the same low byte -- but a different note
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
        0x58, 0xA4, 0x22, // 0
        0x62, // 1
        0x58, 0xA4, 0x22, // 2: the same latch, the same value -- redundant
        0x62, // 3
        0x58, 0xA5, 0x22, // 4: the same value, a different address -- kept
        0x62, // 5
        0x58, 0xA4, 0x22, // 6: the group moved since -- kept
    ]);
    assert_eq!(redundant_indices(&s, None), [2]);
}

/// `0xAC`-`0xAE` are the second latch, not the first: a write to one does not
/// invalidate the other.
#[test]
fn the_two_opn_latch_groups_are_separate() {
    let s = stream(vec![
        0x58, 0xA4, 0x22, // 0
        0x62, // 1
        0x58, 0xAC, 0x22, // 2: the other group
        0x62, // 3
        0x58, 0xA4, 0x22, // 4: still the last write to its own group
    ]);
    assert_eq!(redundant_indices(&s, None), [4]);
}

/// But the group is one latch for the whole chip, ports included: `0x59` is
/// the YM2610's port 1, and its `0xA4` overwrites what port 0 latched.
#[test]
fn the_opn_latch_group_spans_both_ports() {
    let s = stream(vec![
        0x58, 0xA4, 0x22, // 0: port 0
        0x62, // 1
        0x59, 0xA4, 0x22, // 2: port 1 -- the same latch, a different address
        0x62, // 3
        0x58, 0xA4, 0x22, // 4: port 0 again, and the latch moved since
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
        0x62, // 4
        0xB0, 0x01, 0xFF, // 5: now it repeats
        0xB0, 0x07, 0xC0, // 6: back to channel 0
        0x62, // 7
        0xB0, 0x01, 0xFF, // 8: channel 0 still holds 0xFF -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [5, 8]);
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
        0x62, // 4
        0xB9, 0x00, 0x01, // 5: the same selection again -- redundant
        0xB9, 0x04, 0x9F, // 6: and now the control byte repeats
    ]);
    assert_eq!(redundant_indices(&s, None), [5, 6]);
}

// -- a sample of the per-chip judgements -------------------------------------

/// The Game Boy has no pure latches: every register does something on write
/// that the value it already holds does not excuse, the mixer pair included
/// (SameBoy forces a sample update on all four channels for `NR50`/`NR51`).
/// Nothing is dropped from it -- not even an adjacent override, since no
/// register is Latch-classified.
#[test]
fn the_game_boy_keeps_every_write() {
    let s = stream(vec![
        0xB3, 0x04, 0x87, // 0: NR14, a trigger
        0xB3, 0x04, 0x87, // 1: again -- a second note
        0xB3, 0x15, 0xFF, // 2: NR51, panning
        0xB3, 0x15, 0xFF, // 3
    ]);
    assert!(redundant_indices(&s, None).is_empty());
}

/// The SAA1099's envelope registers reload their generator. `vgm_cmp` judges
/// this chip with the YM2413's rules by way of a missing `break`, and drops
/// these; the built-in does not.
#[test]
fn the_saa1099_envelope_registers_keep_every_write() {
    let s = stream(vec![
        0xBD, 0x18, 0x80, // 0
        0xBD, 0x18, 0x80, // 1: reloads the envelope again
        0xBD, 0x00, 0x0F, // 2: an amplitude register
        0x62, // 3
        0xBD, 0x00, 0x0F, // 4: a plain latch -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [4]);
}

/// The NES APU's `$4003` reloads the length counter and restarts the envelope.
#[test]
fn the_nes_length_counter_registers_keep_every_write() {
    let s = stream(vec![
        0xB4, 0x03, 0x08, // 0
        0xB4, 0x03, 0x08, // 1: another note
        0xB4, 0x00, 0x9F, // 2: the duty/volume register
        0x62, // 3
        0xB4, 0x00, 0x9F, // 4: a plain latch -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [4]);
}

/// The SSG's envelope-shape register restarts the envelope; the tone registers
/// beside it do not.
#[test]
fn the_ssg_envelope_shape_keeps_every_write() {
    let s = stream(vec![
        0xA0, 0x0D, 0x0E, // 0
        0xA0, 0x0D, 0x0E, // 1: restarts it again
        0xA0, 0x00, 0x40, // 2: channel A period
        0x62, // 3
        0xA0, 0x00, 0x40, // 4: a plain latch -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [4]);
}

/// SegaPCM's play cursor and channel-enable byte move under the driver's feet.
#[test]
fn the_segapcm_play_cursor_keeps_every_write() {
    let s = stream(vec![
        0xC0, 0x86, 0x00, 0x01, // 0: channel 0 flags
        0xC0, 0x86, 0x00, 0x01, // 1: the chip may have changed it since
        0xC0, 0x82, 0x00, 0x40, // 2: its volume
        0x62, // 3
        0xC0, 0x82, 0x00, 0x40, // 4: a plain latch -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [4]);
}

/// The OKIM6295's command port is a two-write protocol whose second write means
/// what the first said.
#[test]
fn the_okim6295_command_port_keeps_every_write() {
    let s = stream(vec![
        0xB8, 0x00, 0x81, // 0: play sample 1
        0xB8, 0x00, 0x81, // 1: again
        0xB8, 0x0C, 0x01, // 2: the clock divider
        0x62, // 3
        0xB8, 0x0C, 0x01, // 4: a plain latch -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [4]);
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
        0x62, // 5
        0xB5, 0x00, 0x40, // 6: now it repeats
        0xB5, 0x02, 0x01, // 7: select the sample register
        0xB5, 0x00, 0x10, // 8: load a sample
        0xB5, 0x00, 0x10, // 9: again -- it would reload the LFO pair
        0xB5, 0x02, 0x04, // 10: select KEYONOFF
        0xB5, 0x00, 0x80, // 11: key on
        0xB5, 0x00, 0x80, // 12: key on again -- a second note
    ]);
    assert_eq!(redundant_indices(&s, None), [6]);
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
        0x62, // 4
        0xBE, 0x01, 0x20, // 5: now it repeats
        0xBE, 0x0D, 0x00, // 6: back to page 0
        0x62, // 7
        0xBE, 0x01, 0x20, // 8: page 0 still holds it -- redundant
    ]);
    assert_eq!(redundant_indices(&s, None), [5, 8]);
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
