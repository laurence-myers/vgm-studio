//! Pack mode's headless core: the loaded folder, the editable package metadata,
//! the derived track list and the readiness model, with no egui. Testable
//! without a window, like [`crate::editor::Editor`]. [`super::view`] draws it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use vgms_core::pack::naming::doc_file_stem;
use vgms_core::pack::readiness::{
    MetaField, ReadinessCategory, ReadinessItem, ReadinessTarget, Severity, TrackFacts, readiness,
};
use vgms_core::pack::{
    DEFAULT_OS, DEFAULT_SYSTEM, PackMeta, PngInfo, TrackEntry, generate_description, generate_m3u,
    music_hardware_suggestion, parse_description,
};
use vgms_core::{Gd3Tag, OplType, VgmFile};
use vgms_synth::AudioSource;
use vgms_synth::Peak;

use crate::platform::{PackEntry, PackEntryKind, PackJobRequest, PickedFile, PickedFolder};

/// What a scanned song file turned out to be.
///
/// There is one kind of track, because a pack is a folder of VGMs -- that is
/// what a VGMRips submission is, and what [`classify`] scans for. A VGM is a
/// VGM whatever chips it declares; whether those chips also unlock the OPL
/// extras is [`PackTrack::opl_type`]'s question, asked per feature, not a
/// different kind of track.
#[derive(Debug, Clone)]
pub enum PackSong {
    /// A VGM, for any chips at all.
    Vgm(Arc<VgmFile>),
    /// Not a VGM this app can read, with the reason.
    Unreadable(String),
}

/// One song file in the pack: its bytes (kept for export and opening in the
/// editor) and what parsing made of it (a failure shows inline rather than
/// aborting the scan).
#[derive(Debug, Clone)]
pub struct PackTrack {
    pub file_name: String,
    pub path: Option<PathBuf>,
    pub bytes: Vec<u8>,
    pub song: PackSong,
    /// The table entry (title, durations) computed once at scan, rather than
    /// re-summing the whole song per row per frame. `Some` iff the file parsed.
    pub entry: Option<TrackEntry>,
}

impl PackTrack {
    /// The parsed VGM, if the file parsed.
    #[must_use]
    pub fn vgm(&self) -> Option<&Arc<VgmFile>> {
        match &self.song {
            PackSong::Vgm(file) => Some(file),
            PackSong::Unreadable(_) => None,
        }
    }

    /// Whether the file parsed at all.
    #[must_use]
    pub const fn is_readable(&self) -> bool {
        !matches!(self.song, PackSong::Unreadable(_))
    }

    /// Why the file could not be read, if it could not.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        match &self.song {
            PackSong::Unreadable(error) => Some(error),
            PackSong::Vgm(_) => None,
        }
    }

    /// The OPL this track's chips are, or `None` when the OPL-only features do
    /// not apply to it.
    ///
    /// The one question every OPL feature asks.
    #[must_use]
    pub fn opl_type(&self) -> Option<OplType> {
        Some(self.vgm()?.opl()?.opl_type())
    }

    /// Whether this app can make a sound from the track: an OPL stream, or a
    /// chip it has a core for.
    ///
    /// Asks [`preview_source`](Self::preview_source)'s question without building
    /// its answer. **The table asks this once per row per frame**, and
    /// `preview_source` materialises a whole [`DroSong`](vgms_core::DroSong) -- a copy
    /// of every command byte and every offset in the file -- to say yes. A pack
    /// of 38 OPL rips spent 9 ms of every frame on that in release and 150 ms in
    /// a dev build, which is the whole of the Tracks view's slowdown.
    /// `the_two_playability_answers_agree` pins the two against each other.
    #[must_use]
    pub fn is_playable(&self) -> bool {
        let Some(file) = self.vgm() else {
            return false;
        };
        // An OPL file is playable; this is the same branch `preview_source`
        // takes -- minus the snapshot.
        if file.opl().is_some() {
            return true;
        }
        vgms_synth::playability(&chip_kinds(file)).can_play()
    }

    /// The track's chips this app has no core for, as a comma-separated list.
    ///
    /// Empty when it can play all of them -- which for an OPL track is always,
    /// and for a Mega Drive rip is not: the PSG plays and the FM does not, so
    /// the preview is worth offering and worth labelling.
    #[must_use]
    pub fn chips_without_cores(&self) -> String {
        let Some(file) = self.vgm() else {
            return String::new();
        };
        if file.is_opl() {
            return String::new();
        }
        vgms_synth::playability(&chip_kinds(file))
            .missing()
            .iter()
            .map(|chip| chip.name())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The track as something an engine can play, or `None` when nothing would
    /// be heard.
    ///
    /// The track as something an engine can play -- always its whole VGM file
    /// now, OPL or not (Stage K): an OPL track plays through the same VgmEngine
    /// path as any other, its per-channel panning coming from the OPL core. A
    /// track whose chips all lack cores is not offered, because a preview button
    /// that plays silence is worse than one that is not there.
    #[must_use]
    pub fn preview_source(&self) -> Option<AudioSource> {
        let file = self.vgm()?;
        vgms_synth::playability(&chip_kinds(file))
            .can_play()
            .then(|| AudioSource::Vgm(Arc::clone(file)))
    }

    /// Whether the editor can open the track: it needs rows to show.
    #[must_use]
    pub fn is_editable(&self) -> bool {
        self.vgm().is_some_and(|file| file.stream().is_some())
    }

    /// The track's GD3 tag, if it has one.
    #[must_use]
    pub fn tag(&self) -> Option<&Gd3Tag> {
        self.vgm()?.tag.as_ref()
    }

    /// The chips this track declares, e.g. `"YM2612, SN76489"`, or `None` if it
    /// did not parse.
    #[must_use]
    pub fn chip_list(&self) -> Option<String> {
        Some(self.vgm()?.chip_list())
    }

    /// The track's VGM volume modifier, if it is a VGM.
    #[must_use]
    pub fn volume_modifier(&self) -> Option<u8> {
        Some(self.vgm()?.header.volume_modifier())
    }

    /// Re-serialises the track under `new_name` with `tag` applied. The name
    /// drives the output format, so a `.vgm` -> `.vgz` rename gzips the result.
    ///
    /// `None` when the file did not parse.
    #[must_use]
    pub fn retagged(&self, new_name: &str, tag: Gd3Tag) -> Option<Result<Vec<u8>, String>> {
        let mut file = VgmFile::clone(self.vgm()?);
        file.name = new_name.to_owned();
        file.tag = Some(tag);
        Some(write_vgm(&file))
    }

    /// Re-serialises the track with its VGM volume modifier set to `modifier`,
    /// keeping the file name (and thus the format). `None` if it is not a VGM.
    #[must_use]
    pub fn revolumed(&self, modifier: u8) -> Option<Result<Vec<u8>, String>> {
        let mut file = VgmFile::clone(self.vgm()?);
        // A header too short to hold the field predates it; there is nowhere to
        // write the modifier, so the track is left alone.
        file.header
            .set_volume_modifier(modifier)
            .then(|| write_vgm(&file))
    }
}

/// The chips a file declares, as the kinds [`vgms_synth::playability`] takes.
fn chip_kinds(file: &VgmFile) -> Vec<vgms_core::vgm::ChipKind> {
    file.header.chips().iter().map(|chip| chip.kind).collect()
}

/// Writes a VGM, gzipping when its name says `.vgz`.
fn write_vgm(file: &VgmFile) -> Result<Vec<u8>, String> {
    let result = if file.name.to_ascii_lowercase().ends_with(".vgz") {
        vgms_core::vgm::file::write_gzipped(file)
    } else {
        vgms_core::vgm::file::write(file)
    };
    result.map_err(|error| error.to_string())
}

/// A screenshot in the pack folder. Its bytes are shared (`Arc<[u8]>`) so the
/// inline preview's per-frame `Image::from_bytes` clone is an Arc bump, not a
/// full copy of the PNG.
#[derive(Debug, Clone)]
pub struct PackImage {
    pub name: String,
    pub path: Option<PathBuf>,
    pub bytes: Arc<[u8]>,
    /// The PNG header's facts, read once at scan rather than per frame. `None`
    /// when the file is not a readable PNG, which the inspector reports.
    pub info: Option<PngInfo>,
}

/// Which sub-section of the pack view is open. The pack is four separate jobs --
/// describe it, order and level the tracks, check the screenshot, work the
/// checklist -- and stacking all four on one scrolling page meant the one being
/// worked on was rarely the one on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PackSection {
    /// The package metadata form.
    #[default]
    Tags,
    /// The track list, with the batch tools that act on it.
    Tracks,
    /// The screenshots that ship with the pack.
    Screenshots,
    /// Every submission-readiness finding.
    Checklist,
}

impl PackSection {
    /// Every section, in strip order.
    pub const ALL: [Self; 4] = [Self::Tags, Self::Tracks, Self::Screenshots, Self::Checklist];

    /// The tab label naming this section.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Tags => "Tags",
            Self::Tracks => "Tracks",
            Self::Screenshots => "Screenshots",
            Self::Checklist => "Checklist",
        }
    }
}

/// What a verified per-track optimise made of a song, for the Tracks table's
/// savings column beside Peak. Kept per file name on [`PackState`], the same
/// cheap-per-frame home the Peak column's [`PackState::peaks`] uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackOptimizeStatus {
    /// Shrank on disk and verified identical: the sizes before and after.
    Saved { from: usize, to: usize },
    /// Nothing to gain on disk -- already optimal.
    Optimal,
    /// Kept the original because the render differed.
    KeptDiffered,
    /// Kept the original because the result could not be verified, or the pass
    /// itself could not run.
    Unverifiable,
}

impl TrackOptimizeStatus {
    /// The savings column's short label, e.g. `-19.6%`, `optimal`, `kept`.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Saved { from, to } if from > 0 => {
                let saved = from.saturating_sub(to);
                format!("-{:.1}%", saved as f64 * 100.0 / from as f64)
            }
            Self::Saved { .. } => "-".to_owned(),
            Self::Optimal => "optimal".to_owned(),
            Self::KeptDiffered => "kept".to_owned(),
            Self::Unverifiable => "kept".to_owned(),
        }
    }

    /// The full sentence for the cell's hover tooltip.
    #[must_use]
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Saved { .. } => "Optimized and verified: renders identically.",
            Self::Optimal => "Already optimal: nothing to gain.",
            Self::KeptDiffered => "Kept the original: the optimized file rendered differently.",
            Self::Unverifiable => "Kept the original: the optimized file could not be verified.",
        }
    }
}

