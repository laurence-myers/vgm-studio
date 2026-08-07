//! rc-1: a per-render core override reaches the WAV render.
//!
//! [`with_render_choices`] scopes a one-shot [`CoreChoices`] pick to the current
//! thread; the OPL render's chip selection (`build_opl` + `DroEngine::with_chip`)
//! reads it, so a render dialog can export through a different OPL emulator than
//! the one Settings plays with, without disturbing playback. This needs the full
//! app registry (two OPL cores: the default `nuked`, and `cqm`), so it lives here
//! rather than in `vgms-synth`, whose own tests see only the built-in `nuked`.

use vgms_synth::registry::{CoreChoices, with_render_choices};

/// The OPL core's native rate, so no resampler enters the comparison.
const RATE: u32 = vgms_synth::NATIVE_SAMPLE_RATE;

/// A real single-chip OPL capture as a DRO song -- what the WAV render renders
/// for an OPL document through `DroEngine` (the per-render core override the
/// rc-1 feature scopes applies on this path).
fn opl_song() -> vgms_core::DroSong {
    let bytes = include_bytes!("../../../tests/lsl3_score_up_dro2.dro");
    vgms_core::io::read_song("lsl3_score_up_dro2.dro", bytes).expect("the DRO reads")
}

fn render(song: &vgms_core::DroSong, choices: Option<CoreChoices>) -> Vec<u8> {
    with_render_choices(choices, || {
        vgms_synth::render_dro_wav(song, RATE, 16).expect("the render succeeds")
    })
}

fn opl_choice(name: &str) -> CoreChoices {
    CoreChoices::from([("opl3".to_owned(), name.to_owned())])
}

/// A per-render OPL core override reaches the OPL render: naming the default core
/// changes nothing, naming a different emulator changes the samples, and no
/// override renders exactly the default -- proof the override is both honoured and
/// scoped.
#[test]
fn a_per_render_opl_core_override_changes_the_render() {
    vgms_app::install_cores();
    let song = opl_song();

    let default_render = render(&song, None);
    assert!(default_render.starts_with(b"RIFF"), "a WAV came back");

    // Choosing the default OPL core explicitly is byte-identical to no override.
    let nuked = render(&song, Some(opl_choice("nuked")));
    assert_eq!(
        nuked, default_render,
        "explicitly choosing the default OPL core renders identically"
    );

    // A different OPL emulator produces a different render of the same length.
    let cqm = render(&song, Some(opl_choice("cqm")));
    assert_eq!(
        cqm.len(),
        default_render.len(),
        "same song, same rate: same number of samples"
    );
    assert_ne!(
        cqm, default_render,
        "a different OPL core produces different samples"
    );

    // And the override did not leak: the next unwrapped render is the default
    // again.
    assert_eq!(
        render(&song, None),
        default_render,
        "the override is scoped to its render, not sticky"
    );
}
