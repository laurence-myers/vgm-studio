//! The native `ConfigStore`: `drotrim.ini` on disk.
//!
//! Loading follows the lookup order (working directory, then the
//! executable's directory, the latter overriding). Saving targets the
//! executable's copy first, because that is the one whose values win the
//! load order.

use std::fs;
use std::path::PathBuf;

use dro_core::config::{AppConfig, ConfigStore};
use dro_core::{Error, Result};

#[derive(Debug, Default)]
pub struct IniConfigStore;

impl IniConfigStore {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn exe_ini() -> Option<PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("drotrim.ini")))
    }
}

impl ConfigStore for IniConfigStore {
    fn load(&self) -> AppConfig {
        crate::config::load_config()
    }

    fn save(&self, config: &AppConfig) -> Result<()> {
        let text = config.to_ini_string();
        let exe_ini = Self::exe_ini();
        if let Some(path) = &exe_ini
            && fs::write(path, &text).is_ok()
        {
            return Ok(());
        }
        // The exe directory is not writable. The working-directory copy only
        // takes effect if no exe-dir ini exists to shadow it.
        if exe_ini.as_deref().is_none_or(|path| !path.exists()) {
            return fs::write("drotrim.ini", &text)
                .map_err(|error| Error::config(format!("Could not write drotrim.ini: {error}")));
        }
        Err(Error::config(
            "Could not write drotrim.ini next to the executable.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_falls_back_to_defaults() {
        // The test working directory has no drotrim.ini; the exe dir (target/)
        // does not either. Either way this must not fail.
        let config = IniConfigStore::new().load();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn saved_ini_round_trips_through_the_parser() {
        let mut config = AppConfig::default();
        config.audio.frequency = 49_716;
        config.ui.tail_length = 4500;
        // The booleans write as Rust's "true"/"false" and are read back by a
        // parser that also accepts yes/on/1 -- worth pinning, since a setting
        // that applies live but evaporates on restart looks like it never worked.
        config.ui.dro_info_edit_enabled = true;
        config.ui.maximize_window = true;
        let text = config.to_ini_string();
        assert_eq!(AppConfig::from_ini_sources(&[&text]), config);
    }
}
