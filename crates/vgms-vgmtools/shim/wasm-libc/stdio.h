/* A freestanding stand-in for libc's <stdio.h>, for wasm32-unknown-unknown,
 * serving the vgmtools optimisers (vgm_cmp, vgm_sro, optdac).
 *
 * `FILE*` is backed by `shim/memfile.c`: two in-memory slots, one read (the
 * input the host preloads) and one write (the output the host reads back).
 * `printf`/`sprintf`/`snprintf` are served by `shim/wasm_printf.c`, which
 * captures the text into a ring so a failing tool's last words can be quoted.
 *
 * This mirrors `crates/vgms-cores-libvgm/shim/wasm-libc/stdio.h`, but that one
 * declares only the printf family libvgm's `logging.c` needs; the tools also do
 * real file I/O, so this header grows `FILE` and the `f*` family. See
 * `docs/vgm-multichip-2026-07/OPTIMIZER-WASM-PLAN.md`.
 */

#pragma once

#include <stddef.h>
#include <stdarg.h>

#ifndef EOF
#define EOF (-1)
#endif

#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

/* Opaque to the tools: they only ever hold a `FILE*` and pass it back. The full
 * struct lives in `shim/memfile.c`. */
typedef struct vgmt_file FILE;

/* `common.h`'s ReadFilename reads a name from stdin when none is on argv. We
 * always pass argv, so it never runs -- but it is compiled, so `stdin` must
 * link. `shim/memfile.c` points it at an always-empty read slot. */
extern FILE* stdin;

FILE* fopen(const char* path, const char* mode);
size_t fread(void* ptr, size_t size, size_t nmemb, FILE* stream);
size_t fwrite(const void* ptr, size_t size, size_t nmemb, FILE* stream);
int fseek(FILE* stream, long offset, int whence);
long ftell(FILE* stream);
void rewind(FILE* stream);
int fclose(FILE* stream);
int feof(FILE* stream);
int fgetc(FILE* stream);
char* fgets(char* s, int size, FILE* stream);

int printf(const char* format, ...);
int sprintf(char* str, const char* format, ...);
int snprintf(char* str, size_t size, const char* format, ...);
int vsnprintf(char* str, size_t size, const char* format, va_list ap);

int getchar(void);
