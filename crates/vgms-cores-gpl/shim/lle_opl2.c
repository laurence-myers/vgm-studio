/* Pin access for YM3812-LLE, compiled against the upstream's own header.
 *
 * The LLE core is a die simulation: its API is the chip's pins, which are
 * fields of `fmopl2_t`. The Rust side keeps its no-struct-mirroring rule (the
 * allocation is opaque, sized by `sizeof` at compile time), so the field
 * access happens here instead -- in C, against the same header the upstream
 * was compiled with, where an upstream layout change is absorbed by
 * recompilation rather than silently misread.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "fmopl2.h"

/* Defined in the upstream's fmopl2.c, which declares no prototypes. */
void FMOPL2_Clock(fmopl2_t *chip);

size_t vgms_fmopl2_sizeof(void) { return sizeof(fmopl2_t); }
size_t vgms_fmopl2_alignof(void) {
	return offsetof(struct { char c; fmopl2_t member; }, member);
}

/* The bus and control pins, all at once. Levels are the electrical ones:
 * ic/cs/wr are active-low, so 1 means deasserted. */
void vgms_fmopl2_set_pins(fmopl2_t *chip, int ic, int cs, int wr, int a0,
                            int data) {
	chip->input.ic = ic;
	chip->input.cs = cs;
	chip->input.wr = wr;
	chip->input.rd = 1; /* never reading */
	chip->input.address = a0;
	chip->input.data_i = data;
}

/* Unlike the OPM/OPNA upstreams, this one reads the master-clock pin's level
 * from its input struct rather than a parameter -- presented here so the Rust
 * driver keeps the same clock_edge() shape as its siblings. */
void vgms_fmopl2_clock(fmopl2_t *chip, int mclk) {
	chip->input.mclk = mclk;
	FMOPL2_Clock(chip);
}

/* The serial DAC pins: the sample-and-hold strobe, the bit stream it frames,
 * and the serial bit clock that paces it. */
int vgms_fmopl2_out_sh(const fmopl2_t *chip) { return chip->o_sh; }
int vgms_fmopl2_out_mo(const fmopl2_t *chip) { return chip->o_mo; }
int vgms_fmopl2_out_sy(const fmopl2_t *chip) { return chip->o_sy; }
