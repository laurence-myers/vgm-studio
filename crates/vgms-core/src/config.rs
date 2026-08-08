//! Application settings.
//!
//! Parsing lives here, wasm-clean and file-free. *Finding* the settings is the
//! platform's job: the native shell reads `vgmstudio.ini` from the working
//! directory and then the executable's directory; the web shell reads
//! `localStorage`. Both hand the text to [`AppConfig::from_ini_sources`].

use std::collections::BTreeMap;

use ini::Ini;

use crate::error::{Error, Result};

/// Where playback goes: the emulator, or real hardware. Stored as
/// `output_backend=` in `[audio]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputBackend {
    /// Nuked OPL3, rendered to the sound card (the default).
    #[default]
    Emulated,
    /// A RetroWave OPL3 board: a real YMF262, heard through its own output.
    RetroWave,
}

impl core::fmt::Display for OutputBackend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Emulated => "emulated",
            Self::RetroWave => "retrowave",
        })
    }
}

impl core::str::FromStr for OutputBackend {
    type Err = ();

    /// Accepts hyphen or underscore, case-insensitively. Anything else errors,
    /// discarding the whole config like every other malformed value here.
    fn from_str(value: &str) -> core::result::Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "emulated" | "nuked" => Ok(Self::Emulated),
            "retrowave" | "retro-wave" | "retro_wave" => Ok(Self::RetroWave),
            _ => Err(()),
        }
    }
}

/// Audio settings. `[audio]` in `vgmstudio.ini`.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioConfig {
    pub bit_depth: u16,
    /// Playback volume multiplier, applied through a peak limiter so a boosted
    /// signal cannot clip. Live playback only -- never the WAV render or the
    /// waveform display.
    ///
    /// A bidirectional factor in `0.25..=64.0`, matching the reachable range of
    /// a VGM volume modifier (below `1.0` attenuates); `1.0` is bit-transparent.
    /// The GUI snaps it to the modifier factor ladder (see
    /// [`volume`](crate::volume)), but any value in range loads.
    ///
    /// Only kept across songs (and persisted) when [`Self::lock_boost`] is set;
    /// otherwise each song derives its own from its header volume modifier.
    pub boost: f32,
    /// Whether to keep [`Self::boost`] across songs. Off by default: each song's
    /// volume starts from its own header modifier and manual changes are
    /// transient. On: the boost is remembered and written to `vgmstudio.ini`.
    pub lock_boost: bool,
    pub buffer_size: u32,
    /// Sample rate of both the emulated chip and the audio output. 49716 is the
    /// OPL3's native rate and gives the best quality.
    ///
    /// Ignored by hardware output, which has no sound card to configure and
    /// steps the song at the chip's own rate.
    pub frequency: u32,
    /// Which core plays each chip, keyed by the chip's slot slug (`"opl3"`,
    /// `"ym2612"`) and valued with the core's short name (`"nuked"`, `"cqm"`).
    ///
    /// Stored as `core.<slug>=<name>` in `[audio]`. A slot with no entry gets
    /// whatever the core registry ranks first, so the map holds only what the
    /// user actually chose -- and a name this build does not have (a config
    /// written by the native app and read by the web one) falls back the same
    /// way rather than failing.
    ///
    /// The core names are *not* validated here: this crate knows nothing about
    /// emulators, and inventing a list to check against would put the truth in
    /// two places. `vgms-synth`'s registry is that list.
    pub cores: BTreeMap<String, String>,
    /// How non-OPL chips are brought to the output rate: `sinc`
    /// (band-limited, the default) or `linear` (the aliased, "crunchy"
    /// conversion VGMPlay and most classic players use).
    ///
    /// Stored as the slug rather than an enum, for the same reason the core
    /// names are strings: `vgms-synth` owns the list of methods, and a copy
    /// here to validate against would put the truth in two places. A value
    /// this build does not know falls back to the default at the point of
    /// use.
    pub resampling: String,
    /// The serial port of the RetroWave board, such as `COM3`. `None` picks the
    /// first port that looks like one.
    pub retrowave_port: Option<String>,
    /// This machine's measured speed relative to the core-speed baseline
    /// machine (`vgms_synth::speed::BASELINE`), from the Settings "measure"
    /// action. Scales every core-speed estimate the picker shows and the
    /// fidelity auto-select gates on. `None` until measured, which the
    /// estimates then read as "assume the baseline machine".
    pub machine_speed: Option<f32>,
}

/// The slot slug the OPL family shares -- one selector for OPL2, OPL3, YM3526
/// and Y8950, because one core (or one board) plays all four.
///
/// Duplicated from `vgms_synth::registry::OPL_SLOT_SLUG` rather than depended
/// on: this crate sits *below* vgms-synth and cannot see it. A test in vgms-synth
/// asserts the two agree.
pub const OPL_SLOT: &str = "opl3";

/// The *optional* slot splitting the OPL2 generation (YM3812, YM3526, Y8950)
/// off the family's shared `opl3` slot.
///
/// Absent -- the default -- the whole family reads `opl3`: one core for both
/// generations. Present, the OPL2-generation chips read this key instead, so
/// an OPL2-only core can sit under OPL2 captures while OPL3 material keeps
/// its own. [`AudioConfig::output_backend`] deliberately keeps reading the
/// family slot alone: hardware output is a whole-family routing decision, not
/// a per-generation core.
///
/// Duplicated from `vgms_synth::registry::OPL2_SLOT_SLUG`, as [`OPL_SLOT`] is
/// from its twin; a test in vgms-synth asserts the two agree.
pub const OPL2_SLOT: &str = "opl2";

