//! VGM and VGZ: the command stream, the GD3 tag, and the container.

pub mod data;
pub mod file;
pub mod header;
pub mod io;
pub mod stream;

pub use crate::util::VGM_SAMPLE_RATE;
pub use data::{Gd3Tag, VgmData, VgmMeta};
pub use file::{VgmBody, VgmFile};
pub use header::{ChipKind, ChipSettings, ChipUse, ExtraHeader, VgmHeader};
pub use stream::{ChipTarget, VgmCommand, VgmStream};
