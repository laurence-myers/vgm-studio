## Rust rewrite (in progress)

The `rust` branch is porting DRO Trimmer to Rust; the Python sources under `src/`
stay put during the transition, for parity comparison. Both suites run.

The project is licensed **LGPL-2.1-or-later**, because the OPL emulation core it
statically links (`nuked-opl3`, a port of Nuke.YKT's Nuked-OPL3) is. The full
licence text is in `LICENSE` at the repo root.

Install the toolchain (Windows; MSVC build tools are already a pre-requisite below):

```PowerShell
scoop install main/rustup
```

`rust-toolchain.toml` pins the channel, components and the `wasm32-unknown-unknown`
target, so the first `cargo` command installs whatever is missing.

```PowerShell
cargo test --workspace                                       # unit + integration tests
cargo fmt --all                                              # format
cargo clippy --workspace --all-targets -- -D warnings        # lint
cargo check --target wasm32-unknown-unknown -p dro-core -p dro-synth   # wasm stays clean
```

### The `drotrim` command line

The workspace builds **one executable**. Run it with no arguments (or with just a
file) for the GUI; run it with a subcommand for what used to be the `dro_player`,
`dro_split` and `dro2to1` binaries:

```PowerShell
cargo run -p dro-trimmer -- help                    # list the subcommands
cargo run -p dro-trimmer -- play song.dro           # play through the speakers
cargo run -p dro-trimmer -- render song.dro         # write song.dro.wav
cargo run -p dro-trimmer -- split song.dro          # one WAV per channel used
cargo run -p dro-trimmer -- split --song song.vgm   # one VGM per channel instead
```

DRO v2 -> v1 conversion (the old `dro2to1`) is GUI-only now, under Edit >
Convert to DRO v1.

Two things to know about the release build, which is linked as a *GUI-subsystem*
executable so double-clicking it does not flash a console window:

- An interactive shell does not wait for a GUI-subsystem process, so the prompt
  comes back immediately and the subcommand's output interleaves with it. Piping
  (`drotrim help | more`) and `cmd`-style redirection (`cmd /c "drotrim help >
  out.txt"`) both capture it, but **PowerShell's `>` writes an empty file** —
  PowerShell has already moved on by the time the process prints. Debug builds
  are console-subsystem and behave like any other console program, so this only
  affects a release build run by hand.
- A file whose name is exactly `play`, `render`, `split`, `convert` or `help`
  parses as a subcommand. Open it as `drotrim .\play`.

Run the file-format round trips as real wasm, under Node. The CLI version must
match the `wasm-bindgen` version in `Cargo.lock`:

```PowerShell
cargo install wasm-bindgen-cli --version 0.2.126 --locked
cargo test -p dro-core --target wasm32-unknown-unknown
```

`dro-ui` has headless GUI tests (`egui_kittest`). The interaction tests run on
CPU; the visual-regression tests compare against PNG baselines under
`crates/dro-ui/tests/snapshots/` rendered via wgpu, so they are machine- and
GPU-specific. Regenerate them after an intentional UI change:

```PowerShell
$env:UPDATE_SNAPSHOTS='1'; cargo test -p dro-ui; Remove-Item Env:\UPDATE_SNAPSHOTS
```

Pack mode's zip export pulls native-only crates: `zip`, `oxipng` (which builds
the C `libdeflate` via `cc`) and `chrono`. The MSVC build tools below already
satisfy the C toolchain; `dro-core`/`dro-synth` stay free of them and wasm-clean.

Optional: prove the pure-Rust OPL core is still bit-identical to Nuked-OPL3's C
sources. Needs a C compiler and libclang (`scoop install main/llvm`):

```PowerShell
cargo test -p dro-synth --features c-parity
```

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
python -m src.drotrimmer.drotrim 
```

## RetroWave OPL3 hardware output

Playback can go to a RetroWave OPL3 board — a real YMF262 heard through its own
3.5mm jack — instead of the emulator. Pick it in Settings > Output, or from the
command line. Both board generations work; an original board's mode switch must
be set to USB.

Check that the board is found and makes sound:

```PowerShell
drotrim retrowave-probe
```

That lists every serial port with its USB descriptors, then plays a chord on each
register bank of the first board it recognises. Add `--list` to stop after the
listing, or `--port COM3` to choose one. On Windows a board reports the generic
name "USB Serial Device", so it is recognised by USB ID (`04d8:e966`) instead.

Play a song through it:

```PowerShell
drotrim play song.dro --retrowave
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
