// SPDX-License-Identifier: GPL-2.0-or-later
//! The write fold: turning our engine's `(port, addr, data)` back into the raw
//! bus bytes each libvgm core expects.
//!
//! [`fold`] is pure, total and `const`; the wrapper's write dispatch feeds it
//! the engine's normalised operands and puts the [`Bus`] it returns onto
//! libvgm's FFI writers. Every [`WriteRule`] arm is the inverse of a
//! normalisation `vgms_core::vgm::stream` applied on the way in, each pinned by
//! a test naming the upstream handler it mirrors.

use vgms_core::vgm::stream::{BANK_PORT, MEMORY_PORT, STEREO_PORT};

/// How a chip's `(port, addr, data)` reaches libvgm's register writer.
///
/// **Every variant is transcribed from a handler in libvgm's
/// `player/vgmplayer_cmdhandler.cpp`**, named in its doc comment. The mapping is
/// the inverse of what `vgms_core::vgm::stream` did on the way in: our decoder
/// normalises (folding the QSound's `0xC4`, reading the C352's `0xE1`
/// big-endian, splitting the `0xD3`/`0xD4` address, routing RF memory pokes to
/// port 1), and libvgm's cores expect the raw conventions those fixes hid, so
/// each rule puts one back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteRule {
    /// `write8(addr, data)`. Upstream's `Cmd_SN76489`, `Cmd_GGStereo`,
    /// `Cmd_Ofs8_Data8` and `Cmd_Port_Ofs8_Data8`.
    ///
    /// `addr` is passed through rather than forced to zero: the SN76489's `0x50`
    /// arrives as address 0 and `0x4F` (Game Gear stereo) as address 1, exactly
    /// libvgm's `SN76496_W_REG` and `SN76496_W_GGST`.
    Register,
    /// The Yamaha address/data latch pair, on `port`: `write8((port<<1)|0, reg)`
    /// then `write8((port<<1)|1, data)`. Upstream's `SendYMCommand`.
    ///
    /// **Two writes, not one**: getting that wrong is the classic silent core --
    /// the chip latches a register number and never receives a value.
    RegisterLatch,
    /// `writeM8(addr, data)` -- the *memory* space, not the register file.
    /// Upstream's `Cmd_RF5C_Mem` and `Cmd_Ofs16_Data8` for the chips whose
    /// address arrives whole (`0xC5`-`0xC8`: SCSP, WonderSwan RAM, VSU, X1-010).
    ///
    /// Undoes our port-1 convention: `vgms_core` routes `0xC1`/`0xC2` to port 1
    /// so a RAM poke cannot be mistaken for a register write, but here the two
    /// are different *functions*, so the port has done its job and is dropped.
    Memory,
    /// `writeM8((port << 8) | addr, data)`. Upstream's `Cmd_Ofs16_Data8` for
    /// `0xD3`/`0xD4` (K054539, C140): the 16-bit offset arrives split (high seven
    /// bits in `port`, low eight in `addr`) and is recombined here.
    MemoryPortHigh,
    /// `writeM16(addr, data)` -- fetched with `RWF_REGISTER`, despite upstream's
    /// field name. `Cmd_Ofs16_Data16`, the C352's `0xE1` alone.
    ///
    /// Our decoder already reads both operands big-endian (as libvgm's
    /// `ReadBE16` does), so both values pass straight through.
    RegisterAddr16Data16,
    /// The QSound's three writes: data MSB to `0x00`, data LSB to `0x01`, then
    /// the register to `0x02`. Upstream's `Cmd_QSound_Reg`.
    ///
    /// Our decoder normalised `0xC4` so `addr` is the register and `data` the
    /// 16-bit value; this splits them back apart in bus order. **The register
    /// goes last**: it is the write that commits the pair.
    QSound,
    /// Port 0 is a register write, port 1 a memory poke: `write8(addr, data)`
    /// or `writeM8(addr, data)`. Upstream's `Cmd_RF5C_Reg` (`0xB0`/`0xB1`) and
    /// `Cmd_RF5C_Mem` (`0xC1`/`0xC2`).
    ///
    /// **The one rule that reads `port` as meaning rather than as address.** The
    /// RF5C68's register file and 4 KiB RAM window overlap to a core, so
    /// `vgms_core` puts registers on port 0 and memory on port 1; here they are
    /// different libvgm functions, so the port chooses which.
    RegisterOrMemoryByPort,
    /// The MultiPCM: its register file on port 0, its `0xC3` bank select on
    /// [`BANK_PORT`]. Upstream's `Cmd_Ofs8_Data8` (`0xB5`) and `Cmd_YMW_Bank`
    /// (`0xC3`).
    ///
    /// **The bank is not a register write.** The YMW258 has no bank register on
    /// its bus; libvgm's core invents three (`0x10` for Sega Model 1's 1 MB
    /// window, `0x11`/`0x12` for Multi 32's two 512 KiB banks) and `Cmd_YMW_Bank`
    /// translates the command into them. This has to be here, not in the decoder,
    /// because the register numbers are libvgm's invention.
    ///
    /// `addr` is the bank mask, `data` the offset in 64 KiB units. The offset's
    /// high byte is dropped (as upstream drops `fData[0x03]`): these registers
    /// hold a byte and cannot express a ROM over 16 MB.
    MultiPcmBank,
    /// The NES APU with upstream's FDS remap: `Cmd_NES_Reg`. `0x3F` becomes
    /// `0x23` (FDS I/O enable) and `0x20`-`0x3E` become `0x80 | (a & 0x1F)`
    /// (the FDS registers), everything else passes through.
    ///
    /// The remap is the *player's*, not the chip's: VGM stores FDS writes in a
    /// compressed range the NSFPlay core does not use.
    NesApu,
    /// The OKIM6295 with upstream's pin-7 strip: `Cmd_OKIM6295_Reg`. A write to
    /// `0x0B` (the clock select) drops bit 7 -- "a bug in some MAME VGM logs",
    /// per upstream -- and everything else passes.
    Okim6295,
    /// The WonderSwan: registers offset by `0x80` on port 0 (`Cmd_WSwan_Reg`, as
    /// the chip's ports really live there), wave RAM through the memory writer on
    /// [`MEMORY_PORT`] (`Cmd_Ofs16_Data8`, our `0xC6` convention).
    WonderSwan,
    /// The SAA1099's two-write pair in the *reverse* order to the Yamaha one:
    /// `Cmd_SAA_Reg` puts the register at offset 1 and the data at offset 0,
    /// where the chip's own bus has them.
    ReversedLatch,
    /// One 16-bit-data register write: `writeD16(addr, data)`. Upstream's
    /// `Cmd_Ofs4_Data12` -- the 32X PWM, whose nibble register and 12-bit value
    /// our decoder delivers as `addr`/`data`. (The ES5506's `0xD6` would take a
    /// two-width sibling, but libvgm ships only a stub for that device.)
    Data16,
    /// The AY8910: an address/data latch pair on offsets 0/1, its `0x31` stereo
    /// mask on [`STEREO_PORT`] going to a dedicated function (`Cmd_AY_Stereo`
    /// fetches it by the `'ST'` user code), never the register file.
    ///
    /// What both AY cores file under `DEVRW_A8D8` is their *IO-port* interface
    /// (`EPSG_writeIO`, MAME's `ay8910_write`): an even offset latches the
    /// address, an odd one writes the data. A single direct `write8(reg, data)`
    /// lands every write in the wrong register and renders as digital noise.
    RegisterWithStereo,
    /// The OPN family (YM2203/2608/2610): the Yamaha latch pair on its ports,
    /// plus the YM2203's SSG stereo mask on [`STEREO_PORT`], which lands on the
    /// *linked* AY8910's mask function.
    OpnFamily,
}

