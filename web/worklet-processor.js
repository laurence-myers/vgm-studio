// SPDX-License-Identifier: GPL-2.0-or-later
//
// The AudioWorkletProcessor that hosts the `vgms-synth-worklet` wasm module. It
// runs on the audio rendering thread: it instantiates the module from bytes
// passed in `processorOptions` (the AudioWorkletGlobalScope has no fetch, and the
// module is deliberately bindgen-free so it needs no glue), then per 128-frame
// quantum drains queued command messages and renders planar f32 straight into the
// output. Playback state is posted back to the page every few quanta.
//
// Registered as 'vgms-engine'; the page creates one AudioWorkletNode per song.

const QUANTUM = 128; // the AudioWorklet render quantum, fixed by the platform
const POSTS_EVERY = 8; // post state ~ every 8 quanta (~23 ms at 44.1 kHz)

// The AudioWorkletGlobalScope has no TextEncoder/TextDecoder -- the very reason
// the worklet wasm module is bindgen-free -- so encode/decode UTF-8 by hand.
function utf8Encode(str) {
  const out = [];
  for (let i = 0; i < str.length; i++) {
    let c = str.charCodeAt(i);
    if (c < 0x80) {
      out.push(c);
    } else if (c < 0x800) {
      out.push(0xc0 | (c >> 6), 0x80 | (c & 0x3f));
    } else if (c >= 0xd800 && c <= 0xdbff) {
      const c2 = str.charCodeAt(++i);
      c = 0x10000 + ((c & 0x3ff) << 10) + (c2 & 0x3ff);
      out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 0x3f), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
    } else {
      out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
    }
  }
  return new Uint8Array(out);
}

function utf8Decode(bytes) {
  let str = "";
  let i = 0;
  while (i < bytes.length) {
    let c = bytes[i++];
    if (c >= 0x80) {
      if (c < 0xe0) {
        c = ((c & 0x1f) << 6) | (bytes[i++] & 0x3f);
      } else if (c < 0xf0) {
        c = ((c & 0x0f) << 12) | ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f);
      } else {
        c = ((c & 0x07) << 18) | ((bytes[i++] & 0x3f) << 12) | ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f);
      }
    }
    if (c > 0xffff) {
      c -= 0x10000;
      str += String.fromCharCode(0xd800 + (c >> 10), 0xdc00 + (c & 0x3ff));
    } else {
      str += String.fromCharCode(c);
    }
  }
  return str;
}

class VgmsEngineProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const opts = options.processorOptions || {};

    // Compile + instantiate synchronously. This happens once, before playback,
    // and the module is import-free so the empty import object suffices.
    this.instance = new WebAssembly.Instance(new WebAssembly.Module(opts.wasmBytes), {});
    this.ex = this.instance.exports;

    this.ex.vgmsw_init();
    for (const [slug, id] of opts.coreChoices || []) {
      this._withString(slug, (slugPtr, slugLen) =>
        this._withString(id, (idPtr, idLen) =>
          this.ex.vgmsw_set_core_choice(slugPtr, slugLen, idPtr, idLen),
        ),
      );
    }

    const loaded = this._load(opts.songName, opts.songBytes, opts.sampleRate, opts.resampleMode);
    if (!loaded) {
      this.port.postMessage({ type: "error", message: this._readError() });
    }

    // Output scratch inside the module: two planar f32 buffers of one quantum.
    this.leftPtr = this.ex.vgmsw_alloc(QUANTUM * 4);
    this.rightPtr = this.ex.vgmsw_alloc(QUANTUM * 4);

    // Play/pause arrives as a command message; a fresh node starts paused.
    this.playing = false;
    this.tick = 0;
    // Peaks accumulate between state posts so a transient is never dropped.
    this.peakL = 0;
    this.peakR = 0;
    this.limited = false;
    // Set by the `dispose` command when the page supersedes this node; the next
    // `process()` returns false so the browser stops the processor and lets it be
    // collected, instead of the default (return true, run forever).
    this.disposed = false;

    this.port.onmessage = (event) => this._onCommand(event.data);
  }

  _mem() {
    return new Uint8Array(this.ex.memory.buffer);
  }

  // Copies `bytes` into a fresh module allocation, runs `fn(ptr, len)`, frees it.
  _withBytes(bytes, fn) {
    const len = bytes.length;
    const ptr = this.ex.vgmsw_alloc(len);
    this._mem().set(bytes, ptr);
    try {
      return fn(ptr, len);
    } finally {
      this.ex.vgmsw_free(ptr, len);
    }
  }

  _withString(text, fn) {
    return this._withBytes(utf8Encode(text), fn);
  }

  _load(name, songBytes, sampleRate, resampleMode) {
    const nameBytes = utf8Encode(name || "song.vgm");
    return this._withBytes(nameBytes, (namePtr, nameLen) =>
      this._withBytes(new Uint8Array(songBytes), (bytesPtr, bytesLen) => {
        const code = this.ex.vgmsw_load(
          namePtr,
          nameLen,
          bytesPtr,
          bytesLen,
          sampleRate >>> 0,
          resampleMode >>> 0,
        );
        return code === 0;
      }),
    );
  }

  _readError() {
    const len = this.ex.vgmsw_error_len();
    if (len === 0) return "";
    const ptr = this.ex.vgmsw_alloc(len);
    this.ex.vgmsw_error_copy(ptr, len);
    const text = utf8Decode(this._mem().slice(ptr, ptr + len));
    this.ex.vgmsw_free(ptr, len);
    return text;
  }

  _onCommand(data) {
    switch (data.cmd) {
      case "play":
        this.playing = true;
        break;
      case "pause":
        this.playing = false;
        break;
      case "dispose":
        this.disposed = true;
        break;
      case "seekMs":
        this.ex.vgmsw_seek_ms(data.ms >>> 0);
        break;
      case "seekPos":
        this.ex.vgmsw_seek_pos(data.pos >>> 0);
        break;
      case "rewind":
        this.ex.vgmsw_rewind();
        break;
      case "setBoost":
        this.ex.vgmsw_set_boost(data.boost);
        break;
      case "setLoop":
        this.ex.vgmsw_set_loop(
          data.enabled ? 1 : 0,
          data.start >>> 0,
          data.end >>> 0,
          data.countTag >>> 0,
          data.countTimes >>> 0,
          data.startFrames,
        );
        break;
      case "setMuting":
        this.ex.vgmsw_set_muting(data.channels >>> 0, data.perc0 & 0xff, data.perc1 & 0xff);
        break;
      case "setPanning":
        if (data.mode === 1 && data.pans) {
          this._withBytes(new Uint8Array(data.pans), (ptr, len) =>
            this.ex.vgmsw_set_panning(1, ptr, len),
          );
        } else {
          this.ex.vgmsw_set_panning(0, 0, 0);
        }
        break;
      case "setChipMute":
        this._withString(data.slug, (ptr, len) =>
          this.ex.vgmsw_set_chip_mute(ptr, len, data.instance & 0xff, data.mask >>> 0),
        );
        break;
      case "setChipPan": {
        const pans = new Int16Array(data.pans);
        const bytes = new Uint8Array(pans.buffer, pans.byteOffset, pans.byteLength);
        this._withString(data.slug, (slugPtr, slugLen) =>
          this._withBytes(bytes, (pansPtr, pansLen) =>
            this.ex.vgmsw_set_chip_pan(slugPtr, slugLen, data.instance & 0xff, pansPtr, pansLen),
          ),
        );
        break;
      }
      case "setChipTrim":
        // A scalar percent like the mute mask, not a buffer as the pans are.
        this._withString(data.slug, (ptr, len) =>
          this.ex.vgmsw_set_chip_trim(ptr, len, data.instance & 0xff, data.percent & 0xff),
        );
        break;
      default:
        break;
    }
  }

  process(_inputs, outputs) {
    // A disposed node stops here: returning false removes the processor from the
    // graph so a superseded node does not keep rendering (and leaking) forever.
    if (this.disposed) return false;

    const out = outputs[0];
    const left = out[0];
    const right = out.length > 1 ? out[1] : out[0];
    const frames = left.length;

    if (this.playing) {
      this.ex.vgmsw_render(this.leftPtr, this.rightPtr, frames);
      // Re-view the memory each call: a first render can grow it, detaching an
      // older view, but the pointers stay valid in the new buffer.
      const buffer = this.ex.memory.buffer;
      const l = new Float32Array(buffer, this.leftPtr, frames);
      const r = new Float32Array(buffer, this.rightPtr, frames);
      left.set(l);
      if (right !== left) right.set(r);
    } else {
      left.fill(0);
      if (right !== left) right.fill(0);
    }

    // Fold this quantum's peaks into the running maxima, then flush a state
    // message every few quanta.
    this.peakL = Math.max(this.peakL, this.ex.vgmsw_take_peak(0));
    this.peakR = Math.max(this.peakR, this.ex.vgmsw_take_peak(1));
    if (this.ex.vgmsw_take_limited() !== 0) this.limited = true;

    if (++this.tick >= POSTS_EVERY) {
      this.tick = 0;
      this.port.postMessage({
        type: "state",
        frames: this.ex.vgmsw_position_frames(),
        ms: this.ex.vgmsw_position_ms() >>> 0,
        row: this.ex.vgmsw_position_row() >>> 0,
        loopIteration: this.ex.vgmsw_loop_iteration() >>> 0,
        finished: this.ex.vgmsw_is_finished() !== 0,
        peakL: this.peakL,
        peakR: this.peakR,
        limited: this.limited,
        minEngagedBoost: this.ex.vgmsw_min_engaged_boost(),
      });
      this.peakL = 0;
      this.peakR = 0;
      this.limited = false;
    }

    // Keep the processor alive across the whole song, playing or paused.
    return true;
  }
}

registerProcessor("vgms-engine", VgmsEngineProcessor);
