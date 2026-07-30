//! Loading `drotrim.ini` for the native tools, which only need to read; text is
//! handed to [`AppConfig::from_ini_sources`].

use std::fs;

use vgms_core::config::AppConfig;

/// Reads `drotrim.ini` from the working directory then the executable's
/// directory, the later overriding the earlier.
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
        && let Ok(text) = fs::read_to_string(dir.join("drotrim.ini"))
    {
        sources.push(text);
    }
    let sources: Vec<&str> = sources.iter().map(String::as_str).collect();
    AppConfig::from_ini_sources(&sources)
}
