//! Which core plays which chip -- as data, not as a `match`.
//!
//! A registry of [`CoreInfo`] rows rather than a hard-coded arm per chip. A
//! `match` answers "is this chip playable" and nothing else -- not *how many*
//! cores a chip has, what they are called, what they cost in license terms, or
//! which one the user asked for, every one of which the Settings core picker
//! and the About credits need answered.
//!
//! **Dependency direction is the point.** A provider crate depends on this
//! crate for the [`ChipCore`] trait and exports a plain
//! `register(&mut CoreRegistry)`; this crate names no provider. That keeps the
//! copyleft cores out of a permissively-licensed `vgms-synth` (see
//! `licenses/README.md`) while letting the *application* link them, and it
//! means registration is explicit and ordered rather than link-time magic --
//! which matters on wasm, where a provider may simply not exist and the UI
//! should follow the registry rather than offer something absent.
//!
//! **Priority is registration order**, with one named escape hatch. The first
//! core registered for a chip is its default: the app registers
//! `vgms-cores-libvgm` ahead of the other providers, so libvgm is the default
//! for every chip it serves and the Nuked and LLE integrations are the
//! picker's alternatives. OPL is the standing exception -- libvgm compiles no
//! OPL device, so the built-in Nuked-OPL3 row keeps that family -- and
//! [`CoreRegistry::promote`] is the owner's per-chip override for the cases
//! where one chip's default should come from a later provider without
//! dragging that provider's crate-mates forward (the app promotes Nuked back
//! over libvgm for the YM2612, YM2151 and YM2413).

use vgms_core::vgm::ChipKind;

use crate::channel_gate::{ChannelGate, GateAction};
use crate::chip::ChipCore;
use crate::opl::OplChip;
use crate::opl_adapter::OplCoreAdapter;

/// How a registered core is brought into being.
///
/// Only a [`Routed`](Self::Routed) entry builds no [`ChipCore`]: RetroWave
/// output is a whole audio service in a native-only crate, chosen by id rather
/// than pulled for samples. The OPL entries build both ways -- an [`OplChip`]
/// for `DroEngine`, and, since ou-1, a `ChipCore` for `VgmEngine` through the
/// [`OplCoreAdapter`].
pub enum CoreMaker {
    /// Built here and driven by `VgmEngine`.
    Generic(fn() -> Box<dyn ChipCore>),
    /// An OPL core. Built as an [`OplChip`], then wrapped in an
    /// [`OplCoreAdapter`] as a `ChipCore` for `VgmEngine`
    /// ([`build`](CoreInfo::build)). Takes the sample rate, because an OPL core
    /// resamples to it rather than declaring its own.
    Opl(fn(u32) -> Box<dyn OplChip>),
    /// Registered so it can be listed and selected, but not constructed here:
    /// the app knows what its id means. RetroWave hardware is one -- choosing
    /// it swaps the whole audio *service*, since a board that mixes its own
    /// sound is not something the engine pulls samples from.
    Routed,
}

impl core::fmt::Debug for CoreMaker {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Generic(_) => "Generic(..)",
            Self::Opl(_) => "Opl(..)",
            Self::Routed => "Routed",
        })
    }
}

/// One core, for one chip.
///
/// A core serving several chips gets one row per chip, because everything a row
/// answers -- is it the default, did the user pick it, is it real-time here --
/// is asked per chip.
#[derive(Debug)]
pub struct CoreInfo {
    /// Stable identifier, `"<chip slug>.<core>"`, e.g. `"opl3.nuked"`.
    ///
    /// This is what lands in `vgmstudio.ini`, so it outlives any label change.
    pub id: &'static str,
    /// The chip this row serves.
    pub chip: ChipKind,
    /// What the Settings row calls it, e.g. `"Nuked-CQM (SB16 Vibra)"`.
    pub label: &'static str,
    /// Who wrote it; `"this project"` for a core written here.
    pub authors: &'static str,
    /// SPDX expression. Shown small beside the label, because a user choosing
    /// an LLE core deserves to see "GPL-2.0" before they choose it.
    pub license: &'static str,
    /// Where the source lives; empty for a clean-room core with no upstream.
    pub upstream: &'static str,
    /// Whether it can keep up with playback. `false` marks the LLE tier:
    /// offline render and oracle use only, never the transport.
    /// [`playability`](crate::chip::playability) filters on it (the transport
    /// must not offer a core that cannot keep up), while the WAV render does
    /// not (it has all the time in the world).
    pub realtime: bool,
    /// Whether this core can place individual channels in the stereo image --
    /// [`ChipCore::set_channel_pans`](crate::ChipCore::set_channel_pans) for a
    /// generic core, the stereo-ext register path for an OPL one. The UI
    /// hides pan controls when the resolved core says `false`, rather than
    /// drawing knobs that turn and do nothing.
    pub channel_pan: bool,
    /// Whether this core honours per-channel muting --
    /// [`ChipCore::set_channel_mutes`](crate::ChipCore::set_channel_mutes). The
    /// libvgm cores implement it, and the Nuked OPN2/OPLL bindings gate their
    /// own render loops (the chips' outputs are time-multiplexed, so muting is
    /// choosing which cycles to add); Nuked-OPM cannot -- its DAC accumulates
    /// all eight channels inside the chip -- and the LLE tier inherits the
    /// trait's no-op default. The UI disables the mute buttons for a chip whose
    /// resolved core says `false`, rather than drawing toggles that silence
    /// nothing. (An OPL document mutes through the register-gating path
    /// instead, which every OPL core supports, so this only gates the generic
    /// multichip panel.)
    pub channel_mute: bool,
    /// This core's output calibration, in 8.8 fixed point
    /// ([`LEVEL_UNITY`] = 1.0). Applied to every sample it renders.
    ///
    /// The calibration belongs to the *core*, not the chip: **two cores for
    /// one chip need not agree on how loud that chip is.** 8.8 fixed point
    /// rather than a float because the reference expresses its own chip volumes
    /// that way (VGMPlay's `MulFixed8x8`), and because [`ChipCore`] forbids
    /// output that could differ across targets.
    ///
    /// **A number here is a measurement, not a preference.** It is
    /// `LEVEL_UNITY / lvl`, where `lvl` is the RMS ratio the parity harness
    /// reports for this core against the pinned reference -- so a core at half
    /// the reference's level carries `0x200`. Leave it at [`LEVEL_UNITY`] until
    /// measured; an unmeasured guess is worse than none.
    ///
    /// **The RMS ratio, not the harness's `gain` column.** The least-squares
    /// fit is `α = ρ · σ_reference / σ_ours`, so it reports a small α for any
    /// decorrelated pair whatever its level, and two cores for one chip are
    /// exactly the pair most likely to decorrelate. `parity::ChannelScore`
    /// spells the trap out; the libvgm YM2612 walked into it, reading `lvl
    /// 0.466 gain 1.766` on the same twelve files.
    pub level: u16,
    /// How to build it, or why it is not built here.
    pub make: CoreMaker,
}

/// Unity in the 8.8 fixed point [`CoreInfo::level`] uses: no correction.
pub const LEVEL_UNITY: u16 = 0x0100;

