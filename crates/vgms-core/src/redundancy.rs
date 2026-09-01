//! Which register writes a VGM can lose without changing what it plays --
//! under **any** core the user can select, not merely the one selected now.
//!
//! The optimiser's first phase walks the command stream holding a shadow copy
//! of every chip's registers, and drops a write whose cell already holds the
//! value being written. That rule is only sound twice over:
//!
//! 1. The register must *latch*: a write with no effect beyond storing the
//!    byte. A register that **triggers** -- a key that re-attacks, a counter
//!    that reloads, a FIFO that advances, an address the chip itself moves
//!    while playing -- must keep every write, because the second one does
//!    something the first did not, and the failure is silent: the file gets
//!    smaller and plays wrong.
//! 2. Every core that can render the chip must apply writes the instant they
//!    arrive. A **write-paced** core (a Nuked wrapper's queue, the OPL
//!    adapter's buffer) renders audio *between* zero-wait writes, so removing
//!    even a value-identical repeat shifts every write behind it a slot
//!    earlier -- audibly. [`write_timing_audible`] is that list; those chips
//!    drop no register writes at all, and get only the delay merge.
//!
//! So every chip is classified register by register, in [`classify`]. The
//! verdict is one of two:
//!
//! * `Keep` -- never dropped.
//! * `Latch` -- droppable (on an immediate-core chip) when the named cell
//!   already holds the named value, or -- for the pure-store subset -- when
//!   the very next command on the chip overwrites the cell before any time
//!   passes.
//!
//! # Cells, not addresses
//!
//! A *cell* is whatever piece of chip state the write lands in, which is not
//! always "this address on this port":
//!
//! * **Indirected registers.** The RF5C68's `0x00`-`0x06`, the HuC6280's
//!   `0x02`-`0x07` and the MultiPCM's data port address whichever channel or
//!   slot a select register last pointed at. Their cell carries that selection,
//!   so a repeat only counts as one when the same channel is selected.
//! * **Shared latches.** The OPN family's `0xA4`-`0xA6` write one latch that
//!   the whole group shares; `0xAC`-`0xAE` write another. Their cell is the
//!   *group*, and the value carries the address as well as the byte -- so a
//!   write is redundant only when the last write to that latch was this same
//!   address with this same value. That holds whether the hardware latch is
//!   shared or per-register, which is what makes it safe without settling the
//!   question.
//! * **Banked files.** The ES5505's registers live behind a page select whose
//!   number differs between the 5505 and the 5506. Both candidates are folded
//!   into the cell, so a write to either forces the next write to be kept.
//!
//! # Where the rules come from
//!
//! `vgm_cmp`'s `chip_cmp.c` is two decades of this same classification, and it
//! is the primary source. Where it is known to be wrong or is switched off
//! (the SAA1099 fallthrough, the Game Boy's unconditional keep, the YM3812
//! waveform flush that never runs, the YM2608 prescaler compare that never
//! assigns) this module is stricter, never looser: it keeps writes upstream
//! drops rather than the other way round.
//!
//! Everything here is then checked against the corpus by
//! `the_builtin_optimizer_never_changes_audio`, which renders every file before
//! and after and requires the samples to be byte-identical. **Three chips it
//! cannot check**: the corpus mirror holds no SCSP, Mikey or ES5505 material at
//! all, so their rules are reasoned rather than measured. On the interactive
//! paths the per-file render gate still stands behind them; on the unverified
//! paths they are the three worth doubting first if a file ever comes back
//! wrong. See `docs/optimizer-coverage-2026-09/REPORT.md`.

use std::collections::BTreeMap;

use crate::vgm::ChipKind;
use crate::vgm::stream::{ChipTarget, MEMORY_PORT, VgmCommand, VgmStream};

/// What may become of one write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Kept whatever the chip already holds, and leaving nothing this map has
    /// to remember.
    Keep,
    /// Kept whatever the chip already holds -- but it *does* leave a value in a
    /// cell the map tracks, so record it.
    ///
    /// The distinction matters wherever one cell can be reached by both a
    /// keep and a latch. The SN76489 is the reason it exists: the same latch
    /// byte is droppable or not depending on what *follows* it, and a kept one
    /// that went unrecorded left the shadow register holding a value the chip
    /// no longer had -- so the next repeat was dropped against a stale cache
    /// and the tone came out at the wrong pitch (caught by the corpus gate,
    /// peak 29792).
    KeepAndRecord { cell: u64, value: u32 },
    /// Dropped when `cell` already holds `value` (an immediate-core chip), or
    /// when a kept write to the same cell follows before any time passes and
    /// `store` is true.
    ///
    /// `store` is the override rule's opt-in: true only for a **pure store**,
    /// where the value fully replaces the cell's state and no bit edges on a
    /// change. A register can be safe to *dedup* and still unsafe to override:
    /// the OPN's `0x28` holds per-channel key state selected by the data (so
    /// two "same-cell" writes may address different channels), and key or
    /// rhythm bits pulse an edge on the way through an intermediate value even
    /// in zero elapsed time. The corpus gate proved both failure modes: init
    /// key-off runs collapsed to one channel on 132 of 193 Mega Drive files
    /// before this flag existed.
    Latch { cell: u64, value: u32, store: bool },
}

