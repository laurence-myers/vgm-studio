//! Rip mode: preparing a VGMRips submission from a folder of songs.
//!
//! [`RipState`] is the headless core -- the loaded folder, the editable package
//! metadata, and the derived track list -- with no egui, so it is testable
//! without a window (like [`crate::editor::Editor`]). [`show`] draws the view.
//!
//! The description file *is* the project: opening a folder re-parses any
//! `Game Name.txt` back into the form, so a pack can be reopened and updated.
//! When there is no description (a fresh rip), the fields are prefilled from the
//! songs' GD3 tags.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use dro_synth::Peak;

use dro_core::rip::{
    DEFAULT_OS, DEFAULT_SYSTEM, PRESETS, RipMeta, Severity, TrackEntry, TrackFacts, doc_file_stem,
    format_track_time, generate_description, generate_m3u, music_hardware_suggestion,
    parse_description, readiness,
};
use dro_core::vgm::data::GD3_FIELD_COUNT;
use dro_core::{Gd3Tag, OplType, Song};
use egui_extras::{Column, TableBuilder};

use crate::action::Action;
use crate::platform::{PickedFile, PickedFolder, RipEntry, RipEntryKind, RipJobRequest};
use crate::theme::{Palette, bevel};

/// One song file in the rip: its bytes (kept for export and opening in the
/// editor) and the parse result (an error shows inline rather than aborting).
#[derive(Debug, Clone)]
pub struct RipTrack {
    pub file_name: String,
    pub path: Option<PathBuf>,
    pub bytes: Vec<u8>,
    pub song: Result<Arc<Song>, String>,
    /// The table entry (title, durations) computed once at scan, rather than
    /// re-summing the whole song per row per frame. `Some` iff the song parsed.
    pub entry: Option<TrackEntry>,
}

impl RipTrack {
    /// The parsed song, if it loaded.
    #[must_use]
    pub fn song(&self) -> Option<&Arc<Song>> {
        self.song.as_ref().ok()
    }
}

/// A screenshot in the rip folder. Its bytes are shared (`Arc<[u8]>`) so the
/// inline preview's per-frame `Image::from_bytes` clone is an Arc bump, not a
/// full copy of the PNG (uiwidget-9).
#[derive(Debug, Clone)]
pub struct RipImage {
    pub name: String,
    pub path: Option<PathBuf>,
    pub bytes: Arc<[u8]>,
}

/// The whole rip project: what a folder scan produced, plus the editable
/// package metadata.
#[derive(Debug)]
pub struct RipState {
    pub folder_name: String,
    pub folder_path: Option<PathBuf>,
    pub meta: RipMeta,
    pub tracks: Vec<RipTrack>,
    pub images: Vec<RipImage>,
    /// The description file that was parsed, if any.
    pub description_file: Option<String>,
    /// Set when an existing description could not be parsed; saving overwrites it.
    pub parse_warning: Option<String>,
    /// Unsaved edits to the package metadata.
    pub dirty: bool,
    /// Gzip `.vgm` songs to `.vgz` on export (the VGMRips convention).
    pub gzip_on_export: bool,
    /// Strip redundant OPL writes from each VGM on export (`vgm_cmp`, the final
    /// step of the VGMRips optimisation pipeline).
    pub optimize_on_export: bool,
    /// The row currently previewing through the audio output (rip mode playback).
    pub preview: Option<usize>,
    /// Measured peak per track, keyed by `file_name` (the stable identity that
    /// survives a rescan/reorder). Filled by "Scan Volumes"; drives the Peak
    /// column and the suggested modifiers.
    pub peaks: HashMap<String, Peak>,
    /// Whether "Apply suggested modifiers" levels the whole pack by its loudest
    /// track (album mode, the VGMRips convention) rather than normalising each
    /// track to its own peak.
    pub album_normalize: bool,
}

