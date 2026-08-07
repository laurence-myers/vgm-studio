//! The loaded document, handed to whatever needs it.
//!
//! A job that renders, plays, loop-searches or splits a document does not care
//! which format it came from -- it cares only "here is the document, in the shape
//! I can work with". That shape is one of two: a DRO [`DroSong`], or a [`VgmFile`]
//! (any VGM, OPL or not). This one type carries either, so the audio backend, the
//! WAV render, the loop search and the song split share it rather than each
//! declaring its own identical pair.
//!
//! It lives here, in the permissive core, so `vgms-synth`'s public API can take
//! it (as `AudioSource`) without either crate growing a second copy of the type.

use std::sync::Arc;

use crate::song::DroSong;
use crate::split_songs::{Segment, detect_segments, detect_segments_in_vgm, native_rate};
use crate::util::VGM_SAMPLE_RATE;
use crate::vgm::VgmFile;

/// The loaded document as whichever of the two shapes a job needs.
///
/// The `Dro` arm carries the DRO the document is; the `Vgm` arm carries the file
/// itself (any VGM, OPL or not). Both wrap an [`Arc`] so handing the document to a
/// background job is a reference-count bump, not a copy.
#[derive(Debug, Clone)]
pub enum DocSource {
    Dro(Arc<DroSong>),
    Vgm(Arc<VgmFile>),
}

impl DocSource {
    /// The document's name -- for logs and errors, and the name a render or split
    /// piece is offered under.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Dro(song) => &song.name,
            Self::Vgm(file) => &file.name,
        }
    }

    /// The DRO song, when the document is one; `None` for any VGM (OPL included,
    /// since an OPL VGM travels as the `Vgm` arm). For "does this document belong
    /// on the OPL board" ask [`Self::is_opl`] instead, which an OPL VGM answers
    /// `true`.
    #[must_use]
    pub fn dro(&self) -> Option<&Arc<DroSong>> {
        match self {
            Self::Dro(song) => Some(song),
            Self::Vgm(_) => None,
        }
    }

    /// Whether this document is OPL -- a DRO, or an OPL VGM. The hardware-routing
    /// decision keys on this rather than [`Self::dro`], because an OPL VGM travels
    /// as the `Vgm` arm (its [`Self::dro`] is `None`) yet still belongs on the OPL
    /// board, which the RetroWave service drives from the `VgmFile`.
    #[must_use]
    pub fn is_opl(&self) -> bool {
        match self {
            Self::Dro(_) => true,
            Self::Vgm(file) => file.is_opl(),
        }
    }

    /// Delay units per second in this capture's native unit: 44100 for a VGM
    /// (samples), 1000 for a DRO (milliseconds). Lets a UI convert the native-unit
    /// [`Segment`] fields and a gap threshold to and from seconds.
    #[must_use]
    pub fn rate(&self) -> u32 {
        match self {
            Self::Dro(song) => native_rate(song),
            Self::Vgm(_) => VGM_SAMPLE_RATE,
        }
    }

    /// The songs in the capture at `threshold` native units -- the OPL detector
    /// over a `DroSong`, the chip-generic one over a `VgmFile`.
    #[must_use]
    pub fn detect(&self, threshold: u32) -> Vec<Segment> {
        match self {
            Self::Dro(song) => detect_segments(song, threshold),
            Self::Vgm(file) => detect_segments_in_vgm(file, threshold),
        }
    }

    /// The file name each split piece is numbered against, and its extension.
    #[must_use]
    pub fn stem_and_extension(&self) -> (&str, &'static str) {
        let (name, extension) = match self {
            Self::Dro(song) => (song.name.as_str(), "dro"),
            Self::Vgm(file) => (file.name.as_str(), "vgm"),
        };
        let stem = name
            .rsplit_once('.')
            .map_or(name, |(stem, _extension)| stem);
        (stem, extension)
    }
}
