/* Pin access for YMF276-LLE, compiled against the upstream's own header.
 *
 * The LLE core is a die simulation: its API is the chip's pins, which are
 * fields of `fmopn2_t`. The Rust side keeps its no-struct-mirroring rule (the
 * allocation is opaque, sized by `sizeof` at compile time), so the field
 * access happens here instead -- in C, against the same header the upstream
 * was compiled with, where an upstream layout change is absorbed by
 * recompilation rather than silently misread.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "fmopn2.h"

size_t vgms_fmopn2_sizeof(void) { return sizeof(fmopn2_t); }
size_t vgms_fmopn2_alignof(void) {
	return offsetof(struct { char c; fmopn2_t member; }, member);
}

/* The die's build flags: 0 is the YMF276 configuration, whose serial DAC is
 * the only pin-level audio output this upstream drives. */
void vgms_fmopn2_set_flags(fmopn2_t *chip, int flags) { chip->flags = flags; }

/* The bus and control pins, all at once. The caller's levels are the
 * electrical ones (ic/cs/wr active-low, 1 meaning deasserted), as on every
 * sibling die -- but unlike them, THIS upstream models the pins logically:
 * `input.ic` true means reset asserted, `cs && wr` means a write is on. The
 * inversion happens here, once, so the Rust driver keeps one convention.
 * The OPN2 has two address lines: a0 picks address/value, a1 the bank. */
void vgms_fmopn2_set_pins(fmopn2_t *chip, int ic, int cs, int wr, int a0,
                            int a1, int data) {
	chip->input.ic = !ic;
	chip->input.cs = !cs;
	chip->input.wr = !wr;
	chip->input.rd = 0; /* never reading */
	chip->input.address = (a0 & 1) | ((a1 & 1) << 1);
	chip->input.data = data;
	chip->input.test = 0;
}

/* The serial DAC pins, packed: bit clock, word clock, left/right select and
 * the data line. */
int vgms_fmopn2_dac_pins(const fmopn2_t *chip) {
	return (chip->o_bco != 0) | ((chip->o_wco != 0) << 1) |
	       ((chip->o_lro != 0) << 2) | ((chip->o_so != 0) << 3);
}
