//! Which register a failed parity gate is actually about (D-orw-4, at write
//! granularity).
//!
//! The gate says "this file plays differently after the built-in touched it".
//! This says *why*: it takes the writes the optimiser would drop, groups them by
//! the register they land on, and removes one group at a time -- so a group whose
//! removal alone changes the render names a rule that is wrong.
//!
//! Point it at a file the gate named:
//!
//!   $env:VGMSTUDIO_BLAME_FILE = 'F:/.../08 Seashore.vgz'
//!   cargo test -p vgms-app --release --test optimizer_blame -- --ignored --nocapture

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use vgms_core::vgm::VgmFile;
use vgms_core::vgm::stream::VgmCommand;
use vgms_synth::registry::{CoreChoices, with_render_choices};
use vgms_synth::vgm_engine::VgmEngine;

const OUTPUT_RATE: u32 = 44_100;
const FRAMES: usize = 44_100 * 8;

/// The same immediate-write cores the gate uses -- otherwise every class comes
/// back guilty, because a write-paced core hears the write *count*. See
/// `optimizer_investigation::gate_cores`.
fn blame_cores() -> CoreChoices {
    CoreChoices::from([
        ("ym2612".to_owned(), "libvgm".to_owned()),
        ("ym2151".to_owned(), "libvgm".to_owned()),
        ("ym2413".to_owned(), "libvgm".to_owned()),
        ("sn76489".to_owned(), "libvgm".to_owned()),
    ])
}

fn render(file: &VgmFile) -> Vec<i16> {
    with_render_choices(Some(blame_cores()), || {
        let mut engine = VgmEngine::new(Arc::new(file.clone()), OUTPUT_RATE);
        engine.set_immediate_writes(true);
        let mut out = vec![0i16; FRAMES * 2];
        let mut done = 0usize;
        while done < FRAMES {
            let rendered = engine.render(&mut out[done * 2..]);
            if rendered == 0 {
                break;
            }
            done += rendered;
        }
        out.truncate(done * 2);
        out
    })
}

fn difference(a: &[i16], b: &[i16]) -> Option<(usize, i32)> {
    let mut first = None;
    let mut peak = 0i32;
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let d = (i32::from(*x) - i32::from(*y)).abs();
        if d != 0 && first.is_none() {
            first = Some(i);
        }
        peak = peak.max(d);
    }
    if a.len() != b.len() && first.is_none() {
        first = Some(a.len().min(b.len()));
    }
    first.map(|i| (i, peak))
}

#[test]
#[ignore = "diagnostic, needs VGMSTUDIO_BLAME_FILE; run explicitly"]
fn which_register_class_changes_the_audio() {
    let path = PathBuf::from(
        std::env::var_os("VGMSTUDIO_BLAME_FILE").expect("VGMSTUDIO_BLAME_FILE must name a VGM"),
    );
    vgms_app::install_cores();
    let raw = std::fs::read(&path).expect("the file reads");
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let file = vgms_core::vgm::file::read(&name, &raw).expect("it parses");

    let stream = file.stream().expect("a command stream");
    let dropped = vgms_core::redundancy::redundant_indices(stream, file.loop_index());
    println!("{name}: {} writes would be dropped", dropped.len());

    // Group the drops by the register they land on, so the report names a rule
    // rather than an offset.
    let mut classes: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for &index in &dropped {
        if let Some(VgmCommand::Write { target, addr, data }) = stream.get(index) {
            classes
                .entry(class_of(
                    target.kind,
                    target.instance,
                    target.port,
                    addr,
                    data,
                ))
                .or_default()
                .push(index);
        }
    }
    println!("{} register classes", classes.len());

    let original = render(&file);
    let mut guilty = 0usize;
    for (class, indices) in &classes {
        let mut work = file.clone();
        work.delete_commands(indices);
        let verdict = difference(&original, &render(&work));
        if let Some((first, peak)) = verdict {
            guilty += 1;
            println!(
                "  GUILTY {class}: {} drop(s) -> differs at sample {first}, peak {peak}",
                indices.len()
            );
            // Narrow to one write: the smallest prefix of the class whose
            // removal already changes the render ends at the culprit.
            let mut low = 1usize;
            let mut high = indices.len();
            while low < high {
                let mid = low + (high - low) / 2;
                let mut probe = file.clone();
                probe.delete_commands(&indices[..mid]);
                if difference(&original, &render(&probe)).is_some() {
                    high = mid;
                } else {
                    low = mid + 1;
                }
            }
            let culprit = indices[low - 1];
            println!("    first bad drop is command {culprit}; context:");
            let stream = file.stream().expect("a command stream");
            for index in culprit.saturating_sub(6)..(culprit + 4).min(stream.len()) {
                let mark = if index == culprit { ">>" } else { "  " };
                println!("    {mark} {index}: {}", stream.describe(index));
            }
        }
    }
    println!("{guilty} of {} classes change the audio", classes.len());
}

/// What to call the register a write lands on.
///
/// The SN76489 carries its register in the *data*, so grouping it by address
/// would put every write in one bucket and name nothing.
fn class_of(kind: vgms_core::ChipKind, instance: u8, port: u8, addr: u16, data: u16) -> String {
    let where_ = format!("{} #{instance} p{port}", kind.name());
    if kind == vgms_core::ChipKind::Sn76489 && addr == 0 {
        return if data & 0x80 != 0 {
            format!("{where_} latch r{}", (data >> 4) & 0x07)
        } else {
            format!("{where_} continuation")
        };
    }
    format!("{where_} {addr:#06X}")
}
