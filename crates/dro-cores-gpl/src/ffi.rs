//! The C ABI of the pinned upstream cores, and the only `unsafe` in this crate.
//!
//! Declared by hand rather than bindgen, with no struct mirrored: the state is
//! allocated by a size the C reports, so an upstream that adds a field changes
//! a number rather than silently outgrowing a Rust twin of itself.

use std::ffi::c_void;

unsafe extern "C" {
    // Ours (shim/layout.c), so the size comes from the compiler.
    fn drotrim_opll_sizeof() -> usize;
    fn drotrim_opll_alignof() -> usize;
    fn drotrim_ympsg_sizeof() -> usize;
    fn drotrim_ympsg_alignof() -> usize;

    fn OPLL_Reset(chip: *mut c_void, chip_type: u32);
    fn OPLL_Clock(chip: *mut c_void, buffer: *mut i32);
    fn OPLL_Write(chip: *mut c_void, port: u32, data: u8);

    fn YMPSG_Init(chip: *mut c_void);
    fn YMPSG_Write(chip: *mut c_void, data: u8);
    fn YMPSG_Clock(chip: *mut c_void);
    fn YMPSG_GetOutput(chip: *mut c_void) -> f32;

    fn drotrim_fmopm_sizeof() -> usize;
    fn drotrim_fmopm_alignof() -> usize;
    fn drotrim_fmopm_set_pins(
        chip: *mut c_void,
        ym2164: i32,
        ic: i32,
        cs: i32,
        wr: i32,
        a0: i32,
        data: i32,
    );
    fn drotrim_fmopm_out_sh1(chip: *const c_void) -> i32;
    fn drotrim_fmopm_out_sh2(chip: *const c_void) -> i32;
    fn drotrim_fmopm_out_so(chip: *const c_void) -> i32;
    fn FMOPM_Clock(chip: *mut c_void, clk: i32);

    fn drotrim_fmopna2612_sizeof() -> usize;
    fn drotrim_fmopna2612_alignof() -> usize;
    fn drotrim_fmopna2612_set_pins(
        chip: *mut c_void,
        ic: i32,
        cs: i32,
        wr: i32,
        a0: i32,
        a1: i32,
        data: i32,
    );
    fn drotrim_fmopna2612_out_mol(chip: *const c_void) -> i32;
    fn drotrim_fmopna2612_out_mor(chip: *const c_void) -> i32;
    fn FMOPNA_2612_Clock(chip: *mut c_void, clk: i32);

    fn drotrim_fmopna2608_sizeof() -> usize;
    fn drotrim_fmopna2608_alignof() -> usize;
    fn drotrim_fmopna2608_set_pins(
        chip: *mut c_void,
        ic: i32,
        cs: i32,
        wr: i32,
        a0: i32,
        a1: i32,
        data: i32,
    );
    fn drotrim_fmopna2608_serve_dm(chip: *mut c_void, dm: i32);
    fn drotrim_fmopna2608_dram_pins(chip: *const c_void, dm: *mut i32, a8: *mut i32) -> i32;
    fn drotrim_fmopna2608_dac_pins(chip: *const c_void, analog: *mut f32) -> i32;
    fn FMOPNA_Clock(chip: *mut c_void, clk: i32);
}

/// Upstream's `opll_type_ym2413`: the Yamaha part a VGM means by "YM2413".
const OPLL_TYPE_YM2413: u32 = 0x00;
/// Upstream's `opll_type_ds1001`: Konami's VRC VII, which a VGM signals with
/// bit 31 of the clock.
const OPLL_TYPE_DS1001: u32 = 0x01;

/// Zeroed bytes sized for the upstream chip struct.
///
/// `u64`-backed for eight-byte alignment; the constructor asserts rather than
/// assumes, because a silently under-aligned struct is undefined behaviour.
struct OpaqueChip {
    storage: Box<[u64]>,
}

