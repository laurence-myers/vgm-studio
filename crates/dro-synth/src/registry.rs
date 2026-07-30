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
//! copyleft cores out of a permissively-licensed `dro-synth` (see
//! `licenses/README.md`) while letting the *application* link them, and it
//! means registration is explicit and ordered rather than link-time magic --
//! which matters on wasm, where a provider may simply not exist and the UI
//! should follow the registry rather than offer something absent.
//!
//! **Priority is registration order**, with one named escape hatch. The first
//! core registered for a chip is its default: the app registers
//! `dro-cores-libvgm` ahead of the other providers, so libvgm is the default
//! for every chip it serves and the Nuked and LLE integrations are the
//! picker's alternatives. OPL is the standing exception -- libvgm compiles no
//! OPL device, so the built-in Nuked-OPL3 row keeps that family -- and
//! [`CoreRegistry::promote`] is the owner's per-chip override for the cases
//! where one chip's default should come from a later provider without
//! dragging that provider's crate-mates forward (the app promotes Nuked back
//! over libvgm for the YM2612, YM2151 and YM2413).

use dro_core::vgm::ChipKind;

use crate::chip::ChipCore;
use crate::opl::OplChip;

/// How a registered core is brought into being.
///
/// Not every entry builds a [`ChipCore`]: the OPL family plays through
/// `PlayerEngine`, which carries register policy (muting, panning, the buffered
/// write spacing) that the generic trait has no place for, and RetroWave output
/// is a whole audio service in a native-only crate. Those entries exist to be
/// *named and chosen*; the app routes on their id.
pub enum CoreMaker {
    /// Built here and driven by `VgmEngine`.
    Generic(fn() -> Box<dyn ChipCore>),
    /// Built here and driven by `PlayerEngine`, which carries the OPL register
    /// policy the generic trait has no place for. Takes the sample rate,
    /// because an OPL core resamples to it rather than declaring its own.
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
    /// This is what lands in `drotrim.ini`, so it outlives any label change.
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
    /// This core's output calibration, in 8.8 fixed point
    /// ([`LEVEL_UNITY`] = 1.0). Applied to every sample it renders.
    ///
    /// The calibration belongs to the *core*, not the chip: **two cores for
    /// one chip need not agree on how loud that chip is.** 8.8 fixed point
    /// rather than a float because the reference expresses its own chip volumes
    /// that way (VGMPlay's `MulFixed8x8`), and because [`ChipCore`] forbids
    /// output that could differ across targets.
    ///
    /// **A number here is a measurement, not a preference.** It is the
    /// least-squares gain the parity harness reports against the pinned
    /// reference, meaningful only for a core whose correlation is high enough
    /// that a single scalar describes the difference. Leave it at
    /// [`LEVEL_UNITY`] until measured; an unmeasured guess is worse than none.
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
    #[must_use]
    pub fn build(&self) -> Option<Box<dyn ChipCore>> {
        match self.make {
            CoreMaker::Generic(make) => Some(Leveled::wrap(make(), self.level)),
            CoreMaker::Opl(_) | CoreMaker::Routed => None,
        }
    }

