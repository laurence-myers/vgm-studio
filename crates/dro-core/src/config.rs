//! Application settings.
//!
//! Parsing lives here, wasm-clean and file-free. *Finding* the settings is the
//! platform's job: the native shell reads `drotrim.ini` from the working
//! directory and then the executable's directory; the web shell reads
//! `localStorage`. Both hand the text to [`AppConfig::from_ini_sources`].

use ini::Ini;

use crate::error::{Error, Result};

/// Audio settings. `[audio]` in `drotrim.ini`.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// transient. On: the boost is remembered and written to `drotrim.ini`.
    pub lock_boost: bool,
    pub buffer_size: u32,
    /// Sample rate of both the emulated chip and the audio output. 49716 is the
    /// OPL3's native rate and gives the best quality.
    pub frequency: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            bit_depth: 16,
            boost: 1.0,
            lock_boost: false,
            buffer_size: 512,
            frequency: 48_000,
        }
    }
}

/// The GUI colour scheme. Both are DOS-tracker looks after FastTracker II; see
/// the `theme` module in `dro-ui`. Stored as `theme=` in `[ui]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    /// The ft2-clone dark teal scheme.
    CloneDark,
    /// The original DOS FastTracker II steel-blue scheme.
    Ft2Classic,
    /// Bassoon "Variation 2" navy plate (the default).
    #[default]
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
    /// Bassoon petrol: a dark blue-green plate.
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

/// How the buttons (pads) or the panel they sit on (the deck) are coloured,
/// overriding what the chosen theme asks for. `ThemeDefault` leaves the theme's
/// own choice alone; the rest force a fixed treatment on any theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceChoice {
    /// Use whatever the selected theme specifies.
    #[default]
    ThemeDefault,
    /// Force a light (bone/cream) treatment.
    Light,
    /// Force a dark (charcoal/rubber) treatment.
    Dark,
    /// Force a neutral grey treatment, ignoring the theme's plate.
    Grey,
    /// Force a treatment tinted to match the theme's plate.
    Tint,
}

impl SurfaceChoice {
    /// Every option, in dropdown order.
    pub const ALL: [Self; 5] = [
        Self::ThemeDefault,
        Self::Light,
        Self::Dark,
        Self::Grey,
        Self::Tint,
    ];
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

/// Interface settings. `[ui]` in `drotrim.ini`.
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

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AppConfig {
    pub audio: AudioConfig,
    pub ui: UiConfig,
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
                "Could not read config from drotrim.ini, using default values. (Error: {error})"
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
            return Err(Error::config("Could not read drotrim.ini."));
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
        Ok(())
    }

    /// Renders the config as `drotrim.ini`, comments and all.
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
             deck_style={deck_style}\n",
            frequency = self.audio.frequency,
            bit_depth = self.audio.bit_depth,
            buffer_size = self.audio.buffer_size,
            boost = self.audio.boost,
            lock_boost = self.audio.lock_boost,
            tail_length = self.ui.tail_length,
            maximize_window = self.ui.maximize_window,
            dro_info_edit_enabled = self.ui.dro_info_edit_enabled,
            theme = self.ui.theme,
            pad_style = self.ui.pad_style,
            deck_style = self.ui.deck_style,
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

    /// The `drotrim.ini` that ships beside the executable. Compiled in, so this
    /// suite fails loudly if the shipped file ever stops parsing.
    const SHIPPED_INI: &str = include_str!("../../../src/drotrim.ini");

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
        assert_eq!(config.ui.theme, ThemeChoice::Navy);
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
        // With no readable `drotrim.ini`, the result is all defaults.
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
        // `chip_write_delay` was dropped once the OPL core's own write buffer
        // took over spacing register writes. Every ini written before that still
        // carries the key, and must still load rather than fall back to the
        // defaults wholesale.
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