impl OpaqueChip {
    fn new(size: usize, align: usize) -> Self {
        assert!(
            align <= align_of::<u64>(),
            "the upstream wants {align}-byte alignment; this can promise {}",
            align_of::<u64>()
        );
        Self {
            storage: vec![0u64; size.div_ceil(size_of::<u64>()).max(1)].into_boxed_slice(),
        }
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast()
    }
}

impl std::fmt::Debug for OpaqueChip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpaqueChip")
            .field("bytes", &(self.storage.len() * size_of::<u64>()))
            .finish()
    }
}

// SAFETY: plain zeroed memory owned solely by this value, and the upstream
// keeps no global mutable state reachable through it (`chip_type` is a field
// of the struct, not a `static`).
unsafe impl Send for OpaqueChip {}

/// A Nuked-OPLL chip.
#[derive(Debug)]
pub(crate) struct OpllChip {
    state: OpaqueChip,
}

impl OpllChip {
    pub(crate) fn new() -> Self {
        // SAFETY: both shims return a compile-time constant and touch nothing.
        let (size, align) = unsafe { (drotrim_opll_sizeof(), drotrim_opll_alignof()) };
        Self {
            state: OpaqueChip::new(size, align),
        }
    }

    /// Re-initialises as a YM2413, or as Konami's VRC VII variant.
    pub(crate) fn reset(&mut self, vrc7: bool) {
        let kind = if vrc7 {
            OPLL_TYPE_DS1001
        } else {
            OPLL_TYPE_YM2413
        };
        // SAFETY: the block is sized by the C's own `sizeof(opll_t)`, so the
        // memset inside `OPLL_Reset` stays within the allocation.
        unsafe { OPLL_Reset(self.state.as_ptr(), kind) }
    }

    /// Presents `data` on `port` (0 selects a register, 1 its value).
    pub(crate) fn write(&mut self, port: u32, data: u8) {
        // SAFETY: as above; the call writes only inside the chip block.
        unsafe { OPLL_Write(self.state.as_ptr(), port, data) }
    }

    /// Advances one internal cycle and returns the melody and rhythm outputs.
    ///
    /// The chip has two DACs, time-multiplexed across its rotation, so a sample
    /// is the whole rotation of both summed.
    pub(crate) fn clock(&mut self) -> (i32, i32) {
        let mut out = [0i32; 2];
        // SAFETY: upstream writes exactly two i32s through `buffer`, and `out`
        // is two i32s. The pointer is not retained past the call.
        unsafe { OPLL_Clock(self.state.as_ptr(), out.as_mut_ptr()) }
        (out[0], out[1])
    }
}

/// A Nuked-PSG chip: the SN76489 as Sega's VDPs integrate it.
#[derive(Debug)]
pub(crate) struct PsgChip {
    state: OpaqueChip,
}

impl PsgChip {
    pub(crate) fn new() -> Self {
        // SAFETY: both shims return a compile-time constant and touch nothing.
        let (size, align) = unsafe { (drotrim_ympsg_sizeof(), drotrim_ympsg_alignof()) };
        Self {
            state: OpaqueChip::new(size, align),
        }
    }

    /// Re-initialises: upstream's init memsets the struct and pulses IC for a
    /// full prescaler cycle, exactly what power-on does.
    pub(crate) fn reset(&mut self) {
        // SAFETY: the block is sized by the C's own `sizeof(ympsg_t)`, so the
        // memset inside `YMPSG_Init` stays within the allocation.
        unsafe { YMPSG_Init(self.state.as_ptr()) }
    }

    /// Presents one command byte on the chip's single write port. The chip
    /// consumes it on its next internal clock.
    pub(crate) fn write(&mut self, data: u8) {
        // SAFETY: as above; the call writes only inside the chip block.
        unsafe { YMPSG_Write(self.state.as_ptr(), data) }
    }

    /// Advances one internal clock.
    pub(crate) fn clock(&mut self) {
        // SAFETY: as above.
        unsafe { YMPSG_Clock(self.state.as_ptr()) }
    }

