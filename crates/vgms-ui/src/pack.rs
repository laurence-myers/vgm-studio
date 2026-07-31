//! Pack mode: preparing a VGMRips submission from a folder of songs.
//!
//! [`PackState`] is the headless core -- the loaded folder, the editable package
//! metadata, and the derived track list -- with no egui, so it is testable
//! without a window (like [`crate::editor::Editor`]). [`show`] draws the view.
//!
//! The description file *is* the project: opening a folder re-parses any
//! `Game Name.txt` back into the form, so a pack can be reopened and updated.
//! When there is no description (a fresh pack), the fields are prefilled from the
//! songs' GD3 tags.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use vgms_synth::Peak;

use egui_extras::{Column, TableBuilder};
use vgms_core::pack::{
    CONSOLE_PRESETS, DEFAULT_OS, DEFAULT_SYSTEM, MetaField, PRESETS, PackMeta, PngInfo,
    ReadinessCategory, ReadinessItem, ReadinessTarget, Severity, TrackEntry, TrackFacts,
    doc_file_stem, format_byte_count, format_track_time, generate_description, generate_m3u,
    music_hardware_suggestion, parse_description, readiness,
};
use vgms_core::vgm::data::GD3_FIELD_COUNT;
use vgms_core::{Gd3Tag, OplType, Song, VgmFile};
use vgms_synth::AudioSource;

