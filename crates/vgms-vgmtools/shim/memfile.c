/* A FILE* layer over two in-memory slots, for the vgmtools optimisers on
 * wasm32-unknown-unknown. See OPTIMIZER-WASM-PLAN.md, Re-evaluation (2026-07-31)
 * correction C.
 *
 * The browser has no filesystem, but the tools need exactly two files per run:
 * the input they read and the output they write. `WriteVGMFile` opens its
 * output with `fopen(name, "wb")`; `OpenVGMFile` reads its input with
 * `gzopen(name, "rb")`, which `zshim.c` serves through `fopen(name, "rb")`. So
 * routing by *open mode* -- read to the input slot, write to the output slot --
 * satisfies every call without ever looking at a name. `zshim.c` sits on top of
 * this unchanged.
 *
 * A fresh wasm instance runs one file and is dropped, so the slots are plain
 * statics that never need resetting between runs -- that is the whole point of
 * an instance-per-run: zero-initialised data on every instantiate, and O(1)
 * reclamation when the host drops the instance.
 *
 * Host ABI (called from JS / the wasm parity harness):
 *   vgmt_input_reserve(len) -> ptr   reserve `len` input bytes; host fills them
 *   run()                            (each tool's own export) runs the tool
 *   vgmt_output_len() / _ptr()       the bytes the tool wrote (0 == "unchanged")
 * The log ring (vgmt_log_*) lives in wasm_printf.c.
 */

#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct vgmt_file {
	unsigned char* data; /* base of the slot's bytes */
	size_t len;          /* logical length written/available */
	size_t cap;          /* allocation capacity (output grows; input cap==len) */
	size_t pos;          /* read/write cursor */
	int writable;        /* 1 = output slot, 0 = read slot */
};

/* Exactly one input and one output per instance. `g_stdin` is an always-empty
 * read slot so `common.h`'s unreached ReadFilename links and returns "no name". */
static struct vgmt_file g_input;
static struct vgmt_file g_output;
static struct vgmt_file g_stdin;

FILE* stdin = &g_stdin;

/* --- host ABI ---------------------------------------------------------- */

/* Reserve `len` bytes for the input and return where to write them. Called once
 * before run(). */
unsigned char* vgmt_input_reserve(unsigned int len)
{
	free(g_input.data);
	g_input.data = (unsigned char*)malloc(len ? len : 1);
	g_input.len = g_input.data ? len : 0;
	g_input.cap = g_input.len;
	g_input.pos = 0;
	g_input.writable = 0;
	return g_input.data;
}

unsigned int vgmt_output_len(void)
{
	return (unsigned int)g_output.len;
}

unsigned char* vgmt_output_ptr(void)
{
	return g_output.data;
}

/* --- FILE* implementation ---------------------------------------------- */

/* zlib/stdio mode strings carry flags ("wb9", "rb"); only the direction matters
 * here, as in `zshim.c`'s own `wants_write`. */
static int wants_write(const char* mode)
{
	return mode != NULL &&
		(strchr(mode, 'w') != NULL || strchr(mode, 'a') != NULL || strchr(mode, '+') != NULL);
}

FILE* fopen(const char* path, const char* mode)
{
	(void)path; /* routing is by mode; see the file note */
	if (wants_write(mode))
	{
		/* A fresh output: the tools write at most one, only when it shrank. */
		free(g_output.data);
		g_output.data = NULL;
		g_output.len = 0;
		g_output.cap = 0;
		g_output.pos = 0;
		g_output.writable = 1;
		return &g_output;
	}
	g_input.pos = 0;
	g_input.writable = 0;
	return &g_input;
}

size_t fread(void* ptr, size_t size, size_t nmemb, FILE* stream)
{
	size_t want, avail, take;

	if (stream == NULL || size == 0)
		return 0;
	want = size * nmemb;
	avail = (stream->pos <= stream->len) ? (stream->len - stream->pos) : 0;
	take = (want < avail) ? want : avail;
	if (take != 0)
		memcpy(ptr, stream->data + stream->pos, take);
	stream->pos += take;
	return take / size; /* element count, as C demands */
}

size_t fwrite(const void* ptr, size_t size, size_t nmemb, FILE* stream)
{
	size_t total, need;

	if (stream == NULL || !stream->writable || size == 0)
		return 0;
	total = size * nmemb;
	need = stream->pos + total;
	if (need > stream->cap)
	{
		size_t newcap = stream->cap ? stream->cap : 4096;
		unsigned char* grown;
		while (newcap < need)
			newcap *= 2;
		grown = (unsigned char*)realloc(stream->data, newcap);
		if (grown == NULL)
			return 0;
		stream->data = grown;
		stream->cap = newcap;
	}
	memcpy(stream->data + stream->pos, ptr, total);
	stream->pos += total;
	if (stream->pos > stream->len)
		stream->len = stream->pos;
	return nmemb;
}

int fseek(FILE* stream, long offset, int whence)
{
	long base, target;

	if (stream == NULL)
		return -1;
	switch (whence)
	{
	case SEEK_SET: base = 0; break;
	case SEEK_CUR: base = (long)stream->pos; break;
	case SEEK_END: base = (long)stream->len; break;
	default: return -1;
	}
	target = base + offset;
	if (target < 0)
		return -1;
	stream->pos = (size_t)target;
	return 0;
}

long ftell(FILE* stream)
{
	if (stream == NULL)
		return -1;
	return (long)stream->pos;
}

void rewind(FILE* stream)
{
	if (stream != NULL)
		stream->pos = 0;
}

/* Does not free the slot: the host reads the output back through
 * vgmt_output_ptr *after* run() (and fclose) returns, so the bytes must
 * outlive the handle. The instance being dropped is what frees them. */
int fclose(FILE* stream)
{
	(void)stream;
	return 0;
}

int feof(FILE* stream)
{
	return stream == NULL || stream->pos >= stream->len;
}

int fgetc(FILE* stream)
{
	if (stream == NULL || stream->pos >= stream->len)
		return EOF;
	return stream->data[stream->pos++];
}

char* fgets(char* s, int size, FILE* stream)
{
	int i = 0;

	if (s == NULL || size <= 0 || stream == NULL || stream->pos >= stream->len)
		return NULL;
	while (i < size - 1 && stream->pos < stream->len)
	{
		char c = (char)stream->data[stream->pos++];
		s[i++] = c;
		if (c == '\n')
			break;
	}
	s[i] = '\0';
	return s;
}

int getchar(void)
{
	return EOF;
}
