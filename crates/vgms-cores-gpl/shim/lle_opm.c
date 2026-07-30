/* Pin access for YM2151-LLE, compiled against the upstream's own header.
 *
 * The LLE core is a die simulation: its API is the chip's pins, which are
 * fields of `fmopm_t`. The Rust side keeps its no-struct-mirroring rule (the
 * allocation is opaque, sized by `sizeof` at compile time), so the field
 * access happens here instead -- in C, against the same header the upstream
 * was compiled with, where an upstream layout change is absorbed by
 * recompilation rather than silently misread.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "fmopm.h"

size_t vgms_fmopm_sizeof(void) { return sizeof(fmopm_t); }
size_t vgms_fmopm_alignof(void) {
	return offsetof(struct { char c; fmopm_t member; }, member);
}

/* The bus and control pins, all at once. Levels are the electrical ones:
 * ic/cs/wr/rd are active-low, so 1 means deasserted. */
void vgms_fmopm_set_pins(fmopm_t *chip, int ym2164, int ic, int cs, int wr,
                            int a0, int data) {
	chip->input.ym2164 = ym2164;
	chip->input.ic = ic;
	chip->input.cs = cs;
	chip->input.wr = wr;
	chip->input.rd = 1; /* never reading */
	chip->input.a0 = a0;
	chip->input.data = data;
}

/* The serial DAC pins: sample-and-hold strobes for the two channels, and the
 * bit stream they frame. */
int vgms_fmopm_out_sh1(const fmopm_t *chip) { return chip->o_sh1; }
int vgms_fmopm_out_sh2(const fmopm_t *chip) { return chip->o_sh2; }
int vgms_fmopm_out_so(const fmopm_t *chip) { return chip->o_so; }
