//! Song-format channel split: rewriting a VGM's command stream so that only one
//! chip channel sounds, producing a standalone VGM.
//!
//! The WAV split ([`split_vgm_cancellable`](crate::split_vgm_cancellable))
//! *renders* each channel soloed; this one *rewrites* the command stream
//! instead, dropping or transforming the writes that would sound on any channel
//! but the chosen one. It is the [`ChannelGate`] hosted over a stream rather than
//! over a live core (as [`GatedCore`](crate::registry) does): the mute mask is
//! fixed before the replay begins, so there are no mute edges to sequence and no
//! seek-replay ambiguity -- the simplest of the gate's hosts.
//!
//! Two facts keep it small:
//!
//! - **No write is ever encoded from scratch.** A passed write is copied
//!   verbatim; a transformed one ([`GateAction::Replace`]) has only its data
//!   value changed, and for every gated chip the data is the write command's
//!   final byte -- so re-encoding is a copy with that byte swapped. No
//!   chip-specific command encoder is needed.
//! - **No mute edge is prepended.** Every gated chip's channels are silent at
//!   power-up (the SN76489 and SSG at attenuation floor, the FM/OPL keys off,
//!   the YM2612 DAC disabled), so a channel muted for this stem stays silent
//!   until its first write arrives -- which the gate then drops or forces. The
//!   fixed mask is applied with its edge writes discarded.
//!
//! Only a gate-covered chip can be soloed this way -- native mute is a
//! render-time trick, no help to a stream rewrite -- so
//! [`ChannelGate::exists`](crate::channel_gate::ChannelGate::exists) gates the
//! caller. Every *other* chip in the file is silenced wholesale by dropping its
//! writes, which needs no gate table at all.
//!
//! Both the dropped chips and a soloed chip's never-written channels rely on the
//! player resetting every chip to silence at load -- the same assumption every
//! trim, crop and edit in this codebase makes, and one VGMPlay, libvgm and this
//! app's own engine all honour. A hardware-accurate core whose chip powers up in
//! an undefined (possibly loud) state and never receives the driver's own
//! silencing burst (dropped here) is the one place a stem could sound a chip it
//! means to mute; that is an accepted limitation (plan §3), not a defect in the
//! rewrite.

use vgms_core::Result;
use vgms_core::vgm::stream::END_OF_DATA;
use vgms_core::vgm::{ChipKind, VgmCommand, VgmFile, channels_of};

use crate::channel_gate::{ChannelGate, GateAction};

/// The YM2612 DAC's sample register -- the one stream-fed register among the
/// gated chips (see [`ChannelGate::stream_channel`]).
const YM2612_DAC_REGISTER: u8 = 0x2A;
/// The `0x90` stream-setup opcode.
const DAC_STREAM_SETUP: u8 = 0x90;
/// The `0x93`/`0x95` stream-start opcodes -- the ones a muted-channel-bound
/// stream must not reach, since its samples are synthesised at render time.
const DAC_STREAM_START: u8 = 0x93;
const DAC_STREAM_START_FAST: u8 = 0x95;

/// Where a `0x90`-set-up stream writes: the chip, its instance, its port, and
/// its register.
type StreamBinding = (ChipKind, u8, u8, u8);

