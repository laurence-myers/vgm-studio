//! How fast each core runs, relative to realtime -- the picker's speed
//! readout and the fidelity auto-select's gate.
//!
//! Two parts, multiplied:
//!
//! - A **baseline** per core, measured once on the project's reference
//!   machine (see [`BASELINE`]) with the same silent-render workload
//!   [`measure_speed`] uses. Only the cores whose speed is *interesting* --
//!   the die sims that flirt with realtime and the emulators beside them in
//!   a picker -- carry a number; a core absent from the table is comfortably
//!   fast, and the UI says nothing rather than inventing a figure.
//! - One **machine ratio**, "this computer versus the reference machine",
//!   measured on demand by [`measure_machine_ratio`]: a quick render through
//!   two probe cores of very different character (a die sim and a fast
//!   model), each compared to its own baseline, geometric-meaned. One number
//!   scales every baseline -- coarse, but honest about being an estimate,
//!   and it is what lets a faster CPU promote the die sims some day.
//!
//! The ratio is process-global like the core choices: seeded from the config
//! at startup, re-set the moment a measurement completes (a measurement is a
//! fact about the machine, not a preference), persisted by Settings.

// Used only by the native measurement path; wasm compiles it out.
#[cfg(not(target_arch = "wasm32"))]
use crate::registry::CoreInfo;

/// Per-core baseline speeds, ×realtime on the reference machine.
///
/// Measured by `core_speed_baseline` in vgms-app (`cargo test --release --
/// --ignored core_speed_baseline`), which prints this table's rows; paste its
/// output here when cores or the reference machine change. The workload is
/// [`measure_speed`]'s: a silent render at the chip's native rate -- write
/// cost excluded, which flatters nothing the gate cares about (the die sims'
/// cost is clock-driven, not write-driven).
pub const BASELINE: &[(&str, f32)] = &[
    // Measured 2026-08-08 on the reference machine (the project owner's box),
    // release build, quiet system.
    ("opl3.nuked", 50.0),
    ("opl3.cqm", 7.56),
    ("opl3.opl2-lite", 40.0),
    ("opl3.ym3812-lle", 1.24),
    ("opl3.ymf262-lle", 0.22),
    ("ym2151.nuked", 3.44),
    ("ym2151.lle", 1.58),
    ("ym2612.nuked", 10.0),
    ("ym2612.lle", 0.65),
    ("ym2612.ymf276-lle", 1.07),
    ("ym2608.lle", 0.16),
    ("ym2203.lle", 1.77),
    ("ym2413.nuked", 20.0),
    ("sn76489.nuked-psg", 30.0),
];

/// The baseline speed for a core id, ×realtime on the reference machine.
/// `None` for a core the table does not track: comfortably fast.
#[must_use]
pub fn baseline_speed(id: &str) -> Option<f32> {
    BASELINE
        .iter()
        .find(|(entry, _)| *entry == id)
        .map(|&(_, speed)| speed)
}

/// This machine's measured speed relative to the reference machine, applied
/// to every baseline. `None` until measured (treated as 1.0 -- "assume the
/// reference machine", which the UI marks as an estimate).
static MACHINE_RATIO: std::sync::RwLock<Option<f32>> = std::sync::RwLock::new(None);

/// Installs the machine ratio: from the config at startup, or fresh from
/// [`measure_machine_ratio`]. `None` returns to the unmeasured state.
pub fn set_machine_ratio(ratio: Option<f32>) {
    *MACHINE_RATIO.write().expect("not poisoned") = ratio.filter(|r| r.is_finite() && *r > 0.0);
}

/// The installed machine ratio, if one has been measured.
#[must_use]
pub fn machine_ratio() -> Option<f32> {
    *MACHINE_RATIO.read().expect("not poisoned")
}

/// The estimated speed of a core on *this* machine, ×realtime: its baseline
/// scaled by the machine ratio. `None` for an untracked (comfortably fast)
/// core.
#[must_use]
pub fn effective_speed(id: &str) -> Option<f32> {
    baseline_speed(id).map(|speed| speed * machine_ratio().unwrap_or(1.0))
}

/// A coarse speed band, for the picker's readout -- three words instead of a
/// number, because a user chooses on "can this play live" not on a decimal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedTier {
    /// Below realtime on this machine: render-only, live playback stutters.
    Offline,
    /// Holds realtime, but with little headroom -- a heavy song may struggle.
    Slow,
    /// Comfortably above realtime.
    Fast,
}

/// Where the "slow" band ends. An estimate below this holds realtime but
/// leaves the audio callback little room; the fidelity auto-select's
/// [`crate::registry`] headroom sits inside this band.
const FAST_FLOOR: f32 = 5.0;

impl SpeedTier {
    /// Every band, slowest first -- for the picker legend.
    pub const ALL: [Self; 3] = [Self::Offline, Self::Slow, Self::Fast];

