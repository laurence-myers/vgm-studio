//! Headless GUI tests: drive the fully rendered `VgmStudioApp` through egui_kittest
//! and assert on editor state and what the fake platform services were asked to
//! do. Mounted as a child module of `app`, so it can read `VgmStudioApp`'s private
//! fields (`editor`, `dialogs`, `alerts`, `status`) directly.
//!
//! Interaction tests use the default (LazyRenderer) harness -- no GPU. The
//! snapshot tests at the bottom need the wgpu renderer and compare against PNG
//! baselines under `tests/snapshots/`; generate/refresh them with
//! `UPDATE_SNAPSHOTS=1 cargo test -p vgms-ui`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use vgms_core::DroSong;
use vgms_core::config::{AppConfig, ThemeChoice};
use vgms_synth::LoopCount;

use super::VgmStudioApp;
use crate::action::{
    Action, AppTab, EditAction, FileAction, LoopAction, MixerAction, PackAction, PlaybackAction,
    UiAction,
};
use crate::pack::PackSection;
use crate::platform::{
    OptimizedImage, PackJobOutcome, PickedFile, PickedFolder, SaveOutcome, SaveRequest,
};
use crate::tasks::TaskKind;
use crate::test_song::{
    bogus_leading_delay_song, dro_song_v2, dual_tone_song, looping_vgm, looping_vgm_bytes,
    multi_song_capture, multi_song_capture_dro, paced_song, redundant_vgm_file, tone_song,
};
use crate::test_support::{
    AudioLog, FakeAudioService, FakeFileService, FakePackService, FileLog, InlineTaskService,
    MemoryConfigStore, NoopTaskService, PackLog, TaskLog,
};

/// Shared handles onto the fake services, for scripting and inspection.
struct Handles {
    files: Rc<RefCell<FileLog>>,
    audio: Rc<RefCell<AudioLog>>,
    tasks: Rc<RefCell<TaskLog>>,
    pack: Rc<RefCell<PackLog>>,
    saved_configs: Rc<RefCell<Vec<AppConfig>>>,
}

/// Serialise a fixture song back to bytes and wrap it as a picked file, exactly
/// as the editor's own unit tests do. The path gives Save somewhere to land.
fn picked(song: &DroSong) -> PickedFile {
    PickedFile {
        name: song.name.clone(),
        path: Some(PathBuf::from(format!("C:/songs/{}", song.name))),
        bytes: vgms_core::io::write_song(song).unwrap(),
    }
}

/// The same, for a document held as a VGM ([`vgms_core::VgmFile`]).
fn picked_vgm(file: &vgms_core::VgmFile) -> PickedFile {
    PickedFile {
        name: file.name.clone(),
        path: Some(PathBuf::from(format!("C:/songs/{}", file.name))),
        bytes: vgms_core::vgm::file::write(file).unwrap(),
    }
}

/// Build a harness around a `VgmStudioApp` wired to fresh fakes.
///
/// - `inline_tasks`: run the waveform render synchronously (so it has pixels)
///   instead of dropping it on the floor.
/// - `wgpu`: use the wgpu renderer, required for snapshots.
fn build(
    initial: Option<PickedFile>,
    inline_tasks: bool,
    wgpu: bool,
) -> (Harness<'static, VgmStudioApp>, Handles) {
    // Tall enough for the five stacked panels plus table rows.
    build_sized(initial, inline_tasks, wgpu, egui::vec2(1000.0, 720.0))
}

