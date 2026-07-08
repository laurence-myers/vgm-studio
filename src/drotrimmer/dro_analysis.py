#!/usr/bin/python
#
#    Use, distribution, and modification of the DRO Trimmer binaries, source code,
#    or documentation, is subject to the terms of the MIT license, as below.
#
#    Copyright (c) 2008 - 2023 Laurence Dougal Myers
#
#    Permission is hereby granted, free of charge, to any person obtaining a copy
#    of this software and associated documentation files (the "Software"), to deal
#    in the Software without restriction, including without limitation the rights
#    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
#    copies of the Software, and to permit persons to whom the Software is
#    furnished to do so, subject to the following conditions:
#
#    The above copyright notice and this permission notice shall be included in
#    all copies or substantial portions of the Software.
#
#    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
#    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
#    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
#    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
#    THE SOFTWARE.
import typing
from collections import defaultdict

from . import dro_data, regdata
from .dro_config import get_config
from .dro_util import DROTrimmerException, smp_to_ms

DetailedRegisterEntry = tuple[int, str, int]
DetailedRegisterInfo = list[DetailedRegisterEntry]

# Duplicated from dro_data to avoid circular import. TODO: move to common location.
DRO_FILE_V1 = 1
DRO_FILE_V2 = 2


class DROTotalDelayCalculator(object):
    def sum_delay(self, dro_song: dro_data.AbstractSong) -> int:
        # Bleh
        calc_delay = 0
        for inst in dro_song.data:
            if inst.inst_type == dro_data.DROInstructionType.DELAY_MS:
                calc_delay += inst.value
        return calc_delay


class DROTotalSamplesCalculator(object):
    def sum_delay(self, dro_song: dro_data.AbstractSong) -> int:
        calc_delay = 0
        for inst in dro_song.data:
            if inst.inst_type == dro_data.DROInstructionType.DELAY_SMP:
                calc_delay += inst.value
        return calc_delay


class DROTotalDelayWithWriteDelayCalculator(object):
    def __init__(self) -> None:
        config = get_config()
        self.chip_write_delay: float = config.audio.chip_write_delay

    def sum_delay(self, dro_song: dro_data.AbstractSong):
        calc_delay: float = 0.0  # milliseconds
        total_write_delay: float = 0.0  # microseconds
        for inst in dro_song.data:
            if inst.inst_type == dro_data.DROInstructionType.DELAY_MS:
                calc_delay += inst.value
            elif inst.inst_type == dro_data.DROInstructionType.REGISTER:
                total_write_delay += self.chip_write_delay
        calc_delay += total_write_delay // 1000
        return calc_delay


class DROFirstDelayAnalyzer(object):
    def __init__(self):
        self.result = False

    def analyze_dro(self, dro_song: dro_data.AbstractSong):
        if not len(dro_song.data):
            return
        inst = dro_song.data[0]
        if inst.inst_type == dro_data.DROInstructionType.DELAY_MS:
            self.result = True


class DROTotalDelayMismatchAnalyzer(object):
    def __init__(self):
        self.result = False

    def analyze_dro(self, dro_song: dro_data.AbstractSong):
        calc_delay = DROTotalDelayCalculator().sum_delay(dro_song)
        self.result = calc_delay != dro_song.ms_length


