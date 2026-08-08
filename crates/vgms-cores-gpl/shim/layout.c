/* How big the upstream chip struct is, and how it wants to be aligned.
 *
 * The Rust side allocates the state and never declares a twin of it -- see
 * `vgms-synth`'s opaque-allocation reasoning, mirrored here. Asking the compiler
 * that actually compiled the upstream is the only way to get these right by
 * construction: a size copied out of a header drifts precisely when the header
 * does, which is the case worth defending against on a submodule that exists to
 * be pulled.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "opll.h"

#define VGMSTUDIO_ALIGNOF(type)                                                  \
	(offsetof(struct { char c; type member; }, member))

size_t vgms_opll_sizeof(void) { return sizeof(opll_t); }
size_t vgms_opll_alignof(void) { return VGMSTUDIO_ALIGNOF(opll_t); }

#include "ympsg.h"

size_t vgms_ympsg_sizeof(void) { return sizeof(ympsg_t); }
size_t vgms_ympsg_alignof(void) { return VGMSTUDIO_ALIGNOF(ympsg_t); }

#include "opl2.h"

size_t vgms_opl2lite_sizeof(void) { return sizeof(opl2_chip); }
size_t vgms_opl2lite_alignof(void) { return VGMSTUDIO_ALIGNOF(opl2_chip); }