/// The whole pack project: what a folder scan produced, plus the editable
/// package metadata.
#[derive(Debug)]
pub struct PackState {
    pub folder_name: String,
    pub folder_path: Option<PathBuf>,
    /// Where the files live: a writable directory, or an in-memory zip archive
    /// that needs an explicit Save Pack (wt-8). Set by the app when the folder is
    /// opened; preserved across an in-place rescan.
    pub origin: crate::platform::PackOrigin,
    pub meta: PackMeta,
    pub tracks: Vec<PackTrack>,
    pub images: Vec<PackImage>,
    /// The description file that was parsed, if any.
    pub description_file: Option<String>,
    /// Set when an existing description could not be parsed; saving overwrites it.
    pub parse_warning: Option<String>,
    /// Unsaved edits to the package metadata.
    pub dirty: bool,
    /// Gzip `.vgm` songs to `.vgz` on export (the VGMRips convention).
    pub gzip_on_export: bool,
    /// Strip redundant register writes from each VGM on export (`vgm_cmp`, the
    /// final step of the VGMRips optimisation pipeline).
    pub optimize_on_export: bool,
    /// The row currently previewing through the audio output (pack mode playback).
    pub preview: Option<usize>,
    /// Measured peak per track, keyed by `file_name` (the stable identity that
    /// survives a rescan/reorder). Filled by "Scan Volumes"; drives the Peak
    /// column and the suggested modifiers.
    pub peaks: HashMap<String, Peak>,
    /// The result of a verified per-track optimise, keyed by `file_name`. Drives
    /// the Tracks table's savings column. Kept across a rescan (the optimise
    /// rewrites the file, so a peaks-style audio-equality prune would erase its
    /// own record); cleared only when the pack is reopened or a track is
    /// re-optimised.
    pub optimize_results: HashMap<String, TrackOptimizeStatus>,
    /// Per-track optimiser options, keyed by `file_name`, overriding the global
    /// Settings default for that track's Optimize. A user intent (not an audio
    /// fact), so like [`Self::optimize_results`] it is kept across a rescan.
    pub track_optimize_overrides: HashMap<String, vgms_core::config::OptimizeOptions>,
    /// Whether "Apply suggested modifiers" levels the whole pack by its loudest
    /// track (album mode, the VGMRips convention) rather than normalising each
    /// track to its own peak.
    pub album_normalize: bool,
    /// A metadata field the submission checklist asked to focus. The form
    /// consumes it on the next frame (scrolling to and focusing the field), so a
    /// checklist click can jump straight to the offending input.
    pub focus_field: Option<MetaField>,
    /// The sub-section on screen.
    pub section: PackSection,
    /// Whether the system / OS / music-hardware fields are unfolded for editing.
    pub show_hardware: bool,
    /// Which checklist categories are folded away, by their index in
    /// [`ReadinessCategory::ALL`].
    pub collapsed: [bool; ReadinessCategory::ALL.len()],
    /// The row the keyboard acts on: set by clicking a track, carried along by
    /// an Alt+arrow reorder so the keys can be held down, and dropped as soon as
    /// the pointer moves -- whichever the user last touched is in charge.
    pub focused_track: Option<usize>,
    /// A row the table scrolls into view on the next frame, so a track moved by
    /// the keyboard cannot walk off the top or bottom of the view.
    pub scroll_to_track: Option<usize>,
}

impl PackState {
    /// Builds the state from a scanned folder. `today` prefills the initial
    /// package-history line when there is no description to parse.
    #[must_use]
    pub fn from_folder(folder: PickedFolder, today: Option<(i32, u8, u8)>) -> Self {
        let mut tracks = Vec::new();
        let mut images = Vec::new();
        let mut texts: Vec<PickedFile> = Vec::new();
        for file in folder.files {
            match classify(&file.name) {
                FileClass::Song => {
                    let (song, entry) = read_track(&file.name, &file.bytes);
                    tracks.push(PackTrack {
                        file_name: file.name,
                        path: file.path,
                        bytes: file.bytes,
                        song,
                        entry,
                    });
                }
                FileClass::Image => images.push(PackImage {
                    info: PngInfo::parse(&file.bytes),
                    name: file.name,
                    path: file.path,
                    bytes: file.bytes.into(),
                }),
                FileClass::Doc => texts.push(file),
                FileClass::Other => {}
            }
        }

        let chosen = choose_description(&texts, &folder.name);
        let (meta, description_file, parse_warning) = match chosen {
            Some(file) => {
                let text = String::from_utf8_lossy(&file.bytes);
                match parse_description(&text) {
                    Ok(meta) => (meta, Some(file.name.clone()), None),
                    Err(error) => (
                        prefilled(&tracks, today),
                        Some(file.name.clone()),
                        Some(format!("{} could not be parsed: {error}", file.name)),
                    ),
                }
            }
            None => (prefilled(&tracks, today), None, None),
        };

        Self {
            folder_name: folder.name,
            folder_path: folder.path,
            // The app overrides this for a zip-opened pack right after building.
            origin: crate::platform::PackOrigin::default(),
            meta,
            tracks,
            images,
            description_file,
            parse_warning,
            dirty: false,
            gzip_on_export: true,
            optimize_on_export: true,
            preview: None,
            peaks: HashMap::new(),
            optimize_results: HashMap::new(),
            track_optimize_overrides: HashMap::new(),
            album_normalize: true,
            focus_field: None,
            section: PackSection::default(),
            show_hardware: false,
            collapsed: [false; ReadinessCategory::ALL.len()],
            focused_track: None,
            scroll_to_track: None,
        }
    }

    /// Re-scans the folder's files, keeping the edited metadata and dirty flag.
    /// Used after returning from the editor or renaming a track.
    pub fn refresh_files(&mut self, folder: PickedFolder) {
        // Remember which file is previewing so the rescan (which can reorder the
        // track list) keeps the marker on the same track by name rather than
        // dropping playback on an in-place refresh -- e.g. after a screenshot
        // optimise redelivers the folder. Cleared if that track is now gone.
        let previewing = self
            .preview
            .and_then(|index| self.tracks.get(index))
            .map(|track| track.file_name.clone());
        let rescanned = Self::from_folder(folder, None);
        let old_tracks = std::mem::replace(&mut self.tracks, rescanned.tracks);
        self.images = rescanned.images;
        self.preview = previewing
            .and_then(|name| self.tracks.iter().position(|track| track.file_name == name));
        // A measured peak is a fact about the *audio* -- the command stream and
        // chip -- so it survives a rescan only while those are unchanged. Header
        // rewrites (a new volume modifier, GD3 tags) keep it; a track whose audio
        // was edited, renamed away, or replaced loses it and shows unscanned.
        let audio_of = |tracks: &[PackTrack], name: &str| {
            let track = tracks.iter().find(|track| track.file_name == name)?;
            Some((track.opl_type(), track.vgm()?.body.raw().to_vec()))
        };
        self.peaks.retain(|name, _| {
            match (audio_of(&old_tracks, name), audio_of(&self.tracks, name)) {
                (Some(old), Some(new)) => old == new,
                _ => false,
            }
        });
        // A keyboard reorder finishes here, several frames after the key: the
        // renames run, then the folder is rescanned, and only then is the moved
        // track at its new index. Re-arm the scroll so it is *this* list the row
        // is brought into view in -- the request made at key-press time was
        // spent on the list as it was before the move.
        self.scroll_to_track = self.focused_track.filter(|row| *row < self.tracks.len());
    }

    /// The optimiser options for `file_name`: its per-track override if one is
    /// set, else the given global default.
    #[must_use]
    pub fn effective_optimize_options(
        &self,
        file_name: &str,
        global: vgms_core::config::OptimizeOptions,
    ) -> vgms_core::config::OptimizeOptions {
        self.track_optimize_overrides
            .get(file_name)
            .copied()
            .unwrap_or(global)
    }

    /// The transaction that moves the track at `from` to `to`, renumbering the
    /// affected files' names. `None` when nothing would change or the folder has
    /// no path (the web has none, and pack mode is native-only anyway).
    #[must_use]
    pub fn reorder_transaction(&self, from: usize, to: usize) -> Option<PackTransaction> {
        let folder = self.folder_path.as_ref()?;
        let names: Vec<String> = self
            .tracks
            .iter()
            .map(|track| track.file_name.clone())
            .collect();
        let pairs = reorder_renames(&names, from, to);
        if pairs.is_empty() {
            return None;
        }
        let inverse_pairs: Vec<(String, String)> =
            pairs.iter().map(|(a, b)| (b.clone(), a.clone())).collect();
        Some(PackTransaction {
            label: "Reorder tracks".to_owned(),
            forward: rename_batch_mutations(folder, &pairs),
            inverse: rename_batch_mutations(folder, &inverse_pairs),
        })
    }

    /// The transaction that sets each scanned track's VGM volume modifier to
    /// level the pack: album mode (one factor from the loudest peak, the VGMRips
    /// convention) unless [`Self::album_normalize`] is off, when each track is
    /// normalised to its own peak.
    ///
    /// `None` when nothing would change -- no peaks scanned yet, or every track's
    /// modifier already matches its suggestion. Tracks with no peak, no path, or
    /// that are not VGMs are skipped, so the batch touches only what it must.
    #[must_use]
    pub fn suggested_modifier_transaction(&self) -> Option<PackTransaction> {
        let album_peak = self.peaks.values().map(|peak| peak.max_level).max()?;
        let album = self.album_normalize.then_some(album_peak);
        let mut forward = Vec::new();
        let mut inverse = Vec::new();
        for track in &self.tracks {
            let (Some(path), Some(peak)) = (track.path.clone(), self.peaks.get(&track.file_name))
            else {
                continue;
            };
            // Only VGMs carry a volume modifier; the pack list is VGM/VGZ only, but
            // guard so a non-VGM can never be rewritten.
            let Some(current) = track.volume_modifier() else {
                continue;
            };
            let new_modifier = vgms_core::suggest_volume_modifier(peak.max_level, album);
            if new_modifier == current {
                continue; // nothing changed for this track
            }
            if let Some(Ok(bytes)) = track.revolumed(new_modifier) {
                forward.push(PackMutation::Write {
                    path: path.clone(),
                    bytes,
                });
                inverse.push(PackMutation::Write {
                    path,
                    bytes: track.bytes.clone(),
                });
            }
        }
        if forward.is_empty() {
            return None;
        }
        let count = forward.len();
        Some(PackTransaction {
            label: format!(
                "Set volume modifier on {count} track{}",
                if count == 1 { "" } else { "s" }
            ),
            forward,
            inverse,
        })
    }

    /// Whether any pack or track release date is a slash-separated date the
    /// "Convert dates to hyphens" fix-assist could rewrite (see
    /// [`vgms_core::pack::readiness::hyphenate_date`]).
    #[must_use]
    pub fn has_convertible_dates(&self) -> bool {
        vgms_core::pack::readiness::hyphenate_date(&self.meta.release_date).is_some()
            || self.tracks.iter().any(|track| {
                track.tag().is_some_and(|tag| {
                    vgms_core::pack::readiness::hyphenate_date(&tag.release_date).is_some()
                })
            })
    }