impl CoreInfo {
    /// Builds this core with its [`level`](Self::level) applied, or `None`
    /// when it is one the app routes rather than constructs.
    ///
    /// The wrapper is what makes the calibration belong to the registry rather
    /// than to each provider: a core stays raw, and the row that describes it
    /// says how loud it is. At [`LEVEL_UNITY`] there is no wrapper at all, so
    /// every core that has not been measured pays nothing for the mechanism.
    ///
    /// An OPL core is hosted through an [`OplCoreAdapter`], so `VgmEngine` can
    /// drive the OPL family like any other chip (Stage K / ou-1); only a
    /// [`Routed`](CoreMaker::Routed) core (RetroWave) genuinely has no
    /// [`ChipCore`] to build.
    #[must_use]
    pub fn build(&self) -> Option<Box<dyn ChipCore>> {
        let core: Box<dyn ChipCore> = match self.make {
            CoreMaker::Generic(make) => make(),
            // An OPL core answers to `DroEngine` as an `OplChip`; the adapter
            // presents it as a `ChipCore` so `VgmEngine` can host it too. It runs
            // the chip at its native rate and lets the engine's Voice resampler
            // convert to the output rate, so no output rate need reach here.
            CoreMaker::Opl(make) => Box::new(OplCoreAdapter::new(make(crate::NATIVE_SAMPLE_RATE))),
            CoreMaker::Routed => return None,
        };
        // A core with no native mute gets the write-gating wrapper, so
        // per-channel muting works on it too (Nuked-OPM, the LLE tier, and the
        // OPL rows). A core that mutes natively keeps that path -- output-masking
        // isolates better -- and a chip with no gate table yet is left honestly
        // un-muteable. The gate goes *inside* the level wrapper so both the
        // Settings path and a per-render pick get it for free.
        let core = if !self.channel_mute && ChannelGate::exists(self.chip) {
            GatedCore::wrap(core, self.chip)
        } else {
            core
        };
        Some(Leveled::wrap(core, self.level))
    }
}

/// A core with its [`CoreInfo::level`] applied to everything it renders.
///
/// Only interposed when the level is not unity, so the common case is the bare
/// core and not a virtual call per buffer.
struct Leveled {
    inner: Box<dyn ChipCore>,
    /// 8.8 fixed point, as [`CoreInfo::level`].
    level: u16,
}

// `ChipCore` is not `Debug` (a core is a pile of emulator state, and printing
// it would be neither possible nor useful), so this reports what it wraps
// rather than deriving.
impl core::fmt::Debug for Leveled {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Leveled")
            .field("level", &self.level)
            .finish_non_exhaustive()
    }
}

impl Leveled {
    fn wrap(inner: Box<dyn ChipCore>, level: u16) -> Box<dyn ChipCore> {
        if level == LEVEL_UNITY {
            return inner;
        }
        Box::new(Self { inner, level })
    }
}

impl ChipCore for Leveled {
    fn reset(&mut self, clock: u32, variant: bool) {
        self.inner.reset(clock, variant);
    }

    fn configure(&mut self, settings: &vgms_core::vgm::ChipSettings) {
        self.inner.configure(settings);
    }

    fn native_rate(&self) -> u32 {
        self.inner.native_rate()
    }

    fn write(&mut self, port: u8, addr: u16, data: u16) {
        self.inner.write(port, addr, data);
    }

    fn replay_write(&mut self, port: u8, addr: u16, data: u16) {
        self.inner.replay_write(port, addr, data);
    }

    fn load_rom(&mut self, block_type: u8, total_size: u32, start: u32, data: &[u8]) {
        self.inner.load_rom(block_type, total_size, start, data);
    }

    fn write_ram(&mut self, offset: u32, data: &[u8]) {
        self.inner.write_ram(offset, data);
    }

    fn write_ram_absolute(&mut self, address: u32, data: &[u8]) {
        self.inner.write_ram_absolute(address, data);
    }

    fn set_channel_mutes(&mut self, muted: u32) {
        self.inner.set_channel_mutes(muted);
    }

    fn set_channel_pans(&mut self, pans: &[i16]) {
        self.inner.set_channel_pans(pans);
    }

    fn supports_pan(&self) -> bool {
        self.inner.supports_pan()
    }

    /// Renders, then scales.
    ///
    /// Integer throughout: `i64` for the product so a loud sample times a gain
    /// above unity cannot wrap, and a saturating cast rather than a truncating
    /// one so the worst case is a clipped peak instead of a sign flip. The
    /// engine still clips once at the end -- this only stops the intermediate
    /// from lying.
    fn render(&mut self, out: &mut [i32]) {
        self.inner.render(out);
        let level = i64::from(self.level);
        for sample in out.iter_mut() {
            let scaled = (i64::from(*sample) * level) >> 8;
            *sample = scaled.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        }
    }
}

/// A core given per-channel muting by write-gating, for a core whose emulator
/// has none of its own.
///
/// The decorator sibling of [`Leveled`]: it runs every write through a
/// [`ChannelGate`] and applies the gate's verdict against the inner core, and
/// turns a mute mask into the synthesised writes that carry it. Only interposed
/// for a `channel_mute: false` row whose chip the gate covers (see
/// [`CoreInfo::build`]); everything else it forwards verbatim, exactly as
/// `Leveled` does.
struct GatedCore {
    inner: Box<dyn ChipCore>,
    gate: ChannelGate,
    /// The whole chip is muted right now: the gate stands down (passing every
    /// write, keeping its shadows current) and the engine's own whole-chip
    /// silence takes over, so un-muting resumes held notes like a native-mute
    /// chip. The next partial mask re-asserts from a clean baseline.
    standing_down: bool,
    /// Whether a whole-chip mute is allowed to stand the gate down. True on the
    /// emulated path, where [`Voice::silenced`](crate::vgm_engine) zeroes the
    /// muted chip's frames in the mix, so the gate need not touch the writes.
    /// **False for hardware** ([`opl_hardware_core`]): a real chip *is* the
    /// audio -- nothing downstream masks it -- so a whole-chip mute must gate
    /// every channel's key at the register level, exactly as a partial mute
    /// does, or the muted notes sound.
    stand_down_allowed: bool,
    /// Whether the mute mask is also forwarded to the inner core. True in the
    /// build path; the A/B harness ([`gate_without_forwarding`]) sets it false so
    /// a native-mute core underneath does not mute too, letting the gate's own
    /// isolation be measured against that native mute.
    forward_mask: bool,
}

impl core::fmt::Debug for GatedCore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GatedCore")
            .field("gate", &self.gate)
            .field("standing_down", &self.standing_down)
            .finish_non_exhaustive()
    }
}

impl GatedCore {
    /// Wraps `inner` in the gate for `chip`, or returns it bare when the chip has
    /// no gate table (which [`CoreInfo::build`] has already checked, so this is a
    /// safety net rather than a path taken).
    fn wrap(inner: Box<dyn ChipCore>, chip: ChipKind) -> Box<dyn ChipCore> {
        match ChannelGate::new(chip) {
            Some(gate) => Box::new(Self {
                inner,
                gate,
                standing_down: false,
                stand_down_allowed: true,
                forward_mask: true,
            }),
            None => inner,
        }
    }
}

/// Hosts an [`OplChip`] as a [`ChipCore`] for the hardware pump: the
/// [`OplCoreAdapter`] that lets [`VgmEngine`](crate::vgm_engine) drive it, wrapped
/// in the OPL [`ChannelGate`] with the whole-chip stand-down *disabled*.
///
/// The emulated path lets a whole-chip mute stand the gate down, because
/// [`Voice::silenced`](crate::vgm_engine) zeroes that chip's frames in the mix.
/// A hardware chip has no such mix -- its registers *are* the sound -- so the
/// gate must gate every channel's key at the register level, a full mask
/// included. Everything else is a normal gated OPL core: partial mutes, seek
/// replays and pan all behave exactly as they do on the emulated path.
#[must_use]
pub fn opl_hardware_core(opl: Box<dyn OplChip>, chip: ChipKind) -> Box<dyn ChipCore> {
    let adapter: Box<dyn ChipCore> = Box::new(OplCoreAdapter::new(opl));
    match ChannelGate::new(chip) {
        Some(gate) => Box::new(GatedCore {
            inner: adapter,
            gate,
            standing_down: false,
            stand_down_allowed: false,
            forward_mask: true,
        }),
        // Every OPL chip has a gate table, so this is a safety net rather than a
        // path taken; an un-gated OPL core would simply mute nothing.
        None => adapter,
    }
}