impl RipState {
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
                    let song = dro_core::io::read_song(&file.name, &file.bytes)
                        .map(Arc::new)
                        .map_err(|error| error.to_string());
                    let entry = song
                        .as_ref()
                        .ok()
                        .map(|song| TrackEntry::from_song(song, &file.name));
                    tracks.push(RipTrack {
                        file_name: file.name,
                        path: file.path,
                        bytes: file.bytes,
                        song,
                        entry,
                    });
                }
                FileClass::Image => images.push(RipImage {
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
        // rewrites (a new volume modifier, GD3 tags) keep it; a track that was
        // edited in the editor and saved back, renamed away, or replaced
        // wholesale loses it and shows unscanned again.
        let song_of = |tracks: &[RipTrack], name: &str| {
            tracks
                .iter()
                .find(|track| track.file_name == name)
                .and_then(RipTrack::song)
                .cloned()
        };
        self.peaks.retain(|name, _| {
            match (song_of(&old_tracks, name), song_of(&self.tracks, name)) {
                (Some(old), Some(new)) => old.data() == new.data() && old.opl_type == new.opl_type,
                _ => false,
            }
        });
    }

    /// The transaction that moves the track at `from` to `to`, renumbering the
    /// affected files' names. `None` when nothing would change or the folder has
    /// no path (the web has none, and rip mode is native-only anyway).
    #[must_use]
    pub fn reorder_transaction(&self, from: usize, to: usize) -> Option<RipTransaction> {
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
        Some(RipTransaction {
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
    pub fn suggested_modifier_transaction(&self) -> Option<RipTransaction> {
        let album_peak = self.peaks.values().map(|peak| peak.max_level).max()?;
        let album = self.album_normalize.then_some(album_peak);
        let mut forward = Vec::new();
        let mut inverse = Vec::new();
        for track in &self.tracks {
            let (Some(song), Some(path), Some(peak)) = (
                track.song(),
                track.path.clone(),
                self.peaks.get(&track.file_name),
            ) else {
                continue;
            };
            // Only VGMs carry a volume modifier; the rip list is VGM/VGZ only, but
            // guard so a non-VGM can never be rewritten.
            let Some(current) = song.vgm_meta().map(|meta| meta.volume_modifier) else {
                continue;
            };
            let new_modifier = dro_core::suggest_volume_modifier(peak.max_level, album);
            if new_modifier == current {
                continue; // nothing changed for this track
            }
            if let Some(Ok(bytes)) = revolumed_bytes(song, new_modifier) {
                forward.push(RipMutation::Write {
                    path: path.clone(),
                    bytes,
                });
                inverse.push(RipMutation::Write {
                    path,
                    bytes: track.bytes.clone(),
                });
            }
        }
        if forward.is_empty() {
            return None;
        }
        let count = forward.len();
        Some(RipTransaction {
            label: format!(
                "Set volume modifier on {count} track{}",
                if count == 1 { "" } else { "s" }
            ),
            forward,
            inverse,
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

    /// Whether the metadata is ready to save (a game name is required, since it
    /// names every output file).
    #[must_use]
    pub fn can_save(&self) -> bool {
        !self.meta.game_name.trim().is_empty()
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
                tag: track
                    .song()
                    .and_then(|song| song.vgm_meta())
                    .and_then(|meta| meta.tag.clone()),
                loops: track
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.loop_samples.is_some()),
                readable: track.song().is_some(),
            })
            .collect()
    }

    /// The export gate: blocking errors, soft warnings, and optional notes.
    ///
    /// The file-level shape checks (game name, readable songs, `NN Title`
    /// numbering, screenshot) live here; the GD3-tag / metadata content checks
    /// come from the shared, wasm-clean [`dro_core::rip::readiness`], merged in by
    /// severity. Notes are surfaced in the checklist but never gate an export.
    #[must_use]
    pub fn validations(&self) -> RipValidations {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut notes = Vec::new();

        if self.meta.game_name.trim().is_empty() {
            errors.push("Enter a game name (it names every file in the pack).".to_owned());
        }
        let readable = self
            .tracks
            .iter()
            .filter(|track| track.song().is_some())
            .count();
        if readable == 0 {
            errors.push("There are no readable songs to export.".to_owned());
        }

        if self.tracks.iter().any(|track| track.song().is_none()) {
            warnings.push(
                "Some files could not be read; they ship as-is, without a track-list entry."
                    .to_owned(),
            );
        }
        if self.images.is_empty() {
            warnings.push("There is no screenshot (.png) in the folder.".to_owned());
        }

        let numbers: Vec<u32> = self
            .tracks
            .iter()
            .filter_map(|track| track_number(&track.file_name))
            .collect();
        if numbers.len() != self.tracks.len() {
            warnings.push("Some files are not named \"NN Title.ext\".".to_owned());
        }
        let mut unique = numbers.clone();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() != numbers.len() {
            warnings.push("Some track numbers are duplicated.".to_owned());
        } else if !numbers.is_empty() && unique != (1..=numbers.len() as u32).collect::<Vec<_>>() {
            warnings.push("Track numbers are not a contiguous 01, 02, 03... sequence.".to_owned());
        }

        // The GD3 / metadata content checks, merged in by severity.
        for item in readiness(&self.meta, &self.track_facts()) {
            match item.severity {
                Severity::Error => errors.push(item.message),
                Severity::Warning => warnings.push(item.message),
                Severity::Note => notes.push(item.message),
            }
        }

        RipValidations {
            errors,
            warnings,
            notes,
        }
    }

    /// Builds the export job: every song, every screenshot, and freshly
    /// generated docs whose names reflect the final (post-gzip) song names.
    #[must_use]
    pub fn export_request(&self) -> RipJobRequest {
        let stem = self.doc_stem();
        let mut entries: Vec<RipEntry> = Vec::new();
        for track in &self.tracks {
            entries.push(RipEntry {
                name: track.file_name.clone(),
                bytes: track.bytes.clone(),
                kind: RipEntryKind::Song,
            });
        }
        for image in &self.images {
            entries.push(RipEntry {
                name: image.name.clone(),
                bytes: image.bytes.to_vec(),
                kind: RipEntryKind::Image,
            });
        }
        entries.push(RipEntry {
            name: format!("{stem}.txt"),
            bytes: self.description_text().into_bytes(),
            kind: RipEntryKind::Doc,
        });
        entries.push(RipEntry {
            name: format!("{stem}.m3u"),
            bytes: self.m3u_text(true).into_bytes(),
            kind: RipEntryKind::Doc,
        });
        RipJobRequest {
            zip_name: format!("{stem}.zip"),
            entries,
            gzip_vgms: self.gzip_on_export,
            optimize_vgms: self.optimize_on_export,
        }
    }
}

/// The result of [`RipState::validations`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RipValidations {
    /// Problems that block an export.
    pub errors: Vec<String>,
    /// Advisories the user may choose to ignore (the "export anyway?" confirm).
    pub warnings: Vec<String>,
    /// Optional observations (the note tier): surfaced in the submission
    /// checklist, but never shown in the export dialog and never gating.
    pub notes: Vec<String>,
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

/// One reversible file operation on the rip folder, the unit the app's file-op
/// executor runs. `Rename`'s `to` is a bare name in the same directory; `Write`
/// overwrites `path` outright.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RipMutation {
    Rename { from: PathBuf, to: String },
    Write { path: PathBuf, bytes: Vec<u8> },
}

/// A user edit as an ordered list of file mutations, with the exact `inverse`
/// that undoes it. Applying `forward` then `inverse` returns the folder to its
/// starting state; the app replays whichever list an undo or redo needs.
#[derive(Debug, Clone)]
pub struct RipTransaction {
    pub label: String,
    pub forward: Vec<RipMutation>,
    pub inverse: Vec<RipMutation>,
}

/// The mutations that apply a set of `(src, dst)` renames without a transient
/// collision: rename every source to a unique temp name first, then each temp to
/// its destination. Safe for any permutation (including cycles and swaps).
fn rename_batch_mutations(
    folder: &std::path::Path,
    pairs: &[(String, String)],
) -> Vec<RipMutation> {
    let temp = |i: usize| format!(".drotrim-reorder-{i}");
    let mut muts = Vec::with_capacity(pairs.len() * 2);
    for (i, (src, _)) in pairs.iter().enumerate() {
        muts.push(RipMutation::Rename {
            from: folder.join(src),
            to: temp(i),
        });
    }
    for (i, (_, dst)) in pairs.iter().enumerate() {
        muts.push(RipMutation::Rename {
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

/// Default metadata for a fresh rip, seeded from the songs' GD3 tags.
fn prefilled(tracks: &[RipTrack], today: Option<(i32, u8, u8)>) -> RipMeta {
    let songs: Vec<&Arc<Song>> = tracks.iter().filter_map(RipTrack::song).collect();

    let mut meta = RipMeta {
        system: DEFAULT_SYSTEM.to_owned(),
        os: DEFAULT_OS.to_owned(),
        version: "1.00".to_owned(),
        ..RipMeta::default()
    };
    if let Some(opl) = highest_opl(&songs) {
        meta.music_hardware = music_hardware_suggestion(opl).to_owned();
    }
    for song in &songs {
        if let Some(tag) = song.vgm_meta().and_then(|meta| meta.tag.as_ref()) {
            fill_if_empty(&mut meta.game_name, &tag.game_name_en);
            fill_if_empty(&mut meta.creator, &tag.creator);
            fill_if_empty(&mut meta.release_date, &tag.release_date);
        }
    }
    meta.music_authors = unique_authors(&songs);

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

/// The most capable chip across the songs (OPL3 > dual OPL2 > OPL2).
fn highest_opl(songs: &[&Arc<Song>]) -> Option<OplType> {
    songs
        .iter()
        .map(|song| song.opl_type)
        .max_by_key(|opl| match opl {
            OplType::Opl2 => 0,
            OplType::DualOpl2 => 1,
            OplType::Opl3 => 2,
        })
}

/// Distinct GD3 track authors, in track order, comma-joined.
fn unique_authors(songs: &[&Arc<Song>]) -> String {
    let mut authors: Vec<String> = Vec::new();
    for song in songs {
        if let Some(tag) = song.vgm_meta().and_then(|meta| meta.tag.as_ref()) {
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

/// Draws the rip view: the package-metadata form and the track list.
pub fn show(ui: &mut egui::Ui, state: &mut RipState, palette: &Palette, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);

    // Header strip: folder name, save, and the gzip-on-export toggle.
    ui.horizontal(|ui| {
        ui.visuals_mut().override_text_color = Some(palette.data_label);
        ui.label(egui::RichText::new(&state.folder_name).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if bevel::button(ui, palette, "Export Zip\u{2026}")
                .on_hover_text(
                    "Build the submission zip (songs, screenshot, description, playlist)",
                )
                .clicked()
            {
                actions.push(Action::RipExportZip);
            }
            if bevel::button(ui, palette, "Save Package Files")
                .on_hover_text("Write Game Name.txt and Game Name.m3u into the folder")
                .clicked()
            {
                actions.push(Action::RipSaveDocs);
            }
            if bevel::button(ui, palette, "Bulk Tag\u{2026}")
                .on_hover_text("Write shared GD3 fields (game, system, composer\u{2026}) to many tracks at once")
                .clicked()
            {
                actions.push(Action::OpenBulkTag);
            }
            if bevel::button(ui, palette, "Scan Volumes")
                .on_hover_text(
                    "Measure every track's peak level (dBFS) for the Peak column and the \
                     suggested volume modifiers",
                )
                .clicked()
            {
                actions.push(Action::RipScanVolumes);
            }
            if bevel::button(ui, palette, "Apply Modifiers")
                .on_hover_text(
                    "Set each track's VGM volume modifier from the scanned peaks to level the \
                     pack (one undoable edit)",
                )
                .clicked()
            {
                actions.push(Action::RipApplySuggestedModifiers);
            }
            ui.checkbox(&mut state.album_normalize, "Album")
                .on_hover_text(
                    "Level the whole pack by its loudest track (album mode); off normalises each \
                     track to its own peak",
                );
            ui.checkbox(&mut state.gzip_on_export, "Gzip to .vgz on export");
            ui.checkbox(&mut state.optimize_on_export, "Optimize VGMs on export")
                .on_hover_text(
                    "Strip redundant OPL register writes from each VGM before packing (vgm_cmp)",
                );
        });
    });
    if let Some(warning) = &state.parse_warning {
        ui.colored_label(palette.muted, warning);
    }
    crate::theme::separator_full(ui, palette);

    let scroll_out = egui::ScrollArea::vertical().show(ui, |ui| {
        let mut dirty = false;
        egui::Grid::new("rip-meta")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                dirty |= field(ui, palette, "Game name:", &mut state.meta.game_name);
                dirty |= field(ui, palette, "System:", &mut state.meta.system);
                dirty |= field(ui, palette, "OS:", &mut state.meta.os);
                dirty |= field(
                    ui,
                    palette,
                    "Music hardware:",
                    &mut state.meta.music_hardware,
                );
                // One-click chip presets for the three fields above.
                ui.label("Presets:");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for preset in &PRESETS {
                        if bevel::button(ui, palette, preset.name)
                            .on_hover_text(format!(
                                "{} / {} / {}",
                                preset.system, preset.os, preset.music_hardware
                            ))
                            .clicked()
                        {
                            state.meta.system = preset.system.to_owned();
                            state.meta.os = preset.os.to_owned();
                            state.meta.music_hardware = preset.music_hardware.to_owned();
                            dirty = true;
                        }
                    }
                });
                ui.end_row();
                dirty |= field(ui, palette, "Music author:", &mut state.meta.music_authors);
                dirty |= field(ui, palette, "Game developer:", &mut state.meta.developer);
                dirty |= field(ui, palette, "Game publisher:", &mut state.meta.publisher);
                dirty |= field(
                    ui,
                    palette,
                    "Game release date:",
                    &mut state.meta.release_date,
                );
                dirty |= field(ui, palette, "Package created by:", &mut state.meta.creator);
                dirty |= field(ui, palette, "Package version:", &mut state.meta.version);
            });

        ui.add_space(4.0);
        dirty |= multiline(ui, palette, "Notes:", &mut state.meta.notes);
        dirty |= multiline(ui, palette, "Package history:", &mut state.meta.history);
        if dirty {
            state.dirty = true;
        }

        // A clipped divider (not separator_full): inside the scroll area it must
        // not bleed over the status bar below or past the scrollbar beside it.
        crate::theme::separator_clipped(ui, palette);
        track_table(ui, state, palette, actions);
        screenshots(ui, state, palette, actions);
    });

    // When the page overflows, frame the vertical scrollbar's channel with the
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

/// Re-serialises `song` under `new_name` with `tag` applied. The name drives the
/// output format, so a `.vgm` -> `.vgz` rename gzips the result. Used by the
/// quick-edit dialog to rewrite a track without loading it into the editor.
pub fn retagged_bytes(song: &Song, new_name: &str, tag: Gd3Tag) -> Result<Vec<u8>, String> {
    let mut song = song.clone();
    song.name = new_name.to_owned();
    if let Some(meta) = song.vgm_meta_mut() {
        meta.tag = Some(tag);
    }
    dro_core::io::write_song(&song).map_err(|error| error.to_string())
}

/// Re-serialises `song` with its VGM volume modifier set to `modifier`, keeping
/// the file name (and thus format/extension). The clone/set/write precedent for
/// the "Apply suggested modifiers" batch, mirroring [`retagged_bytes`]. `None`
/// if `song` is not a VGM, which has no modifier.
#[must_use]
pub fn revolumed_bytes(song: &Song, modifier: u8) -> Option<Result<Vec<u8>, String>> {
    let mut song = song.clone();
    song.vgm_meta_mut()?.volume_modifier = modifier;
    Some(dro_core::io::write_song(&song).map_err(|error| error.to_string()))
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
pub fn seed_from_meta(meta: &RipMeta) -> BulkTagOverlay {
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

/// A labelled single-line field. Returns whether it changed.
fn field(ui: &mut egui::Ui, palette: &Palette, label: &str, value: &mut String) -> bool {
    ui.label(label);
    let response = ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(340.0)
            .text_color(palette.data_text),
    );
    ui.end_row();
    response.changed()
}

fn multiline(ui: &mut egui::Ui, palette: &Palette, label: &str, value: &mut String) -> bool {
    ui.label(label);
    let response = ui.add(
        egui::TextEdit::multiline(value)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace)
            .text_color(palette.data_text),
    );
    response.changed()
}

fn track_table(ui: &mut egui::Ui, state: &RipState, palette: &Palette, actions: &mut Vec<Action>) {
    ui.label(
        egui::RichText::new("Tracks (double-click to open in the editor)")
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
        TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .vscroll(false)
            .column(Column::auto().at_least(30.0)) // #
            .column(Column::remainder().at_least(180.0)) // Title (GD3)
            .column(Column::auto().at_least(55.0)) // Total
            .column(Column::auto().at_least(55.0)) // Loop
            .column(Column::auto().at_least(60.0)) // Peak (dBFS)
            .column(Column::auto().at_least(200.0)) // actions (reorder + preview + open + tags)
            .header(row_height + 2.0, |mut header| {
                for title in ["#", "Title (GD3)", "Total", "Loop", "Peak", ""] {
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
                        row.col(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{:02}", index + 1))
                                    .monospace()
                                    .color(palette.muted),
                            )
                            .on_hover_text(&track.file_name);
                        });
                        match &track.song {
                            Ok(_) => {
                                let entry = track
                                    .entry
                                    .as_ref()
                                    .expect("a parsed song has a cached entry");
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
                                            let dbfs = dro_core::peak_dbfs(peak.max_level);
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
                                                "Peak reaches full scale (clipping)"
                                            } else {
                                                "Loudest peak, in dBFS"
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
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 3.0;
                                        // Reorder: up/down move the track one slot,
                                        // renumbering the affected files. The move
                                        // is a no-op (ignored) at the list's ends.
                                        if bevel::button(ui, palette, "\u{25B2}")
                                            .on_hover_text("Move up")
                                            .clicked()
                                        {
                                            actions.push(Action::RipMoveTrack { index, delta: -1 });
                                        }
                                        if bevel::button(ui, palette, "\u{25BC}")
                                            .on_hover_text("Move down")
                                            .clicked()
                                        {
                                            actions.push(Action::RipMoveTrack { index, delta: 1 });
                                        }
                                        let previewing = state.preview == Some(index);
                                        // U+25A0 stop / U+25B6 play.
                                        let symbol =
                                            if previewing { "\u{25A0}" } else { "\u{25B6}" };
                                        if bevel::button(ui, palette, symbol)
                                            .on_hover_text("Preview")
                                            .clicked()
                                        {
                                            actions.push(if previewing {
                                                Action::RipStopPreview
                                            } else {
                                                Action::RipTrackPreview(index)
                                            });
                                        }
                                        if bevel::button(ui, palette, "Open")
                                            .on_hover_text("Open the track in the editor")
                                            .clicked()
                                        {
                                            actions.push(Action::RipTrackOpen(index));
                                        }
                                        if bevel::button(ui, palette, "Tags")
                                            .on_hover_text("Edit the track's GD3 tags")
                                            .clicked()
                                        {
                                            actions.push(Action::OpenTrackQuickEdit(index));
                                        }
                                    });
                                });
                            }
                            Err(error) => {
                                row.col(|ui| {
                                    ui.colored_label(palette.muted, "unreadable")
                                        .on_hover_text(error);
                                });
                                // Total, Loop, Peak, actions -- empty for a track
                                // that did not parse.
                                row.col(|_ui| {});
                                row.col(|_ui| {});
                                row.col(|_ui| {});
                                row.col(|_ui| {});
                            }
                        }

                        if row.response().double_clicked() {
                            actions.push(Action::RipTrackOpen(index));
                        }
                    });
                }
            });
    });
}

