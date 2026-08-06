//! Pack mode's egui view: the pack header, the sub-section tabs and each
//! section's body -- the metadata form, the track table, the screenshots panel,
//! the submission checklist -- and the output deck at the foot of the window.
//! [`show`] is the entry point; the headless model it draws is [`super::state`].

use egui_extras::{Column, TableBuilder};

use vgms_core::pack::readiness::{
    MetaField, ReadinessCategory, ReadinessItem, ReadinessTarget, Severity,
};
use vgms_core::pack::{CONSOLE_PRESETS, PRESETS, format_byte_count, format_track_time};

use crate::action::Action;
use crate::theme::{Palette, bevel};

use super::state::{PackImage, PackSection, PackState, PackTrack};

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
            // A zip-opened pack has no folder to write docs to; its save is a
            // re-export of the whole archive (wt-8).
            if state.origin.is_memory() {
                if bevel::button(ui, palette, "Save .zip\u{2026}")
                    .on_hover_text(crate::strings::PACK_SAVE_ARCHIVE_TIP)
                    .clicked()
                {
                    actions.push(Action::PackSaveArchive);
                }
            } else if bevel::button(ui, palette, "Save Pack")
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
        let mut row_rects: Vec<(usize, egui::Rect)> = Vec::new();
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
            // `rows`, not a `row` per track: this is an immediate-mode table
            // inside a scroll area, so a `for` loop lays out and hit-tests every
            // track in the pack on every frame whether or not it is on screen.
            // `rows` draws only the rows the clip rect actually shows -- about
            // fifteen -- which is what stops the view getting slower the longer
            // the pack is. Uniform `row_height` is what makes it applicable.
            .body(|body| {
                let count = state.tracks.len();
                body.rows(row_height, count, |mut row| {
                    let index = row.index();
                    let Some(track) = state.tracks.get(index) else {
                        return;
                    };
                    // The keyboard's row, lit like a selection so Alt+arrow
                    // has something visible to act on.
                    row.set_selected(state.focused_track == Some(index));
                    row.col(|ui| {
                        // The row's own response rect overshoots into the
                        // next row; a cell's does not, and the y-range is
                        // all the drop target needs. Only the drawn rows
                        // land here, so the index travels with the rect --
                        // a position in this list is no longer a track
                        // number.
                        row_rects.push((index, ui.max_rect()));
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
                                        .on_hover_text(
                                            if peak.clipped {
                                                crate::strings::PACK_PEAK_TIP_CLIPPED
                                            } else {
                                                crate::strings::PACK_PEAK_TIP
                                            },
                                        );
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
                });
            });
        drop_target(ui, &row_rects, palette, actions);
        // A row moved by the keyboard must not walk off the top or bottom of
        // the view. Asked here rather than from the row itself, for two
        // reasons: a culled row never draws, so the one case that needs
        // scrolling is the one that could not ask; and a `scroll_to_me` inside
        // the table is swallowed by the table's own (disabled) scroll area
        // before the section's scroll area -- the one that actually moves --
        // ever sees it.
        //
        // Where the row *would* be is extrapolated from a row that did draw,
        // rather than from the header's height: the rows are a uniform pitch
        // apart, so one drawn rect and its index fix the whole column, and
        // nothing here has to guess what the header cost.
        if let Some(index) = scroll_to
            && let Some(&(known, rect)) = row_rects.first()
        {
            let step = row_height + ui.spacing().item_spacing.y;
            let top = rect.top() + step * (index as f32 - known as f32);
            let row = egui::Rect::from_x_y_ranges(rect.x_range(), top..=top + row_height);
            ui.scroll_to_rect(row, Some(egui::Align::Center));
        }
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
/// land, and on release emits the move. `row_rects` is `(track index, rect)` for
/// each row the table actually drew, in list order -- which since the table
/// culls is the visible window, not the whole pack. That is no loss: a drop can
/// only be aimed at a row that can be seen.
///
/// The insertion slot is a boundary (0 = above the first row, `len` = below the
/// last), so it is converted to a destination *index* -- one less when the track
/// is moving down, since removing it first shifts everything below up.
fn drop_target(
    ui: &mut egui::Ui,
    row_rects: &[(usize, egui::Rect)],
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
    if let Some(pointer) = ui.ctx().pointer_interact_pos()
        && let (Some(&(first, _)), Some(&(last, last_rect))) = (row_rects.first(), row_rects.last())
    {
        // A boundary in *track* numbers: the index of the row the dragged track
        // would push down, or one past the last drawn row when it goes below
        // them all.
        let slot = row_rects
            .iter()
            .find(|(_, rect)| pointer.y < rect.center().y)
            .map_or(last + 1, |&(index, _)| index)
            .clamp(first, last + 1);
        ui.ctx().data_mut(|data| data.insert_temp(slot_id, slot));
        // The boundary that slot sits on: the top of the row it would push down,
        // or the foot of the table when it is going last.
        let y = row_rects
            .iter()
            .find(|(index, _)| *index == slot)
            .map_or_else(|| last_rect.bottom(), |(_, rect)| rect.top());
        ui.painter().hline(
            ui.max_rect().x_range(),
            y,
            egui::Stroke::new(2.0, palette.data_text),
        );
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
            actions.push(Action::RecompressImage(index));
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