/// The core name meaning "a RetroWave OPL3 board", in the `opl3` slot.
pub const RETROWAVE_CORE: &str = "retrowave";

/// The core name meaning "the built-in OPL emulator", in the `opl3` slot.
pub const NUKED_CORE: &str = "nuked";

impl AudioConfig {
    /// The core chosen for `slot`, or `None` to take the registry's default.
    #[must_use]
    pub fn core(&self, slot: &str) -> Option<&str> {
        self.cores.get(slot).map(String::as_str)
    }

    /// Chooses a core for `slot`; `None` clears the choice back to the default.
    pub fn set_core(&mut self, slot: &str, core: Option<&str>) {
        match core {
            Some(core) => {
                self.cores.insert(slot.to_owned(), core.to_owned());
            }
            None => {
                self.cores.remove(slot);
            }
        }
    }

    /// Where live OPL playback goes.
    ///
    /// A *view* of the `opl3` slot rather than a setting of its own, which is
    /// what it always was: the RetroWave board is an OPL3, so choosing it was
    /// choosing how OPL played. Now that every chip has a core choice, OPL's is
    /// spelled the same way as the rest, and this reads it back for the two
    /// places that genuinely ask about a *backend* -- which audio service to
    /// run, and whether samples pass through this program at all.
    #[must_use]
    pub fn output_backend(&self) -> OutputBackend {
        match self.core(OPL_SLOT) {
            Some(RETROWAVE_CORE) => OutputBackend::RetroWave,
            _ => OutputBackend::Emulated,
        }
    }

    /// Points the `opl3` slot at a backend.
    pub fn set_output_backend(&mut self, backend: OutputBackend) {
        self.set_core(
            OPL_SLOT,
            Some(match backend {
                OutputBackend::Emulated => NUKED_CORE,
                OutputBackend::RetroWave => RETROWAVE_CORE,
            }),
        );
    }
    /// Whether live playback passes through this program as samples.
    ///
    /// False for hardware output, where the board mixes its own sound and sends
    /// it out its own socket: nothing here can measure it (the peak meter) or
    /// shape it (the volume boost, per-channel panning). The controls for those
    /// are inert in that mode, so the GUI disables them rather than leaving them
    /// looking live.
    ///
    /// Offline work -- WAV rendering, splitting, the waveform display -- always
    /// uses the emulator and is unaffected either way.
    #[must_use]
    pub fn renders_samples(&self) -> bool {
        matches!(self.output_backend(), OutputBackend::Emulated)
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            bit_depth: 16,
            boost: 1.0,
            lock_boost: false,
            buffer_size: 512,
            frequency: 48_000,
            cores: BTreeMap::new(),
            resampling: "sinc".to_owned(),
            retrowave_port: None,
            machine_speed: None,
        }
    }
}

/// The GUI colour scheme. Both are DOS-tracker looks after FastTracker II; see
/// the `theme` module in `vgms-ui`. Stored as `theme=` in `[ui]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    /// The ft2-clone dark teal scheme.
    CloneDark,
    /// The original DOS FastTracker II steel-blue scheme.
    Ft2Classic,
    /// Bassoon "Variation 2" navy plate.
    Navy,
    /// Bassoon cream: a light plate, dark silkscreen, tone-on-tone keys.
    Cream,
    /// Bassoon verdigris: a patinated-copper three-stop metal plate.
    Verdigris,
    /// Bassoon moss: a muted green plate.
    Moss,
    /// Bassoon plum: a muted purple plate.
    Plum,
    /// Bassoon rust: a burnt orange-brown plate.
    Rust,
    /// Bassoon petrol: a dark blue-green plate (the default).
    #[default]
    Petrol,
    /// Bassoon slate: a cool blue-grey plate.
    Slate,
    /// Bassoon olive: a dark yellow-green plate.
    Olive,
    /// Bassoon wine: a deep burgundy plate.
    Wine,
}

impl ThemeChoice {
    /// Every theme, in dropdown order. Lets exhaustive consumers (the theme
    /// showcase snapshot test) iterate all themes; a new variant added here
    /// then fails that test with a missing baseline until one is generated.
    pub const ALL: [Self; 12] = [
        Self::CloneDark,
        Self::Ft2Classic,
        Self::Navy,
        Self::Cream,
        Self::Verdigris,
        Self::Moss,
        Self::Plum,
        Self::Rust,
        Self::Petrol,
        Self::Slate,
        Self::Olive,
        Self::Wine,
    ];
}

impl core::fmt::Display for ThemeChoice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::CloneDark => "clone-dark",
            Self::Ft2Classic => "ft2-classic",
            Self::Navy => "navy",
            Self::Cream => "cream",
            Self::Verdigris => "verdigris",
            Self::Moss => "moss",
            Self::Plum => "plum",
            Self::Rust => "rust",
            Self::Petrol => "petrol",
            Self::Slate => "slate",
            Self::Olive => "olive",
            Self::Wine => "wine",
        })
    }
}

impl core::str::FromStr for ThemeChoice {
    type Err = ();

    /// Accepts hyphen or underscore, case-insensitively. Anything else errors,
    /// so a typo in `theme=` discards the whole config like every other
    /// malformed value here.
    fn from_str(value: &str) -> core::result::Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "clone-dark" | "clone_dark" | "clonedark" => Ok(Self::CloneDark),
            "ft2-classic" | "ft2_classic" | "ft2classic" => Ok(Self::Ft2Classic),
            "navy" => Ok(Self::Navy),
            "cream" => Ok(Self::Cream),
            "verdigris" => Ok(Self::Verdigris),
            "moss" => Ok(Self::Moss),
            "plum" => Ok(Self::Plum),
            "rust" => Ok(Self::Rust),
            "petrol" => Ok(Self::Petrol),
            "slate" => Ok(Self::Slate),
            "olive" => Ok(Self::Olive),
            "wine" => Ok(Self::Wine),
            _ => Err(()),
        }
    }
}

