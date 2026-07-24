# RetroWave OPL3 hardware output — implementation plan

Status: **IMPLEMENTED** (2026-07-23). Planned, reviewed and built the same day; see
§8 for what the hardware taught us and where the code diverged from this plan.

Goal: let the user switch live playback from the Nuked OPL3 software emulator to a
**RetroWave OPL3** USB device — a real YMF262 with its own 3.5mm audio output. Both the
original RetroWave OPL3 (the user's device) and the newer RetroWave OPL3 Express must
work; they are 100% software-compatible with each other, so one backend covers both.

**Song eligibility:** hardware output applies to **DRO files** (always OPL-family) and
**VGM/VGZ files whose chip data is OPL2, dual-OPL2, or OPL3 only**. Dual-OPL2 *is*
supported: the board's single YMF262 hosts both OPL2s via its two register arrays,
exactly the mapping the emulator uses today (§3.5). Today this eligibility rule is
vacuously true — `dro-core` hard-errors on VGMs containing any non-OPL command — but it
becomes a real gate when the any-chip VGM plan
([docs/vgm-multichip-2026-07/HANDOVER.md](../vgm-multichip-2026-07/HANDOVER.md)) lands;
see §3.5 for the required seam.

Out of scope: render/split/convert/pack-mode (these need PCM and stay on Nuked), audio
*capture* from the device (sound leaves via its 3.5mm jack, not USB), non-OPL3
RetroWave boards (MiniBlaster, MasterGear), and any non-OPL chip data (above).

---

## 1. The device and its protocol (self-contained spec)

Everything needed to implement the wire format is written down here so the implementer
never needs to consult the reference C code (see §2 Licensing for why, and for the
provenance of each part of this section).

### 1.1 Transport

Both devices enumerate as a **USB CDC-ACM serial port** (`COMx` on Windows,
`/dev/ttyACM*` on Linux). Baud rate is irrelevant over CDC (the reference sets 115200 on
Windows and 2,000,000 on Linux; any value works) but must be set to something; use
115200 8N1, no flow control. The protocol is write-only: the host never reads from the
device.

*Our addition (not in the reference, which never touches DTR):* assert **DTR** after
opening. Some CDC stacks discard writes until DTR is set, and it is harmless otherwise.
If the device is silent during bring-up, do not treat DTR as a documented device
requirement — it is a defensive measure only.

Internally the device is an SPI bus bridged over serial: an MCP23S17-style 16-bit I/O
expander (SPI address `0x21`) drives the YMF262's bus. The host does not need to know
this beyond the framing below, but it explains the byte layout.

### 1.2 Serial framing (the "7-bit pack")

Each SPI transaction is sent as one frame:

1. `0x00` — control byte: SPI chip-select ON.
2. The payload bytes, **bit-packed 7 bits per wire byte**: the payload is treated as a
   bit stream (MSB first); each wire byte carries 7 payload bits in its top 7 bits and
   has bit 0 (the flag bit) set to `1`, marking it as data. The final partial group is
   padded with anything.
3. `0x02` — control byte: SPI chip-select OFF. This also clears the device's 7-bit
   accumulator — CS OFF is the resynchronization point of the whole protocol.

Packed length = `ceil(len * 8 / 7) + 2`.

Golden test vectors: payload `CA FE BA BE` encodes to `00 CB 7F AF 57 E1 02`; the
single-byte payload `00` encodes to `00 01 01 02`.

### 1.3 OPL3 board commands (SPI payloads)

All payloads start with the board address byte `0x42` (= `0x21 << 1`) followed by the
expander register `0x12` (GPIOA). After that header, bytes alternate between the
expander's A port (control lines to the YMF262) and B port (the data bus). The byte
sequences, as interface facts:

| Operation | SPI payload |
|---|---|
| Write bank 0 register | `42 12 E1 <reg> E3 <val> FB <val>` |
| Write bank 1 register | `42 12 E5 <reg> E7 <val> FB <val>` |
| Chip reset (IC line) | `42 12 FE 00` then `42 12 FF 00` |

**Coalescing:** consecutive register writes may share one frame: send the `42 12` header
once, then the 6-byte groups back to back, all inside one CS-ON/CS-OFF frame. The
reference allocates an 8 KiB command buffer and flushes it on demand (before each delay)
without ever bounds-checking it; our `CmdBuffer` additionally auto-flushes at 8 KiB so a
dense burst can never overflow. A frame must address only one board (always true here).

**Init sequence** (once, after opening the port):

1. Send the framed empty transaction — wire bytes `00 01 01 02` (the pack of a single
   `0x00` payload). The load-bearing part is the trailing CS OFF, which resynchronizes
   the device's bit accumulator after any unclean previous session.
2. For each SPI address `a` in `0x20..=0x27`, send three frames configuring a possible
   expander at that address (broadcast-style init; only the OPL3 at 0x21 exists here):
   - `(a<<1) 0A 28 28` — IOCON: enable HAEN + SEQOP,
   - `(a<<1) 00 00 00` — IODIRA: all pins output,
   - `(a<<1) 12 FF FF` — GPIOA/B: all lines high (bus idle).
3. OPL3 chip reset (IC-line toggle above), then allow a settle delay (the reference
   waits 200 ms at startup; treat that as the safe initial value and tune down on real
   hardware). This hard reset is paid **only at connect** — never on seeks (§3.4).
4. Mute sweep (below), so a freshly plugged device is silent.

**Mute sweep** (silence without knowing current register state): for every register `r`
in `0x20..=0xF5` on both banks, write `0xFF` if `0x40 <= r <= 0x55` (max total-level
attenuation) else `0x00`. Note the sweep deliberately starts at `0x20`: registers
`0x01..=0x08` — and critically bank 1's `0x04`/`0x05` (= `0x104` connection-select and
`0x105` NEW) — must not be caught in a blind sweep, because clearing NEW mid-sweep makes
the chip ignore all subsequent bank-1 writes. The sweep clobbers register state by
design; the shadow/diff mechanism (§3.4) reconstructs state before playing again.

