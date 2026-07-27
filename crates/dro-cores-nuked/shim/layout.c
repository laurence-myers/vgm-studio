/* How big each upstream chip struct is, and how it wants to be aligned.
 *
 * The Rust side allocates the state and hands the upstream a pointer to it, but
 * never declares a twin of the struct -- see `src/opaque.rs` for why. That
 * leaves exactly two numbers to get across the boundary, and asking the
 * compiler that actually compiled the upstream is the only way to get them
 * right by construction: a size copied out of a header drifts precisely when
 * the header does, which is the case worth defending against on a submodule
 * that exists to be pulled.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "cqm.h"
#include "ym3438.h"

/* `offsetof` past a char gives the alignment the compiler actually uses;
 * <stdalign.h> is not guaranteed in a freestanding build. */
#define DROTRIM_ALIGNOF(type)                                                  \
	(offsetof(struct { char c; type member; }, member))

size_t drotrim_cqm_sizeof(void) { return sizeof(cqm_t); }
size_t drotrim_cqm_alignof(void) { return DROTRIM_ALIGNOF(cqm_t); }

size_t drotrim_ym3438_sizeof(void) { return sizeof(ym3438_t); }
size_t drotrim_ym3438_alignof(void) { return DROTRIM_ALIGNOF(ym3438_t); }
