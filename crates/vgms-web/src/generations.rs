// SPDX-License-Identifier: GPL-2.0-or-later
//! Per-kind generation bookkeeping for the Worker task service.
//!
//! A background result can arrive after its task was superseded (a newer submit,
//! or a cancel): the Worker is terminated, but a result it already posted may
//! already sit in the main thread's queue. Each spawn tags its Worker with a
//! generation, and a result is kept only if it still carries the current one for
//! its kind -- the same filter the native task service uses. Kept out of the
//! wasm-only `services` module so it can be unit-tested off-target.

use std::collections::HashMap;

use vgms_ui::tasks::TaskKind;

/// The current generation per task kind.
#[derive(Debug, Default)]
pub(crate) struct Generations {
    current: HashMap<TaskKind, u64>,
}

impl Generations {
    /// Advances `kind` to a new generation and returns it, to tag a fresh spawn.
    /// A terminate or cancel calls this too, so a superseded Worker's late result
    /// no longer matches the current generation.
    pub(crate) fn bump(&mut self, kind: TaskKind) -> u64 {
        let generation = self.current.entry(kind).or_insert(0);
        *generation += 1;
        *generation
    }

    /// The current generation for `kind` (`0` if it has never been spawned).
    pub(crate) fn current(&self, kind: TaskKind) -> u64 {
        self.current.get(&kind).copied().unwrap_or(0)
    }

    /// Whether a result tagged `generation` for `kind` is still current.
    pub(crate) fn is_current(&self, kind: TaskKind, generation: u64) -> bool {
        self.current.get(&kind).copied() == Some(generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_from_a_superseded_generation_is_dropped() {
        let mut generations = Generations::default();
        let first = generations.bump(TaskKind::RenderWaveform);
        assert!(generations.is_current(TaskKind::RenderWaveform, first));

        // A newer spawn (or a cancel) supersedes it.
        let second = generations.bump(TaskKind::RenderWaveform);
        assert!(generations.is_current(TaskKind::RenderWaveform, second));
        assert!(
            !generations.is_current(TaskKind::RenderWaveform, first),
            "a late result from the first generation no longer matches"
        );
    }

    #[test]
    fn kinds_are_tracked_independently() {
        let mut generations = Generations::default();
        let wave = generations.bump(TaskKind::RenderWaveform);
        let _scan = generations.bump(TaskKind::VolumeScan);
        // Bumping one kind does not supersede another.
        assert!(generations.is_current(TaskKind::RenderWaveform, wave));
    }

    #[test]
    fn an_untracked_kind_has_no_current_generation() {
        let generations = Generations::default();
        assert_eq!(generations.current(TaskKind::RenderWaveform), 0);
        assert!(!generations.is_current(TaskKind::RenderWaveform, 1));
    }
}
