/* What the C side thinks the chip structs measure.
 *
 * `ffi.rs` mirrors `cqm_t` field for field so the struct can be allocated on
 * the Rust side, which means a layout disagreement is memory corruption rather
 * than a compile error -- the one thing about the binding that reading it
 * cannot catch.
 *
 * So the test asks the compiler that actually compiled the upstream, rather
 * than comparing against a size copied out of the header. A constant copied out
 * of a header drifts exactly when the header does, which is the case it is
 * supposed to catch.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "cqm.h"

size_t drotrim_cqm_sizeof(void) { return sizeof(cqm_t); }

size_t drotrim_cqm_alignof(void) {
	/* No <stdalign.h> (not freestanding-guaranteed): the classic offsetof
	 * trick gives the alignment the compiler actually uses. */
	struct probe { char c; cqm_t chip; };
	return offsetof(struct probe, chip);
}
