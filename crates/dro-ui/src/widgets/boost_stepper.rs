//! The live-playback boost stepper, extracted from the transport row to keep
//! `DroApp::update_impl` lean (uishell-6).

use crate::action::Action;
use crate::theme::{self, Palette};

/// Draws the boost control right-aligned in the transport row: up/down arrows,
/// an editable integer value (`1..=5`), a "Boost" label, and a dividing groove.
/// Emits [`Action::SetBoost`] on any change.
pub fn boost_stepper(ui: &mut egui::Ui, palette: &Palette, boost: f32, actions: &mut Vec<Action>) {
    // Live playback boost, right-aligned in the row. A limiter behind it prevents
    // clipping; the WAV render and the waveform stay at the un-boosted level.
    // Built right-to-left: the up/down arrows, the editable value, the "Boost"
    // label, then a full-height groove dividing it from the transport buttons.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let row_h = ui.spacing().interact_size.y;
        // The control runs boost in integer steps 1..=5; a hand-edited ini may
        // hold a fractional value, floored here.
        let current = boost.floor().clamp(1.0, 5.0) as i32;

        // Up/down arrows, snug together (rightmost in the row). A nested
        // `ui.horizontal` inherits the enclosing right-to-left layout, so add down
        // first and up second and they come out up-on-the-left, down-on-the-right,
        // like a stepper. (Forcing left-to-right here corrupts the parent.)
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            let arrow = egui::vec2(20.0, row_h);
            if theme::bevel::button_sized(ui, palette, "\u{25BC}", arrow)
                .on_hover_text("Quieter")
                .clicked()
                && current > 1
            {
                actions.push(Action::SetBoost {
                    value: (current - 1) as f32,
                    persist: true,
                });
            }
            if theme::bevel::button_sized(ui, palette, "\u{25B2}", arrow)
                .on_hover_text("Louder")
                .clicked()
                && current < 5
            {
                actions.push(Action::SetBoost {
                    value: (current + 1) as f32,
                    persist: true,
                });
            }
        });

        // The value: a dark well with the tracker-yellow digit, click to type.
        // Typed input floors to an integer 1..=5.
        ui.scope(|ui| {
            let widgets = &mut ui.visuals_mut().widgets;
            for w in [
                &mut widgets.inactive,
                &mut widgets.hovered,
                &mut widgets.active,
            ] {
                w.weak_bg_fill = palette.data_bg;
                w.bg_fill = palette.data_bg;
                w.fg_stroke.color = palette.data_text;
            }
            let mut value = boost;
            let db = 20.0 * (current as f32).log10();
            let response = ui
                .add(
                    egui::DragValue::new(&mut value)
                        .speed(0.0)
                        .update_while_editing(false)
                        .custom_formatter(|n, _| format!("{}", n.floor().clamp(1.0, 5.0) as i64))
                        .custom_parser(|s| {
                            s.trim().parse::<f64>().ok().map(|v| v.floor().clamp(1.0, 5.0))
                        }),
                )
                .on_hover_text(format!("{current}\u{00d7} ({db:+.1} dB)"));
            // No continuous drag (speed 0), so a change is always a committed edit
            // -- persist it once, like an arrow click.
            if response.changed() {
                actions.push(Action::SetBoost {
                    value,
                    persist: true,
                });
            }
        });

        // The label sits left of the value...
        ui.label("Boost");
        // ...and a 2px beveled groove at full row height separates the boost
        // section from the transport buttons, matching the grooves between the
        // stacked panels.
        theme::separator(ui, palette);
    });
}
