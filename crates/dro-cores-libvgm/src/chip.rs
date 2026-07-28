// SPDX-License-Identifier: GPL-2.0-or-later
//! One wrapper, every chip: [`LibVgmChip`] over libvgm's uniform device API.
//!
//! This is what makes libvgm different in kind from the other providers. A
//! `DEV_DEF` is a vtable of `Start`/`Stop`/`Reset`/`Update` plus a table of
//! width-typed register writers, so the twenty lines below drive a QSound and
//! a SAA1099 alike. What varies per chip is *data* -- a [`ChipSpec`] row saying
//! which device ID, which writer width, and how our engine's
//! `(port, addr, data)` folds into that writer's arguments.
//!
//! # The two conventions that have to be reconciled
//!
//! **libvgm takes the clock at construction.** `DEV_GEN_CFG::clock` is read by
//! `Start`, and the sample rate falls out of it. Our [`ChipCore::reset`] hands
//! a clock to a chip that already exists. So `reset` here *restarts*: stop the
//! old device, start a new one. Same shape as `dro-cores-ymfm`, for the same
//! reason.
//!
//! **libvgm renders planar, our engine wants interleaved.** `Update` writes
//! `outputs[0]` and `outputs[1]` as two separate `INT32` runs and *overwrites*
//! rather than accumulating (upstream's own silent-path `memset` proves it), so
//! [`render`](ChipCore::render) keeps two scratch planes and weaves them.

use std::ffi::c_void;

use dro_core::vgm::{ChipKind, ChipSettings};
use dro_synth::chip::ChipCore;

use crate::ffi::{
    self, DevFuncWriteA8D8, DevFuncWriteA8D16, DevFuncWriteA16D8, DevFuncWriteA16D16, DevGenCfg,
    DevInfo, EERR_OK, RWF_REGISTER, RWF_WRITE, Sn76496Cfg,
};

/// The rate asked for in `DEV_GEN_CFG::smplRate`.
///
/// Nominally unused: we start every chip in `DEVRI_SRMODE_NATIVE` so it renders
/// at its own rate and `dro_synth::resample` does the conversion. But upstream
/// warns that *some cores ignore `srMode` and always use `smplRate`*, and
/// Maxim's SN76489 is one of them -- so for those chips this is not a fallback,
/// it is the rate they will run at.
///
/// 44100 because that is what the pinned parity reference renders at, so a
/// rate-fixed core measures against it with no resampler on either side. The
/// engine is unaffected either way: it resamples from whatever
/// [`native_rate`](ChipCore::native_rate) reports, and that is read back from
/// libvgm rather than assumed.
const REQUESTED_RATE: u32 = 44_100;

/// How a chip's `(port, addr, data)` reaches libvgm's register writer.
///
/// **One variant, because lv-2 has one chip.** lv-3 is where this becomes the
/// real table, and each variant it adds arrives with the chip that needs it --
/// a fold with no chip behind it is a guess about a convention, and the
/// conventions here are exactly what the corpus has to arbitrate. Every
/// variant names the `DEVRW_` width it fetches, because the width and the fold
/// have to agree or the transmute in [`Writer::fetch`] is unsound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteRule {
    /// A chip with no register address: the VGM command carries one byte and
    /// that byte *is* the write. The SN76489 is the archetype -- its `0x50`
    /// command is a bare data byte, and our decoder passes it as `data` with
    /// `port` and `addr` both meaningless.
    ///
    /// Fetches `DEVRW_A8D8` and writes `(0, data)`. The zero is libvgm's
    /// `SN76496_W_REG`, "a normal register write", as opposed to the Game Gear
    /// stereo latch at address 1.
    DataOnly,
}

