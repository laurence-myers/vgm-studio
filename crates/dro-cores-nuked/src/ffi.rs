//! The C ABI of the pinned upstream cores, and the only `unsafe` in this crate.
//!
//! Declared by hand rather than bindgen: the surface is five functions and one
//! opaque-to-us struct, bindgen would add libclang to every build, and a
//! generated binding is a thing that can silently drift from the header it was
//! generated against. These declarations are checked against
//! `vendor/upstream/nuked-cqm/cqm.h` at the pinned commit, named in
//! `PROVENANCE.md`.
//!
//! **The struct is allocated on the Rust side**, which is why its size matters
//! and why [`CqmChip`] mirrors the upstream layout. The upstream has no
//! allocator of its own -- `CQM_Reset` `memset`s whatever it is handed -- so
//! this is the intended usage, not a shortcut.

/// Upstream's write-buffer depth, from `cqm.h`.
const WRITEBUF_SIZE: usize = 2048;

/// One buffered register write. Mirrors `cqm_writebuf`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct CqmWriteBuf {
    time: u64,
    reg: u16,
    data: u8,
}

/// One operator slot. Mirrors `cqmslot_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct CqmSlot {
    env: i32,
    phase: u32,
    mod_: [i32; 2],
}

/// The chip state. Mirrors `cqm_t` from the pinned `cqm.h`.
///
/// Every field is here so the *size* is right; nothing outside this module
/// reads one. If upstream adds a field, this must grow with it -- which is
/// what `the_chip_state_is_the_size_upstream_thinks_it_is` guards, by asking
/// the C side rather than trusting this declaration.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct CqmChip {
    regs: [u8; 256],
    slotz: [CqmSlot; 48],
    oddeven: u8,
    newm: u8,
    rhy: u8,
    mode: u8,
    key: [u8; 18],
    keyl: u32,
    okeyl: u32,
    counter: u16,
    trem_cnt1: u8,
    trem_cnt2: u8,
    trem_cnt3: u8,
    trem_cnt: u8,
    dooutput: i32,
    wave_prev: i32,
    wavesample: i32,
    waveshift: i32,
    wavepan: u8,
    is4op2: u8,
    noise: u32,
    hh_bit1: u8,
    hh_bit2: u8,
    rhy_bit: u8,

    rateratio: i32,
    samplecnt: i32,
    oldsamples: [i16; 2],
    samples: [i16; 2],

    writebuf_samplecnt: u64,
    writebuf_cur: u32,
    writebuf_last: u32,
    writebuf_lasttime: u64,
    writebuf: [CqmWriteBuf; WRITEBUF_SIZE],
}

impl Default for CqmChip {
    fn default() -> Self {
        // Never played in this state: `CQM_Reset` zeroes the whole struct and
        // sets the chip's real reset values. This exists so the struct can be
        // created at all before that call.
        Self {
            regs: [0; 256],
            slotz: [CqmSlot::default(); 48],
            oddeven: 0,
            newm: 0,
            rhy: 0,
            mode: 0,
            key: [0; 18],
            keyl: 0,
            okeyl: 0,
            counter: 0,
            trem_cnt1: 0,
            trem_cnt2: 0,
            trem_cnt3: 0,
            trem_cnt: 0,
            dooutput: 0,
            wave_prev: 0,
            wavesample: 0,
            waveshift: 0,
            wavepan: 0,
            is4op2: 0,
            noise: 0,
            hh_bit1: 0,
            hh_bit2: 0,
            rhy_bit: 0,
            rateratio: 0,
            samplecnt: 0,
            oldsamples: [0; 2],
            samples: [0; 2],
            writebuf_samplecnt: 0,
            writebuf_cur: 0,
            writebuf_last: 0,
            writebuf_lasttime: 0,
            writebuf: [CqmWriteBuf::default(); WRITEBUF_SIZE],
        }
    }
}

unsafe extern "C" {
    fn CQM_Reset(chip: *mut CqmChip, samplerate: u32, genrate: u32);
    fn CQM_WriteReg(chip: *mut CqmChip, reg: u16, data: u8);
    fn CQM_WriteRegBuffered(chip: *mut CqmChip, reg: u16, data: u8);
    fn CQM_GenerateStream(chip: *mut CqmChip, sndptr: *mut i16, numsamples: u32);
}

impl CqmChip {
    /// Re-initialises the chip: `output_rate` is what samples come out at,
    /// `native_rate` what the chip itself runs at, and upstream resamples
    /// between them.
    pub(crate) fn reset(&mut self, output_rate: u32, native_rate: u32) {
        // SAFETY: `self` is a live, correctly-sized `cqm_t` (see the layout
        // guard test). `CQM_Reset` only writes through the pointer, and
        // `native_rate` is non-zero at every call site -- it divides.
        unsafe { CQM_Reset(self, output_rate, native_rate.max(1)) }
    }

    pub(crate) fn write_reg(&mut self, reg: u16, data: u8) {
        // SAFETY: as above; `CQM_WriteReg` reads and writes only `*self`.
        unsafe { CQM_WriteReg(self, reg, data) }
    }

    pub(crate) fn write_reg_buffered(&mut self, reg: u16, data: u8) {
        // SAFETY: as above. The write lands in the chip's own ring buffer,
        // which is part of the struct.
        unsafe { CQM_WriteRegBuffered(self, reg, data) }
    }

    /// Fills `buffer` with interleaved stereo frames.
    pub(crate) fn generate(&mut self, buffer: &mut [i16]) {
        let frames = buffer.len() / 2;
        if frames == 0 {
            return;
        }
        let Ok(frames) = u32::try_from(frames) else {
            // Upstream counts frames in a u32. No caller comes close -- an
            // audio callback asks for hundreds -- but chunking beats wrapping.
            for chunk in buffer.chunks_mut(u32::MAX as usize & !1) {
                self.generate(chunk);
            }
            return;
        };
        // SAFETY: upstream writes exactly `frames * 2` i16s through `sndptr`
        // (`CQM_GenerateResampled` per frame, two channels each), and `frames`
        // is `buffer.len() / 2`, so the write stays inside `buffer`. The
        // pointer comes from a live mutable slice and is not retained.
        unsafe { CQM_GenerateStream(self, buffer.as_mut_ptr(), frames) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Upstream's own `sizeof(cqm_t)`, reported by the C side rather than
    // asserted from this file -- the point is to catch *this* declaration
    // drifting from the header, and a constant copied out of the header would
    // drift with it.
    unsafe extern "C" {
        #[link_name = "drotrim_cqm_sizeof"]
        fn cqm_sizeof() -> usize;
        #[link_name = "drotrim_cqm_alignof"]
        fn cqm_alignof() -> usize;
    }

    /// The struct is allocated on the Rust side and written by the C side, so a
    /// layout disagreement is memory corruption rather than a compile error.
    /// This is the one thing about the binding that cannot be checked by
    /// reading it.
    #[test]
    fn the_chip_state_is_the_size_upstream_thinks_it_is() {
        // SAFETY: both shims return a compile-time constant and touch nothing.
        let (size, align) = unsafe { (cqm_sizeof(), cqm_alignof()) };
        assert_eq!(
            size_of::<CqmChip>(),
            size,
            "the Rust mirror of cqm_t has drifted from the pinned header"
        );
        assert_eq!(align_of::<CqmChip>(), align, "alignment disagrees");
    }
}