use crate::action::Action;
use crate::platform::{PackEntry, PackEntryKind, PackJobRequest, PickedFile, PickedFolder};
use crate::theme::{Palette, bevel};

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
    #[must_use]
    pub fn is_playable(&self) -> bool {
        self.preview_source().is_some()
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
        let chips: Vec<_> = file.header.chips().iter().map(|chip| chip.kind).collect();
        vgms_synth::playability(&chips)
            .missing()
            .iter()
            .map(|chip| chip.name())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The track as something an engine can play, or `None` when nothing would
    /// be heard.
    ///
    /// An OPL track goes to the OPL player, which is what gives the preview its
    /// per-channel panning; anything else goes to the generic engine. A track
    /// whose chips all lack cores is not offered, because a preview button that
    /// plays silence is worse than one that is not there.
    #[must_use]
    pub fn preview_source(&self) -> Option<AudioSource> {
        let file = self.vgm()?;
        if let Some(song) = file.to_song() {
            return Some(AudioSource::Opl(Arc::new(song)));
        }
        let chips: Vec<_> = file.header.chips().iter().map(|chip| chip.kind).collect();
        vgms_synth::playability(&chips)
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

/// The whole pack project: what a folder scan produced, plus the editable
/// package metadata.
#[derive(Debug)]
pub struct PackState {
    pub folder_name: String,
    pub folder_path: Option<PathBuf>,
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
    /// [`vgms_core::pack::hyphenate_date`]).
    #[must_use]
    pub fn has_convertible_dates(&self) -> bool {
        vgms_core::pack::hyphenate_date(&self.meta.release_date).is_some()
            || self.tracks.iter().any(|track| {
                track
                    .tag()
                    .is_some_and(|tag| vgms_core::pack::hyphenate_date(&tag.release_date).is_some())
            })
    }

    /// Converts the pack's release date from slashes to hyphens in place, if it is
    /// a convertible slash date. Returns whether it changed (and marks the pack
    /// dirty). A pack-metadata edit, like typing in the form -- not a file op.
    pub fn hyphenate_meta_date(&mut self) -> bool {
        if let Some(hyphenated) = vgms_core::pack::hyphenate_date(&self.meta.release_date) {
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
            let Some(hyphenated) = vgms_core::pack::hyphenate_date(&tag.release_date) else {
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
    /// already named correctly. See [`vgms_core::pack::tag_file_name`]: the title
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
    /// checks from [`vgms_core::pack::readiness`]. One list feeds both the export
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
    vgms_core::pack::tag_file_name(index + 1, tag.track_name_en.trim(), &track.file_name)
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

// -- view --------------------------------------------------------------------

/// Draws the pack view: the pack's name, the sub-section tabs, the batch tools
/// that belong to the open section, and that section's body.
pub fn show(
    ui: &mut egui::Ui,
    state: &mut PackState,
    palette: &Palette,
    scanning: bool,
    actions: &mut Vec<Action>,
) {
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);

    // What is open, on a row of its own so the pack's identity never moves or
    // shares space with a control that might wrap away from it.
    ui.horizontal(|ui| {
        ui.visuals_mut().override_text_color = Some(palette.data_label);
        ui.label(egui::RichText::new(&state.folder_name).strong());
        if state.dirty {
            ui.colored_label(palette.data_text, "\u{2022}")
                .on_hover_text(crate::strings::PACK_DIRTY_TIP);
        }
    });

    section_tabs(ui, state, palette, actions);

    // The batch tools edit the *tracks*, so they live with them rather than
    // riding above every section. Everything that produces the submission is on
    // the output deck at the foot of the window instead -- batch and export are
    // different verbs, and mixing them in one row was what overflowed the old
    // header.
    if state.section == PackSection::Tracks {
        track_tools(ui, state, palette, scanning, actions);
    }

    if let Some(warning) = &state.parse_warning {
        ui.colored_label(palette.muted, warning);
    }
    crate::theme::separator_full(ui, palette);

    // Horizontal `auto_shrink` off: left on, a scroll area shrinks to its
    // content, which would park the scrollbar against the right edge of whatever
    // the widest widget happens to be -- in the middle of the panel on the
    // narrow Tags form -- instead of at the panel edge. Vertical shrink stays on,
    // so a short section does not claim the whole viewport as one big hit target.
    let scroll_out = egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| match state.section {
            PackSection::Tags => meta_form(ui, state, palette),
            PackSection::Tracks => {
                // The table's status glyphs read the same readiness list the
                // checklist does, so the two can never disagree.
                let items = state.readiness_items();
                // Taken here so the request cannot re-fire every frame; the row
                // it names asks to be scrolled to as it draws.
                let scroll_to = state.scroll_to_track.take();
                track_table(ui, state, &items, scroll_to, palette, actions);
            }
            PackSection::Screenshots => screenshots(ui, state, palette, actions),
            PackSection::Checklist => {
                let items = state.readiness_items();
                submission_checklist(ui, state, &items, palette, actions);
            }
        });

    // When the section overflows, frame the vertical scrollbar's channel with the
    // sunken well bevel, flush to the panel edge -- the same treatment the editor
    // table's bar gets.
    if scroll_out.content_size.y > scroll_out.inner_rect.height() {
        let bar_width = ui.spacing().scroll.bar_width;
        let viewport = scroll_out.inner_rect;
        let bar = egui::Rect::from_min_max(
            egui::pos2(viewport.right(), viewport.top()),
            egui::pos2(viewport.right() + bar_width, viewport.bottom()),
        );
        crate::theme::frame_scrollbar(ui, palette, bar);
    }
}

/// The sub-section strip. Uses the same tab chrome as the Editor/Pack strip, so
/// switching section reads as the same gesture as switching view.
fn section_tabs(
    ui: &mut egui::Ui,
    state: &PackState,
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    let tabs: Vec<crate::theme::tabs::Tab> = PackSection::ALL
        .iter()
        .map(|section| crate::theme::tabs::Tab::new(section.label()))
        .collect();
    let selected = PackSection::ALL
        .iter()
        .position(|section| *section == state.section)
        .unwrap_or(0);
    if let Some(index) = crate::theme::tabs::strip(ui, palette, &tabs, selected) {
        actions.push(Action::PackSelectSection(PackSection::ALL[index]));
    }
}

/// The batch operations that edit the folder in place, grouped by what they
/// touch. Shown with the track list, which is what they act on.
fn track_tools(
    ui: &mut egui::Ui,
    state: &mut PackState,
    palette: &Palette,
    scanning: bool,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        crate::theme::silkscreen_group(ui, palette.data_label, "LEVELS", |ui| {
            // Greyed while the scan runs: clicking again would cancel it and
            // start over, which reads as the button doing nothing.
            ui.add_enabled_ui(!scanning, |ui| {
                if bevel::button(ui, palette, "Scan Volumes")
                    .on_hover_text(if scanning {
                        crate::strings::PACK_SCAN_VOLUMES_TIP_SCANNING
                    } else {
                        crate::strings::PACK_SCAN_VOLUMES_TIP
                    })
                    .clicked()
                {
                    actions.push(Action::PackScanVolumes);
                }
            });
            // Both wait on the scan: until a peak has been measured there is
            // nothing to level from, and Apply could only say so.
            let scanned = !state.peaks.is_empty();
            ui.add_enabled_ui(scanned, |ui| {
                if bevel::button(ui, palette, "Apply")
                    .on_hover_text(if scanned {
                        crate::strings::PACK_APPLY_TIP_SCANNED
                    } else {
                        crate::strings::PACK_APPLY_TIP_UNSCANNED
                    })
                    .clicked()
                {
                    actions.push(Action::PackApplySuggestedModifiers {
                        album: state.album_normalize,
                    });
                }
                // A lit pad, not a checkbox: this modifies what Apply does, so
                // it belongs beside it, and "lit = on" is the chrome's own rule.
                bevel::toggle(ui, palette, &mut state.album_normalize, "Album")
                    .on_hover_text(crate::strings::PACK_ALBUM_TIP);
            });
        });
        crate::theme::silkscreen_group(ui, palette.data_label, "TAGS", |ui| {
            if bevel::button(ui, palette, "Bulk Tag\u{2026}")
                .on_hover_text(crate::strings::PACK_BULK_TAG_TIP)
                .clicked()
            {
                actions.push(Action::OpenBulkTag);
            }
            // A fix-assist for the most common mechanical problem, greyed rather
            // than hidden once there is no slash date left to convert.
            ui.add_enabled_ui(state.has_convertible_dates(), |ui| {
                if bevel::button(ui, palette, "Fix Dates")
                    .on_hover_text(crate::strings::PACK_FIX_DATES_TIP)
                    .clicked()
                {
                    actions.push(Action::PackConvertDatesToHyphens);
                }
            });
            // The other mechanical fix: pull every file name back into step with
            // the tag it should have come from.
            ui.add_enabled_ui(state.has_tag_renames(), |ui| {
                if bevel::button(ui, palette, "Fix File Names")
                    .on_hover_text(crate::strings::PACK_FIX_FILE_NAMES_TIP)
                    .clicked()
                {
                    actions.push(Action::PackRenameFromTags);
                }
            });
        });
    });
}

/// The package-metadata form: what the description file is generated from.
fn meta_form(ui: &mut egui::Ui, state: &mut PackState, palette: &Palette) {
    // The checklist may have asked (last frame) to focus a field; take that
    // request now so the form honours it this frame and it does not re-fire.
    // Taken here rather than in `show` so a request made from another section
    // survives until this one is actually drawn.
    let focus = state.focus_field.take();
    let mut dirty = false;
    egui::Grid::new("pack-meta")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            dirty |= field(
                ui,
                palette,
                "Game name:",
                &mut state.meta.game_name,
                Some(MetaField::GameName),
                focus,
            );
            // One-click presets for the three hardware fields below, the OPL PC
            // cards first then the common non-OPL systems -- a dropdown rather
            // than a button per system, which had grown to two wrapped rows.
            // Picking one fills the fields and the box reverts to its prompt,
            // since the fields stay editable afterwards.
            ui.label("Presets:");
            ui.scope(|ui| {
                crate::theme::style_dropdown(ui, palette);
                let mut picked = String::new();
                egui::ComboBox::from_id_salt("pack-preset")
                    .selected_text(crate::strings::PACK_PRESET_PROMPT)
                    .show_ui(ui, |ui| {
                        for preset in PRESETS.iter().chain(&CONSOLE_PRESETS) {
                            // Skip an empty OS in the hover so a cartridge console
                            // reads "System / hardware", not "System /  / hardware".
                            let hover = [preset.system, preset.os, preset.music_hardware]
                                .into_iter()
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join(" / ");
                            ui.selectable_value(&mut picked, preset.name.to_owned(), preset.name)
                                .on_hover_text(hover);
                        }
                    });
                if let Some(preset) = PRESETS
                    .iter()
                    .chain(&CONSOLE_PRESETS)
                    .find(|preset| preset.name == picked)
                {
                    state.meta.system = preset.system.to_owned();
                    state.meta.os = preset.os.to_owned();
                    state.meta.music_hardware = preset.music_hardware.to_owned();
                    dirty = true;
                }
            });
            ui.end_row();
            dirty |= hardware_fields(ui, state, palette, focus);
            dirty |= field(
                ui,
                palette,
                "Music author:",
                &mut state.meta.music_authors,
                Some(MetaField::MusicAuthors),
                focus,
            );
            dirty |= field(
                ui,
                palette,
                "Game developer:",
                &mut state.meta.developer,
                None,
                focus,
            );
            dirty |= field(
                ui,
                palette,
                "Game publisher:",
                &mut state.meta.publisher,
                None,
                focus,
            );
            dirty |= field(
                ui,
                palette,
                "Game release date:",
                &mut state.meta.release_date,
                Some(MetaField::ReleaseDate),
                focus,
            );
            dirty |= field(
                ui,
                palette,
                "Package created by:",
                &mut state.meta.creator,
                Some(MetaField::Creator),
                focus,
            );
            dirty |= field(
                ui,
                palette,
                "Package version:",
                &mut state.meta.version,
                None,
                focus,
            );
        });

    ui.add_space(4.0);
    dirty |= multiline(ui, palette, "Notes:", &mut state.meta.notes, None, focus);
    dirty |= multiline(
        ui,
        palette,
        "Package history:",
        &mut state.meta.history,
        Some(MetaField::History),
        focus,
    );
    if dirty {
        state.dirty = true;
    }
}

