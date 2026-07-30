/* Pin access for the YM2612 die of YM2608-LLE, compiled against the
 * upstream's own header.
 *
 * Same arrangement and same reasoning as lle_opm.c: the Rust side keeps its
 * allocation opaque and this file, compiled with the header the upstream was
 * compiled with, does the field access.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "fmopna_2612.h"

size_t drotrim_fmopna2612_sizeof(void) { return sizeof(fmopna_2612_t); }
size_t drotrim_fmopna2612_alignof(void) {
	return offsetof(struct { char c; fmopna_2612_t member; }, member);
}

/* The bus and control pins. ic/cs/wr/rd are active-low levels; a1 picks the
 * register bank, a0 address-or-data; test idles at 1 per the header note. */
void drotrim_fmopna2612_set_pins(fmopna_2612_t *chip, int ic, int cs, int wr,
                                 int a0, int a1, int data) {
	chip->input.ic = ic;
	chip->input.cs = cs;
	chip->input.wr = wr;
	chip->input.rd = 1; /* never reading */
	chip->input.a0 = a0;
	chip->input.a1 = a1;
	chip->input.data = data;
	chip->input.test = 1;
}

/* The time-multiplexed 9-bit DAC outputs, ladder asymmetry included. */
int drotrim_fmopna2612_out_mol(const fmopna_2612_t *chip) {
	return chip->o_mol;
}
int drotrim_fmopna2612_out_mor(const fmopna_2612_t *chip) {
	return chip->o_mor;
}