/// A verdict, plus whether the write invalidates everything else on its chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Decision {
    verdict: Verdict,
    /// The write resets or reprograms the chip, so no earlier cell can be
    /// trusted afterwards. QSound's `0xE3`, which picks an update routine and
    /// may reset the part, is the one.
    forgets_chip: bool,
}

const fn keep() -> Decision {
    Decision {
        verdict: Verdict::Keep,
        forgets_chip: false,
    }
}

/// [`keep`], and forget everything else this chip instance holds.
const fn keep_and_forget() -> Decision {
    Decision {
        verdict: Verdict::Keep,
        forgets_chip: true,
    }
}

/// The ordinary cell: this address, on this port. Dedup-eligible only.
const fn latch(port: u8, addr: u16, data: u16) -> Decision {
    cell(plain_cell(port, addr), data as u32)
}

/// A cell named by hand -- an indirected register, or a shared latch.
/// Dedup-eligible only.
const fn cell(cell: u64, value: u32) -> Decision {
    Decision {
        verdict: Verdict::Latch {
            cell,
            value,
            store: false,
        },
        forgets_chip: false,
    }
}

/// A **pure store** at this address: the value fully replaces the cell's state
/// and no bit edges on a change, so it is override-eligible as well as
/// dedup-eligible.
const fn store(port: u8, addr: u16, data: u16) -> Decision {
    store_cell(plain_cell(port, addr), data as u32)
}

/// A pure store into a hand-named cell.
const fn store_cell(cell: u64, value: u32) -> Decision {
    Decision {
        verdict: Verdict::Latch {
            cell,
            value,
            store: true,
        },
        forgets_chip: false,
    }
}

/// Never dropped, but it still leaves `value` in `cell`.
const fn keep_recording(cell: u64, value: u32) -> Decision {
    Decision {
        verdict: Verdict::KeepAndRecord { cell, value },
        forgets_chip: false,
    }
}

const fn plain_cell(port: u8, addr: u16) -> u64 {
    ((port as u64) << 16) | addr as u64
}

/// A cell that is not "address on port": tagged out of that range so the two
/// spaces cannot collide within one chip.
const fn indirect_cell(tag: u64) -> u64 {
    0x1_0000_0000 | tag
}

/// Per-chip state a single address cannot carry: what a select register last
/// selected, and what a two-byte protocol last latched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Aux {
    /// SN76489: the register a bit-7-clear continuation byte extends.
    sn_reg: u8,
    /// RF5C68/RF5C164 and HuC6280: the channel their indirected registers
    /// address.
    channel: u8,
    /// MultiPCM: the slot its data port writes through.
    slot: u8,
    /// MultiPCM: the slot register its data port writes through.
    slot_reg: u8,
    /// ES5505/ES5506: the two candidate page-select bytes, folded into every
    /// other cell of the chip.
    page: u16,
}

/// Whether this module has redundancy rules for `chip`.
///
/// True for every chip the format defines: each one is classified below, even
/// where the classification is "keep nearly everything". What that buys a
/// caller is the routing decision -- a file whose chips are all covered needs
/// no external optimiser to dedupe its writes.
#[must_use]
pub const fn has_latch_rules(_chip: ChipKind) -> bool {
    true
}

/// Whether any core this app can select for `chip` makes the *timing* of a
/// register write audible -- so removing even a value-identical repeat can
/// change what plays.
///
/// A write-paced core queues register writes and releases one per settling
/// period, because the real chip's busy flag stalls its bus that way. During a
/// zero-wait burst it therefore renders audio *between* the writes: drop one
/// and every write behind it lands a slot earlier, which is audible whenever
/// the transient is. The owner's rule (2026-09): optimisation must never change
/// what a file plays under **any** selectable core -- so a chip with even one
/// paced core in the registry drops **no register writes at all** -- only the
/// delay merge touches its files. Even the setup-prefix override was measured
/// audible on these chips (the init backlog outlives the prefix; see
/// [`redundant_indices`]).
///
/// The list is every chip a paced core is registered for, wrapper by wrapper:
///
/// - **The OPL family** (`YM3812`, `YM3526`, `Y8950`, `YMF262` --
///   `vgms_synth::registry::OPL_CHIPS`): `OplCoreAdapter` sends playback
///   through Nuked-OPL3's `OPL3_WriteRegBuffered`, and the CQM, OPL2-Lite and
///   both LLE OPL cores ride the same adapter or pace on their own pins.
/// - **`YM2612`**: the Nuked-OPN2 wrapper's `WriteQueue`, and both LLE dies.
/// - **`YM2151`**: the Nuked-OPM wrapper's `WriteQueue`, and the LLE die.
/// - **`YM2413`**: the Nuked-OPLL wrapper's `WriteQueue`.
/// - **`SN76489`**: the Nuked-PSG wrapper's byte queue and settle.
/// - **`YM2203`** / **`YM2608`**: the LLE OPN/OPNA dies.
///
/// The `YM2610` and `YMF278B` render through libvgm alone, which applies
/// writes the instant they arrive, so they are *not* here -- and a new core
/// registration for any chip must revisit this list before it ships.
#[must_use]
pub const fn write_timing_audible(chip: ChipKind) -> bool {
    use ChipKind as K;
    matches!(
        chip,
        K::Ym3812
            | K::Ym3526
            | K::Y8950
            | K::Ymf262
            | K::Ym2612
            | K::Ym2151
            | K::Ym2413
            | K::Sn76489
            | K::Ym2203
            | K::Ym2608
    )
}