/// System / OS / music hardware, behind a disclosure. A preset sets all three,
/// which is how nearly every pack fills them in, so they are folded away by
/// default -- but their current values are summarised on the disclosure row, so
/// collapsing hides the *editing*, never the facts. Returns whether one changed.
fn hardware_fields(
    ui: &mut egui::Ui,
    state: &mut PackState,
    palette: &Palette,
    focus: Option<MetaField>,
) -> bool {
    ui.label("Hardware:");
    // An inline triangle glyph that toggles the fields, matching the Settings
    // "All chips" disclosure -- a clickable muted label, not a pad button. When
    // folded, the field summary trails the glyph in the same clickable label, so
    // collapsing hides the editing, never the facts. CP437 triangles, as the
    // volume stepper uses, so the DOS face has the glyph rather than a box.
    let (glyph, tip) = if state.show_hardware {
        ("\u{25BC}", crate::strings::PACK_HARDWARE_TIP_HIDE)
    } else {
        ("\u{25BA}", crate::strings::PACK_HARDWARE_TIP_EDIT)
    };
    let text = if state.show_hardware {
        glyph.to_owned()
    } else {
        let summary = [
            state.meta.system.as_str(),
            state.meta.os.as_str(),
            state.meta.music_hardware.as_str(),
        ]
        .iter()
        .filter(|value| !value.trim().is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" \u{00B7} ");
        if summary.is_empty() {
            glyph.to_owned()
        } else {
            format!("{glyph} {summary}")
        }
    };
    let header = ui
        .add(
            egui::Label::new(egui::RichText::new(text).color(palette.muted))
                .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip);
    if header.clicked() {
        state.show_hardware = !state.show_hardware;
    }
    ui.end_row();
    if !state.show_hardware {
        return false;
    }
    let mut dirty = field(ui, palette, "System:", &mut state.meta.system, None, focus);
    dirty |= field(ui, palette, "OS:", &mut state.meta.os, None, focus);
    dirty |= field(
        ui,
        palette,
        "Music hardware:",
        &mut state.meta.music_hardware,
        None,
        focus,
    );
    dirty
}

/// Draws the output deck: the pack's readiness lamp on the left, and on the
/// right everything that turns the folder into a submission -- the two export
/// options, the docs, and the zip itself.
///
/// The app hosts this as a bottom panel, so export is reachable however far the
/// form and track list have scrolled, and the verdict sits beside the gate it
/// reports on rather than paragraphs away from it.
pub fn deck(
    ui: &mut egui::Ui,
    state: &mut PackState,
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    let (severity, summary) = state.readiness_summary();
    ui.horizontal(|ui| {
        ui.set_min_height(ui.spacing().interact_size.y);
        ui.spacing_mut().item_spacing.x = 6.0;
        crate::theme::led(ui, lamp_colour(severity, palette)).on_hover_text(match severity {
            None => crate::strings::PACK_READINESS_TIP_NONE,
            Some(Severity::Error) => crate::strings::PACK_READINESS_TIP_ERROR,
            Some(Severity::Warning) => crate::strings::PACK_READINESS_TIP_WARNING,
            Some(Severity::Note) => crate::strings::PACK_READINESS_TIP_NOTE,
        });
        ui.label(summary);
        // Only worth a jump when the checklist has something to say.
        if severity.is_some() {
            let ink = crate::theme::deck_ink(palette);
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("view checklist").color(ink).underline())
                        .frame(false),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(crate::strings::PACK_VIEW_CHECKLIST_TIP)
                .clicked()
            {
                actions.push(Action::PackSelectSection(PackSection::Checklist));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if bevel::button(ui, palette, "Export Zip\u{2026}")
                .on_hover_text(crate::strings::PACK_EXPORT_ZIP_TIP)
                .clicked()
            {
                actions.push(Action::PackExportZip);
            }
            if bevel::button(ui, palette, "Save Pack")
                .on_hover_text(crate::strings::PACK_SAVE_DOCS_TIP)
                .clicked()
            {
                actions.push(Action::PackSaveDocs);
            }
            crate::theme::separator(ui, palette);
            // The two export options, as lit pads: the tooltip carries the
            // detail, and "lit = on" is the same rule every other pad follows.
            bevel::toggle(ui, palette, &mut state.optimize_on_export, "Opt.")
                .on_hover_text(crate::strings::PACK_OPT_TIP);
            bevel::toggle(ui, palette, &mut state.gzip_on_export, "VGZ")
                .on_hover_text(crate::strings::PACK_VGZ_TIP);
        });
    });
}

/// The colour the deck's verdict lamp burns for a readiness severity. Only an
/// error is red: a warning merely prompts on export, and a note never gates at
/// all, so both of those read as "the pack can ship".
fn lamp_colour(severity: Option<Severity>, palette: &Palette) -> egui::Color32 {
    match severity {
        Some(Severity::Error) => palette.meter_high,
        Some(Severity::Warning) => palette.meter_mid,
        None | Some(Severity::Note) => palette.meter_low,
    }
}

/// Re-serialises `song` under `new_name` with `tag` applied. The name drives the
/// output format, so a `.vgm` -> `.vgz` rename gzips the result. Used by the
/// quick-edit dialog to rewrite a track without loading it into the editor.
pub fn retagged_bytes(song: &Song, new_name: &str, tag: Gd3Tag) -> Result<Vec<u8>, String> {
    let mut song = song.clone();
    song.name = new_name.to_owned();
    if let Some(meta) = song.vgm_meta_mut() {
        meta.tag = Some(tag);
    }
    vgms_core::io::write_song(&song).map_err(|error| error.to_string())
}

// GD3 field indices (file order), for the bulk-tag seeding below. The "native"
// fields are GD3's original-language variants, paired with their English siblings.
mod gd3_index {
    pub(super) const GAME_NAME_EN: usize = 2;
    pub(super) const GAME_NAME_NATIVE: usize = 3;
    pub(super) const SYSTEM_NAME_EN: usize = 4;
    pub(super) const SYSTEM_NAME_NATIVE: usize = 5;
    pub(super) const TRACK_AUTHOR_EN: usize = 6;
    pub(super) const TRACK_AUTHOR_NATIVE: usize = 7;
    pub(super) const RELEASE_DATE: usize = 8;
    pub(super) const CREATOR: usize = 9;
}

/// A bulk GD3 edit: which of the eleven fields to write, and the value for each.
///
/// Applying it overlays only the *checked* fields onto a track's existing tag,
/// so every unchecked field keeps that track's own value. That is the whole
/// point of a bulk edit: correct the composer on half the tracks, or stamp the
/// shared game name onto all of them, without disturbing anything else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BulkTagOverlay {
    /// Per field, in GD3 file order: whether `values[i]` is written.
    pub apply: [bool; GD3_FIELD_COUNT],
    /// Per field, in GD3 file order: the value written when `apply[i]` is set.
    pub values: [String; GD3_FIELD_COUNT],
}

impl BulkTagOverlay {
    /// The tag that results from writing the checked fields onto `base`, leaving
    /// the unchecked fields at their existing values.
    #[must_use]
    pub fn apply_to(&self, base: &Gd3Tag) -> Gd3Tag {
        let mut fields = base.fields().map(str::to_owned);
        for (slot, (on, value)) in fields.iter_mut().zip(self.apply.iter().zip(&self.values)) {
            if *on {
                slot.clone_from(value);
            }
        }
        Gd3Tag::from_fields(fields)
    }

    /// Whether any field is checked. With none, a bulk edit has nothing to do.
    #[must_use]
    pub fn writes_anything(&self) -> bool {
        self.apply.iter().any(|&on| on)
    }
}

