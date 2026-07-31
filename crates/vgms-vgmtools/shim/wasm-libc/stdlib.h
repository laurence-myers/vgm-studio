/* A freestanding stand-in for libc's <stdlib.h>, for wasm32-unknown-unknown.
 *
 * The tools reach only the allocator family; the *symbols* come from
 * `src/wasm_libc.rs`, which forwards to Rust's own allocator (with a size
 * header so C's free/realloc can rebuild the layout). No `exit`, `atoi`,
 * `strtoul`, `qsort` or `rand` -- the five sources use none of them.
 */

#pragma once

#include <stddef.h>

void* malloc(size_t size);
void* calloc(size_t count, size_t size);
void* realloc(void* ptr, size_t size);
void free(void* ptr);
