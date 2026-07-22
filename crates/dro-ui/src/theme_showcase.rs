//! A test-only "theme showcase": every themed surface laid out on one canvas,
//! snapshotted once per [`ThemeChoice`] so a single test guards the whole theme.
//!
//! This view is **not** reachable from the application UI -- it exists purely
//! for the snapshot test at the bottom and is compiled only under `#[cfg(test)]`.
//! The rest of the GUI's snapshot tests render just the default theme; this one
//! renders each theme, so a palette or style change is caught here without every
//! other baseline having to be duplicated per theme.
//!
//! It covers the theme *surface* -- palette roles, the [`egui::Style`] built by
//! `theme::style_for`, the DOS font, and the custom painters (bevels, grooves,
//! waveform, peak meter, table) -- not app *chrome composition* (menu bar,
//! transport rows), which is theme-independent and already covered by the app's
//! own baselines.
//!
//! Like the app's other snapshots, the baselines are rendered through the wgpu
//! DX12 WARP software adapter on the maintainer's Windows machine and are
//! therefore GPU/OS-specific; regenerate them there (see `DEVELOPMENT.md`).

use dro_core::Song;
use dro_core::config::{AppConfig, ThemeChoice};
use dro_synth::render_waveform;
use egui::{Color32, Sense};

use crate::dialogs::SettingsDialog;
use crate::editor::Editor;
use crate::platform::PickedFile;
use crate::test_song::tone_song;
use crate::theme::bevel::{self, Bevel};
use crate::theme::icon::Icon;
use crate::theme::{self, Palette};
use crate::widgets::channels::ChannelPanel;
use crate::widgets::peak_meter::{self, PeakMeterState};
use crate::widgets::position_panel::PositionPanel;
use crate::widgets::table;
use crate::widgets::waveform::{self, NUM_BUCKETS, WaveformState};

/// The OPL3 native rate, as the app's default rendering frequency.
const FREQUENCY: u32 = 49_716;

/// Everything the showcase's widgets need to be driven deterministically.
#[derive(Debug)]
struct ShowcaseState {
    editor: Editor,
    waveform: WaveformState,
    meter: PeakMeterState,
    channels: ChannelPanel,
    position: PositionPanel,
    settings: SettingsDialog,
    text: String,
    text_empty: String,
    slider: f32,
    drag: u32,
    check_on: bool,
    check_off: bool,
}

impl ShowcaseState {
    fn new() -> Self {
        let song = tone_song();
        // Same integer DSP the app renders through, so the wave is bit-faithful
        // to a real render (and stable across platforms, unlike an f32 synthetic).
        let buckets = render_waveform(&song, NUM_BUCKETS, FREQUENCY);

        let mut editor = Editor::new();
        editor.load(picked(&song)).expect("the tone fixture parses");
        editor.selection.select_only(1);

        // Prime the holds to the top, then drop the bars: the left column lands
        // loud (into the red zone) and the right column quiet, both with a
        // peak-hold marker sitting above the bar.
        let mut meter = PeakMeterState::default();
        meter.update(Some([1.0, 1.0]), 0.0);
        meter.update(Some([0.9, 0.25]), 0.3);

        let mut position = PositionPanel::new(FREQUENCY);
        position.set_length_ms(300_000);
        position.set_position_ms(123_456);

        // A mix of muted and audible toggles across both banks, and Custom
        // panning engaged with a spread of positions so the pan knobs render at
        // varied angles (hard left, left, centre, right, hard right).
        let mut channels = ChannelPanel::new();
        channels.toggle_channel(2);
        channels.toggle_channel(4);
        channels.toggle_channel(12);
        let mut pans = [0x80u8; 18];
        for (slot, pan) in pans.iter_mut().enumerate() {
            *pan = [0x00, 0x40, 0x80, 0xC0, 0xFF][slot % 5];
        }
        channels.set_showcase_pans(pans);

        Self {
            editor,
            waveform: WaveformState {
                buckets,
                start_ms: 90,
                cursor_ms: 210,
                // A marked, actively looping region so the brackets, their solid
                // flags and the wash are all on screen for the per-theme baseline
                // to guard. (`loop_overlay` covers the hollow, unapplied flags.)
                loop_overlay: Some(waveform::LoopOverlay {
                    start_ms: 140,
                    end_ms: 330,
                    active: true,
                    unapplied: false,
                }),
            },
            meter,
            channels,
            position,
            settings: SettingsDialog::new(&AppConfig::default()),
            text: "Sample text".to_owned(),
            text_empty: String::new(),
            slider: 0.35,
            drag: 1337,
            check_on: true,
            check_off: false,
        }
    }
}