/// Seeds a bulk edit from the package metadata: the GD3 fields a pack typically
/// shares across every track.
///
/// Game, system, composer, release date and ripper are pre-filled and
/// pre-checked when present, so opening the dialog on a filled-in pack and
/// hitting Apply writes them to every track with no extra clicks. The
/// original-language ("orig") variants of game, system and composer are seeded
/// with the same values -- the pack metadata holds no separate native names,
/// and this app's PC/AT games rarely have one, so mirroring the English value
/// keeps both variants filled. The two track-name fields are never seeded: a
/// title is per-track by definition. To tag a subset with a different value
/// (say, the half of the pack a second composer wrote), edit the value and
/// deselect the tracks it does not apply to.
#[must_use]
pub fn seed_from_meta(meta: &PackMeta) -> BulkTagOverlay {
    let mut overlay = BulkTagOverlay::default();
    let seeds = [
        (gd3_index::GAME_NAME_EN, &meta.game_name),
        (gd3_index::GAME_NAME_NATIVE, &meta.game_name),
        (gd3_index::SYSTEM_NAME_EN, &meta.system),
        (gd3_index::SYSTEM_NAME_NATIVE, &meta.system),
        (gd3_index::TRACK_AUTHOR_EN, &meta.music_authors),
        (gd3_index::TRACK_AUTHOR_NATIVE, &meta.music_authors),
        (gd3_index::RELEASE_DATE, &meta.release_date),
        (gd3_index::CREATOR, &meta.creator),
    ];
    for (index, value) in seeds {
        overlay.values[index] = value.clone();
        overlay.apply[index] = !value.trim().is_empty();
    }
    overlay
}

/// The width of a one-line metadata field. Kept to a readable line rather than
/// stretched across the window the way the notes boxes below are: this is a
/// full-width panel, not a dialog, and a game name in a 900pt box reads as a
/// mistake. Values longer than it wrap inside it (see [`dialogs::text_field`]).
///
/// [`dialogs::text_field`]: crate::dialogs::text_field
const FIELD_WIDTH: f32 = 340.0;

/// A labelled one-line field. `meta_field` names it so the submission
/// checklist can jump here: when it matches `focus`, the field grabs keyboard
/// focus and scrolls into view this frame. Returns whether it changed.
///
/// The same wrapping field the dialogs use: a game name longer than the box
/// wraps and pushes it taller instead of scrolling out of sight.
fn field(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    value: &mut String,
    meta_field: Option<MetaField>,
    focus: Option<MetaField>,
) -> bool {
    ui.label(label);
    let response = crate::dialogs::text_field(ui, palette, value, FIELD_WIDTH);
    focus_if_targeted(&response, meta_field, focus);
    ui.end_row();
    response.changed()
}

fn multiline(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    value: &mut String,
    meta_field: Option<MetaField>,
    focus: Option<MetaField>,
) -> bool {
    ui.label(label);
    let response = ui.add(
        egui::TextEdit::multiline(value)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace)
            .text_color(palette.data_text),
    );
    focus_if_targeted(&response, meta_field, focus);
    response.changed()
}

/// Grabs focus for and scrolls to a field the checklist asked to jump to.
fn focus_if_targeted(
    response: &egui::Response,
    meta_field: Option<MetaField>,
    focus: Option<MetaField>,
) {
    if meta_field.is_some() && meta_field == focus {
        response.request_focus();
        response.scroll_to_me(Some(egui::Align::Center));
    }
}

/// The glyph and colour that mark a severity in the checklist and the track
/// table's status column. The bundled IBM VGA font has no check/warning glyphs,
/// so this uses a CP437 tick (`\u{221A}`, handled by the caller) and a
/// colour-coded `!`.
fn severity_marker(severity: Severity, palette: &Palette) -> (&'static str, egui::Color32) {
    match severity {
        Severity::Error => ("!", palette.meter_high),
        Severity::Warning => ("!", palette.meter_mid),
        Severity::Note => ("\u{00B7}", palette.data_label), // middle dot
    }
}

/// The most severe severity among some items -- the glyph a group's heading wears.
fn worst_severity(items: &[&ReadinessItem]) -> Severity {
    if items.iter().any(|item| item.severity == Severity::Error) {
        Severity::Error
    } else if items.iter().any(|item| item.severity == Severity::Warning) {
        Severity::Warning
    } else {
        Severity::Note
    }
}

/// Draws the submission checklist: one line per category -- a green tick when the
/// category is clean, otherwise a heading that folds its findings away, each one
/// a clickable line that jumps to the fix (a meta field opens the Tags form with
/// that field focused; a track opens its quick-edit dialog).
fn submission_checklist(
    ui: &mut egui::Ui,
    state: &mut PackState,
    items: &[ReadinessItem],
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    ui.add_space(2.0);
    let loops = state.loop_tally();
    for (slot, category) in ReadinessCategory::ALL.into_iter().enumerate() {
        let group: Vec<&ReadinessItem> = items
            .iter()
            .filter(|item| item.category == category)
            .collect();
        // A tally beside the heading answers "how much of this is done" without
        // expanding the group. Only Loops has a meaningful one so far.
        let tally = (category == ReadinessCategory::Loops)
            .then(|| format!("{}/{} looping", loops.0, loops.1));

        if group.is_empty() {
            // Nothing to fold away, so a clean category stays a plain tick line.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(DISCLOSURE_WIDTH);
                ui.colored_label(palette.meter_low, "\u{221A}"); // CP437 tick
                ui.colored_label(palette.muted, category.label());
                if let Some(tally) = tally {
                    ui.colored_label(palette.muted, tally);
                }
            });
            continue;
        }

        let open = !state.collapsed[slot];
        let (glyph, color) = severity_marker(worst_severity(&group), palette);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if disclosure(ui, palette, open, category.label()).clicked() {
                state.collapsed[slot] = open;
            }
            ui.colored_label(color, glyph);
            ui.colored_label(
                palette.data_label,
                egui::RichText::new(category.label()).strong(),
            );
            if let Some(tally) = tally {
                ui.colored_label(palette.muted, tally);
            }
            // Collapsed, the count is the only sign of what is inside.
            if !open {
                ui.colored_label(
                    palette.muted,
                    format!(
                        "({} item{})",
                        group.len(),
                        if group.len() == 1 { "" } else { "s" }
                    ),
                );
            }
        });
        if !open {
            continue;
        }
        for item in group {
            checklist_item(ui, state, item, palette, actions);
        }
    }
}

/// The width a disclosure triangle occupies, so a clean category's tick lines up
/// with the ticks of the categories that have one.
const DISCLOSURE_WIDTH: f32 = 18.0;

/// A fold/unfold triangle for a checklist category. Frameless, so a row of them
/// reads as an outline rather than a row of buttons.
///
/// The glyph would be its whole accessible name, so the name is set explicitly:
/// a screen reader (and `get_by_label`) needs to know *which* group a triangle
/// belongs to, and five identical `\u{25BC}`s say nothing.
fn disclosure(ui: &mut egui::Ui, palette: &Palette, open: bool, label: &str) -> egui::Response {
    let glyph = if open { "\u{25BC}" } else { "\u{25B6}" };
    let name = if open {
        format!("Hide {label}")
    } else {
        format!("Show {label}")
    };
    let response = ui
        .add_sized(
            egui::vec2(DISCLOSURE_WIDTH, ui.spacing().interact_size.y),
            egui::Button::new(egui::RichText::new(glyph).color(palette.data_label)).frame(false),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &name));
    response
}