**Pacing between writes:** none needed at the host. Each register write costs one 8-byte
SPI payload at 2 MHz on the device side (~32 µs), which already exceeds the YMF262's
required address/data hold times. The device applies backpressure through USB CDC.

### 1.4 Throughput envelope

USB full-speed CDC sustains roughly 500 KiB/s–1 MiB/s. A register write is 6–8 payload
bytes ≈ 9–12 wire bytes, so >40k writes/sec are available — far above any DRO/VGM's
demands. Seeks never replay history onto the wire: the shadow/diff mechanism (§3.4)
bounds any state reconstruction to at most ~2×190 register writes (~4 KiB wire,
<10 ms), no matter how deep into the song the seek lands.

---

## 2. Licensing constraints

- The reference implementation ([SudoMaker/RetroWave](https://github.com/SudoMaker/RetroWave))
  is **AGPL-3.0**, with explicit anti-copying warnings in its headers. This project is
  **LGPL-2.1-or-later**; AGPL code cannot be incorporated or translated. (The any-chip
  VGM plan contemplates relicensing the project to GPL-2.0-or-later; that would not
  change the approach here — incorporating AGPL code would still impose AGPL's terms
  on the whole app, and the upstream author's intent is plainly against reuse — so the
  independent-implementation rule below holds under either license.)
- Approach: **independent implementation from documented interface facts** (not
  "clean-room" in the strict two-team sense). Provenance of §1, stated plainly:
  - §1.2 (framing, control bytes, packing rule, the `CA FE BA BE` golden vector) comes
    from SudoMaker's *published* protocol description (`RetroWaveLib/Protocol/README.md`).
  - §1.3's command tables, init/mute sequences, buffer size, and timing constants are
    interface facts observed from the AGPL reference implementation and recorded here.
    Byte values, register sequences, and framing rules are unprotectable facts, not
    copyrightable expression.
  - The rule that makes this defensible: **the Rust implementation is written from this
    document only.** Do not port, paraphrase, or consult the C sources while writing
    the code.
- New dependencies, both compatible with LGPL-2.1-or-later:
  - `serialport` 4.x — **MPL-2.0**. Port enumeration with USB metadata, plus the serial
    I/O. MPL-2.0 §3.2(a) requires telling binary recipients how to obtain the
    MPL-covered source: extend the About dialog's existing notice section (it already
    carries the LGPL §6 notice) with a serialport attribution + source pointer
    (step 8). If serialport were ever vendored *and modified*, its file-level copyleft
    would require publishing the modified files — plain dependency use has no such
    obligation.
  - `spin_sleep` — **Apache-2.0**. Note: since Rust 1.75, `std::thread::sleep` on
    Windows 10+ already uses high-resolution waitable timers (~0.5 ms accuracy), so
    spin_sleep's value here is only its spin-tail for sub-quantum precision. Decide in
    step 3 whether plain std sleep against absolute deadlines suffices for the ~1.3 ms
    quantum; drop the dependency if so.

---

## 3. Architecture

Two candidate seams exist:

- **A. App-level:** a second `AudioService` implementation with its own event walker.
  Rejected as primary: it would duplicate the engine's carefully-tested semantics —
  mute/pan gating, seek-by-replay, loop-seam behavior ("no chip reset at the seam"),
  delay math (`FrameClock` integer carry) — and those must not drift between backends.
- **B. Chip-level + wall-clock pump (chosen):** implement `OplChip`
  ([opl.rs:12](../../crates/dro-synth/src/opl.rs)) for the serial device and drive the
  *existing* `PlayerEngine` (via `PlayerEngine::with_chip`,
  [engine.rs:359](../../crates/dro-synth/src/engine.rs)) from a plain thread paced by
  the wall clock instead of by a cpal callback. The engine already treats the chip as a
  write sink + sample generator; every engine behavior (mute/pan gating, loop seam,
  seek replay) flows through unchanged. The engine's `set_muting` even force-keys-off
  newly muted channels via `write_reg_buffered`
  ([engine.rs:393-411](../../crates/dro-synth/src/engine.rs)), so hardware muting
  silences a sounding note exactly like the emulator — for free.

The pump thread mirrors `dro-audio-native`'s structure (rtrb command queue in, shared
atomics out) so the service layer stays symmetric.

### 3.1 New crate: `crates/dro-retrowave`