/// Serialise a fixture song and wrap it as a picked file (no path -- the
/// showcase never saves), the same trick the editor's own tests use.
fn picked(song: &Song) -> PickedFile {
    PickedFile {
        name: song.name.clone(),
        path: None,
        bytes: dro_core::io::write_song(song).expect("the tone fixture serialises"),
    }
}

/// Renders the whole showcase for `choice` into `ui`.
fn show(ui: &mut egui::Ui, state: &mut ShowcaseState, choice: ThemeChoice) {
    let p = theme::palette(choice);

    // Chrome widgets are designed to sit on the panel face, where each theme's
    // label colour is legible (near-white on Clone, black on FT2); the data
    // widgets bring their own dark wells. So put the whole surface on `face`;
    // the `desktop` role still shows through the canvas's transparent rim and
    // as its own swatch below.
    let dialog_top = egui::Frame::new()
        .fill(p.face)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.heading(format!("Theme showcase \u{2014} {choice}"));

            section(ui, p, "Palette roles");
            swatches(ui, p);

            section(ui, p, "Standard widgets");
            widget_gallery(ui, state, p);

            section(ui, p, "Bevels, grooves, separators");
            primitives(ui, p);

            section(ui, p, "Pads and icons");
            pads_and_icons(ui, p);
            theme::separator_full(ui, p);

            section(ui, p, "Scroll area (striped, solid scrollbar)");
            scroll_sample(ui);

            section(ui, p, "Channel panel");
            state.channels.show(ui, p);

            section(ui, p, "Position panel");
            // `PositionPanel::show` centres-and-justifies within its columns,
            // which fills the whole remaining height in an unbounded layout
            // (the app hosts it in a fixed-height bottom panel); bound it here.
            ui.allocate_ui(egui::vec2(ui.available_width(), 28.0), |ui| {
                state.position.show(ui, p);
            });

            section(ui, p, "Waveform and peak meter");
            waveform_and_meter(ui, state, p);

            section(ui, p, "Instruction table");
            table_sample(ui, state, p);

            // Reserve blank face for the floating Settings window below, and
            // report where it starts so the window's area can anchor to the
            // real layout position rather than a guessed absolute coordinate.
            section(ui, p, "Settings dialog (window fill, title bar, inputs)");
            let top = ui.cursor().top();
            ui.add_space(384.0);
            top
        })
        .inner;

    // A modeless window painted at the ctx level, constrained into the blank
    // band reserved above. Fresh-memory auto-placement plus `constrain_to`
    // pins it to the band's top-left, the same way the app's track-edit
    // snapshot places its dialog.
    let area = egui::Rect::from_min_max(
        egui::pos2(24.0, dialog_top),
        egui::pos2(1000.0, dialog_top + 376.0),
    );
    state.settings.show(ui.ctx(), p, area, &mut Vec::new());
}

/// A section heading, in the palette's chrome-label colour so it reads on the
/// face in either theme.
fn section(ui: &mut egui::Ui, p: &Palette, title: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).strong().color(p.label));
    ui.add_space(2.0);
}

/// Every [`Palette`] field, destructured without `..` so a new role added to
/// the struct is a compile error here until it is given a swatch.
fn roles(p: &Palette) -> [(&'static str, Color32); 46] {
    let Palette {
        face,
        face_hover,
        face_active,
        desktop,
        bevel_light,
        bevel_dark,
        bevel_border,
        data_bg,
        data_stripe,
        data_hover,
        data_text,
        data_label,
        trough,
        label,
        muted,
        button_face,
        button_hover,
        button_active,
        button_light,
        button_shadow,
        button_text,
        button_pressed,
        button_pressed_text,
        pad_cap_top,
        pad_cap_bottom,
        pad_border,
        pad_ink,
        accent,
        selection_text,
        wf_bg,
        wf_wave,
        wf_hover,
        wf_start,
        wf_cursor,
        wf_dim,
        meter_off,
        meter_low,
        meter_mid,
        meter_high,
        meter_hold,
        latch_top,
        latch_bottom,
        latch_border,
        latch_ink,
        wf_loop,
        wf_loop_region,
    } = *p;
    [
        ("face", face),
        ("face_hover", face_hover),
        ("face_active", face_active),
        ("desktop", desktop),
        ("bevel_light", bevel_light),
        ("bevel_dark", bevel_dark),
        ("bevel_border", bevel_border),
        ("data_bg", data_bg),
        ("data_stripe", data_stripe),
        ("data_hover", data_hover),
        ("data_text", data_text),
        ("data_label", data_label),
        ("trough", trough),
        ("label", label),
        ("muted", muted),
        ("button_face", button_face),
        ("button_hover", button_hover),
        ("button_active", button_active),
        ("button_light", button_light),
        ("button_shadow", button_shadow),
        ("button_text", button_text),
        ("button_pressed", button_pressed),
        ("button_pressed_text", button_pressed_text),
        ("pad_cap_top", pad_cap_top),
        ("pad_cap_bottom", pad_cap_bottom),
        ("pad_border", pad_border),
        ("pad_ink", pad_ink),
        ("accent", accent),
        ("selection_text", selection_text),
        ("wf_bg", wf_bg),
        ("wf_wave", wf_wave),
        ("wf_hover", wf_hover),
        ("wf_start", wf_start),
        ("wf_cursor", wf_cursor),
        ("wf_dim", wf_dim),
        ("wf_loop", wf_loop),
        ("wf_loop_region", wf_loop_region),
        ("meter_off", meter_off),
        ("meter_low", meter_low),
        ("meter_mid", meter_mid),
        ("meter_high", meter_high),
        ("meter_hold", meter_hold),
        ("latch_top", latch_top),
        ("latch_bottom", latch_bottom),
        ("latch_border", latch_border),
        ("latch_ink", latch_ink),
    ]
}

/// A grid of named colour chips covering every palette role, including the
/// hover-only ones (`face_hover`, `data_hover`, ...) that no still widget shows.
fn swatches(ui: &mut egui::Ui, p: &Palette) {
    egui::Grid::new("showcase-swatches")
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for (index, (name, color)) in roles(p).into_iter().enumerate() {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 16.0), Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, color);
                    bevel::paint_bevel(ui.painter(), rect, p, Bevel::Sunken);
                    ui.label(egui::RichText::new(name).size(11.0).color(p.label));
                });
                if index % 5 == 4 {
                    ui.end_row();
                }
            }
        });
}