/// One checklist line under a category heading: an indented, colour-coded marker
/// and the message, clickable when it points at a field or track to fix.
fn checklist_item(
    ui: &mut egui::Ui,
    state: &mut PackState,
    item: &ReadinessItem,
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    let (glyph, color) = severity_marker(item.severity, palette);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.colored_label(color, glyph);
        match item.target {
            ReadinessTarget::Meta(field) => {
                if checklist_link(ui, palette, &item.message).clicked() {
                    // The field lives on the Tags form, so the jump has to change
                    // section as well; both are honoured next frame, when that
                    // form draws and takes `focus_field`.
                    state.section = PackSection::Tags;
                    state.focus_field = Some(field);
                    ui.ctx().request_repaint();
                }
            }
            ReadinessTarget::Track(index) => {
                if checklist_link(ui, palette, &item.message).clicked() {
                    actions.push(Action::OpenTrackQuickEdit(index));
                }
            }
            ReadinessTarget::Pack => {
                // Wrapped explicitly: inside a row a plain label extends, and
                // these messages are sentences -- the loop note is a whole list.
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(item.message.as_str()).color(palette.muted),
                    )
                    .wrap(),
                );
            }
        }
    });
}

/// A clickable checklist message: a frameless button that reads as plain data
/// text (so it is a proper click/keyboard target), with a hand cursor and a hint
/// on hover.
///
/// It **wraps**. These messages are sentences, and several run past 90
/// characters -- at the app's default 800pt window an extending line overflowed
/// the panel and was drawn straight over the vertical scrollbar, burying the
/// handle (which is what "the scrollbar has no puck" turned out to be).
fn checklist_link(ui: &mut egui::Ui, palette: &Palette, message: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(message).color(palette.data_text))
            .frame(false)
            .wrap_mode(egui::TextWrapMode::Wrap),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .on_hover_text(crate::strings::PACK_CHECKLIST_LINK_TIP)
}

/// The track table's status cell: a green tick when the track is submission-ready,
/// otherwise a colour-coded `!` whose tooltip lists the track's problems and which
/// opens the track's quick-edit on click (an unreadable track has no tag to edit,
/// so its marker is informational only).
fn track_status_glyph(
    ui: &mut egui::Ui,
    index: usize,
    track: &PackTrack,
    items: &[ReadinessItem],
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    let unreadable = !track.is_readable();
    let problems: Vec<&str> = items
        .iter()
        .filter(|item| item.target == ReadinessTarget::Track(index))
        .map(|item| item.message.as_str())
        .collect();
    if !unreadable && problems.is_empty() {
        ui.colored_label(palette.meter_low, "\u{221A}")
            .on_hover_text(crate::strings::PACK_TRACK_READY_TIP);
        return;
    }
    let tooltip = if unreadable {
        crate::strings::PACK_TRACK_UNREADABLE_TIP.to_owned()
    } else {
        problems.join("\n")
    };
    let response = ui
        .add(
            egui::Label::new(
                egui::RichText::new("!")
                    .monospace()
                    .strong()
                    .color(palette.meter_mid),
            )
            .sense(egui::Sense::click()),
        )
        .on_hover_text(tooltip);
    if !unreadable {
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            actions.push(Action::OpenTrackQuickEdit(index));
        }
    }
}

fn track_table(
    ui: &mut egui::Ui,
    state: &PackState,
    items: &[ReadinessItem],
    scroll_to: Option<usize>,
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    ui.label(
        egui::RichText::new(crate::strings::PACK_TRACKS_HEADING)
            .color(palette.data_label)
            .strong(),
    );
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 6.0;
    // A sunken data well on the desktop, like the editor's table. The table's
    // own vertical scrolling is off so the whole view scrolls as one.
    let frame = egui::Frame::new()
        .fill(palette.data_bg)
        .inner_margin(egui::Margin::same(4));
    frame.show(ui, |ui| {
        // The rows are click / double-click targets; a selectable label inside a
        // cell would otherwise capture the drag as a text selection -- showing an
        // I-beam and swallowing the double-click -- so turn label selection off
        // for the whole table. The row stays highlighted on hover instead.
        ui.style_mut().interaction.selectable_labels = false;
        // Every fixed column states the width it actually draws in, rather than a
        // floor the row then overruns: `remainder` budgets the title from these
        // figures, so an under-declared cell is laid out over the scrollbar and
        // off the panel edge.
        let mut row_rects: Vec<egui::Rect> = Vec::new();
        TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .vscroll(false)
            .column(Column::exact(GRIP_WIDTH)) // drag handle
            .column(Column::exact(14.0)) // status glyph
            .column(Column::exact(24.0)) // #
            .column(Column::exact(16.0)) // preview
            .column(Column::remainder().at_least(120.0).clip(true)) // Title (GD3)
            .column(Column::exact(46.0)) // Total
            .column(Column::exact(46.0)) // Loop
            .column(Column::exact(50.0)) // Peak (dBFS)
            .column(Column::exact(20.0)) // row menu
            .header(row_height + 2.0, |mut header| {
                for title in ["", "", "#", "", "Title (GD3)", "Total", "Loop", "Peak", ""] {
                    header.col(|ui| {
                        ui.label(
                            egui::RichText::new(title)
                                .monospace()
                                .color(palette.data_text),
                        );
                    });
                }
            })
            .body(|mut body| {
                for (index, track) in state.tracks.iter().enumerate() {
                    body.row(row_height, |mut row| {
                        // The keyboard's row, lit like a selection so Alt+arrow
                        // has something visible to act on.
                        row.set_selected(state.focused_track == Some(index));
                        row.col(|ui| {
                            // The row's own response rect overshoots into the
                            // next row; a cell's does not, and the y-range is
                            // all the drop target needs.
                            row_rects.push(ui.max_rect());
                            drag_grip(ui, index, track, palette);
                        });
                        row.col(|ui| {
                            track_status_glyph(ui, index, track, items, palette, actions);
                        });
                        row.col(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{:02}", index + 1))
                                    .monospace()
                                    .color(palette.muted),
                            )
                            .on_hover_text(&track.file_name);
                        });
                        row.col(|ui| {
                            // The preview control sits with the thing it plays,
                            // as an inline glyph rather than a keycap: a row of
                            // pads is what pushed this table off its own edge.
                            // Playable means a chip this app can render -- an OPL
                            // stream, or any chip with a core; a track with
                            // neither gets the label, not a button that plays
                            // nothing.
                            if !track.is_playable() {
                                if let Some(chips) = track.chip_list() {
                                    ui.add_enabled(
                                        false,
                                        egui::Label::new(
                                            egui::RichText::new("\u{25B6}").color(palette.muted),
                                        ),
                                    )
                                    .on_disabled_hover_text(
                                        crate::strings::pack_playback_unsupported(&chips),
                                    );
                                }
                                return;
                            }
                            let previewing = state.preview == Some(index);
                            // U+25A0 stop / U+25B6 play.
                            let (glyph, name) = if previewing {
                                ("\u{25A0}", "Stop preview")
                            } else {
                                ("\u{25B6}", "Preview")
                            };
                            if row_icon(ui, palette, glyph, name).clicked() {
                                actions.push(if previewing {
                                    Action::PackStopPreview
                                } else {
                                    Action::PackTrackPreview(index)
                                });
                            }
                        });
                        match &track.entry {
                            Some(entry) => {
                                row.col(|ui| {
                                    ui.label(
                                        egui::RichText::new(&entry.title)
                                            .monospace()
                                            .color(palette.data_text),
                                    );
                                });
                                row.col(|ui| {
                                    ui.label(
                                        egui::RichText::new(format_track_time(entry.total_samples))
                                            .monospace()
                                            .color(palette.data_text),
                                    );
                                });
                                row.col(|ui| {
                                    let loop_str = entry
                                        .loop_samples
                                        .map_or_else(|| "-".to_owned(), format_track_time);
                                    ui.label(
                                        egui::RichText::new(loop_str)
                                            .monospace()
                                            .color(palette.muted),
                                    );
                                });
                                row.col(|ui| {
                                    // Peak in dBFS once scanned; clipped tracks in
                                    // the meter's "hot" colour, "-" until scanned.
                                    match state.peaks.get(&track.file_name) {
                                        Some(peak) => {
                                            let dbfs = vgms_core::peak_dbfs(peak.max_level);
                                            let text = if dbfs.is_finite() {
                                                format!("{dbfs:.1}")
                                            } else {
                                                "silent".to_owned()
                                            };
                                            let color = if peak.clipped {
                                                palette.meter_high
                                            } else {
                                                palette.data_text
                                            };
                                            ui.label(
                                                egui::RichText::new(text).monospace().color(color),
                                            )
                                            .on_hover_text(if peak.clipped {
                                                crate::strings::PACK_PEAK_TIP_CLIPPED
                                            } else {
                                                crate::strings::PACK_PEAK_TIP
                                            });
                                        }
                                        None => {
                                            ui.label(
                                                egui::RichText::new("-")
                                                    .monospace()
                                                    .color(palette.muted),
                                            );
                                        }
                                    }
                                });
                                row.col(|ui| {
                                    row_menu(ui, index, track, palette, actions);
                                });
                            }
                            None => {
                                row.col(|ui| {
                                    ui.colored_label(palette.muted, "unreadable")
                                        .on_hover_text(track.error().unwrap_or_default());
                                });
                                // Total, Loop, Peak, menu -- empty for a track
                                // that did not parse.
                                row.col(|_ui| {});
                                row.col(|_ui| {});
                                row.col(|_ui| {});
                                row.col(|_ui| {});
                            }
                        }

                        let response = row.response();
                        if response.clicked() {
                            actions.push(Action::PackFocusTrack(index));
                        }
                        if response.double_clicked() {
                            actions.push(Action::PackTrackOpen(index));
                        }
                        // A row moved by the keyboard must not walk off the top
                        // or bottom of the view.
                        if scroll_to == Some(index) {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                    });
                }
            });
        drop_target(ui, &row_rects, palette, actions);
    });
}

