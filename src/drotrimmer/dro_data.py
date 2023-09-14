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

import array
import math
import threading
from abc import ABC, abstractmethod
from enum import Enum
from typing import Self, Literal, Any, overload, Iterator, Iterable

from . import dro_globals, dro_undo, dro_util, regdata

# Duplicated from dro_analysis to avoid circular import. TODO: move to common location.
DetailedRegisterEntry = tuple[int, str, int]
DetailedRegisterInfo = list[DetailedRegisterEntry]

DRO_FILE_V1 = 1
DRO_FILE_V2 = 2


class DROInstructionType(Enum):
    REGISTER = 0
    DELAY = 1
    BANK_SWITCH = 2


class DROInstruction(object):
    __slots__ = ["inst_type", "command", "value", "bank"]

    def __init__(
        self,
        inst_type: DROInstructionType,
        command: int,
        value: int,
        bank: Literal[0, 1] | None = None,
    ) -> None:
        self.inst_type = inst_type
        self.command = command
        self.value = value
        self.bank = bank

    def __repr__(self) -> str:
        return "DROInstruction(DROInstructionType.%s, %s, %s, bank=%s)" % (
            DROInstructionType(self.inst_type),
            self.command,
            self.value,
            self.bank,
        )

    def __eq__(self, other: Any) -> bool:
        if type(other) == DROInstruction:
            if (
                self.inst_type == other.inst_type
                and self.command == other.command
                and self.value == other.value
                and self.bank == other.bank
            ):
                return True
        return False

    def __hash__(self) -> int:
        return hash((self.inst_type, self.command, self.value, self.bank))


class DROData(ABC):
    """Wraps around the DRO data, providing access to each instruction,
    while efficiently storing the item in memory.
    Locking should be performed by any outer code that mutates the data.
    """

    def __init__(
        self, data: array.array, short_delay_code: int, long_delay_code: int
    ) -> None:
        self.data = data
        self.short_delay_code = short_delay_code
        self.long_delay_code = long_delay_code
        self.delay_codes = (short_delay_code, long_delay_code)

    @abstractmethod
    def _translate_index(self, key: int) -> int:
        ...

    @abstractmethod
    def _interpret_data(self, real_index: int) -> DROInstruction:
        ...

    @abstractmethod
    def __len__(self) -> int:
        ...

    @abstractmethod
    def _iter_indexes(self):
        ...

    @abstractmethod
    def shallow_copy(self, new_data=None) -> Self:
        """Copies everything except the actual underlying data. You can pass in
        new data to assign to the copy."""
        ...

    def __delitem__(self, key: int | slice) -> None:
        if type(key) == slice:
            if key.start is None:
                first_index = None
            else:
                first_index = self._translate_index(key.start)
            if key.stop is None:
                second_index = None
            else:
                try:
                    second_index = self._translate_index(key.stop + 1)
                except IndexError:
                    second_index = None  # possibly dangerous
        else:
            first_index = self._translate_index(key)
            try:
                second_index = self._translate_index(key + 1)
            except IndexError:
                second_index = None  # possibly dangerous
        del self.data[first_index:second_index]

    def delete_multiple(self, index_list: list[int], is_sorted: bool = False) -> None:
        """NOTE: the given index_list will be sorted in-place, and reversed in-place."""
        # Sort if required
        if not is_sorted:
            index_list.sort()  # dodgy, hidden side effects
        index_list.reverse()  # dodgy, hidden side effects
        # We're usually going to delete slices, so convert ranges of
        #  indexes into slices. Will be more efficient to do one
        #  deletion of 10000 items, than 10000 deletions of one item.
        #  (That's my theory, anyway.)
        index_list = dro_util.condense_slices(index_list)
        for i in index_list:
            del self[i]

    @overload
    def __getitem__(self, key: int) -> DROInstruction:
        ...

    @overload
    def __getitem__(self, key: slice) -> Self:
        ...

    def __getitem__(self, key: int | slice) -> DROInstruction | Self:
        """Returns the item, translated from the "logical" index
        to the real index in the underlying array. The returned item is
        a DROInstruction object, interpreted from the raw data.

        Supports slices, which returns a new DROData."""
        if type(key) == slice:
            first_index = (
                None if key.start is None else self._translate_index(key.start)
            )
            last_index = None if key.stop is None else self._translate_index(key.stop)
            new_data = self.data[first_index:last_index]
            new_copy = self.shallow_copy(new_data)
            return new_copy
        else:
            real_index = self._translate_index(key)
            return self._interpret_data(real_index)

    def __iter__(self) -> Iterator[DROInstruction]:
        for i in self._iter_indexes():
            yield self[i]

    def _insert(self, key: int, value_array: array.array) -> None:
        assert type(value_array) == array.array
        real_i = self._translate_index(key)
        self.data[real_i:real_i] = value_array

    def insert_multiple(
        self,
        i_and_val_list: Iterable[tuple[int, array.array]],
    ) -> None:
        for i, val in i_and_val_list:
            self._insert(i, val)

    def tofile(self, file_handle):
        self.data.tofile(file_handle)

    def raw_len(self):
        return len(self.data)

    def raw_iter(self):
        return iter(self.data)

    def get_raw(self, key: int) -> array.array:
        first_index = self._translate_index(key)
        try:
            second_index = self._translate_index(key + 1)
        except IndexError:
            second_index = None
        if second_index is None:
            return self.data[first_index:]
        else:
            return self.data[first_index:second_index]

    def append_raw(self, value_array):
        self.data.extend(value_array)