/// Rewrites `file`'s command stream to solo channel `channel_index` of the
/// `instance`-th `kind`, returning a standalone VGM named `name`.
///
/// `kind` must be one [`ChannelGate::exists`] covers; the caller checks.
///
/// # Errors
/// If `file`'s command stream did not walk (an opaque body), so there is nothing
/// to filter.
pub(crate) fn solo_channel_to_vgm(
    file: &VgmFile,
    kind: ChipKind,
    instance: u8,
    channel_index: usize,
    name: String,
) -> Result<VgmFile> {
    let stream = file.stream().ok_or_else(|| {
        vgms_core::Error::file("Cannot split a VGM whose command stream did not parse")
    })?;

    // Build the gate for the soloed chip, keyed to its clock and variant, and
    // mute every channel but the chosen one. The edge writes the mask emits are
    // discarded: nothing sounds yet (see the module note), so there is nothing
    // to silence at t=0.
    let chip = file.header.chips().iter().find(|c| c.kind == kind);
    let variant = chip.is_some_and(|c| c.variant);
    let clock = chip.map_or(0, |c| c.clock);
    let mut gate = ChannelGate::new(kind).expect("the caller checked ChannelGate::exists");
    gate.reset(clock, variant);
    gate.configure(file.header.settings());
    let roster = channels_of(kind, variant).len() as u32;
    let roster_mask = if roster >= 32 {
        u32::MAX
    } else {
        (1u32 << roster) - 1
    };
    let mask = !(1u32 << channel_index) & roster_mask;
    gate.set_mask(mask, &mut Vec::new());

    // The YM2612 DAC (channel 6, register 0x2A on the first instance) is the one
    // channel fed by the `0x8n` fast path and the DAC streams; soloing it keeps
    // both, soloing anything else drops them.
    let dac_soloed = kind == ChipKind::Ym2612
        && instance == 0
        && gate.stream_channel(0, YM2612_DAC_REGISTER) == Some(channel_index as u8);

    let orig_loop = file.loop_index();
    let orig_loop_end = file.loop_end_index();
    let mut loop_at = None;
    let mut loop_end_at = None;

    let mut out = Vec::with_capacity(stream.raw().len());
    // Each defined stream's target, learned from its `0x90` setup.
    let mut stream_targets: [Option<StreamBinding>; 256] = [None; 256];

    for index in 0..stream.len() {
        let Some(command) = stream.get(index) else {
            continue;
        };
        let raw = stream.raw_command(index).unwrap_or(&[]);
        let at = out.len();
        let emitted = match command {
            VgmCommand::Write { target, addr, data } => {
                if target.kind == kind && target.instance == instance {
                    match gate.filter(target.port, addr, data) {
                        GateAction::Pass => {
                            out.extend_from_slice(raw);
                            true
                        }
                        GateAction::Drop => false,
                        GateAction::Replace(new) => {
                            push_reencoded(&mut out, raw, new);
                            true
                        }
                    }
                } else {
                    // Every other chip instance is silent in this stem, so its
                    // register writes are dropped whole -- no gate table needed.
                    false
                }
            }
            VgmCommand::DacWrite { wait } => {
                if dac_soloed {
                    out.extend_from_slice(raw);
                    true
                } else {
                    // The DAC is muted: drop the sample byte, keep the wait, so
                    // the timing is preserved to the sample.
                    push_wait_samples(&mut out, wait);
                    wait > 0
                }
            }
            VgmCommand::DacStream { opcode, stream_id } => {
                if opcode == DAC_STREAM_SETUP {
                    stream_targets[stream_id as usize] = parse_stream_setup(raw);
                }
                let is_start = matches!(opcode, DAC_STREAM_START | DAC_STREAM_START_FAST);
                let muted = stream_is_muted(
                    stream_targets[stream_id as usize],
                    kind,
                    instance,
                    &gate,
                    mask,
                );
                if is_start && muted {
                    false
                } else {
                    out.extend_from_slice(raw);
                    true
                }
            }
            // Waits, data blocks, RAM writes, PCM seeks and anything unmodelled
            // pass verbatim: the timeline and the sample banks are untouched.
            _ => {
                out.extend_from_slice(raw);
                true
            }
        };
        if emitted {
            // The loop point and its short end follow the first surviving
            // command at or after their original row -- a dropped write takes no
            // time, so the loop still restarts the same instant of music.
            if loop_at.is_none() && orig_loop.is_some_and(|l| index >= l) {
                loop_at = Some(at);
            }
            if loop_end_at.is_none() && orig_loop_end.is_some_and(|e| index >= e) {
                loop_end_at = Some(at);
            }
        }
    }
    out.push(END_OF_DATA);
    let tail = out.len() - 1;
    // A loop point (or short end) whose row ran off the end of the surviving
    // stream lands on the new tail: a loop that ran to the end, or one whose
    // whole body was muted away (which `with_filtered_body` then normalises).
    if orig_loop.is_some() && loop_at.is_none() {
        loop_at = Some(tail);
    }
    if orig_loop_end.is_some() && loop_end_at.is_none() {
        loop_end_at = Some(tail);
    }

    let mut filtered = file.with_filtered_body(out, loop_at, loop_end_at);
    filtered.name = name;
    Ok(filtered)
}

/// Whether a stream bound as `target` writes to a channel muted in this stem.
///
/// A stream on another chip instance is muted (that whole chip is silent here); a
/// stream on the soloed chip is muted iff its register drives a muted channel. An
/// unrouted stream, or one whose register drives no single channel, is kept --
/// silencing a voice the gate does not model is worse than leaving it.
fn stream_is_muted(
    target: Option<StreamBinding>,
    kind: ChipKind,
    instance: u8,
    gate: &ChannelGate,
    mask: u32,
) -> bool {
    let Some((tk, ti, port, register)) = target else {
        return false;
    };
    if tk != kind || ti != instance {
        return true;
    }
    gate.stream_channel(port, register)
        .is_some_and(|channel| mask & (1u32 << channel) != 0)
}

/// The `(kind, instance, port, register)` a `0x90 ss tt pp cc` setup names, or
/// `None` for a chip id the spec does not number.
fn parse_stream_setup(raw: &[u8]) -> Option<StreamBinding> {
    let tt = *raw.get(2)?;
    let port = *raw.get(3)?;
    let register = *raw.get(4)?;
    let kind = ChipKind::from_id(tt & 0x7F)?;
    Some((kind, u8::from(tt & 0x80 != 0), port, register))
}

/// Emits a `0x8n` DAC write's embedded wait (0..=15 samples) as one short-wait
/// opcode (`0x70` = 1 sample .. `0x7F` = 16). A zero wait emits nothing.
fn push_wait_samples(out: &mut Vec<u8>, samples: u32) {
    if samples == 0 {
        return;
    }
    debug_assert!(samples <= 16, "a 0x8n wait is at most 15 samples");
    out.push(0x70 | ((samples as u8).saturating_sub(1) & 0x0F));
}

/// Re-emits a write command with only its data value replaced.
///
/// Every gate that returns [`GateAction::Replace`] does so for a chip whose
/// register write carries its data in the final byte (the SN76489, the YM/OPL
/// family, the AY8910), so this is the original command's bytes with that byte
/// swapped -- no per-chip command encoder. A future exotic gate table (16-bit or
/// non-trailing data) must extend this.
fn push_reencoded(out: &mut Vec<u8>, raw: &[u8], new: u16) {
    debug_assert!(new <= 0xFF, "a gated Replace value is a single byte");
    debug_assert!(!raw.is_empty(), "a decoded write has at least an opcode");
    let split = raw.len() - 1;
    out.extend_from_slice(&raw[..split]);
    out.push(new as u8);
}