/// The writes that can be dropped without changing what is heard, ascending.
///
/// Both rules apply only to chips whose every selectable core applies writes
/// immediately; a [`write_timing_audible`] chip drops **nothing** here, and
/// gets only the delay merge:
///
/// - **Value dedup**: a write is redundant when the rules call its register a
///   pure latch and the cell it lands in already holds that value.
/// - **Zero-wait override**, on [`store`]-classified registers only: a write is
///   dead when the very next command touching the same chip instance -- with no
///   time passing and nothing but other chips' writes in between -- is a kept
///   write to the same cell. The value never lands anywhere audible, because an
///   immediate core renders no samples between the two.
///
/// The owner's sanctioned exception -- a setup write immediately overridden --
/// was tried on the paced chips and the corpus refused it: a Mega Drive init
/// block is hundreds of writes, the paced queue's backlog outlives the setup
/// prefix, and the shifted drain collided with the song's opening notes on 131
/// of 193 files. So the exception survives only where it is provably invisible.
///
/// The strictness of the override rule is deliberate twice over. Requiring the
/// overriding write to be the very next command on that chip means no
/// intervening write can have observed the dropped one -- not an OPN commit
/// reading the latch, not an SN76489 continuation byte reading the register
/// select, not a MultiPCM data write reading the slot registers. And requiring
/// a [`store`] classification means the value really is the whole story -- see
/// [`Verdict::Latch`] for the two ways "same cell, overwritten" can lie.
///
/// `loop_at` is the row the song loops back to, if any. Every cell is forgotten
/// there, so the loop body re-establishes its own state and still sounds right
/// on the second pass through -- the same rule `chip_cmp` applies.
#[must_use]
pub fn redundant_indices(stream: &VgmStream, loop_at: Option<usize>) -> Vec<usize> {
    let mut held: BTreeMap<(ChipKind, u8, u64), u32> = BTreeMap::new();
    let mut aux: BTreeMap<(ChipKind, u8), Aux> = BTreeMap::new();
    // Per chip instance: the last still-surviving latch write, and its cell --
    // the candidate the next same-cell write on that chip may override. Cleared
    // by any passage of time, by anything that is not a register write, and by
    // any kept non-latch write on the chip.
    let mut pending: BTreeMap<(ChipKind, u8), (usize, u64)> = BTreeMap::new();
    let mut redundant = Vec::new();
    // Built the first time a chip needs to see past the write in hand, which
    // today is only the SN76489. Most files never pay for it.
    let mut successors: Option<Vec<Option<usize>>> = None;

    for index in 0..stream.len() {
        if loop_at == Some(index) {
            held.clear();
            aux.clear();
            pending.clear();
        }
        let Some(VgmCommand::Write { target, addr, data }) = stream.get(index) else {
            // A wait of zero samples changes nothing and blocks nothing; any
            // other command -- real waits, DAC writes (which are YM2612 writes
            // with a wait attached), data blocks, stream control -- ends every
            // chip's zero-wait window.
            if !matches!(stream.get(index), Some(VgmCommand::Wait(0))) {
                pending.clear();
            }
            continue;
        };
        let next = if target.kind == ChipKind::Sn76489 {
            let table = successors.get_or_insert_with(|| successors_by_target(stream));
            next_sn_byte(stream, table, index)
        } else {
            None
        };
        let state = aux.entry((target.kind, target.instance)).or_default();
        let decision = classify(target, addr, data, state, next);
        let chip_key = (target.kind, target.instance);
        if decision.forgets_chip {
            held.retain(|(kind, instance, _), _| {
                *kind != target.kind || *instance != target.instance
            });
        }
        match decision.verdict {
            Verdict::Keep => {
                pending.remove(&chip_key);
            }
            Verdict::KeepAndRecord { cell, value } => {
                held.insert((target.kind, target.instance, cell), value);
                pending.remove(&chip_key);
            }
            // A paced chip's Latch verdicts drop nothing at all: not the value
            // dedup (the repeat's arrival is itself rendered) and not the
            // zero-wait override -- the corpus killed even the setup-prefix
            // version of that. A Mega Drive init block is hundreds of writes,
            // and the paced queue's backlog outlives the prefix: the drain is
            // still running when the first notes key on, so a dropped prefix
            // write shifted audible content on 131 of 193 files (divergence in
            // the first ~1000 samples on every one).
            Verdict::Latch { .. } if write_timing_audible(target.kind) => {}
            Verdict::Latch { cell, value, store } => {
                let key = (target.kind, target.instance, cell);
                let previous = held.insert(key, value);
                if previous == Some(value) {
                    // The value dedup. A dropped write neither overrides the
                    // pending write nor replaces it: the pending one is what
                    // still carries the value.
                    redundant.push(index);
                } else if store {
                    if let Some(&(overridden, pending_cell)) = pending.get(&chip_key)
                        && pending_cell == cell
                    {
                        // The zero-wait override: this kept write lands in the
                        // same cell with no time passed and nothing on the chip
                        // between, so the previous write's value was never
                        // observable -- the chip's cores apply writes
                        // immediately, so not a single sample rendered between
                        // the two.
                        redundant.push(overridden);
                    }
                    pending.insert(chip_key, (index, cell));
                } else {
                    // A dedup-only latch: it neither offers itself for
                    // override nor lets a write ride over it.
                    pending.remove(&chip_key);
                }
            }
        }
    }
    // An override drops an *earlier* index than the write that triggered it,
    // so with several chips interleaved the pushes are not globally ordered.
    redundant.sort_unstable();
    redundant
}

