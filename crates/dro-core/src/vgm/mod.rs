//! VGM and VGZ: the command stream, the GD3 tag, and the container.

pub mod audit;
pub mod data;
pub mod file;
pub mod header;
pub mod io;
pub mod projection;
pub mod stream;

pub use crate::util::VGM_SAMPLE_RATE;
pub use audit::HeaderFinding;
pub use data::{Gd3Tag, VgmData, VgmMeta};
pub use file::{RegionReport, VgmBody, VgmFile};
pub use header::{ChipKind, ChipSettings, ChipUse, ExtraHeader, VgmHeader};
pub use projection::{OplProjection, opl_type_of};
pub use stream::{ChipTarget, VgmCommand, VgmStream};
