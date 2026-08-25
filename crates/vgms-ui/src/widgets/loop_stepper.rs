//! The loop repeat-count stepper in the transport row.
//!
//! Counts `1..=9` and then "without end". The values form a *ring*: stepping up
//! past "without end" wraps back to `1`, and stepping down past `1` wraps to
//! "without end", so neither arrow ever dead-ends. The count means *total passes
//! over the region*, matching how players report a loop count, so `1` is "play it
//! once and move on" rather than "repeat it once".

use vgms_synth::LoopCount;

use crate::action::{Action, LoopAction};
use crate::theme::{self, Palette};

/// The highest finite count the stepper offers before "without end". Also the
/// cap a Render to WAV puts on an endless loop, so the export always terminates.
pub(crate) const MAX_FINITE: u32 = 9;

/// Draws the stepper: down arrow, the current count, up arrow. Emits
/// [`LoopAction::SetCount`] on a change.
pub(crate) fn loop_count_stepper(
    ui: &mut egui::Ui,
    palette: &Palette,
    count: LoopCount,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        let row_h = ui.spacing().interact_size.y;
        let arrow = egui::vec2(20.0, row_h);

        // "-" and "+" rather than the boost stepper's triangles: a count steps
        // rather than rises, and it keeps the two steppers' labels distinct for
        // accessibility (and for the headless tests, which query by label).
        if theme::bevel::button_sized(ui, palette, "\u{2212}", arrow)
            .on_hover_text(crate::strings::LOOP_STEPPER_FEWER)
            .clicked()
        {
            actions.push(Action::Loop(LoopAction::SetCount(decrement(count))));
        }

        // The count in a sunken well, like the boost value beside it. Painted
        // first, then a real label put in the same rect so the headless tests can
        // still find the value by text.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, row_h), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, palette.data_bg);
        theme::bevel::paint_bevel(ui.painter(), rect, palette, theme::bevel::Bevel::Sunken);
        ui.put(
            rect,
            egui::Label::new(egui::RichText::new(label(count)).color(palette.data_text)),
        )
        .on_hover_text(crate::strings::loop_stepper_hover(count));

        if theme::bevel::button_sized(ui, palette, "+", arrow)
            .on_hover_text(crate::strings::LOOP_STEPPER_MORE)
            .clicked()
        {
            actions.push(Action::Loop(LoopAction::SetCount(increment(count))));
        }
    });
}

/// The next count up, wrapping "without end" back to `1\u{00d7}` at the top of the ring.
fn increment(count: LoopCount) -> LoopCount {
    match count {
        LoopCount::Infinite => LoopCount::Times(1),
        LoopCount::Times(times) if times >= MAX_FINITE => LoopCount::Infinite,
        LoopCount::Times(times) => LoopCount::Times(times.max(1) + 1),
    }
}

/// The next count down, wrapping `1\u{00d7}` to "without end" at the bottom of the ring.
fn decrement(count: LoopCount) -> LoopCount {
    match count {
        LoopCount::Infinite => LoopCount::Times(MAX_FINITE),
        LoopCount::Times(times) if times <= 1 => LoopCount::Infinite,
        LoopCount::Times(times) => LoopCount::Times(times - 1),
    }
}

fn label(count: LoopCount) -> String {
    match count {
        LoopCount::Infinite => "\u{221E}".to_owned(),
        LoopCount::Times(times) => format!("{}\u{00d7}", times.max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_wraps_around_the_ring() {
        assert_eq!(increment(LoopCount::Times(1)), LoopCount::Times(2));
        assert_eq!(
            increment(LoopCount::Times(MAX_FINITE)),
            LoopCount::Infinite,
            "past the last finite count is 'without end'"
        );
        assert_eq!(
            increment(LoopCount::Infinite),
            LoopCount::Times(1),
            "stepping up past 'without end' wraps to 1x"
        );
        assert_eq!(
            decrement(LoopCount::Infinite),
            LoopCount::Times(MAX_FINITE),
            "stepping back down re-enters at the highest finite count"
        );
        assert_eq!(
            decrement(LoopCount::Times(1)),
            LoopCount::Infinite,
            "stepping down past 1x wraps to 'without end'"
        );
    }

    #[test]
    fn a_zero_count_is_treated_as_one() {
        // Nothing in the UI produces `Times(0)`, but the engine equates it with
        // "no repeat", so the stepper must not show or step it as something else.
        assert_eq!(label(LoopCount::Times(0)), "1\u{00d7}");
        assert_eq!(decrement(LoopCount::Times(0)), LoopCount::Infinite);
        assert_eq!(increment(LoopCount::Times(0)), LoopCount::Times(2));
    }

    #[test]
    fn the_label_reads_as_a_count() {
        assert_eq!(label(LoopCount::Infinite), "\u{221E}");
        assert_eq!(label(LoopCount::Times(3)), "3\u{00d7}");
    }
}
