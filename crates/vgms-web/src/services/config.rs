// SPDX-License-Identifier: GPL-2.0-or-later
//! [`LocalStorageStore`]: the web build's [`ConfigStore`], over `localStorage`.
//!
//! The settings live as the same INI text the native build writes to
//! `vgmstudio.ini`, under one `localStorage` key. `AppConfig` already round-trips
//! through that text and already tolerates a core id this build has never heard
//! of, so the web and native configs are interchangeable byte for byte.

use vgms_core::config::{AppConfig, ConfigStore};
use vgms_core::error::{Error, Result};

/// The `localStorage` key the settings INI lives under.
const KEY: &str = "vgmstudio.ini";

/// Reads and writes the settings as INI text in `localStorage`.
#[derive(Debug, Default)]
pub struct LocalStorageStore;

impl LocalStorageStore {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The page's `localStorage`, or `None` if the browser withholds it (private
    /// mode with storage disabled, say). A missing store is not an error: the app
    /// runs on defaults and simply cannot persist.
    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }
}

impl ConfigStore for LocalStorageStore {
    fn load(&self) -> AppConfig {
        match Self::storage().and_then(|storage| storage.get_item(KEY).ok().flatten()) {
            Some(ini) => AppConfig::from_ini_sources(&[&ini]),
            None => AppConfig::default(),
        }
    }

    fn save(&self, config: &AppConfig) -> Result<()> {
        let ini = config.to_ini_string();
        let storage =
            Self::storage().ok_or_else(|| Error::config("localStorage is unavailable"))?;
        storage
            .set_item(KEY, &ini)
            .map_err(|_| Error::config("could not write settings to localStorage"))
    }
}