/// Wraps `inner` in the channel gate for `chip` *without* forwarding the mute
/// mask to the inner core, so only the gate's write-filtering silences a channel.
///
/// The A/B harness's tool, not part of the app's build path (`build` always
/// forwards). It lets a test compare the gate's isolation against the *same*
/// core's native mute: a normally-built gate forwards the mask too, so a
/// native-mute core underneath would mute natively and the comparison would be
/// vacuous. `None` for a chip the gate does not cover.
#[doc(hidden)]
#[must_use]
pub fn gate_without_forwarding(
    inner: Box<dyn ChipCore>,
    chip: ChipKind,
) -> Option<Box<dyn ChipCore>> {
    ChannelGate::new(chip).map(|gate| {
        Box::new(GatedCore {
            inner,
            gate,
            standing_down: false,
            stand_down_allowed: true,
            forward_mask: false,
        }) as Box<dyn ChipCore>
    })
}

impl ChipCore for GatedCore {
    fn reset(&mut self, clock: u32, variant: bool) {
        self.gate.reset(clock, variant);
        self.standing_down = false;
        self.inner.reset(clock, variant);
    }

    fn configure(&mut self, settings: &vgms_core::vgm::ChipSettings) {
        self.gate.configure(settings);
        self.inner.configure(settings);
    }

    fn native_rate(&self) -> u32 {
        self.inner.native_rate()
    }

    fn write(&mut self, port: u8, addr: u16, data: u16) {
        match self.gate.filter(port, addr, data) {
            GateAction::Pass => self.inner.write(port, addr, data),
            GateAction::Drop => {}
            GateAction::Replace(value) => self.inner.write(port, addr, value),
        }
    }

    fn replay_write(&mut self, port: u8, addr: u16, data: u16) {
        // Gate a replayed write exactly as a live one -- the muted channels'
        // keys must stay cleared through a seek too -- but hand it to the inner
        // core's immediate path.
        match self.gate.filter(port, addr, data) {
            GateAction::Pass => self.inner.replay_write(port, addr, data),
            GateAction::Drop => {}
            GateAction::Replace(value) => self.inner.replay_write(port, addr, value),
        }
    }

    fn load_rom(&mut self, block_type: u8, total_size: u32, start: u32, data: &[u8]) {
        self.inner.load_rom(block_type, total_size, start, data);
    }

    fn write_ram(&mut self, offset: u32, data: &[u8]) {
        self.inner.write_ram(offset, data);
    }

    fn write_ram_absolute(&mut self, address: u32, data: &[u8]) {
        self.inner.write_ram_absolute(address, data);
    }

    fn set_channel_mutes(&mut self, muted: u32) {
        let mut writes = Vec::new();
        if self.stand_down_allowed && self.gate.is_full(muted) {
            // Whole chip muted: stand the gate down and let the engine's own
            // silence take over. Restating a full mask (every seek does) is
            // idempotent. Hardware forbids this (`stand_down_allowed` false) --
            // there is no mix to silence the chip, so a full mask keeps gating.
            self.gate.stand_down();
            self.standing_down = true;
        } else if self.standing_down {
            // Leaving the stand-down: the gate passed writes untouched while it
            // stood down, so re-assert every channel rather than diff.
            self.gate.reassert_mask(muted, &mut writes);
            self.standing_down = false;
        } else {
            self.gate.set_mask(muted, &mut writes);
        }
        for (port, addr, data) in writes {
            self.inner.write(port, addr, data);
        }
        // Forward the mask too: a no-op on the cores we wrap (they do not mute),
        // but correct if a future gated core also has native mute. The A/B
        // harness turns this off to isolate the gate's own effect.
        if self.forward_mask {
            self.inner.set_channel_mutes(muted);
        }
    }

    fn set_channel_pans(&mut self, pans: &[i16]) {
        self.inner.set_channel_pans(pans);
    }

    fn supports_pan(&self) -> bool {
        self.inner.supports_pan()
    }

    fn render(&mut self, out: &mut [i32]) {
        self.inner.render(out);
    }
}

/// Every core this build can offer, in priority order per chip.
#[derive(Debug, Default)]
pub struct CoreRegistry {
    entries: Vec<CoreInfo>,
}

impl CoreRegistry {
    /// An empty registry. Only useful to a test that wants to control the whole
    /// list; the app starts from [`with_builtins`](Self::with_builtins).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// This crate's own cores: the permissively-licensed ones, and the OPL
    /// entries the app routes.
    ///
    /// The OPL row is registered here despite being routed elsewhere, because
    /// the *list* of what OPL can play through belongs with every other such
    /// list. RetroWave is not here: it is native-only, so `vgms-retrowave`
    /// registers it and a wasm build simply lacks the entry.
    #[must_use]
    pub fn with_builtins() -> Self {
        // `mut` is idle without the OPL feature: the OPL row is the only
        // registration left in here.
        #[cfg_attr(not(feature = "nuked-opl"), allow(unused_mut))]
        let mut registry = Self::new();
        // Absent from a `--no-default-features` build, because Nuked-OPL3 is
        // the LGPL dependency that build exists to drop. The UI then reports
        // OPL as having no core, which is exactly true: OPL documents still
        // open, edit and seek, they just render silence.
        #[cfg(feature = "nuked-opl")]
        for chip in OPL_CHIPS {
            registry.register(CoreInfo {
                id: NUKED_OPL_ID,
                chip,
                label: "Nuked OPL3 (emulated)",
                authors: "Nuke.YKT; Rust port by the nuked-opl3 crate authors",
                license: "LGPL-2.1-or-later",
                upstream: "https://github.com/nukeykt/Nuked-OPL3",
                realtime: true,
                // The OPL3's per-channel pan is the `stereo-ext` register path
                // `DroEngine` drives (not the `ChipCore` mute/pan API); CQM
                // and the RetroWave board cannot, and keep `false`.
                channel_pan: true,
                // On the VgmEngine path the OPL adapter mutes through the write
                // gate (the OPL `ChannelGate` rows), so `build` wraps it in a
                // `GatedCore` -- `false` engages that. OPL *documents* still mute
                // through `DroEngine`'s own register gating, untouched by this.
                channel_mute: false,
                level: LEVEL_UNITY,
                make: CoreMaker::Opl(|rate| Box::new(crate::opl::NukedOpl3::new(rate))),
            });
        }
        // Every non-OPL chip is served by provider crates -- vgms-cores-libvgm
        // first and foremost -- registered by the application. A build that
        // registers no provider simply has no generic cores, and the UI
        // reports exactly that.
        registry
    }

    /// Appends a core. Later registrations rank lower for their chip, so a
    /// provider that wants to be the default must register before the builtins.
    pub fn register(&mut self, info: CoreInfo) {
        self.entries.push(info);
    }

    /// Makes the row with `id` the default for `chip`, leaving every other
    /// chip's order alone.
    ///
    /// The owner's per-chip override on top of registration order: provider
    /// crates register wholesale, and sometimes one chip's default should
    /// come from a *later* provider without dragging its crate-mates forward
    /// -- Nuked-OPLL leads the YM2413 while Nuked-PSG and the LLE dies, from
    /// the same crate, stay behind libvgm. Returns whether the row was found;
    /// a `false` is a normal build difference (the web build lacks native
    /// providers), not an error.
    pub fn promote(&mut self, chip: ChipKind, id: &str) -> bool {
        let Some(from) = self
            .entries
            .iter()
            .position(|info| info.chip == chip && info.id == id)
        else {
            return false;
        };
        let Some(first) = self.entries.iter().position(|info| info.chip == chip) else {
            unreachable!("the row at `from` is itself a row for `chip`");
        };
        if from != first {
            let row = self.entries.remove(from);
            self.entries.insert(first, row);
        }
        true
    }

    /// Every core, in registration order.
    pub fn all(&self) -> impl Iterator<Item = &CoreInfo> {
        self.entries.iter()
    }

    /// The cores serving `chip`, best first.
    pub fn for_chip(&self, chip: ChipKind) -> impl Iterator<Item = &CoreInfo> {
        self.entries.iter().filter(move |info| info.chip == chip)
    }