/// Which optimiser compresses a VGM on pack export and Edit > Optimize. Stored
/// as `optimizer=` in `[optimize]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptimizerChoice {
    /// The built-in optimiser for a file whose every chip it covers, the
    /// external vgmtools (`vgm_cmp`, `vgm_sro`, `optdac`) as the fallback for
    /// the rest. The recommended default.
    #[default]
    Auto,
    /// The built-in optimiser only -- never spawn the external tools. A chip the
    /// built-in has no rules for gets only its (safe) delay-merge, not the
    /// tools' redundancy pass. What the web build uses, and a minimal-dependency
    /// desktop option.
    BuiltInOnly,
    /// The external vgmtools always, whatever the file -- the original behaviour,
    /// kept as an A/B control against the built-in.
    Tools,
}

impl OptimizerChoice {
    /// Every option, in dropdown order.
    pub const ALL: [Self; 3] = [Self::Auto, Self::BuiltInOnly, Self::Tools];

    /// The label the Settings dropdown shows.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic (built-in, tools as fallback)",
            Self::BuiltInOnly => "Built-in only",
            Self::Tools => "External tools (vgmtools)",
        }
    }
}

impl core::fmt::Display for OptimizerChoice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::BuiltInOnly => "built-in",
            Self::Tools => "tools",
        })
    }
}

impl core::str::FromStr for OptimizerChoice {
    type Err = ();

    /// Accepts hyphen or underscore, case-insensitively, like the other choices.
    fn from_str(value: &str) -> core::result::Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "built-in" | "built_in" | "builtin" | "built-in-only" | "builtinonly" => {
                Ok(Self::BuiltInOnly)
            }
            "tools" | "vgmtools" | "external" => Ok(Self::Tools),
            _ => Err(()),
        }
    }
}

/// How the buttons (pads) or the panel they sit on (the deck) are coloured,
/// overriding what the chosen theme asks for. `ThemeDefault` leaves the theme's
/// own choice alone; the rest force a fixed treatment on any theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceChoice {
    /// Use whatever the selected theme specifies.
    #[default]
    ThemeDefault,
    /// Force a light (neutral white/grey) treatment.
    Light,
    /// Force a dark (charcoal/rubber) treatment.
    Dark,
    /// Force a neutral grey treatment, ignoring the theme's plate.
    Grey,
    /// Force a treatment tinted to match the theme's plate.
    Tint,
}

impl SurfaceChoice {
    /// Every option, in dropdown order. What the pads offer.
    pub const ALL: [Self; 5] = [
        Self::ThemeDefault,
        Self::Light,
        Self::Dark,
        Self::Grey,
        Self::Tint,
    ];

    /// What the *deck* offers. A grey deck reads as flat and dirty under every
    /// plate, so it is not one of the treatments the deck can take.
    pub const DECK: [Self; 4] = [Self::ThemeDefault, Self::Light, Self::Dark, Self::Tint];

    /// `self` as the deck can express it. [`Self::Grey`] is not a deck
    /// treatment, so it falls back to the theme's own choice -- an ini written
    /// by hand (or by an older build) can still name it without painting one.
    #[must_use]
    pub fn for_deck(self) -> Self {
        if self == Self::Grey {
            Self::ThemeDefault
        } else {
            self
        }
    }
}

impl core::fmt::Display for SurfaceChoice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::ThemeDefault => "default",
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Grey => "grey",
            Self::Tint => "tint",
        })
    }
}

impl core::str::FromStr for SurfaceChoice {
    type Err = ();

    /// Accepts hyphen or underscore, case-insensitively, like [`ThemeChoice`].
    fn from_str(value: &str) -> core::result::Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" | "theme-default" | "theme_default" | "themedefault" => {
                Ok(Self::ThemeDefault)
            }
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            // Accept both spellings; the config is hand-edited.
            "grey" | "gray" => Ok(Self::Grey),
            "tint" => Ok(Self::Tint),
            _ => Err(()),
        }
    }
}

/// Interface settings. `[ui]` in `vgmstudio.ini`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiConfig {
    /// Whether the DRO Info dialog allows editing the header.
    pub dro_info_edit_enabled: bool,
    /// Whether to maximize the window at launch. Native only; the web canvas
    /// always fills the viewport.
    pub maximize_window: bool,
    /// How many milliseconds the "play last X seconds" button plays.
    pub tail_length: u32,
    /// The GUI colour scheme.
    pub theme: ThemeChoice,
    /// Overrides the theme's keycap treatment.
    pub pad_style: SurfaceChoice,
    /// Overrides the theme's control-panel (deck) treatment.
    pub deck_style: SurfaceChoice,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            dro_info_edit_enabled: false,
            maximize_window: false,
            tail_length: 3000,
            theme: ThemeChoice::default(),
            pad_style: SurfaceChoice::default(),
            deck_style: SurfaceChoice::default(),
        }
    }
}

/// Not `Copy`: [`AudioConfig::retrowave_port`] is an owned port name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppConfig {
    pub audio: AudioConfig,
    pub ui: UiConfig,
    /// Which optimiser pack export and Edit > Optimize use. `[optimize]`.
    pub optimizer: OptimizerChoice,
}

