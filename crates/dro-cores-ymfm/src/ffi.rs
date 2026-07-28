// SPDX-License-Identifier: MIT OR Apache-2.0
//! The FFI boundary, and the [`ChipCore`] implementation over it.
//!
//! Every `unsafe` in this crate lives here, as in the other providers. The
//! shim (`shim/ymfm_c.cpp`) owns the C++ side; this owns the handle's
//! lifetime and the translation between our engine's conventions and
//! ymfm's.

use std::ffi::c_void;

use dro_synth::chip::ChipCore;

/// Which chip to build. Must match `kind_t` in `shim/ymfm_c.cpp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Kind {
    Ym2203 = 0,
    Ym2608 = 1,
    Ym2610 = 2,
    Ym2610b = 3,
    Ym2612 = 4,
    Ym3438 = 5,
}

// The shim's surface. The handle is opaque -- its layout is C++'s business.
unsafe extern "C" {
    fn ymfm_create(kind: i32, clock: u32) -> *mut c_void;
    fn ymfm_destroy(chip: *mut c_void);
    fn ymfm_reset(chip: *mut c_void);
    fn ymfm_sample_rate(chip: *const c_void, clock: u32) -> u32;
    fn ymfm_write(chip: *mut c_void, offset: u32, data: u8);
    fn ymfm_generate(chip: *mut c_void, out: *mut i32, frames: u32);
    fn ymfm_load_data(chip: *mut c_void, access: i32, offset: u32, data: *const u8, len: u32);
}

/// ymfm's `access_class` values, for [`ymfm_load_data`].
mod access {
    pub(super) const ADPCM_A: i32 = 1;
    pub(super) const ADPCM_B: i32 = 2;
}

/// One ymfm chip, owned.
pub struct YmfmChip {
    handle: *mut c_void,
    kind: Kind,
    clock: u32,
    rate: u32,
}

impl std::fmt::Debug for YmfmChip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YmfmChip")
            .field("kind", &self.kind)
            .field("clock", &self.clock)
            .field("rate", &self.rate)
            .finish()
    }
}

// The handle is exclusively owned and never shared; the C++ behind it holds
// no globals (ymfm chips are self-contained objects), so moving one between
// threads is sound. Not `Sync`: two threads must not write it at once.
unsafe impl Send for YmfmChip {}

impl YmfmChip {
    /// Builds a chip of `kind`, or `None` if the shim does not know it.
    ///
    /// The clock is provisional: [`reset`](ChipCore::reset) is what the
    /// engine calls with the header's real figure, and it rebuilds.
    #[must_use]
    pub fn new(kind: Kind) -> Option<Self> {
        Self::with_clock(kind, default_clock(kind))
    }

    fn with_clock(kind: Kind, clock: u32) -> Option<Self> {
        // SAFETY: the shim returns null for an unknown kind and a valid
        // owned pointer otherwise; nothing else can come back.
        let handle = unsafe { ymfm_create(kind as i32, clock) };
        if handle.is_null() {
            return None;
        }
        // SAFETY: handle is non-null and freshly built.
        let rate = unsafe { ymfm_sample_rate(handle, clock) }.max(1);
        Some(Self {
            handle,
            kind,
            clock,
            rate,
        })
    }
}

impl Drop for YmfmChip {
    fn drop(&mut self) {
        // SAFETY: built by `ymfm_create`, dropped exactly once, and never
        // handed out.
        unsafe { ymfm_destroy(self.handle) };
    }
}

impl ChipCore for YmfmChip {
    /// Rebuilds at `clock`, because a ymfm chip derives its sample rate from
    /// the clock it was constructed with rather than taking one later.
    fn reset(&mut self, clock: u32, _variant: bool) {
        if clock != 0
            && clock != self.clock
            && let Some(fresh) = Self::with_clock(self.kind, clock)
        {
            *self = fresh;
            return;
        }
        // SAFETY: an owned, non-null handle.
        unsafe { ymfm_reset(self.handle) };
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// Ports map onto ymfm's flat register offset: the OPN parts' second
    /// port is the high half of the address space.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        let offset = (u32::from(port) << 1) | (u32::from(addr) & 1);
        // ymfm's `write` takes an address/data pair on alternating offsets:
        // the even offset latches the register, the odd one the value.
        let register = (addr & 0xFF) as u8;
        // SAFETY: an owned, non-null handle; the shim bounds-checks nothing
        // because ymfm's own `write` masks the offset itself.
        unsafe {
            ymfm_write(self.handle, offset & !1, register);
            ymfm_write(self.handle, offset | 1, (data & 0xFF) as u8);
        }
    }