    /// Converts the pack's release date from slashes to hyphens in place, if it is
    /// a convertible slash date. Returns whether it changed (and marks the pack
    /// dirty). A pack-metadata edit, like typing in the form -- not a file op.
    pub fn hyphenate_meta_date(&mut self) -> bool {
        if let Some(hyphenated) =
            vgms_core::pack::readiness::hyphenate_date(&self.meta.release_date)
        {
            self.meta.release_date = hyphenated;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// The transaction that rewrites every track's GD3 release date from slashes
    /// to hyphens (`1994/03/01` -> `1994-03-01`), skipping tracks whose date needs
    /// no change. `None` when nothing would change. Mirrors
    /// [`Self::suggested_modifier_transaction`]: per-track `Write` mutations with
    /// undo for free.
    #[must_use]
    pub fn date_hyphenation_transaction(&self) -> Option<PackTransaction> {
        let mut forward = Vec::new();
        let mut inverse = Vec::new();
        for track in &self.tracks {
            let (Some(tag), Some(path)) = (track.tag().cloned(), track.path.clone()) else {
                continue;
            };
            let Some(hyphenated) = vgms_core::pack::readiness::hyphenate_date(&tag.release_date)
            else {
                continue;
            };
            let mut new_tag = tag;
            new_tag.release_date = hyphenated;
            if let Some(Ok(bytes)) = track.retagged(&track.file_name, new_tag) {
                forward.push(PackMutation::Write {
                    path: path.clone(),
                    bytes,
                });
                inverse.push(PackMutation::Write {
                    path,
                    bytes: track.bytes.clone(),
                });
            }
        }
        if forward.is_empty() {
            return None;
        }
        let count = forward.len();
        Some(PackTransaction {
            label: format!(
                "Convert {count} track date{} to hyphens",
                if count == 1 { "" } else { "s" }
            ),
            forward,
            inverse,
        })
    }

    /// The file name each track should carry, from its GD3 Track Name and its
    /// 1-based position, paired with the name it has now -- omitting the tracks
    /// already named correctly. See [`vgms_core::pack::naming::tag_file_name`]: the title
    /// goes through `vgm_ren`'s replacements, so this agrees with the file-name
    /// check by construction.
    ///
    /// Unreadable and untagged tracks are skipped (there is nothing to derive a
    /// name from), as is a title `vgm_ren` would empty out entirely.
    #[must_use]
    pub fn tag_renames(&self) -> Vec<(String, String)> {
        self.tracks
            .iter()
            .enumerate()
            .filter_map(|(index, track)| {
                let wanted = wanted_file_name(index, track)?;
                (wanted != track.file_name).then(|| (track.file_name.clone(), wanted))
            })
            .collect()
    }

    /// Whether any track's file name has drifted from its tag -- what greys the
    /// "Fix File Names" tool out once there is nothing left to rename.
    #[must_use]
    pub fn has_tag_renames(&self) -> bool {
        self.tracks.iter().enumerate().any(|(index, track)| {
            wanted_file_name(index, track).is_some_and(|wanted| wanted != track.file_name)
        })
    }

    /// The transaction that renames every drifted file to the name its tag asks
    /// for, as one undoable step. `None` when nothing would change or the folder
    /// has no path (the web has none, and pack mode is native-only anyway).
    ///
    /// Every target is `NN Title.ext` for a distinct `NN`, so the batch can never
    /// collide -- and it goes through [`rename_batch_mutations`] regardless, which
    /// stages via temp names and so survives swaps and case-only renames.
    #[must_use]
    pub fn rename_from_tags_transaction(&self) -> Option<PackTransaction> {
        let folder = self.folder_path.as_ref()?;
        let pairs = self.tag_renames();
        if pairs.is_empty() {
            return None;
        }
        let inverse_pairs: Vec<(String, String)> =
            pairs.iter().map(|(a, b)| (b.clone(), a.clone())).collect();
        let count = pairs.len();
        Some(PackTransaction {
            label: format!(
                "Rename {count} file{} from their tags",
                if count == 1 { "" } else { "s" }
            ),
            forward: rename_batch_mutations(folder, &pairs),
            inverse: rename_batch_mutations(folder, &inverse_pairs),
        })
    }

    /// The track list for the description, skipping songs that failed to parse.
    #[must_use]
    pub fn track_entries(&self) -> Vec<TrackEntry> {
        self.tracks
            .iter()
            .filter_map(|track| track.entry.clone())
            .collect()
    }

    /// The file-name stem for the `.txt`/`.m3u`/`.zip`, from the game name.
    #[must_use]
    pub fn doc_stem(&self) -> String {
        doc_file_stem(&self.meta.game_name)
    }

    /// The generated description file text.
    #[must_use]
    pub fn description_text(&self) -> String {
        generate_description(&self.meta, &self.track_entries())
    }

    /// The generated playlist. `final_names` swaps `.vgm` for `.vgz` when the
    /// export gzips songs, so the zip's playlist names its actual entries.
    #[must_use]
    pub fn m3u_text(&self, final_names: bool) -> String {
        let names: Vec<String> = self
            .tracks
            .iter()
            .map(|track| {
                if final_names && self.gzip_on_export {
                    to_vgz_name(&track.file_name)
                } else {
                    track.file_name.clone()
                }
            })
            .collect();
        generate_m3u(&names)
    }

    /// Whether the metadata is ready to save: a game name is required, and one
    /// the file-name rules leave something of, since it names every output file.
    #[must_use]
    pub fn can_save(&self) -> bool {
        !self.doc_stem().is_empty()
    }

    /// The per-track facts the submission-readiness checks read, one entry per
    /// track in list order -- so a readiness `Track(index)` maps straight back to
    /// `self.tracks[index]` for click-to-fix.
    #[must_use]
    pub fn track_facts(&self) -> Vec<TrackFacts> {
        self.tracks
            .iter()
            .map(|track| TrackFacts {
                file_name: track.file_name.clone(),
                tag: track.tag().cloned(),
                loops: track
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.loop_samples.is_some()),
                readable: track.is_readable(),
            })
            .collect()
    }

    /// The distinct chip sets this app cannot preview, in track order.
    ///
    /// A track for chips there is no core for is unplayable by definition -- it means no
    /// core exists for its chips yet. Listing the chips rather than the tracks
    /// keeps the note short for a pack where every track is the same hardware,
    /// which is nearly all of them.
    #[must_use]
    pub fn silent_chips(&self) -> Vec<String> {
        let mut chips: Vec<String> = Vec::new();
        for track in &self.tracks {
            if !track.is_readable() {
                continue;
            }
            let missing = track.chips_without_cores();
            if !missing.is_empty() && !chips.contains(&missing) {
                chips.push(missing);
            }
        }
        chips
    }

    /// Every submission-readiness finding, in checklist order: this app's own
    /// file-level shape checks (readable songs, `NN Title` numbering, screenshot,
    /// game name) followed by the shared, wasm-clean GD3 / metadata content
    /// checks from [`vgms_core::pack::readiness::readiness`]. One list feeds both the export
    /// gate ([`Self::validations`]) and the submission checklist, so they can
    /// never disagree.
    #[must_use]
    pub fn readiness_items(&self) -> Vec<ReadinessItem> {
        let mut items = Vec::new();

        if self.doc_stem().is_empty() {
            // The game name is package info (and the only file-level hard error).
            // Judged by the stem it yields, not the raw text: a name the file
            // rules empty out ("?!") names the pack ".zip".
            items.push(ReadinessItem {
                severity: Severity::Error,
                category: ReadinessCategory::PackInfo,
                target: ReadinessTarget::Meta(MetaField::GameName),
                message: crate::strings::PACK_CHECK_GAME_NAME.to_owned(),
            });
        }
        let readable = self
            .tracks
            .iter()
            .filter(|track| track.is_readable())
            .count();
        if readable == 0 {
            items.push(file_item(
                Severity::Error,
                crate::strings::PACK_CHECK_NO_READABLE.to_owned(),
            ));
        }
        if self.tracks.iter().any(|track| !track.is_readable()) {
            items.push(file_item(
                Severity::Warning,
                crate::strings::PACK_CHECK_UNREADABLE_FILES.to_owned(),
            ));
        }
        // Not an error, and not the checklist's business to block: the pack is
        // perfectly submittable, this app just cannot make every sound in it.
        let silent = self.silent_chips();
        if !silent.is_empty() {
            items.push(file_item(
                Severity::Note,
                crate::strings::pack_check_playback(&silent.join(", ")),
            ));
        }
        if self.images.is_empty() {
            items.push(file_item(
                Severity::Warning,
                crate::strings::PACK_CHECK_NO_SCREENSHOT.to_owned(),
            ));
        }

        let numbers: Vec<u32> = self
            .tracks
            .iter()
            .filter_map(|track| track_number(&track.file_name))
            .collect();
        if numbers.len() != self.tracks.len() {
            items.push(file_item(
                Severity::Warning,
                crate::strings::PACK_CHECK_NAMING.to_owned(),
            ));
        }
        let mut unique = numbers.clone();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() != numbers.len() {
            items.push(file_item(
                Severity::Warning,
                crate::strings::PACK_CHECK_DUP_NUMBERS.to_owned(),
            ));
        } else if !numbers.is_empty() && unique != (1..=numbers.len() as u32).collect::<Vec<_>>() {
            items.push(file_item(
                Severity::Warning,
                crate::strings::PACK_CHECK_NONCONTIGUOUS.to_owned(),
            ));
        }

        items.extend(readiness(&self.meta, &self.track_facts()));
        items
    }

    /// The export gate: blocking errors, soft warnings, and optional notes,
    /// bucketed from [`Self::readiness_items`] by severity. Notes are surfaced in
    /// the checklist but never gate an export.
    #[must_use]
    pub fn validations(&self) -> PackValidations {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut notes = Vec::new();
        for item in self.readiness_items() {
            match item.severity {
                Severity::Error => errors.push(item.message),
                Severity::Warning => warnings.push(item.message),
                Severity::Note => notes.push(item.message),
            }
        }
        PackValidations {
            errors,
            warnings,
            notes,
        }
    }

    /// How many tracks carry a loop point, out of how many there are -- the
    /// tally beside the checklist's Loops heading. A track that failed to parse
    /// counts in the total but can carry no loop, which is the honest reading:
    /// the pack is not all-looping until every file in it is.
    #[must_use]
    pub fn loop_tally(&self) -> (usize, usize) {
        let looping = self
            .tracks
            .iter()
            .filter(|track| {
                track
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.loop_samples.is_some())
            })
            .count();
        (looping, self.tracks.len())
    }

    /// The transaction that deletes the screenshot named `name`, with the
    /// inverse that writes it back from the bytes already in memory -- so a
    /// deleted screenshot is recoverable for as long as the pack stays open.
    ///
    /// Keyed by file name rather than index because a confirm prompt sits
    /// between the click and the deletion, and a rescan in between can reorder
    /// the list. `None` when no such image is loaded, or the folder has no path.
    #[must_use]
    pub fn delete_image_transaction(&self, name: &str) -> Option<PackTransaction> {
        let image = self.images.iter().find(|image| image.name == name)?;
        let path = image.path.clone()?;
        Some(PackTransaction {
            label: format!("Delete {name}"),
            forward: vec![PackMutation::Delete { path: path.clone() }],
            inverse: vec![PackMutation::Write {
                path,
                bytes: image.bytes.to_vec(),
            }],
        })
    }

