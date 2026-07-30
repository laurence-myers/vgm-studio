/* The four gz* calls vgmtools makes, served from `FILE*`. See `zlib.h` beside
 * this file for why there is no real zlib here.
 *
 * Reading is all the tools do through this API, so writing is not implemented
 * at all rather than implemented wrongly.
 */

#include <stdio.h>

#include "zlib.h"

/* zlib's `gzopen` returns NULL for a file it cannot read, and the tools all
 * check for it -- so refusing a gzip stream here surfaces as each tool's own
 * "Error opening the file!" rather than as a signature mismatch further in.
 *
 * The check is a bug detector, not a feature gate: this crate always writes its
 * own uncompressed temporaries, so a gzip magic here means a caller passed a
 * path it should have unpacked.
 */
gzFile gzopen(const char* path, const char* mode)
{
	FILE* handle;
	unsigned char magic[2];
	size_t got;

	handle = fopen(path, mode);
	if (handle == NULL)
		return NULL;

	got = fread(magic, 1, sizeof(magic), handle);
	if (got == sizeof(magic) && magic[0] == 0x1F && magic[1] == 0x8B)
	{
		fclose(handle);
		return NULL;
	}

	rewind(handle);
	return (gzFile)handle;
}

/* zlib returns the byte count, or -1 on error. A short read is not an error to
 * zlib and is not one here: the tools ask for the length the header claims and
 * tolerate getting less. */
int gzread(gzFile file, void* buf, unsigned int len)
{
	size_t got;

	if (file == NULL)
		return -1;

	got = fread(buf, 1, len, (FILE*)file);
	return (int)got;
}

long gzseek(gzFile file, long offset, int whence)
{
	if (file == NULL)
		return -1;

	if (fseek((FILE*)file, offset, whence) != 0)
		return -1;

	return ftell((FILE*)file);
}

/* Tolerates NULL because zlib does: some of the tools' error paths close a
 * handle they never proved open. */
int gzclose(gzFile file)
{
	if (file == NULL)
		return 0;

	return fclose((FILE*)file);
}
