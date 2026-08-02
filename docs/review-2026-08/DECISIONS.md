# Review remediation — decisions needed from the owner

Date: 2026-08-02. Status: **OPEN — nothing here is settled.** Branch: `review-2026-08`.

Companion to [PLAN.md](PLAN.md). Every step in that plan is either something a
competent implementer can just do, or it is blocked on one of the questions
below. Nothing on this page has been guessed at in the plan.

Each entry gives the question, the concrete options, and a recommendation. If
you agree with every recommendation, the shortest possible answer is *"take the
recommendations"* and the whole programme unblocks. Where a recommendation
would change bytes users ship, or change a legal posture, that is called out —
those are the ones worth reading properly.

The list is ordered by **when it blocks**, not by importance.

---

## Answer before anything can be committed

### D1 — What ceiling does gunzip get, and where is the amplification bounded?

`.vgz` decompression has no size limit, on either of the two byte-identical
readers (`vgm/io.rs:80`, `vgm/file.rs:698`). A ~1 MB file of compressed zeros
expands to ~1 GB, and on the `vgm::file` path the command index multiplies that
by roughly twelve (4 bytes in `offsets` plus 8 in `wait_prefix` per one-byte
`0x00` command). On wasm32 that is a guaranteed abort of the whole module.

- **(a)** One conservative cap shared by both targets.
- **(b)** A per-target cap under `cfg(target_pointer_width)`.
- **(c)** A generous byte cap **plus** a separate bound on the index build in
  `VgmStream::parse`, which is where the 12× actually lands.

**Recommendation: (c), with the byte cap identical on both targets (256 MiB).**
Option (b) manufactures a file that opens natively and fails in the browser,
which is precisely the divergence `crates/vgms-core/tests/wasm_roundtrip.rs`
exists to prevent. The cap is a *refusal*, so it is the one number in Stage A a
conservative guess is not free on — a legitimate oversized rip becomes
unopenable.

*Blocks: hf-3 (Stage A).*

### D2 — How far does loop-end preservation reach?

Saving after almost any edit silently widens a deliberately-short loop
(`vgm/file.rs:674`). The fix's scope is a real choice:

- **(a)** The delete path only — ~20 lines, covers the reported repro.
- **(b)** Thread a loop end through `rebuild` and all three region edits.
- **(c)** Preserve the declared `loop_samples` count rather than a row index —
  no signature changes, but a preserved count can miss a command boundary and
  the next save widens it back anyway.

**Recommendation: (b) minus `optimize`.** Fix delete, crop and delete-region;
let the optimiser widen the loop and say so in its doc comment. Giving
`merge_stream_delays` a second merge barrier changes optimiser output bytes and
can flip its "nothing to do" guard — a bad trade for a rare short loop end.

*Blocks: sw-1 (Stage C).*

### D3 — The `0x64` wait-override divergence

`wait_prefix` ignores the `0x64` wait override that the playback engine
honours, so the timeline, cursor, seeks and the audit's total-samples check all
disagree with what actually plays.

- **(A)** Honour the override in `VgmStream::parse`. Changes `total_samples`,
  makes `audit()` newly report files that are currently clean, and drags the
  `Song`/`VgmData` mirror along with it.
- **(B)** Make the engine ignore the override and document the divergence at
  `command_wait`. A five-line deletion; everything then agrees, and we
  knowingly differ from VGMPlay on a command almost no rip uses.

**Recommendation: (B).** Model coherence is worth more here than fidelity to
`0x64`. (A) also forces sw-1, sw-2, sw-3 and mg-4 to be rewritten against a
moved baseline.

Either way, a separate bug gets fixed regardless: the engine currently matches
on the *value* 735/882, so a literal `0x61 DF 02` wait is wrongly re-mapped by
an active override.

*Blocks: sw-6 (Stage C), and constrains mg-4 (Stage I).*

### D11 — The web-dist copy manifest