    /// Builds this core as an OPL chip, or `None` when it is not one.
    ///
    /// Separate from [`build`](Self::build) because the two engines are
    /// separate: `VgmEngine` pulls samples from a `ChipCore`, `PlayerEngine`
    /// drives an `OplChip` through muting, panning and buffered writes. A core
    /// answers to one or the other, never both.
    #[must_use]
    pub fn build_opl(&self, sample_rate: u32) -> Option<Box<dyn OplChip>> {
        match self.make {
            CoreMaker::Opl(make) => Some(make(sample_rate)),
            CoreMaker::Generic(_) | CoreMaker::Routed => None,
        }
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

    fn configure(&mut self, settings: &dro_core::vgm::ChipSettings) {
        self.inner.configure(settings);
    }

    fn native_rate(&self) -> u32 {
        self.inner.native_rate()
    }

    fn write(&mut self, port: u8, addr: u16, data: u16) {
        self.inner.write(port, addr, data);
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
    /// list. RetroWave is not here: it is native-only, so `dro-retrowave`
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
                // `PlayerEngine` drives (not the `ChipCore` mute/pan API); CQM
                // and the RetroWave board cannot, and keep `false`.
                channel_pan: true,
                level: LEVEL_UNITY,
                make: CoreMaker::Opl(|rate| Box::new(crate::opl::NukedOpl3::new(rate))),
            });
        }
        // Every non-OPL chip is served by provider crates -- dro-cores-libvgm
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
    /// Stricter than [`has_core`](Self::has_core), and deliberately so: the OPL
    /// family is listed but [`Routed`](CoreMaker::Routed), because OPL plays
    /// through `PlayerEngine` and its register policy. Treating "listed" as
    /// "playable here" would route an OPL file into the generic engine, which
    /// would render silence rather than fail.
    #[must_use]
    pub fn can_build(&self, chip: ChipKind) -> bool {
        self.for_chip(chip)
            .any(|info| matches!(info.make, CoreMaker::Generic(_)))
    }

    /// The core for `chip` given what the config stores: the short name, not
    /// the full id.
    ///
    /// `drotrim.ini` says `core.opl3=cqm`, because repeating the slot in the
    /// value (`opl3.cqm`) is noise in a file a person edits. The id keeps the
    /// prefix, because ids are unique across the whole registry and the About
    /// box lists them side by side. This is the one place that knows both.
    #[must_use]
    pub fn resolve_choice(&self, chip: ChipKind, choice: Option<&str>) -> Option<&CoreInfo> {
        let id = choice.map(|choice| format!("{}.{}", slot_slug(chip), choice));
        self.resolve(chip, id.as_deref())
    }

    /// Builds the OPL core the config names, at `sample_rate`.
    ///
    /// `None` when the choice is a routed one (hardware) or this build has no
    /// OPL core at all -- both mean "`PlayerEngine` must fall back to its
    /// default chip", which is what the caller does.
    #[must_use]
    pub fn build_opl(&self, choice: Option<&str>, sample_rate: u32) -> Option<Box<dyn OplChip>> {
        self.resolve_choice(ChipKind::Ymf262, choice)?
            .build_opl(sample_rate)
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

/// The process-wide registry, installed once at startup.
static INSTALLED: std::sync::OnceLock<CoreRegistry> = std::sync::OnceLock::new();

/// The user's per-slot core choices, `slot slug -> short name` -- the map
/// `drotrim.ini`'s `core.<slug>=<name>` lines populate.
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
        // only the OPL entry the app routes.
        assert!(!registry.has_core(ChipKind::Sn76489), "providers only");
        assert!(!registry.has_core(ChipKind::Ym2612), "providers only");
        assert!(registry.build(ChipKind::Ymf262, None).is_none());
    }

    /// The distinction that would be a silent bug if it blurred: the OPL family
    /// is listed for the Settings picker but is *not* something `VgmEngine` can
    /// be handed, because OPL plays through `PlayerEngine`. Every caller of
    /// `playability` has already routed its OPL documents elsewhere, so an OPL
    /// chip counted as buildable would mean an OPL file rendering silence
    /// through the generic engine rather than failing visibly.
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
                !registry.can_build(chip),
                "{} must not reach VgmEngine",
                chip.name()
            );
        }
        assert!(registry.can_build(ChipKind::Sn76489));
        assert!(!registry.can_build(ChipKind::Ym2612));
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

    /// `dro-core` writes `core.opl3=` without being able to see this crate --
    /// it sits below it. The two spellings of the OPL slot must agree or a
    /// user's saved choice lands in a slot nothing reads.
    #[test]
    fn the_config_and_the_registry_agree_on_the_opl_slot() {
        assert_eq!(OPL_SLOT_SLUG, dro_core::config::OPL_SLOT);
        assert_eq!(
            NUKED_OPL_ID,
            format!("{}.{}", OPL_SLOT_SLUG, dro_core::config::NUKED_CORE)
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
        let chosen = registry.resolve_choice(ChipKind::Ymf262, Some(dro_core::config::NUKED_CORE));
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
        core.configure(&dro_core::vgm::ChipSettings::default());
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
}
