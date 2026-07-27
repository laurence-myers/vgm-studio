//! The C ABI of the pinned upstream cores, and the only `unsafe` in this crate.
//!
//! Declared by hand rather than bindgen: the surface is a dozen functions and
//! two opaque structs, bindgen would add libclang to every build, and a
//! generated binding is a thing that can silently drift from the header it was
//! generated against. These declarations are checked against the headers at the
//! pinned commits named in `PROVENANCE.md`.
//!
//! **No struct is mirrored.** The state is allocated by size reported from C
//! (see [`crate::opaque`]), so an upstream that adds a field changes a number
//! rather than silently outgrowing a Rust twin of itself.

use std::ffi::c_void;

use crate::opaque::OpaqueChip;

unsafe extern "C" {
    // --- ours (shim/layout.c), so the sizes come from the compiler ---
    fn drotrim_cqm_sizeof() -> usize;
    fn drotrim_cqm_alignof() -> usize;
    fn drotrim_ym3438_sizeof() -> usize;
    fn drotrim_ym3438_alignof() -> usize;
    fn drotrim_opm_sizeof() -> usize;
    fn drotrim_opm_alignof() -> usize;

    // --- Nuked-CQM ---
    fn CQM_Reset(chip: *mut c_void, samplerate: u32, genrate: u32);
    fn CQM_WriteReg(chip: *mut c_void, reg: u16, data: u8);
    fn CQM_WriteRegBuffered(chip: *mut c_void, reg: u16, data: u8);
    fn CQM_GenerateStream(chip: *mut c_void, sndptr: *mut i16, numsamples: u32);

    // --- Nuked-OPN2 ---
    fn OPN2_Reset(chip: *mut c_void);
    fn OPN2_SetChipType(chip_type: u32);
    fn OPN2_Clock(chip: *mut c_void, buffer: *mut i16);
    fn OPN2_Write(chip: *mut c_void, port: u32, data: u8);

    // --- Nuked-OPM ---
    fn OPM_Reset(chip: *mut c_void, flags: u32);
    fn OPM_Clock(chip: *mut c_void, output: *mut i32, sh1: *mut u8, sh2: *mut u8, so: *mut u8);
    fn OPM_Write(chip: *mut c_void, port: u32, data: u8);
}

/// Upstream's `opm_flags_ym2164`: the rebadged OPP rather than the YM2151.
const OPM_FLAGS_YM2164: u32 = 1;

/// Upstream's `ym3438_mode_ym2612`: the discrete YM2612 rather than the CMOS
/// YM3438, which differs audibly in its DAC.
pub(crate) const OPN2_MODE_YM2612: u32 = 0x01;
/// Upstream's `ym3438_mode_readmode`, its own default.
pub(crate) const OPN2_MODE_READMODE: u32 = 0x02;

/// A Nuked-CQM chip.
#[derive(Debug)]
pub(crate) struct CqmChip {
    state: OpaqueChip,
}

impl CqmChip {
    pub(crate) fn new() -> Self {
        // SAFETY: both shims return a compile-time constant and touch nothing.
        let (size, align) = unsafe { (drotrim_cqm_sizeof(), drotrim_cqm_alignof()) };
        Self {
            state: OpaqueChip::new(size, align),
        }
    }

    /// Re-initialises: `output_rate` is what samples come out at, `native_rate`
    /// what the chip itself runs at, and upstream resamples between them.
    pub(crate) fn reset(&mut self, output_rate: u32, native_rate: u32) {
        // SAFETY: the block is sized by the C's own `sizeof(cqm_t)`, so
        // `CQM_Reset`'s memset of it stays inside the allocation.
        // `native_rate` divides, so it must not be zero.
        unsafe { CQM_Reset(self.state.as_ptr(), output_rate, native_rate.max(1)) }
    }

    pub(crate) fn write_reg(&mut self, reg: u16, data: u8) {
        // SAFETY: as above; the call reads and writes only the chip block.
        unsafe { CQM_WriteReg(self.state.as_ptr(), reg, data) }
    }