    /// The four DAC levels summed, as upstream's float arithmetic has them --
    /// unipolar, `0.0..=4.0` at the rails.
    pub(crate) fn output(&mut self) -> f32 {
        // SAFETY: as above; reads chip state and touches nothing else.
        unsafe { YMPSG_GetOutput(self.state.as_ptr()) }
    }
}

/// The YM2151-LLE die simulation, driven by its pins.
///
/// The API surface *is* the package: a clock pin, the bus, and the serial
/// DAC output. Everything above wire level -- write pacing, reset timing,
/// the YM3012 float decode -- belongs to the wrapper, which is the point of
/// an LLE core: nothing between the VGM and the netlist but electricity.
#[derive(Debug)]
pub(crate) struct OpmLleChip {
    state: OpaqueChip,
    /// The bus pins as last presented; re-presented on every clock edge.
    pins: LlePins,
}

/// The non-clock input pins. `ic`/`cs`/`wr` are active-low levels, kept
/// electrical here so the driver reads like a schematic.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LlePins {
    pub ym2164: bool,
    /// Reset, active low.
    pub ic: bool,
    /// Chip select, active low.
    pub cs: bool,
    /// Write strobe, active low.
    pub wr: bool,
    /// 0 presents an address, 1 a value.
    pub a0: bool,
    pub data: u8,
}

impl Default for LlePins {
    fn default() -> Self {
        Self {
            ym2164: false,
            ic: true,
            cs: true,
            wr: true,
            a0: false,
            data: 0,
        }
    }
}

impl OpmLleChip {
    pub(crate) fn new() -> Self {
        // SAFETY: both shims return a compile-time constant and touch nothing.
        let (size, align) = unsafe { (drotrim_fmopm_sizeof(), drotrim_fmopm_alignof()) };
        Self {
            state: OpaqueChip::new(size, align),
            pins: LlePins::default(),
        }
    }

    /// Zeroes the die. The electrical reset (holding `ic` low while
    /// clocking) is the wrapper's job; this only clears the allocation, as
    /// power-off does.
    pub(crate) fn power_cycle(&mut self) {
        self.state.storage.fill(0);
        self.pins = LlePins::default();
    }

    pub(crate) fn set_pins(&mut self, pins: LlePins) {
        self.pins = pins;
        // SAFETY: the shim writes only the input fields of the sized block.
        unsafe {
            drotrim_fmopm_set_pins(
                self.state.as_ptr(),
                i32::from(pins.ym2164),
                i32::from(pins.ic),
                i32::from(pins.cs),
                i32::from(pins.wr),
                i32::from(pins.a0),
                i32::from(pins.data),
            );
        }
    }

    /// One half of the master clock: `high` is the level of the clk pin.
    pub(crate) fn clock_edge(&mut self, high: bool) {
        // SAFETY: the block is sized by the C's own `sizeof(fmopm_t)`.
        unsafe { FMOPM_Clock(self.state.as_ptr(), i32::from(high)) }
    }

    /// The serial DAC pins after the last edge: (sh1, sh2, so).
    pub(crate) fn dac_pins(&mut self) -> (bool, bool, bool) {
        let chip = self.state.as_ptr();
        // SAFETY: the shim reads three fields of the sized block.
        unsafe {
            (
                drotrim_fmopm_out_sh1(chip) != 0,
                drotrim_fmopm_out_sh2(chip) != 0,
                drotrim_fmopm_out_so(chip) != 0,
            )
        }
    }
}

/// The YM2612 die of YM2608-LLE, driven by its pins.
///
/// Same idea as [`OpmLleChip`], different package: two bank-select address
/// lines instead of one, and the DAC leaves on two parallel time-multiplexed
/// 9-bit pins rather than a serial stream -- the ladder asymmetry included,
/// because the die computes it.
#[derive(Debug)]
pub(crate) struct Opn2LleChip {
    state: OpaqueChip,
}