class DRODetailedRegisterAnalyzer(object):
    # TODO: output channels and banks in the table.
    OPL_TYPE_OPL2 = 0
    OPL_TYPE_DUAL_OPL2 = 1
    OPL_TYPE_OPL3 = 2

    def __init__(self) -> None:
        self.current_bank: int = 0
        self.current_state: list[int | None] = []
        self.OPL_TYPE_DRO1_MAP: list[int] = [
            self.OPL_TYPE_OPL2,
            self.OPL_TYPE_OPL3,
            self.OPL_TYPE_DUAL_OPL2,
        ]
        self.OPL_TYPE_DRO2_MAP: list[int] = [
            self.OPL_TYPE_OPL2,
            self.OPL_TYPE_DUAL_OPL2,
            self.OPL_TYPE_OPL3,
        ]  # a bit pointless, but added for consistency.

    def analyze_dro(
        self,
        dro_song: dro_data.AbstractSong,
    ) -> typing.Iterator[DetailedRegisterEntry]:
        self.current_state = [None] * 0x1FF
        # Wait for the data lock to become available.
        with dro_song.data_lock:
            total_delay_ms = 0
            for inst in dro_song.data:
                match inst.inst_type:
                    case dro_data.DROInstructionType.DELAY_MS:
                        yield (
                            self.current_bank,
                            "Delay: %s ms" % (inst.value,),
                            total_delay_ms,
                        )
                        total_delay_ms += inst.value

                    case dro_data.DROInstructionType.DELAY_SMP:
                        yield (
                            self.current_bank,
                            "Delay: %s smp" % (inst.value,),
                            total_delay_ms,
                        )
                        total_delay_ms += smp_to_ms(inst.value)

                    case dro_data.DROInstructionType.BANK_SWITCH:
                        self.current_bank = inst.value
                        yield (
                            self.current_bank,
                            "Bank switch: %s" % (("low", "high")[self.current_bank],),
                            total_delay_ms,
                        )

                    case dro_data.DROInstructionType.REGISTER:
                        if inst.bank is not None:
                            self.current_bank = inst.bank
                        desc = self.__analyze_and_update_register(
                            self.current_bank, inst.command, inst.value
                        )
                        yield (self.current_bank, desc, total_delay_ms)

                    case _:
                        raise DROTrimmerException(
                            f"Unrecognised instruction type: {inst.inst_type}"
                        )

    def __analyze_and_update_register(
        self,
        bank: int,
        reg: int,
        val: int,
    ):
        try:
            if bank and (0x100 | reg) in regdata.registers:
                register_description = regdata.registers[0x100 | reg]
            else:
                register_description = regdata.registers[reg]
        except Exception:
            return "Unknown register: %s" % (reg,)

        reg_and_bank = (bank << 8) | reg
        old_val = self.current_state[reg_and_bank]

        changed_desc = []
        bitmasks = regdata.register_bitmask_lookup[register_description]
        for bm in bitmasks:
            # Output the description for this bitmask, if the old value is None (start of the song), or the
            #  value has changed.
            if old_val is None or (bm.mask & old_val) ^ (bm.mask & val):
                changed_desc.append(bm.description)

        self.current_state[reg_and_bank] = val

        return " / ".join(changed_desc) if len(changed_desc) else "(no changes)"


class DRORegisterUsageAnalyzer(object):
    PERC_CHANNEL = 0xBD

    def __init__(self, detailed_percussion_analysis: bool = False):
        self.detailed_percussion_analysis = detailed_percussion_analysis
        self.perc_usage: defaultdict[int, bool] = defaultdict(bool)
        self.usage: defaultdict[int, int] = defaultdict(int)

    def analyze_dro(self, dro_song):
        """Returns two dicts. First dict is register usage, second dictt is perc inst usage.
        Keys are registers, with the bank set in bit 0x100. e.g.
         register 0xDB on the high bank will return a key of 0x1DB.
        Values are the number of times that register is used in the DRO file.

        Perc usage dict:
        Keys are bitmasks (powers of 2), values are "True" if that bit was set during
        the analysis."""
        self.usage = defaultdict(int)
        self.perc_usage = defaultdict(bool)
        perc_bitmasks = regdata.register_bitmask_lookup[
            regdata.registers[self.PERC_CHANNEL]
        ]
        with dro_song.data_lock:
            bank = 0
            for inst in dro_song.data:
                if inst.bank is not None:
                    bank = inst.bank
                if inst.inst_type == dro_data.DROInstructionType.BANK_SWITCH:
                    bank = inst.value
                if inst.inst_type == dro_data.DROInstructionType.REGISTER:
                    self.usage[(bank << 8) | inst.command] += 1
                    if (
                        inst.command == self.PERC_CHANNEL
                        and self.detailed_percussion_analysis
                    ):
                        # Go through all bitmasks, mark any usages.
                        for i, pb in enumerate(perc_bitmasks):
                            if inst.value & pb.mask:
                                self.perc_usage[(bank << 8) | pb.mask] = True
        return self.usage, self.perc_usage


class DRODebugAnalyzer(object):
    def __init__(self):
        pass

    def analyze_dro(self, dro_song: dro_data.AbstractSong):
        """Prints out the DRO song info, then prints each instruction."""
        with dro_song.data_lock:
            print(dro_song)
            for inst in dro_song.data:
                print(inst)