/// For each command, the next command in the stream that writes the same chip
/// instance and port.
fn successors_by_target(stream: &VgmStream) -> Vec<Option<usize>> {
    let mut next = vec![None; stream.len()];
    let mut last: BTreeMap<(ChipKind, u8, u8), usize> = BTreeMap::new();
    for index in (0..stream.len()).rev() {
        if let Some(VgmCommand::Write { target, .. }) = stream.get(index) {
            next[index] = last.insert((target.kind, target.instance, target.port), index);
        }
    }
    next
}

/// The next byte written to this SN76489's data port, skipping its Game Gear
/// stereo latch -- which shares the port but not the register-select state.
fn next_sn_byte(stream: &VgmStream, next: &[Option<usize>], from: usize) -> Option<u16> {
    let mut at = next[from];
    while let Some(index) = at {
        if let Some(VgmCommand::Write { addr: 0, data, .. }) = stream.get(index) {
            return Some(data);
        }
        at = next[index];
    }
    None
}

/// What may become of one write to `target`.
///
/// `next` is the next byte the same chip receives, where the chip's rules need
/// it; `aux` is its running select state. Both are threaded in rather than
/// looked up here so this stays one flat table of per-register judgements.
#[allow(clippy::too_many_lines)]
fn classify(
    target: ChipTarget,
    addr: u16,
    data: u16,
    aux: &mut Aux,
    next: Option<u16>,
) -> Decision {
    use ChipKind as K;
    let port = target.port;
    match target.kind {
        // -- the PSGs ---------------------------------------------------------
        K::Sn76489 => match addr {
            // The Game Gear stereo latch, which is a plain register.
            1 => store(port, 1, data),
            _ => sn76489(data, aux, next),
        },
        K::Ay8910 => ay8910(port, addr, data),
        K::Saa1099 => match addr {
            // A write to either envelope register reloads its generator. This
            // is the rule `vgm_cmp` misses, its SAA1099 case falling through
            // into the YM2413's.
            0x18 | 0x19 => keep(),
            0x00..=0x1F => latch(port, addr, data),
            _ => keep(),
        },
        K::Pokey => match addr & 0x0F {
            // STIMER resets the counters, SKREST and POTGO strobe, SEROUT
            // starts a transfer, IRQEN and SKCTL reset the serial logic.
            0x09..=0x0B | 0x0D..=0x0F => keep(),
            reg => latch(port, reg, data),
        },
        // The Game Boy has no pure latches at all -- the one chip here that is
        // trigger all the way down. The core this app ships for it is SameBoy,
        // which models the DMG's write-time behaviour, and it fires on the value
        // the register already holds: NR10 runs a sweep calculation on every
        // write, NRx2 runs the "zombie mode" envelope glitch whenever its
        // channel is active, NRx3 reloads the sample countdown if the counter
        // just reloaded, NR43 has a page of counter-alignment quirks, NR30 can
        // corrupt wave RAM through a *random* index, NRx1 reloads the length
        // counter, NRx4 retriggers and NR52 powers the chip down. Even the
        // mixer pair is not exempt: NR50 and NR51 force a sample update on
        // every channel, and the corpus gate caught a rule that let them dedupe
        // (1918 dropped NR50 writes, audible from sample 143390).
        //
        // `vgm_cmp` keeps every Game Boy write too, its handler switched off
        // with a note about not knowing what breaks real hardware. This is the
        // same answer with the reason measured.
        K::GameBoyDmg => keep(),
        K::NesApu => nes_apu(port, addr, data),
        K::Vsu => match addr {
            // The wave-table RAM.
            0x000..=0x0FF => latch(port, addr, data),
            // The stop-all register and everything above it.
            0x160.. => keep(),
            // Mode (bit 7 keys on), and the frequency and envelope registers,
            // which the sweep and envelope units rewrite behind the driver.
            _ => match addr & 0x0F {
                0x00 | 0x02..=0x05 => keep(),
                _ => latch(port, addr, data),
            },
        },
        K::WonderSwan => {
            if port == MEMORY_PORT {
                // The wave RAM, whose window moves with a register this map
                // does not track.
                return keep();
            }
            match addr {
                // Channel-2 frequency under sweep, and channel-1's PCM value.
                0x04 | 0x05 | 0x09 => keep(),
                // Sweep time reloads the countdown; the noise type resets its
                // shift register.
                0x0D | 0x0E => keep(),
                0x00..=0x1F => latch(port, addr, data),
                _ => keep(),
            }
        }

        // -- the OPL family ---------------------------------------------------
        //
        // Every register latches, key-on included: the `0xB0` key bit is
        // level-sensitive, so re-writing it does not re-attack. OPL3's second
        // bank arrives on its own port and is therefore its own cell.
        //
        // The operator ranges, the F-number lows, the algorithm block and the
        // waveform selects are pure stores; `0xB0`-`0xB8` and `0xBD` are not --
        // their key and rhythm bits pulse an edge on the way through an
        // intermediate value even in zero time -- and neither are the timers
        // and modes below `0x20`.
        K::Ym3812 | K::Ym3526 | K::Y8950 | K::Ymf262 => match addr {
            0x20..=0xAF | 0xC0..=0xFF => store(port, addr, data),
            _ => latch(port, addr, data),
        },
        K::Ymf278b => {
            if port < 2 {
                // The OPL3-compatible FM side.
                latch(port, addr, data)
            } else {
                // The OPL4 wave side, whose registers this app has not checked.
                keep()
            }
        }
        // The YM2413's `0x20`-`0x28` carry the key-on bits.
        K::Ym2413 => match addr {
            // `0x20`-`0x28` carry the key-on bits.
            0x20..=0x28 => keep(),
            // The user patch, the F-number lows and the instrument/volume
            // selects: pure stores. `0x0E` (rhythm key bits) is not.
            0x00..=0x07 | 0x10..=0x18 | 0x30..=0x38 => store(port, addr, data),
            _ => latch(port, addr, data),
        },

        // -- the OPN family ---------------------------------------------------
        K::Ym2612 => match (port, addr) {
            // The DAC data port: every write is a sample.
            (0, 0x2A) => keep(),
            _ => opn_fm(port, addr, data),
        },
        K::Ym2203 => match addr {
            0x00..=0x0F => ssg(port, addr, data),
            // The prescaler registers select a mode by *being written*; there
            // is no value to compare.
            0x2D..=0x2F => keep(),
            _ => opn_fm(port, addr, data),
        },
        K::Ym2608 => match (port, addr) {
            (0, 0x00..=0x0F) => ssg(port, addr, data),
            // The rhythm key-on / dump register restarts its samples.
            (0, 0x10) => keep(),
            (0, 0x11..=0x1F) => latch(port, addr, data),
            (0, 0x2D..=0x2F) => keep(),
            (1, 0x00..=0x0F) => delta_t(port, addr, addr, data),
            // Delta-T flag control.
            (1, 0x10) => keep(),
            _ => opn_fm(port, addr, data),
        },
        K::Ym2610 => match (port, addr) {
            (0, 0x00..=0x0F) => ssg(port, addr, data),
            (0, 0x10..=0x1B) => delta_t(port, addr, addr - 0x10, data),
            (0, 0x1C..=0x1F) => keep(),
            // The ADPCM-A key on/off register restarts its samples.
            (1, 0x00) => keep(),
            (1, 0x01..=0x2F) => latch(port, addr, data),
            _ => opn_fm(port, addr, data),
        },
        // The OPM. Its test register's bit 1 resets the LFO phase.
        K::Ym2151 => match addr {
            0x01 => keep(),
            // The channel and operator registers are pure stores. Below 0x20
            // sit the key register (`0x08`, channel in the data), the AMD/PMD
            // pair (`0x19`, target in the data's top bit), the timers and the
            // LFO -- dedup-only.
            0x20..=0xFF => store(port, addr, data),
            _ => latch(port, addr, data),
        },
        K::Ymf271 => ymf271(port, addr, data),

        // -- the samplers -----------------------------------------------------
        K::SegaPcm => {
            let offset = addr & 0x07FF;
            // Sixteen channels of eight registers, mirrored through the window.
            match offset & !0x78 {
                // The play cursor and the channel-enable byte: the chip moves
                // these itself while a voice is running, so what the file wrote
                // last is not what the chip now holds.
                0x04 | 0x05 | 0x84..=0x86 => keep(),
                _ => latch(port, offset, data),
            }
        }
        K::Rf5c68 | K::Rf5c164 => {
            if port == MEMORY_PORT {
                // Direct pokes into wave RAM, whose bank moves with register
                // `0x07`.
                return keep();
            }
            match addr {
                0x07 => {
                    if data & 0x40 != 0 {
                        aux.channel = (data & 0x07) as u8;
                    }
                    latch(port, addr, data)
                }
                0x08 => latch(port, addr, data),
                0x00..=0x06 => cell(
                    indirect_cell((u64::from(aux.channel) << 4) | u64::from(addr)),
                    data as u32,
                ),
                _ => keep(),
            }
        }
        K::HuC6280 => match addr & 0x0F {
            0x00 => {
                aux.channel = (data & 0x07) as u8;
                latch(port, 0x00, data)
            }
            0x01 | 0x08 | 0x09 => latch(port, addr & 0x0F, data),
            // Every write pushes one more sample into the waveform table and
            // advances its index.
            0x06 => keep(),
            reg @ (0x02..=0x05 | 0x07) => cell(
                indirect_cell((u64::from(aux.channel) << 4) | u64::from(reg)),
                data as u32,
            ),
            _ => keep(),
        },
        K::MultiPcm => multi_pcm(port, addr, data, aux),
        K::Ymz280b => {
            if addr >= 0x80 {
                // The memory-access block, whose data port auto-increments.
                keep()
            } else {
                latch(port, addr, data)
            }
        }
        K::Upd7759 => match addr {
            // The reset and start lines, which are levels rather than strobes.
            0x00 | 0x01 => latch(port, addr, data),
            // The sample FIFO, and ports this app has not identified.
            _ => keep(),
        },
        K::Okim6258 => match addr {
            // Start/stop, the ADPCM data port, and the pan register -- which
            // upstream also keeps unless `-do6258` says the file has already
            // been through `opt_oki`.
            0x00..=0x02 => keep(),
            // The master clock only reaches the chip on the fourth byte, so it
            // must survive even when its own byte did not change; and the clock
            // divider is read at points this map cannot see.
            0x0B | 0x0C => keep(),
            0x08..=0x0A => latch(port, addr, data),
            _ => keep(),
        },
        K::Okim6295 => match addr {
            // The command port: a play is two writes, and what the second means
            // depends on the first.
            0x00 => keep(),
            0x0B => keep(),
            0x08..=0x13 => latch(port, addr, data),
            _ => keep(),
        },
        K::K051649 => match port {
            // The waveform RAM, and the test register.
            0x00 | 0x04 | 0x05 => keep(),
            // A frequency write can reset the channel's counter.
            0x01 => keep(),
            0x02 | 0x03 => latch(port, addr, data),
            _ => keep(),
        },
        K::K054539 => {
            let reg = (u16::from(port) << 8) | addr;
            match reg {
                // Key on and key off, which the chip clears as samples end.
                0x214 | 0x215 => keep(),
                // The RAM pointer advances on every write.
                0x22D => keep(),
                0x230.. => keep(),
                // The position-index registers latch inside the chip and are
                // committed at key-on.
                _ if reg < 0x100 && (0x0C..=0x0E).contains(&(addr & 0x1F)) => keep(),
                _ => cell(u64::from(reg), data as u32),
            }
        }
        K::K053260 => match addr {
            // The ROM read port advances, and the control register strobes.
            0x2E | 0x2F => keep(),
            0x00..=0x2D => latch(port, addr, data),
            _ => keep(),
        },
        K::C140 => {
            let reg = ((u16::from(port) << 8) | addr) & 0x1FF;
            // Voice register 5's bit 7 is the key-on.
            if reg < 0x180 && reg & 0x0F == 0x05 {
                keep()
            } else {
                cell(u64::from(reg), data as u32)
            }
        }
        K::QSound => match addr {
            // The play cursor, which the chip advances as it plays.
            _ if addr < 0x80 && matches!(addr & 0x07, 0x01 | 0x03) => keep(),
            // The ADPCM start triggers.
            0xD6..=0xD8 => keep(),
            // "Recalculate delays", and the update-routine select, which may
            // reset the chip.
            0xE2 => keep(),
            0xE3 => keep_and_forget(),
            _ => latch(port, addr, data),
        },
        K::Scsp => {
            // Each of the 32 slots holds its control word in its first two
            // bytes, and writing it executes a pending key on or off.
            if addr < 0x400 && addr & 0x1E == 0x00 {
                keep()
            } else if (0x400..0x410).contains(&addr) {
                // The common control block: master volume, MIDI, DMA.
                keep()
            } else {
                latch(port, addr, data)
            }
        }
        K::Es5503 => {
            let reg = addr & 0xFF;
            // The oscillator control page, which the chip rewrites as
            // oscillators halt, and the interrupt/enable block above it.
            if reg & 0xE0 == 0xA0 || reg >= 0xE2 {
                keep()
            } else {
                latch(port, reg, data)
            }
        }
        K::Es5505 => es5505(port, addr, data, aux),
        K::X1010 => {
            // A voice's status byte carries the key-on flag, which the chip
            // clears itself when a one-shot finishes.
            if addr < 0x80 && addr & 0x07 == 0x00 {
                keep()
            } else {
                latch(port, addr, data)
            }
        }
        K::C352 => {
            if addr >= 0x200 {
                // The control block, including the refresh register that
                // commits channel writes.
                keep()
            } else if addr & 0x07 >= 0x03 {
                // Flags and the four address registers, which a linked loop
                // makes the chip rewrite.
                keep()
            } else {
                latch(port, addr, data)
            }
        }
        K::Ga20 => match addr & 0x07 {
            // A write to the control register restarts the voice from its
            // start address; register 7 is the read-only voice status.
            0x06 | 0x07 => keep(),
            _ => latch(port, addr, data),
        },
        K::Pwm => match addr {
            // The control and cycle registers. Everything else is a FIFO push.
            0x00 | 0x01 => latch(port, addr, data),
            _ => keep(),
        },
        K::Mikey => match addr {
            // Below the audio block the core ignores the write entirely.
            0x00..=0x1F => keep(),
            0x20..=0x3F => match addr & 0x07 {
                // Backup, control and counter each schedule a timer action.
                0x04..=0x06 => keep(),
                _ => latch(port, addr, data),
            },
            _ => latch(port, addr, data),
        },
    }
}