    /// The band an ×realtime estimate falls in.
    #[must_use]
    pub fn of(speed: f32) -> Self {
        if speed < 1.0 {
            Self::Offline
        } else if speed < FAST_FLOOR {
            Self::Slow
        } else {
            Self::Fast
        }
    }

    /// The picker's one-word readout.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Offline => "Offline",
            Self::Slow => "Slow",
            Self::Fast => "Fast",
        }
    }

    /// One line for the tooltip and the picker legend.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Offline => "Below realtime here. Use it for renders; live playback stutters.",
            Self::Slow => "Holds realtime, with little headroom.",
            Self::Fast => "Comfortably faster than realtime.",
        }
    }
}

/// Measures one core's silent-render speed, ×realtime.
///
/// Builds the row, resets it at `clock`, and renders at its native rate until
/// enough wall time has passed to trust the figure. Silent (no writes): the
/// die sims' cost is clock-driven so this is their true cost, and the fast
/// tier only needs to clear the bar, which idling understates rather than
/// overstates for the envelope-gated models. Native only -- wasm has no
/// monotonic clock here, and the worklet never measures.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn measure_speed(info: &CoreInfo, clock: u32) -> Option<f32> {
    let mut core = info.build()?;
    core.reset(clock, false);
    let rate = core.native_rate().max(1);
    // A second of audio per pass for the fast tier, an eighth for a die sim
    // -- enough wall time either way for a stable figure.
    let chunk_frames = (rate / 8).max(1) as usize;
    let mut buffer = vec![0i32; chunk_frames * 2];
    let started = std::time::Instant::now();
    let mut rendered_frames = 0u64;
    // At least 60 ms of wall time and one whole chunk, capped so a very slow
    // core cannot hang the caller: two seconds of wall time is the ceiling.
    while started.elapsed() < std::time::Duration::from_millis(60) {
        core.render(&mut buffer);
        rendered_frames += chunk_frames as u64;
        if started.elapsed() > std::time::Duration::from_secs(2) {
            break;
        }
    }
    let audio_seconds = rendered_frames as f64 / f64::from(rate);
    let wall = started.elapsed().as_secs_f64().max(1e-9);
    Some((audio_seconds / wall) as f32)
}

/// The probe cores the machine measurement renders: one die sim and one fast
/// model, deliberately different in character, each at its chip's usual
/// clock. Both are OPL2-generation rows so every build that has the GPL
/// providers has them.
#[cfg(not(target_arch = "wasm32"))]
const PROBES: &[(&str, vgms_core::vgm::ChipKind, u32)] = &[
    (
        "opl3.ym3812-lle",
        vgms_core::vgm::ChipKind::Ym3812,
        3_579_545,
    ),
    (
        "opl3.opl2-lite",
        vgms_core::vgm::ChipKind::Ym3812,
        3_579_545,
    ),
];

/// Measures this machine against the baseline: renders the probe cores,
/// divides each by its baseline, and geometric-means the ratios. `None` when
/// no probe core is registered (a build without the GPL providers).
///
/// Takes about a second of one core -- the die-sim probe renders an eighth of
/// a second of audio below realtime. Call off the UI thread.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn measure_machine_ratio() -> Option<f32> {
    let registry = crate::registry::registry();
    let mut product = 1.0f64;
    let mut count = 0u32;
    for &(id, chip, clock) in PROBES {
        let Some(info) = registry.find(chip, id) else {
            continue;
        };
        let Some(baseline) = baseline_speed(id) else {
            continue;
        };
        let Some(measured) = measure_speed(info, clock) else {
            continue;
        };
        product *= f64::from(measured / baseline);
        count += 1;
    }
    (count > 0).then(|| (product.powf(1.0 / f64::from(count))) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ratio scales every baseline; unmeasured means "assume the
    /// reference machine", not "no answer".
    #[test]
    fn the_ratio_scales_the_baselines() {
        // Not a shared-state test hazard: this module's other tests do not
        // read the global, and the value is restored at the end.
        set_machine_ratio(None);
        let base = baseline_speed("opl3.ym3812-lle").expect("tracked");
        assert_eq!(effective_speed("opl3.ym3812-lle"), Some(base));

        set_machine_ratio(Some(2.0));
        assert_eq!(effective_speed("opl3.ym3812-lle"), Some(base * 2.0));
        assert_eq!(
            effective_speed("sn76489.libvgm"),
            None,
            "an untracked core stays untracked at any ratio"
        );

        set_machine_ratio(Some(-1.0));
        assert_eq!(
            machine_ratio(),
            None,
            "a nonsense ratio is refused, not installed"
        );
        set_machine_ratio(None);
    }

    /// Every baseline id spells a real registry id shape -- `<slot>.<name>`.
    #[test]
    fn every_baseline_id_is_slot_qualified() {
        for (id, speed) in BASELINE {
            assert!(id.contains('.'), "{id} is missing its slot prefix");
            assert!(speed.is_finite() && *speed > 0.0, "{id}: {speed}");
        }
    }
}