/// One chip: which libvgm device it is, and how to talk to it.
///
/// A `&'static` table row rather than a trait object, because everything here
/// is data and the alternative is a virtual call per register write.
#[derive(Debug)]
pub(crate) struct ChipSpec {
    /// The registry id, `"<chip slug>.libvgm"`. Written out rather than
    /// composed at runtime because [`CoreInfo::id`](dro_synth::CoreInfo::id) is
    /// a `&'static str` that lands in `drotrim.ini`.
    pub(crate) id: &'static str,
    /// Our engine's name for the chip -- what the registry keys on.
    pub(crate) kind: ChipKind,
    /// libvgm's `DEVID_` constant.
    pub(crate) device: u8,
    /// A four-character code from `EmuCores.h`, or 0 for the device's default
    /// core. lv-6 publishes the alternatives as picker entries; until then
    /// every row takes the default.
    pub(crate) emu_core: u32,
    /// How writes fold.
    pub(crate) write: WriteRule,
    /// Fills in the chip-specific half of the configuration from the VGM
    /// header, if it has one.
    ///
    /// Called with the config already carrying clock, sample-rate mode and the
    /// variant flag, so an implementation only sets what is its own.
    pub(crate) configure: fn(&mut DevConfig, &ChipSettings),
    /// Builds this chip, boxed, for the registry.
    ///
    /// The registry takes a bare `fn` pointer, which cannot capture a spec --
    /// so [`chip_specs!`] emits one of these per row, each naming its own
    /// [`ChipKind`]. That is the whole reason the macro exists.
    pub(crate) make: fn() -> Box<dyn ChipCore>,
}

impl ChipSpec {
    /// This chip's registry id.
    #[must_use]
    pub(crate) const fn registry_id(&self) -> &'static str {
        self.id
    }

    /// How the registry builds it.
    #[must_use]
    pub(crate) const fn maker(&self) -> dro_synth::CoreMaker {
        dro_synth::CoreMaker::Generic(self.make)
    }
}

/// The configuration handed to `SndEmu_Start`.
///
/// libvgm's chips with settings define a struct whose first member is a
/// `DEV_GEN_CFG` and pass a pointer to it cast down. Modelling that as an enum
/// rather than a byte buffer keeps the field access type-checked; the cast at
/// [`as_ptr`](Self::as_ptr) is the same one upstream's own `emutest.c` makes,
/// and `layout.rs` pins the prefix property it relies on.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DevConfig {
    /// A chip whose configuration is only the generic fields.
    Generic(DevGenCfg),
    /// The SN76496 family, whose noise taps and shift-register width decide
    /// which of a dozen parts it actually is.
    Sn76496(Sn76496Cfg),
}

impl DevConfig {
    /// The generic half, which every variant has and every start reads.
    fn generic_mut(&mut self) -> &mut DevGenCfg {
        match self {
            Self::Generic(cfg) => cfg,
            Self::Sn76496(cfg) => &mut cfg.gen_cfg,
        }
    }

    /// A pointer `SndEmu_Start` can read, whatever the real struct is.
    fn as_ptr(&self) -> *const DevGenCfg {
        match self {
            Self::Generic(cfg) => std::ptr::from_ref(cfg),
            Self::Sn76496(cfg) => std::ptr::from_ref(cfg).cast::<DevGenCfg>(),
        }
    }
}

/// A register writer, kept with the width it was fetched at.
///
/// Pairing the pointer with its width in one value is what stops a fold and a
/// signature drifting apart: there is no way to hold an `A8D16` pointer and
/// call it as `A8D8`.
#[derive(Debug, Clone, Copy)]
enum Writer {
    A8D8(DevFuncWriteA8D8),
    A8D16(DevFuncWriteA8D16),
    A16D8(DevFuncWriteA16D8),
    A16D16(DevFuncWriteA16D16),
}

impl Writer {
    /// Fetches the writer `rule` needs from a started device.
    ///
    /// # Safety
    /// `dev_def` must belong to a live device.
    unsafe fn fetch(dev_def: *const ffi::DevDef, rule: WriteRule) -> Option<Self> {
        let width = match rule {
            WriteRule::DataOnly => ffi::DEVRW_A8D8,
        };
        // SAFETY: the caller guarantees `dev_def`.
        let ptr = unsafe { ffi::device_func(dev_def, RWF_REGISTER | RWF_WRITE, width, 0) }?;
        // SAFETY: `width` and the arm below are chosen together, and libvgm's
        // `DEVRW_` constants are exactly the signature contract -- a function
        // filed under `DEVRW_A8D8` takes `(void*, UINT8, UINT8)`.
        Some(unsafe {
            match width {
                ffi::DEVRW_A8D16 => {
                    Self::A8D16(std::mem::transmute::<*mut c_void, DevFuncWriteA8D16>(ptr))
                }
                ffi::DEVRW_A16D8 => {
                    Self::A16D8(std::mem::transmute::<*mut c_void, DevFuncWriteA16D8>(ptr))
                }
                ffi::DEVRW_A16D16 => {
                    Self::A16D16(std::mem::transmute::<*mut c_void, DevFuncWriteA16D16>(ptr))
                }
                _ => Self::A8D8(std::mem::transmute::<*mut c_void, DevFuncWriteA8D8>(ptr)),
            }
        })
    }
}

