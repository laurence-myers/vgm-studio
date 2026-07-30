/* A freestanding stand-in for libc's <stdlib.h>, for wasm32-unknown-unknown.
 *
 * Same policy as vgms-cores-nuked's shim/string.h: the submodule is compiled
 * unmodified, so what libc it expects is declared on our side. The *symbols*
 * come from `src/wasm_libc.rs`, which forwards the allocator family to Rust's
 * own allocator -- see the note there about the size header.
 */

#pragma once

#include <stddef.h>

void *malloc(size_t size);
void *calloc(size_t count, size_t size);
void *realloc(void *ptr, size_t size);
void free(void *ptr);

int abs(int value);
long labs(long value);

#define RAND_MAX 0x7FFFFFFF
int rand(void);
void srand(unsigned int seed);