Native-only workspace member. Dependencies: `dro-core`, `dro-synth`, `serialport`,
`rtrb`, `log` (+ `spin_sleep` pending the step-3 decision). Modules:

- **`protocol`** — pure functions: `packed_len`, `pack`, and a `CmdBuffer` that
  coalesces register writes into frames (header once; flush on demand or at 8 KiB).
  Unit-tested against the §1.2 golden vectors and §1.3 tables.
- **`device`** — `SerialIo` trait (open/write/close, mockable) with the `serialport`
  implementation; `enumerate() -> Vec<PortInfo>` — a **free function needing no open
  device** (the Settings dialog must list ports while the emulator backend is still
  active). `PortInfo { port_name, label, looks_like_retrowave }` where the heuristic is
  a case-insensitive "retrowave" match on the USB product string. **Windows caveat:**
  CDC devices bound to the in-box `usbser.sys` often report the generic registry
  FriendlyName "USB Serial Device (COMx)" rather than the device's own product string,
  so the heuristic may never fire on the primary platform — the probe tool (step 2)
  captures what the user's real device reports before we commit to any label format,
  and manual port selection is the reliable fallback. `Device::open(port)` runs the
  §1.3 init sequence; also `reset()` (IC toggle) and `mute()` (sweep).
- **`chip`** — `SerialOpl3Chip: OplChip` (§3.4) — the register shadow + diff engine.
  Constructed with the song's `OplType` for OPL2-compatibility translation (§3.5).
- **`player`** — `RetroWaveAudio`, the analog of `NativeAudio`
  ([dro-audio-native/src/lib.rs:74](../../crates/dro-audio-native/src/lib.rs)): pump
  thread + rtrb commands + shared atomics (`frames_rendered`, `next_instruction`,
  `finished`, `loop_iteration`, an `error` slot). The command vocabulary extends the
  native one (seek, rewind, muting, panning, loop config) with **`Pause` and `Resume`**
  — the native backend pauses via `cpal::Stream::pause()`, which has no pump analog,
  and pause/resume have real work to do on hardware (§3.3). `sample_rate()` reports
  `NATIVE_SAMPLE_RATE` (49,716 — [lib.rs:37](../../crates/dro-synth/src/lib.rs)); this
  is load-bearing for the UI (§4.3).

### 3.2 The pump thread

```
engine = PlayerEngine::with_chip(song, SerialOpl3Chip::new(opl_type), 49_716)
quantum = 64 frames (≈1.3 ms at 49716)
deadline = Instant::now()
loop {
    drain rtrb commands -> engine / transport transitions:
        Pause  -> playing=false; emit transient key-offs (§3.3); flush
        Resume -> playing=true;  chip.materialize() (§3.4); flush
        seek/mute/pan/loop -> engine (chip absorbs writes into shadow/queue);
                              if a seek was applied while playing -> chip.materialize()
    device.flush(chip.take_bytes())      // UNCONDITIONAL: mute-change and pause
                                         // key-offs must reach the device even while
                                         // not playing. (Paused seeks emit nothing
                                         // by design — §3.4.)
    if playing && !finished {
        engine.render(&mut scratch[..quantum*2])   // silence out; writes queue up
        device.flush(chip.take_bytes())
        if engine just finished { chip.mute_sweep(); flush }  // tails must not ring
    }
    deadline += quantum_duration                   // absolute, drift-free
    sleep until deadline (std high-res sleep or spin_sleep — step 3 decision)
    if fell behind by > 250 ms { clamp deadline to now }  // e.g. system sleep
}
on exit (any path, incl. catch_unwind of the loop body): best-effort chip.mute_sweep()
+ flush, then hand the Device back to the owner (§4.2). The IC-line reset is NOT part
of routine unload — it happens only when the owner tears the Device down for good
(app exit, backend switch, error), so an edit→Play reload never pays the reset settle.
```

- Pacing uses absolute deadlines (the reference player does the same via
  `clock_nanosleep(TIMER_ABSTIME)`), so serial latency never accumulates drift.
- The engine's own `FrameClock` still converts DRO ms / VGM samples to frames at
  49,716 Hz; the pump only decides *when* those frames have elapsed in wall time.
  `AudioConfig.frequency` is ignored in this mode.
- Optional later optimization: a `PlayerEngine::skip(frames)` method to advance without
  zero-filling a scratch buffer. Not needed for v1 — a 128-sample memset per ms is free.

### 3.3 Pause and resume (no engine seek involved)

A real YMF262 keeps sounding whatever its registers say, forever. Pause therefore must
silence it, and resume must restore it — *without* disturbing the engine, because
`seek_to_pos` is not position-preserving (it zeroes the in-progress delay and restarts
the loop counters, [engine.rs:579-594](../../crates/dro-synth/src/engine.rs)); pausing
mid-delay and resuming via seek would audibly skip and corrupt the "loop 2/5" readout.

- **Pause**: stop advancing the engine. Emit **transient** writes — key-off (bit 5
  clear) for `0xB0..=0xB8` on both banks *and* clear rhythm key bits (`0xBD &= 0xE0`) —
  built from the shadow's current values, sent via the device but **not** recorded in
  the shadow (they update only the `hw` model, §3.4). Notes enter release instead of
  droning. (The reference player just lets notes sustain during pause; key-off is our
  deliberate choice.)