/// One libvgm chip, owned.
pub struct LibVgmChip {
    spec: &'static ChipSpec,
    /// Zeroed while stopped; `data_ptr` non-null means started.
    dev: DevInfo,
    writer: Option<Writer>,
    /// What the last [`reset`](ChipCore::reset) asked for, kept so
    /// [`configure`](ChipCore::configure) can restart at the same clock.
    clock: u32,
    variant: bool,
    settings: ChipSettings,
    /// The two planes `Update` writes, grown as needed and never shrunk.
    left: Vec<i32>,
    right: Vec<i32>,
}

impl std::fmt::Debug for LibVgmChip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibVgmChip")
            .field("chip", &self.spec.kind)
            .field("device", &self.spec.device)
            .field("clock", &self.clock)
            .field("started", &self.is_started())
            .field("rate", &self.dev.sample_rate)
            .finish()
    }
}

// SAFETY: the device is exclusively owned -- the handle is never cloned or
// handed out -- and the cores this crate compiles hold no mutable file-scope
// state, so all of a chip's mutation is behind `data_ptr`. That was checked
// against the pinned tree rather than assumed, and it is a **per-core**
// property: a core added to `build.rs`'s ENABLED list must be checked for
// mutable globals before it is trusted here. Not `Sync`: two threads must not
// write one chip at once.
unsafe impl Send for LibVgmChip {}

impl LibVgmChip {
    /// A chip built to `spec`, not yet started.
    ///
    /// Starting waits for [`reset`](ChipCore::reset), which is what supplies
    /// the clock -- and the clock is a construction parameter to libvgm, so
    /// there is nothing to build before it arrives.
    #[must_use]
    pub(crate) fn new(spec: &'static ChipSpec) -> Self {
        Self {
            spec,
            dev: DevInfo::empty(),
            writer: None,
            clock: 0,
            variant: false,
            settings: ChipSettings::default(),
            left: Vec::new(),
            right: Vec::new(),
        }
    }

    fn is_started(&self) -> bool {
        !self.dev.data_ptr.is_null()
    }

    /// Stops the device, if one is running. Idempotent.
    fn stop(&mut self) {
        if !self.is_started() {
            return;
        }
        // SAFETY: `dev` was filled by a successful `SndEmu_Start` and is
        // stopped exactly once -- `data_ptr` is cleared below, and every path
        // that starts a device goes through `start`, which stops first.
        unsafe {
            ffi::SndEmu_FreeDevLinkData(&raw mut self.dev);
            ffi::SndEmu_Stop(&raw mut self.dev);
        }
        self.dev = DevInfo::empty();
        self.writer = None;
    }

    /// Stops whatever is running and starts a device at the current clock and
    /// settings.
    ///
    /// A failure leaves the chip stopped rather than half-built, so
    /// [`render`](ChipCore::render) renders silence and nothing reads a dangling
    /// pointer. That is the honest outcome for "this build has no such device":
    /// the registry is what should have prevented it, and a silent chip is
    /// visible in a way a crash is not useful.
    fn start(&mut self) {
        self.stop();
        if self.clock == 0 {
            return;
        }

        let mut config = match self.spec.kind {
            // The one chip whose extended config lv-2 carries; lv-3's table
            // is where this becomes a per-spec constructor rather than a
            // match. Kept here for now so the enum has exactly one writer.
            ChipKind::Sn76489 => DevConfig::Sn76496(Sn76496Cfg::default()),
            _ => DevConfig::Generic(DevGenCfg::default()),
        };
        {
            let generic = config.generic_mut();
            generic.emu_core = self.spec.emu_core;
            generic.sr_mode = ffi::DEVRI_SRMODE_NATIVE;
            generic.flags = u8::from(self.variant);
            generic.clock = self.clock;
            generic.smpl_rate = REQUESTED_RATE;
        }
        (self.spec.configure)(&mut config, &self.settings);

        let mut dev = DevInfo::empty();
        // SAFETY: `config` outlives the call, its pointer is the documented
        // cast to the generic prefix, and `dev` is a valid out-param.
        let started = unsafe { ffi::SndEmu_Start(self.spec.device, config.as_ptr(), &raw mut dev) };
        if started != EERR_OK || dev.data_ptr.is_null() || dev.dev_def.is_null() {
            log::warn!(
                "libvgm refused to start {} (device {:#04x}): error {started:#04x}",
                self.spec.kind.name(),
                self.spec.device,
            );
            return;
        }

        self.dev = dev;
        // SAFETY: a live device definition from the successful start above.
        self.writer = unsafe { Writer::fetch(dev.dev_def, self.spec.write) };
        if self.writer.is_none() {
            log::warn!(
                "libvgm's {} has no register writer of the width {:?} expects",
                self.spec.kind.name(),
                self.spec.write,
            );
        }

        // SAFETY: as above -- a live device, reset exactly as upstream's own
        // example does immediately after starting.
        unsafe {
            if let Some(reset) = (*dev.dev_def).reset {
                reset(dev.data_ptr);
            }
        }
    }
}