`tools/build-web.ps1:61` copies all of `web/` into the servable dist, dragging
27 MB of Playwright `node_modules` and stale `test-results` with it. The CI job
at `rust.yaml:202` has its own drifted copy of the same list, and **that copy
omits `web/wasi-shim/` entirely** — which is why the export specs would be red
the first time that job runs, for reasons unrelated to any fix here.

- **(a)** Allowlist in each place (explicit and auditable; silently drops future
  additions — the failure already live in CI).
- **(b)** Denylist (`-Exclude e2e`; unreliable with `-Recurse`).
- **(c)** One shared manifest consumed by both the build script and the CI job.

**Recommendation: (c).** They are already two drifted copies of one list, and
that drift is the bug. Whichever wins, `web/wasi-shim/` must travel as a
*directory*, carrying its `LICENSE-MIT` and `LICENSE-APACHE`.

*Blocks: ci-3 (Stage B), which in turn blocks most of Stage D's test hooks.*

### (c) — `build.yaml` and the absent release workflow

`check.yaml` runs on every push and fails on every push (it `pip install`s a
requirements file deleted in `5e9ece7`). `build.yaml` is the same dead Python
pipeline, escaping notice only because it is `workflow_dispatch`. Deleting both
is uncontroversial — but it makes explicit that **the repo has no working
release workflow at all**.

**Recommendation: delete both now** (Stage B), and treat "do you want automated
releases?" as the real question. If yes, a tag-triggered `release.yaml` running
`cargo build --release` on windows-latest plus `tools/build-web.ps1`, uploading
both artefacts, is about half a day. If no, say so in `DEVELOPMENT.md` so the
gap reads as deliberate rather than as rot.

*Blocks: nothing — but answer it while you are in that file.*

---

## Answer before the middle of the programme

### D4 — Vendored WASI shim: fork it or wrap it?

**Correction to the review first:** the review said the shim's debug logger
defaults to on. More precisely — `web/wasi-shim/debug.js` constructs the
singleton **disabled**; it is the `WASI` *constructor* that turns it on when the
`debug` option is absent, and `enable()` never updates `isEnabled`, so
`debug.enabled` is permanently stale. The exposure is real; the fix site is not
where the review said.

- **(a)** Patch the vendored file in place — the next upgrade silently reverts it.
- **(b)** Keep the vendor byte-identical; add `web/wasi-host.js` owning argv, fds
  and `debug: false`.
- **(c)** No code change; a grep-level test asserting every `new WASI(` passes
  `debug: false`.

**Recommendation: (b).**

*Blocks: wb-6 (Stage D).*

### D5 — The parity render cache: is the warm corpus worth preserving?

Widening the cache key to include the pinned config, the extra args and the
player identity invalidates the entire existing `VGMSTUDIO_PARITY_CACHE` —
potentially hours of re-rendering across 72k files.

- **(a)** Widen the key and eat one cold rebuild.
- **(b)** A sidecar manifest recording what produced each WAV (keeps warm
  entries; more code).
- **(c)** Put the reference identity in the cache *directory* name.

**Recommendation: always re-copy the staged player (mechanical, land it now)
plus (c).** Only you know the state of that cache.

Note for whoever lands this: the project memory's claim that the cache is keyed
per `(file, rate, player, config)` is false, and should be corrected then.

*Blocks: sn-2 (Stage E).*

### D6 — Where does the shipped WASI host get gated?

**Unresolved factual conflict — settle this first.** Two reviewers disagreed
about whether any test exercises the shim today: one found the e2e pack fixture
is `lsl3_score_up.vgm` (YM2203, so the wholly-OPL bypass does *not* apply and
the tools really run); the other found the fixture is OPL-only, so
`__vgms_run_tool` is never reached. **Check which is true before choosing** —
it decides whether sn-6 is "strengthen an existing gate" or "build the first
one."

- **(a)** Browser e2e — needs a committed non-OPL fixture, a rewritten
  `tests/e2e-pack.zip`, and the wasi-sdk cache plus pwsh build step imported
  into the `web-e2e` job, roughly doubling its setup.