- **Resume**: materialize the shadow-vs-hw diff (§3.4) — which is by construction
  exactly the key bits pause cleared, plus whatever mute/pan changes happened while
  paused — then continue advancing. Engine state (position, pending delay, loop
  counters) was never touched. Envelope phase is unrecoverable on real silicon, so
  resumed notes retrigger; documented behavior.

### 3.4 `SerialOpl3Chip`: register shadow + diff (the core trick)

The naive chip ("every `write_reg` becomes wire bytes") fails two ways: the engine's
seek path replays *every historical register write* from position 0
([engine.rs:579-594](../../crates/dro-synth/src/engine.rs) → `execute` →
`chip.write_reg` per instruction), which would stream potentially hundreds of
thousands of writes to the device — seconds of transfer during which the chip audibly
zips through the song's history — and paused seeks would re-arm key-on bits that pause
had cleared.

Instead the chip keeps two 2×256 register files (the same shape as `dro-core`'s
`OplState`, whose diff-of-two-folds pattern the crop feature already proved):

- `shadow` — the *target* state: **every** write (`write_reg` and
  `write_reg_buffered`) records here first.
- `hw` — the model of what the hardware currently holds, as `Option<u8>` per register:
  `None` = unknown. A new chip starts all-`None`, so its first materialize writes the
  full register file (~512 writes, <15 ms, once per song load) — which is also what
  makes the persistent-Device/per-song-chip split (§4.2) correct without any state
  handover between chips. **Invariant: every byte that reaches the wire goes through
  the chip so `hw` stays truthful.** The chip therefore owns the mute sweep too —
  `chip.mute_sweep()` emits the §1.3 sweep *and* stamps the swept values into `hw`
  (shadow untouched). The device-level sweep exists only for the §1.3 connect
  sequence, before any chip is alive. A pump-level sweep that bypassed the chip would
  silently desync `hw` and make the next materialize skip registers whose song-start
  values equal their song-end values — breaking replay-after-natural-end.

Behavior:

- `write_reg_buffered(reg, val)` (the engine's playback path): record in `shadow`,
  emit to the `CmdBuffer`, update `hw`. Live playback is a straight passthrough.
- `write_reg(reg, val)` (the engine's seek/replay path — immediate, unbuffered):
  record in `shadow` **only**. A seek thus mutates the shadow at memory speed and puts
  zero bytes on the wire.
- `reset()` (the engine calls this at the top of every seek): clear `shadow` to the
  all-zero reset state. **No wire traffic** — the diff handles the hardware side.
- `materialize()` (pump calls it on Resume, and at the end of any seek that happens
  while playing): emit exactly the registers where `shadow` ≠ `hw`, in a safe order:
  1. `0x105` (NEW) first if it rises 0→1 — bank 1 must be writable before bank-1 diffs;
  2. all bank-1 non-key diffs, then bank-0 non-key diffs (ascending order gives
     F-numbers before key-ons for free, but key regs are deferred regardless);
  3. key-carrying regs last: `0xB0..=0xB8` bank 1, then bank 0, then `0xBD`;
  4. `0x105` last if it falls 1→0 (bank-1 cleanup happened while it was still
     writable).
  A register is emitted when `hw` is `None` or differs from the (translated, §3.5)
  shadow value; registers known in `hw` but zero in `shadow` are written to their
  reset value (with `0x40..=0x55 → 0xFF`), so stale state cannot ring through. The
  diff is bounded by the register file (~2×190 meaningful regs), which is what makes
  §1.4's <10 ms seek claim true.

Seeks while playing: pump applies the engine seek (shadow now holds the target state),
then calls `materialize()` immediately. Seeks while paused: shadow updates, nothing hits
the wire, the chip stays silent with its keys off — resume's materialize catches
everything up. The pause-deferral quirks of `NativeAudioService` are *not* needed for
queue reasons (the pump always drains), but this shadow gating is the hardware
equivalent — document the asymmetry where the native service documents its deferrals
([services/audio.rs:8-13](../../crates/dro-trimmer/src/services/audio.rs)).

`generate_samples` zero-fills (trivially chunk-invariant). Bank addressing follows the
engine's existing convention (bank encoded as a `0x100` register offset,
[engine.rs:654](../../crates/dro-synth/src/engine.rs); DRO v1 `BankSwitch` is already
folded into that offset by the engine). The `stereo-ext` panpot pseudo-registers
(`0xD0..=0xD8` per bank) are recorded in the shadow but **never** emitted — real
hardware has no such registers.

### 3.5 OPL2 and dual-OPL2 songs on a real YMF262

OPL2 data never writes `0x105`, and the Nuked emulator is forgiving in compat mode
(it forces both speakers on when NEW=0 — see
[core.rs:1186-1191](../../vendor/nuked-opl3/src/core.rs)); a real YMF262 is not: with
NEW=0 it ignores the second register array entirely (dual-OPL2's second chip falls
silent), and with NEW=1 the OPL2 song's `0xC0` writes carry CHA/CHB=0, routing every
channel to *no* speaker.

`SerialOpl3Chip` is therefore constructed with the song's `OplType` and, for
`Opl2`/`DualOpl2` songs, applies a translation **at the wire boundary** (shadow always
keeps the song's own values; `hw` stores wire bytes; both the diff comparison and the
emission use the translated value, so pinned/translated registers do not re-emit on
every materialize):

- pin `0x105` to `0x01`: emitted (first, per the §3.4 ordering) whenever
  `hw[0x105] != Some(1)` — which covers chip construction (all-`None` `hw`),
  every materialize, and the first playback write, regardless of what any previous
  song left on the persistent Device; never let a diff clear it;
- OR `0x30` into every `0xC0..=0xC8` value on both banks (both speakers on).

OPL3 songs pass through untouched — their `0xC0` data carries real CHA/CHB bits. Add
chip tests for both translations; keep dual-OPL2 in the hardware checklist (§5.7).
(Dual-OPL2 fidelity note: the YMF262's second register array has no functional rhythm
set, so a dual-OPL2 song whose *second* chip uses rhythm mode loses that percussion —
but Nuked models the same silicon, so hardware output matches what the emulator
already plays today. Not a regression.)

**Eligibility gate (ties into the any-chip VGM plan).** The hardware backend can play
exactly what one YMF262 can express: DRO files, and VGM/VGZ whose chip data is
OPL2 / dual-OPL2 / OPL3 — nothing else, and no mixed-chip VGMs (a song pairing OPL3
with, say, a SN76489 cannot split its audio between the device's analog jack and the
PC speakers). Today every loadable song qualifies, because `dro-core`'s VGM reader
hard-errors on non-OPL commands ([data.rs](../../crates/dro-core/src/vgm/data.rs),
`build_offsets`). When the any-chip plan
([docs/vgm-multichip-2026-07/HANDOVER.md](../vgm-multichip-2026-07/HANDOVER.md))
relaxes that, it must also add a chip-set query on `Song` (e.g.
`Song::is_opl_only()`), and `SwitchingAudioService::load` (§4.2) must consult it:
a non-OPL-only song loaded while `output_backend = retrowave` plays through the
emulated path for that song, with a one-line notice — the config stays on RetroWave
and the next OPL-only song uses the hardware again. (Unlike the missing-port case,
which is transient and user-fixable, ineligibility is a property of the song, so a
per-song fallback with a visible notice beats a hard error.) Record this requirement
in the multichip handover when that work starts.

### 3.6 Behavior matrix (hardware mode)

| Operation | Behavior |
|---|---|
| Play | Engine renders (silence) + writes stream to device in real time. |
| Pause | §3.3: stop advancing, transient key-offs incl. rhythm bits. |
| Resume | §3.3: materialize diff, continue. Notes retrigger (inherent). |
| Stop (UI = pause+rewind) | Pause behavior (already acoustically silent — key-offs + release decay); the rewind's engine reset + replay-of-nothing leaves an all-zero shadow, so the next materialize writes the clean reset image. |
| Seek (incl. waveform click) | Engine replay mutates shadow only. While playing: one bounded diff burst (<10 ms) follows immediately. While paused: nothing hits the wire until Resume (§3.4). |
| Loop seam | Engine `wrap_to_loop_start` — cursor rewind, **no chip reset**, register state carries across the seam on real hardware exactly as the loop-points feature designed for the emulator. |
| Channel mute/solo | Engine gates writes *and* force-keys-off newly muted channels — reaches hardware via the buffered path unchanged. |
| Panning | Panpot pseudo-regs filtered (§3.4); per-channel pan is inert on hardware → grey out the pan sliders in hardware mode (like boost), tooltip "panning applies to emulated output only". |
| Boost / limiter | PCM-domain — meaningless here. `min_engaged_boost` → `None`; the transport's boost stepper (it lives in the controls panel, not Settings) is disabled in hardware mode. |
| VU meter | No PCM → `take_peaks` returns `None`; the meter already decays to silence on `None` ([peak_meter.rs:48-49](../../crates/dro-ui/src/widgets/peak_meter.rs)). (Optional later: a "monitor" mode wrapping a parallel `NukedOpl3` for meter data only.) |
| Waveform display | Unchanged — always an offline Nuked render; the live cursor derives from `position()` at 49,716 Hz (§4.3). |
| Natural end | Pump emits the **chip-level** mute sweep when `finished` flips (keeps `hw` truthful, §3.4), so tails don't ring forever and replay-after-end reconstructs correctly. |
| Device unplugged / write error | Pump parks, publishes the error, **and sets the finished/error atomics so `is_playing()` goes false** — transport UI recovers alongside the error toast; `play()` errors until a successful reload. |
| Configured port absent at load | `load()` fails with the surfaced error; stay on the RetroWave backend (no silent fallback), retry on each Play; Settings re-enumeration is the recovery path. |
| Non-OPL / mixed-chip VGM (future, once any-chip lands) | Plays through the emulated path for that song with a one-line notice; config stays on RetroWave (§3.5 eligibility gate). Not reachable today — the VGM reader rejects non-OPL data. |
| App exit / backend switch | Pump signalled, joined; its exit path (even via `catch_unwind`) runs the chip mute sweep and returns the `Device`; the owner then IC-resets and closes the port. Routine song unload stops at the mute (no reset, no settle — §3.2). |

---

## 4. Integration points

### 4.1 Config (`dro-core`)

Extend `AudioConfig` ([config.rs:14](../../crates/dro-core/src/config.rs)):

```rust
pub output_backend: OutputBackend,   // enum { Emulated (default), RetroWave }
pub retrowave_port: Option<String>,  // e.g. "COM5"; None = not yet chosen
```

INI keys `[audio] output_backend = emulated|retrowave`, `retrowave_port = COM5`, wired
through `apply_ini`/`to_ini_string`/`validate` with round-trip tests. Per the config's
established convention, an *unrecognized* `output_backend` value errors and discards
the whole document back to defaults (which are Emulated — playback still works); no
special-case silent fallback. `dro-core` stays wasm-clean — plain data fields; web
ignores them.

### 4.2 Backend switching (`dro-trimmer` services)

`DroApp` keeps its single `Box<dyn AudioService>`. The switch lives inside a new
composite in `crates/dro-trimmer/src/services/audio.rs`:

```rust
SwitchingAudioService {
    native: NativeAudioService,
    retrowave: Option<RetroWaveAudioService>,
    active: OutputBackend,
}
```

`load(song, &AudioConfig)` reads `config.output_backend`, activates the right inner
service, and tears down the other (releasing the COM port when switching away, the cpal
stream when switching to hardware). This is also where the §3.5 eligibility gate will
live once the any-chip VGM work lands: a song that is not OPL-only routes to `native`
for that load regardless of the configured backend (with a notice; today this branch
is unreachable). `list_hardware_ports()` calls
`dro_retrowave::device::enumerate()` **directly** — no inner service needed, so the
Settings dropdown works while the emulator is active (first-run setup flow).

**Port ownership across loads:** every editor edit invalidates `audio_revision` and the
next Play re-`load`s. The COM port must **not** be reopened per load — reopening costs
the 200 ms reset settle and Windows CDC ports can transiently fail to reopen right
after close. `RetroWaveAudioService` therefore keeps the opened `Device` persistent
across `load()`/`unload()` (each load hands it to the new pump thread; join returns
it), releasing it only on backend switch, app exit, or error. Two app instances cannot
share the port — the second open fails with a clear error (exclusive access is the OS
default; that is the desired behavior, surfaced not swallowed).

**When the switch takes effect:** `apply_settings` clears `audio_revision`, so a
backend change normally lands at the *next* play-triggering action
([app.rs:2882-2918](../../crates/dro-ui/src/app.rs)). That leaves the old backend
playing and the port held after Save — so `apply_settings` additionally calls
`audio.unload()` when `output_backend` or `retrowave_port` changed (one small, explicit
addition to the otherwise zero-plumbing path).

`RetroWaveAudioService::output_rate()` returns `Some(49_716)`.

### 4.3 `AudioService` trait additions (`dro-ui/src/platform.rs`)

Two defaulted methods so web/fake impls need no changes:

```rust
fn list_hardware_ports(&self) -> Vec<HardwarePortInfo> { Vec::new() }
fn last_error(&mut self) -> Option<String> { None }   // drained once per poll
```

`playback_tick` polls `last_error` and raises the standard error toast. Note
`output_rate()` is load-bearing for hardware mode: `push_loop_config` denominates
`LoopConfig::start_frames` in it and `ensure_audio` retunes the position readout from
it ([app.rs:3222-3227, 3284-3289](../../crates/dro-ui/src/app.rs)); returning anything
but 49,716 would skew the cursor and loop seam by ~3.5%. Covered by a service test.

### 4.4 Settings UI (`dro-ui/src/dialogs/settings.rs`)

New "Output" section above the existing audio grid:

- Backend dropdown: **Nuked OPL3 (emulated)** / **RetroWave OPL3 (hardware)**.
- When RetroWave is selected: port dropdown (from `list_hardware_ports`, auto-selecting
  the first `looks_like_retrowave` port when config has none) + a refresh button. Label
  format: port name + USB product string when a distinguishing one exists, bare port
  name otherwise (see the §3.1 Windows caveat — decide the final format from the probe
  tool's real-device dump in step 2).
- Frequency / buffer / bit-depth rows grey out in hardware mode (PCM-path settings).
  (The boost stepper lives in the transport controls, not Settings — it is disabled
  there per §3.6; the Settings dialog deliberately doesn't expose boost.)

Emits the existing `Action::ApplySettings(Box<AppConfig>)`. kittest snapshots change;
regenerate with `UPDATE_SNAPSHOTS=1` per the established baseline process.

### 4.5 CLI (`dro-trimmer/src/cli/play.rs`)

`drotrim play --output retrowave[:COM5] file.dro`. The current `play()` takes
`NativeAudio` concretely, so first extract the poll loop (`is_finished`/`position`)
into a small generic function, then branch on the flag. Also `drotrim retrowave-probe`:
dump all serial ports with full USB descriptors (VID/PID/strings — this is how we learn
what the real devices report), open the chosen one, play a two-second test chord, mute,
exit. First hardware smoke test and a user support tool.

---

## 5. Testing strategy

1. **Protocol unit tests** (pure): both §1.2 golden vectors; packed-length property vs.
   `pack` for lengths 0..64; frame structure (starts `0x00`, ends `0x02`, data bytes
   have bit 0 set); coalescing (two writes share one header; 8 KiB auto-flush).
2. **Command-layer tests**: bank 0/1 write byte sequences match §1.3 exactly; init
   sequence frame-by-frame (including the framed-empty-transaction first bytes
   `00 01 01 02`); mute sweep covers exactly `0x20..=0xF5` × 2 banks with the
   `0x40..0x55 → 0xFF` exception and touches nothing below `0x20`.
3. **Chip tests (the bulk of the new logic):**
   - playback writes pass through and update both files; seek-path writes mutate
     shadow only (zero wire bytes);
   - `reset()` emits nothing;
   - fresh chip (`hw` all-`None`): first materialize writes the full register file;
   - `materialize()` ordering: NEW first on rise / last on fall; key regs
     (`0xB0..=0xB8`, `0xBD`) strictly last; known-`hw`/`shadow`-zero regs written back
     to reset values with the TL exception; second materialize with no changes emits
     nothing (incl. for OPL2 songs — the translated-compare rule);
   - `chip.mute_sweep()` emits the §1.3 sweep and stamps `hw` so a following
     materialize rewrites the song state in full (the replay-after-natural-end case);
   - transient key-off builder: clears bit 5 of `0xB0..=0xB8` both banks + `0xBD`
     rhythm bits, updates `hw` only — a following materialize re-arms exactly those
     bits;
   - panpot regs (`0xD0..=0xD8`) never emitted;
   - OPL2/dual-OPL2 translation: `0x105` pinned to 1 and emitted whenever
     `hw[0x105] != Some(1)`, `0xC0..=0xC8 | 0x30` at the wire with shadow keeping song
     values, OPL3 songs untouched;
   - bank addressing mirrors the `NukedOpl3` convention tests.
4. **Pump tests with a mock `SerialIo`** capturing timestamped flushes: `write, delay
   100 ms, write` → two bursts ~100 ms apart (±20 ms, CI-safe); Pause emits key-offs
   even though not playing (the unconditional flush); paused seek emits nothing;
   playing seek emits one immediate diff burst; Resume emits the diff; finished →
   chip mute sweep; drop/join → mute sweep + Device handed back with **no** IC reset
   on routine unload; simulated write error → error + finished atomics set.
5. **Service/config tests**: INI round-trip incl. whole-config-discard on a bad
   `output_backend`; `SwitchingAudioService` activates/releases correctly (mock both);
   `output_rate() == Some(49716)`; Device persists across two `load()` calls. (The
   §3.5 eligibility-gate test arrives with the any-chip work, since no ineligible
   song can be constructed today.)
6. **UI kittest**: settings dialog Output section; hardware mode greys the PCM rows and
   pan sliders + boost stepper in the transport; snapshot regeneration.
7. **Hardware-in-the-loop checklist** (manual, with the user's original RetroWave
   OPL3): probe tool dump + chord → DRO v1, DRO v2, OPL3 VGM, dual-OPL2 VGM → pause /
   resume (notes stop, then retrigger) → seek by waveform click (no history-zipping,
   no stuck notes — the §3.4 payoff) → loop seam audition ("Play Seam") → channel mute
   mid-note → unplug mid-song (UI recovers) → backend switch mid-session both ways →
   app exit silences device. Tune the 200 ms settle down if tolerated.

CI note: workspace tests run on **windows-latest only**; the Linux jobs are per-crate
(wasm check + c-parity) and never build `dro-retrowave`, so serialport's Linux-side
`libudev` needs no CI change today. If a Linux workspace job is ever added, install
`libudev-dev` or disable serialport's default `libudev` feature (losing USB metadata).

---

## 6. Staged implementation checklist (one commit per step, workspace green each step)

1. **feat(retrowave): protocol + command building** — `dro-retrowave` crate,
   `protocol` module, §5.1–5.2 tests. No I/O yet.
2. **feat(retrowave): device layer** — `SerialIo` + serialport impl + `enumerate()` +
   init/reset/mute. `drotrim retrowave-probe` subcommand (▶ *user smoke test*: descriptor
   dump decides the §3.1 heuristic + §4.4 label format; chord proves the wire format).
3. **feat(retrowave): chip + pump** — `SerialOpl3Chip` shadow/diff, OPL2 translation,
   `RetroWaveAudio` pump, §5.3–5.4 tests. Decide std-sleep vs spin_sleep here.
4. **feat(core): audio backend config** — `AudioConfig` fields + INI round-trip.
5. **feat(app): switching service** — `RetroWaveAudioService` (persistent Device) +
   `SwitchingAudioService` wired at `drotrim.rs`; `AudioService` trait additions;
   `apply_settings` unload-on-backend-change.
6. **feat(ui): output settings** — settings dialog Output section, greyed PCM rows,
   pan/boost disabling, error toast path, snapshots (▶ *user end-to-end test*).
7. **feat(cli): --output retrowave** — poll-loop extraction + flag.
8. **docs** — update this file to IMPLEMENTED; README note (original board must be in
   USB mode via its hardware switch); About-dialog MPL-2.0 attribution for serialport.

Steps 1–4 are UI-independent; 3 depends on 1–2, 5 on 3–4, 6–7 on 5.

---

## 7. Risks and open questions

- **Discovery strings unknown** — and on Windows likely generic ("USB Serial Device").
  The probe dump (step 2) settles it; manual selection is the floor, and it is fine.
- **Reset settle time** (200 ms) is paid only at connect thanks to the shadow/diff
  design; tuning it down is optional polish.
- **Retrigger on resume/seek** is inherent — envelope phase is unrecoverable on real
  silicon. Documented behavior, not a bug.
- **Write-timing jitter ≈ quantum (~1.3 ms)** vs. the reference's exact per-command
  sleeps. Inaudible in practice; the `skip()` API or delay-boundary rendering closes
  the gap if ever needed.
- **Dense VGMs** (register-PCM abuse) could exceed CDC throughput during *live
  playback* (seeks are immune now); the reference has the same ceiling. Out of scope.
- **Real-chip NEW=0 edge cases**: the §3.5 translation makes OPL2-family behavior
  deterministic by construction, but the hardware checklist (§5.7) is the final word —
  if the real YMF262 surprises us anywhere, it will be there, on the user's device.
- **Pump panic** is contained by `catch_unwind` + unconditional mute/reset on the exit
  path; without it a panic would leave the chip playing a stuck chord until replug.

## 8. What was actually built

All eight steps landed on `rust` (commits `ca41084` … `791d17e`). The plan held up;
these are the differences worth knowing.

**Confirmed against the real board (the original RetroWave OPL3, 2026-07-23):**

- It enumerates as **USB `04d8:e966`** (a Microchip VID) — the first published ID for
  this hardware anywhere, as far as we can tell. `KNOWN_USB_IDS` in
  [device.rs](../../crates/dro-retrowave/src/device.rs) carries it.
- The §3.1 Windows caveat was exactly right, and worse than feared: the board reports
  manufacturer "Microsoft" and product "USB Serial Device (COM3)", both from
  `usbser.sys`. **No product-string match can ever work on Windows.** Detection is by
  USB ID, with `is_generic_description` suppressing the placeholder so the picker shows
  a bare port name rather than a misleading one.
- The wire format is right: `drotrim retrowave-probe` plays a chord on each register
  bank, and an OPL2 DRO plays in real time (2.68 s of song took 3.09 s of wall clock,
  the difference being process startup and the connect sequence).

**Design changes from the plan:**

- **`hw` is `Option<u8>` per register, not a plain byte** (§3.4 as revised). A fresh
  chip knows nothing about the hardware, so its first materialize writes the whole
  register file. This is what lets the device persist across song loads without any
  state handover between chips.
- **The transient key-off ordering was inverted.** The plan built pause key-offs from
  the shadow; the code builds them from `hw` (what is actually sounding) and updates
  only `hw`. Same effect, but it cannot key off a note the hardware never started.
- **`spin_sleep` was dropped** (the step-3 decision the plan left open). Since Rust
  1.75 `std::thread::sleep` uses high-resolution timers on Windows, which is finer
  than the 1.3 ms quantum. One less dependency.
- **The CLI flag landed with step 3, not step 7**, because `drotrim play --retrowave`
  is how the pump was verified on hardware in the first place.
- **`AppConfig` lost `Copy`** — an owned port name is a `String`. Eight call sites now
  clone, which is what they were doing implicitly anyway.
- **`apply_settings` unloads on a backend or port change**, which §4.2 called for; the
  plan under-sold it as "one small addition". It is the only reason switching away from
  hardware releases the serial port without pressing Play again.

**Still open** (nothing here blocks use):

- The 200 ms reset settle is untuned; it is paid only on connect.
- No monitor mode, so the VU meter stays dark during hardware playback (§3.6).
- Pan sliders and the boost stepper are still enabled in hardware mode. They are inert
  rather than wrong, and §3.6 wants them disabled with a tooltip.
- The §3.5 eligibility gate is unreachable until the any-chip VGM work lands.
- Rhythm-mode percussion on a dual-OPL2 song's *second* chip cannot sound on one
  YMF262 — the same limit the emulator has, so not a regression.

## 9. References

- Provenance of every §1 fact: see §2.
- Architecture seams verified against the repo on 2026-07-23: engine
  [engine.rs](../../crates/dro-synth/src/engine.rs), chip trait
  [opl.rs](../../crates/dro-synth/src/opl.rs), native driver
  [dro-audio-native](../../crates/dro-audio-native/src/lib.rs), service seam
  [platform.rs](../../crates/dro-ui/src/platform.rs), config
  [config.rs](../../crates/dro-core/src/config.rs), settings dialog
  [settings.rs](../../crates/dro-ui/src/dialogs/settings.rs).
- [SudoMaker/RetroWave](https://github.com/SudoMaker/RetroWave) (AGPL — see §2),
  [RetroWave OPL3 Express product page](https://shop.sudomaker.com/products/retrowave-opl3-express).