impl Drop for LibVgmChip {
    fn drop(&mut self) {
        self.stop();
    }
}

impl ChipCore for LibVgmChip {
    /// Restarts at `clock`, because libvgm reads the clock at construction and
    /// derives the sample rate from it.
    fn reset(&mut self, clock: u32, variant: bool) {
        self.clock = clock;
        self.variant = variant;
        // A reset discards the header settings too: `configure` always follows
        // it (see `VgmEngine::voice`), and carrying the previous file's noise
        // taps into the gap between the two would be a silent bug.
        self.settings = ChipSettings::default();
        self.start();
    }

    /// Restarts again, now that the header's chip settings are known.
    ///
    /// libvgm wants them at construction and our engine delivers them after
    /// reset, so the second start is how the two orders are reconciled. It
    /// costs one allocation per chip per file load, and it happens before any
    /// register write, so nothing is lost.
    fn configure(&mut self, settings: &ChipSettings) {
        self.settings = *settings;
        self.start();
    }

    fn native_rate(&self) -> u32 {
        self.dev.sample_rate.max(1)
    }

    /// `_port` and `_addr` are underscored because the only rule lv-2 carries
    /// has no register address -- the same signature the clean-room SN76489
    /// writes. lv-3's table is what starts reading them.
    fn write(&mut self, _port: u8, _addr: u16, data: u16) {
        let Some(writer) = self.writer else {
            return;
        };
        let (address, value) = match self.spec.write {
            WriteRule::DataOnly => (0u16, data),
        };
        // SAFETY: `writer` was fetched from the device that `data_ptr` belongs
        // to and is held with its own width, so each arm calls the signature
        // libvgm filed the pointer under.
        unsafe {
            match writer {
                Writer::A8D8(write) => {
                    write(self.dev.data_ptr, address as u8, value as u8);
                }
                Writer::A8D16(write) => write(self.dev.data_ptr, address as u8, value),
                Writer::A16D8(write) => write(self.dev.data_ptr, address, value as u8),
                Writer::A16D16(write) => write(self.dev.data_ptr, address, value),
            }
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        if !self.is_started() {
            out.fill(0);
            return;
        }

        // Grown, never shrunk: a worklet pulling 128 frames after an offline
        // render pulled 4096 should not reallocate.
        if self.left.len() < frames {
            self.left.resize(frames, 0);
            self.right.resize(frames, 0);
        }
        let mut planes = [self.left.as_mut_ptr(), self.right.as_mut_ptr()];

        // SAFETY: a live device; `Update` writes exactly `frames` samples into
        // each of the two planes, both of which are at least that long and
        // outlive the call.
        unsafe {
            let Some(update) = (*self.dev.dev_def).update else {
                out.fill(0);
                return;
            };
            update(self.dev.data_ptr, frames as u32, planes.as_mut_ptr());
        }

        for (frame, (&left, &right)) in out
            .chunks_exact_mut(2)
            .zip(self.left.iter().zip(self.right.iter()))
        {
            frame[0] = left;
            frame[1] = right;
        }
    }
}

/// Declares the chip table and, per row, the bare `fn` the registry needs.
///
/// A registry entry is `(id, ChipKind, fn() -> Box<dyn ChipCore>)` and that
/// last one cannot be a closure over a spec, so each chip needs a function
/// that names its own kind. Writing them by hand would be two lines of
/// boilerplate per chip and one opportunity per chip to pair the wrong id with
/// the wrong device; this way a chip is one line and the three cannot drift.
macro_rules! chip_specs {
    ($(
        $make:ident : $id:literal => $kind:ident,
        $device:expr, $emu_core:expr, $write:expr, $configure:expr ;
    )*) => {
        /// Every chip this crate can build, in the order the registry lists
        /// them.
        ///
        /// lv-4 is what grows this; a row here must also have its device named
        /// in `build.rs`'s `ENABLED`, or the start fails and the chip is
        /// silent. A `static` rather than a `const` on purpose: the makers
        /// below take `&'static` references into it, which a `const` -- being
        /// a fresh value at each use -- could not give them.
        pub(crate) static SPECS: &[ChipSpec] = &[$(
            ChipSpec {
                id: $id,
                kind: ChipKind::$kind,
                device: $device,
                emu_core: $emu_core,
                write: $write,
                configure: $configure,
                make: $make,
            },
        )*];

        $(
            fn $make() -> Box<dyn ChipCore> {
                Box::new(LibVgmChip::new(spec_for(ChipKind::$kind)))
            }
        )*
    };
}

chip_specs! {
    // `FCC_MAXM`, not the device default (`FCC_MAME`), and the difference is
    // the whole lv-2 measurement. The pinned reference config selects
    // `[SN76496] Core = MAXM`; asking libvgm for its default put Maxim's core
    // on one side and MAME's on the other and scored 0.5353 -- a number about
    // two emulators disagreeing, not about our binding. Matching the reference
    // is what makes the comparison a test of *this crate*.
    make_sn76489: "sn76489.libvgm" => Sn76489,
        ffi::DEVID_SN76496, ffi::FCC_MAXM, WriteRule::DataOnly, configure_sn76496;
}

/// The spec for `kind`.
///
/// # Panics
/// If `kind` has no row -- which only a maker generated by [`chip_specs!`] can
/// ask for, and the macro generates one maker per row, so it cannot happen.
#[must_use]
pub(crate) fn spec_for(kind: ChipKind) -> &'static ChipSpec {
    SPECS
        .iter()
        .find(|spec| spec.kind == kind)
        .unwrap_or_else(|| unreachable!("chip_specs! generates a maker per row"))
}