/// The width of the drag-handle column. Wide enough that the grip is a target
/// rather than a decoration, narrow enough to read as part of the number.
const GRIP_WIDTH: f32 = 16.0;

/// A frameless glyph that behaves as a button: the row's controls are ink in the
/// data well, not keycaps, so five of them per row cost 100pt instead of 345.
///
/// The glyph would be the whole accessible name, so `name` is set explicitly --
/// a screen reader (and `get_by_label`) needs the verb, not the character.
fn row_icon(ui: &mut egui::Ui, palette: &Palette, glyph: &str, name: &str) -> egui::Response {
    let response = ui
        .add(
            egui::Button::new(
                egui::RichText::new(glyph)
                    .monospace()
                    .color(palette.data_label),
            )
            .frame(false),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(name);
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
    response
}

/// The row's drag handle. Dragging a row is how a pack is reordered: the payload
/// is the track's current position, and [`drop_target`] turns where it was let go
/// into the slot it lands in.
///
/// Keyed by file name rather than index so the drag id survives the renumbering
/// that a completed move triggers.
fn drag_grip(ui: &mut egui::Ui, index: usize, track: &PackTrack, palette: &Palette) {
    let id = egui::Id::new(("pack-track-grip", &track.file_name));
    ui.dnd_drag_source(id, index, |ui| {
        // U+2195, CP437 0x12: the bundled VGA font has no braille, so the
        // conventional dotted grip would draw as a tofu box. The up-down arrow
        // is in the ROM font and says which way the row moves anyway.
        ui.label(
            egui::RichText::new("\u{2195}")
                .monospace()
                .color(palette.muted),
        );
    })
    .response
    .on_hover_text(crate::strings::PACK_DRAG_TIP);
}

/// The per-row menu: the two commands that open a window, which are the only
/// ones with no glyph of their own.
fn row_menu(
    ui: &mut egui::Ui,
    index: usize,
    track: &PackTrack,
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    let button = egui::Button::new(
        egui::RichText::new("\u{22EF}") // midline horizontal ellipsis
            .monospace()
            .color(palette.data_label),
    )
    .frame(false);
    let editable = track.is_editable();
    let response = egui::containers::menu::MenuButton::from_button(button)
        .ui(ui, |ui| {
            let open = ui.add_enabled(editable, egui::Button::new("Open in editor"));
            if !editable {
                open.on_disabled_hover_text(crate::strings::PACK_OPEN_DISABLED_TIP);
            } else if open.clicked() {
                actions.push(Action::PackTrackOpen(index));
                ui.close();
            }
            if ui.button("Quick edit\u{2026}").clicked() {
                actions.push(Action::OpenTrackQuickEdit(index));
                ui.close();
            }
        })
        .0
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let enabled = ui.is_enabled();
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "Track menu"));
}

/// Turns a drag in progress into a drop: paints the line where the row would
/// land, and on release emits the move. `row_rects` is one rect per row, in list
/// order, collected as the table drew.
///
/// The insertion slot is a boundary (0 = above the first row, `len` = below the
/// last), so it is converted to a destination *index* -- one less when the track
/// is moving down, since removing it first shifts everything below up.
fn drop_target(
    ui: &mut egui::Ui,
    row_rects: &[egui::Rect],
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    let Some(from) = egui::DragAndDrop::payload::<usize>(ui.ctx()).map(|from| *from) else {
        return;
    };
    // The slot is remembered rather than recomputed on release: letting go also
    // takes the pointer off the screen (a touch release, and what kittest sends),
    // and a drop must not be lost because the cursor's position went with it.
    let slot_id = egui::Id::new("pack-track-drop-slot");
    if let Some(pointer) = ui.ctx().pointer_interact_pos() {
        let slot = row_rects
            .iter()
            .position(|rect| pointer.y < rect.center().y)
            .unwrap_or(row_rects.len());
        ui.ctx()
            .data_mut(|data| data.insert_temp(slot_id, slot.min(row_rects.len())));
        // The boundary that slot sits on: the top of the row it would push down,
        // or the foot of the table when it is going last.
        let y = match row_rects.get(slot) {
            Some(rect) => rect.top(),
            None => row_rects.last().map_or(pointer.y, egui::Rect::bottom),
        };
        if !row_rects.is_empty() {
            ui.painter().hline(
                ui.max_rect().x_range(),
                y,
                egui::Stroke::new(2.0, palette.data_text),
            );
        }
    }
    if ui.input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::take_payload::<usize>(ui.ctx());
        let slot: Option<usize> = ui.ctx().data_mut(|data| data.remove_temp(slot_id));
        let Some(slot) = slot else {
            return; // released without ever hovering the table
        };
        // A slot is a boundary; as an index it is one less when the track is
        // moving down, since taking it out first shifts everything below it up.
        let to = if slot > from { slot - 1 } else { slot };
        if to != from {
            actions.push(Action::PackMoveTrackTo { from, to });
        }
    }
}

