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

use std::path::PathBuf;
use std::sync::Arc;

use dro_core::rip::{
    DEFAULT_OS, DEFAULT_SYSTEM, PRESETS, RipMeta, TrackEntry, doc_file_stem, format_track_time,
    generate_description, generate_m3u, music_hardware_suggestion, parse_description,
};
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
    /// The row currently previewing through the audio output (rip mode playback).
    pub preview: Option<usize>,
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
            preview: None,
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
        self.tracks = rescanned.tracks;
        self.images = rescanned.images;
        self.preview = previewing
            .and_then(|name| self.tracks.iter().position(|track| track.file_name == name));
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

    /// Blocking errors and non-blocking warnings for an export.
    #[must_use]
    pub fn validations(&self) -> RipValidations {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

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

        RipValidations { errors, warnings }
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
        }
    }
}

/// The result of [`RipState::validations`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RipValidations {
    /// Problems that block an export.
    pub errors: Vec<String>,
    /// Advisories the user may choose to ignore.
    pub warnings: Vec<String>,
}

/// The `NN` from a `NN Title.ext` file name, if present.
fn track_number(file_name: &str) -> Option<u32> {
    let digits: String = file_name.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !file_name[digits.len()..].starts_with(' ') {
        return None;
    }
    digits.parse().ok()
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
            ui.checkbox(&mut state.gzip_on_export, "Gzip to .vgz on export");
        });
    });
    if let Some(warning) = &state.parse_warning {
        ui.colored_label(palette.muted, warning);
    }
    crate::theme::separator_full(ui, palette);

    egui::ScrollArea::vertical().show(ui, |ui| {
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

        crate::theme::separator_full(ui, palette);
        track_table(ui, state, palette, actions);
        screenshots(ui, state, palette, actions);
    });
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
            .column(Column::auto().at_least(80.0)) // actions
            .header(row_height + 2.0, |mut header| {
                for title in ["#", "Title (GD3)", "Total", "Loop", ""] {
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
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 3.0;
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
    fn export_request_lists_songs_then_docs_with_final_names() {
        let files = vec![
            tagged_song("01 Intro.vgz", tag("Cool Game", "A", "R")),
            tagged_song("02 Boss.vgm", tag("Cool Game", "A", "R")),
        ];
        let state = RipState::from_folder(folder("Cool Game", files), Some((2026, 7, 16)));

        let request = state.export_request();
        assert_eq!(request.zip_name, "Cool Game.zip");
        assert!(request.gzip_vgms);
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
}
