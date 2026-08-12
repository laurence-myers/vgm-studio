# Plan: DRO v1 opcode disambiguation (gap audit H5)

Status: **planned, not implemented.** A first implementation was built on
2026-08-12, caught three real design conflicts in the test suite, and was
reverted in favour of this plan. Gaps H1-H4/H6 landed independently; this is
the one high-severity audit item still open.

## The gap (from GAP-AUDIT-2026-08.md, H5)

DRO v1's opcodes collide with OPL register numbers: `0x01` is both "long
delay" and the waveform-select register, `0x04` both "escape" and the timer
control register. DOSBox wrote low-register writes **unescaped** (the
developers changed the docs but not the code), so a real v1 capture contains
raw `01 xx` / `04 xx` pairs. Our decoder is unconditional (`0x01` = 3-byte
delay, `0x04` = 3-byte escape), so one unescaped pair shifts the parse by one
byte and everything after decodes as garbage.

VGMPlay disambiguates (`droplayer.cpp`, `ScanInitBlock` + the v1 `case 0x01`/
`case 0x04` at ~990-1025) with two devices:

- **Init-block scan**: the file starts with a register dump in ascending
  register order. Phase 1 walks it (chip-selects `02`/`03` exempt, the port
  joins the ordering as bit 8); phase 2 continues past the first descending
  register up to the first delay opcode or a *proper* escape (`04 op` with
  `op < 0x08`). Inside the block, `01`/`04` are register writes.
- **Operand-pattern test** (outside the block): `01 a b` is an unescaped
  write iff `!(a & ~0x20) && (b == 0x08 || b >= 0x20)`; `04 op` is a direct
  write iff `op >= 0x08`.

## Why a verbatim port is wrong for this app (found by the test suite)

There are two disjoint populations of v1 files, and the reference heuristic
serves only one:

1. **Raw DOSBox captures** — contain unescaped low-register writes and *no*
   `0x04` escapes (DOSBox never wrote any; droplayer's comment confirms
   escapes come from editing tools). The heuristic exists for these.
2. **Tool-written v1 files** — our `dro2_to_dro1` output and anything this
   app's predecessor (DRO Trimmer) ever saved — escape *every* low-register
   write and use `0x01` only as a genuine delay.

VGMPlay's phase 1 has no escape awareness, so it consumes a leading
`04 01 20` (escaped reg 0x01 = 0x20 — our converter's first output for the
lsl3 fixture) as a 2-byte write to register 4 and desyncs. **VGMPlay misplays
tool-written v1 files; a verbatim port makes us misplay our own output.**
`convert::tests::dro2_to_dro1_converts_the_fixture` caught exactly this.

Two more findings from the attempt:

- **Editing invariants**: re-running the scan after `delete_many` /
  `insert_many` re-interprets surviving `01`/`04` instructions whenever an
  edit moves the init-block boundary, breaking the delete-then-insert
  exact-inverse contract. Proptest pinned `selection = [0, 3]` on the v1
  fixture (delete the first register write + a bank switch) as the shrunken
  counterexample.
- **Reference detail the audit overstated**: the `01`-as-write *positional*
  rule only bites at the head of the dump (phase 1 consumes `01` while the
  last register is <= 1; phase 2 *terminates* at any `01`), so mid-file the
  operand-pattern test governs alone. Boundary arithmetic (reference tests
  the operand offset, an implementation naturally tests the opcode offset)
  was verified equivalent because the scan only ends on instruction
  boundaries.

## Agreed design

1. **Scan at load, pin sizes in the index map.** Run the (modified) init-block
   scan once in `DroDataV1::scan_index_map`; the `01`/`04` decision is stored
   as each instruction's *size*. `get()` reads the size back (`0x01` of size
   2 = register write, size 3 = delay; `0x04` of size 3 = escape, size 2 =
   direct write) and never re-decides.
2. **Escape-terminate phase 1** — the one deliberate divergence from the
   reference: both phases stop at a proper escape (`04 op`, `op < 0x08`), so
   a tool-written file whose first low-register write is escaped gets
   `init_block_end == 0` and decodes exactly as today. Raw captures are
   unaffected (they contain no escapes). Comment this as a deliberate
   improvement over droplayer, with this file as the reference.
3. **Edits shift, never rescan.** `delete_many`/`insert_many` rebuild the
   index map from the surviving instructions' *preserved sizes* (drop the
   deleted sizes / splice in each re-inserted entry's byte length, then
   prefix-sum). Surviving bytes keep their meaning; delete-then-insert stays
   an exact inverse. Re-add the proptest regression entry
   (`cc 43b1db6a...` / `selection = [0, 3]`) when re-implementing.
4. **Harden `dro2_to_dro1`**: a generated long delay whose bytes match the
   pattern test (`01 {00|20} {08|>=20}` — an 8-second-plus delay with low
   byte exactly 0x00/0x20) is misread by VGMPlay *and* the new decoder. Split
   such a delay in two (total preserved) so our output is unambiguous under
   both players.
5. **Adjust the two synthetic tests** that start a stream with a bare long
   delay (`v1_long_delay_is_little_endian`,
   `a_long_delay_becomes_several_wait_commands`): a head-of-file `01` is
   dump territory (the reference eats it as a register write too), so prefix
   a register write. Add: a capture-shaped stream decodes per the reference;
   an escaped-head stream decodes as today; the scan terminator fires on a
   proper escape.

## Residual, format-inherent ambiguities (document, do not chase)

- A capture whose dump writes register 4 with a value < 8 ends our scan at
  that byte; later unescaped low-`04` writes before the first delay would be
  read as escapes. The reference's phase 2 stops at the same byte, so the
  divergence window is a few bytes of a vanishingly rare shape.
- An *edit* that moves ambiguous bytes to the head of the stream changes
  their meaning on save-and-reload. True in VGMPlay too; inherent to v1.
  Worth a doc note in `dro_data.rs`, possibly a save-time warning later.

## Affected code

- `crates/vgms-core/src/song/dro_data.rs` — scan, sizes, `get()`, edit paths.
- `crates/vgms-core/src/convert.rs` — `dro2_to_dro1` delay splitting; tests.
- `crates/vgms-core/src/io/dro.rs` — unchanged (raw bytes still stored
  verbatim; `new_truncating` semantics unchanged).
