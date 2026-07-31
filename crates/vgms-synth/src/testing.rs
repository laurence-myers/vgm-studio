//! Test support: a registry with a sounding stub, for tests of the paths that
//! read the ambient registry.
//!
//! This crate ships no generic cores of its own --
//! they all come from provider crates the app links -- so an in-crate test of
//! `render_vgm_wav`, the waveform renderer or `playability` has nothing real
//! to build. What those tests assert is *engine* behaviour (lengths, routing,
//! mixing, bucket shapes), not emulation, so a deterministic stub is the
//! honest instrument; the real-core end-to-end lives downstream in
//! `vgms-app`, where the providers are linked and registered.

use vgms_core::vgm::ChipKind;

use crate::chip::ChipCore;
use crate::registry::{CoreInfo, CoreMaker, CoreRegistry, LEVEL_UNITY};

/// A square wave that obeys the SN76489's volume latches and nothing else.
///
/// Enough behaviour for a test file to turn sound on and off -- `0x90` means
/// "channel 0 to full volume", `0x9F` means "and off again" -- which is what
/// the waveform tests need to see a shape. Pitch, noise and periods are
/// ignored; the tone is a fixed square so output is deterministic and
/// chunk-independent.
#[derive(Debug)]
pub(crate) struct ToneStub {
    /// Per-channel attenuation, 0xF = silent, as the SN76489 has it.
    volumes: [u8; 4],
    /// The channel mute mask, bit `c` = channel `c` muted -- so the channel
    /// splitter, which solos one channel at a time, sees silence for the ones
    /// it muted.
    muted: u32,
    /// Absolute frame counter, so chunked renders line up.
    at: u64,
}

impl ToneStub {
    pub(crate) fn new() -> Self {
        Self {
            volumes: [0xF; 4],
            muted: 0,
            at: 0,
        }
    }
}

impl ChipCore for ToneStub {
    fn reset(&mut self, _clock: u32, _variant: bool) {
        self.volumes = [0xF; 4];
        self.muted = 0;
        self.at = 0;
    }

    fn native_rate(&self) -> u32 {
        44_100
    }

    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        // Address 1 is the Game Gear stereo mask; only the command byte at
        // address 0 carries latches.
        if addr != 0 {
            return;
        }
        let byte = data as u8;
        // A latch byte selecting a volume register: `1cc1vvvv`.
        if byte & 0x90 == 0x90 {
            self.volumes[usize::from((byte >> 5) & 3)] = byte & 0x0F;
        }
    }

    fn set_channel_mutes(&mut self, muted: u32) {
        self.muted = muted;
    }

    fn render(&mut self, out: &mut [i32]) {
        // A channel sounds only if it is un-attenuated and not muted -- the
        // splitter mutes every channel but the one it is isolating.
        let sounding = self
            .volumes
            .iter()
            .enumerate()
            .any(|(channel, &volume)| volume < 0xF && (self.muted >> channel) & 1 == 0);
        for frame in out.chunks_exact_mut(2) {
            let sample = if sounding {
                // ~441 Hz square at 44.1 kHz: flip every 50 frames.
                if (self.at / 50).is_multiple_of(2) {
                    8_000
                } else {
                    -8_000
                }
            } else {
                0
            };
            frame[0] = sample;
            frame[1] = sample;
            self.at += 1;
        }
    }
}

/// Installs the ambient registry these tests share: the builtins plus the
/// stub, registered for the SN76489.
///
/// Idempotent and order-safe: every test that reads the ambient registry calls
/// this first, and whichever call wins installs the same content.
pub(crate) fn install_registry_with_stub() {
    let mut registry = CoreRegistry::with_builtins();
    registry.register(CoreInfo {
        id: "sn76489.stub",
        chip: ChipKind::Sn76489,
        label: "Test tone stub",
        authors: "this project",
        license: "MIT OR Apache-2.0",
        upstream: "",
        realtime: true,
        channel_pan: false,
        channel_mute: true,
        level: LEVEL_UNITY,
        make: CoreMaker::Generic(|| Box::new(ToneStub::new())),
    });
    let _ = crate::registry::install(registry);
}
