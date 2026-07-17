# Vendored `nuked-opl3` 0.1.0 (dro-trimmer local patch)

This is an unmodified copy of `nuked-opl3` 0.1.0 from crates.io **except** for two
one-line defect fixes in `src/core.rs`, applied so the workspace can enable the
crate's off-by-default `stereo-ext` feature (per-channel constant-power panning
via register `0xD0+ch`) without changing the rendered output of any song that is
not actively being panned.

It is wired in through `[patch.crates-io]` in the workspace root `Cargo.toml`; the
version and public API are identical to upstream 0.1.0.

## The two patches (both `src/core.rs`)

1. **`pan_from_channel_mask` sign bug.** Upstream returns
   `((mask as u32) << 16) as i32`, which for an enabled gate (`mask == 0xFFFF`) is
   `-65536`, not `+0x10000`. Because the front mix is unconditionally
   `(accm * pan) >> 16` once `stereo-ext` is compiled, this polarity-inverts every
   channel that has received a `0xC0` write — even when stereo-ext is disengaged at
   runtime (`0x105` bit 1 clear) — and mixed polarity cancels unison voices. Patched
   to return the intended unity pan `0x10000` (and `0` for a cleared gate), so the
   disengaged path is `accm * 0x10000 >> 16 == accm`, bit-identical to the original
   `masked_accm(accm, 0xFFFF)`.

2. **`CHANNEL_SAMPLE_DELAY` feature coupling.** Upstream defines
   `const CHANNEL_SAMPLE_DELAY: bool = !cfg!(feature = "stereo-ext")`, so merely
   *compiling* the feature drops the 4-channel sample-delay quirk and changes
   `generate_4ch`'s slot/mix interleaving for every song. Pinned to `true`
   unconditionally so enabling the feature does not alter timing.

Together these make a `stereo-ext` build with stereoext disengaged at runtime
byte-for-byte identical to a stock (feature-off) build. `dro-synth`'s
`golden_opl` hash and `c_parity` suites are the regression gate for that property.

Both fixes are upstream-PR material; drop this vendor directory and the
`[patch.crates-io]` entry once a release carrying them is on crates.io.
