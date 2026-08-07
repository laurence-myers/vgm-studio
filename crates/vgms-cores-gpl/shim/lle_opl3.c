/* Pin access for YMF262-LLE, compiled against the upstream's own header.
 *
 * The LLE core is a die simulation: its API is the chip's pins, which are
 * fields of `fmopl3_t`. The Rust side keeps its no-struct-mirroring rule (the
 * allocation is opaque, sized by `sizeof` at compile time), so the field
 * access happens here instead -- in C, against the same header the upstream
 * was compiled with, where an upstream layout change is absorbed by
 * recompilation rather than silently misread.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "fmopl3.h"

/* Defined in the upstream's fmopl3.c, which declares no prototypes. */
void FMOPL3_Clock(fmopl3_t *chip);

size_t vgms_fmopl3_sizeof(void) { return sizeof(fmopl3_t); }
size_t vgms_fmopl3_alignof(void) {
	return offsetof(struct { char c; fmopl3_t member; }, member);
}

/* The bus and control pins, all at once. Levels are the electrical ones:
 * ic/cs/wr are active-low, so 1 means deasserted. The OPL3 has two address
 * lines: a0 picks address/value, a1 the register bank. */
void vgms_fmopl3_set_pins(fmopl3_t *chip, int ic, int cs, int wr, int a0,
                            int a1, int data) {
	chip->input.ic = ic;
	chip->input.cs = cs;
	chip->input.wr = wr;
	chip->input.rd = 1; /* never reading */
	chip->input.address = (a0 & 1) | ((a1 & 1) << 1);
	chip->input.data_i = data;
}

/* As the OPL2 upstream: the master-clock pin's level lives in the input
 * struct, presented here so the Rust driver keeps its clock_edge() shape. */
void vgms_fmopl3_clock(fmopl3_t *chip, int mclk) {
	chip->input.mclk = mclk;
	FMOPL3_Clock(chip);
}

/* The serial DAC pins, packed: the two data lines (DOAB carries the A and B
 * words, DOCD the C and D), the bit clock, and the two sample strobes. */
int vgms_fmopl3_dac_pins(const fmopl3_t *chip) {
	return (chip->o_doab != 0) | ((chip->o_docd != 0) << 1) |
	       ((chip->o_sy != 0) << 2) | ((chip->o_smpac != 0) << 3) |
	       ((chip->o_smpbd != 0) << 4);
}
