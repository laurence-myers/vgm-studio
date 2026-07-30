/* Pin access for the YM2608 die of YM2608-LLE, compiled against the
 * upstream's own header.
 *
 * Same arrangement as lle_opm.c and lle_opn2.c, plus this package's
 * external memory: the Delta-T sample store hangs off a DRAM-style bus
 * (RAS/CAS multiplexed address on the dm lines, WE for writes) that the
 * wrapper serves. The rhythm section's ROM is *internal* -- the decap
 * carries it -- so no pins are needed for the drums at all.
 *
 * The 2610 configuration of this upstream does not compile (unguarded
 * 2608-only GPIO writes at the pinned commit, a different error one commit
 * earlier -- checked 2026-07-28), so the 2608 die is the family's OPNA
 * witness for now.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "fmopna_2608.h"

size_t drotrim_fmopna2608_sizeof(void) { return sizeof(fmopna_t); }
size_t drotrim_fmopna2608_alignof(void) {
	return offsetof(struct { char c; fmopna_t member; }, member);
}

/* The bus and control pins. ic/cs/wr/rd active low; test idles at 1; the
 * analog loop (ad/da), serial test (dt0) and GPIO inputs idle at 0. The
 * DRAM data-in pin is set separately so serving memory does not disturb a
 * write in flight. */
void drotrim_fmopna2608_set_pins(fmopna_t *chip, int ic, int cs, int wr,
                                 int a0, int a1, int data) {
	chip->input.ic = ic;
	chip->input.cs = cs;
	chip->input.wr = wr;
	chip->input.rd = 1; /* never reading */
	chip->input.a0 = a0;
	chip->input.a1 = a1;
	chip->input.data = data;
	chip->input.test = 1;
	chip->input.gpio_a = 0;
	chip->input.gpio_b = 0;
	chip->input.dt0 = 0;
	chip->input.ad = 0;
	chip->input.da = 0;
}

/* The served Delta-T memory byte, on the DRAM data-in lines. */
void drotrim_fmopna2608_serve_dm(fmopna_t *chip, int dm) {
	chip->input.dm = dm;
}

/* The DRAM bus as the die drives it: the multiplexed address/data lines
 * and their extra ninth bit, with the strobes packed in the return --
 * bit 0 = ras, bit 1 = cas, bit 2 = we (all active low), bit 3 = the
 * direction latch (1 when the die expects data in). */
int drotrim_fmopna2608_dram_pins(const fmopna_t *chip, int *dm, int *a8) {
	*dm = chip->o_dm;
	*a8 = chip->o_a8;
	return (chip->o_ras & 1) | ((chip->o_cas & 1) << 1) |
	       ((chip->o_we & 1) << 2) | ((chip->o_dm_d & 1) << 3);
}

/* The serial DAC strobes and data, and the analog (SSG) pin. */
int drotrim_fmopna2608_dac_pins(const fmopna_t *chip, float *analog) {
	*analog = chip->o_analog;
	/* Packed: bit 0 = sh1, bit 1 = sh2, bit 2 = opo (serial data),
	 * bit 3 = s (the serial bit clock). */
	return (chip->o_sh1 & 1) | ((chip->o_sh2 & 1) << 1) |
	       ((chip->o_opo & 1) << 2) | ((chip->o_s & 1) << 3);
}