/// The non-clock input pins of the OPN2 bus. `ic`/`cs`/`wr` are active-low
/// levels, as electrical as [`LlePins`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Opn2Pins {
    /// Reset, active low.
    pub ic: bool,
    /// Chip select, active low.
    pub cs: bool,
    /// Write strobe, active low.
    pub wr: bool,
    /// 0 presents an address, 1 a value.
    pub a0: bool,
    /// Selects the register bank: part I or part II.
    pub a1: bool,
    pub data: u8,
}

impl Default for Opn2Pins {
    fn default() -> Self {
        Self {
            ic: true,
            cs: true,
            wr: true,
            a0: false,
            a1: false,
            data: 0,
        }
    }
}

impl Opn2LleChip {
    pub(crate) fn new() -> Self {
        // SAFETY: both shims return a compile-time constant and touch nothing.
        let (size, align) = unsafe { (drotrim_fmopna2612_sizeof(), drotrim_fmopna2612_alignof()) };
        Self {
            state: OpaqueChip::new(size, align),
        }
    }

    /// Zeroes the die, as power-off does; the electrical reset is the
    /// wrapper's job.
    pub(crate) fn power_cycle(&mut self) {
        self.state.storage.fill(0);
    }

    pub(crate) fn set_pins(&mut self, pins: Opn2Pins) {
        // SAFETY: the shim writes only the input fields of the sized block.
        unsafe {
            drotrim_fmopna2612_set_pins(
                self.state.as_ptr(),
                i32::from(pins.ic),
                i32::from(pins.cs),
                i32::from(pins.wr),
                i32::from(pins.a0),
                i32::from(pins.a1),
                i32::from(pins.data),
            );
        }
    }

    /// One half of the master clock: `high` is the level of the clk pin.
    pub(crate) fn clock_edge(&mut self, high: bool) {
        // SAFETY: the block is sized by the C's own `sizeof(fmopna_2612_t)`.
        unsafe { FMOPNA_2612_Clock(self.state.as_ptr(), i32::from(high)) }
    }

    /// The multiplexed DAC pins after the last edge: (left, right).
    pub(crate) fn dac_pins(&mut self) -> (i32, i32) {
        let chip = self.state.as_ptr();
        // SAFETY: the shim reads two fields of the sized block.
        unsafe {
            (
                drotrim_fmopna2612_out_mol(chip),
                drotrim_fmopna2612_out_mor(chip),
            )
        }
    }
}

/// The YM2608 die of YM2608-LLE, driven by its pins.
///
/// The package with its own memory bus: the Delta-T sample store is DRAM
/// the wrapper serves -- RAS/CAS multiplexed address on the dm lines, WE
/// for the die's own writes -- while the rhythm ROM is internal to the
/// decap and needs no pins at all. FM, rhythm and ADPCM leave on the
/// serial DAC; the SSG on the analog pin.
#[derive(Debug)]
pub(crate) struct OpnaLleChip {
    state: OpaqueChip,
}

/// The DRAM bus as the die presents it after a clock.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DramBus {
    /// The 8-bit multiplexed address/data lines.
    pub dm: i32,
    /// The ninth bit presented alongside.
    pub a8: bool,
    /// Row-address strobe, de-inverted: true means asserted.
    pub ras: bool,
    /// Column-address strobe, de-inverted.
    pub cas: bool,
    /// Write-enable, de-inverted: true means the die is writing.
    pub we: bool,
    /// True when the die expects data *in* on the lines.
    pub reading: bool,
}

impl OpnaLleChip {
    pub(crate) fn new() -> Self {
        // SAFETY: both shims return a compile-time constant and touch nothing.
        let (size, align) = unsafe { (drotrim_fmopna2608_sizeof(), drotrim_fmopna2608_alignof()) };
        Self {
            state: OpaqueChip::new(size, align),
        }
    }

    /// Zeroes the die, as power-off does.
    pub(crate) fn power_cycle(&mut self) {
        self.state.storage.fill(0);
    }

