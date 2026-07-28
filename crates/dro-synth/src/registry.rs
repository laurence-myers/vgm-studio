//! Which core plays which chip -- as data, not as a `match`.
//!
//! `core_for` used to be a hard-coded arm per chip, which answers "is this chip
//! playable" and nothing else. It cannot say *how many* cores a chip has, what
//! they are called, what they cost in license terms, or which one the user
//! asked for. Every one of those is a question the Settings core picker and the
//! About credits need answered, so the mapping becomes a registry of
//! [`CoreInfo`] rows.
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
//! **Priority is registration order.** The first core registered for a chip is
//! its default, so the app registers accuracy-tier providers *before*
//! [`CoreRegistry::with_builtins`]'s permissive ones and a Nuked-class core
//! wins where present.

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
    ///
    /// Every core registered today is `true`, so nothing reads this yet.
    /// cr-11 brings the first `false` one and with it the split the plan
    /// describes: [`playability`](crate::chip::playability) filters on it (the
    /// transport must not offer a core that cannot keep up), while the WAV
    /// render does not (it has all the time in the world).
    pub realtime: bool,
    /// How to build it, or why it is not built here.
    pub make: CoreMaker,
}

impl CoreInfo {
    /// Builds this core, or `None` when it is one the app routes rather than
    /// constructs.
    #[must_use]
    pub fn build(&self) -> Option<Box<dyn ChipCore>> {
        match self.make {
            CoreMaker::Generic(make) => Some(make()),
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
                make: CoreMaker::Opl(|rate| Box::new(crate::opl::NukedOpl3::new(rate))),
            });
        }
        registry.register(CoreInfo {
            id: "sn76489.native",
            chip: ChipKind::Sn76489,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::Sn76489::new())),
        });
        registry.register(CoreInfo {
            id: "nesapu.native",
            chip: ChipKind::NesApu,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::NesApu::new())),
        });
        registry.register(CoreInfo {
            id: "gameboydmg.native",
            chip: ChipKind::GameBoyDmg,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::GbDmg::new())),
        });
        registry.register(CoreInfo {
            id: "ay8910.native",
            chip: ChipKind::Ay8910,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::Ay8910::new())),
        });
        registry.register(CoreInfo {
            id: "huc6280.native",
            chip: ChipKind::HuC6280,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::HuC6280::new())),
        });
        registry.register(CoreInfo {
            id: "okim6295.native",
            chip: ChipKind::Okim6295,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::Okim6295::new())),
        });
        registry.register(CoreInfo {
            id: "k051649.native",
            chip: ChipKind::K051649,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::K051649::new())),
        });
        // The Y8950 core exists (`cores::y8950`, FM + Delta-T, tests green)
        // but is deliberately NOT registered: the Y8950 is one of the OPL
        // chips, and the standing invariant -- OPL is listed for Settings
        // but never buildable for `VgmEngine`, because OPL documents route
        // through `PlayerEngine` -- is load-bearing for every `playability`
        // caller. Registering it here would send Y8950 files down two paths
        // at once. Letting it in means auditing that routing first; the
        // core waits for that step rather than pretending it happened.
        registry.register(CoreInfo {
            id: "qsound.native",
            chip: ChipKind::QSound,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::QSound::new())),
        });
        registry.register(CoreInfo {
            id: "c352.native",
            chip: ChipKind::C352,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::C352::new())),
        });
        registry.register(CoreInfo {
            id: "c140.native",
            chip: ChipKind::C140,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::C140::new())),
        });
        registry.register(CoreInfo {
            id: "k054539.native",
            chip: ChipKind::K054539,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::K054539::new())),
        });
        registry.register(CoreInfo {
            id: "rf5c68.native",
            chip: ChipKind::Rf5c68,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::Rf5c68::new())),
        });
        registry.register(CoreInfo {
            id: "rf5c164.native",
            chip: ChipKind::Rf5c164,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::Rf5c68::new())),
        });
        registry.register(CoreInfo {
            id: "okim6258.native",
            chip: ChipKind::Okim6258,
            label: "Clean-room (this project)",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::Okim6258::new())),
        });
        registry
    }

    /// Appends a core. Later registrations rank lower for their chip, so a
    /// provider that wants to be the default must register before the builtins.
    pub fn register(&mut self, info: CoreInfo) {
        self.entries.push(info);
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
        assert!(registry.has_core(ChipKind::Sn76489));
        assert!(
            registry.has_core(ChipKind::Ymf262),
            "OPL is listed, not built"
        );
        assert!(!registry.has_core(ChipKind::Ym2612), "no core yet");

        // Routed entries are listed but not constructed here.
        assert!(registry.build(ChipKind::Sn76489, None).is_some());
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
        let registry = CoreRegistry::with_builtins();
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

    #[test]
    fn the_ambient_registry_has_the_builtins_when_nothing_was_installed() {
        assert!(registry().has_core(ChipKind::Sn76489));
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

    fn info(id: &'static str) -> CoreInfo {
        CoreInfo {
            id,
            chip: ChipKind::Sn76489,
            label: id,
            authors: "test",
            license: "MIT",
            upstream: "",
            realtime: true,
            make: CoreMaker::Generic(|| Box::new(crate::cores::Sn76489::new())),
        }
    }
}