fn screenshots(ui: &mut egui::Ui, state: &RipState, palette: &Palette, actions: &mut Vec<Action>) {
    // A divider sets the screenshots apart from the track list above (clipped,
    // like the Tracks divider, so it stays inside the scroll well).
    crate::theme::separator_clipped(ui, palette);
    ui.add_space(6.0);
    if state.images.is_empty() {
        ui.colored_label(palette.muted, "No screenshot (.png) in the folder.");
        return;
    }
    ui.label(
        egui::RichText::new("Screenshots")
            .color(palette.data_label)
            .strong(),
    );
    for (index, image) in state.images.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&image.name)
                    .monospace()
                    .color(palette.data_text),
            );
            ui.label(
                egui::RichText::new(format!("({} bytes)", image.bytes.len())).color(palette.muted),
            );
            if bevel::button(ui, palette, "Optimize")
                .on_hover_text("Losslessly recompress with oxipng and save in place")
                .clicked()
            {
                actions.push(Action::OptimizeImage(index));
            }
        });
        // Inline preview at natural size (capped). The URI carries the byte
        // length, so a freshly optimised file busts the texture cache.
        let uri = format!("bytes://rip/{}/{}", image.bytes.len(), image.name);
        ui.add(
            egui::Image::from_bytes(uri, image.bytes.clone())
                .fit_to_original_size(1.0)
                .max_width(480.0),
        );
        ui.add_space(6.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dro_core::vgm::data::Gd3Tag;

    const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

    /// A VGM fixture re-serialised with a given file name and GD3 tag, wrapped as
    /// a picked file -- the same trick the editor's tests use.
    fn tagged_song(name: &str, tag: Gd3Tag) -> PickedFile {
        let mut song = dro_core::io::read_song(name, VGM_FIXTURE).unwrap();
        if let Some(meta) = song.vgm_meta_mut() {
            meta.tag = Some(tag);
        }
        PickedFile {
            name: name.to_owned(),
            path: Some(PathBuf::from(format!("C:/pack/{name}"))),
            bytes: dro_core::io::write_song(&song).unwrap(),
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
    fn prefills_from_gd3_when_there_is_no_description() {
        let files = vec![
            tagged_song("01 Intro.vgz", tag("Cool Game", "Ada", "Ripper")),
            tagged_song("02 Level.vgz", tag("Cool Game", "Bob", "Ripper")),
        ];
        let state = RipState::from_folder(folder("Cool Game", files), Some((2026, 7, 16)));

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
        let files = vec![tagged_song("01 Intro.vgz", tag("G", "A", "Rip"))];
        let state = RipState::from_folder(folder("G", files), None);
        assert_eq!(state.meta.history, "1.00 <date> Rip: Initial release.");
    }

    /// The volume_modifier a set of serialised song bytes decode to.
    fn modifier_of(bytes: &[u8]) -> u8 {
        dro_core::io::read_song("x.vgm", bytes)
            .unwrap()
            .vgm_meta()
            .unwrap()
            .volume_modifier
    }

    #[test]
    fn revolumed_bytes_round_trips_the_modifier() {
        let song = dro_core::io::read_song("t.vgm", VGM_FIXTURE).unwrap();
        let bytes = revolumed_bytes(&song, 0x20)
            .expect("a VGM")
            .expect("writes");
        assert_eq!(modifier_of(&bytes), 0x20);
    }

    #[test]
    fn refresh_keeps_peaks_for_header_edits_and_drops_them_for_audio_edits() {
        let files = vec![
            tagged_song("01 A.vgm", tag("G", "A", "R")),
            tagged_song("02 B.vgm", tag("G", "B", "R")),
        ];
        let mut state = RipState::from_folder(folder("G", files), None);
        state.peaks.insert("01 A.vgm".to_owned(), peak(0x4000));
        state.peaks.insert("02 B.vgm".to_owned(), peak(0x2000));

        // Redeliver the folder with track 1 rewritten header-only (a new volume
        // modifier -- what Apply Modifiers produces) and track 2's audio replaced
        // wholesale (what an editor edit saved back produces).
        let header_edit = revolumed_bytes(state.tracks[0].song().unwrap(), 0x20)
            .expect("a VGM")
            .expect("writes");
        let other_song =
            dro_core::convert::dro_to_vgm(&crate::test_song::tone_song()).expect("converts");
        let audio_edit = dro_core::io::write_song(&other_song).expect("writes");
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
        let mut state = RipState::from_folder(folder("Game", files), None);
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
                RipMutation::Write { bytes, .. } => modifier_of(bytes),
                RipMutation::Rename { .. } => panic!("only writes"),
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
        let mut state = RipState::from_folder(folder("Game", files), None);
        state.album_normalize = false;
        state.peaks.insert("01 Loud.vgm".to_owned(), peak(0x4000)); // half -> 0x20
        state.peaks.insert("02 Quiet.vgm".to_owned(), peak(0x1000)); // eighth -> 0x60

        let txn = state.suggested_modifier_transaction().expect("both change");
        let modifiers: Vec<u8> = txn
            .forward
            .iter()
            .map(|mutation| match mutation {
                RipMutation::Write { bytes, .. } => modifier_of(bytes),
                RipMutation::Rename { .. } => panic!("only writes"),
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
        let state = RipState::from_folder(folder("Existing Pack", files), Some((2026, 7, 16)));
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
        let state = RipState::from_folder(folder("Pack", files), Some((2026, 7, 16)));
        assert!(state.parse_warning.is_some());
        assert_eq!(state.meta.game_name, "GD3 Game", "prefilled from GD3");
    }

    #[test]
    fn retagged_bytes_applies_the_tag_and_follows_the_new_extension() {
        let song = dro_core::io::read_song("01 Old.vgm", VGM_FIXTURE).unwrap();
        let new_tag = Gd3Tag {
            track_name_en: "Renamed Track".to_owned(),
            ..Gd3Tag::default()
        };

        // Same extension: uncompressed VGM bytes carrying the new tag.
        let vgm = retagged_bytes(&song, "01 New.vgm", new_tag.clone()).unwrap();
        assert!(
            !dro_core::vgm::io::is_gzipped(&vgm),
            "a .vgm stays uncompressed"
        );
        let reparsed = dro_core::io::read_song("01 New.vgm", &vgm).unwrap();
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
        assert!(dro_core::vgm::io::is_gzipped(&vgz), "a .vgz is gzipped");
        assert_eq!(
            dro_core::io::read_song("01 New.vgz", &vgz).unwrap().name,
            "01 New.vgz"
        );
    }

    #[test]
    fn validations_report_hard_errors_and_soft_warnings() {
        let files = vec![tagged_song("01 Intro.vgz", tag("Game", "A", "R"))];
        let mut state = RipState::from_folder(folder("Game", files), Some((2026, 7, 16)));

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
        let state = RipState::from_folder(folder("Game", files), Some((2026, 7, 16)));
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
        let mut state = RipState::from_folder(folder("Game", files), Some((2026, 7, 16)));
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
    fn the_optimize_toggle_flows_into_the_export_request() {
        let files = vec![tagged_song("01 Intro.vgz", tag("Game", "A", "R"))];
        let mut state = RipState::from_folder(folder("Game", files), Some((2026, 7, 16)));
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
        let state = RipState::from_folder(folder("Cool Game", files), Some((2026, 7, 16)));

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
        let mut state = RipState::from_folder(folder("G", files), Some((2026, 7, 16)));
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
            tagged_song("01 Intro.vgz", tag("Cool Game", "Ada", "Rip")),
            tagged_song("02 Boss.vgm", tag("Cool Game", "Ada", "Rip")),
        ];
        let state = RipState::from_folder(folder("Cool Game", files), Some((2026, 7, 16)));

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
        let mut state = RipState::from_folder(folder("Untitled", files), None);
        state.meta.game_name = String::new();
        assert!(!state.can_save());
        state.meta.game_name = "Named".to_owned();
        assert!(state.can_save());
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

    #[test]
    fn reorder_transaction_is_temp_safe_and_reversible() {
        let files = vec![
            tagged_song("01 Intro.vgz", tag("G", "A", "R")),
            tagged_song("02 Boss.vgm", tag("G", "B", "R")),
        ];
        let state = RipState::from_folder(folder("G", files), None);

        // Swap the two tracks: both move, so 2 renames -> a 4-step temp-then-final
        // batch, forward and inverse alike.
        let txn = state
            .reorder_transaction(0, 1)
            .expect("a real move builds a transaction");
        assert_eq!(txn.forward.len(), 4);
        assert_eq!(txn.inverse.len(), 4);

        let finals = |muts: &[RipMutation]| -> Vec<String> {
            muts.iter()
                .filter_map(|m| match m {
                    RipMutation::Rename { to, .. } if !to.starts_with(".drotrim") => {
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
        let meta = RipMeta {
            game_name: "Cool Game".to_owned(),
            system: "IBM PC/AT".to_owned(),
            release_date: "1994-03-01".to_owned(),
            creator: "Ripper".to_owned(),
            music_authors: "Ada, Bob".to_owned(),
            ..RipMeta::default()
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
        let overlay = seed_from_meta(&RipMeta::default());
        assert!(
            !overlay.writes_anything(),
            "a blank pack pre-checks nothing"
        );
    }
}