fn build_sized(
    initial: Option<PickedFile>,
    inline_tasks: bool,
    wgpu: bool,
    size: egui::Vec2,
) -> (Harness<'static, VgmStudioApp>, Handles) {
    // Every GUI test sees the app-shaped registry: since the cull the
    // builtins alone have no generic core, so playability, the transport and
    // the render menu would all answer differently without it.
    crate::widgets::chip_output::install_test_cores();
    let files = Rc::new(RefCell::new(FileLog::default()));
    let audio = Rc::new(RefCell::new(AudioLog::default()));
    let tasks = Rc::new(RefCell::new(TaskLog::default()));
    let pack = Rc::new(RefCell::new(PackLog::default()));
    let saved_configs = Rc::new(RefCell::new(Vec::new()));

    let handles = Handles {
        files: files.clone(),
        audio: audio.clone(),
        tasks: tasks.clone(),
        pack: pack.clone(),
        saved_configs: saved_configs.clone(),
    };

    let app_builder = move |cc: &mut eframe::CreationContext<'_>| {
        // Match vgmstudio.rs startup: the embedded DOS font and feathering-off are
        // what make layout and snapshots deterministic.
        crate::theme::install(&cc.egui_ctx, ThemeChoice::default());
        VgmStudioApp::new(
            Box::new(FakeFileService(files)),
            Box::new(FakeAudioService(audio)),
            if inline_tasks {
                Box::new(InlineTaskService::new(tasks))
            } else {
                Box::new(NoopTaskService(tasks))
            },
            Box::new(FakePackService(pack)),
            Box::new(MemoryConfigStore {
                initial: AppConfig::default(),
                saved: saved_configs,
            }),
            initial,
        )
    };

    // `max_steps` well above the default 4 gives settling room; playback tests
    // still avoid `run`.
    let builder = Harness::builder().with_size(size).with_max_steps(64);
    let mut harness = if wgpu {
        // One shared DX12/WARP device for every snapshot test, in place of
        // kittest's per-test device -- see `crate::test_gpu`.
        builder
            .wgpu_setup(crate::test_gpu::shared_wgpu_setup())
            .build_eframe(app_builder)
    } else {
        builder.build_eframe(app_builder)
    };
    harness.run();
    // The chip deck ships folded -- asserted here so every test guards that
    // default -- then opened, because the mixer tests drive its widgets. The
    // fold itself is covered by
    // `interaction::the_chip_deck_folds_behind_its_disclosure`.
    assert!(
        !harness.state().chips_expanded,
        "the chip deck ships folded"
    );
    harness.state_mut().chips_expanded = true;
    // Twice: the bottom panel's height follows its content with a one-frame
    // lag, and tests click by (possibly stale) accessibility rects.
    harness.run();
    harness.run();
    (harness, handles)
}

/// The common case: interaction harness, no song loaded.
fn empty_harness() -> (Harness<'static, VgmStudioApp>, Handles) {
    build(None, false, false)
}

/// Interaction harness with `song` already loaded (via the first-frame open).
fn harness_with_song(song: &DroSong) -> (Harness<'static, VgmStudioApp>, Handles) {
    build(Some(picked(song)), false, false)
}

fn harness_with_vgm(file: &vgms_core::VgmFile) -> (Harness<'static, VgmStudioApp>, Handles) {
    build(Some(picked_vgm(file)), false, false)
}

// -- shared helpers promoted from the section files (st-6) -------------------

/// The VGM fixture with its header volume modifier set to `modifier`. Patches the
/// Volume Modifier header byte (spec offset 0x7C) directly, since a VGM document
/// is a [`vgms_core::VgmFile`].
fn vgm_with_modifier(modifier: u8) -> vgms_core::VgmFile {
    let mut bytes = VGM_FIXTURE.to_vec();
    bytes[0x7C] = modifier;
    vgms_core::vgm::file::read("m.vgm", &bytes).unwrap()
}

/// Opens File > Render to WAV..., which needs the menu to be walked.
fn open_render_wav_dialog(harness: &mut Harness<'static, VgmStudioApp>) {
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label_contains("Render to WAV").click();
    harness.run();
}

/// Opens File > Split Channels...
fn open_split_dialog(harness: &mut Harness<'static, VgmStudioApp>) {
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label_contains("Split Channels").click();
    harness.run();
}

/// Opens File > Split Songs...
fn open_split_songs_dialog(harness: &mut Harness<'static, VgmStudioApp>) {
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label_contains("Split Songs").click();
    harness.run();
}

/// Settle the pointer out of the frame, then snapshot. Since 0.34, kittest
/// paints a synthetic mouse cursor whenever a pointer position is live (e.g.
/// after a click), which would bake a cursor triangle and hover state into
/// the baselines.
fn settled_snapshot(harness: &mut Harness<'static, VgmStudioApp>, name: &str) {
    harness.remove_cursor();
    harness.run();
    // Cap concurrent GPU renders on the shared device; the permit is handed back
    // when this scope ends, even if the snapshot comparison panics on a mismatch.
    let _permit = crate::test_gpu::render_permit();
    harness.snapshot(name);
}

/// The Neo Geo file, tagged, with a stream that walks cleanly. Its two waits
/// sum to 10735 samples, and the loop covers both.
fn other_chip_vgm_file() -> PickedFile {
    let bytes = other_chip_vgm_bytes(
        &[
            0x58, 0x28, 0xF0, // YM2610 port 0
            0x61, 0x10, 0x27, // wait 10000
            0xA0, 0x07, 0x38, // AY8910
            0x62, // wait 735
            0x66, // end
        ],
        10_735,
        10_735,
    );
    let mut file = vgms_core::vgm::file::read("03 Psycho Soldier.vgm", &bytes).unwrap();
    file.tag = Some(vgms_core::Gd3Tag {
        track_name_en: "Psycho Soldier".to_owned(),
        game_name_en: "Athena".to_owned(),
        ..vgms_core::Gd3Tag::default()
    });
    PickedFile {
        name: "03 Psycho Soldier.vgm".to_owned(),
        path: Some(PathBuf::from("C:/rips/Athena/03 Psycho Soldier.vgm")),
        bytes: vgms_core::vgm::file::write(&file).unwrap(),
    }
}

