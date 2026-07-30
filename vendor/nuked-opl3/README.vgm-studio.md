# Vendored `nuked-opl3` 0.1.0 (vgms-app local patch)

This is a copy of `nuked-opl3` 0.1.0 from crates.io with **two one-line defect
fixes** and **one deliberate pan-law change** in `src/core.rs`, applied so the
workspace can enable the crate's off-by-default `stereo-ext` feature (per-channel
panning via register `0xD0+ch`) without changing the rendered output of any song
that is not actively being panned.

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

Together these two fixes make a `stereo-ext` build with stereoext disengaged at
runtime byte-for-byte identical to a stock (feature-off) build. `vgms-synth`'s
`golden_opl` hash and `c_parity` suites are the regression gate for that property.
Both are upstream-PR material.

## The pan-law change (`src/core.rs`, `panpot`)

Upstream's `panpot` is a constant-power law (`sin(v*PI/512)*65536`), so a centred
pan sits ~3 dB down per side. Since a disengaged/OPL2 channel plays both speakers
at unity, toggling Custom panning on would audibly drop every centred channel.
`panpot` is changed to a linear **balance** law -- the active side holds unity from
the centre outward, only the opposite side attenuates -- so a centred Custom pan
matches the song's original level. This is a deliberate product choice, **not** a
bug fix, and only affects the engaged path (`panpot` is never called while
stereoext is disengaged, so the byte-identity property above is unaffected).

Drop this vendor directory and the `[patch.crates-io]` entry once a release
carrying the two fixes is on crates.io, and reapply the pan-law change (or move it
into `vgms-synth`) if it is still wanted then.