    /// A screenshot file name free in this folder: `stem.ext` if nothing holds
    /// it, else `stem (2).ext`, `stem (3).ext`... -- so adding a second
    /// screenshot lands beside the first rather than on top of it.
    #[must_use]
    pub fn free_image_name(&self, stem: &str, ext: &str) -> String {
        let taken = |name: &str| {
            self.images
                .iter()
                .any(|image| image.name.eq_ignore_ascii_case(name))
        };
        let first = format!("{stem}.{ext}");
        if !taken(&first) {
            return first;
        }
        (2..=self.images.len() + 2)
            .map(|n| format!("{stem} ({n}).{ext}"))
            .find(|name| !taken(name))
            .expect("more candidates than screenshots, so one of them is free")
    }

    /// The name the next added screenshot lands under: the pack's own file-name
    /// stem, made unique. `None` when there is no game name to derive one from,
    /// which is when an added file keeps the name it arrived with.
    #[must_use]
    pub fn next_screenshot_name(&self) -> Option<String> {
        let stem = self.doc_stem();
        (!stem.is_empty()).then(|| self.free_image_name(&stem, "png"))
    }

    /// The transaction that renames the screenshot `name` to `new_name`, keyed
    /// by name for the same reason [`Self::delete_image_transaction`] is: the
    /// dialog sits between the click and the rename. `None` when no such image
    /// is loaded, or it has no path (the web has none).
    #[must_use]
    pub fn rename_image_transaction(&self, name: &str, new_name: &str) -> Option<PackTransaction> {
        let image = self.images.iter().find(|image| image.name == name)?;
        let path = image.path.clone()?;
        Some(PackTransaction {
            label: format!("Rename {name} to {new_name}"),
            forward: vec![PackMutation::Rename {
                from: path.clone(),
                to: new_name.to_owned(),
            }],
            inverse: vec![PackMutation::Rename {
                from: path.with_file_name(new_name),
                to: name.to_owned(),
            }],
        })
    }

    /// The output deck's verdict: the worst outstanding severity (`None` when
    /// nothing is outstanding at all) and the phrase shown beside its lamp.
    ///
    /// Only errors block an export, so they are the only tier whose phrase says
    /// so; notes are counted but read as ready, matching
    /// [`Self::validations`]'s rule that the note tier never gates.
    #[must_use]
    pub fn readiness_summary(&self) -> (Option<Severity>, String) {
        let checks = self.validations();
        let count = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
        if !checks.errors.is_empty() {
            let n = checks.errors.len();
            (
                Some(Severity::Error),
                format!("{} \u{2014} export blocked", count(n, "error")),
            )
        } else if !checks.warnings.is_empty() {
            (
                Some(Severity::Warning),
                count(checks.warnings.len(), "warning"),
            )
        } else if !checks.notes.is_empty() {
            (Some(Severity::Note), count(checks.notes.len(), "note"))
        } else {
            (None, crate::strings::PACK_READY_TO_SUBMIT.to_owned())
        }
    }

    /// Builds the export job: every song, every screenshot, and freshly
    /// generated docs whose names reflect the final (post-gzip) song names.
    #[must_use]
    pub fn export_request(&self) -> PackJobRequest {
        let stem = self.doc_stem();
        let mut entries: Vec<PackEntry> = Vec::new();
        for track in &self.tracks {
            entries.push(PackEntry {
                name: track.file_name.clone(),
                bytes: track.bytes.clone(),
                kind: PackEntryKind::Song,
            });
        }
        for image in &self.images {
            entries.push(PackEntry {
                name: image.name.clone(),
                bytes: image.bytes.to_vec(),
                kind: PackEntryKind::Image,
            });
        }
        entries.push(PackEntry {
            name: format!("{stem}.txt"),
            bytes: self.description_text().into_bytes(),
            kind: PackEntryKind::Doc,
        });
        entries.push(PackEntry {
            name: format!("{stem}.m3u"),
            bytes: self.m3u_text(true).into_bytes(),
            kind: PackEntryKind::Doc,
        });
        PackJobRequest {
            zip_name: format!("{stem}.zip"),
            entries,
            gzip_vgms: self.gzip_on_export,
            optimize_vgms: self.optimize_on_export,
            // The pack state does not carry the Settings choice; the app fills it
            // in from `config.optimizer` before the request is submitted. `Auto`
            // is the safe default for any caller that does not (the tests).
            optimizer: vgms_core::config::OptimizerChoice::Auto,
            // Likewise the two tool-stage switches, filled from config before
            // submit; the pipeline's own defaults (both on) are the fallback.
            sample_roms: true,
            dac_runs: true,
        }
    }
}

/// The result of [`PackState::validations`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PackValidations {
    /// Problems that block an export.
    pub errors: Vec<String>,
    /// Advisories the user may choose to ignore (the "export anyway?" confirm).
    pub warnings: Vec<String>,
    /// Optional observations (the note tier): surfaced in the submission
    /// checklist, but never shown in the export dialog and never gating.
    pub notes: Vec<String>,
}

/// A file-level ([`ReadinessCategory::Files`]) readiness item, targeting the pack
/// as a whole (there is no single field or track to jump to).
fn file_item(severity: Severity, message: String) -> ReadinessItem {
    ReadinessItem {
        severity,
        category: ReadinessCategory::Files,
        target: ReadinessTarget::Pack,
        message,
    }
}

/// The file name a track at 0-based pack position `index` should carry, from its
/// GD3 Track Name. `None` when it has no readable song, no tag, or a track name
/// no file can be named after.
fn wanted_file_name(index: usize, track: &PackTrack) -> Option<String> {
    let tag = track.tag()?;
    vgms_core::pack::naming::tag_file_name(index + 1, tag.track_name_en.trim(), &track.file_name)
}

/// The `NN` from a `NN Title.ext` file name, if present.
fn track_number(file_name: &str) -> Option<u32> {
    let digits: String = file_name.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !file_name[digits.len()..].starts_with(' ') {
        return None;
    }
    digits.parse().ok()
}

/// Replaces a `NN Title.ext` file name's leading track number with `number`,
/// keeping the title and extension. A name with no `NN ` prefix just gains one.
fn renumber(file_name: &str, number: usize) -> String {
    let rest = match file_name.split_once(' ') {
        Some((prefix, rest))
            if !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_digit()) =>
        {
            rest
        }
        _ => file_name,
    };
    format!("{number:02} {rest}")
}

/// The renames needed to move the track at `from` to `to`: every track whose
/// position changes is renumbered to its new 1-based slot, keeping its title and
/// extension. Returns `(old_name, new_name)` pairs, omitting tracks that keep
/// their name. An out-of-range or no-op move yields nothing.
#[must_use]
pub fn reorder_renames(names: &[String], from: usize, to: usize) -> Vec<(String, String)> {
    if from >= names.len() || to >= names.len() || from == to {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..names.len()).collect();
    let moved = order.remove(from);
    order.insert(to, moved);
    order
        .iter()
        .enumerate()
        .filter_map(|(new_pos, &old)| {
            let new_name = renumber(&names[old], new_pos + 1);
            (new_name != names[old]).then_some((names[old].clone(), new_name))
        })
        .collect()
}

/// One reversible file operation on the pack folder, the unit the app's file-op
/// executor runs. `Rename`'s `to` is a bare name in the same directory; `Write`
/// overwrites `path` outright; `Delete` removes it.
///
/// `Delete` is reversible only because its inverse is a `Write` of the bytes the
/// app still holds in memory, which is why deleting a pack file goes through a
/// transaction rather than straight to the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackMutation {
    Rename { from: PathBuf, to: String },
    Write { path: PathBuf, bytes: Vec<u8> },
    Delete { path: PathBuf },
}

/// A user edit as an ordered list of file mutations, with the exact `inverse`
/// that undoes it. Applying `forward` then `inverse` returns the folder to its
/// starting state; the app replays whichever list an undo or redo needs.
#[derive(Debug, Clone)]
pub struct PackTransaction {
    pub label: String,
    pub forward: Vec<PackMutation>,
    pub inverse: Vec<PackMutation>,
}

impl PackTransaction {
    /// This edit followed by `next`, as one step: the forwards run in order, the
    /// inverses in reverse. Used to fold a run of keyboard reorders on one track
    /// into a single undo -- nine Alt+Ups are one edit to the person pressing
    /// them, however many renames they cost.
    ///
    /// The merged label is `next`'s, since the pair reads as "what just
    /// happened" and the last one said it most recently.
    #[must_use]
    pub fn then(mut self, next: Self) -> Self {
        let mut inverse = next.inverse;
        inverse.append(&mut self.inverse);
        self.forward.extend(next.forward);
        Self {
            label: next.label,
            forward: self.forward,
            inverse,
        }
    }
}

/// The mutations that apply a set of `(src, dst)` renames without a transient
/// collision: rename every source to a unique temp name first, then each temp to
/// its destination. Safe for any permutation (including cycles and swaps).
fn rename_batch_mutations(
    folder: &std::path::Path,
    pairs: &[(String, String)],
) -> Vec<PackMutation> {
    let temp = |i: usize| format!(".vgmstudio-reorder-{i}");
    let mut muts = Vec::with_capacity(pairs.len() * 2);
    for (i, (src, _)) in pairs.iter().enumerate() {
        muts.push(PackMutation::Rename {
            from: folder.join(src),
            to: temp(i),
        });
    }
    for (i, (_, dst)) in pairs.iter().enumerate() {
        muts.push(PackMutation::Rename {
            from: folder.join(temp(i)),
            to: dst.clone(),
        });
    }
    muts
}

enum FileClass {
    Song,
    Image,
    Doc,
    Other,
}

fn classify(name: &str) -> FileClass {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".vgm") || lower.ends_with(".vgz") {
        FileClass::Song
    } else if lower.ends_with(".png") {
        FileClass::Image
    } else if lower.ends_with(".txt") {
        FileClass::Doc
    } else {
        FileClass::Other
    }
}

/// Parses one scanned song file, and the table entry that goes with it.
///
/// One reader, for every chip -- there is no "try the OPL reader, then fall
/// back", because a VGM is a VGM whatever chips it turns out to declare.
fn read_track(name: &str, bytes: &[u8]) -> (PackSong, Option<TrackEntry>) {
    match vgms_core::vgm::file::read(name, bytes) {
        Ok(file) => {
            let entry = TrackEntry::from_vgm_file(&file);
            (PackSong::Vgm(Arc::new(file)), Some(entry))
        }
        Err(error) => (PackSong::Unreadable(error.to_string()), None),
    }
}