- **(b)** Node — point `tools/web/vgmtools_smoke.mjs` at
  `web/wasi-shim/index.js`. Near-zero cost, never proves the browser wiring.

**Recommendation: (b) now, (a) as a tracked follow-up.**

*Blocks: sn-6 (Stage E).*

### D7 — May the permissive pair's public API be trimmed on in-tree evidence alone?

`vgms-core` and `vgms-synth` are `MIT OR Apache-2.0` and meant to be reusable,
so "no caller in this repo" is only "dead" if there is no downstream. Affected:
`TrackEntry::from_song`, `render_wav_muted(_with_progress)`, `Compression`,
`VgmFile::unoptimised_chips`, and the eight-entry `wav.rs` render surface.

**Recommendation: yes, delete.** Neither crate is published to crates.io, so
in-tree evidence is the whole story. Shrink `wav.rs` to the entry points that
have callers and document the survivors as the intended library API. **If you
intend to publish these crates, say so** — then they stay, with `#[deprecated]`
aliases instead.

*Blocks: dd-2 (Stage F), and subsumes named decision (f).*

### D12 — Spelling policy for identifiers and UI strings

The two spellings interleave *within one surface*: the Edit menu says "Optimize
VGM" while the undo entry beside it says "Undo Optimise shot.png".

- **(a)** US in identifiers **and** every user-visible string; British in
  comments, logs and docs.
- **(b)** British everywhere except Rust identifiers — which means the existing
  "Optimize VGM" menu item and `strings.rs`'s "Optimized" change instead.

**Recommendation: (a).** It matches what the menus already say, so it changes
the least user-visible text.

*Blocks: tm-1 (Stage G).*

### D13 — One corpus environment variable or two?

Four test suites read `VGMSTUDIO_CORPUS`; four read `VGMSTUDIO_VGMRIPS_CORPUS`.
Setting one silently skips half the suite.

**Recommendation: keep both, documented, plus a loud skip.** They are not quite
the same thing — the second names the VGMRips tree the chip index is built
from, and collapsing them silently repoints the chip-selected suites at the
wrong tree. Document both (and `VGMSTUDIO_CORPUS_LIMIT`) in `DEVELOPMENT.md`,
and make a required-but-unset corpus **fail with the variable name** instead of
`eprintln`-and-return. Say explicitly whether the two `vgms-vgmtools` suites
come along — they cannot see `vgms-app`, so they stay split unless you accept a
duplicated fallback.

*Blocks: tm-4 (Stage G), and is a prerequisite for named decision (b).*

### D14 — Where does the unified pack-zip builder live?

`vgms-pack-archive` cannot depend on `vgms-ui` (cycle), and `PackEntry` lives in
`vgms-ui`.

- **(1)** Move `PackEntry`/`PackEntryKind` down into `vgms-pack-archive` and
  re-export from `vgms-ui`.
- **(2)** A portable entry type in the archive crate with `From` impls at both
  call sites — one extra copy of every entry's bytes per export, and the
  multi-megabyte case is exactly the one that matters.
- **(3)** Leave the builder in `vgms-web` and have `vgms-app` depend on it.

**Recommendation: (1).** Also settle the null-optimizer wording: the shared
builder should take `Option<&dyn ImageOptimizer>` and let the web supply a null
optimizer that logs its own browser-specific line, so no browser sentence lands
in a target-independent crate.

*Blocks: fk-1 (Stage H).*

### D15 — Which crate hosts `register_common_cores`?

- **(A)** A new GPL-2.0-or-later `vgms-cores` crate that both `vgms-app` and
  `vgms-synth-worklet` depend on.
- **(B)** Host it in `vgms-synth-worklet` and have `vgms-app` depend on that.
- **(C)** Leave both copies; add a cross-checking test.

