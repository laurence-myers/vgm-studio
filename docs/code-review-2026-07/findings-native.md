# Findings: dro-trimmer native shell reviewer (returned complete)

### [native-1] Case-only track renames are refused on case-insensitive filesystems (Windows/macOS)
- Severity: Medium | Category: Bug | Confidence: High
- Location: crates/dro-trimmer/src/services/file.rs:170-180 (caller guard at crates/dro-ui/src/app.rs:1438-1441)
- Evidence: `rename_in_place`: `let dest = parent.join(to_name); if dest == from { return Ok(()); } if dest.exists() { return Err(format!("{to_name} already exists")); }`. `Path` equality is case-sensitive, so `01 intro.vgm` → `01 Intro.vgm` fails the same-file short-circuit; on NTFS/APFS `dest.exists()` resolves to the file itself → refused with "already exists". UI guard (`app.rs:1438 if new_name != old_name`) also case-sensitive, so the request reaches the service. Fixing capitalisation is a core rip-mode quick-edit op (VGMRips naming); maintainer's platform is Windows. On Windows `fs::rename` onto a case-variant succeeds and updates case — the refusal is purely the pre-check's false positive.
- Suggestion: treat "dest is the same file as from" as allowed (case-insensitive file_name compare, or canonicalize both) before the exists() refusal, then fall through to fs::rename.

### [native-2] preview_track error paths leave `audio_revision` stale — editor Play breaks, or resumes the preview song
- Severity: Medium | Category: Bug | Confidence: High (traced; manifests only when audio load/play fails, e.g. no output device)
- Location: crates/dro-ui/src/app.rs:1350-1365, interacting with crates/dro-trimmer/src/services/audio.rs:73-82; fix belongs app-side
- Evidence: `NativeAudioService::load` unloads first, assigns only on success (`self.unload(); self.audio = Some(NativeAudio::new(song, config)...?);`) — failed load leaves service cleanly empty. But `preview_track` returns early on error BEFORE line 1365's `self.audio_revision = None;`. `ensure_audio` (app.rs:1688) short-circuits when `audio_revision == Some(editor.revision())`, so: (a) after load failure, every later editor Play short-circuits then fails "No song is loaded into the audio output." — permanently until an edit bumps the revision; (b) after play failure, the service holds the preview track while the revision claims the editor song → later editor Play plays the preview song under the editor UI — exactly the wrong-song scenario the comment at app.rs:1363-1364 says that line prevents. Also `rip.preview` is never set on the play-failure path, so `stop_preview` won't clean up.
- Suggestion: invalidate `audio_revision` unconditionally as soon as `preview_track` calls `self.audio.load(...)` — any load, successful or failed, destroys the editor's snapshot in the service.

### [native-3] ThreadTaskService: global generation counter contradicts its per-kind design
- Severity: Low (correct today; a trap for the first added TaskKind) | Category: Simplify | Confidence: High
- Location: crates/dro-trimmer/src/services/task.rs:33-39, 131-135, 158-167; crates/dro-ui/src/tasks.rs:19-21
- Evidence: full per-kind machinery (`pending: HashMap<TaskKind, Pending>`, `running: HashMap<TaskKind, Arc<AtomicBool>>`, `cancel(kind)`) but staleness judged by ONE service-wide counter (`self.generation += 1` on any submit; poll keeps only `generation == latest`). With two kinds in flight, kind A's uncancelled results would be silently discarded. Correct only because TaskKind has exactly one variant — which also makes the HashMaps over-general (single `Option<Pending>` + `Option<Arc<AtomicBool>>`, the shape NativeRipService uses, would express reality).
  Race check (focus 3): submit(debounce) → submit(None) cannot land a stale render — checks on BOTH sides (worker checks cancel flag before each send task.rs:99-102; poll filters by generation; superseded debounced submission never spawns). `stale_results_are_dropped_by_generation` covers emitted-before-cancel.
- Suggestion: per-kind generation (stored with the running entry) or collapse maps to single-slot fields.