/// The SN76489's identity, from the VGM header.
///
/// Every field here changes what the part *is* rather than how it sounds, and
/// the frozen scorecard records what a wrong one costs: the noise channel
/// emits a different pseudo-random sequence entirely.
///
/// **Transcribed from libvgm's own `player/vgmplayer.cpp`**, not derived from
/// the VGM specification, and that is the rule this function exists to
/// establish for lv-3. A first attempt here read the spec and got six of the
/// seven fields wrong -- inverted sense on `stereo`, hard-coded `negate` and
/// `clkDiv`, `segaPSG` and `ncrPSG` missed entirely, and both defaults set to
/// the TI part when libvgm's are the SEGA PSG's. Every one of those is a
/// silent wrongness: the chip still starts, still sounds, and is simply a
/// different part. The player is the authority because it is the code the
/// reference measurement runs.
///
/// The flag bits, for reading alongside `vgmplayer.cpp`:
/// `0x01` frequency 0 is 0x400 (so *clear* means the SEGA behaviour),
/// `0x02` negate output, `0x04` stereo off, `0x08` clock divider off,
/// `0x10` NCR noise algorithm.
fn configure_sn76496(config: &mut DevConfig, settings: &ChipSettings) {
    let DevConfig::Sn76496(cfg) = config else {
        debug_assert!(false, "the SN76489 spec must be given an Sn76496 config");
        return;
    };
    let flags = settings.sn76489_flags;
    cfg.shift_reg_width = if settings.sn76489_shift_width == 0 {
        0x10
    } else {
        settings.sn76489_shift_width
    };
    cfg.noise_taps = if settings.sn76489_feedback == 0 {
        0x09
    } else {
        settings.sn76489_feedback
    };
    cfg.sega_psg = u8::from(flags & 0x01 == 0);
    cfg.negate = u8::from(flags & 0x02 != 0);
    cfg.stereo = u8::from(flags & 0x04 == 0);
    cfg.clk_div = if flags & 0x08 != 0 { 1 } else { 8 };
    cfg.ncr_psg = u8::from(flags & 0x10 != 0);
    cfg.t6w28_tone = std::ptr::null_mut();
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::spec_for as spec;

    fn energy(out: &[i32]) -> i64 {
        out.iter().map(|&s| i64::from(s.abs())).sum()
    }

    /// Construction, native rate, writes and render, end to end through the
    /// `ChipCore` trait rather than through the raw FFI -- the lv-2 gate.
    #[test]
    fn the_generic_binding_drives_a_chip_end_to_end() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
        chip.reset(3_579_545, false);
        chip.configure(&ChipSettings::default());

        assert!(
            chip.native_rate() > 8_000,
            "rate {} looks wrong",
            chip.native_rate()
        );

        let mut quiet = vec![0i32; 4096];
        chip.render(&mut quiet);
        let at_rest = energy(&quiet);

        // The `0x50` command's byte, exactly as our decoder hands it over:
        // latch channel 0's period, its high bits, then un-attenuate it.
        chip.write(0, 0, 0x8E);
        chip.write(0, 0, 0x02);
        chip.write(0, 0, 0x90);

        let mut loud = vec![0i32; 4096];
        chip.render(&mut loud);
        assert!(
            energy(&loud) > at_rest * 4 + 1000,
            "the chip must sound after a write (rest {at_rest}, playing {})",
            energy(&loud)
        );
    }

    /// A chip that was never reset has no clock, so it never started -- and
    /// renders silence rather than reading a null `data_ptr`.
    #[test]
    fn an_unstarted_chip_is_silent_rather_than_unsound() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
        assert_eq!(chip.native_rate(), 1, "no device, so no rate");

        let mut out = vec![7i32; 64];
        chip.render(&mut out);
        assert!(out.iter().all(|&s| s == 0));

        // And a write with nowhere to go is dropped, not dereferenced.
        chip.write(0, 0, 0x90);
    }

    /// Reset is a restart, and it really does discard state: a chip made loud,
    /// then reset, is quiet again.
    #[test]
    fn reset_discards_the_previous_devices_state() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
        chip.reset(3_579_545, false);
        chip.write(0, 0, 0x8E);
        chip.write(0, 0, 0x02);
        chip.write(0, 0, 0x90);

        let mut loud = vec![0i32; 2048];
        chip.render(&mut loud);
        assert!(energy(&loud) > 1000, "sanity: it should be playing");

        chip.reset(3_579_545, false);
        let mut after = vec![0i32; 2048];
        chip.render(&mut after);
        assert!(
            energy(&after) * 4 < energy(&loud),
            "a reset chip should be far quieter than a playing one \
             (was {}, now {})",
            energy(&loud),
            energy(&after)
        );
    }

    /// `native_rate` reports what the core will *actually* render at, which is
    /// not always derived from the clock.
    ///
    /// Upstream warns that some cores ignore `srMode` and always use
    /// `smplRate`, and Maxim's SN76489 is one: asked for native mode it still
    /// answers [`REQUESTED_RATE`]. That is not a defect and not something to
    /// work around -- the engine resamples from whatever `native_rate` says --
    /// but it *is* worth pinning, because the obvious assumption (a libvgm
    /// chip's rate follows its clock, as ymfm's does) is false, and code
    /// written on it would look right and drift pitch.
    #[test]
    fn the_rate_is_whatever_the_core_will_really_render_at() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
        chip.reset(3_579_545, false);
        let slow = chip.native_rate();
        chip.reset(3_579_545 * 2, false);
        let fast = chip.native_rate();

        assert!(slow > 0 && fast > 0, "a started chip has a rate");
        assert_eq!(
            (slow, fast),
            (REQUESTED_RATE, REQUESTED_RATE),
            "Maxim's SN76489 ignores srMode and renders at the rate it was \
             asked for; if this ever starts following the clock, the core \
             changed and the parity row must be re-measured"
        );
    }

    /// Rendering repeatedly must not depend on how the caller chunks it: the
    /// engine relies on a 128-frame worklet pull sounding identical to a
    /// 4096-frame offline render.
    #[test]
    fn chunking_the_render_does_not_change_it() {
        let play = |chunk: usize| -> Vec<i32> {
            let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
            chip.reset(3_579_545, false);
            chip.write(0, 0, 0x8E);
            chip.write(0, 0, 0x02);
            chip.write(0, 0, 0x90);
            let mut out = vec![0i32; 4096];
            for block in out.chunks_mut(chunk * 2) {
                chip.render(block);
            }
            out
        };
        assert_eq!(play(2048), play(128), "render must be chunk-independent");
    }

    /// The header's noise taps and shift width reach libvgm, and changing them
    /// changes the sound. Without this, `configure` could be a no-op and every
    /// test above would still pass -- which is exactly the bug the frozen
    /// scorecard caught in our own core.
    #[test]
    fn the_headers_noise_settings_reach_the_chip() {
        let noise_with = |feedback: u16, width: u8| -> Vec<i32> {
            let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
            chip.reset(3_579_545, false);
            chip.configure(&ChipSettings {
                sn76489_feedback: feedback,
                sn76489_shift_width: width,
                ..ChipSettings::default()
            });
            // Channel 3 is the noise channel: select white noise at the
            // fastest rate, then un-attenuate it.
            chip.write(0, 0, 0xE4);
            chip.write(0, 0, 0xF0);
            let mut out = vec![0i32; 8192];
            chip.render(&mut out);
            out
        };

        let ti = noise_with(0x0003, 15);
        let sega = noise_with(0x0009, 16);
        assert!(energy(&ti) > 1000, "the noise channel should sound");
        assert_ne!(
            ti, sega,
            "a 15-bit 0x0003 register and a 16-bit 0x0009 one must produce \
             different sequences -- equal output means `configure` never \
             reached the chip"
        );
    }

    /// The header-to-config mapping is libvgm's player's, field for field.
    ///
    /// Pinned because getting it wrong is *silent*: every field here selects a
    /// different real part, and the chip starts and sounds either way. The
    /// expected values are read straight off `player/vgmplayer.cpp`'s
    /// `DEVID_SN76496` arm at the pinned commit.
    #[test]
    fn the_header_maps_to_libvgms_own_config_fields() {
        let built = |settings: ChipSettings| -> Sn76496Cfg {
            let mut config = DevConfig::Sn76496(Sn76496Cfg::default());
            configure_sn76496(&mut config, &settings);
            let DevConfig::Sn76496(cfg) = config else {
                unreachable!()
            };
            cfg
        };

        // An empty header: libvgm falls back to the SEGA PSG, *not* the TI
        // part -- 16-bit register, taps 0x09 -- and every flag reads as its
        // zero sense.
        let empty = built(ChipSettings::default());
        assert_eq!(empty.shift_reg_width, 0x10);
        assert_eq!(empty.noise_taps, 0x09);
        assert_eq!(empty.sega_psg, 1, "flag 0x01 clear means SEGA frequencies");
        assert_eq!(empty.negate, 0);
        assert_eq!(empty.stereo, 1, "flag 0x04 clear means stereo *on*");
        assert_eq!(empty.clk_div, 8, "flag 0x08 clear means the divider is on");
        assert_eq!(empty.ncr_psg, 0);

        // The TI SN76489 as the corpus's own files declare it.
        let ti = built(ChipSettings {
            sn76489_feedback: 0x0003,
            sn76489_shift_width: 15,
            sn76489_flags: 0x02,
            ..ChipSettings::default()
        });
        assert_eq!((ti.shift_reg_width, ti.noise_taps), (15, 0x0003));
        assert_eq!(ti.negate, 1, "flag 0x02 set negates the output");

        // Every flag set: the opposite sense of each.
        let all = built(ChipSettings {
            sn76489_flags: 0x1F,
            ..ChipSettings::default()
        });
        assert_eq!(all.sega_psg, 0);
        assert_eq!(all.negate, 1);
        assert_eq!(all.stereo, 0);
        assert_eq!(all.clk_div, 1);
        assert_eq!(all.ncr_psg, 1);
    }

    /// Dropping a started chip stops its device. Nothing observable proves a
    /// free happened, so this is a leak-check under a loop rather than an
    /// assertion: it exists so a missing `Stop` shows up under a sanitiser or
    /// as unbounded growth rather than never.
    #[test]
    fn chips_can_be_built_and_dropped_repeatedly() {
        for _ in 0..64 {
            let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
            chip.reset(3_579_545, false);
            let mut out = vec![0i32; 256];
            chip.render(&mut out);
        }
    }
}