    /// The core `chip` gets when the user has expressed no preference.
    #[must_use]
    pub fn default_for(&self, chip: ChipKind) -> Option<&CoreInfo> {
        self.for_chip(chip).next()
    }

    /// The core with this id, for this chip.
    #[must_use]
    pub fn find(&self, chip: ChipKind, id: &str) -> Option<&CoreInfo> {
        self.for_chip(chip).find(|info| info.id == id)
    }

    /// The core to use for `chip`, honouring a configured `id`.
    ///
    /// An id that no longer exists -- a config written by a native build and
    /// read by the web one, or a provider dropped from a release -- falls back
    /// to priority order rather than to silence. Never fatal: a settings file
    /// naming a core this build lacks is a normal thing, not a corrupt file.
    #[must_use]
    pub fn resolve(&self, chip: ChipKind, id: Option<&str>) -> Option<&CoreInfo> {
        match id {
            Some(id) => self.find(chip, id).or_else(|| {
                log::debug!("no core {id:?} for {}; using the default", chip.name());
                self.default_for(chip)
            }),
            None => self.default_for(chip),
        }
    }

    /// Whether any core is *listed* for `chip`, routed ones included.
    ///
    /// The question the Settings picker and the About credits ask: what is
    /// there to show. Not the question an engine asks -- see
    /// [`can_build`](Self::can_build), and mind the difference.
    #[must_use]
    pub fn has_core(&self, chip: ChipKind) -> bool {
        self.for_chip(chip).next().is_some()
    }

    /// Whether `VgmEngine` can be handed a core for `chip`.
    ///
    /// Stricter than [`has_core`](Self::has_core), and deliberately so: a
    /// [`Routed`](CoreMaker::Routed) core (RetroWave hardware) is listed and
    /// selectable but is a whole audio *service*, not a [`ChipCore`] the engine
    /// pulls samples from -- treating "listed" as "playable here" would report a
    /// file playable that would render silence. OPL *is* playable here since
    /// ou-1: [`CoreInfo::build`] hosts a [`CoreMaker::Opl`] through the
    /// [`OplCoreAdapter`], so every non-routed maker counts.
    #[must_use]
    pub fn can_build(&self, chip: ChipKind) -> bool {
        self.for_chip(chip)
            .any(|info| !matches!(info.make, CoreMaker::Routed))
    }

    /// The core for `chip` given what the config stores: the short name, not
    /// the full id.
    ///
    /// `vgmstudio.ini` says `core.opl3=cqm`, because repeating the slot in the
    /// value (`opl3.cqm`) is noise in a file a person edits. The id keeps the
    /// prefix, because ids are unique across the whole registry and the About
    /// box lists them side by side. This is the one place that knows both.
    #[must_use]
    pub fn resolve_choice(&self, chip: ChipKind, choice: Option<&str>) -> Option<&CoreInfo> {
        let id = choice.map(|choice| format!("{}.{}", slot_slug(chip), choice));
        self.resolve(chip, id.as_deref())
    }

    /// Builds the generic core for `chip`, honouring a configured id.
    ///
    /// `None` for a chip with no core *and* for one whose core is routed: both
    /// mean "`VgmEngine` cannot drive this", which is the only question this
    /// answers.
    #[must_use]
    pub fn build(&self, chip: ChipKind, id: Option<&str>) -> Option<Box<dyn ChipCore>> {
        self.resolve(chip, id)?.build()
    }

    /// Builds the generic core for `kind` honouring an explicit per-render
    /// [`CoreChoices`] map, resolved *offline* -- a non-realtime LLE core is a
    /// legitimate render pick even though the transport could not play it, which
    /// is the whole point of a per-render override.
    ///
    /// A slot the map does not name falls back to the registry default, so an
    /// empty map builds exactly the registry's defaults. `None` for a chip with
    /// no generic core or a routed one -- the same contract as
    /// [`build`](Self::build). Goes through [`CoreInfo::build`], so `Leveled`
    /// (and, later, the channel gate) apply exactly as on any other path.
    ///
    /// The A/B harness and the split's ungated-chip guard pin a specific core
    /// this way; the live render/split honour the map through
    /// [`with_render_choices`] instead, so a whole render tree agrees without
    /// threading the map through every signature.
    #[must_use]
    pub fn build_with(&self, choices: &CoreChoices, kind: ChipKind) -> Option<Box<dyn ChipCore>> {
        self.resolve_choice(kind, choices.get(slot_slug(kind)).map(String::as_str))?
            .build()
    }

    /// As [`resolve_choice`](Self::resolve_choice), but never a core that
    /// cannot keep up with playback.
    ///
    /// The transport's half of the [`CoreInfo::realtime`] split: an offline
    /// tier core (the LLE die sims) may be *chosen*, and the WAV render
    /// honours that choice, but live playback substituting it would underrun
    /// the audio callback -- so the transport falls back to the chip's best
    /// realtime core instead, exactly as it falls back from a core this build
    /// does not have.
    #[must_use]
    pub fn resolve_choice_realtime(
        &self,
        chip: ChipKind,
        choice: Option<&str>,
    ) -> Option<&CoreInfo> {
        let resolved = self.resolve_choice(chip, choice)?;
        if resolved.realtime {
            return Some(resolved);
        }
        log::debug!(
            "{} is offline-only; live playback uses {}'s realtime default instead",
            resolved.id,
            chip.name()
        );
        self.for_chip(chip)
            .find(|info| info.realtime && matches!(info.make, CoreMaker::Generic(_)))
    }

    /// Whether the core live playback would actually use for `chip` can place
    /// individual channels in the stereo image.
    ///
    /// Asks the *resolved* choice -- the user's pick, through the realtime
    /// fallback -- because that is the core whose knobs the UI would be
    /// drawing. `false` for a chip with no core at all.
    #[must_use]
    pub fn pan_capable(&self, chip: ChipKind) -> bool {
        self.resolve_choice_realtime(chip, core_choice(chip).as_deref())
            .is_some_and(|info| info.channel_pan)
    }

    /// Whether the core the transport would build for `chip` honours per-channel
    /// muting -- either natively, or through the write-gate the build wraps it in.
    ///
    /// Mirrors [`pan_capable`](Self::pan_capable): the resolved realtime core is
    /// the one whose mute toggles the UI would draw. A generic core with no native
    /// mute is muteable all the same when the gate covers its chip, because
    /// [`CoreInfo::build`] wraps it in a [`ChannelGate`] -- so the UI enables the
    /// toggles for exactly the cores [`build`](CoreInfo::build) makes muteable.
    /// `false` for a chip with no core at all.
    #[must_use]
    pub fn mute_capable(&self, chip: ChipKind) -> bool {
        self.resolve_choice_realtime(chip, core_choice(chip).as_deref())
            .is_some_and(|info| {
                // Mirror `build`: it wraps ANY buildable maker (Generic or the OPL
                // adapter) in a gate when the chip has a table and no native mute.
                // Only a Routed core (RetroWave) has no `ChipCore` to gate at all.
                info.channel_mute
                    || (ChannelGate::exists(chip) && !matches!(info.make, CoreMaker::Routed))
            })
    }
}

/// The chips one OPL selector governs.
///
/// An OPL3 *is* an OPL2 with more of it, and the YM3526 and Y8950 are the same
/// register file again -- one core (or one board) plays all four, so they share
/// a row and a config key. [`opl_slot_slug`] is that key.
pub const OPL_CHIPS: [ChipKind; 4] = [
    ChipKind::Ymf262,
    ChipKind::Ym3812,
    ChipKind::Ym3526,
    ChipKind::Y8950,
];

/// The id of the built-in emulated OPL core.
pub const NUKED_OPL_ID: &str = "opl3.nuked";

/// The config slug the OPL family shares, rather than one key per OPL chip.
pub const OPL_SLOT_SLUG: &str = "opl3";

/// Whether `chip` is governed by the single OPL selector.
#[must_use]
pub fn is_opl(chip: ChipKind) -> bool {
    OPL_CHIPS.contains(&chip)
}

