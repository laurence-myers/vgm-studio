/* What the C compiler thinks libvgm's public structs measure.
 *
 * Unlike `dro-cores-nuked`, this crate *does* declare Rust twins of the
 * upstream structs -- it has to, because `DEV_INFO` and `DEV_DEF` are read
 * field by field rather than passed around as an opaque blob. That buys a
 * failure mode the opaque approach does not have: a field added or reordered
 * upstream still compiles, still links, and then reads the wrong pointer.
 *
 * So the sizes come from the compiler that actually compiled libvgm, and
 * `src/layout.rs` asserts Rust's agree. A pin bump that moves a field fails a
 * test instead of corrupting a chip.
 *
 * Ours, not upstream's: the submodule is compiled unmodified.
 */

#include <stddef.h>

#include "EmuStructs.h"
#include "cores/ayintf.h"
#include "cores/okim6258.h"
#include "cores/segapcm.h"
#include "cores/sn764intf.h"

/* `offsetof` past a char gives the alignment the compiler actually uses;
 * <stdalign.h> is not guaranteed in a freestanding build. */
#define DROTRIM_ALIGNOF(type)                                                  \
	(offsetof(struct { char c; type member; }, member))

size_t drotrim_libvgm_gencfg_sizeof(void) { return sizeof(DEV_GEN_CFG); }
size_t drotrim_libvgm_gencfg_alignof(void) { return DROTRIM_ALIGNOF(DEV_GEN_CFG); }

size_t drotrim_libvgm_devinfo_sizeof(void) { return sizeof(DEV_INFO); }
size_t drotrim_libvgm_devinfo_alignof(void) { return DROTRIM_ALIGNOF(DEV_INFO); }

size_t drotrim_libvgm_devdef_sizeof(void) { return sizeof(DEV_DEF); }
size_t drotrim_libvgm_devdef_alignof(void) { return DROTRIM_ALIGNOF(DEV_DEF); }

size_t drotrim_libvgm_rwfunc_sizeof(void) { return sizeof(DEVDEF_RWFUNC); }
size_t drotrim_libvgm_rwfunc_alignof(void) { return DROTRIM_ALIGNOF(DEVDEF_RWFUNC); }

size_t drotrim_libvgm_sn76496cfg_sizeof(void) { return sizeof(SN76496_CFG); }
size_t drotrim_libvgm_sn76496cfg_alignof(void) { return DROTRIM_ALIGNOF(SN76496_CFG); }

size_t drotrim_libvgm_ay8910cfg_sizeof(void) { return sizeof(AY8910_CFG); }
size_t drotrim_libvgm_ay8910cfg_alignof(void) { return DROTRIM_ALIGNOF(AY8910_CFG); }
size_t drotrim_libvgm_ay8910cfg_off_chiptype(void) { return offsetof(AY8910_CFG, chipType); }

size_t drotrim_libvgm_msm6258cfg_sizeof(void) { return sizeof(MSM6258_CFG); }
size_t drotrim_libvgm_msm6258cfg_alignof(void) { return DROTRIM_ALIGNOF(MSM6258_CFG); }
size_t drotrim_libvgm_msm6258cfg_off_divider(void) { return offsetof(MSM6258_CFG, divider); }

size_t drotrim_libvgm_segapcmcfg_sizeof(void) { return sizeof(SEGAPCM_CFG); }
size_t drotrim_libvgm_segapcmcfg_alignof(void) { return DROTRIM_ALIGNOF(SEGAPCM_CFG); }
size_t drotrim_libvgm_segapcmcfg_off_bnkshift(void) { return offsetof(SEGAPCM_CFG, bnkshift); }

size_t drotrim_libvgm_devlink_sizeof(void) { return sizeof(DEVLINK_INFO); }
size_t drotrim_libvgm_devlink_alignof(void) { return DROTRIM_ALIGNOF(DEVLINK_INFO); }
size_t drotrim_libvgm_devlink_off_linkid(void) { return offsetof(DEVLINK_INFO, linkID); }
size_t drotrim_libvgm_devlink_off_cfg(void) { return offsetof(DEVLINK_INFO, cfg); }

/* Where the fields actually sit, for the two structs we index into most.
 * A size match alone would not catch two fields swapped. */
size_t drotrim_libvgm_devinfo_off_dataptr(void) { return offsetof(DEV_INFO, dataPtr); }
size_t drotrim_libvgm_devinfo_off_samplerate(void) { return offsetof(DEV_INFO, sampleRate); }
size_t drotrim_libvgm_devinfo_off_devdef(void) { return offsetof(DEV_INFO, devDef); }
size_t drotrim_libvgm_devinfo_off_linkdevcount(void) { return offsetof(DEV_INFO, linkDevCount); }
size_t drotrim_libvgm_devinfo_off_linkdevs(void) { return offsetof(DEV_INFO, linkDevs); }

size_t drotrim_libvgm_devdef_off_start(void) { return offsetof(DEV_DEF, Start); }
size_t drotrim_libvgm_devdef_off_stop(void) { return offsetof(DEV_DEF, Stop); }
size_t drotrim_libvgm_devdef_off_reset(void) { return offsetof(DEV_DEF, Reset); }
size_t drotrim_libvgm_devdef_off_update(void) { return offsetof(DEV_DEF, Update); }
size_t drotrim_libvgm_devdef_off_rwfuncs(void) { return offsetof(DEV_DEF, rwFuncs); }

size_t drotrim_libvgm_gencfg_off_srmode(void) { return offsetof(DEV_GEN_CFG, srMode); }
size_t drotrim_libvgm_gencfg_off_flags(void) { return offsetof(DEV_GEN_CFG, flags); }
size_t drotrim_libvgm_gencfg_off_clock(void) { return offsetof(DEV_GEN_CFG, clock); }
size_t drotrim_libvgm_gencfg_off_smplrate(void) { return offsetof(DEV_GEN_CFG, smplRate); }