/// Prefers a description whose stem matches the folder name, else the first.
fn choose_description<'a>(texts: &'a [PickedFile], folder_name: &str) -> Option<&'a PickedFile> {
    texts
        .iter()
        .find(|file| {
            let stem = file
                .name
                .rsplit_once('.')
                .map_or(file.name.as_str(), |(stem, _)| stem);
            stem.eq_ignore_ascii_case(folder_name)
        })
        .or_else(|| texts.first())
}

/// Default metadata for a fresh pack, seeded from the songs' GD3 tags.
///
/// The system and OS default to the PC ones only while the pack looks like a PC
/// pack -- which is to say, while its chips are OPL. A pack of Mega Drive rips
/// is some other machine entirely, and guessing "IBM PC/AT" for one would be
/// worse than leaving the field for the checklist to ask about -- so those come
/// from the tracks' own GD3 System Name instead, and the hardware line from the
/// chips they declare.
fn prefilled(tracks: &[PackTrack], today: Option<(i32, u8, u8)>) -> PackMeta {
    let opl_types: Vec<OplType> = tracks.iter().filter_map(PackTrack::opl_type).collect();
    let is_pc_pack = !opl_types.is_empty();

    let mut meta = PackMeta {
        system: if is_pc_pack {
            DEFAULT_SYSTEM.to_owned()
        } else {
            String::new()
        },
        os: if is_pc_pack {
            DEFAULT_OS.to_owned()
        } else {
            String::new()
        },
        version: "1.00".to_owned(),
        ..PackMeta::default()
    };
    if let Some(opl) = highest_opl(&opl_types) {
        meta.music_hardware = music_hardware_suggestion(opl).to_owned();
    } else if let Some(chips) = tracks.iter().find_map(PackTrack::chip_list) {
        meta.music_hardware = chips;
    }
    for track in tracks {
        if let Some(tag) = track.tag() {
            fill_if_empty(&mut meta.game_name, &tag.game_name_en);
            fill_if_empty(&mut meta.creator, &tag.creator);
            fill_if_empty(&mut meta.release_date, &tag.release_date);
            if !is_pc_pack {
                fill_if_empty(&mut meta.system, &tag.system_name_en);
            }
        }
    }
    meta.music_authors = unique_authors(tracks);

    let date = today.map_or_else(
        || "<date>".to_owned(),
        |(year, month, day)| format!("{year:04}-{month:02}-{day:02}"),
    );
    let creator = if meta.creator.is_empty() {
        "<creator>"
    } else {
        &meta.creator
    };
    meta.history = format!("1.00 {date} {creator}: Initial release.");
    meta
}

fn fill_if_empty(slot: &mut String, value: &str) {
    if slot.is_empty() && !value.trim().is_empty() {
        *slot = value.trim().to_owned();
    }
}

/// The most capable chip across the tracks (OPL3 > dual OPL2 > OPL2).
fn highest_opl(opl_types: &[OplType]) -> Option<OplType> {
    opl_types.iter().copied().max_by_key(|opl| match opl {
        OplType::Opl2 => 0,
        OplType::DualOpl2 => 1,
        OplType::Opl3 => 2,
    })
}

/// Distinct GD3 track authors, in track order, comma-joined.
fn unique_authors(tracks: &[PackTrack]) -> String {
    let mut authors: Vec<String> = Vec::new();
    for track in tracks {
        if let Some(tag) = track.tag() {
            let author = tag.track_author_en.trim();
            if !author.is_empty() && !authors.iter().any(|a| a == author) {
                authors.push(author.to_owned());
            }
        }
    }
    authors.join(", ")
}