class DROSimpleNoteAnalyser(object):
    PITCH_REGISTERS = frozenset(range(0xA0, 0xA8 + 1))
    KEY_ON_REGISTERS = frozenset(range(0xB0, 0xB8 + 1))
    CHANNELS_PER_BANK = 9

    class NoteStatus(object):
        PITCH_MAP = {
            0x015B: " C",
            0x016B: "C#",
            0x0181: " D",
            0x0198: "D#",
            0x01B0: " E",
            0x01CA: " F",
            0x01E5: "F#",
            0x0202: " G",
            0x0220: "G#",
            0x0241: " A",
            0x0263: "A#",
            0x0287: " B",
            0x02AE: " C",
        }

        def __init__(self, channel=None, note_status_to_clone=None):
            if note_status_to_clone is not None:
                self.channel = note_status_to_clone.channel
                self.pitch = note_status_to_clone.pitch
                self.octave = note_status_to_clone.octave
                self.on = note_status_to_clone.on
            else:
                self.channel = channel
                self.pitch = 0
                self.octave = 0
                self.on = False

        def __str__(self):
            # 0x241 = 440.0 hz
            # 0x241 = 577
            # 24.7 hz between notes
            # approx 1.31 per hz
            closest_value = min(
                self.PITCH_MAP.keys(), key=lambda x: abs(x - self.pitch)
            )
            note_name = "%s-%s" % (self.PITCH_MAP[closest_value], self.octave)
            return "(ch: %s, pitch: %x, oct: %s, note: %s)" % (
                self.channel,
                self.pitch,
                self.octave,
                note_name,
            )

    def analyze_dro(self, dro_song):
        """
        Returns a list of of "Note on" pitch values (as NoteStatus objects), containing one list per channel.
        Ignores pitch bends and other pitch changes while a note is on.

        @type dro_song: DROSong
        """
        channel_notes = [
            DROSimpleNoteAnalyser.NoteStatus(channel=i + 1)
            for i in range(DROSimpleNoteAnalyser.CHANNELS_PER_BANK * 2)
        ]
        output = [[] for _ in range(DROSimpleNoteAnalyser.CHANNELS_PER_BANK * 2)]
        with dro_song.data_lock:
            # Track the bank the way DRORegisterUsageAnalyzer does. DRO v2 and VGM
            #  carry the bank on every register write, but DRO v1 tracks it with
            #  separate bank switch instructions and leaves `inst.bank` as None.
            #  Multiplying that None by CHANNELS_PER_BANK raised a TypeError on
            #  every DRO v1 file.
            bank = 0
            for inst in dro_song.data:
                if inst.bank is not None:
                    bank = inst.bank
                if inst.inst_type == dro_data.DROInstructionType.BANK_SWITCH:
                    bank = inst.value
                # Ignore non-register stuff.
                if inst.inst_type != dro_data.DROInstructionType.REGISTER:
                    continue
                # If it's A0 - A8, update the pitch
                elif inst.command in DROSimpleNoteAnalyser.PITCH_REGISTERS:
                    note_status = self.get_channel_status(channel_notes, bank, inst)
                    note_status.pitch = (note_status.pitch & 0xFF00) | inst.value
                # If it's B0 - B8, update the pitch and note on/off
                elif inst.command in DROSimpleNoteAnalyser.KEY_ON_REGISTERS:
                    note_status = self.get_channel_status(channel_notes, bank, inst)
                    note_status.pitch = (note_status.pitch & 0x00FF) | (
                        (inst.value & 0x03) << 8
                    )
                    note_status.octave = (inst.value & 0x1C) >> 2
                    orig_on_value = note_status.on
                    note_status.on = (inst.value & 0x20) > 0
                    # If note on status changes, make a new entry in the output list
                    if note_status.on ^ orig_on_value and note_status.on:
                        output[note_status.channel - 1].append(
                            DROSimpleNoteAnalyser.NoteStatus(
                                note_status_to_clone=note_status
                            )
                        )
        return output

    def get_channel_status(self, channel_notes, bank, inst):
        channel_index = (inst.command & 0x0F) + (
            bank * DROSimpleNoteAnalyser.CHANNELS_PER_BANK
        )
        return channel_notes[channel_index]