/// Exactly what a [`WriteRule`] decided to put on libvgm's bus.
///
/// Named rather than called straight through so **the fold is testable**: with
/// the decision separated from the FFI call, a per-entry test is an ordinary
/// `assert_eq!` on a value instead of intercepting a C function pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bus {
    /// One `write8(addr, data)`.
    Reg8(u8, u8),
    /// Two, in order -- the Yamaha address/data latch.
    Reg8Pair((u8, u8), (u8, u8)),
    /// Three, in order -- the QSound's MSB, LSB, register.
    Reg8Triple((u8, u8), (u8, u8), (u8, u8)),
    /// One `writeM8(addr, data)`.
    Mem8(u16, u8),
    /// One `writeM16(addr, data)`, fetched as a register writer.
    Reg16(u16, u16),
    /// One `writeD16(addr, data)` -- an 8-bit address with 16-bit data.
    RegD16(u8, u16),
    /// The chip's dedicated stereo-mask function, with the six mask bits.
    StereoMask(u8),
    /// Nothing at all. Only [`WriteRule::MultiPcmBank`] produces it, for a bank
    /// select whose mask names no bank -- upstream's `Cmd_YMW_Bank` takes neither
    /// of its two `if`s.
    Nothing,
}

/// Turns our engine's `(port, addr, data)` into the bytes libvgm expects.
///
/// Pure, total and `const`. Every arm is the inverse of a normalisation
/// `vgms_core::vgm::stream` applied on the way in, each pinned by a test naming
/// the upstream handler it mirrors.
pub(crate) const fn fold(rule: WriteRule, port: u8, addr: u16, data: u16) -> Bus {
    match rule {
        WriteRule::Register => Bus::Reg8(addr as u8, data as u8),
        WriteRule::RegisterLatch => {
            Bus::Reg8Pair(((port << 1), addr as u8), ((port << 1) | 1, data as u8))
        }
        WriteRule::Memory => Bus::Mem8(addr, data as u8),
        WriteRule::MemoryPortHigh => Bus::Mem8((port as u16) << 8 | (addr & 0xFF), data as u8),
        WriteRule::RegisterAddr16Data16 => Bus::Reg16(addr, data),
        WriteRule::QSound => Bus::Reg8Triple(
            (0x00, (data >> 8) as u8),
            (0x01, (data & 0xFF) as u8),
            (0x02, addr as u8),
        ),
        WriteRule::RegisterOrMemoryByPort => {
            if port == 0 {
                Bus::Reg8(addr as u8, data as u8)
            } else {
                Bus::Mem8(addr, data as u8)
            }
        }
        WriteRule::NesApu => {
            // `Cmd_NES_Reg`'s FDS remap, register for register.
            let a = addr as u8;
            let remapped = if a == 0x3F {
                0x23
            } else if a & 0xE0 == 0x20 {
                0x80 | (a & 0x1F)
            } else {
                a
            };
            Bus::Reg8(remapped, data as u8)
        }
        WriteRule::Okim6295 => {
            // `Cmd_OKIM6295_Reg`: the clock-select register drops the stray
            // pin-7 bit some MAME logs carry.
            let value = if addr == 0x0B {
                (data as u8) & 0x7F
            } else {
                data as u8
            };
            Bus::Reg8(addr as u8, value)
        }
        WriteRule::WonderSwan => {
            if port == MEMORY_PORT {
                Bus::Mem8(addr, data as u8)
            } else {
                // `Cmd_WSwan_Reg`: the audio ports live at 0x80 on the chip's
                // own bus, and our decoder keeps the file's 0-based numbers.
                Bus::Reg8(0x80 + (addr as u8 & 0x7F), data as u8)
            }
        }
        WriteRule::ReversedLatch => {
            // `Cmd_SAA_Reg`: register to offset 1, then data to offset 0 --
            // the mirror image of the Yamaha pair.
            Bus::Reg8Pair((0x01, addr as u8), (0x00, data as u8))
        }
        WriteRule::Data16 => Bus::RegD16(addr as u8, data),
        WriteRule::RegisterWithStereo => {
            if port == STEREO_PORT {
                Bus::StereoMask(data as u8 & 0x3F)
            } else {
                // The IO-port latch: address to offset 0, data to offset 1.
                Bus::Reg8Pair((0x00, addr as u8), (0x01, data as u8))
            }
        }
        WriteRule::OpnFamily => {
            if port == STEREO_PORT {
                Bus::StereoMask(data as u8 & 0x3F)
            } else {
                Bus::Reg8Pair(((port << 1), addr as u8), ((port << 1) | 1, data as u8))
            }
        }
        WriteRule::MultiPcmBank => {
            if port != BANK_PORT {
                return Bus::Reg8(addr as u8, data as u8);
            }
            // `Cmd_YMW_Bank`, register for register. Each register divides the
            // offset's low byte down to its own shift (`data << 20` for `0x10`,
            // `data << 19` for `0x11`/`0x12`), both landing on
            // `(offset & 0xFF) << 16` bytes -- the 64 KiB units the command counts.
            let bank = (data & 0xFF) as u8;
            match addr & 0x03 {
                // 1 MB banking (Sega Model 1): one window, only for a whole
                // megabyte. Bit 3 set is half a megabyte in, which one window
                // cannot express, so fall through to the pair below.
                0x03 if bank & 0x08 == 0 => Bus::Reg8(0x10, bank / 0x10),
                // 512 KB banking (Sega Multi 32): mask bit 1 is the low bank,
                // bit 0 the high one, and both may be set.
                0x03 => Bus::Reg8Pair((0x11, bank / 0x08), (0x12, bank / 0x08)),
                0x02 => Bus::Reg8(0x11, bank / 0x08),
                0x01 => Bus::Reg8(0x12, bank / 0x08),
                _ => Bus::Nothing,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bytes each rule puts on the bus.
    ///
    /// Every case is checked against the handler in
    /// `player/vgmplayer_cmdhandler.cpp` it mirrors: these are inversions of our
    /// decoder's normalisations, and a wrong one is silent -- the chip takes the
    /// write and plays something else.
    #[test]
    fn each_write_rule_puts_the_documented_bytes_on_the_bus() {
        // `Cmd_SN76489` / `Cmd_GGStereo` / `Cmd_Ofs8_Data8`: straight through,
        // and the address is *not* forced to zero -- the SN76489's `0x4F`
        // (Game Gear stereo) arrives as address 1 and must stay there.
        assert_eq!(
            fold(WriteRule::Register, 0, 0x00, 0x8E),
            Bus::Reg8(0x00, 0x8E)
        );
        assert_eq!(
            fold(WriteRule::Register, 0, 0x01, 0xDE),
            Bus::Reg8(0x01, 0xDE),
            "SN76496_W_GGST is address 1; folding it to 0 would send a stereo \
             mask to the tone latch"
        );

        // `SendYMCommand`: the address/data latch pair, port-shifted. Two
        // writes -- one alone latches a register number and plays nothing.
        assert_eq!(
            fold(WriteRule::RegisterLatch, 0, 0x28, 0xF1),
            Bus::Reg8Pair((0x00, 0x28), (0x01, 0xF1))
        );
        assert_eq!(
            fold(WriteRule::RegisterLatch, 1, 0x28, 0xF1),
            Bus::Reg8Pair((0x02, 0x28), (0x03, 0xF1)),
            "port 1 is offsets 2 and 3, not 1 and 2"
        );

        // `Cmd_RF5C_Mem` / `Cmd_Ofs16_Data8` with a whole address.
        assert_eq!(
            fold(WriteRule::Memory, 0, 0x0F12, 0x7F),
            Bus::Mem8(0x0F12, 0x7F)
        );

        // `Cmd_Ofs16_Data8` for `0xD3`/`0xD4`, where our decoder split the
        // 16-bit offset across `port` and `addr`. Recombining it wrongly is
        // the difference between C140 voice 0 and voice 8.
        assert_eq!(
            fold(WriteRule::MemoryPortHigh, 0x12, 0x34, 0x56),
            Bus::Mem8(0x1234, 0x56)
        );

        // `Cmd_Ofs16_Data16`: the C352, both operands already big-endian.
        assert_eq!(
            fold(WriteRule::RegisterAddr16Data16, 0, 0x0108, 0xBEEF),
            Bus::Reg16(0x0108, 0xBEEF)
        );

        // `Cmd_QSound_Reg`: MSB, LSB, then the register -- and the register
        // last, because it is the write that commits the pair.
        assert_eq!(
            fold(WriteRule::QSound, 0, 0x09, 0x1234),
            Bus::Reg8Triple((0x00, 0x12), (0x01, 0x34), (0x02, 0x09))
        );

        // The RF5C pair: our port-1 convention chooses the *function*, so the
        // same address means two different places.
        assert_eq!(
            fold(WriteRule::RegisterOrMemoryByPort, 0, 0x07, 0xC0),
            Bus::Reg8(0x07, 0xC0),
            "port 0 is the register file"
        );
        assert_eq!(
            fold(WriteRule::RegisterOrMemoryByPort, 1, 0x07, 0xC0),
            Bus::Mem8(0x0007, 0xC0),
            "port 1 is the RAM window -- the same number, a different space"
        );

        // `Cmd_Ofs8_Data8` and `Cmd_YMW_Bank`, the MultiPCM's two commands.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, 0, 0x01, 0x1E),
            Bus::Reg8(0x01, 0x1E),
            "port 0 is still the ordinary `0xB5` register file"
        );
        // A whole-megabyte offset with both banks named: Model 1's single 1 MB
        // window, register 0x10, the value divided down to what the core's
        // `data << 20` expects (`0x10` counts 64 KiB units).
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x03, 0x0010),
            Bus::Reg8(0x10, 0x01)
        );
        // Half a megabyte in, so one window cannot express it and upstream
        // takes the Multi 32 path for both banks instead -- `0x11` before
        // `0x12`, and `data << 19` this time.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x03, 0x0018),
            Bus::Reg8Pair((0x11, 0x03), (0x12, 0x03))
        );
        // One bank at a time: **mask bit 1 is the low bank and bit 0 the high
        // one**, the way round `Cmd_YMW_Bank` has it and the opposite of how the
        // bits read.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x02, 0x0020),
            Bus::Reg8(0x11, 0x04)
        );
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x01, 0x0028),
            Bus::Reg8(0x12, 0x05)
        );
        // A mask naming no bank writes nothing, rather than writing zero
        // somewhere: upstream takes neither `if`.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x00, 0x0010),
            Bus::Nothing
        );
        // The offset's high byte is dropped where upstream drops `fData[0x03]`.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x03, 0x0110),
            Bus::Reg8(0x10, 0x01),
            "these registers hold a byte; a ROM over 16 MB is not expressible"
        );

        // `Cmd_NES_Reg`'s FDS remap: 0x3F is the I/O enable at 0x23, the
        // 0x20-0x3E block moves to 0x80-0x9E, and the APU's own registers
        // pass through untouched.
        assert_eq!(
            fold(WriteRule::NesApu, 0, 0x3F, 0x80),
            Bus::Reg8(0x23, 0x80)
        );
        assert_eq!(
            fold(WriteRule::NesApu, 0, 0x22, 0x7F),
            Bus::Reg8(0x82, 0x7F)
        );
        assert_eq!(
            fold(WriteRule::NesApu, 0, 0x15, 0x0F),
            Bus::Reg8(0x15, 0x0F)
        );

        // `Cmd_OKIM6295_Reg`: only the clock-select register strips bit 7.
        assert_eq!(
            fold(WriteRule::Okim6295, 0, 0x0B, 0x85),
            Bus::Reg8(0x0B, 0x05),
            "the stray pin-7 bit some MAME logs carry is dropped"
        );
        assert_eq!(
            fold(WriteRule::Okim6295, 0, 0x00, 0x85),
            Bus::Reg8(0x00, 0x85)
        );

        // `Cmd_WSwan_Reg`: the audio ports live at 0x80 on the chip's own
        // bus; wave RAM keeps its own writer through our port convention.
        assert_eq!(
            fold(WriteRule::WonderSwan, 0, 0x0F, 0x42),
            Bus::Reg8(0x8F, 0x42)
        );
        assert_eq!(
            fold(WriteRule::WonderSwan, MEMORY_PORT, 0x0123, 0x42),
            Bus::Mem8(0x0123, 0x42),
            "wave RAM is a memory poke, not an offset register"
        );

        // `Cmd_SAA_Reg`: the mirror image of the Yamaha pair -- register to
        // offset 1, then data to offset 0.
        assert_eq!(
            fold(WriteRule::ReversedLatch, 0, 0x14, 0xFF),
            Bus::Reg8Pair((0x01, 0x14), (0x00, 0xFF))
        );

        // `Cmd_Ofs4_Data12`: the PWM's nibble register with its 12-bit value,
        // through the 16-bit-data writer.
        assert_eq!(
            fold(WriteRule::Data16, 0, 0x02, 0x0155),
            Bus::RegD16(0x02, 0x0155)
        );

        // `Cmd_AY_Stereo`: the mask goes to the dedicated function, never the
        // register file -- and a register write is the IO-port latch pair
        // (`Cmd_DReg8_Data8` -> `SendYMCommand` on port 0), because the AY
        // cores' `A8D8` writer is `EPSG_writeIO`: even offset latches the
        // address, odd offset carries the data.
        assert_eq!(
            fold(WriteRule::RegisterWithStereo, STEREO_PORT, 0, 0x2D),
            Bus::StereoMask(0x2D)
        );
        assert_eq!(
            fold(WriteRule::RegisterWithStereo, 0, 0x01, 0x0F),
            Bus::Reg8Pair((0x00, 0x01), (0x01, 0x0F))
        );

        // The OPN family: the latch pair on its ports, the YM2203's stereo
        // mask on the SSG's function.
        assert_eq!(
            fold(WriteRule::OpnFamily, 1, 0x28, 0xF1),
            Bus::Reg8Pair((0x02, 0x28), (0x03, 0xF1))
        );
        assert_eq!(
            fold(WriteRule::OpnFamily, STEREO_PORT, 0, 0x15),
            Bus::StereoMask(0x15)
        );
    }
}