    pub(crate) fn set_pins(&mut self, pins: Opn2Pins) {
        // SAFETY: the shim writes only the input fields of the sized block.
        unsafe {
            drotrim_fmopna2608_set_pins(
                self.state.as_ptr(),
                i32::from(pins.ic),
                i32::from(pins.cs),
                i32::from(pins.wr),
                i32::from(pins.a0),
                i32::from(pins.a1),
                i32::from(pins.data),
            );
        }
    }

    /// Puts the served memory byte on the DRAM data-in lines.
    pub(crate) fn serve_dm(&mut self, dm: u8) {
        // SAFETY: the shim writes one input field of the sized block.
        unsafe { drotrim_fmopna2608_serve_dm(self.state.as_ptr(), i32::from(dm)) }
    }

    /// One half of the master clock.
    pub(crate) fn clock_edge(&mut self, high: bool) {
        // SAFETY: the block is sized by the C's own `sizeof(fmopna_t)`.
        unsafe { FMOPNA_Clock(self.state.as_ptr(), i32::from(high)) }
    }

    /// The DRAM bus after the last edge. The strobes come back active-low
    /// from the pins and are de-inverted here, once.
    pub(crate) fn dram_bus(&mut self) -> DramBus {
        let (mut dm, mut a8) = (0, 0);
        // SAFETY: the shim reads fields of the sized block into our locals.
        let strobes =
            unsafe { drotrim_fmopna2608_dram_pins(self.state.as_ptr(), &mut dm, &mut a8) };
        DramBus {
            dm,
            a8: a8 != 0,
            ras: strobes & 1 == 0,
            cas: strobes & 2 == 0,
            we: strobes & 4 == 0,
            reading: strobes & 8 != 0,
        }
    }

    /// The serial DAC strobes and data bit, plus the analog (SSG) level:
    /// (sh1, sh2, serial bit, analog).
    pub(crate) fn dac_pins(&mut self) -> (bool, bool, bool, f32) {
        let mut analog = 0.0f32;
        // SAFETY: as above.
        let packed = unsafe { drotrim_fmopna2608_dac_pins(self.state.as_ptr(), &mut analog) };
        (packed & 1 != 0, packed & 2 != 0, packed & 4 != 0, analog)
    }

    /// The serial bit-clock pin.
    pub(crate) fn s_pin(&mut self) -> bool {
        let mut analog = 0.0f32;
        // SAFETY: as above.
        let packed = unsafe { drotrim_fmopna2608_dac_pins(self.state.as_ptr(), &mut analog) };
        packed & 8 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zero size would mean the shim did not link and every core would be
    /// writing into a one-word allocation.
    #[test]
    fn the_shim_reports_a_real_size() {
        // SAFETY: both return compile-time constants.
        let (size, align) = unsafe { (drotrim_opll_sizeof(), drotrim_opll_alignof()) };
        assert!(size > 128, "opll_t came back as {size} bytes");
        assert!(align <= align_of::<u64>(), "{align}");

        // SAFETY: as above.
        let (size, align) = unsafe { (drotrim_ympsg_sizeof(), drotrim_ympsg_alignof()) };
        assert!(size > 64, "ympsg_t came back as {size} bytes");
        assert!(align <= align_of::<u64>(), "{align}");

        // SAFETY: as above.
        let (size, align) = unsafe { (drotrim_fmopm_sizeof(), drotrim_fmopm_alignof()) };
        assert!(size > 1024, "fmopm_t came back as {size} bytes");
        assert!(align <= align_of::<u64>(), "{align}");

        // SAFETY: as above.
        let (size, align) = unsafe { (drotrim_fmopna2612_sizeof(), drotrim_fmopna2612_alignof()) };
        assert!(size > 1024, "fmopna_2612_t came back as {size} bytes");
        assert!(align <= align_of::<u64>(), "{align}");

        // SAFETY: as above.
        let (size, align) = unsafe { (drotrim_fmopna2608_sizeof(), drotrim_fmopna2608_alignof()) };
        assert!(size > 1024, "fmopna_t came back as {size} bytes");
        assert!(align <= align_of::<u64>(), "{align}");
    }
}
