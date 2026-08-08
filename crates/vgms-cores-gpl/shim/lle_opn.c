/* Pin access for YM2203-LLE, compiled against the upstream's own header.
 *
 * The LLE core is a die simulation: its API is the chip's pins, which are
 * fields of `fmopn_t`. The Rust side keeps its no-struct-mirroring rule (the
 * allocation is opaque, sized by `sizeof` at compile time), so the field
 * access happens here instead -- in C, against the same header the upstream
 * was compiled with, where an upstream layout change is absorbed by
 * recompilation rather than silently misread.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "fmopn.h"

size_t vgms_fmopn_sizeof(void) { return sizeof(fmopn_t); }
size_t vgms_fmopn_alignof(void) {
	return offsetof(struct { char c; fmopn_t member; }, member);
}

/* The bus and control pins, all at once. Levels are the electrical ones:
 * ic/cs/wr are active-low, so 1 means deasserted. The GPIO ports are the
 * SSG's parallel I/O, unconnected on a sound board: held low. */
void vgms_fmopn_set_pins(fmopn_t *chip, int ic, int cs, int wr, int a0,
                            int data) {
	chip->input.ic = ic;
	chip->input.cs = cs;
	chip->input.wr = wr;
	chip->input.rd = 1; /* never reading */
	chip->input.a0 = a0;
	chip->input.data = data;
	chip->input.gpio_a = 0;
	chip->input.gpio_b = 0;
}

/* The FM serial DAC pins packed (sh | opo<<1 | sy<<2), and the three SSG
 * channels' analog levels summed into one, as a mono board mixes them. */
int vgms_fmopn_dac_pins(const fmopn_t *chip, float *analog) {
	*analog = chip->o_analog_a + chip->o_analog_b + chip->o_analog_c;
	return (chip->o_sh != 0) | ((chip->o_opo != 0) << 1) |
	       ((chip->o_sy != 0) << 2);
}