**Recommendation: (A)**, despite the three hand-maintained lists a new crate
must join (`[workspace.dependencies]`, `licenses/README.md`, `rust.yaml`'s
wasm-check line). (B) makes the desktop binary depend on the AudioWorklet
crate, which exports `#[unsafe(no_mangle)] extern "C"` symbols and overrides
`unsafe_code = "deny"`.

Whichever wins, the signature must be `fn register_common_cores(&mut
CoreRegistry)` — `install` is a process-global one-shot, so any comparison test
needs a registry it can build without installing.

*Blocks: fk-2 (Stage H).*

---

## Answer before the late stages

### D16 — Dialog registry shape

- **(a)** A `macro_rules!` listing emitting the struct, `any_open()` and
  `show_all()`, keeping named fields so all 68 call sites are untouched.
- **(b)** A trait-object registry with a `Scope` enum — genuinely removes the
  lockstep, rewrites all 68 sites.
- **(c)** No registry; just add sw-5's two missing lines.

**Recommendation: (a), with two independent tag columns**
(`closes_with_song`, `closes_with_pack`) and `goto` as a hand-written,
documented exception — it survives a song load but closes on a tab switch, so
it is neither. A single `song_bound()` flag would silently change dismissal
behaviour that no test currently covers.

*Blocks: st-1 (Stage J).*

### D17 — Dialog footer scaffold

The ten `Cell<bool>` sites are five different shapes, so the saving is smaller
than the review implied.

- **(a)** Generic footer: `footer: impl FnOnce(&mut Ui) -> T`, scaffold returns
  `(dismissed, T)`; each dialog declares a tiny button enum. **~40 lines saved,
  not 150.**
- **(b)** Scaffold owns Close plus one primary button and returns a
  `FooterClick`. Nine sites lose their button code, but `dro_info`'s label
  flipping and `find_loop`'s third button need escape hatches.
- **(c)** Leave the `Cell`s; extract only the repeated comment into the
  scaffold's rustdoc.

**Recommendation: (a)** — and if the appetite for this is low, **(c) is a
legitimate answer that costs nothing.**

*Blocks: st-2 (Stage J).*

### D18 — `app.rs` method visibility after the split

Rust privacy is descendant-only, so the ~90 methods that move into `app::*`
submodules become invisible to the 533-line `handle_action` that stays behind.

- **(i)** `pub(super)` on each — precise, but the diff stops reading as a pure
  move.
- **(ii)** `pub(crate)` — leaks 90 methods into the rest of `vgms-ui`.
- **(iii)** Split `handle_action` itself so each submodule owns its arms behind
  two or three entry points.

**Recommendation: (i) for the move, then (iii) as a separate follow-up commit.**
Doing (iii) inside the move turns a mechanical relocation into a redesign of the
dispatch, and this is already the riskiest step in the programme.

*Blocks: st-4 (Stage J).*

### D19 — Splitting a module in the permissive pair

Splitting `vgms-core::pack` changes import paths for 24 call sites.

- **(a)** Split with `pub use` back from `pack/mod.rs` — no break, but two
  importable paths per item.
- **(b)** `pub mod readiness;` with no re-export — clean tree, path break.
- **(c)** Don't split; the finding's stated harm is the stale module doc, so
  refresh the doc and leave 2,412 lines.

**Recommendation: (b) if D7 concludes the crates are unpublished**, else (a).
Also settle what `naming.rs` actually takes — the answer decides whether the
re-export list is 9 names or 15.

*Blocks: st-8 (Stage J).*

### D20 — Does the OPL reader keep its own acceptance rules?

Routing `vgm::io::read` through `vgm::file::read` is **not** behaviour-preserving
in four places: the v1.51 version floor, the "Unsupported VGM command"
rejection, the stream extent (`0x66` anywhere versus bounded by declared
EOF/GD3 — a latent bug *fix*), and two `log::warn` paths.

- **(A)** Pure delegation with widened acceptance; rewrite four rejection tests.
- **(B)** Delegation with the old gates re-imposed after `file::read`.
- **(C)** Skip `io.rs` entirely and repoint `io/mod.rs`.

