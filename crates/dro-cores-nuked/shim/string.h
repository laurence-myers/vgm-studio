/* A freestanding stand-in for libc's <string.h>.
 *
 * The upstream cores are freestanding C in every respect but one: they include
 * <string.h> for a `memset` or two. `wasm32-unknown-unknown` has no libc and
 * clang ships only freestanding headers (stddef, stdint, stdarg), so that
 * include is the single thing standing between these sources and a wasm build.
 *
 * The sourcing policy says glue is written on our side and the submodule is
 * compiled unmodified -- so rather than patch the include out, this header is
 * put earlier on the include path and declares what the cores actually use.
 *
 * The *symbols* need no shim: Rust's `compiler_builtins` provides `memset`,
 * `memcpy`, `memmove` and `memcmp` on every target, wasm included. Only the
 * declarations were missing.
 */

#pragma once

#include <stddef.h>

void *memset(void *dest, int c, size_t n);
void *memcpy(void *dest, const void *src, size_t n);
void *memmove(void *dest, const void *src, size_t n);
int memcmp(const void *a, const void *b, size_t n);