fn to_vgz_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("vgm") => format!("{stem}.vgz"),
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgms_core::ChipKind;
    use vgms_core::vgm::data::Gd3Tag;

    const VGM_FIXTURE: &[u8] = include_bytes!("../../../../tests/lsl3_score_up.vgm");
    const PNG_FIXTURE: &[u8] = include_bytes!("../../../../tests/screenshot.png");

    /// A VGM fixture re-serialised with a given file name and GD3 tag, wrapped as
    /// a picked file -- the same trick the editor's tests use.
    fn tagged_song(name: &str, tag: Gd3Tag) -> PickedFile {
        let mut file = vgms_core::vgm::file::read(name, VGM_FIXTURE).unwrap();
        file.tag = Some(tag);
        PickedFile {
            name: name.to_owned(),
            path: Some(PathBuf::from(format!("C:/pack/{name}"))),
            bytes: vgms_core::vgm::file::write(&file).unwrap(),
        }
    }

    fn folder(name: &str, files: Vec<PickedFile>) -> PickedFolder {
        PickedFolder {
            name: name.to_owned(),
            path: Some(PathBuf::from(format!("C:/{name}"))),
            files,
        }
    }

    /// A non-clipping peak at `max_level`, for the volume-modifier tests.
    fn peak(max_level: i16) -> Peak {
        Peak {
            max_level,
            clipped: false,
        }
    }

    fn tag(game: &str, author: &str, creator: &str) -> Gd3Tag {
        Gd3Tag {
            game_name_en: game.to_owned(),
            track_author_en: author.to_owned(),
            creator: creator.to_owned(),
            release_date: "1994".to_owned(),
            ..Gd3Tag::default()
        }
    }

    #[test]
    fn a_per_track_optimize_override_wins_over_the_global_default() {
        use vgms_core::config::{OptimizeOptions, OptimizerChoice};
        let mut pack = PackState::from_folder(folder("Game", vec![]), None);
        let global = OptimizeOptions::default();
        // No override: the global default is returned.
        assert_eq!(pack.effective_optimize_options("01 A.vgm", global), global);
        // An override wins for its track only.
        let custom = OptimizeOptions {
            optimizer: OptimizerChoice::Tools,
            sample_roms: false,
            dac_runs: true,
        };
        pack.track_optimize_overrides
            .insert("01 A.vgm".to_owned(), custom);
        assert_eq!(pack.effective_optimize_options("01 A.vgm", global), custom);
        assert_eq!(
            pack.effective_optimize_options("02 B.vgm", global),
            global,
            "another track still falls back to the global default"
        );
    }

    /// A Mega Drive VGM: a YM2612 and an SN76489, and a body of commands the OPL
    /// reader cannot even size, so it is certain to decline the file.
    fn other_chip_song(name: &str, tag: Gd3Tag) -> PickedFile {
        fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        const VERSION: usize = 0x08;
        const TOTAL_SAMPLES: usize = 0x18;
        const LOOP_OFFSET: usize = 0x1C;
        const LOOP_NUM_SAMPLES: usize = 0x20;
        const DATA_OFFSET: usize = 0x34;

        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, VERSION, 0x161);
        put_u32(&mut bytes, DATA_OFFSET, (0x100 - DATA_OFFSET) as u32);
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        put_u32(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
        put_u32(&mut bytes, TOTAL_SAMPLES, 220_500); // five seconds
        // A loop covering the second half, so the Loop column has something.
        put_u32(&mut bytes, LOOP_OFFSET, (0x100 + 5 - LOOP_OFFSET) as u32);
        put_u32(&mut bytes, LOOP_NUM_SAMPLES, 110_250);
        bytes.extend_from_slice(&[
            0x52, 0x28, 0xF0, // YM2612 write
            0x50, 0x9F, // SN76489 write
            0x61, 0x10, 0x27, // wait
            0x66, // end of data
        ]);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);

        // Through the real reader and writer, so the tag lands exactly where a
        // genuine file would carry it.
        let mut file = vgms_core::vgm::file::read(name, &bytes).expect("a valid VGM");
        file.tag = Some(tag);
        PickedFile {
            name: name.to_owned(),
            path: Some(PathBuf::from(format!("C:/pack/{name}"))),
            bytes: vgms_core::vgm::file::write(&file).expect("writes"),
        }
    }

    // -- tracks of any chip --------------------------------------------------

    #[test]
    fn a_folder_of_mixed_chips_opens_with_every_track_placed() {
        let files = vec![
            tagged_song("01 Opl.vgm", tag("Cool Game", "Ada", "Ripper")),
            other_chip_song("02 Mega Drive.vgm", tag("Cool Game", "Bob", "Ripper")),
            PickedFile {
                name: "03 Broken.vgm".to_owned(),
                path: Some(PathBuf::from("C:/pack/03 Broken.vgm")),
                bytes: b"not a vgm at all".to_vec(),
            },
        ];
        let state = PackState::from_folder(folder("Cool Game", files), None);
        assert_eq!(state.tracks.len(), 3);

        // Both VGMs are VGMs; only the chips they declare differ.
        assert!(state.tracks[0].vgm().is_some());
        assert!(state.tracks[1].vgm().is_some());
        assert!(state.tracks[2].vgm().is_none());

        assert_eq!(
            state.tracks[0].opl_type(),
            Some(OplType::Opl2),
            "the OPL track unlocks the OPL features"
        );
        assert_eq!(
            state.tracks[1].opl_type(),
            None,
            "the Mega Drive one does not"
        );
        assert!(state.tracks[0].is_playable());
        assert!(
            state.tracks[1].is_playable(),
            "its PSG has a core, so a preview would be heard..."
        );
        assert_eq!(
            state.tracks[1].chips_without_cores(),
            "YM2612",
            "...without its FM"
        );
        assert!(
            state.tracks[1].is_editable(),
            "but it still opens for trimming"
        );

        assert!(state.tracks[0].is_readable());
        assert!(
            state.tracks[1].is_readable(),
            "not being OPL is not a fault"
        );
        assert!(!state.tracks[2].is_readable());
        assert!(state.tracks[2].error().is_some());
    }

    /// `is_playable` answers `preview_source`'s question the cheap way, because
    /// the track table asks it once per row per frame and `preview_source`
    /// copies the whole command stream to answer. The two must never disagree:
    /// a `▶` that plays nothing, or a missing one on a track that would.
    #[test]
    fn the_two_playability_answers_agree() {
        let files = vec![
            tagged_song("01 Theme.vgm", tag("Cool Game", "Composer", "Ripper")),
            other_chip_song("02 Mega.vgm", tag("Sonic", "Nakamura", "Ripper")),
            PickedFile {
                name: "03 Broken.vgm".to_owned(),
                path: None,
                bytes: b"not a vgm at all".to_vec(),
            },
        ];
        let state = PackState::from_folder(folder("Cool Game", files), None);
        assert_eq!(state.tracks.len(), 3);
        for track in &state.tracks {
            assert_eq!(
                track.is_playable(),
                track.preview_source().is_some(),
                "{} disagrees about being playable",
                track.file_name
            );
        }
        // ...and the fixtures actually cover both answers, or the loop above
        // would pass on three tracks that are all the same case. (Whether the
        // Mega Drive one plays depends on which providers this build links, so
        // it is only held to agreeing with itself.)
        assert!(state.tracks[0].is_playable(), "the OPL rip plays");
        assert!(!state.tracks[2].is_playable(), "a non-VGM does not");
    }

    #[test]
    fn a_track_for_other_chips_gets_the_same_row_facts_as_any_other() {
        let files = vec![other_chip_song(
            "01 Theme.vgm",
            Gd3Tag {
                track_name_en: "Green Hill".to_owned(),
                ..tag("Sonic", "Nakamura", "Ripper")
            },
        )];
        let state = PackState::from_folder(folder("Sonic", files), None);

        let entry = state.tracks[0].entry.as_ref().expect("an entry");
        assert_eq!(entry.title, "Green Hill");
        assert_eq!(entry.total_samples, 220_500);
        assert_eq!(entry.loop_samples, Some(110_250));
        assert_eq!(state.tracks[0].chip_list().unwrap(), "SN76489, YM2612");
        assert_eq!(state.tracks[0].tag().unwrap().track_author_en, "Nakamura");
    }

    /// The whole point of the step: a track the editor cannot open still has its
    /// tags fixed, and comes back out with its chips intact.
    #[test]
    fn retagging_a_track_for_other_chips_keeps_its_chips_and_its_music() {
        let files = vec![other_chip_song("01 Theme.vgm", tag("Sonic", "N", "Ripper"))];
        let state = PackState::from_folder(folder("Sonic", files), None);
        let track = &state.tracks[0];

        let mut new_tag = track.tag().cloned().unwrap();
        new_tag.notes = "Ripped from a cartridge".to_owned();
        let bytes = track
            .retagged("01 Theme.vgm", new_tag)
            .expect("a readable track")
            .expect("writes");

        let reread = vgms_core::vgm::file::read("01 Theme.vgm", &bytes).unwrap();
        assert_eq!(
            reread.tag.as_ref().unwrap().notes,
            "Ripped from a cartridge"
        );
        assert_eq!(reread.chip_list(), "SN76489, YM2612");
        assert_eq!(reread.body, track.vgm().unwrap().body);
    }

    /// One reader and one writer for every VGM: an OPL track retagged keeps the
    /// chip clock it was logged at verbatim, rather than having it re-derived
    /// from the chip *type* and canonicalised.
    #[test]
    fn retagging_an_opl_track_no_longer_rewrites_its_chip_clock() {
        const ODD_CLOCK: u32 = 3_600_000; // not the canonical 3_579_545
        let mut bytes = vgms_core::vgm::file::write(
            &vgms_core::vgm::file::read("01 Tune.vgm", VGM_FIXTURE).unwrap(),
        )
        .unwrap();
        let at = ChipKind::Ym3812.clock_offset();
        bytes[at..at + 4].copy_from_slice(&ODD_CLOCK.to_le_bytes());

        let files = vec![PickedFile {
            name: "01 Tune.vgm".to_owned(),
            path: Some(PathBuf::from("C:/pack/01 Tune.vgm")),
            bytes,
        }];
        let state = PackState::from_folder(folder("G", files), None);
        let track = &state.tracks[0];
        assert_eq!(
            track.opl_type(),
            Some(OplType::Opl2),
            "an odd clock is still an OPL2"
        );

        let written = track
            .retagged("01 Tune.vgm", tag("G", "A", "R"))
            .unwrap()
            .unwrap();
        let reread = vgms_core::vgm::file::read("01 Tune.vgm", &written).unwrap();
        assert_eq!(
            reread.header.chips()[0].clock,
            ODD_CLOCK,
            "the clock the file was logged at is what it keeps"
        );
    }

    /// A rename to `.vgz` must gzip, exactly as the OPL path does.
    #[test]
    fn retagging_a_track_for_other_chips_to_vgz_compresses_it() {
        let files = vec![other_chip_song("01 Theme.vgm", tag("Sonic", "N", "R"))];
        let state = PackState::from_folder(folder("Sonic", files), None);
        let tag = state.tracks[0].tag().cloned().unwrap();

        let bytes = state.tracks[0]
            .retagged("01 Theme.vgz", tag)
            .unwrap()
            .unwrap();
        assert!(vgms_core::vgm::io::is_gzipped(&bytes));
        assert_eq!(
            vgms_core::vgm::file::read("01 Theme.vgz", &bytes)
                .unwrap()
                .chip_list(),
            "SN76489, YM2612"
        );
    }

    #[test]
    fn a_track_for_other_chips_carries_a_volume_modifier_like_any_vgm() {
        let files = vec![other_chip_song("01 Theme.vgm", tag("Sonic", "N", "R"))];
        let state = PackState::from_folder(folder("Sonic", files), None);
        assert_eq!(state.tracks[0].volume_modifier(), Some(0));

        let bytes = state.tracks[0].revolumed(0x20).unwrap().unwrap();
        let reread = vgms_core::vgm::file::read("01 Theme.vgm", &bytes).unwrap();
        assert_eq!(reread.header.volume_modifier(), 0x20);
        assert_eq!(reread.chip_list(), "SN76489, YM2612");
    }

    /// A non-PC pack must not be prefilled as a PC one: the system comes from
    /// the tags, and the hardware line from the chips the files declare.
    #[test]
    fn a_non_opl_pack_is_not_prefilled_as_a_pc_pack() {
        let files = vec![other_chip_song(
            "01 Theme.vgm",
            Gd3Tag {
                system_name_en: "Sega Mega Drive".to_owned(),
                ..tag("Sonic", "Nakamura", "Ripper")
            },
        )];
        let state = PackState::from_folder(folder("Sonic", files), None);

        assert_eq!(state.meta.system, "Sega Mega Drive");
        assert_eq!(state.meta.os, "", "no DOS on a Mega Drive");
        assert_eq!(state.meta.music_hardware, "SN76489, YM2612");
        assert_eq!(state.meta.game_name, "Sonic");
        assert_eq!(state.meta.music_authors, "Nakamura");
    }

    /// One OPL track is enough to make it a PC pack again.
    #[test]
    fn a_pack_with_any_opl_track_keeps_the_pc_defaults() {
        let files = vec![
            tagged_song("01 Opl.vgm", tag("G", "A", "R")),
            other_chip_song("02 Other.vgm", tag("G", "B", "R")),
        ];
        let state = PackState::from_folder(folder("G", files), None);
        assert_eq!(state.meta.system, "IBM PC/AT");
        assert_eq!(state.meta.os, "DOS");
        assert_eq!(state.meta.music_hardware, "AdLib/Sound Blaster (YM3812)");
        assert_eq!(state.meta.music_authors, "A, B", "both tracks' authors");
    }

    /// A track for other chips is a full citizen of the checklist -- counted, exported,
    /// never flagged as broken -- with one note naming the chips that would be
    /// silent if it were previewed.
    #[test]
    fn the_checklist_notes_what_would_be_silent_without_blocking_it() {
        let files = vec![other_chip_song(
            "01 Theme.vgm",
            Gd3Tag {
                track_name_en: "Theme".to_owned(),
                ..tag("Sonic", "N", "R")
            },
        )];
        let mut state = PackState::from_folder(folder("Sonic", files), None);
        state.meta.game_name = "Sonic".to_owned();

        // The PSG has a core and the FM does not, so a preview is worth
        // offering -- with the FM named as what will be missing from it.
        assert_eq!(state.silent_chips(), ["YM2612"]);
        assert!(state.tracks[0].is_playable(), "the PSG half plays");
        let items = state.readiness_items();
        assert!(
            !items
                .iter()
                .any(|item| item.message.contains("could not be read")),
            "a track for other chips is not unreadable: {items:?}"
        );
        let note = items
            .iter()
            .find(|item| item.message.contains("Playback"))
            .expect("the preview note");
        assert_eq!(note.severity, Severity::Note, "a note never gates export");
        assert!(note.message.contains("YM2612"), "{}", note.message);
        assert!(
            !note.message.contains("SN76489"),
            "the PSG is not silent: {}",
            note.message
        );
        assert!(
            state.validations().errors.is_empty(),
            "a track for other chips does not block export"
        );
    }

    #[test]
    fn a_track_for_other_chips_is_listed_in_the_description() {
        let files = vec![other_chip_song(
            "01 Theme.vgm",
            Gd3Tag {
                track_name_en: "Theme".to_owned(),
                ..tag("Sonic", "N", "R")
            },
        )];
        let state = PackState::from_folder(folder("Sonic", files), None);
        let description = state.description_text();
        assert!(description.contains("Theme"), "{description}");
        assert!(
            description.contains("0:05"),
            "its length too: {description}"
        );
    }

    #[test]
    fn prefills_from_gd3_when_there_is_no_description() {
        let files = vec![
            tagged_song("01 Intro.vgz", tag("Cool Game", "Ada", "Ripper")),
            tagged_song("02 Level.vgz", tag("Cool Game", "Bob", "Ripper")),
        ];
        let state = PackState::from_folder(folder("Cool Game", files), Some((2026, 7, 16)));

        assert_eq!(state.meta.game_name, "Cool Game");
        assert_eq!(state.meta.creator, "Ripper");
        assert_eq!(state.meta.system, "IBM PC/AT");
        assert_eq!(state.meta.os, "DOS");
        assert_eq!(state.meta.music_authors, "Ada, Bob");
        assert_eq!(state.meta.version, "1.00");
        assert_eq!(
            state.meta.history,
            "1.00 2026-07-16 Ripper: Initial release."
        );
        assert!(state.description_file.is_none());
        assert!(!state.dirty);
    }

    #[test]
    fn prefill_history_uses_a_placeholder_date_without_a_clock() {
        let files = vec![tagged_song("01 Intro.vgz", tag("G", "A", "Ripper"))];
        let state = PackState::from_folder(folder("G", files), None);
        assert_eq!(state.meta.history, "1.00 <date> Ripper: Initial release.");
    }

    /// The volume_modifier a set of serialised song bytes decode to.
    fn modifier_of(bytes: &[u8]) -> u8 {
        vgms_core::vgm::file::read("x.vgm", bytes)
            .unwrap()
            .header
            .volume_modifier()
    }

    #[test]
    fn refresh_keeps_peaks_for_header_edits_and_drops_them_for_audio_edits() {
        let files = vec![
            tagged_song("01 A.vgm", tag("G", "A", "R")),
            tagged_song("02 B.vgm", tag("G", "B", "R")),
        ];
        let mut state = PackState::from_folder(folder("G", files), None);
        state.peaks.insert("01 A.vgm".to_owned(), peak(0x4000));
        state.peaks.insert("02 B.vgm".to_owned(), peak(0x2000));

        // Redeliver the folder with track 1 rewritten header-only (a new volume
        // modifier -- what Apply Modifiers produces) and track 2's audio replaced
        // wholesale (what an editor edit saved back produces).
        let header_edit = state.tracks[0]
            .revolumed(0x20)
            .expect("a VGM")
            .expect("writes");
        let other_file =
            vgms_core::convert::dro_to_vgm(&crate::test_song::tone_song()).expect("converts");
        let audio_edit = vgms_core::vgm::file::write(&other_file).expect("writes");
        let file = |name: &str, bytes: Vec<u8>| PickedFile {
            name: name.to_owned(),
            path: Some(PathBuf::from(format!("C:/pack/{name}"))),
            bytes,
        };
        state.refresh_files(folder(
            "G",
            vec![file("01 A.vgm", header_edit), file("02 B.vgm", audio_edit)],
        ));

        assert!(
            state.peaks.contains_key("01 A.vgm"),
            "a header-only rewrite keeps the measured peak"
        );
        assert!(
            !state.peaks.contains_key("02 B.vgm"),
            "changed audio must drop the stale peak"
        );
    }

    #[test]
    fn album_mode_gives_every_changed_track_the_same_modifier() {
        let files = vec![
            tagged_song("01 Loud.vgm", tag("Game", "A", "R")),
            tagged_song("02 Quiet.vgm", tag("Game", "B", "R")),
        ];
        let mut state = PackState::from_folder(folder("Game", files), None);
        // No peaks scanned yet: nothing to apply.
        assert!(state.suggested_modifier_transaction().is_none());

        // The loudest track peaks at half scale, the other at an eighth.
        state.peaks.insert("01 Loud.vgm".to_owned(), peak(0x4000));
        state.peaks.insert("02 Quiet.vgm".to_owned(), peak(0x1000));

        // Album mode levels both by the loudest peak, so they share one modifier
        // (a half-scale album peak means +1 doubling, 0x20) -- preserving their
        // relative loudness.
        let txn = state
            .suggested_modifier_transaction()
            .expect("both differ from the fixture default");
        let modifiers: Vec<u8> = txn
            .forward
            .iter()
            .map(|mutation| match mutation {
                PackMutation::Write { bytes, .. } => modifier_of(bytes),
                other => panic!("only writes, got {other:?}"),
            })
            .collect();
        assert_eq!(
            modifiers,
            vec![0x20, 0x20],
            "album mode: one factor for all"
        );
    }

    #[test]
    fn per_track_mode_normalises_each_track_to_its_own_peak() {
        let files = vec![
            tagged_song("01 Loud.vgm", tag("Game", "A", "R")),
            tagged_song("02 Quiet.vgm", tag("Game", "B", "R")),
        ];
        let mut state = PackState::from_folder(folder("Game", files), None);
        state.album_normalize = false;
        state.peaks.insert("01 Loud.vgm".to_owned(), peak(0x4000)); // half -> 0x20
        state.peaks.insert("02 Quiet.vgm".to_owned(), peak(0x1000)); // eighth -> 0x60

        let txn = state.suggested_modifier_transaction().expect("both change");
        let modifiers: Vec<u8> = txn
            .forward
            .iter()
            .map(|mutation| match mutation {
                PackMutation::Write { bytes, .. } => modifier_of(bytes),
                other => panic!("only writes, got {other:?}"),
            })
            .collect();
        // Each track is boosted to its own full scale, so the quieter one gets the
        // bigger modifier -- unlike album mode's shared factor.
        assert_eq!(modifiers, vec![0x20, 0x60]);
    }

    #[test]
    fn parses_an_existing_description_verbatim_over_prefilling() {
        let description = "Game name:           Existing Pack\r\n\
            \r\n\
            Notes:\r\n\
            Handwritten note.\r\n\
            \r\n\
            Package history:\r\n\
            1.00 2015-01-01 Someone: Initial release.\r\n";
        let files = vec![
            tagged_song("01 Intro.vgz", tag("Ignored GD3 Game", "A", "R")),
            PickedFile {
                name: "Existing Pack.txt".to_owned(),
                path: Some(PathBuf::from("C:/p/Existing Pack.txt")),
                bytes: description.as_bytes().to_vec(),
            },
        ];
        let state = PackState::from_folder(folder("Existing Pack", files), Some((2026, 7, 16)));
        assert_eq!(
            state.meta.game_name, "Existing Pack",
            "the .txt wins over GD3"
        );
        assert_eq!(state.meta.notes, "Handwritten note.");
        assert_eq!(state.description_file.as_deref(), Some("Existing Pack.txt"));
        assert!(state.parse_warning.is_none());
    }

    #[test]
    fn a_garbage_description_warns_and_falls_back_to_prefill() {
        let files = vec![
            tagged_song("01 Intro.vgz", tag("GD3 Game", "A", "R")),
            PickedFile {
                name: "notes.txt".to_owned(),
                path: None,
                bytes: b"total garbage\r\nnot a description".to_vec(),
            },
        ];
        let state = PackState::from_folder(folder("Pack", files), Some((2026, 7, 16)));
        assert!(state.parse_warning.is_some());
        assert_eq!(state.meta.game_name, "GD3 Game", "prefilled from GD3");
    }

    #[test]
    fn validations_report_hard_errors_and_soft_warnings() {
        let files = vec![tagged_song("01 Intro.vgz", tag("Game", "A", "R"))];
        let mut state = PackState::from_folder(folder("Game", files), Some((2026, 7, 16)));

        // A named, single-track pack: no hard errors, but no screenshot is a
        // soft warning.
        let checks = state.validations();
        assert!(checks.errors.is_empty());
        assert!(checks.warnings.iter().any(|w| w.contains("screenshot")));

        // An empty game name is a hard error.
        state.meta.game_name.clear();
        assert!(!state.validations().errors.is_empty());
    }

    #[test]
    fn validations_merge_the_gd3_content_checks_as_warnings() {
        // The fixture tag() fills game/author/creator/date but leaves Track Name
        // and System blank, so the readiness pass adds a per-track "missing"
        // warning beyond the file-level ones -- and never a hard error.
        let files = vec![tagged_song("01 Intro.vgz", tag("Game", "A", "R"))];
        let state = PackState::from_folder(folder("Game", files), Some((2026, 7, 16)));
        let checks = state.validations();
        assert!(checks.errors.is_empty());
        assert!(
            checks.warnings.iter().any(|w| w.contains("missing")),
            "a merged content warning is expected: {:?}",
            checks.warnings
        );
    }

    #[test]
    fn notes_are_reported_in_their_own_tier_and_never_gate() {
        let files = vec![tagged_song("01 Intro.vgz", tag("Game", "A", "R"))];
        let mut state = PackState::from_folder(folder("Game", files), Some((2026, 7, 16)));
        // Credit a second composer the tracks never carry: a note, not a warning.
        state.meta.music_authors = "A & B".to_owned();
        let checks = state.validations();
        assert!(
            checks.notes.iter().any(|n| n.contains("composers")),
            "the composer-set mismatch lands in notes: {:?}",
            checks.notes
        );
        // A note must never leak into the gating tiers.
        assert!(!checks.errors.iter().any(|item| item.contains("composers")));
        assert!(
            !checks
                .warnings
                .iter()
                .any(|item| item.contains("composers"))
        );
    }

    #[test]
    fn date_hyphenation_transaction_rewrites_every_slash_date() {
        let slash = |name: &str| {
            tagged_song(
                name,
                Gd3Tag {
                    game_name_en: "Game".to_owned(),
                    release_date: "1994/03/01".to_owned(),
                    ..Gd3Tag::default()
                },
            )
        };
        let files = vec![slash("01 A.vgm"), slash("02 B.vgm")];
        let state = PackState::from_folder(folder("Game", files), None);
        let txn = state
            .date_hyphenation_transaction()
            .expect("both tracks carry a slash date");
        assert_eq!(txn.forward.len(), 2);
        for mutation in &txn.forward {
            let PackMutation::Write { bytes, .. } = mutation else {
                panic!("date conversion is writes only");
            };
            let file = vgms_core::vgm::file::read("x.vgm", bytes).unwrap();
            assert_eq!(file.tag.as_ref().unwrap().release_date, "1994-03-01");
        }
    }

    #[test]
    fn date_hyphenation_transaction_is_none_when_dates_are_clean() {
        // The fixture tag()'s release date is a hyphen-free "1994": nothing to do.
        let files = vec![tagged_song("01 A.vgm", tag("Game", "A", "R"))];
        let state = PackState::from_folder(folder("Game", files), None);
        assert!(state.date_hyphenation_transaction().is_none());
    }

    #[test]
    fn has_convertible_dates_and_meta_conversion() {
        let files = vec![tagged_song("01 A.vgm", tag("Game", "A", "R"))];
        let mut state = PackState::from_folder(folder("Game", files), None);
        assert!(
            !state.has_convertible_dates(),
            "the fixture's dates are hyphen-free years"
        );

        state.meta.release_date = "1994/03".to_owned();
        assert!(state.has_convertible_dates());
        assert!(state.hyphenate_meta_date(), "the slash date converts");
        assert_eq!(state.meta.release_date, "1994-03");
        assert!(state.dirty);
        // Idempotent: a second pass finds nothing left to fix.
        assert!(!state.hyphenate_meta_date());
    }

    #[test]
    fn the_optimize_toggle_flows_into_the_export_request() {
        let files = vec![tagged_song("01 Intro.vgz", tag("Game", "A", "R"))];
        let mut state = PackState::from_folder(folder("Game", files), Some((2026, 7, 16)));
        assert!(state.optimize_on_export, "defaults on");

        state.optimize_on_export = false;
        assert!(!state.export_request().optimize_vgms);
        state.optimize_on_export = true;
        assert!(state.export_request().optimize_vgms);
    }

    #[test]
    fn export_request_lists_songs_then_docs_with_final_names() {
        let files = vec![
            tagged_song("01 Intro.vgz", tag("Cool Game", "A", "R")),
            tagged_song("02 Boss.vgm", tag("Cool Game", "A", "R")),
        ];
        let state = PackState::from_folder(folder("Cool Game", files), Some((2026, 7, 16)));

        let request = state.export_request();
        assert_eq!(request.zip_name, "Cool Game.zip");
        assert!(request.gzip_vgms);
        assert!(request.optimize_vgms, "optimize-on-export defaults on");
        let names: Vec<&str> = request
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "01 Intro.vgz",
                "02 Boss.vgm",
                "Cool Game.txt",
                "Cool Game.m3u"
            ]
        );

        // The playlist inside the zip names the post-gzip .vgz file.
        let m3u = request
            .entries
            .iter()
            .find(|entry| entry.name.ends_with(".m3u"))
            .unwrap();
        let text = String::from_utf8(m3u.bytes.clone()).unwrap();
        assert!(text.contains("02 Boss.vgz"));
    }

    #[test]
    fn refresh_keeps_edited_metadata_but_re_reads_files() {
        let files = vec![tagged_song("01 Intro.vgz", tag("G", "A", "R"))];
        let mut state = PackState::from_folder(folder("G", files), Some((2026, 7, 16)));
        state.meta.game_name = "Edited Name".to_owned();
        state.dirty = true;

        let more = vec![
            tagged_song("01 Intro.vgz", tag("G", "A", "R")),
            tagged_song("02 New.vgz", tag("G", "B", "R")),
        ];
        state.refresh_files(folder("G", more));
        assert_eq!(state.tracks.len(), 2, "the new file is picked up");
        assert_eq!(state.meta.game_name, "Edited Name", "the edit survives");
        assert!(state.dirty, "and so does the dirty flag");
    }

    #[test]
    fn description_and_m3u_reflect_the_tracks() {
        let files = vec![
            tagged_song("01 Intro.vgz", tag("Cool Game", "Ada", "Ripper")),
            tagged_song("02 Boss.vgm", tag("Cool Game", "Ada", "Ripper")),
        ];
        let state = PackState::from_folder(folder("Cool Game", files), Some((2026, 7, 16)));

        let description = state.description_text();
        assert!(description.contains("Game name:           Cool Game"));
        assert!(description.contains("01 Intro"));

        // The folder playlist keeps real names; the export flips .vgm -> .vgz.
        assert_eq!(state.m3u_text(false), "01 Intro.vgz\r\n02 Boss.vgm\r\n");
        assert_eq!(state.m3u_text(true), "01 Intro.vgz\r\n02 Boss.vgz\r\n");
        assert_eq!(state.doc_stem(), "Cool Game");
    }

    #[test]
    fn can_save_requires_a_game_name() {
        let files = vec![tagged_song("01 Intro.vgz", tag("", "A", "R"))];
        let mut state = PackState::from_folder(folder("Untitled", files), None);
        state.meta.game_name = String::new();
        assert!(!state.can_save());
        state.meta.game_name = "Named".to_owned();
        assert!(state.can_save());
        // A name the file rules empty out would name the pack ".zip", so it is
        // no more savable than a blank one -- and the checklist says so.
        state.meta.game_name = "?!".to_owned();
        assert!(!state.can_save());
        assert!(
            state
                .readiness_items()
                .iter()
                .any(|item| item.message.contains("Enter a game name")),
            "the blocking error still names the field"
        );
    }

    /// A pack whose folder holds `images` alongside one track.
    fn pack_with_images(names: &[&str]) -> PackState {
        let mut files = vec![named_song("01 Intro.vgz", "Intro")];
        files.extend(names.iter().map(|name| PickedFile {
            name: (*name).to_owned(),
            path: Some(PathBuf::from(format!("C:/Cool Game/{name}"))),
            bytes: PNG_FIXTURE.to_vec(),
        }));
        PackState::from_folder(folder("Cool Game", files), None)
    }

    #[test]
    fn an_added_screenshot_never_lands_on_one_already_there() {
        // Nothing in the way: the pack's own name, as before.
        assert_eq!(
            pack_with_images(&[]).next_screenshot_name().as_deref(),
            Some("Cool Game.png")
        );
        // Taken -> numbered, so the first screenshot is not overwritten. The
        // check is case-insensitive: Windows would treat these as one file.
        assert_eq!(
            pack_with_images(&["cool game.png"])
                .next_screenshot_name()
                .as_deref(),
            Some("Cool Game (2).png")
        );
        assert_eq!(
            pack_with_images(&["Cool Game.png", "Cool Game (2).png"])
                .next_screenshot_name()
                .as_deref(),
            Some("Cool Game (3).png")
        );
    }

    #[test]
    fn with_no_game_name_there_is_no_name_to_propose() {
        let mut state = pack_with_images(&[]);
        state.meta.game_name.clear();
        assert_eq!(state.next_screenshot_name(), None);
        // The picked file's own name is then made unique instead.
        assert_eq!(state.free_image_name("dosbox_000", "png"), "dosbox_000.png");
    }

    #[test]
    fn renaming_a_screenshot_is_reversible() {
        let state = pack_with_images(&["dosbox_000.png"]);
        let txn = state
            .rename_image_transaction("dosbox_000.png", "Cool Game (Japan).png")
            .expect("the image is in the folder");
        assert_eq!(
            txn.forward,
            vec![PackMutation::Rename {
                from: PathBuf::from("C:/Cool Game/dosbox_000.png"),
                to: "Cool Game (Japan).png".to_owned(),
            }]
        );
        assert_eq!(
            txn.inverse,
            vec![PackMutation::Rename {
                from: PathBuf::from("C:/Cool Game/Cool Game (Japan).png"),
                to: "dosbox_000.png".to_owned(),
            }]
        );
        // An image that rescanned away has no transaction to run.
        assert!(
            state
                .rename_image_transaction("gone.png", "x.png")
                .is_none()
        );
    }

    #[test]
    fn the_doc_stem_follows_the_same_rules_as_the_track_names() {
        let files = vec![named_song("01 Intro.vgz", "Intro")];
        let mut state = PackState::from_folder(folder("Doom II", files), None);
        state.meta.game_name = "Doom II: Hell on Earth".to_owned();
        assert_eq!(state.doc_stem(), "Doom II - Hell on Earth");
    }

    #[test]
    fn renumber_replaces_the_prefix() {
        assert_eq!(renumber("03 Boss.vgz", 1), "01 Boss.vgz");
        assert_eq!(renumber("10 Long Title.vgm", 4), "04 Long Title.vgm");
        // No prefix -> gains one.
        assert_eq!(renumber("Intro.vgz", 2), "02 Intro.vgz");
    }

    #[test]
    fn reorder_renumbers_every_moved_track() {
        let names = vec![
            "01 A.vgz".to_owned(),
            "02 B.vgz".to_owned(),
            "03 C.vgz".to_owned(),
        ];
        // Move C (index 2) to the front: new order C, A, B.
        assert_eq!(
            reorder_renames(&names, 2, 0),
            vec![
                ("03 C.vgz".to_owned(), "01 C.vgz".to_owned()),
                ("01 A.vgz".to_owned(), "02 A.vgz".to_owned()),
                ("02 B.vgz".to_owned(), "03 B.vgz".to_owned()),
            ]
        );
        // A no-op or out-of-range move renames nothing.
        assert!(reorder_renames(&names, 1, 1).is_empty());
        assert!(reorder_renames(&names, 5, 0).is_empty());
    }

    #[test]
    fn reorder_only_renames_the_tracks_that_actually_moved() {
        let names = vec![
            "01 A.vgz".to_owned(),
            "02 B.vgz".to_owned(),
            "03 C.vgz".to_owned(),
        ];
        // Swap the last two (move index 1 to 2): A keeps 01, only B and C move.
        assert_eq!(
            reorder_renames(&names, 1, 2),
            vec![
                ("03 C.vgz".to_owned(), "02 C.vgz".to_owned()),
                ("02 B.vgz".to_owned(), "03 B.vgz".to_owned()),
            ]
        );
    }

    /// A song tagged with `track_name`, plus the fields the pack meta wants.
    fn named_song(file_name: &str, track_name: &str) -> PickedFile {
        tagged_song(
            file_name,
            Gd3Tag {
                track_name_en: track_name.to_owned(),
                ..tag("Cool Game", "Ada", "Ripper")
            },
        )
    }

    #[test]
    fn tag_renames_lists_only_the_files_that_drifted() {
        let files = vec![
            named_song("01 Intro.vgz", "Intro"), // already correct
            named_song("02 Boss.vgm", "Boss Theme"),
            named_song("03 Untagged.vgz", ""), // nothing to derive a name from
        ];
        let state = PackState::from_folder(folder("Cool Game", files), None);
        assert!(state.has_tag_renames());
        assert_eq!(
            state.tag_renames(),
            vec![("02 Boss.vgm".to_owned(), "02 Boss Theme.vgm".to_owned())],
            "the extension is kept and the untagged track left alone"
        );
    }

    #[test]
    fn tag_renames_follow_the_vgm_ren_character_rules() {
        // The names vgm_ren would write: a colon becomes " - ", a slash ", ",
        // and '?' vanishes -- so a file already named that way is left alone.
        let files = vec![
            named_song("01 Boss.vgz", "Doom II: Hell on Earth"),
            named_song("02 Hard, Soft.vgz", "Hard / Soft"),
            named_song("03 Who.vgz", "Who?"),
        ];
        let state = PackState::from_folder(folder("Cool Game", files), None);
        assert_eq!(
            state.tag_renames(),
            vec![(
                "01 Boss.vgz".to_owned(),
                "01 Doom II - Hell on Earth.vgz".to_owned()
            )]
        );
    }

    #[test]
    fn rename_from_tags_transaction_is_temp_safe_and_reversible() {
        // A swap through the tags: each file wants the other's name. Renaming
        // straight over would clobber one, so the batch must stage via temps.
        let files = vec![
            named_song("01 A.vgz", "B"),
            named_song("02 B.vgz", "A"), // wants "02 A.vgz"
        ];
        let state = PackState::from_folder(folder("Cool Game", files), None);
        let txn = state
            .rename_from_tags_transaction()
            .expect("both names drifted");
        assert_eq!(txn.forward.len(), 4, "a temp-then-final batch");
        assert_eq!(txn.inverse.len(), 4);
        let finals: Vec<&String> = txn
            .forward
            .iter()
            .filter_map(|mutation| match mutation {
                PackMutation::Rename { to, .. } if !to.starts_with(".vgmstudio") => Some(to),
                _ => None,
            })
            .collect();
        assert_eq!(finals, ["01 B.vgz", "02 A.vgz"]);

        // Nothing adrift -> no transaction at all.
        let tidy = PackState::from_folder(
            folder("Cool Game", vec![named_song("01 Intro.vgz", "Intro")]),
            None,
        );
        assert!(tidy.rename_from_tags_transaction().is_none());
        assert!(!tidy.has_tag_renames());
    }

    #[test]
    fn reorder_transaction_is_temp_safe_and_reversible() {
        let files = vec![
            tagged_song("01 Intro.vgz", tag("G", "A", "R")),
            tagged_song("02 Boss.vgm", tag("G", "B", "R")),
        ];
        let state = PackState::from_folder(folder("G", files), None);

        // Swap the two tracks: both move, so 2 renames -> a 4-step temp-then-final
        // batch, forward and inverse alike.
        let txn = state
            .reorder_transaction(0, 1)
            .expect("a real move builds a transaction");
        assert_eq!(txn.forward.len(), 4);
        assert_eq!(txn.inverse.len(), 4);

        let finals = |muts: &[PackMutation]| -> Vec<String> {
            muts.iter()
                .filter_map(|m| match m {
                    PackMutation::Rename { to, .. } if !to.starts_with(".vgmstudio") => {
                        Some(to.clone())
                    }
                    _ => None,
                })
                .collect()
        };
        // Forward renumbers to the new order; inverse restores the original names.
        let fwd = finals(&txn.forward);
        assert!(fwd.contains(&"01 Boss.vgm".to_owned()));
        assert!(fwd.contains(&"02 Intro.vgz".to_owned()));
        let inv = finals(&txn.inverse);
        assert!(inv.contains(&"01 Intro.vgz".to_owned()));
        assert!(inv.contains(&"02 Boss.vgm".to_owned()));

        // A no-op or out-of-range move builds nothing.
        assert!(state.reorder_transaction(0, 0).is_none());
        assert!(state.reorder_transaction(0, 9).is_none());
    }
}