/// The stock egui widgets, as styled by `theme::style_for`. None is ever
/// focused or hovered (snapshots remove the cursor), so states are shown by
/// pairing explicit on/off variants.
fn widget_gallery(ui: &mut egui::Ui, state: &mut ShowcaseState, p: &Palette) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Label");
        ui.label(egui::RichText::new("weak / muted").weak());
        ui.hyperlink_to("hyperlink", "https://example.invalid");
        ui.monospace("monospace");
        ui.code("code");
    });
    ui.horizontal_wrapped(|ui| {
        let _ = ui.button("Button");
        ui.add_enabled(false, egui::Button::new("Disabled"));
        let _ = ui.selectable_label(true, "Selected");
        let _ = ui.selectable_label(false, "Unselected");
        ui.checkbox(&mut state.check_on, "Checked");
        ui.checkbox(&mut state.check_off, "Unchecked");
        let _ = ui.radio(true, "Radio on");
        let _ = ui.radio(false, "Radio off");
    });
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::Slider::new(&mut state.slider, 0.0..=1.0).text("slider"));
        ui.add(egui::DragValue::new(&mut state.drag));
        ui.add(
            egui::TextEdit::singleline(&mut state.text)
                .desired_width(120.0)
                .text_color(p.data_text),
        );
        ui.add(
            egui::TextEdit::singleline(&mut state.text_empty)
                .desired_width(120.0)
                .hint_text("hint text"),
        );
        egui::ComboBox::from_id_salt("showcase-combo-plain")
            .selected_text("Plain combo")
            .show_ui(ui, |_| {});
        ui.scope(|ui| {
            theme::style_dropdown(ui, p);
            egui::ComboBox::from_id_salt("showcase-combo-styled")
                .selected_text("Styled combo")
                .show_ui(ui, |_| {});
        });
    });
    // A horizontal groove in a vertical layout...
    theme::separator(ui, p);
    // ...and a vertical groove within a row.
    ui.horizontal(|ui| {
        ui.label("left");
        theme::separator(ui, p);
        ui.label("right");
    });
}

/// The custom bevel primitives: raised/sunken edges, both grooves, and the
/// from-scratch FT2 push-buttons.
fn primitives(ui: &mut egui::Ui, p: &Palette) {
    ui.horizontal(|ui| {
        for (label, style) in [("Raised", Bevel::Raised), ("Sunken", Bevel::Sunken)] {
            ui.vertical(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(72.0, 36.0), Sense::hover());
                ui.painter().rect_filled(rect, 0.0, p.face);
                bevel::paint_bevel(ui.painter(), rect, p, style);
                ui.label(egui::RichText::new(label).size(11.0).color(p.label));
            });
        }

        bevel::button(ui, p, "Bevel button");
        bevel::button_sized(ui, p, "\u{23EE}", egui::vec2(34.0, 36.0));

        ui.vertical(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(140.0, 16.0), Sense::hover());
            bevel::groove_h(ui.painter(), rect.x_range(), rect.center().y - 1.0, p);
            ui.label(egui::RichText::new("groove_h").size(11.0).color(p.label));
        });

        let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 36.0), Sense::hover());
        bevel::groove_v(ui.painter(), rect.center().x - 1.0, rect.y_range(), p);
    });
}