/// Drags the widget centred at `from` by `delta`, in the three steps the
/// harness needs (press, move, release).
fn drag_by(harness: &mut Harness<'static, VgmStudioApp>, from: egui::Pos2, delta: egui::Vec2) {
    harness.drag_at(from);
    harness.run();
    harness.hover_at(from + delta);
    harness.run();
    harness.drop_at(from + delta);
    harness.run();
}

/// A Master System rip: one chip, and one this app has a core for.
fn sms_vgm_file() -> PickedFile {
    use vgms_core::ChipKind;
    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let stream: &[u8] = &[
        0x50, 0x8E, 0x50, 0x0F, // tone 0, period 254
        0x50, 0x90, // full volume
        0x61, 0x44, 0xAC, // a second
        0x66,
    ];
    let mut bytes = vec![0u8; 0x100];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, 0x08, 0x171);
    put_u32(&mut bytes, 0x34, 0x100 - 0x34);
    put_u32(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
    put_u32(&mut bytes, 0x18, 44_100);
    bytes.extend_from_slice(stream);
    let eof = bytes.len();
    put_u32(&mut bytes, 0x04, (eof - 4) as u32);

    PickedFile {
        name: "01 Bios.vgm".to_owned(),
        path: Some(PathBuf::from("C:/rips/SMS/01 Bios.vgm")),
        bytes,
    }
}

const VGM_FIXTURE: &[u8] = include_bytes!("../../../../tests/lsl3_score_up.vgm");

/// Drives one action through the app the way the frame loop would.
fn act(harness: &mut Harness<'static, VgmStudioApp>, action: Action) {
    let ctx = harness.ctx.clone();
    harness.state_mut().handle_action(&ctx, action);
}

/// Opens a pack sub-section. Each section draws only its own widgets, so a test
/// that clicks one has to be looking at the right tab first.
///
/// Run **twice**: egui hit-tests a click against the widget rects registered on
/// the *previous* frame, so a control that appears for the first time on the
/// frame the section switches is not clickable until the frame after. (A real
/// user cannot hit that window; a synthetic click landing the same millisecond
/// can.)
fn pack_section(harness: &mut Harness<'static, VgmStudioApp>, section: PackSection) {
    act(harness, Action::Pack(PackAction::SelectSection(section)));
    harness.run();
    harness.run();
}

/// A Neo Geo VGM: a v1.61 header, a loop and a tag, and a `stream` of commands
/// the OPL table cannot size, so the editor is certain to decline it as a song.
///
/// **Its chips must have no core**, because the tests built on it assert the
/// not-playable and not-renderable paths. The assertion below makes a future
/// core break *this* fixture by name rather than the tests that never mention it.
///
/// `total` and `loop_samples` go in the header verbatim; a real file's agree
/// with its stream, and so do the ones passed here.
fn other_chip_vgm_bytes(stream: &[u8], total: u32, loop_samples: u32) -> Vec<u8> {
    use vgms_core::ChipKind;

    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    assert!(
        !vgms_synth::registry::registry().can_build(ChipKind::Ym2610),
        "the YM2610 now has a core, so this fixture no longer stands for a \
         document nothing can play -- pick a chip that still has none"
    );

    let mut bytes = vec![0u8; 0x100];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, 0x08, 0x161);
    put_u32(&mut bytes, 0x34, 0x100 - 0x34);
    put_u32(&mut bytes, ChipKind::Ym2610.clock_offset(), 8_000_000);
    put_u32(&mut bytes, 0x18, total);
    put_u32(&mut bytes, 0x1C, (0x100 + 3 - 0x1C) as u32); // loop at command 1
    put_u32(&mut bytes, 0x20, loop_samples);
    bytes.extend_from_slice(stream);
    let eof = bytes.len();
    put_u32(&mut bytes, 0x04, (eof - 4) as u32);
    bytes
}

// -- the tests, one child module per section -----------------------------
mod channel_panning;
mod find_loop;
mod interaction;
mod loop_points;
mod menu_items;
mod optimize;
mod pack_mode;
mod render_wav;
mod snapshot;
mod split_channels;
mod split_songs;