**Recommendation: (B) first**, then widen deliberately in a separate change
with corpus evidence. `read_song` is public API, and the widening is **invisible
to the corpus gate** — newly-openable files are counted as a success, so (A)
would ship an unmeasured behaviour change through a test reporting green.

*Blocks: mg-1 (Stage I).*

### D21 — How does the Editor hold its `VgmFile`?

Today `ensure_audio` on a large Mega Drive rip deep-clones the whole indexed
stream on every edit-then-play, in six places.

- Ownership: **(i)** cache an `Arc<VgmFile>` rebuilt in `bump_revision`, exactly
  as `refresh_projection` already rebuilds the OPL projection; **(ii)**
  `Option<Arc<VgmFile>>` + `Arc::make_mut`; **(iii)** dedup the match arms only
  and keep cloning.
- Type: reuse `vgms_synth::AudioSource`, or a new `vgms_ui::DocSource` that
  `AudioSource` is built from.

**Recommendation: (i) plus a new `vgms_ui::DocSource`** — `SplitSource`'s
`rate`/`detect`/`can_preview` are UI-and-core knowledge that does not belong in
the permissive synth crate.

*Blocks: mg-5 (Stage I).*

### D22 — Retiring the OPL state fold from the split path

- **(A)** Route `SplitSource::Opl` to prefer `editor.vgm()` and narrow
  `state_patch` to DRO. **This changes the bytes users submit to VGMRips** —
  split pieces get a different prelude ordering and explicit zeros restored.
- **(B)** Teach the generic prelude the OPL fold's ordering and zero-skip so the
  two are byte-identical, then upgrade `compare_split` from state-equality to
  byte-equality.
- **(C)** Keep both stacks; fix the docs and mark `state_patch` DRO-only with a
  pointer to `chip_state`.

**Recommendation: (C) now, (B) as the real completion.** (A) is the
migration-completing answer, but it silently changes exported bytes for a
feature people use on real rips.

*Blocks: mg-6 (Stage I).*

---

## Standing questions, not tied to one step

### (a) — `vgms-cores-ymfm`: delete or freeze?

Zero dependents; no `register()`; its C++ submodule is compiled by every
`cargo test --workspace`; its lib.rs claims YM2608/YM2610/YMF278B/Y8950 coverage
while `build.rs` compiles only `ymfm_opn.cpp`. `CORES-REUSE-PLAN.md` ru-2
records a deliberate freeze — but nothing in the crate says so, which is why
two independent reviewers flagged it for deletion. Note there is no
`default-members` key today (`members = ["crates/*"]`), so "exclude it from
default-members" means *adding* one.

**Recommendation: delete the crate.** The decision is preserved in
`CORES-REUSE-PLAN.md` and in git. Keeping it costs workspace build time, a
`licenses/README.md` row, a wasm-check line, and a permissive-crate exception in
a provider table that exists to make a copyleft point.

**If you disagree:** keep it, add a "frozen PoC — see CORES-REUSE-PLAN.md ru-2"
note at the top of its `lib.rs`, drop the unused `vgms-core` dependency, and add
a `default-members` key excluding it.

### (b) — Scheduled CI for the 17 `#[ignore]`d parity and corpus suites?

They need a local 72k-file corpus and a licensed reference player, neither of
which can live on a GitHub-hosted runner — and every one of them currently
`eprintln`s and returns when its variable is unset, so a scheduled job would
report green having compared nothing.

**Recommendation: no scheduled job.** Instead: (i) land fixture-scale,
non-ignored versions of the cheap gates so `cargo test --workspace` covers the
*shape*; (ii) make a required-but-unset corpus fail loudly (D13); (iii) put a
`cargo test -- --ignored` pre-release checklist in `DEVELOPMENT.md`, carrying
the absolute-path warning (a relative `DROTRIM_REF_CONFIG` makes every row skip
silently). Add a self-hosted runner only if you want the corpus gated
continuously.

