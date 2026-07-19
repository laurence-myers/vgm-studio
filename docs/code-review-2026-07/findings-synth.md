# Findings: dro-synth + dro-audio-native + worklet/web reviewer (returned complete)

### [synth-1] Live mute/pan control writes can be overtaken by song writes still queued in the chip's write buffer
- Severity: Medium | Category: Bug | Confidence: Medium
- Location: crates/dro-synth/src/engine.rs:309-321 (set_muting), 337-349 (set_panning), 505-511 (playback path); vendor/nuked-opl3/src/core.rs:501-523, 932-943 (read for interface only)
- Evidence: playback writes use write_reg_buffered (timestamped lasttime + WRITEBUF_DELAY = 2 chip samples; drains one timestamp per generated sample). A burst of N writes needs ~2N samples to drain; if the cpal buffer fills right after a burst, entries persist across the callback return. Next callback pops UI commands first (dro-audio-native lib.rs:251-260); set_muting/set_panning apply via immediate write_reg — landing BEFORE older queued song writes. Consequences: (a) a queued 0xB0..=0xB8 key-on drains after set_muting's key-off; every later write to that register is gated → note rings stuck until unmute/seek, defeating the "keys it off immediately" contract (engine.rs:304-308). (b) a song 0xD0 write queued while Original passes the gate (only dropped at execute time while engaged, engine.rs:490-492), then drains after set_panning(Custom)'s panpots — the 9aadc07 clobber resurfacing through the queue window.
- Suggestion: route control-path register writes through the same buffered channel as playback writes (ordering holds by construction), or flush the chip queue before immediate control writes. Offline paths set muting before render start → golden bytes unaffected.

### [synth-2] The cpal callback allocates (scratch Vec) on its first run, contradicting the stated lock-free contract
- Severity: Medium | Category: Bug | Confidence: High
- Location: crates/dro-audio-native/src/lib.rs:247, 262-265 (module doc line 8)
- Evidence: `let mut scratch: Vec<i16> = Vec::new();` captured by the callback closure; inside: `if scratch.len() < frames * 2 { scratch.resize(frames * 2, 0); }`. First callback always allocates; later larger buffers reallocate. Module doc: "Nothing locks in the audio path." — malloc may take a lock.
- Suggestion: pre-size at stream build (device buffer-size range or generous max), keep in-callback resize as never-in-practice fallback.

### [synth-3] Seek replays the whole song prefix inside the audio callback — unbounded real-time work
- Severity: Low | Category: Bug | Confidence: Medium
- Location: crates/dro-audio-native/src/lib.rs:253-254 → crates/dro-synth/src/engine.rs:434-446
- Evidence: Command::SeekMs/SeekPos run seek_to_pos, whose `for i in 0..index` loop performs one chip write per instruction synchronously in the callback. DROs small; VGMs can carry hundreds of thousands of writes → a late seek can exceed the callback deadline and glitch. In-callback execution verified; actual overrun song-size dependent (unmeasured).
- Suggestion: nothing needed for DROs; if large VGMs matter, bound per-callback replay (incremental seek).

### [synth-4] Dead public API: PlayerEngine::sample_rate / muting / panning have zero callers
- Severity: Low | Category: Simplify | Confidence: High
- Location: crates/dro-synth/src/engine.rs:293-296, 299-302, 324-327
- Evidence: whole-crates greps match only cpal's default_config.sample_rate(), NativeAudio's own field, and ChannelPanel::{muting,panning} in dro-ui. No production or test code calls the three PlayerEngine getters; service/UI layers track their own copies.
- Suggestion: drop the three getters; trivially re-added when the worklet needs them.

### [synth-5] render_waveform_cancellable is an exported middle layer with no external consumer
- Severity: Low | Category: Simplify | Confidence: High
- Location: crates/dro-synth/src/waveform.rs:153-171 (export lib.rs:25); callers waveform.rs:146 + own tests only
- Evidence: GUI task calls render_waveform_progressive directly (dro-ui/src/tasks.rs:93-100); batch render_waveform serves theme_showcase + tests. The cancel-and-return-Option shape has no consumer — exists only so render_waveform can reuse the progressive loop. Related: private `total_output_frames<B: Borrow<Song>>` (227) has exactly one caller passing &Song (198) — unused genericity.
- Suggestion: fold wrapper into render_waveform (or demote cancellable to private); make total_output_frames take &Song.

### [synth-6] The elapsed-ms derivation is duplicated between the engine and the native position poll
- Severity: Low | Category: Duplication | Confidence: High
- Location: crates/dro-synth/src/engine.rs:386-389; crates/dro-audio-native/src/lib.rs:192-200
- Evidence: same formula (`frames * 1000 / rate`, u32 try_from, unwrap_or(MAX)) in two crates; a rounding change in one silently desyncs the other.
- Suggestion: one shared constructor (e.g. Position::from_frames) used by both.