/// Every line-icon glyph on an idle pad, then a latched toggle (lit amber) and a
/// couple of text pads, so a change to the pad chrome or any glyph is caught per
/// theme.
fn pads_and_icons(ui: &mut egui::Ui, p: &Palette) {
    const GLYPHS: [(Icon, &str); 14] = [
        (Icon::Del, "Delete"),
        (Icon::Play, "Play"),
        (Icon::Stop, "Stop"),
        (Icon::Tail, "Tail"),
        (Icon::Seam, "Seam"),
        (Icon::Loop, "Loop"),
        (Icon::Lock, "Lock"),
        (Icon::Match, "Match"),
        (Icon::Custom, "Custom"),
        (Icon::Reset, "Reset"),
        (Icon::Perc, "Perc"),
        (Icon::All, "All"),
        (Icon::Up, "Up"),
        (Icon::Dn, "Down"),
    ];
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (glyph, label) in GLYPHS {
            bevel::icon_button(ui, p, glyph, label);
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        // A latched icon toggle (lit amber) beside an idle one, then text pads.
        let mut on = true;
        let mut off = false;
        bevel::icon_toggle(ui, p, &mut off, Icon::Loop, "Loop (off)");
        bevel::icon_toggle(ui, p, &mut on, Icon::Loop, "Loop (on)");
        bevel::button(ui, p, "Text pad");
        let mut latched = true;
        bevel::toggle(ui, p, &mut latched, "On");
        let mut clear = false;
        bevel::toggle(ui, p, &mut clear, "Off");
    });
}

/// A short striped list taller than its viewport, so the theme's solid 14px
/// scrollbar (trough + handle) is rendered.
fn scroll_sample(ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .max_height(72.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("showcase-scroll")
                .striped(true)
                .num_columns(2)
                .show(ui, |ui| {
                    for row in 0..14 {
                        ui.label(format!("Row {row:02}"));
                        ui.monospace(format!("value {}", row * 7));
                        ui.end_row();
                    }
                });
        });
}

/// The waveform well and peak meter, wrapped exactly as the app's waveform
/// panel does (fixed height, the meter's width reserved on the right).
fn waveform_and_meter(ui: &mut egui::Ui, state: &ShowcaseState, p: &Palette) {
    egui::Frame::new()
        .fill(p.data_bg)
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            // Bound the row height so the meter (which fills the available
            // height) does not stretch down the rest of the canvas.
            ui.allocate_ui(egui::vec2(ui.available_width(), 120.0), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let wave_width =
                        ui.available_width() - peak_meter::WIDTH - ui.spacing().item_spacing.x;
                    ui.allocate_ui(egui::vec2(wave_width, 120.0), |ui| {
                        waveform::show(ui, &state.waveform, state.editor.song(), p);
                    });
                    peak_meter::show(ui, &state.meter, p);
                });
            });
        });
}

/// The real virtual instruction table, bounded so its own scrollbar appears.
fn table_sample(ui: &mut egui::Ui, state: &mut ShowcaseState, p: &Palette) {
    egui::Frame::new()
        .fill(p.data_bg)
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.allocate_ui(egui::vec2(ui.available_width(), 168.0), |ui| {
                table::show(ui, &mut state.editor, None, p);
            });
        });
}

/// Renders the showcase once per theme and snapshots each. A single test guards
/// every theme; the other GUI snapshots need only the default theme.
#[test]
fn snapshot_theme_showcase() {
    // One shared accumulator across both harnesses: `harness.snapshot()` panics
    // on drop when two unhandled `SnapshotResults` exist in a test (even when
    // green), and this reports both themes' diffs together.
    let mut results = egui_kittest::SnapshotResults::new();
    for choice in ThemeChoice::ALL {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1024.0, 1600.0))
            .with_max_steps(64)
            .wgpu()
            .build_ui_state(
                move |ui, state: &mut ShowcaseState| show(ui, state, choice),
                ShowcaseState::new(),
            );

        // `build_ui_state` has no `CreationContext`, so install the theme on the
        // public ctx after building. `apply_palette` replaces both style slots
        // wholesale, which drops kittest's own snapshot invariants (`style_for`
        // restores `animation_time = 0` but not these), so re-apply them before
        // rendering the frame that gets snapshotted.
        theme::install(&harness.ctx, choice);
        harness.ctx.all_styles_mut(|style| {
            style.visuals.text_cursor.blink = false;
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });

        harness.run();
        // Settle the pointer out of frame so no synthetic cursor/hover is baked
        // into the baseline, then re-render and capture.
        harness.remove_cursor();
        harness.run();
        results.add(harness.try_snapshot(format!("theme_showcase_{choice}")));
    }
    results.unwrap();
}
