//! Shared logic for the native binaries: the `dro_player`, `dro2to1` and
//! `dro_split` CLI tools now, and the `drotrim` GUI shell in Step 6.

pub mod config;
pub mod split;

pub use config::load_config;
pub use split::{SplitData, SplitFormat, SplitOptions, SplitOutput, split};