/// The SN76489's two-byte protocol, whose register travels in the data.
///
/// A byte with bit 7 set latches a register (bits 6-4) and its low four bits; a
/// byte with bit 7 clear extends the last-latched register with six more. So
/// dropping a latch byte moves what the *next* continuation byte writes, and a
/// latch is only droppable when the next byte to this chip is another latch --
/// which is what `next` is for. By induction the first latch that survives
/// before any continuation byte re-establishes the register select, so the chip
/// and this model never disagree about it when it matters.
fn sn76489(data: u16, aux: &mut Aux, next: Option<u16>) -> Decision {
    let byte = data as u8;
    // Each register is two cells, because the two byte shapes write different
    // halves of it: the latch byte the low four bits, a continuation the high
    // six. Every write records what it left in its half, dropped or not -- a
    // kept-but-unrecorded latch is what made the shadow register go stale.
    let low = |reg: u8| indirect_cell(u64::from(reg));
    let high = |reg: u8| indirect_cell(0x10 | u64::from(reg));

    if byte & 0x80 == 0 {
        // A continuation of the last-latched register.
        return match aux.sn_reg {
            // The three tone periods take six more bits of frequency.
            // Re-writing one does not restart its counter.
            0 | 2 | 4 => store_cell(high(aux.sn_reg), u32::from(byte & 0x3F)),
            // On every other register a continuation replaces the low four
            // bits, as a latch byte does. Rare enough that upstream prints a
            // warning when it sees one, and on the noise register it reseeds
            // the shift register -- so it is kept, but it still counts.
            reg => keep_recording(low(reg), u32::from(byte & 0x0F)),
        };
    }
    let reg = (byte >> 4) & 0x07;
    aux.sn_reg = reg;
    let value = u32::from(byte & 0x0F);
    // Writing the noise register reseeds the shift register even for the value
    // it already holds.
    if reg == 0x06 {
        return keep_recording(low(reg), value);
    }
    // A latch byte also *selects* the register a following continuation
    // extends, so dropping one is only safe when the next byte this chip sees
    // is another latch. By induction the last latch before any continuation is
    // therefore always kept, and the chip's register select never diverges from
    // `aux.sn_reg` where it matters.
    if next.is_none_or(|byte| byte & 0x80 != 0) {
        store_cell(low(reg), value)
    } else {
        keep_recording(low(reg), value)
    }
}