class DRODataV1(DROData):
    def __init__(self, data: array.array):
        super().__init__(data, 0x00, 0x01)
        self.index_map: list[int] = []  # keys are indexes.
        self.generate_index_map()

    def delete_multiple(self, index_list, is_sorted=False):
        super().delete_multiple(index_list, is_sorted)
        self.generate_index_map()

    def insert_multiple(self, i_and_val_list):
        real_offset = 0
        for num_inserted, (i, val) in enumerate(i_and_val_list):
            real_index = self._translate_index(i - num_inserted) + real_offset
            self.data[real_index:real_index] = val
            real_offset += len(val)
        self.generate_index_map()

    def append_raw(self, value_array):
        self.index_map.append(self.raw_len())
        super().append_raw(value_array)

    def shallow_copy(self, new_data: array.array | None = None) -> "DRODataV1":
        new_copy = DRODataV1(
            new_data if new_data is not None else array.array("B"),
        )
        new_copy.generate_index_map()
        return new_copy

    def _translate_index(self, index):
        try:
            return self.index_map[index]
        except IndexError as ie:
            if index == len(self.index_map):
                return len(self.data)
            else:
                raise ie

    def _interpret_data(self, real_index):
        cmd = self.data[real_index]
        if cmd == 0x00:
            inst_type = DROInstructionType.DELAY
            val = self.data[real_index + 1] + 1
        elif cmd == 0x01:
            inst_type = DROInstructionType.DELAY
            val = (self.data[real_index + 1] | (self.data[real_index + 2] << 8)) + 1
        elif cmd == 0x02:
            inst_type = DROInstructionType.BANK_SWITCH
            val = 0x00
        elif cmd == 0x03:
            inst_type = DROInstructionType.BANK_SWITCH
            val = 0x01
        elif cmd == 0x04:
            inst_type = DROInstructionType.REGISTER
            cmd = self.data[real_index + 1]
            val = self.data[real_index + 2]
        else:
            inst_type = DROInstructionType.REGISTER
            val = self.data[real_index + 1]

        return DROInstruction(inst_type, cmd, val)

    def __len__(self):
        return len(self.index_map)

    def _iter_indexes(self):
        return range(len(self.index_map))

    def generate_index_map(self):
        self.index_map = []
        i = 0
        while i < len(self.data):
            # Map the logical index to the real index
            self.index_map.append(i)
            # Skip to the next instruction
            cmd = self.data[i]
            if cmd == 0x00:
                i += 2
            elif cmd == 0x01:
                i += 3
            elif cmd in (0x02, 0x03):
                i += 1
            elif cmd == 0x04:
                i += 3
            else:
                i += 2