/// The widest a screenshot preview is drawn, leaving the facts pane its room.
const PREVIEW_MAX_WIDTH: f32 = 360.0;

/// Draws the Screenshots section: each image beside the facts that decide
/// whether it is the right picture -- dimensions above all.
fn screenshots(ui: &mut egui::Ui, state: &PackState, palette: &Palette, actions: &mut Vec<Action>) {
    ui.add_space(2.0);
    if state.images.is_empty() {
        no_screenshot(ui, state, palette, actions);
        return;
    }
    for (index, image) in state.images.iter().enumerate() {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            preview_well(ui, palette, image);
            ui.vertical(|ui| image_facts(ui, palette, image, index, actions));
        });
        ui.add_space(10.0);
    }
    // A pack may want more than one title screen -- a region, a graphics mode --
    // so Add stays offered once the folder has one, not just while it is empty.
    add_screenshot_button(ui, state, palette, actions);
}

/// The "Add Screenshot..." pad, naming the file it will write. A screenshot is
/// copied in *and* renamed to the pack's convention, which should not be a
/// surprise -- so both the empty state and the foot of the list say where it
/// lands before it is picked.
fn add_screenshot_button(
    ui: &mut egui::Ui,
    state: &PackState,
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    let hover = match state.next_screenshot_name() {
        Some(name) => crate::strings::pack_add_screenshot_named(&name),
        None => crate::strings::PACK_ADD_SCREENSHOT_TIP.to_owned(),
    };
    if bevel::button(ui, palette, "Add Screenshot\u{2026}")
        .on_hover_text(hover)
        .clicked()
    {
        actions.push(Action::PackAddScreenshot);
    }
}

/// The image in a sunken data well, keylined so a dark screenshot still has a
/// visible edge against the well behind it.
fn preview_well(ui: &mut egui::Ui, palette: &Palette, image: &PackImage) {
    let frame = egui::Frame::new()
        .fill(palette.data_bg)
        .inner_margin(egui::Margin::same(6));
    let framed = frame.show(ui, |ui| {
        // The URI carries the byte length, so a freshly recompressed file busts
        // the texture cache rather than showing the stale image.
        let uri = format!("bytes://pack/{}/{}", image.bytes.len(), image.name);
        ui.add(
            egui::Image::from_bytes(uri, image.bytes.clone())
                .fit_to_original_size(1.0)
                .max_width(PREVIEW_MAX_WIDTH),
        )
    });
    bevel::paint_bevel(
        ui.painter(),
        framed.response.rect,
        palette,
        bevel::Bevel::Sunken,
    );
    // A hairline in the display ink around the image itself: the well is dark,
    // and so are most title screens.
    ui.painter().rect_stroke(
        framed.inner.rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0, palette.data_label.gamma_multiply(0.45)),
        egui::StrokeKind::Outside,
    );
}

/// The record beside the preview: what the file is, and what to do about it.
fn image_facts(
    ui: &mut egui::Ui,
    palette: &Palette,
    image: &PackImage,
    index: usize,
    actions: &mut Vec<Action>,
) {
    ui.label(
        egui::RichText::new(&image.name)
            .monospace()
            .color(palette.data_text)
            .strong(),
    );
    ui.add_space(6.0);
    egui::Grid::new(("screenshot-facts", index))
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            let mut fact = |key: &str, value: String| {
                ui.colored_label(palette.data_label, key);
                ui.label(
                    egui::RichText::new(value)
                        .monospace()
                        .color(palette.data_text),
                );
                ui.end_row();
            };
            if let Some(info) = image.info {
                let (wide, high) = info.aspect();
                let aspect = match info.display_mode() {
                    Some(mode) => format!("{wide}:{high}  ({mode})"),
                    None => format!("{wide}:{high}"),
                };
                fact(
                    "Dimensions",
                    format!("{} \u{00D7} {}", info.width, info.height),
                );
                fact("Aspect", aspect);
                fact("Colour", info.colour());
            }
            fact(
                "File size",
                format!("{} bytes", format_byte_count(image.bytes.len())),
            );
        });

    if image.info.is_none() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.colored_label(palette.meter_mid, "!");
            ui.colored_label(palette.muted, crate::strings::PACK_PNG_UNREADABLE);
        });
    } else if image.info.is_some_and(|info| info.display_mode().is_none()) {
        // Not a rule -- VGMRips sets no resolution requirement -- but an
        // unfamiliar size is usually a rescaled capture rather than a real one.
        ui.add_space(6.0);
        ui.colored_label(palette.muted, crate::strings::PACK_PNG_NONSTANDARD);
    }

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        // First, because it acts on the name written above it.
        if bevel::button(ui, palette, "Rename\u{2026}")
            .on_hover_text(crate::strings::PACK_RENAME_SCREENSHOT_TIP)
            .clicked()
        {
            actions.push(Action::PackRenameScreenshotAt(index));
        }
        // "Recompress", not "Optimize": the deck's Optimize pad is the VGM
        // pipeline's vgm_cmp step, and two different jobs must not share one
        // word on the same screen.
        if bevel::button(ui, palette, "Recompress")
            .on_hover_text(crate::strings::PACK_RECOMPRESS_TIP)
            .clicked()
        {
            actions.push(Action::OptimizeImage(index));
        }
        if bevel::button(ui, palette, "Replace\u{2026}")
            .on_hover_text(crate::strings::PACK_REPLACE_SCREENSHOT_TIP)
            .clicked()
        {
            actions.push(Action::PackReplaceScreenshot(index));
        }
        if bevel::button(ui, palette, "Delete")
            .on_hover_text(crate::strings::PACK_DELETE_SCREENSHOT_TIP)
            .clicked()
        {
            actions.push(Action::PackDeleteScreenshot(index));
        }
    });
}

