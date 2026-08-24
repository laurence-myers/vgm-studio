/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Force-included first in every core translation unit (see build.rs).
 *
 * Pulls the platform <stdlib.h> under its real names, then redirects the
 * cores' rand()/srand() to our deterministic, thread-local implementation in
 * src/rng.rs. Doing it here -- rather than with a command-line -Drand= -- means
 * the real rand() declaration is processed under its own name (we never call
 * it), so the redirect target is declared by the plain prototype below with no
 * dllimport. A -Drand= would instead rewrite the platform's
 * `__declspec(dllimport) int rand(void)` into a dllimport declaration of OUR
 * symbol, and the linker would treat a locally defined symbol as imported
 * (MSVC LNK4217/LNK4286). See src/rng.rs for why the redirect exists at all.
 */
#include <stdlib.h>
#undef rand
#undef srand
#define rand vgms_libvgm_rand
#define srand vgms_libvgm_srand
#ifdef __cplusplus
extern "C" {
#endif
int vgms_libvgm_rand(void);
void vgms_libvgm_srand(unsigned int);
#ifdef __cplusplus
}
#endif
