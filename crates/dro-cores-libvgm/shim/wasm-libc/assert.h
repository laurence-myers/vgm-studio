/* A freestanding stand-in for libc's <assert.h>, for wasm32-unknown-unknown:
 * release semantics, as every shipping build of these cores has.
 */

#pragma once

#define assert(condition) ((void)0)
