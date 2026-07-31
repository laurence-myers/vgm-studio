/* A freestanding stand-in for libc's <string.h>, for wasm32-unknown-unknown.
 *
 * The `mem*` family needs declarations only -- Rust's `compiler_builtins`
 * carries the symbols on every target, wasm included. The `str*` family is
 * implemented in `src/wasm_libc.rs`; `strcasecmp`/`strncasecmp` are there for
 * `common.h`'s `stricmp`/`strnicmp` macros, `strchr`/`strrchr` for `zshim.c`
 * and the tools' extension splitting.
 */

#pragma once

#include <stddef.h>

void* memset(void* dest, int c, size_t n);
void* memcpy(void* dest, const void* src, size_t n);
void* memmove(void* dest, const void* src, size_t n);
int memcmp(const void* a, const void* b, size_t n);

size_t strlen(const char* s);
int strcmp(const char* a, const char* b);
int strncmp(const char* a, const char* b, size_t n);
int strcasecmp(const char* a, const char* b);
int strncasecmp(const char* a, const char* b, size_t n);
char* strcpy(char* dest, const char* src);
char* strncpy(char* dest, const char* src, size_t n);
char* strcat(char* dest, const char* src);
char* strdup(const char* s);
char* strchr(const char* s, int c);
char* strrchr(const char* s, int c);