impl AppConfig {
    /// Builds a config from zero or more INI documents, later ones overriding
    /// earlier ones.
    ///
    /// Any failure -- no sources at all, unparseable INI, or a malformed value --
    /// yields the complete set of defaults, with a warning logged.
    #[must_use]
    pub fn from_ini_sources(sources: &[&str]) -> Self {
        Self::try_from_ini_sources(sources).unwrap_or_else(|error| {
            log::warn!(
                "Could not read config from vgmstudio.ini, using default values. (Error: {error})"
            );
            Self::default()
        })
    }

    /// As [`Self::from_ini_sources`], but reporting why it failed.
    ///
    /// # Errors
    /// If `sources` is empty, if any source is not valid INI, if any value present
    /// in a source cannot be parsed as its declared type, or if the result does
    /// not pass [`Self::validate`].
    pub fn try_from_ini_sources(sources: &[&str]) -> Result<Self> {
        if sources.is_empty() {
            return Err(Error::config("Could not read vgmstudio.ini."));
        }
        let mut config = Self::default();
        for source in sources {
            config.apply_ini(source)?;
        }
        config.validate()?;
        Ok(config)
    }

    /// Rejects settings that parse but cannot work.
    ///
    /// An out-of-range `bit_depth` such as `4` would make the playback-sample
    /// calculation return zero and the player divide by it, and `frequency = 0`
    /// would reach the emulator. Both are better caught here, where the answer is
    /// "use the defaults and warn".
    ///
    /// # Errors
    /// If any setting is outside the range the player can honour.
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.audio.bit_depth, 8 | 16) {
            return Err(Error::config(format!(
                "Invalid value for audio.bit_depth: {} (expected 8 or 16)",
                self.audio.bit_depth
            )));
        }
        if self.audio.frequency == 0 {
            return Err(Error::config("Invalid value for audio.frequency: 0"));
        }
        if self.audio.buffer_size == 0 {
            return Err(Error::config("Invalid value for audio.buffer_size: 0"));
        }
        if !self.audio.boost.is_finite() || !(0.25..=64.0).contains(&self.audio.boost) {
            return Err(Error::config(format!(
                "Invalid value for audio.boost: {} (expected a number from 0.25 to 64)",
                self.audio.boost
            )));
        }
        Ok(())
    }

    /// Overrides the keys present in `source`, leaving the rest untouched.
    ///
    /// # Errors
    /// If `source` is not valid INI, or a value present in it is malformed.
    pub fn apply_ini(&mut self, source: &str) -> Result<()> {
        let ini = Ini::load_from_str(source)
            .map_err(|error| Error::config(format!("Could not parse INI: {error}")))?;

        if let Some(value) = lookup(&ini, "audio", "bit_depth") {
            self.audio.bit_depth = parse(value, "audio.bit_depth")?;
        }
        if let Some(value) = lookup(&ini, "audio", "boost") {
            self.audio.boost = parse(value, "audio.boost")?;
        }
        if let Some(value) = lookup(&ini, "audio", "lock_boost") {
            self.audio.lock_boost = parse_bool(value, "audio.lock_boost")?;
        }
        if let Some(value) = lookup(&ini, "audio", "buffer_size") {
            self.audio.buffer_size = parse(value, "audio.buffer_size")?;
        }
        if let Some(value) = lookup(&ini, "audio", "frequency") {
            self.audio.frequency = parse(value, "audio.frequency")?;
        }
        if let Some(value) = lookup(&ini, "audio", "resampling") {
            // Normalised but not validated, matching the core names: an empty
            // value restores the default, anything else is kept verbatim for
            // `vgms-synth` to recognise or fall back from.
            let value = value.trim().to_ascii_lowercase();
            self.audio.resampling = if value.is_empty() {
                AudioConfig::default().resampling
            } else {
                value
            };
        }
        // `output_backend` is the OPL row's core choice under a legacy name.
        // Read it so an existing vgmstudio.ini keeps its hardware setting; it is
        // not written back, so the file converges on the new spelling. Applied
        // *before* the `core.*` keys so an explicit new-style choice wins.
        if let Some(value) = lookup(&ini, "audio", "output_backend") {
            let backend: OutputBackend = parse(value, "audio.output_backend")?;
            self.audio.set_output_backend(backend);
        }
        for (key, value) in section_keys(&ini, "audio") {
            let Some(slot) = key.strip_prefix("core.") else {
                continue;
            };
            let (slot, value) = (slot.trim(), value.trim());
            if slot.is_empty() {
                return Err(Error::config("Invalid key: audio.core. has no chip"));
            }
            // An empty value means "no preference", which is how a slot gets
            // cleared back to the registry default without deleting the line.
            self.audio.set_core(
                &slot.to_ascii_lowercase(),
                (!value.is_empty()).then_some(value),
            );
        }
        if let Some(value) = lookup(&ini, "audio", "retrowave_port") {
            let value = value.trim();
            self.audio.retrowave_port = (!value.is_empty()).then(|| value.to_owned());
        }
        if let Some(value) = lookup(&ini, "audio", "machine_speed") {
            let value = value.trim();
            self.audio.machine_speed = if value.is_empty() {
                None
            } else {
                Some(parse(value, "audio.machine_speed")?)
            };
        }

        if let Some(value) = lookup(&ini, "ui", "dro_info_edit_enabled") {
            self.ui.dro_info_edit_enabled = parse_bool(value, "ui.dro_info_edit_enabled")?;
        }
        if let Some(value) = lookup(&ini, "ui", "maximize_window") {
            self.ui.maximize_window = parse_bool(value, "ui.maximize_window")?;
        }
        if let Some(value) = lookup(&ini, "ui", "tail_length") {
            self.ui.tail_length = parse(value, "ui.tail_length")?;
        }
        if let Some(value) = lookup(&ini, "ui", "theme") {
            self.ui.theme = parse(value, "ui.theme")?;
        }
        if let Some(value) = lookup(&ini, "ui", "pad_style") {
            self.ui.pad_style = parse(value, "ui.pad_style")?;
        }
        if let Some(value) = lookup(&ini, "ui", "deck_style") {
            self.ui.deck_style = parse(value, "ui.deck_style")?;
        }
        if let Some(value) = lookup(&ini, "optimize", "optimizer") {
            self.optimizer = parse(value, "optimize.optimizer")?;
        }
        Ok(())
    }

    /// Renders the config as `vgmstudio.ini`, comments and all.
    ///
    /// Round-trips through [`Self::apply_ini`].
    #[must_use]
    pub fn to_ini_string(&self) -> String {
        format!(
            "[audio]\n\
             # Change frequency to 49716 for optimal quality.\n\
             # This controls the sampling rate of the emulated chip,\n\
             # AND the sampling rate of any audio output.\n\
             frequency={frequency}\n\
             bit_depth={bit_depth}\n\
             buffer_size={buffer_size}\n\
             # Volume multiplier for live playback: 1 = no change, below 1\n\
             # attenuates, above 1 boosts (0.25 to 64). A peak limiter keeps\n\
             # louder values from clipping. Never affects the WAV render or the\n\
             # waveform display.\n\
             boost={boost}\n\
             # Keep the volume across songs (true), or start each song from its\n\
             # own header volume modifier (false, the default).\n\
             lock_boost={lock_boost}\n\
             # Which emulator core plays each chip, one line per chip:\n\
             #   core.<chip>=<core name>\n\
             # The chip names are the ones the Settings dialog lists; the OPL\n\
             # family shares the single slot \"opl3\", since one core -- or one\n\
             # board -- plays all of it. For opl3, \"retrowave\" sends live\n\
             # playback to a RetroWave OPL3 board instead of the emulator.\n\
             # A chip with no line here uses the best core available, and a core\n\
             # name this build does not have falls back the same way.\n\
             # Rendering and splitting always use an emulator, never hardware.\n\
             {cores}\
             # How non-OPL chips reach the output rate: \"sinc\" (band-limited,\n\
             # clean, the default) or \"linear\" (aliased and crunchy, the way\n\
             # VGMPlay and most classic players sound).\n\
             resampling={resampling}\n\
             # Serial port of the RetroWave board, e.g. COM3 or /dev/ttyACM0.\n\
             # Leave empty to use the first one detected.\n\
             retrowave_port={retrowave_port}\n\
             # This machine's measured speed relative to the project's\n\
             # reference machine, from Settings > Output > Measure. Scales the\n\
             # core speed estimates; empty means unmeasured.\n\
             machine_speed={machine_speed}\n\
             \n\
             [ui]\n\
             # Tail length is the value for the \"Play last X seconds\" button,\n\
             #  given in milliseconds.\n\
             tail_length={tail_length}\n\
             # Set this to true/1/yes/on for the window to be maximized at launch.\n\
             maximize_window={maximize_window}\n\
             # Set this to true/1/yes/on to enable editing the DRO metadata.\n\
             dro_info_edit_enabled={dro_info_edit_enabled}\n\
             # Colour scheme (case): navy, cream, verdigris, moss, plum, rust,\n\
             # petrol, slate, olive, wine, clone-dark, ft2-classic.\n\
             theme={theme}\n\
             # Override the theme's keycap / control-panel treatment:\n\
             # default (leave the theme alone), light, dark, grey or tint.\n\
             pad_style={pad_style}\n\
             deck_style={deck_style}\n\
             \n\
             [optimize]\n\
             # Which optimiser compresses a VGM on pack export and Edit >\n\
             # Optimize: \"auto\" (built-in where it covers every chip, the\n\
             # vgmtools as a fallback -- the default), \"built-in\" (built-in\n\
             # only, no external tools), or \"tools\" (the vgmtools always).\n\
             optimizer={optimizer}\n",
            frequency = self.audio.frequency,
            bit_depth = self.audio.bit_depth,
            buffer_size = self.audio.buffer_size,
            boost = self.audio.boost,
            lock_boost = self.audio.lock_boost,
            // `BTreeMap`, so the lines come out in a stable order and a save
            // that changed nothing produces the same file.
            cores = self
                .audio
                .cores
                .iter()
                .map(|(slot, core)| format!("core.{slot}={core}\n"))
                .collect::<String>(),
            resampling = self.audio.resampling,
            retrowave_port = self.audio.retrowave_port.as_deref().unwrap_or_default(),
            machine_speed = self
                .audio
                .machine_speed
                .map(|ratio| ratio.to_string())
                .unwrap_or_default(),
            tail_length = self.ui.tail_length,
            maximize_window = self.ui.maximize_window,
            dro_info_edit_enabled = self.ui.dro_info_edit_enabled,
            theme = self.ui.theme,
            pad_style = self.ui.pad_style,
            deck_style = self.ui.deck_style,
            optimizer = self.optimizer,
        )
    }
}