    pub(crate) fn write_reg_buffered(&mut self, reg: u16, data: u8) {
        // SAFETY: as above. The write lands in the chip's own ring buffer,
        // which is part of the block.
        unsafe { CQM_WriteRegBuffered(self.state.as_ptr(), reg, data) }
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
            for chunk in buffer.chunks_mut((u32::MAX as usize) & !1) {
                self.generate(chunk);
            }
            return;
        };
        // SAFETY: upstream writes exactly `frames * 2` i16s through `sndptr`,
        // and `frames` is `buffer.len() / 2`, so the writes stay inside
        // `buffer`. The pointer comes from a live mutable slice and is not
        // retained past the call.
        unsafe { CQM_GenerateStream(self.state.as_ptr(), buffer.as_mut_ptr(), frames) }
    }
}

/// A Nuked-OPN2 chip: the YM3438, or the YM2612 it is the CMOS version of.
#[derive(Debug)]
pub(crate) struct Opn2Chip {
    state: OpaqueChip,
    /// Which variant this instance is, applied to upstream's **global**
    /// chip-type for the duration of each [`clocking`](Self::clocking) session.
    mode: u32,
}

/// Serialises the window in which upstream's global chip-type is in force.
///
/// `OPN2_SetChipType` writes a `static` in `ym3438.c`, not a field of the chip,
/// so two instances of different variants share one setting. Only `OPN2_Clock`
/// reads it -- for the YM2612's discrete DAC ladder, which the CMOS YM3438
/// lacks -- and `OPN2_Read`, which nothing here calls. So the setting has to
/// hold across a *clocking run*, and this is what makes that true even with two
/// engines rendering on different threads.
///
/// One acquisition per `render` call, not per clock: a render is hundreds of
/// samples, the lock is uncontended unless two OPN2 chips are being driven at
/// once, and every holder does nothing but arithmetic. Locking per internal
/// clock would be 1.3 million acquisitions a second per chip.
static CHIP_TYPE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A clocking run with this chip's variant in force.
///
/// Writes go through here too: they are consumed *by* the clocks, so they
/// belong inside the same window.
pub(crate) struct Opn2Clocking<'a> {
    chip: &'a mut Opn2Chip,
    /// Released on drop, which is what ends the window.
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Opn2Chip {
    pub(crate) fn new() -> Self {
        // SAFETY: both shims return a compile-time constant and touch nothing.
        let (size, align) = unsafe { (drotrim_ym3438_sizeof(), drotrim_ym3438_alignof()) };
        Self {
            state: OpaqueChip::new(size, align),
            mode: OPN2_MODE_READMODE,
        }
    }

    /// Re-initialises as `ym2612` (the discrete chip) or the CMOS YM3438.
    ///
    /// Needs no lock: `OPN2_Reset` does not read the global chip-type, only
    /// `OPN2_Clock` and `OPN2_Read` do.
    pub(crate) fn reset(&mut self, ym2612: bool) {
        self.mode = if ym2612 {
            OPN2_MODE_YM2612 | OPN2_MODE_READMODE
        } else {
            OPN2_MODE_READMODE
        };
        // SAFETY: the block is sized by the C's own `sizeof(ym3438_t)`, so
        // `OPN2_Reset`'s initialisation of it stays inside the allocation.
        unsafe { OPN2_Reset(self.state.as_ptr()) }
    }

    /// Opens a clocking run with this chip's variant in force.
    pub(crate) fn clocking(&mut self) -> Opn2Clocking<'_> {
        // The guarded data is `()`, so a panic while holding leaves nothing
        // invalid behind -- recovering beats propagating someone else's panic
        // into an audio callback.
        let guard = CHIP_TYPE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // SAFETY: writes one `u32` global in the upstream, under the lock that
        // makes it this chip's until the guard drops. No chip state is touched.
        unsafe { OPN2_SetChipType(self.mode) }
        Opn2Clocking {
            chip: self,
            _guard: guard,
        }
    }
}