/// The AY8910, whose register file every OPN part carries as its SSG section.
fn ay8910(port: u8, addr: u16, data: u16) -> Decision {
    if port != 0 {
        // The `0x31` stereo mask, which is an instruction to the player.
        return latch(port, addr, data);
    }
    ssg(port, addr & 0x0F, data)
}

/// One SSG register.
fn ssg(port: u8, reg: u16, data: u16) -> Decision {
    match reg {
        // Writing the envelope shape restarts the envelope.
        0x0D => keep(),
        // The two general-purpose I/O ports, which drive whatever the board
        // wired to them.
        0x0E | 0x0F => keep(),
        _ => latch(port, reg, data),
    }
}

/// The FM half of an OPN part: the registers from `0x30` up, shared by the
/// YM2612, YM2203, YM2608 and YM2610.
///
/// `0xA4`-`0xA6` (and `0xAC`-`0xAE`) latch the block and the F-number's high
/// bits; the *following* `0xA0`-`0xA2` write commits the pair to a channel. So
/// a low-byte write is never redundant -- dropping one leaves the channel at
/// the pitch the previous commit gave it -- and a latch write is redundant only
/// when it repeats the last write to the whole latch group, address and value
/// both. This is the pairing `vgm_cmp` handles with forward look-ahead, and the
/// one whose per-address model corrupted 25 of 500 corpus files before the rule
/// was disabled.
///
/// The two groups are **one latch each for the whole chip**, not one per port:
/// libvgm's `fmopn.c` holds them as `OPN->ST.fn_h` and `OPN->SL3.fn_h`, both on
/// the chip rather than the port, so a `0xA4` on port 1 overwrites what a
/// `0xA4` on port 0 latched. Keying the group by port instead would drop the
/// second of two port-0 latches that a port-1 latch had overwritten between
/// them -- which is exactly what a Mega Drive driver does, every frame, and
/// what the corpus render gate caught.
fn opn_fm(port: u8, addr: u16, data: u16) -> Decision {
    match addr & 0xF4 {
        // A0-A2 and A8-AA: the commit.
        0xA0 if addr & 0x03 != 0x03 => keep(),
        // A4-A6 and AC-AE: the latch, keyed by its chip-wide group. The value
        // carries the port and the address, so only a verbatim repeat of the
        // last write to that latch counts as one. Overwriting the latch is a
        // pure store -- nothing reads it until a commit, and the commit ends
        // the override window.
        0xA4 if addr & 0x03 != 0x03 => store_cell(
            indirect_cell(u64::from(addr & 0x08)),
            (u32::from(port) << 24) | (u32::from(addr) << 16) | data as u32,
        ),
        // The operator block and the algorithm/pan block: pure stores. Below
        // 0x30 sit the key register (`0x28`, channel in the data), the LFO and
        // the DAC controls -- dedup-only.
        _ => match addr {
            0x30..=0x9F | 0xB0..=0xB2 | 0xB4..=0xB6 => store(port, addr, data),
            _ => latch(port, addr, data),
        },
    }
}