/// Where the settings live on this platform.
pub trait ConfigStore {
    /// Reads the settings, falling back to defaults rather than failing.
    fn load(&self) -> AppConfig;

    /// Persists the settings.
    ///
    /// # Errors
    /// If the settings could not be written to their backing store.
    fn save(&self, config: &AppConfig) -> Result<()>;
}

/// Every key in a section, lowercased, for the settings whose *names* carry
/// data -- `core.<chip slug>`, where the chip is part of the key.
fn section_keys<'a>(ini: &'a Ini, section: &str) -> Vec<(String, &'a str)> {
    ini.section(Some(section))
        .into_iter()
        .flat_map(|props| props.iter())
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .collect()
}

/// Finds a key case-insensitively. Section names stay case-sensitive.
fn lookup<'a>(ini: &'a Ini, section: &str, key: &str) -> Option<&'a str> {
    ini.section(Some(section))?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value)
}

fn parse<T>(value: &str, key: &str) -> Result<T>
where
    T: core::str::FromStr,
{
    value
        .trim()
        .parse()
        .map_err(|_| Error::config(format!("Invalid value for {key}: {value:?}")))
}

/// Accepts the standard INI boolean literals, case-insensitively.
fn parse_bool(value: &str, key: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "yes" | "true" | "on" => Ok(true),
        "0" | "no" | "false" | "off" => Ok(false),
        _ => Err(Error::config(format!(
            "Invalid value for {key}: {value:?} (expected 1/yes/true/on or 0/no/false/off)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `vgmstudio.ini` that ships beside the executable. Compiled in, so this
    /// suite fails loudly if the shipped file ever stops parsing.
    const SHIPPED_INI: &str = include_str!("../../../src/vgmstudio.ini");

    #[test]
    fn defaults_are_correct() {
        let config = AppConfig::default();
        assert_eq!(config.audio.bit_depth, 16);
        assert_eq!(config.audio.boost, 1.0);
        assert!(!config.audio.lock_boost);
        assert_eq!(config.audio.buffer_size, 512);
        assert_eq!(config.audio.frequency, 48_000);
        assert!(!config.ui.dro_info_edit_enabled);
        assert!(!config.ui.maximize_window);
        assert_eq!(config.ui.tail_length, 3000);
        assert_eq!(config.ui.theme, ThemeChoice::Petrol);
    }

    #[test]
    fn the_shipped_ini_parses_to_the_defaults() {
        // Every value in the shipped file happens to equal the coded default.
        let config = AppConfig::from_ini_sources(&[SHIPPED_INI]);
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn values_are_read_from_the_ini() {
        let config = AppConfig::from_ini_sources(&[
            "[audio]\nfrequency=49716\nbit_depth=8\nbuffer_size=2048\nboost=2.5\nlock_boost=yes\n\
             [ui]\ntail_length=5000\nmaximize_window=yes\ndro_info_edit_enabled=on\ntheme=ft2-classic\n",
        ]);
        assert_eq!(config.audio.frequency, 49_716);
        assert_eq!(config.audio.bit_depth, 8);
        assert_eq!(config.audio.boost, 2.5);
        assert!(config.audio.lock_boost);
        assert_eq!(config.audio.buffer_size, 2048);
        assert_eq!(config.ui.tail_length, 5000);
        assert!(config.ui.maximize_window);
        assert!(config.ui.dro_info_edit_enabled);
        assert_eq!(config.ui.theme, ThemeChoice::Ft2Classic);
    }

    #[test]
    fn missing_keys_and_sections_keep_their_defaults() {
        let config = AppConfig::from_ini_sources(&["[audio]\nfrequency=49716\n"]);
        assert_eq!(config.audio.frequency, 49_716);
        assert_eq!(config.audio.bit_depth, AudioConfig::default().bit_depth);
        assert_eq!(config.ui, UiConfig::default());

        // An empty document is valid INI and changes nothing.
        assert_eq!(AppConfig::from_ini_sources(&[""]), AppConfig::default());
    }

    #[test]
    fn no_sources_yields_defaults() {
        // With no readable `vgmstudio.ini`, the result is all defaults.
        assert_eq!(AppConfig::from_ini_sources(&[]), AppConfig::default());
        assert!(AppConfig::try_from_ini_sources(&[]).is_err());
    }

    #[test]
    fn later_sources_override_earlier_ones() {
        // Reading both sources lets `b` win, so an ini beside the executable
        // overrides one in the working directory.
        let cwd = "[audio]\nfrequency=44100\nbit_depth=8\n";
        let exe_dir = "[audio]\nfrequency=49716\n";
        let config = AppConfig::from_ini_sources(&[cwd, exe_dir]);
        assert_eq!(config.audio.frequency, 49_716);
        // Untouched by the second source, so the first source's value survives.
        assert_eq!(config.audio.bit_depth, 8);
    }

    #[test]
    fn one_malformed_value_discards_the_whole_config() {
        // One malformed value discards the whole config and returns *all*
        // defaults -- including the settings that parsed fine.
        let source = "[audio]\nfrequency=nonsense\nbit_depth=8\n[ui]\ntail_length=5000\n";
        assert_eq!(AppConfig::from_ini_sources(&[source]), AppConfig::default());

        let error = AppConfig::try_from_ini_sources(&[source]).unwrap_err();
        assert!(matches!(error, Error::Config(_)));
        assert!(error.to_string().contains("audio.frequency"));
    }

    #[test]
    fn boolean_literals_match_configparser() {
        for truthy in ["1", "yes", "true", "on", "YES", "True", "ON"] {
            assert_eq!(parse_bool(truthy, "k"), Ok(true), "{truthy}");
        }
        for falsy in ["0", "no", "false", "off", "NO", "False", "OFF"] {
            assert_eq!(parse_bool(falsy, "k"), Ok(false), "{falsy}");
        }
        for bad in ["", "y", "n", "2", "maybe", "tru"] {
            assert!(parse_bool(bad, "k").is_err(), "{bad}");
        }
    }

    #[test]
    fn keys_are_matched_case_insensitively() {
        // Key names are matched case-insensitively.
        let config = AppConfig::from_ini_sources(&["[audio]\nFREQUENCY=49716\n"]);
        assert_eq!(config.audio.frequency, 49_716);
    }

    #[test]
    fn theme_accepts_both_separators_and_cases() {
        for text in ["clone-dark", "clone_dark", "CloneDark", " CLONE-DARK "] {
            assert_eq!(text.parse(), Ok(ThemeChoice::CloneDark), "{text}");
        }
        for text in ["ft2-classic", "ft2_classic", "FT2Classic", " Ft2-Classic "] {
            assert_eq!(text.parse(), Ok(ThemeChoice::Ft2Classic), "{text}");
        }
        assert_eq!("nonsense".parse::<ThemeChoice>(), Err(()));
    }

    #[test]
    fn theme_display_round_trips_through_from_str() {
        for choice in [ThemeChoice::CloneDark, ThemeChoice::Ft2Classic] {
            assert_eq!(choice.to_string().parse(), Ok(choice));
        }
    }

    #[test]
    fn the_output_backend_parses_its_spellings_and_round_trips() {
        for text in ["emulated", "Emulated", " nuked "] {
            assert_eq!(text.parse(), Ok(OutputBackend::Emulated), "{text}");
        }
        for text in ["retrowave", "RetroWave", "retro-wave", " retro_wave "] {
            assert_eq!(text.parse(), Ok(OutputBackend::RetroWave), "{text}");
        }
        assert_eq!("speakers".parse::<OutputBackend>(), Err(()));
        for choice in [OutputBackend::Emulated, OutputBackend::RetroWave] {
            assert_eq!(choice.to_string().parse(), Ok(choice));
        }
    }

    /// Following the convention: one bad value reverts everything to defaults,
    /// which for the backend means the emulator — so playback still works.
    #[test]
    fn an_invalid_output_backend_discards_the_whole_config() {
        let source = "[audio]\noutput_backend=parallel_port\n";
        let config = AppConfig::from_ini_sources(&[source]);
        assert_eq!(config, AppConfig::default());
        assert_eq!(config.audio.output_backend(), OutputBackend::Emulated);
        let error = AppConfig::try_from_ini_sources(&[source]).unwrap_err();
        assert!(error.to_string().contains("audio.output_backend"));
    }

    /// An existing `vgmstudio.ini` says `output_backend=`; the app now writes
    /// `core.opl3=`. Reading the old spelling must keep a user's board
    /// selected, or upgrading silently moves their playback back to the
    /// emulator.
    #[test]
    fn the_old_output_backend_key_migrates_to_the_opl_slot() {
        let config = AppConfig::from_ini_sources(&["[audio]\noutput_backend=retrowave\n"]);
        assert_eq!(config.audio.output_backend(), OutputBackend::RetroWave);
        assert_eq!(config.audio.core(OPL_SLOT), Some(RETROWAVE_CORE));

        // And it is not written back, so one save converges on the new name.
        let written = config.to_ini_string();
        assert!(written.contains("core.opl3=retrowave"));
        assert!(
            !written.contains("output_backend="),
            "the legacy key is read, never re-emitted"
        );
    }

    /// Both spellings in one file: the explicit new key is the one the user
    /// last chose through the dialog, so it wins.
    #[test]
    fn a_new_style_key_beats_the_migrated_one() {
        let config =
            AppConfig::from_ini_sources(&["[audio]\noutput_backend=retrowave\ncore.opl3=nuked\n"]);
        assert_eq!(config.audio.output_backend(), OutputBackend::Emulated);
    }

    #[test]
    fn core_choices_are_per_chip_and_survive_a_round_trip() {
        let config = AppConfig::from_ini_sources(&[
            "[audio]\ncore.ym2612=nuked\nCORE.SN76489 = native \ncore.opl3=\n",
        ]);
        assert_eq!(config.audio.core("ym2612"), Some("nuked"));
        assert_eq!(
            config.audio.core("sn76489"),
            Some("native"),
            "keys are case-insensitive and values trimmed, like every other key"
        );
        assert_eq!(
            config.audio.core("opl3"),
            None,
            "an empty value clears the slot back to the registry default"
        );

        let reread = AppConfig::from_ini_sources(&[&config.to_ini_string()]);
        assert_eq!(reread.audio.cores, config.audio.cores);
    }

    /// A core name this build has never heard of is a normal thing -- the web
    /// build genuinely lacks ids the native one has -- so it must load. Which
    /// core it resolves to is the registry's problem, not this crate's.
    #[test]
    fn an_unknown_core_name_is_stored_rather_than_rejected() {
        let config = AppConfig::try_from_ini_sources(&["[audio]\ncore.ym2612=nonesuch\n"])
            .expect("an unknown core must not discard the config");
        assert_eq!(config.audio.core("ym2612"), Some("nonesuch"));
    }

    /// The GUI disables the boost, the pan knobs and the peak meter off this:
    /// hardware output mixes on the chip, so none of them can do anything.
    #[test]
    fn only_the_emulator_renders_samples_this_program_can_shape() {
        let mut config = AudioConfig::default();
        assert!(config.renders_samples(), "the emulator renders through us");
        config.set_output_backend(OutputBackend::RetroWave);
        assert!(!config.renders_samples(), "the board mixes its own output");
    }

    /// An unset port means "find one", which is different from a port literally
    /// named the empty string.
    #[test]
    fn an_empty_retrowave_port_reads_back_as_no_port() {
        let config = AppConfig::from_ini_sources(&["[audio]\nretrowave_port=\n"]);
        assert_eq!(config.audio.retrowave_port, None);

        let config = AppConfig::from_ini_sources(&["[audio]\nretrowave_port= COM4 \n"]);
        assert_eq!(config.audio.retrowave_port.as_deref(), Some("COM4"));
    }

    #[test]
    fn an_invalid_theme_discards_the_whole_config() {
        // As with every malformed value, one bad `theme=` reverts to all defaults.
        let source = "[ui]\ntheme=magenta\ntail_length=5000\n";
        assert_eq!(AppConfig::from_ini_sources(&[source]), AppConfig::default());
        let error = AppConfig::try_from_ini_sources(&[source]).unwrap_err();
        assert!(error.to_string().contains("ui.theme"));
    }

    #[test]
    fn full_line_comments_are_ignored_and_inline_ones_are_not() {
        // `rust-ini` with default features treats `#` as starting a comment
        // only at the beginning of a line.
        let config = AppConfig::from_ini_sources(&["[audio]\n# frequency=1\nfrequency=49716\n"]);
        assert_eq!(config.audio.frequency, 49_716);

        // An inline `#` is part of the value, so this is a malformed integer and
        // the whole config falls back to defaults.
        let config = AppConfig::from_ini_sources(&["[audio]\nfrequency=49716 # native rate\n"]);
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn negative_numbers_are_rejected_rather_than_wrapped() {
        assert_eq!(
            AppConfig::from_ini_sources(&["[audio]\nbuffer_size=-1\n"]),
            AppConfig::default()
        );
    }

    #[test]
    fn unusable_values_fall_back_to_defaults() {
        for source in [
            "[audio]\nbit_depth=4\n",
            "[audio]\nbit_depth=24\n", // the OPL emulator renders 8- or 16-bit only
            "[audio]\nfrequency=0\n",
            "[audio]\nbuffer_size=0\n",
            "[audio]\nboost=0\n",   // below the 0.25 floor
            "[audio]\nboost=0.1\n", // below the 0.25 floor
            "[audio]\nboost=100\n", // above the 64.0 ceiling
            "[audio]\nboost=nan\n",
            "[audio]\nboost=inf\n",
        ] {
            assert_eq!(
                AppConfig::from_ini_sources(&[source]),
                AppConfig::default(),
                "{source:?}"
            );
            assert!(
                AppConfig::try_from_ini_sources(&[source]).is_err(),
                "{source:?}"
            );
        }
    }

    #[test]
    fn the_bidirectional_boost_range_is_accepted() {
        // The boost is a two-way volume factor: below 1.0 attenuates, above 1.0
        // boosts, spanning the VGM volume modifier's 0.25..=64 reach.
        for (source, expected) in [
            ("[audio]\nboost=0.25\n", 0.25), // the attenuation floor
            ("[audio]\nboost=0.5\n", 0.5),   // half volume
            ("[audio]\nboost=1\n", 1.0),     // unity
            ("[audio]\nboost=64\n", 64.0),   // the boost ceiling
        ] {
            let config = AppConfig::try_from_ini_sources(&[source])
                .unwrap_or_else(|e| panic!("{source:?} should be valid: {e}"));
            assert_eq!(config.audio.boost, expected, "{source:?}");
        }
    }

    #[test]
    fn the_defaults_are_themselves_valid() {
        assert!(AppConfig::default().validate().is_ok());
    }

    #[test]
    fn a_retired_key_is_ignored_rather_than_rejected() {
        // `chip_write_delay` is a retired key; an ini written before it was
        // dropped still carries it, and must still load rather than fall back to
        // the defaults wholesale.
        let source = "[audio]\nchip_write_delay=26.6\nbuffer_size=2048\n";
        let config = AppConfig::try_from_ini_sources(&[source]).expect("still parses");
        assert_eq!(config.audio.buffer_size, 2048, "the rest is still read");
    }

    #[test]
    fn ini_round_trips() {
        let config = AppConfig {
            audio: AudioConfig {
                bit_depth: 8,
                boost: 2.5,
                lock_boost: true,
                buffer_size: 2048,
                frequency: 49_716,
                // The OPL slot spelled as hardware, plus a second chip's core,
                // so the round trip covers both a migrated key and a new one.
                cores: BTreeMap::from([
                    ("opl3".to_owned(), "retrowave".to_owned()),
                    ("sn76489".to_owned(), "native".to_owned()),
                ]),
                // Not the default, so the round trip is proven to carry it.
                resampling: "linear".to_owned(),
                retrowave_port: Some("COM7".to_owned()),
                machine_speed: Some(1.25),
            },
            ui: UiConfig {
                dro_info_edit_enabled: true,
                maximize_window: true,
                tail_length: 5000,
                theme: ThemeChoice::Ft2Classic,
                // Non-default, so the round-trip actually exercises them.
                pad_style: SurfaceChoice::Light,
                deck_style: SurfaceChoice::Dark,
            },
            // Non-default, so the round-trip carries it.
            optimizer: OptimizerChoice::Tools,
        };
        let rendered = config.to_ini_string();
        assert_eq!(AppConfig::from_ini_sources(&[&rendered]), config);

        // And the defaults render to something that reads back as the defaults.
        let defaults = AppConfig::default();
        assert_eq!(
            AppConfig::from_ini_sources(&[&defaults.to_ini_string()]),
            defaults
        );
    }
}