/// The config slug that decides `chip`'s core: its own, or `"opl3"` for any of
/// the OPL family, which share one selector.
#[must_use]
pub fn slot_slug(chip: ChipKind) -> &'static str {
    if is_opl(chip) {
        OPL_SLOT_SLUG
    } else {
        chip.slug()
    }
}

/// A per-render set of core choices, `slot slug -> short name` -- the same key
/// space as [`set_core_choices`]'s map and `AudioConfig.cores`, so one is seeded
/// from the other with a plain clone.
///
/// Where [`set_core_choices`] sets the *process-wide* choice all of playback,
/// the render, the waveform and the peak scan read, a `CoreChoices` is a
/// *one-shot* override a single render or split carries -- picked in a dialog,
/// never written to `vgmstudio.ini`, and gone when the job ends. Applied through
/// [`with_render_choices`] (the whole render) or [`CoreRegistry::build_with`] (a
/// pinned single core).
pub type CoreChoices = std::collections::BTreeMap<String, String>;

/// The process-wide registry, installed once at startup.
static INSTALLED: std::sync::OnceLock<CoreRegistry> = std::sync::OnceLock::new();

/// The user's per-slot core choices, `slot slug -> short name` -- the map
/// `vgmstudio.ini`'s `core.<slug>=<name>` lines populate.
///
/// Lives beside the registry rather than being threaded through every engine
/// constructor, because the choice is process-wide state exactly as the
/// registry is: playback, the WAV render, the waveform and the peak scan must
/// all agree on which core a chip means *right now*, and passing the map
/// through each of their signatures would say the same thing five ways. The
/// app rewrites it whenever the config changes (startup, Settings apply); a
/// `RwLock` because the audio thread reads it while the GUI thread writes.
static CHOICES: std::sync::RwLock<std::collections::BTreeMap<String, String>> =
    std::sync::RwLock::new(std::collections::BTreeMap::new());

/// Replaces the per-slot core choices with `choices` (the config's
/// `audio.cores` map). Engines built afterwards honour them; engines already
/// built keep the cores they were born with, which is why the app reloads its
/// stream when a choice changes.
pub fn set_core_choices(choices: std::collections::BTreeMap<String, String>) {
    *CHOICES.write().expect("not poisoned") = choices;
}

/// The configured short name for `chip`'s slot, if the user chose one.
#[must_use]
pub fn core_choice(chip: ChipKind) -> Option<String> {
    CHOICES
        .read()
        .expect("not poisoned")
        .get(slot_slug(chip))
        .cloned()
}

thread_local! {
    /// A render's one-shot core override, active only for the thread the render
    /// runs on. Renders run on their own thread (native) or Web Worker (web), so
    /// this never leaks into playback, which reads the process-wide [`CHOICES`]
    /// on its own thread.
    static RENDER_OVERRIDE: std::cell::RefCell<Option<CoreChoices>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `f` with `choices` overriding the process-wide core choices, for the
/// current thread only, then restores whatever was there before (even on a
/// panic).
///
/// This is how a render or split honours a per-render [`CoreChoices`] pick
/// without disturbing what playback reads: [`core_for`](crate::chip::core_for)
/// (and thus the VGM render) and the OPL render's chip selection consult
/// [`render_override`] while `f` runs. `None` restores the plain behaviour, so a
/// caller with no override can wrap unconditionally.
pub fn with_render_choices<R>(choices: Option<CoreChoices>, f: impl FnOnce() -> R) -> R {
    /// Restores the previous override on drop, so an early return or a panic in
    /// `f` cannot leave the override set for the next job on this thread.
    struct Restore(Option<CoreChoices>);
    impl Drop for Restore {
        fn drop(&mut self) {
            RENDER_OVERRIDE.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }

    let _restore = Restore(RENDER_OVERRIDE.with(|slot| slot.replace(choices)));
    f()
}

/// The per-render override for `chip`'s slot, set by [`with_render_choices`] on
/// this thread, or `None` when no render override is active.
///
/// The OPL render reads *this* (never the process-wide choice) to decide whether
/// to build a non-default chip, so a plain render stays byte-for-byte what it
/// always was; the generic render layers it over [`core_choice`] inside
/// [`core_for`](crate::chip::core_for).
#[must_use]
pub fn render_override(chip: ChipKind) -> Option<String> {
    RENDER_OVERRIDE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|choices| choices.get(slot_slug(chip)).cloned())
    })
}

/// Installs the registry the whole program reads.
///
/// Call once, from the app's startup, after every provider crate has had its
/// `register` called. Returns the registry back if one is already installed,
/// which only happens if startup ran twice.
///
/// # Errors
/// The registry passed in, when one is already installed.
pub fn install(registry: CoreRegistry) -> Result<(), CoreRegistry> {
    INSTALLED.set(registry)
}

