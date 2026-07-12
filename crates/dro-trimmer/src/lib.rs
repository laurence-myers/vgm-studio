//! Shared logic for the native binaries: the `dro_player`, `dro2to1` and
//! `dro_split` CLI tools, and the platform services behind the `drotrim` GUI.

pub mod config;
pub mod services;
pub mod split;

pub use config::load_config;
pub use split::{SplitData, SplitFormat, SplitOptions, SplitOutput, split};