### [native-4] Three CLI bins duplicate the read-file → derive-name → parse-song preamble
- Severity: Low | Category: Duplication | Confidence: High
- Location: crates/dro-trimmer/src/bin/dro_player.rs:49-56; bin/dro_split.rs:33-40; bin/dro2to1.rs:26-29 + 59-64
- Evidence: dro_player/dro_split byte-identical (`std::fs::read` + `.file_name().and_then(|s| s.to_str()).unwrap_or("input.dro")` + `read_song(name, &bytes)?`); dro2to1 repeats via private `file_name()` helper. lib.rs ("Shared logic for the native binaries") owns none of it.
- Suggestion: one `read_song_from_path(&Path) -> anyhow::Result<Song>` in lib.rs used by all three.

### [native-5] dro_split clones every rendered WAV buffer before writing it
- Severity: Low | Category: Simplify (needless allocation) | Confidence: High
- Location: crates/dro-trimmer/src/bin/dro_split.rs:57-61
- Evidence: `SplitData::Wav(bytes) => bytes.clone()` exists only to unify match-arm types; a multi-minute 48 kHz stereo render is tens of MB per channel, duplicated once per output. (clippy redundant_clone doesn't fire — genuinely converts `&Vec<u8>` to `Vec<u8>`.)
- Suggestion: fs::write inside each arm (borrow for Wav, owned for Dro), or consume outputs by value.

### [native-6] Worker-thread plumbing repeated across three spawn sites
- Severity: Low | Category: Duplication | Confidence: High (existence; folding is judgment — parallel partly documented)
- Location: crates/dro-trimmer/src/services/task.rs:86-110; services/rip.rs:74-106; services/rip.rs:138-159
- Evidence: all three repeat clone sender/live/notify + `live.fetch_add` + `thread::spawn` + guarded send + notify + `live.fetch_sub`. rip.rs header documents the shape-sharing as deliberate; per-site variation (multi-emit vs single outcome vs no cancel flag) is real.
- Suggestion: optional `spawn_worker` helper owning live-counter/send/notify choreography; otherwise leave as documented parallel.

#### Checked and fine:
- FileService FIFO save contract: every path through save() pushes exactly one outcome (InPlace → write_outcome; Dialog+picked → write_outcome; Dialog+dismissed → Cancelled; file.rs:77-97); poll_saved pops VecDeque front oldest-first; rfd dialogs block so outcomes enqueue in submission order; ordering unit-tested (`saves_deliver_one_outcome_each_in_order` file.rs:282-303). Single-slot picked/folder/renamed match "most recent" trait docs.
- Failed AudioService::load leaves native service fully unloaded (audio.rs:76-81) — clean contract; the defect is app-side (native-2).
- No duplicated engine setup: NativeAudio::new single construction path; f32/i16 split shares one generic build_stream (dro-audio-native lib.rs:104-124, 234-295); deferred-while-paused seek/mute flush implemented as documented.
- oxipng settings exist once: png_options() (rip_zip.rs:108-113) shared by zip job + single-image optimize; differing failure semantics each right for context.
- NativeRipService::is_busy counting optimise threads with export jobs is deliberate (generic "Working..." label, app.rs:294-297).
- Rip staleness guarded both sides (worker cancel-check before send rip.rs:95-99; poll keeps latest generation rip.rs:117-126).
- CLI bins duplicate no render/convert logic: dro_player → render_wav_boosted; dro2to1 → dro2_to_dro1; dro_split → dro_trimmer::split → render_wav_muted/capture; split channel isolation = same Muting gate as GUI soloing.
- split/build_rip_zip single production consumers = stated pure-logic/IO-shell pattern (rip_zip.rs:2-3), both unit-tested.
- drotrim.rs wiring only; CLI file arg queued through open_path so failures reuse the app's error box; viewport built once, no duplication with dro-ui.
- IniConfigStore::save exe-dir-first/cwd-fallback mirrors documented load precedence (services/config.rs:35-52 vs config.rs:15-29).
- split.rs percussion handling matches comments (per-bank isolation, 0xE0|mask preservation, documented Python high-bank fix).
- tests/rip_flow.rs: genuine happy path (scan → prefill → zip → reopen → reparse → playlist). NOT exercised: FIFO save contract beyond back-to-back InPlace unit test (Dialog/Cancelled interleavings untestable headlessly with rfd), rename flow, save/audio failure paths, NativeRipService-level cancellation, gzip_vgms:false. Failure paths live in unit tests or nowhere.
- dro_player append_extension (song.dro.wav) + total_delay_with_write_delay_ms are documented Python-parity choices.