/// The registry in force.
///
/// Falls back to [`CoreRegistry::with_builtins`] when nothing was installed, so
/// this crate's own tests and any library consumer that never calls
/// [`install`] still see the built-in cores rather than an empty world.
#[must_use]
pub fn registry() -> &'static CoreRegistry {
    static FALLBACK: std::sync::OnceLock<CoreRegistry> = std::sync::OnceLock::new();
    INSTALLED
        .get()
        .unwrap_or_else(|| FALLBACK.get_or_init(CoreRegistry::with_builtins))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "nuked-opl")]
    #[test]
    fn the_builtins_cover_what_this_build_can_play() {
        let registry = CoreRegistry::with_builtins();
        assert!(
            registry.has_core(ChipKind::Ymf262),
            "OPL is listed, not built"
        );
        // Every generic core comes from a provider crate; the builtins carry
        // only the OPL entry.
        assert!(!registry.has_core(ChipKind::Sn76489), "providers only");
        assert!(!registry.has_core(ChipKind::Ym2612), "providers only");
        // OPL now builds as a ChipCore too (ou-1): the adapter lets VgmEngine
        // host it.
        assert!(registry.build(ChipKind::Ymf262, None).is_some());
    }

    /// The distinction that used to catch OPL now catches the genuinely-routed
    /// cores: a [`CoreMaker::Routed`] entry (RetroWave hardware) is *listed* for
    /// the Settings picker but is not a [`ChipCore`] `VgmEngine` can be handed,
    /// because it is a whole audio *service*, not something the engine pulls
    /// samples from. OPL, once in that camp, is now buildable through the ou-1
    /// adapter -- so it must answer `can_build` like any played chip.
    #[cfg(feature = "nuked-opl")]
    #[test]
    fn listed_and_buildable_are_different_questions() {
        let mut registry = CoreRegistry::with_builtins();
        // A provider-style generic core, so the buildable half of the
        // distinction is still exercised now that the builtins carry none.
        registry.register(tone_info("sn76489.stub", LEVEL_UNITY));
        for chip in OPL_CHIPS {
            assert!(registry.has_core(chip), "{} is listed", chip.name());
            assert!(
                registry.can_build(chip),
                "{} now reaches VgmEngine through the OPL adapter",
                chip.name()
            );
        }
        assert!(registry.can_build(ChipKind::Sn76489));
        assert!(!registry.can_build(ChipKind::Ym2612), "no core registered");
    }

    #[cfg(feature = "nuked-opl")]
    #[test]
    fn every_opl_chip_shares_one_selector() {
        let registry = CoreRegistry::with_builtins();
        for chip in OPL_CHIPS {
            assert_eq!(slot_slug(chip), OPL_SLOT_SLUG, "{}", chip.name());
            assert_eq!(
                registry.default_for(chip).map(|info| info.id),
                Some(NUKED_OPL_ID),
                "{} plays through the OPL core",
                chip.name()
            );
        }
        // Everything else keeps its own key.
        assert_eq!(slot_slug(ChipKind::Sn76489), "sn76489");
    }

    /// `promote` moves one row to its chip's front, leaves every other chip
    /// alone, and tolerates ids this build lacks -- the owner's per-chip
    /// override on top of registration order.
    #[test]
    fn promote_makes_a_later_row_the_default_without_touching_neighbours() {
        let mut registry = CoreRegistry::new();
        registry.register(info("sn76489.first"));
        registry.register(info("sn76489.second"));
        registry.register(CoreInfo {
            chip: ChipKind::Ym2612,
            ..tone_info("ym2612.only", LEVEL_UNITY)
        });

        assert!(registry.promote(ChipKind::Sn76489, "sn76489.second"));
        assert_eq!(
            registry.default_for(ChipKind::Sn76489).map(|i| i.id),
            Some("sn76489.second")
        );
        // Both rows survive -- the loser is demoted, not dropped.
        assert_eq!(registry.for_chip(ChipKind::Sn76489).count(), 2);
        // The neighbour chip is untouched.
        assert_eq!(
            registry.default_for(ChipKind::Ym2612).map(|i| i.id),
            Some("ym2612.only")
        );

        // An id this build lacks is a normal difference, not an error, and
        // changes nothing.
        assert!(!registry.promote(ChipKind::Sn76489, "sn76489.nonesuch"));
        assert_eq!(
            registry.default_for(ChipKind::Sn76489).map(|i| i.id),
            Some("sn76489.second")
        );

        // Promoting the sitting default is a found no-op.
        assert!(registry.promote(ChipKind::Sn76489, "sn76489.second"));
        assert_eq!(
            registry.default_for(ChipKind::Sn76489).map(|i| i.id),
            Some("sn76489.second")
        );
    }

    #[test]
    fn registration_order_is_priority_order() {
        let mut registry = CoreRegistry::new();
        registry.register(info("sn76489.first"));
        registry.register(info("sn76489.second"));
        assert_eq!(
            registry.default_for(ChipKind::Sn76489).map(|i| i.id),
            Some("sn76489.first")
        );
        assert_eq!(registry.for_chip(ChipKind::Sn76489).count(), 2);
    }

    #[test]
    fn a_configured_id_wins_and_an_unknown_one_falls_back() {
        let mut registry = CoreRegistry::new();
        registry.register(info("sn76489.first"));
        registry.register(info("sn76489.second"));

        let picked = registry.resolve(ChipKind::Sn76489, Some("sn76489.second"));
        assert_eq!(picked.map(|i| i.id), Some("sn76489.second"));

        // The web build genuinely lacks ids the native one has. Falling back
        // beats refusing to play, and beats a hard error in a settings file.
        let missing = registry.resolve(ChipKind::Sn76489, Some("sn76489.nonesuch"));
        assert_eq!(missing.map(|i| i.id), Some("sn76489.first"));

        // A chip with no cores at all still has no core.
        assert!(
            registry
                .resolve(ChipKind::Ym2612, Some("anything"))
                .is_none()
        );
    }

    #[cfg(feature = "nuked-opl")]
    #[test]
    fn the_ambient_registry_has_the_builtins_when_nothing_was_installed() {
        assert!(registry().has_core(ChipKind::Ymf262));
    }

    /// `vgms-core` writes `core.opl3=` without being able to see this crate --
    /// it sits below it. The two spellings of the OPL slot must agree or a
    /// user's saved choice lands in a slot nothing reads.
    #[test]
    fn the_config_and_the_registry_agree_on_the_opl_slot() {
        assert_eq!(OPL_SLOT_SLUG, vgms_core::config::OPL_SLOT);
        assert_eq!(
            NUKED_OPL_ID,
            format!("{}.{}", OPL_SLOT_SLUG, vgms_core::config::NUKED_CORE)
        );
    }

    /// The config stores `core.opl3=nuked`; the registry keys on
    /// `"opl3.nuked"`. A slot-prefixed id is what makes composing the two
    /// unambiguous, and getting it wrong would silently fall back to the
    /// default -- audible only as "my setting keeps resetting".
    #[cfg(feature = "nuked-opl")]
    #[test]
    fn a_config_choice_resolves_to_the_core_it_names() {
        let registry = CoreRegistry::with_builtins();
        let chosen = registry.resolve_choice(ChipKind::Ymf262, Some(vgms_core::config::NUKED_CORE));
        assert_eq!(chosen.map(|info| info.id), Some(NUKED_OPL_ID));

        // A name this build lacks -- "retrowave" on a wasm build, say -- takes
        // the default rather than falling silent.
        let missing = registry.resolve_choice(ChipKind::Ymf262, Some("retrowave"));
        assert_eq!(missing.map(|info| info.id), Some(NUKED_OPL_ID));
    }

    #[test]
    fn every_core_id_is_namespaced_by_its_chip_slot() {
        // An id must be unique across the whole registry, since config stores it
        // per slot and the About box lists them all. Prefixing with the slot
        // slug is what makes that true without a central list to check against.
        for info in CoreRegistry::with_builtins().all() {
            let prefix = format!("{}.", slot_slug(info.chip));
            assert!(
                info.id.starts_with(&prefix),
                "{} should start with {prefix}",
                info.id
            );
        }
    }

    /// A core that renders a constant, so a gain is visible in the output.
    #[derive(Debug, Default)]
    struct Tone {
        writes: usize,
        resets: usize,
    }

    impl ChipCore for Tone {
        fn reset(&mut self, _clock: u32, _variant: bool) {
            self.resets += 1;
        }
        fn native_rate(&self) -> u32 {
            44_100
        }
        fn write(&mut self, _port: u8, _addr: u16, _data: u16) {
            self.writes += 1;
        }
        fn render(&mut self, out: &mut [i32]) {
            out.fill(1000);
        }
    }

    fn tone_info(id: &'static str, level: u16) -> CoreInfo {
        CoreInfo {
            id,
            chip: ChipKind::Sn76489,
            label: id,
            authors: "test",
            license: "MIT",
            upstream: "",
            realtime: true,
            channel_pan: false,
            channel_mute: false,
            level,
            make: CoreMaker::Generic(|| Box::new(Tone::default())),
        }
    }

    fn rendered(info: &CoreInfo) -> Vec<i32> {
        let mut core = info.build().expect("a generic core builds");
        let mut out = vec![0i32; 8];
        core.render(&mut out);
        out
    }

    /// The transport's half of the `realtime` split: a chosen offline-tier
    /// core resolves to the chip's best realtime one instead, while the
    /// choice-honouring resolver keeps it -- the WAV render's half.
    #[test]
    fn an_offline_choice_falls_back_to_realtime_for_the_transport_only() {
        let mut registry = CoreRegistry::new();
        registry.register(tone_info("sn76489.fast", LEVEL_UNITY));
        registry.register(CoreInfo {
            realtime: false,
            channel_pan: false,
            ..tone_info("sn76489.die", LEVEL_UNITY)
        });

        // The offline render honours the choice as made...
        assert_eq!(
            registry
                .resolve_choice(ChipKind::Sn76489, Some("die"))
                .map(|info| info.id),
            Some("sn76489.die")
        );
        // ...the transport substitutes the realtime default...
        assert_eq!(
            registry
                .resolve_choice_realtime(ChipKind::Sn76489, Some("die"))
                .map(|info| info.id),
            Some("sn76489.fast")
        );
        // ...and a realtime choice passes through untouched.
        assert_eq!(
            registry
                .resolve_choice_realtime(ChipKind::Sn76489, Some("fast"))
                .map(|info| info.id),
            Some("sn76489.fast")
        );
    }

    /// A core's level scales what it renders, and unity leaves it alone.
    ///
    /// The point of the whole field: two cores for one chip need not agree on
    /// how loud that chip is, and the registry is where that is recorded.
    #[test]
    fn a_cores_level_scales_what_it_renders() {
        assert_eq!(
            rendered(&tone_info("sn76489.plain", LEVEL_UNITY)),
            [1000; 8]
        );
        // 1.930, the gain the parity harness measured for libvgm's K053260.
        assert_eq!(rendered(&tone_info("sn76489.loud", 494)), [1929; 8]);
        // And a level below unity attenuates rather than being ignored.
        assert_eq!(rendered(&tone_info("sn76489.quiet", 128)), [500; 8]);
    }

    /// The wrapper forwards everything else untouched.
    ///
    /// A gain adapter that swallowed a register write would be the worst kind
    /// of bug here: the chip would play, quietly, and wrongly.
    #[test]
    fn the_level_wrapper_forwards_the_rest_of_the_trait() {
        let info = tone_info("sn76489.loud", 494);
        let mut core = info.build().expect("builds");
        core.reset(3_579_545, true);
        core.configure(&vgms_core::vgm::ChipSettings::default());
        core.write(1, 0x28, 0xF0);
        core.load_rom(0x8E, 16, 0, &[0u8; 16]);
        core.write_ram(0, &[0u8; 4]);
        core.write_ram_absolute(0, &[0u8; 4]);
        assert_eq!(core.native_rate(), 44_100, "the rate is the inner core's");

        // The scaling still happened, so the wrapper is genuinely interposed
        // and these calls really did pass through it.
        let mut out = vec![0i32; 4];
        core.render(&mut out);
        assert_eq!(out, [1929; 4]);
    }

    /// A loud sample times a gain above unity saturates rather than wrapping.
    ///
    /// `i32::MAX * 1.93` does not fit in an `i32`, and a truncating cast would
    /// turn the loudest possible sample into a negative one -- an inaudible
    /// arithmetic detail that would be an audible click.
    #[test]
    fn scaling_saturates_instead_of_wrapping() {
        #[derive(Debug, Default)]
        struct Full;
        impl ChipCore for Full {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn render(&mut self, out: &mut [i32]) {
                out.fill(i32::MAX);
            }
        }

        let mut registry = CoreRegistry::new();
        registry.register(CoreInfo {
            make: CoreMaker::Generic(|| Box::new(Full)),
            ..tone_info("sn76489.full", 494)
        });
        let mut core = registry.build(ChipKind::Sn76489, None).expect("builds");
        let mut out = vec![0i32; 4];
        core.render(&mut out);
        assert_eq!(out, [i32::MAX; 4], "saturated, not wrapped");
    }

    fn info(id: &'static str) -> CoreInfo {
        tone_info(id, LEVEL_UNITY)
    }

    fn render8(core: Option<Box<dyn ChipCore>>) -> Vec<i32> {
        let mut core = core.expect("a generic core builds");
        let mut out = vec![0i32; 8];
        core.render(&mut out);
        out
    }

    /// `build_with` resolves a chip's core from an explicit per-render map:
    /// naming a core picks it, an unnamed slot takes the registry default, and
    /// the level wrapper is applied on the way -- so a per-render core is
    /// indistinguishable from a settings-chosen one below the registry.
    #[test]
    fn build_with_honours_the_render_choices_map() {
        let mut registry = CoreRegistry::new();
        // Registered first, so the default; renders at unity.
        registry.register(tone_info("sn76489.plain", LEVEL_UNITY));
        // A louder alternative, so which core built is visible in the output.
        registry.register(tone_info("sn76489.loud", 494));

        // An empty map builds the registry default.
        assert_eq!(
            render8(registry.build_with(&CoreChoices::new(), ChipKind::Sn76489)),
            [1000; 8],
            "an unnamed slot takes the default"
        );

        // Naming the alternative builds it -- keyed by the slot slug, the same
        // key space Settings uses.
        let choices = CoreChoices::from([("sn76489".to_owned(), "loud".to_owned())]);
        assert_eq!(
            render8(registry.build_with(&choices, ChipKind::Sn76489)),
            [1929; 8],
            "the named core is built, and its level applied"
        );

        // A chip with no generic core is still None, like `build`.
        assert!(
            registry
                .build_with(&CoreChoices::new(), ChipKind::Ym2612)
                .is_none()
        );
    }

    /// A render override is visible only inside its closure, on its own thread,
    /// and nesting restores the outer one -- so a render honours its pick without
    /// leaking it into the next job on the same worker thread.
    #[test]
    fn a_render_override_is_scoped_and_restored() {
        assert_eq!(render_override(ChipKind::Sn76489), None, "none to start");

        let choices = CoreChoices::from([("sn76489".to_owned(), "loud".to_owned())]);
        let seen = with_render_choices(Some(choices), || render_override(ChipKind::Sn76489));
        assert_eq!(seen.as_deref(), Some("loud"), "seen inside the closure");
        assert_eq!(
            render_override(ChipKind::Sn76489),
            None,
            "and gone once it returns"
        );

        // A nested override restores the outer one, not the empty state.
        let outer = CoreChoices::from([("sn76489".to_owned(), "outer".to_owned())]);
        with_render_choices(Some(outer), || {
            assert_eq!(render_override(ChipKind::Sn76489).as_deref(), Some("outer"));
            let inner = CoreChoices::from([("sn76489".to_owned(), "inner".to_owned())]);
            with_render_choices(Some(inner), || {
                assert_eq!(render_override(ChipKind::Sn76489).as_deref(), Some("inner"));
            });
            assert_eq!(
                render_override(ChipKind::Sn76489).as_deref(),
                Some("outer"),
                "outer override restored after the inner one"
            );
        });
        assert_eq!(render_override(ChipKind::Sn76489), None);
    }

    // -- GatedCore (pm-3) ----------------------------------------------------

    /// A test core that records what reached it through a shared handle, so a
    /// test can see what the gate wrapper let through, dropped or synthesised.
    #[derive(Default)]
    struct Recorder {
        writes: Vec<(u8, u16, u16)>,
        mutes: Vec<u32>,
        resets: usize,
    }

    #[derive(Clone, Default)]
    struct Shared(std::sync::Arc<std::sync::Mutex<Recorder>>);

    impl ChipCore for Shared {
        fn reset(&mut self, _clock: u32, _variant: bool) {
            self.0.lock().expect("not poisoned").resets += 1;
        }
        fn native_rate(&self) -> u32 {
            44_100
        }
        fn write(&mut self, port: u8, addr: u16, data: u16) {
            self.0
                .lock()
                .expect("not poisoned")
                .writes
                .push((port, addr, data));
        }
        fn render(&mut self, out: &mut [i32]) {
            out.fill(0);
        }
        fn set_channel_mutes(&mut self, muted: u32) {
            self.0.lock().expect("not poisoned").mutes.push(muted);
        }
    }

    /// The gate's verdict reaches the inner core: a muted channel's key-on is
    /// dropped, an audible one's passes, and muting emits the edge key-off and
    /// still forwards the mask to the inner core.
    #[test]
    fn gated_core_applies_the_gates_verdict() {
        let shared = Shared::default();
        let handle = shared.0.clone();
        let mut core = GatedCore::wrap(Box::new(shared), ChipKind::Ym2151);

        core.set_channel_mutes(0b0000_0100); // mute channel 2
        core.write(0, 0x08, 0x78 | 0x02); // channel 2 key-on -> dropped
        core.write(0, 0x08, 0x78 | 0x03); // channel 3 key-on -> passed

        let recorder = handle.lock().expect("not poisoned");
        assert_eq!(
            recorder.writes,
            [(0, 0x08, 0x02), (0, 0x08, 0x78 | 0x03)],
            "the mute-edge key-off, then only channel 3's key-on"
        );
        assert_eq!(recorder.mutes, [0b0000_0100], "the mask is forwarded too");
    }

    /// A whole-chip mute stands the gate down: no writes are synthesised (the
    /// engine's own silence takes over) and later writes pass untouched. Leaving
    /// the stand-down re-asserts, so a channel still muted is keyed off.
    #[test]
    fn gated_core_stands_down_for_a_whole_chip_mute() {
        let shared = Shared::default();
        let handle = shared.0.clone();
        let mut core = GatedCore::wrap(Box::new(shared), ChipKind::Ym2151); // 8 channels

        core.set_channel_mutes(0xFF); // whole chip
        core.write(0, 0x08, 0x78 | 0x02); // passes untouched while standing down
        {
            let recorder = handle.lock().expect("not poisoned");
            assert_eq!(
                recorder.writes,
                [(0, 0x08, 0x78 | 0x02)],
                "no synthesised writes; the song's write passes through"
            );
        }

        // Leaving the stand-down to a partial mask re-asserts: channel 2 (still
        // muted) is keyed off again.
        core.set_channel_mutes(0b0000_0100);
        let recorder = handle.lock().expect("not poisoned");
        assert!(
            recorder.writes.contains(&(0, 0x08, 0x02)),
            "leaving stand-down re-keys off the still-muted channel: {:?}",
            recorder.writes
        );
    }

    /// A spy [`OplChip`] logging every register write, so the hardware OPL
    /// core's gating can be inspected without an emulator.
    #[derive(Debug, Clone, Default)]
    struct SpyOpl(std::sync::Arc<std::sync::Mutex<Vec<(u16, u8)>>>);

    impl OplChip for SpyOpl {
        fn reset(&mut self, _sample_rate: u32) {}
        fn write_reg(&mut self, reg: u16, value: u8) {
            self.0.lock().expect("not poisoned").push((reg, value));
        }
        fn write_reg_buffered(&mut self, reg: u16, value: u8) {
            self.0.lock().expect("not poisoned").push((reg, value));
        }
        fn generate_samples(&mut self, buffer: &mut [i16]) {
            buffer.fill(0);
        }
    }

    /// The hardware OPL core does **not** stand down on a whole-chip mute: a
    /// real chip is the sound, so a full mask must clear every key at the
    /// register level, exactly as a partial mask does. This is the contract the
    /// RetroWave "everything muted stays silent" behaviour rests on.
    #[test]
    fn the_hardware_opl_core_gates_a_whole_chip_mute_at_the_register_level() {
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut core = opl_hardware_core(
            Box::new(SpyOpl(std::sync::Arc::clone(&log))),
            ChipKind::Ymf262,
        );

        // Mute every one of the OPL3's 23 channels: the whole chip.
        core.set_channel_mutes(u32::MAX);
        log.lock().expect("not poisoned").clear();

        // A key-on for melodic channel 0 must reach the chip with the key bit
        // cleared -- not passed through as a live note, which is what a
        // stood-down gate would have done.
        core.write(0, 0xB0, 0x31); // frequency-high + block + key-on
        let writes = log.lock().expect("not poisoned").clone();
        assert_eq!(
            writes,
            [(0xB0, 0x11)],
            "the whole-chip mute cleared the key bit rather than standing down"
        );
    }

    /// The same core still isolates one channel: a partial mute clears the muted
    /// channel's key and passes the rest, like every other gated OPL core.
    #[test]
    fn the_hardware_opl_core_still_isolates_one_channel() {
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut core = opl_hardware_core(
            Box::new(SpyOpl(std::sync::Arc::clone(&log))),
            ChipKind::Ymf262,
        );

        core.set_channel_mutes(0b1); // channel 0 only
        log.lock().expect("not poisoned").clear();

        core.write(0, 0xB0, 0x31); // muted channel -> key cleared
        core.write(0, 0xB1, 0x31); // audible channel -> passes
        let writes = log.lock().expect("not poisoned").clone();
        assert_eq!(writes, [(0xB0, 0x11), (0xB1, 0x31)]);
    }

    /// The rest of the trait forwards, and `reset` reaches both the gate and the
    /// inner core.
    #[test]
    fn gated_core_forwards_the_rest_of_the_trait() {
        let shared = Shared::default();
        let handle = shared.0.clone();
        let mut core = GatedCore::wrap(Box::new(shared), ChipKind::Sn76489);

        assert_eq!(
            core.native_rate(),
            44_100,
            "native_rate is the inner core's"
        );
        core.reset(3_579_545, false);
        let mut out = [1, 2, 3, 4];
        core.render(&mut out);
        assert_eq!(
            out,
            [0, 0, 0, 0],
            "render forwards to the (silent) inner core"
        );
        assert_eq!(
            handle.lock().expect("not poisoned").resets,
            1,
            "reset reached the inner core"
        );
    }

    // A thread-local write sink, so the `build` path (whose maker is a bare
    // `fn` pointer that cannot capture a handle) can still be observed.
    thread_local! {
        static BUILD_WRITES: std::cell::RefCell<Vec<(u8, u16, u16)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    #[derive(Debug)]
    struct ThreadTap;
    impl ChipCore for ThreadTap {
        fn reset(&mut self, _clock: u32, _variant: bool) {}
        fn native_rate(&self) -> u32 {
            44_100
        }
        fn write(&mut self, port: u8, addr: u16, data: u16) {
            BUILD_WRITES.with(|w| w.borrow_mut().push((port, addr, data)));
        }
        fn render(&mut self, out: &mut [i32]) {
            out.fill(0);
        }
    }

    fn tap_info(chip: ChipKind, channel_mute: bool) -> CoreInfo {
        CoreInfo {
            id: "tap.core",
            chip,
            label: "tap",
            authors: "test",
            license: "MIT",
            upstream: "",
            realtime: true,
            channel_pan: false,
            channel_mute,
            level: LEVEL_UNITY,
            make: CoreMaker::Generic(|| Box::new(ThreadTap)),
        }
    }

    /// `CoreInfo::build` engages the gate exactly for a `channel_mute: false` row
    /// whose chip the gate covers: a native-mute row and a chip with no table are
    /// left bare (muting them synthesises nothing).
    #[test]
    fn build_gates_only_a_covered_chip_without_native_mute() {
        let muted_channel = |chip, channel_mute| {
            BUILD_WRITES.with(|w| w.borrow_mut().clear());
            let mut core = tap_info(chip, channel_mute)
                .build()
                .expect("a generic core builds");
            core.set_channel_mutes(0b0000_0100); // mute channel 2
            BUILD_WRITES.with(|w| w.borrow().clone())
        };

        // Gated chip, no native mute -> wrapped: muting synthesises a key-off.
        assert_eq!(
            muted_channel(ChipKind::Ym2151, false),
            [(0, 0x08, 0x02)],
            "the gate is engaged"
        );
        // Same chip, but the row claims native mute -> not wrapped.
        assert!(
            muted_channel(ChipKind::Ym2151, true).is_empty(),
            "a native-mute row keeps its own path"
        );
        // A chip with no gate table -> not wrapped even without native mute.
        assert!(
            muted_channel(ChipKind::Scsp, false).is_empty(),
            "an ungated chip is left honestly un-muteable"
        );
    }

    /// `mute_capable` counts a generic core the gate covers, even with no native
    /// mute -- so the UI enables the toggles for exactly the cores `build` makes
    /// muteable (natively or through the gate).
    #[test]
    fn mute_capable_counts_a_gated_generic_core() {
        fn core(id: &'static str, chip: ChipKind, native_mute: bool) -> CoreInfo {
            CoreInfo {
                chip,
                channel_mute: native_mute,
                ..tone_info(id, LEVEL_UNITY)
            }
        }
        let mut registry = CoreRegistry::new();
        registry.register(core("ym2151.nuked", ChipKind::Ym2151, false)); // gated, no native mute
        registry.register(core("scsp.libvgm", ChipKind::Scsp, false)); // ungated, no native mute
        registry.register(core("ym2612.libvgm", ChipKind::Ym2612, true)); // native mute

        assert!(
            registry.mute_capable(ChipKind::Ym2151),
            "OPM has no native mute, but the gate covers it"
        );
        assert!(
            !registry.mute_capable(ChipKind::Scsp),
            "no native mute and no gate table -> not muteable"
        );
        assert!(
            registry.mute_capable(ChipKind::Ym2612),
            "a native-mute core is muteable as before"
        );
        assert!(
            !registry.mute_capable(ChipKind::C352),
            "a chip with no core at all is not muteable"
        );
    }
}
