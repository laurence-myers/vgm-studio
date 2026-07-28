//! An ad-hoc harness: renders one file through the generic engine and
//! reports its level. Point `SCRATCH_FILE` at a `.vgm`/`.vgz` and run with
//! `--nocapture`; without the variable the test is a no-op, so the suite
//! stays green everywhere.
use std::sync::Arc;

#[test]
fn corpus_file_renders_sound() {
    let Ok(path) = std::env::var("SCRATCH_FILE") else {
        eprintln!("SCRATCH_FILE not set; skipping");
        return;
    };
    let bytes = std::fs::read(&path).expect("read");
    let file = dro_core::vgm::file::read("scratch.vgz", &bytes).expect("parse");
    let mut engine = dro_synth::vgm_engine::VgmEngine::with_cores(Arc::new(file), 44_100, |kind| {
        dro_synth::registry::registry().build(kind, None)
    });
    let mut samples = Vec::new();
    let mut buffer = vec![0i16; 4096 * 2];
    while samples.len() < 44_100 * 20 * 2 {
        let rendered = engine.render(&mut buffer);
        if rendered == 0 {
            break;
        }
        samples.extend_from_slice(&buffer[..rendered * 2]);
    }
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    let rms = (samples
        .iter()
        .map(|&s| f64::from(s) * f64::from(s))
        .sum::<f64>()
        / samples.len().max(1) as f64)
        .sqrt();
    println!("frames={} peak={peak} rms={rms:.1}", samples.len() / 2);
    assert!(peak > 500, "the file must sound: peak {peak}");
}
