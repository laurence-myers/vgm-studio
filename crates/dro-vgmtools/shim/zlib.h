/* A stand-in for zlib.h, covering exactly the four gz* calls vgmtools makes.
 *
 * The tools read their input through gzopen/gzread/gzseek/gzclose and write
 * their output through plain fopen/fwrite. They use nothing else from zlib --
 * not the deflate API, not gzwrite, not gzprintf -- so linking real zlib to
 * serve four read-only calls would drag a C compression library into a build
 * that has no other use for one. This header plus `zshim.c` serve them from
 * `FILE*` instead.
 *
 * That is sound because of how the binding feeds them: every input is a
 * temporary file this crate wrote itself, from bytes that were already
 * decompressed by `dro_core`. Gzip never reaches this layer. A `.vgz` on disk
 * is unpacked before it becomes a `VgmFile` and repacked after, and both halves
 * of that are flate2's job in Rust.
 *
 * So a gzip stream arriving here is not a case to handle, it is a bug in the
 * caller -- and `zshim.c` says so by refusing to open it rather than letting
 * the magic bytes fail a signature check three frames later.
 *
 * This file must precede any real zlib on the include path.
 */

#ifndef DRO_VGMTOOLS_ZLIB_SHIM_H
#define DRO_VGMTOOLS_ZLIB_SHIM_H

#include <stdio.h>

/* zlib's own opaque handle is `struct gzFile_s*`; the tools only ever store it
 * and pass it back, so any pointer type will do. */
typedef void* gzFile;

gzFile gzopen(const char* path, const char* mode);
int gzread(gzFile file, void* buf, unsigned int len);
long gzseek(gzFile file, long offset, int whence);
int gzclose(gzFile file);

#endif /* DRO_VGMTOOLS_ZLIB_SHIM_H */