impl Opn2Clocking<'_> {
    /// Presents `data` on `port` (0/2 select an address, 1/3 the data).
    ///
    /// The write is *latched*, not applied: upstream sets a pending flag that
    /// the next [`clock`](Self::clock) consumes. Two writes with no clock
    /// between them therefore lose the first, which is why `opn2.rs` queues
    /// them and drains one per clock.
    pub(crate) fn write(&mut self, port: u32, data: u8) {
        // SAFETY: the block is sized by the C's own `sizeof(ym3438_t)`; the
        // call writes only inside it.
        unsafe { OPN2_Write(self.chip.state.as_ptr(), port, data) }
    }

    /// Advances one internal clock (six master clocks) and returns the signed
    /// 9-bit MOL/MOR pin states.
    pub(crate) fn clock(&mut self) -> (i32, i32) {
        let mut pins = [0i16; 2];
        // SAFETY: upstream writes exactly two i16s through `buffer`, and `pins`
        // is two i16s. The pointer is not retained past the call.
        unsafe { OPN2_Clock(self.chip.state.as_ptr(), pins.as_mut_ptr()) }
        (i32::from(pins[0]), i32::from(pins[1]))
    }
}

/// A Nuked-OPM chip: the YM2151, or the YM2164 it was rebadged as.
///
/// No lock, unlike [`Opn2Chip`]: this upstream keeps its variant in the chip's
/// own struct (`opm_t::opp`, set from the reset flags) rather than in a global.
#[derive(Debug)]
pub(crate) struct OpmChip {
    state: OpaqueChip,
}

impl OpmChip {
    pub(crate) fn new() -> Self {
        // SAFETY: both shims return a compile-time constant and touch nothing.
        let (size, align) = unsafe { (drotrim_opm_sizeof(), drotrim_opm_alignof()) };
        Self {
            state: OpaqueChip::new(size, align),
        }
    }

    /// Re-initialises as a YM2151, or a YM2164 when `ym2164`.
    ///
    /// Upstream drives the IC (reset) pin and clocks the chip through its
    /// power-on sequence itself, so there is nothing to arrange around this.
    pub(crate) fn reset(&mut self, ym2164: bool) {
        let flags = if ym2164 { OPM_FLAGS_YM2164 } else { 0 };
        // SAFETY: the block is sized by the C's own `sizeof(opm_t)`, so the
        // memset and the reset clocking stay inside the allocation.
        unsafe { OPM_Reset(self.state.as_ptr(), flags) }
    }

    /// Presents `data` on `port` (0 selects a register, 1 its value).
    ///
    /// Latched, not applied: the register lands when the rotation reaches its
    /// slot, which is why `opm.rs` queues writes.
    pub(crate) fn write(&mut self, port: u32, data: u8) {
        // SAFETY: as above; the call writes only inside the chip block.
        unsafe { OPM_Write(self.state.as_ptr(), port, data) }
    }

    /// Advances one internal cycle and returns the stereo DAC outputs.
    pub(crate) fn clock(&mut self) -> (i32, i32) {
        let mut output = [0i32; 2];
        // SAFETY: upstream writes exactly two i32s through `output`, which is
        // two i32s. The serial-DAC pins are of no interest here and upstream
        // accepts null for them (`if (sh1) *sh1 = ...`). No pointer is retained.
        unsafe {
            OPM_Clock(
                self.state.as_ptr(),
                output.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
        (output[0], output[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes must be plausible, and the alignments must be something
    /// `OpaqueChip` can actually promise. A zero would mean the shim did not
    /// link and every core would be writing into a one-word allocation.
    #[test]
    fn the_shim_reports_real_sizes() {
        // SAFETY: all four return compile-time constants.
        let (cqm, cqm_align, opn2, opn2_align) = unsafe {
            (
                drotrim_cqm_sizeof(),
                drotrim_cqm_alignof(),
                drotrim_ym3438_sizeof(),
                drotrim_ym3438_alignof(),
            )
        };
        assert!(cqm > 1024, "cqm_t came back as {cqm} bytes");
        assert!(opn2 > 1024, "ym3438_t came back as {opn2} bytes");
        // SAFETY: compile-time constants, as above.
        let (opm, opm_align) = unsafe { (drotrim_opm_sizeof(), drotrim_opm_alignof()) };
        assert!(opm > 1024, "opm_t came back as {opm} bytes");
        assert!(opm_align <= align_of::<u64>(), "{opm_align}");
        assert!(cqm_align <= align_of::<u64>(), "{cqm_align}");
        assert!(opn2_align <= align_of::<u64>(), "{opn2_align}");
    }
}