    /// ADPCM sample ROMs. Block type `0x81` is the 2608's ADPCM-A (its
    /// rhythm section's external samples), `0x82`/`0x83` the 2610's two
    /// spaces.
    fn load_rom(&mut self, block_type: u8, _total_size: u32, start: u32, data: &[u8]) {
        let space = match block_type {
            0x81 | 0x82 => access::ADPCM_A,
            0x83 => access::ADPCM_B,
            _ => return,
        };
        // SAFETY: an owned handle; `data` is a valid slice for its length,
        // and the shim copies out of it before returning.
        unsafe {
            ymfm_load_data(self.handle, space, start, data.as_ptr(), data.len() as u32);
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        // SAFETY: an owned handle, and `out` holds `frames * 2` i32s, which
        // is exactly what the shim writes.
        unsafe { ymfm_generate(self.handle, out.as_mut_ptr(), frames as u32) };
    }
}

/// A plausible clock per chip, used only until the header supplies the real
/// one at reset.
const fn default_clock(kind: Kind) -> u32 {
    match kind {
        Kind::Ym2203 => 3_000_000,
        Kind::Ym2608 | Kind::Ym2612 | Kind::Ym3438 => 7_987_200,
        Kind::Ym2610 | Kind::Ym2610b => 8_000_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn energy(out: &[i32]) -> i64 {
        out.iter().map(|&s| i64::from(s.abs())).sum()
    }

    /// **The ru-1 gate.** ymfm compiles into this workspace through
    /// `clang++`, links, builds a chip, takes writes and makes a sound. If
    /// this fails the re-scope needs re-planning, exactly as cr-3 gated the
    /// C submodules.
    #[test]
    fn ymfm_links_and_the_opn_family_sounds() {
        for kind in [
            Kind::Ym2203,
            Kind::Ym2608,
            Kind::Ym2610,
            Kind::Ym2610b,
            Kind::Ym2612,
            Kind::Ym3438,
        ] {
            let mut chip = YmfmChip::new(kind).expect("the shim knows this kind");
            chip.reset(default_clock(kind), false);
            assert!(chip.native_rate() > 8_000, "{kind:?} rate looks wrong");

            // Rest is not necessarily a mathematical zero -- the YM2612's
            // output carries a small DC offset with every channel idle --
            // so the gate is that a key-on dominates rest, not that rest is
            // exactly silent.
            let mut quiet = vec![0i32; 4096];
            chip.render(&mut quiet);
            let at_rest = energy(&quiet);

            // A minimal FM voice on **channel 1**, not channel 0: the YM2610
            // is a cut-down OPNA whose default `channel_mask` is 0x36, so
            // its FM channel 0 does not exist and keying it is silence.
            // Channel 1 is present on every member of the family.
            for (register, value) in [
                (0x31u16, 0x00u16), // DT/MUL, the four slots
                (0x35, 0x00),
                (0x39, 0x00),
                (0x3D, 0x00),
                (0x41, 0x00), // TL: loudest
                (0x45, 0x00),
                (0x49, 0x00),
                (0x4D, 0x00),
                (0x51, 0x1F), // AR: fastest attack
                (0x55, 0x1F),
                (0x59, 0x1F),
                (0x5D, 0x1F),
                (0x61, 0x00), // DR: no decay
                (0x65, 0x00),
                (0x69, 0x00),
                (0x6D, 0x00),
                (0x81, 0x00), // SL/RR
                (0x85, 0x00),
                (0x89, 0x00),
                (0x8D, 0x00),
                (0xB1, 0x07), // algorithm 7: every operator is a carrier
                (0xB5, 0xC0), // both sides on
                (0xA5, 0x22), // block/f-number high
                (0xA1, 0x69), // f-number low
                (0x28, 0xF1), // key on channel 1, all four slots
            ] {
                chip.write(0, register, value);
            }

            let mut loud = vec![0i32; 4096];
            chip.render(&mut loud);
            assert!(
                energy(&loud) > at_rest * 4 + 1000,
                "{kind:?} must sound after a key-on (rest {at_rest}, keyed \
                 {}) -- a core that links but stays quiet is the classic \
                 mis-paced-write symptom",
                energy(&loud)
            );
        }
    }

    /// An unknown kind is a `None`, never a wrong chip or a crash.
    #[test]
    fn the_shim_refuses_what_it_does_not_know() {
        // SAFETY: deliberately calling with an out-of-range kind, which the
        // shim's `default:` arm answers with null.
        let handle = unsafe { ymfm_create(999, 8_000_000) };
        assert!(handle.is_null());
    }
}