### [synth-7] WaveformBucketer::push doc says extra samples are "ignored", but they fold into the last bucket
- Severity: Low | Category: Bug (doc/behavior) | Confidence: High
- Location: crates/dro-synth/src/waveform.rs:61-62 vs 68-71, 87-89
- Evidence: doc: "Extra samples beyond total_frames are ignored." Code: bucket index clamped (.min(num_buckets-1)) and min/max still update — overflow frames alter the final bucket. Overflow does occur (total_output_frames' chip-write-delay estimate documented approximate, 246-249).
- Suggestion: make doc and behaviour agree.

### [synth-8] tests/common re-implements the exported FrameClock verbatim
- Severity: Low | Category: Duplication | Confidence: High
- Location: crates/dro-synth/tests/common/mod.rs:104-126 vs crates/dro-synth/src/engine.rs:46-79 (exported lib.rs:18)
- Evidence: identical algorithm + near-identical doc comment. FrameClock::new(rate, 1000) covers the test copy exactly. Oracle-independence weak defence: golden_opl.rs pins total frame count with its own arithmetic regardless.
- Suggestion: use exported FrameClock in tests/common — or one line declaring the copy a deliberately independent oracle.

### [synth-9] Muting's doc attributes soloing to the CLI player, which deliberately does not have it
- Severity: Low | Category: Bug (doc) | Confidence: High
- Location: crates/dro-synth/src/engine.rs:81-82 vs crates/dro-trimmer/src/bin/dro_player.rs:7-10, crates/dro-ui/src/widgets/channels.rs:3-5
- Evidence: engine doc: "for dro_split's channel isolation and the CLI player's soloing." dro_player.rs documents soloing NOT ported (home is the GUI); channels.rs confirms.
- Suggestion: point the doc at the GUI channel panel.

#### Checked and fine:
- PlayerEngine genericity census: B has two live instantiations (&Song across wav/waveform/panning/engine_render; Arc<Song> in cpal callback); C has three impls (NukedOpl3; CReferenceOpl3 feature-gated parity oracle driven directly in c_parity.rs; RecordingChip test mock enabling muting/panning/seek write-order assertions). Both axes earn their keep; with_chip's only callers are in-crate tests (documented purpose).
- Muting/panning single-sourcing: exactly one Muting::gate (engine.rs:150) shared by playback (501) and capture (capture.rs:196). Pan law lives solely in vendored chip's stereo-ext panpots; engine routes opaque pan bytes; waveform renders Original-only; capture re-emits raw writes — no second pan-law implementation.
- Waveform layering: batch/cancellable/progressive collapse onto one loop + one WaveformBucketer; progressive snapshots verified prefix-stable by test.
- WAV logic: header/finalise only in wav.rs via hound; bins write returned bytes; grep confirms no duplication.
- BoostLimiter: two consumers (render_wav_impl, cpal callback); all methods used; dro-synth placement for worklet reuse documented.
- Load-failure trace: NativeAudioService::load unloads first (deliberate — two streams would overlap), failed NativeAudio::new leaves audio=None/playing=false/pending_seek=None; later play() errors cleanly. No half-initialised state.
- Defer/flush ordering: rtrb FIFO; service keeps only latest deferred seek (correct — seeks are absolute replays), re-flushes muting/panning/boost on every play seek-first; live-pushed seek stranded by immediate pause still drains before newer pending seek on next play. Overflow drops warn-logged; paused-defer keeps queue near-empty; 64 slots ample.
- Position/peaks atomics: frames_rendered + next_instruction minimal; finished not UI-derivable; peaks fetch_max/swap(0) so transients survive. Only synth-6's formula duplication.
- OplChip surface: all four trait methods have callers; default immediate impl serves CReferenceOpl3.
- Chip reset (*self = Self::default()) clears the write queue → seek (reset + replay) cannot leak stale buffered writes; synth-1 applies only to live control changes without a reset.
- Numeric edges sound: FrameClock integer carry; chip-write-delay f64 carry; capture delay re-encoding sums exactly; limiter threshold/bypass/stereo-link/saturating cast; 8-bit top-byte; bucketer empty/zero/short-song padding; channel_peaks i16::MIN unsigned_abs; render() tail zeroing.
- render_wav_muted's B: Borrow<Song> only sees &Song today (split.rs:128) — documented Arc affordance unexercised but zero-cost.
- dro-synth-worklet + dro-web: doc-comment-only lib.rs, dependency-free manifests — sound zero-weight placeholders for plan steps 8/9.