class DRODataV2(DROData):
    def __init__(
        self,
        data: array.array,
        codemap: tuple[int, ...],
        short_delay_code: int,
        long_delay_code: int,
    ) -> None:
        super().__init__(data, short_delay_code, long_delay_code)
        self.codemap = codemap

    def shallow_copy(self, new_data: array.array | None = None) -> "DRODataV2":
        new_copy = DRODataV2(
            new_data if new_data is not None else array.array("B"),
            self.codemap,
            self.short_delay_code,
            self.long_delay_code,
        )
        return new_copy

    def _translate_index(self, key):
        return key * 2

    def _interpret_data(self, real_index):
        cmd = self.data[real_index]
        bank = None
        if cmd == self.short_delay_code:
            inst_type = DROInstructionType.DELAY
            val = self.data[real_index + 1] + 1
        elif cmd == self.long_delay_code:
            inst_type = DROInstructionType.DELAY
            val = (self.data[real_index + 1] + 1) << 8
        else:
            inst_type = DROInstructionType.REGISTER
            bank = (cmd & 0x80) >> 7
            cmd = self.codemap[cmd & 0x7F]
            val = self.data[real_index + 1]

        return DROInstruction(inst_type, cmd, val, bank)

    def __len__(self):
        return len(self.data) // 2

    def _iter_indexes(self):
        return range(len(self.data) // 2)


class DROSong(object):
    """NOTE: this actually implements methods for the V1 file format."""

    OPL_TYPE_MAP = ["OPL-2", "OPL-3", "Dual OPL-2"]

    def __init__(
        self, file_version: int, name: str, data: DROData, ms_length: int, opl_type: int
    ) -> None:
        self.file_version = file_version
        self.name = name
        self.data: DROData = data
        self.ms_length = ms_length
        self.opl_type = opl_type
        self.short_delay_code = 0x00
        self.long_delay_code = 0x01
        self.detailed_register_descriptions: DetailedRegisterInfo | None = None
        self.data_lock = threading.RLock()

    def get_length_ms(self) -> int:
        return self.ms_length

    def get_length_data(self) -> int:
        return len(self.data)

    def find_next_instruction(
        self, start: int, s_inst: str, look_backwards: bool = False
    ) -> int:
        """Takes a starting index and register number (as a hex string) or
        a special value of "DLYS", "DLYL", "DALL", or "BANK", and finds the next
        occurrence of that register after the given index. Returns the index."""

        # This is nuts. Change the comparison test depending on what we're
        #  looking for.
        i = start + (
            -1 if look_backwards else 1
        )  # so we don't get stuck on the currently selected instruction
        look_for: str | int = s_inst
        if s_inst == "DLYS":
            ct = (
                lambda datum, inst: datum.inst_type == DROInstructionType.DELAY
                and datum.command == self.data.short_delay_code
            )
        elif s_inst == "DLYL":
            ct = (
                lambda datum, inst: datum.inst_type == DROInstructionType.DELAY
                and datum.command == self.data.long_delay_code
            )
        elif s_inst == "DALL":
            ct = lambda datum, inst: datum.inst_type == DROInstructionType.DELAY
        elif s_inst == "BANK":
            ct = lambda datum, inst: datum.inst_type == DROInstructionType.BANK_SWITCH
        else:
            ct = (
                lambda datum, inst: datum.inst_type == DROInstructionType.REGISTER
                and datum.command == inst
            )
            look_for = int(s_inst, 16)

        if look_backwards:
            while i >= 0:
                if ct(self.data[i], look_for):
                    return i
                i -= 1
        else:
            while i < len(self.data):
                if ct(self.data[i], look_for):
                    return i
                i += 1

        return -1

    def _insert_instructions(
        self, index_and_value_list: Iterable[tuple[int, array.array]]
    ) -> None:
        """Currently just an internal method, used for undoing deletions.

        (Note to self: if this gets exposed to outside calls, make it
        "undoable" too.)
        """
        with self.data_lock:
            self.data.insert_multiple(index_and_value_list)
        # Keep track of delays inserted, so we can update the total delay count.
        for i, val in index_and_value_list:
            inst = self.data[i]
            if inst.inst_type == DROInstructionType.DELAY:
                self.ms_length += inst.value
        # Also need to update our register descriptions, since the data has changed.
        # This has to be done from outside DROSong, so just clear any existing descriptions.
        self.detailed_register_descriptions = None

    @dro_undo.undoable(
        "Delete Instruction(s)", dro_globals.get_undo_controller, _insert_instructions
    )
    def delete_instructions(
        self, index_list: list[int]
    ) -> list[tuple[int, array.array]]:
        """Deletes instructions at the given indexes.

        Returns a list of tuples, containing the index deleted and the value
        that was stored at that index."""
        # First, copy the data to be deleted.
        deleted_data = []
        index_list.sort()
        for i in index_list:
            # Keep track of delays deleted, so we can update the total delay count.
            inst = self.data[i]
            if inst.inst_type == DROInstructionType.DELAY:
                self.ms_length -= inst.value
            deleted_data.append((i, self.data.get_raw(i)))
        # Now delete each item, in reverse order.
        with self.data_lock:
            self.data.delete_multiple(index_list, is_sorted=True)
        # Also need to update our register descriptions, since the data has changed.
        # This has to be done from outside DROSong, so just clear any existing descriptions.
        self.detailed_register_descriptions = None
        return deleted_data

    def get_register_display(self, item: int) -> str:
        inst = self.data[item]
        if inst.inst_type == DROInstructionType.DELAY:
            if inst.command == self.data.short_delay_code:
                return "DLYS"
            elif inst.command == self.data.long_delay_code:
                return "DLYL"
            else:
                return "???"
        elif inst.inst_type == DROInstructionType.BANK_SWITCH:
            return "BANK"
        else:  # must be a register instruction
            return "0x%02X" % (inst.command,)

    def get_value_display(self, item: int) -> str:
        inst = self.data[item]
        if inst.inst_type == DROInstructionType.DELAY:
            return "%d ms" % (inst.value,)
        elif inst.inst_type == DROInstructionType.BANK_SWITCH:
            return ("low", "high")[inst.value]
        else:  # must be a register instruction
            return "0x%02X (%d)" % (inst.value, inst.value)

    def get_instruction_description(self, item: int) -> str:
        inst = self.data[item]
        if inst.inst_type == DROInstructionType.DELAY:
            if inst.command == self.data.short_delay_code:
                return "Delay (short)"
            elif inst.command == self.data.long_delay_code:
                return "Delay (long)"
            else:
                return "???"
        elif inst.inst_type == DROInstructionType.BANK_SWITCH:
            return "Switch to %s registers (Dual OPL-2 / OPL-3)" % (
                ("low", "high")[inst.value],
            )
        else:  # must be a register instruction
            try:
                reg_desc = regdata.registers[inst.command]
            except KeyError:
                # OPL-3 has some special registers that are only in the high bank
                if inst.bank == 1:
                    try:
                        reg_desc = regdata.registers[0x100 | inst.command]
                    except KeyError:
                        reg_desc = "(unknown)"
                else:
                    reg_desc = "(unknown)"
            return reg_desc

    def get_detailed_register_description(self, item: int) -> str:
        if self.detailed_register_descriptions is None or item >= len(
            self.detailed_register_descriptions
        ):
            return self.get_instruction_description(item)
        else:
            return self.detailed_register_descriptions[item][1]

    def get_bank_description(self, item: int) -> str:
        if self.detailed_register_descriptions is None or item >= len(
            self.detailed_register_descriptions
        ):
            return "?"
        else:
            return str(self.detailed_register_descriptions[item][0])

    def get_index_and_ms_offset_by_position_pct(
        self,
        position_pct: float,
    ) -> tuple[int, int] | None:
        """Given a percentage like 0.5, finds that position in the song by time.
        Rather than going from the start, we start from the index at a similar percentage, e.g. given 100 instructions,
        and position of 80%, start from index 80 (or is it 79? meh)
        We then look backwards or forwards from that index, until we find an index with an ms offset greater/smaller
        than the target offset.
        """
        if not self.detailed_register_descriptions:
            return None
        target_delay = self.ms_length * position_pct
        index = math.floor(len(self.detailed_register_descriptions) * position_pct)
        if index == len(self.detailed_register_descriptions):
            index -= 1
        if 0 > index > len(self.detailed_register_descriptions):
            return None  # Shouldn't normally happen
        item = self.detailed_register_descriptions[index]
        # Not far enough into the song, keep going
        if item[2] < target_delay:
            while (
                index < len(self.detailed_register_descriptions) - 1
                and self.detailed_register_descriptions[index + 1][2] < target_delay
            ):
                index += 1
        # Too far, go back a bit
        elif item[2] > target_delay:
            while (
                index > 0
                and self.detailed_register_descriptions[index - 1][2] > target_delay
            ):
                index -= 1
        return index, self.detailed_register_descriptions[index][2]

    def __str__(self) -> str:
        return (
            "DRO[name = '%s', ver = '%s', opl_type = '%s' (%s), ms_length = '%s']"
            % (
                self.name,
                self.file_version,
                self.opl_type,
                self.OPL_TYPE_MAP[self.opl_type],
                self.ms_length,
            )
        )

    def pretty_string(self) -> str:
        pstr = (
            "DRO Song: %(name)s\n"
            "Format: v%(file_version)s\n"
            "OPL Type: %(opl_type)s\n"
            "Length (ms): %(ms_length)s"
        ) % {
            "name": self.name,
            "file_version": self.file_version,
            "opl_type": self.OPL_TYPE_MAP[self.opl_type],
            "ms_length": self.ms_length,
        }
        return pstr


class DROSongV2(DROSong):
    OPL_TYPE_MAP = [
        "OPL-2",
        "Dual OPL-2",
        "OPL-3",
    ]

    data: DRODataV2

    def __init__(
        self,
        file_version: int,
        name: str,
        data: DRODataV2,
        ms_length: int,
        opl_type: int,
        short_delay_code: int,
        long_delay_code: int,
    ) -> None:
        super().__init__(file_version, name, data, ms_length, opl_type)
        self.short_delay_code = short_delay_code
        self.long_delay_code = long_delay_code