/// One Delta-T (ADPCM-B) register, wherever a part maps the block.
///
/// `cell_addr` is the address to key by, `reg` the register's number within the
/// block.
fn delta_t(port: u8, cell_addr: u16, reg: u16, data: u16) -> Decision {
    match reg {
        // Start, reset and the memory mode: a strobe, not a level.
        0x00 => keep(),
        // The ADPCM data port.
        0x08 => keep(),
        0x0E.. => keep(),
        _ => latch(port, cell_addr, data),
    }
}

/// The YMF271's four FM ports, its PCM port and its group/timer port.
///
/// The registers a sync group mirrors -- the key-on register, and the
/// frequency, address and note registers -- are kept, because which slots a
/// write reaches depends on a group mode this map does not track.
fn ymf271(port: u8, addr: u16, data: u16) -> Decision {
    match port {
        0x00..=0x03 => {
            if addr & 0x03 == 0x03 {
                return keep();
            }
            match (addr >> 4) & 0x0F {
                0 | 9 | 10 | 12 | 13 | 14 => keep(),
                _ => latch(port, addr, data),
            }
        }
        // The PCM registers.
        0x04 => latch(port, addr, data),
        // The timers, beside group registers that reprogram whole slots and an
        // external-memory pointer that advances.
        0x06 if (0x10..=0x13).contains(&addr) => latch(port, addr, data),
        _ => keep(),
    }
}