### (d) — Move `vgms_app::parity` + `::corpus` to a dev-only crate?

2,215 lines plus the `hound` dependency are `pub` in the crate that builds the
shipped GPL binary, consumed only by integration tests.

**Recommendation: yes — a new `vgms-parity` crate consumed as a
`dev-dependency`.** Sequence it **after** sn-1/sn-2 and after D8's `Regime`
deletion, so less code moves and content fixes travel with the `git mv` rather
than needing to be re-anchored onto a moved file.

### (e) — `editor.rs:153`: the `dro` slot's doc versus `Editor::load`'s fallback

The field says it holds only DROs; the load fallback can put a legacy-read VGM
there.

**Recommendation: keep the fallback and re-document.** It is the only rescue
path for a VGM that `vgm::file::read` rejects but the legacy OPL reader accepts
— refusing it turns a degraded open into a failed open. Rename the field to
`legacy_opl`, document both ways it fills, and add a test pinning what a save
does in that state (that is where an OPL-projected VGM can silently lose its VGM
identity). **Sequence after D20**, whose resolution may remove the fallback's
reason to exist.

### D8 — `Regime::CleanRoom` and the parity `Threshold` fields

- Delete just the variant (leaves a one-variant enum and a trivially-true
  guard), delete the whole regime concept, or keep it documented as reserved.

**Recommendation: delete the whole concept** — `Regime`, `Threshold::regime`,
`max_envelope`, the `shared()` const fn and the dead test branch. The clean-room
cores have no survivors, so the concept has no future referent.

### D9 — `licenses/README.md` and libvgm's absent licence grant

- **(a)** Add the libvgm row carrying its Cargo.toml caveat verbatim **and**
  caveat the claim that the distributed binary is GPL-2.0-or-later.
- **(b)** Add the row with the caveat; leave that claim alone.
- **(c)** Hold the docs fix until LIBVGM-PLAN lv-0 resolves.

**Recommendation: (b) now**, plus the two missing app-tier crates. **(a) is a
legal-posture change you should make knowingly, not as a side effect of a docs
sweep.**

### D10 — What is `TODO.md` for?

It currently holds four overlapping "Any-chip VGM support" entries, one of which
describes a world that no longer exists.

**Recommendation: treat it as a dated log** and add a superseded-by note
pointing at `docs/vgm-multichip-2026-07/`. Rewriting it in the past tense
falsifies a record; deleting it loses the history.

---

## Settled without asking

Recorded so the plan does not look like it skipped them. Each had one
defensible answer:

| Question | Answer taken |
|---|---|
| Where the shared gunzip helper lives | A new `vgm/gzip.rs`, not `io.rs` — it survives the later io.rs unification |
| `hf-1`'s `bits_in > 32` clause | Documentation only; `BitReader::read` already refuses `> 32`, so only `bits_out` can crash |
| `hf-7`'s fuzzing tool | No cargo-fuzz (needs nightly; the toolchain is pinned stable by policy). Check the hostile payloads in as a table-driven corpus instead |
| `sw-3`'s second call site | Fix both; the step was incomplete, not undecided |
| `sw-7`'s "shared Unicode helper" | Not achievable (`vgms-pack-archive` deliberately has no such dependency) and not needed — making the collision check unconditional removes the branch |
| `Compression` | Remove the re-export *and* privatise the enum; the half-fix leaves it importable by module path |
| `Device::port_name` | Delete accessor, field and the `with_io` parameter — an unused field fails `clippy -D warnings`, so the cheap option does not compile |
| `st-4`'s module layout | Non-`mod.rs` (`src/app.rs` head + `src/app/*.rs`). The `mod.rs` form breaks the `#[path]` gui-tests mount |
| `vgms-vgmtools`'s lint block | Delete it and inherit `[lints] workspace = true` — it exists solely to flip `unsafe_code` for an `ffi.rs` the crate does not have |
| `mg-7` | Rename, do not fold — folding needs a `VgmStream` where only a `Song` exists |
