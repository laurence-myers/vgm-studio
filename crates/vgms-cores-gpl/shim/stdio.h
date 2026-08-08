/* A freestanding stand-in for libc's <stdio.h>.
 *
 * YMF276-LLE's fmopn2.c includes <stdio.h> but uses nothing from it -- the
 * include is vestigial (a debug-printf era leftover). `wasm32-unknown-unknown`
 * has no libc and clang ships only freestanding headers, so that one line is
 * all that stands between the source and a wasm build.
 *
 * The sourcing policy says glue is written on our side and the submodule is
 * compiled unmodified -- so rather than patch the include out, this header is
 * put earlier on the include path and declares exactly what the cores use of
 * stdio: nothing.
 */

#pragma once
