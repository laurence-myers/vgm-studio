# Licensing

vgm-studio is **not** licensed as one blob. Two halves, on purpose:

| Part | Crates | License |
|---|---|---|
| **The reusable library pair** | `vgms-core`, `vgms-synth` | `MIT OR Apache-2.0` |
| **The application** | `vgms-app`, `vgms-ui`, `vgms-audio-native`, `vgms-retrowave`, `vgms-web`, `vgms-synth-worklet` | `GPL-2.0-or-later` |

**The distributed program — the `vgmstudio` binary and the web build — is
`GPL-2.0-or-later`.** That is the license that reaches a user who downloads a
release, and it is what the About dialog says.

## Why the split

The app links whatever core sounds most like the real chip, and the best
emulator for a given chip is often GPL-2 or LGPL-2.1 (the Nuked family, the
LLE cores, ESFMu). Accuracy wins, so the app is GPL-2.0-or-later and is free
to link them.

`vgms-core` (the VGM/DRO file model) and `vgms-synth` (the playback engine plus
its own permissively-sourced cores) are the half worth reusing. They stay
`MIT OR Apache-2.0` so that anyone wanting a clearly-licensed VGM library —
the thing libvgm is not — can take them without inheriting a copyleft
obligation. Their code is clean-room or ported from MIT/BSD/ISC/zlib sources
with the upstream notices retained; `crates/vgms-synth/PROVENANCE.md` has the
per-core record.

**Copyleft cores never live in `vgms-synth`.** They live in separate provider
crates that the *app* depends on, so the permissive half stays permissive:

| Provider crate | License | Holds |
|---|---|---|
| `vgms-cores-nuked` | `LGPL-2.1-or-later` | Nuked-family cores (CQM, OPN2, OPM, …) |
| `vgms-cores-gpl` | `GPL-2.0-or-later` | GPL-2 cores (OPLL, PSG, the LLE tier) |

`vgms-synth`'s one copyleft dependency, the `nuked-opl3` OPL core, is behind a
**default-on `nuked-opl` feature**. `cargo build -p vgms-synth
--no-default-features` therefore has no copyleft in it at all: OPL files still
load, edit, seek, split and render — that is file-format logic, not emulation —
they just produce silence, and the core registry says so rather than implying
sound. The application enables the feature, as does every release build.

No GPL-3-only code ships in any binary here (that rules out Mesen2 and
BlastEm, which are used as separate A/B oracle *programs*, never linked), and
neither does code carrying a non-commercial clause — a further restriction the
GPL does not permit.

## The texts

| File | Applies to |
|---|---|
| [`LICENSE-GPL-2.0.txt`](LICENSE-GPL-2.0.txt) | the app crates and the distributed binary (also copied to `LICENSE` at the repo root) |
| [`LICENSE-LGPL-2.1.txt`](LICENSE-LGPL-2.1.txt) | `vendor/nuked-opl3` and the `vgms-cores-nuked` provider |
| [`LICENSE-MIT.txt`](LICENSE-MIT.txt) | `vgms-core`, `vgms-synth` (at your option) |
| [`LICENSE-APACHE-2.0.txt`](LICENSE-APACHE-2.0.txt) | `vgms-core`, `vgms-synth` (at your option) |

Third-party notices that are not chip cores — the `serialport` crate (MPL-2.0)
behind RetroWave output, the Px437 IBM VGA font trace, and the vendored
`browser_wasi_shim` (MIT OR Apache-2.0, `web/wasi-shim/`) that hosts the
optimiser modules in the web build — are carried in the About dialog and beside
the assets they cover.

## Contributing

A contribution to `vgms-core` or `vgms-synth` is taken as dual-licensed `MIT OR
Apache-2.0`; a contribution to any other crate as `GPL-2.0-or-later`. Ported
code must keep its upstream notice verbatim and gain a row in
`crates/vgms-synth/PROVENANCE.md`.
