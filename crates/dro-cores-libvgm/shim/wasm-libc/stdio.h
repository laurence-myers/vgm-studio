/* A freestanding stand-in for libc's <stdio.h>, for wasm32-unknown-unknown.
 *
 * Only what the compiled sources reach for: `logging.c` formats messages with
 * vsnprintf before handing them to a callback, and no callback is ever set on
 * wasm, so the stub in `shim/wasm_stubs.c` truncates to an empty string --
 * honest, since nothing would read the text anyway.
 */

#pragma once

#include <stddef.h>
#include <stdarg.h>

int snprintf(char *buffer, size_t size, const char *format, ...);
int vsnprintf(char *buffer, size_t size, const char *format, va_list args);
