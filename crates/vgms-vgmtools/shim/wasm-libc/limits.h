/* A freestanding stand-in for libc's <limits.h>, for wasm32-unknown-unknown.
 *
 * `common.h` on non-Windows does `#include <limits.h>` then
 * `#define MAX_PATH PATH_MAX`. clang ships a freestanding <limits.h> (INT_MAX
 * and friends) but not the POSIX `PATH_MAX`, so pull clang's in and add it.
 */

#pragma once

#include_next <limits.h>

#ifndef PATH_MAX
#define PATH_MAX 4096
#endif
