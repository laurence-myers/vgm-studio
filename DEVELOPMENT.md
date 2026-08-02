## Rust rewrite (in progress)

The `rust` branch is porting VGM Studio to Rust; the Python sources under `src/`
stay put during the transition, for parity comparison. Both suites run.

The workspace is licensed in **two halves**, and which half a file is in decides
what may be copied into it:

| Crates | License |
|---|---|
| `vgms-core`, `vgms-synth` — the reusable file model and playback engine | `MIT OR Apache-2.0` |
| everything else — the application | `GPL-2.0-or-later` |

The **distributed program is GPL-2.0-or-later**: the app links whatever chip
core sounds most like the real hardware, and the best of those are GPL-2 or
LGPL-2.1. `vgms-core` and `vgms-synth` stay permissive so they are reusable on
their own, which means **nothing copyleft may be added to them** — their cores
are clean-room or ported from MIT/BSD/ISC/zlib sources. Copyleft cores go in
provider crates the app depends on.

`licenses/README.md` has the full split and the four license texts;
`crates/vgms-synth/PROVENANCE.md` records where every core came from and under
what terms, and a new core adds its row in the same commit.

Install the toolchain (Windows; MSVC build tools are already a pre-requisite below):

```PowerShell
scoop install main/rustup
```

`rust-toolchain.toml` pins the channel, components and the `wasm32-unknown-unknown`
target, so the first `cargo` command installs whatever is missing.

Some emulator cores are upstream C, consumed as **git submodules pinned to a
commit** and compiled as they stand (`vendor/upstream/`, built by
`crates/vgms-cores-nuked`). A fresh clone needs them:

```PowerShell
git submodule update --init --recursive
```

The build fails with that instruction rather than a missing-file error if it is
skipped. They compile with clang — including to `wasm32-unknown-unknown`, which
is why these cores reach the web build. Never edit anything under
`vendor/upstream/`: upgrading is `git -C vendor/upstream/<x> pull` plus a pin
bump, and that only stays true while there is nothing local to merge against.
Whatever the build needs and an upstream does not provide goes in that crate's
`shim/`. See `crates/vgms-synth/PROVENANCE.md`.

```PowerShell
cargo test --workspace                                       # unit + integration tests
cargo fmt --all                                              # format
cargo clippy --workspace --all-targets -- -D warnings        # lint
cargo check --target wasm32-unknown-unknown -p vgms-core -p vgms-synth   # wasm stays clean
```

### The `vgmstudio` command line

The workspace builds **one executable**. Run it with no arguments (or with just a
file) for the GUI; run it with a subcommand for what used to be the `dro_player`,
`dro_split` and `dro2to1` binaries:

```PowerShell
cargo run -p vgms-app -- help                    # list the subcommands
cargo run -p vgms-app -- play song.dro           # play through the speakers
cargo run -p vgms-app -- render song.dro         # write song.dro.wav
cargo run -p vgms-app -- split song.dro          # one WAV per channel used
cargo run -p vgms-app -- split --song song.vgm   # one VGM per channel instead
```

DRO v2 -> v1 conversion (the old `dro2to1`) is GUI-only now, under Edit >
Convert to DRO v1.

Two things to know about the release build, which is linked as a *GUI-subsystem*
executable so double-clicking it does not flash a console window:

- An interactive shell does not wait for a GUI-subsystem process, so the prompt
  comes back immediately and the subcommand's output interleaves with it. Piping
  (`vgmstudio help | more`) and `cmd`-style redirection (`cmd /c "vgmstudio help >
  out.txt"`) both capture it, but **PowerShell's `>` writes an empty file** —
  PowerShell has already moved on by the time the process prints. Debug builds
  are console-subsystem and behave like any other console program, so this only
  affects a release build run by hand.
- A file whose name is exactly `play`, `render`, `split`, `convert` or `help`
  parses as a subcommand. Open it as `vgmstudio .\play`.

Run the file-format round trips as real wasm, under Node. The CLI version must
match the `wasm-bindgen` version in `Cargo.lock`:

```PowerShell
cargo install wasm-bindgen-cli --version 0.2.126 --locked
cargo test -p vgms-core --target wasm32-unknown-unknown
```

`vgms-ui` has headless GUI tests (`egui_kittest`). The interaction tests run on
CPU; the visual-regression tests compare against PNG baselines under
`crates/vgms-ui/tests/snapshots/` rendered via wgpu, so they are machine- and
GPU-specific. Regenerate them after an intentional UI change:

```PowerShell
$env:UPDATE_SNAPSHOTS='1'; cargo test -p vgms-ui; Remove-Item Env:\UPDATE_SNAPSHOTS
```

Pack mode's zip export pulls native-only crates: `zip`, `oxipng` (which builds
the C `libdeflate` via `cc`) and `chrono`. The MSVC build tools below already
satisfy the C toolchain; `vgms-core`/`vgms-synth` stay free of them and wasm-clean.

Optional: prove the pure-Rust OPL core is still bit-identical to Nuked-OPL3's C
sources. Needs a C compiler and libclang (`scoop install main/llvm`):

```PowerShell
cargo test -p vgms-synth --features c-parity
```

### Continuous integration and releases

CI is `.github/workflows/rust.yaml`: fmt, clippy and tests on Windows, plus a
wasm-cleanliness job, the OPL C-parity gate, and the Playwright web e2e suite.
The old Python `check.yaml` and `build.yaml` workflows were removed with the
port.

There is **deliberately no release workflow yet.** Automated releases are wanted
but deferred; when added it is a tag-triggered job running `cargo build
--release` on `windows-latest` plus `tools/build-web.ps1`, uploading both the
executable and the web bundle. The gap is intentional, not rot.

## Pre-requisites

On Windows, Microsoft Visual C++ 14.0 or greater is required.
Get it with "Microsoft C++ Build Tools": https://visualstudio.microsoft.com/visual-cpp-build-tools/

Once installed, "Modify" it, go to Individual Components, select only:

- Windows SDK (latest)
- C++ x64/x86 build tools (latest)

## Setup

Install Python v3.13. On Windows, I used Scoop in a Powershell terminal.

```PowerShell
scoop install python313
```

In IntelliJ, go to Project Structure -> SDKs, add a new Python (virtualenv).

In Project, select the new SDK.

Install normal dependencies:

```PowerShell
py -m pip install -r requirements.txt
```

Install dev dependencies:

```PowerShell
py -m pip install -r requirements_dev.txt
```

Set up Black for auto-formatting: [here's how to set it up in IntelliJ or PyCharm.](https://www.jetbrains.com/help/pycharm/2023.2/reformat-and-rearrange-code.html#format-python-code-with-black)

Set up Git to ignore bulk change commits (like auto-formatting) when running "blame":

```PowerShell
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

## Run

```PowerShell
cargo run -p vgms-app
```

## RetroWave OPL3 hardware output

Playback can go to a RetroWave OPL3 board — a real YMF262 heard through its own
3.5mm jack — instead of the emulator. Pick it in Settings > Output, or from the
command line. Both board generations work; an original board's mode switch must
be set to USB.

Check that the board is found and makes sound:

```PowerShell
vgmstudio retrowave-probe
```

That lists every serial port with its USB descriptors, then plays a chord on each
register bank of the first board it recognises. Add `--list` to stop after the
listing, or `--port COM3` to choose one. On Windows a board reports the generic
name "USB Serial Device", so it is recognised by USB ID (`04d8:e966`) instead.

Play a song through it:

```PowerShell
vgmstudio play song.dro --retrowave
```

Note the protocol is write-only: the host cannot tell whether the board understood
anything. If a change to the wire format goes wrong, the failure is silence, not an
error — so listen, do not just check the exit code. The design and its reasoning are
in `docs/retrowave-2026-07/PLAN.md`.

## Build .exe

```PowerShell
cd src
python setup.py
```

## Format code

```PowerShell
black src/ tests/
```

## Type-check code

```PowerShell
mypy src/
mypy tests/
```

## Run tests

```Powershell
python -m unittest discover --start-directory tests/
```
