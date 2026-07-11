//! Loading `drotrim.ini` for the native tools.
//!
//! The web build reads `localStorage` instead (Step 8); both hand text to
//! [`AppConfig::from_ini_sources`]. A proper `ConfigStore` implementation arrives
//! with the GUI in Step 6 -- the CLI tools only need to read.

use std::fs;

use dro_core::config::AppConfig;

/// Reads `drotrim.ini` from the working directory then the executable's
/// directory, the later overriding the earlier -- the Python lookup order.
/// Missing or malformed files fall back to the defaults.
#[must_use]
pub fn load_config() -> AppConfig {
    let mut sources: Vec<String> = Vec::new();
    if let Ok(text) = fs::read_to_string("drotrim.ini") {
        sources.push(text);
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    {
        if let Ok(text) = fs::read_to_string(dir.join("drotrim.ini")) {
            sources.push(text);
        }
    }
    let sources: Vec<&str> = sources.iter().map(String::as_str).collect();
    AppConfig::from_ini_sources(&sources)
}
