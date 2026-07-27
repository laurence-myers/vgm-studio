# Licensing

dro-trimmer is **not** licensed as one blob. Two halves, on purpose:

| Part | Crates | License |
|---|---|---|
| **The reusable library pair** | `dro-core`, `dro-synth` | `MIT OR Apache-2.0` |
| **The application** | `dro-trimmer`, `dro-ui`, `dro-audio-native`, `dro-retrowave`, `dro-web`, `dro-synth-worklet` | `GPL-2.0-or-later` |

**The distributed program — the `drotrim` binary and the web build — is
`GPL-2.0-or-later`.** That is the license that reaches a user who downloads a
release, and it is what the About dialog says.

## Why the split

The app links whatever core sounds most like the real chip, and the best
emulator for a given chip is often GPL-2 or LGPL-2.1 (the Nuked family, the
LLE cores, ESFMu). Accuracy wins, so the app is GPL-2.0-or-later and is free
to link them.

`dro-core` (the VGM/DRO file model) and `dro-synth` (the playback engine plus
its own permissively-sourced cores) are the half worth reusing. They stay
`MIT OR Apache-2.0` so that anyone wanting a clearly-licensed VGM library —
the thing libvgm is not — can take them without inheriting a copyleft
obligation. Their code is clean-room or ported from MIT/BSD/ISC/zlib sources
with the upstream notices retained; `crates/dro-synth/PROVENANCE.md` has the
per-core record.

**Copyleft cores never live in `dro-synth`.** They live in separate provider
crates that the *app* depends on, so the permissive half stays permissive:

| Provider crate | License | Holds |
|---|---|---|
| `dro-cores-nuked` | `LGPL-2.1-or-later` | Nuked-family cores (CQM, OPN2, OPM, …) |
| `dro-cores-gpl` | `GPL-2.0-or-later` | GPL-2 cores (OPLL, PSG, the LLE tier) |

> **Caveat, true until step cr-2 lands.** `dro-synth` currently depends on
> `nuked-opl3` (LGPL-2.1-or-later) unconditionally, so a build of it today is
> LGPL-2.1-or-later in effect even though its own source is `MIT OR
> Apache-2.0`. cr-2 makes that dependency an optional, default-on `nuked-opl`
> feature; `--no-default-features` will then give a genuinely permissive
> build. Until then, treat `dro-synth`'s permissive claim as covering its own
> source only.

No GPL-3-only code ships in any binary here (that rules out Mesen2 and
BlastEm, which are used as separate A/B oracle *programs*, never linked), and
neither does code carrying a non-commercial clause — a further restriction the
GPL does not permit.

## The texts

| File | Applies to |
|---|---|
| [`LICENSE-GPL-2.0.txt`](LICENSE-GPL-2.0.txt) | the app crates and the distributed binary (also copied to `LICENSE` at the repo root) |
| [`LICENSE-LGPL-2.1.txt`](LICENSE-LGPL-2.1.txt) | `vendor/nuked-opl3` and the `dro-cores-nuked` provider |
| [`LICENSE-MIT.txt`](LICENSE-MIT.txt) | `dro-core`, `dro-synth` (at your option) |
| [`LICENSE-APACHE-2.0.txt`](LICENSE-APACHE-2.0.txt) | `dro-core`, `dro-synth` (at your option) |

Third-party notices that are not chip cores — the `serialport` crate (MPL-2.0)
behind RetroWave output, and the Px437 IBM VGA font trace — are carried in the
About dialog and beside the assets they cover.

## Contributing

A contribution to `dro-core` or `dro-synth` is taken as dual-licensed `MIT OR
Apache-2.0`; a contribution to any other crate as `GPL-2.0-or-later`. Ported
code must keep its upstream notice verbatim and gain a row in
`crates/dro-synth/PROVENANCE.md`.