/// The empty state. A screenshot is required for a submission -- its absence is
/// already a checklist warning -- so this says what is wanted and offers the fix
/// rather than just reporting that nothing is there.
fn no_screenshot(
    ui: &mut egui::Ui,
    state: &PackState,
    palette: &Palette,
    actions: &mut Vec<Action>,
) {
    ui.add_space(8.0);
    let frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 18));
    let framed = frame.show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(crate::strings::PACK_NO_SCREENSHOT_TITLE)
                    .color(palette.data_label)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.colored_label(palette.muted, crate::strings::PACK_NO_SCREENSHOT_BODY);
            ui.add_space(12.0);
            add_screenshot_button(ui, state, palette, actions);
        });
    });
    // Dashed, not solid: the border marks a slot waiting to be filled rather
    // than framing something that is there.
    let rect = framed.response.rect;
    let stroke = egui::Stroke::new(1.0, palette.data_label.gamma_multiply(0.45));
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    ui.painter()
        .extend(egui::Shape::dashed_line(&corners, stroke, 6.0, 4.0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgms_core::ChipKind;
    use vgms_core::vgm::data::Gd3Tag;

    const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");
    const PNG_FIXTURE: &[u8] = include_bytes!("../../../tests/screenshot.png");

    /// A VGM fixture re-serialised with a given file name and GD3 tag, wrapped as
    /// a picked file -- the same trick the editor's tests use.
    fn tagged_song(name: &str, tag: Gd3Tag) -> PickedFile {
        let mut song = vgms_core::io::read_song(name, VGM_FIXTURE).unwrap();
        if let Some(meta) = song.vgm_meta_mut() {
            meta.tag = Some(tag);
        }
        PickedFile {
            name: name.to_owned(),
            path: Some(PathBuf::from(format!("C:/pack/{name}"))),
            bytes: vgms_core::io::write_song(&song).unwrap(),
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
        let mut bytes = vgms_core::io::write_song(
            &vgms_core::io::read_song("01 Tune.vgm", VGM_FIXTURE).unwrap(),
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
        vgms_core::io::read_song("x.vgm", bytes)
            .unwrap()
            .vgm_meta()
            .unwrap()
            .volume_modifier
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
        let other_song =
            vgms_core::convert::dro_to_vgm(&crate::test_song::tone_song()).expect("converts");
        let audio_edit = vgms_core::io::write_song(&other_song).expect("writes");
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
    fn retagged_bytes_applies_the_tag_and_follows_the_new_extension() {
        let song = vgms_core::io::read_song("01 Old.vgm", VGM_FIXTURE).unwrap();
        let new_tag = Gd3Tag {
            track_name_en: "Renamed Track".to_owned(),
            ..Gd3Tag::default()
        };

        // Same extension: uncompressed VGM bytes carrying the new tag.
        let vgm = retagged_bytes(&song, "01 New.vgm", new_tag.clone()).unwrap();
        assert!(
            !vgms_core::vgm::io::is_gzipped(&vgm),
            "a .vgm stays uncompressed"
        );
        let reparsed = vgms_core::io::read_song("01 New.vgm", &vgm).unwrap();
        assert_eq!(
            reparsed
                .vgm_meta()
                .unwrap()
                .tag
                .as_ref()
                .unwrap()
                .track_name_en,
            "Renamed Track"
        );

        // A .vgz name gzips the same bytes.
        let vgz = retagged_bytes(&song, "01 New.vgz", new_tag).unwrap();
        assert!(vgms_core::vgm::io::is_gzipped(&vgz), "a .vgz is gzipped");
        assert_eq!(
            vgms_core::io::read_song("01 New.vgz", &vgz).unwrap().name,
            "01 New.vgz"
        );
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
            let song = vgms_core::io::read_song("x.vgm", bytes).unwrap();
            assert_eq!(
                song.vgm_meta().unwrap().tag.as_ref().unwrap().release_date,
                "1994-03-01"
            );
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
        assert!(request.optimize_vgms, "optimise-on-export defaults on");
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

    #[test]
    fn overlay_writes_only_the_checked_fields() {
        let base = tag("Old Game", "Ada", "Old Ripper");
        let mut overlay = BulkTagOverlay::default();
        // Check game name and creator; leave author and everything else alone.
        overlay.apply[gd3_index::GAME_NAME_EN] = true;
        overlay.values[gd3_index::GAME_NAME_EN] = "New Game".to_owned();
        overlay.apply[gd3_index::CREATOR] = true;
        overlay.values[gd3_index::CREATOR] = "New Ripper".to_owned();
        // A value present but unchecked must not be written.
        overlay.values[gd3_index::TRACK_AUTHOR_EN] = "Zoe".to_owned();

        let merged = overlay.apply_to(&base);
        assert_eq!(merged.game_name_en, "New Game", "checked field written");
        assert_eq!(merged.creator, "New Ripper", "checked field written");
        assert_eq!(merged.track_author_en, "Ada", "unchecked field kept");
        assert_eq!(merged.release_date, "1994", "untouched field kept");
    }

    #[test]
    fn overlay_can_clear_a_field_by_checking_an_empty_value() {
        let base = tag("Game", "Ada", "Ripper");
        let mut overlay = BulkTagOverlay::default();
        overlay.apply[gd3_index::TRACK_AUTHOR_EN] = true; // empty value
        assert_eq!(overlay.apply_to(&base).track_author_en, "");
    }

    #[test]
    fn writes_anything_reflects_the_checkboxes() {
        let mut overlay = BulkTagOverlay::default();
        assert!(!overlay.writes_anything());
        overlay.apply[gd3_index::SYSTEM_NAME_EN] = true;
        assert!(overlay.writes_anything());
    }

    #[test]
    fn seed_prechecks_every_shared_field_including_the_composer() {
        let meta = PackMeta {
            game_name: "Cool Game".to_owned(),
            system: "IBM PC/AT".to_owned(),
            release_date: "1994-03-01".to_owned(),
            creator: "Ripper".to_owned(),
            music_authors: "Ada, Bob".to_owned(),
            ..PackMeta::default()
        };
        let overlay = seed_from_meta(&meta);

        // Every shared pack field -- composer and the orig variants included --
        // is pre-filled and pre-checked, so "apply to all" needs no extra clicks.
        for index in [
            gd3_index::GAME_NAME_EN,
            gd3_index::GAME_NAME_NATIVE,
            gd3_index::SYSTEM_NAME_EN,
            gd3_index::SYSTEM_NAME_NATIVE,
            gd3_index::TRACK_AUTHOR_EN,
            gd3_index::TRACK_AUTHOR_NATIVE,
            gd3_index::RELEASE_DATE,
            gd3_index::CREATOR,
        ] {
            assert!(overlay.apply[index], "field {index} pre-checked");
        }
        // The orig variants mirror their English siblings' pack values.
        assert_eq!(overlay.values[gd3_index::TRACK_AUTHOR_EN], "Ada, Bob");
        assert_eq!(overlay.values[gd3_index::TRACK_AUTHOR_NATIVE], "Ada, Bob");
        assert_eq!(overlay.values[gd3_index::GAME_NAME_NATIVE], "Cool Game");
        // Neither track-name field is ever seeded (EN index 0, orig index 1).
        for index in [0, 1] {
            assert!(overlay.values[index].is_empty() && !overlay.apply[index]);
        }
    }

    #[test]
    fn seed_leaves_empty_pack_fields_unchecked() {
        let overlay = seed_from_meta(&PackMeta::default());
        assert!(
            !overlay.writes_anything(),
            "a blank pack pre-checks nothing"
        );
    }
}