/// The ES5505/ES5506, whose register file is paged.
///
/// The page register is number `0x0D` on one part and `0x0F` on the other, and
/// the stream does not say which part this is. Both bytes go into every other
/// cell, so a write to either forces the next write through -- correct for
/// whichever the file meant, at the cost of some dedup on the other.
fn es5505(port: u8, addr: u16, data: u16, aux: &mut Aux) -> Decision {
    match addr & 0x7F {
        0x0D => {
            aux.page = (aux.page & 0xFF00) | (data & 0x00FF);
            latch(port, addr, data)
        }
        0x0F => {
            aux.page = (aux.page & 0x00FF) | ((data & 0x00FF) << 8);
            latch(port, addr, data)
        }
        reg => cell(
            indirect_cell((u64::from(port) << 24) | (u64::from(aux.page) << 8) | u64::from(reg)),
            data as u32,
        ),
    }
}

/// The MultiPCM, whose data port writes through two selects.
///
/// `0xB5 01 dd` selects a slot, `0xB5 02 dd` a register within it, and
/// `0xB5 00 dd` writes the byte. Both selects are plain latches -- writing the
/// selection the chip already has changes nothing -- so they dedupe, while the
/// data write is keyed by the pair they name.
fn multi_pcm(port: u8, addr: u16, data: u16, aux: &mut Aux) -> Decision {
    if port != 0 {
        // The `0xC3` bank select, which is its own little register file.
        return latch(port, addr, data);
    }
    match addr {
        0x00 => {
            match aux.slot_reg {
                // Slot register 1 selects the *sample*, and loading one writes
                // registers 6 and 7 (the LFO pair) from the sample's own header
                // and retriggers the voice if it is playing. So a repeat is not
                // redundant twice over -- and 6 and 7 are kept beside it,
                // because this map cannot say what a sample load left in them.
                // `vgm_cmp` has the same handler, commented out.
                0x01 | 0x06 | 0x07 => return keep(),
                // Register 4 is KEYONOFF, and a key-on restarts the envelope
                // however many times it is written.
                0x04 => return keep(),
                _ => {}
            }
            cell(
                indirect_cell((u64::from(aux.slot) << 8) | u64::from(aux.slot_reg)),
                data as u32,
            )
        }
        0x01 => {
            aux.slot = (data & 0x1F) as u8;
            latch(port, addr, data)
        }
        0x02 => {
            // The chip clamps the register select to 0-7, so this must too:
            // an out-of-range select still lands on register 7, and register 7
            // is one of the ones that must be kept.
            aux.slot_reg = (data as u8).min(7);
            latch(port, addr, data)
        }
        _ => keep(),
    }
}

/// The NES APU, and the FDS block above it.
fn nes_apu(port: u8, addr: u16, data: u16) -> Decision {
    match addr {
        // `$4003`/`$4007`/`$400B`/`$400F` reload the length counter and restart
        // the envelope; `$4002`/`$4006` are rewritten by a running sweep;
        // `$400E` reseeds the noise; `$4011` is a direct DAC write and `$4012`
        // the DPCM address; `$4015` must be resent while the DPCM channel runs;
        // `$4017` reclocks the frame counter and so the envelopes.
        0x02 | 0x03 | 0x06 | 0x07 | 0x0B | 0x0E | 0x0F | 0x11 | 0x12 | 0x15 | 0x17 => keep(),
        // The FDS registers that reset a timer, a phase or a table pointer.
        0x20 | 0x23..=0x25 | 0x27 | 0x28 | 0x2A => keep(),
        0x00..=0x3F => latch(port, addr, data),
        _ => keep(),
    }
}

#[cfg(test)]
mod tests;
