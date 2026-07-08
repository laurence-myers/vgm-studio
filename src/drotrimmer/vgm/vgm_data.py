import array
import math
import re
from dataclasses import dataclass
from typing import Literal, Iterable

from ..dro_data import (
    DROData,
    DROInstruction,
    DROInstructionType,
    AbstractSong,
    OPLType,
    SongFileType,
)
from ..dro_util import DROTrimmerException, smp_to_ms

_VGM_CONVERSION_VERSION = 0x00000151


@dataclass
class GD3Tag:
    track_name_en: str
    track_name_native: str
    game_name_en: str
    game_name_native: str
    system_name_en: str
    system_name_native: str
    track_author_en: str
    track_author_native: str
    release_date: str
    creator: str
    notes: str

    def iter_fields(self) -> Iterable[str]:
        yield self.track_name_en
        yield self.track_name_native
        yield self.game_name_en
        yield self.game_name_native
        yield self.system_name_en  # TODO: maybe derive this from AbstractSong.opl_type
        yield self.system_name_native
        yield self.track_author_en
        yield self.track_author_native
        yield self.release_date
        yield self.creator
        yield self.notes


class VGMData(DROData):
    def __init__(self, data: array.array) -> None:
        super().__init__(data)
        self._generate_offsets()

    def _generate_offsets(self) -> None:
        offsets = []
        i = 0
        while i < len(self.data):
            offsets.append(i)
            command = self.data[i]
            match command:
                case 0x5A:
                    """aa dd YM3812, write value dd to register aa"""
                    i += 3
                case 0x5E:
                    """aa dd	YMF262 port 0, write value dd to register aa"""
                    i += 3
                case 0x5F:
                    """aa dd	YMF262 port 1, write value dd to register aa"""
                    i += 3
                case 0x61:
                    """nn nn	Wait n samples, n can range from 0 to 65535 (approx 1.49 seconds).
                    Longer pauses than this are represented by multiple wait commands.
                    """
                    i += 3
                case 0x62:
                    """wait 735 samples (60th of a second), a shortcut for 0x61 0xdf 0x02"""
                    i += 1
                case 0x63:
                    """wait 882 samples (50th of a second), a shortcut for 0x61 0x72 0x03"""
                    i += 1
                case wait if 0x70 <= wait <= 0x7F:
                    """wait n+1 samples, n can range from 0 to 15."""
                    i += 1
                case 0xAA:
                    """aa dd YM3812, write value dd to register aa (chip #2)"""
                    i += 3
                case _:
                    raise DROTrimmerException(
                        f"Unsupported VGM command: {hex(command)}"
                    )
        self._offsets = offsets

    def delete_multiple(self, index_list: list[int]) -> None:
        super().delete_multiple(index_list)
        self._generate_offsets()

    def insert_multiple(
        self, i_and_val_list: Iterable[tuple[int, array.array]]
    ) -> None:
        real_offset = 0
        for num_inserted, (i, val) in enumerate(i_and_val_list):
            real_index = self._translate_index(i - num_inserted) + real_offset
            self.data[real_index:real_index] = val
            real_offset += len(val)
        self._generate_offsets()

    def _translate_index(self, key: int) -> int:
        return self._offsets[key]

    def _interpret_data(self, real_index: int) -> DROInstruction:
        cmd = self.data[real_index]
        bank: Literal[0, 1] | None = None
        match cmd:
            case 0x5A:
                """aa dd YM3812, write value dd to register aa"""
                inst_type = DROInstructionType.REGISTER
                bank = 0
                cmd = self.data[real_index + 1]
                val = self.data[real_index + 2]
            case 0x5E:
                """aa dd	YMF262 port 0, write value dd to register aa"""
                inst_type = DROInstructionType.REGISTER
                bank = 0
                cmd = self.data[real_index + 1]
                val = self.data[real_index + 2]
            case 0x5F:
                """aa dd	YMF262 port 1, write value dd to register aa"""
                inst_type = DROInstructionType.REGISTER
                bank = 1
                cmd = self.data[real_index + 1]
                val = self.data[real_index + 2]
            case 0x61:
                """nn nn	Wait n samples, n can range from 0 to 65535 (approx 1.49 seconds).
                Longer pauses than this are represented by multiple wait commands."""
                inst_type = DROInstructionType.DELAY_SMP
                val = self.data[real_index + 1] | (self.data[real_index + 2] << 8)
            case 0x62:
                """wait 735 samples (60th of a second), a shortcut for 0x61 0xdf 0x02"""
                inst_type = DROInstructionType.DELAY_SMP
                val = 735
            case 0x63:
                """wait 882 samples (50th of a second), a shortcut for 0x61 0x72 0x03"""
                inst_type = DROInstructionType.DELAY_SMP
                val = 882
            case wait if 0x70 <= wait <= 0x7F:
                """wait n+1 samples, n can range from 0 to 15."""
                inst_type = DROInstructionType.DELAY_SMP
                val = (cmd & 0x0F) + 1
            case 0xAA:
                """aa dd YM3812, write value dd to register aa (chip #2)"""
                inst_type = DROInstructionType.REGISTER
                bank = 1
                cmd = self.data[real_index + 1]
                val = self.data[real_index + 2]
            case _:
                raise DROTrimmerException(f"Unsupported VGM command: {hex(cmd)}")
        return DROInstruction(inst_type, cmd, val, bank)

    def __len__(self) -> int:
        return len(self._offsets)

    def _iter_indexes(self) -> Iterable[int]:
        return range(len(self._offsets))

    def shallow_copy(self, new_data: array.array | None = None) -> "VGMData":
        new_copy = VGMData(
            new_data if new_data is not None else array.array("B"),
        )
        return new_copy

    def is_long_delay(self, command: int) -> bool:
        return command == 0x61

    def is_short_delay(self, command: int) -> bool:
        return command == 0x62 or command == 0x63 or 0x70 <= command <= 0x7F


class VGMSong(AbstractSong):
    data: VGMData

    @staticmethod
    def from_song(dro_song: AbstractSong) -> "VGMSong":
        data = array.array("B")
        bank = 0
        total_samples = 0
        cumulative_ms = 0
        for inst in dro_song.data:
            match inst.inst_type:
                case DROInstructionType.BANK_SWITCH:
                    bank = inst.value  # DRO v1
                case DROInstructionType.DELAY_MS:
                    # Convert the *running total* of milliseconds to samples,
                    #  rounding half up, and emit the difference. Rounding each
                    #  delay on its own drifts: dro2vgm turns two identical 16 ms
                    #  delays into 706 and 705 samples, which no per-delay rounding
                    #  can produce.
                    # NOTE: this used to emit delays of 15 ms or less as
                    #  "0x70 | ms", which waits ms + 1 *samples* (about a 300th of
                    #  the intended time), and counted those milliseconds as samples
                    #  in the total. It also dropped the repeated 0x61 opcode when a
                    #  wait exceeded 65535 samples, so the extra pair was decoded as
                    #  commands and corrupted the stream.
                    cumulative_ms += inst.value
                    new_total = (cumulative_ms * 44100 + 500) // 1000
                    samples = new_total - total_samples
                    total_samples = new_total
                    while True:
                        chunk = min(samples, 0xFFFF)
                        data.append(0x61)
                        data.append(chunk & 0xFF)
                        data.append((chunk & 0xFF00) >> 8)
                        samples -= chunk
                        if samples == 0:
                            break
                case DROInstructionType.REGISTER:
                    # Support both DRO v1 (separate bank switch commands)
                    # and DRO v2 (bank is interpreted by DROInstruction)
                    inner_bank = inst.bank if inst.bank is not None else bank
                    match dro_song.opl_type:
                        case OPLType.OPL2:
                            command = 0x5A
                        case OPLType.DUAL_OPL2:
                            command = 0xAA if inner_bank == 1 else 0x5A
                        case OPLType.OPL3:
                            command = 0x5F if inner_bank == 1 else 0x5E
                        case _:
                            raise DROTrimmerException(
                                f"Unexpected OPLType: {dro_song.opl_type}"
                            )
                    data.append(command)  # VGM Chip
                    data.append(inst.command)  # Register
                    data.append(inst.value)
                case _:
                    raise DROTrimmerException(
                        f"Unexpected DROInstructionType: {inst.inst_type}"
                    )
        return VGMSong(
            _VGM_CONVERSION_VERSION,
            # Replace .dro, or any other extension, with .vgm
            re.sub(r"\..{3,4}$", ".vgm", dro_song.name, flags=re.IGNORECASE),
            VGMData(data),
            dro_song.opl_type,
            total_samples,
            loop_offset=0,
            loop_modifier=0,
            loop_num_samples=0,
            tag=None,
            loop_base=0,
            volume_modifier=0,
        )

    def __init__(
        self,
        file_version: int,
        name: str,
        data: VGMData,
        opl_type: OPLType,
        total_samples: int,
        loop_offset: int,
        loop_num_samples: int,
        loop_modifier: int,
        tag: GD3Tag | None,
        loop_base: int,
        volume_modifier: int,
    ) -> None:
        super().__init__(
            SongFileType.VGM,
            file_version,
            name,
            data,
            math.ceil(total_samples // 44.1),
            opl_type,
        )
        self.loop_base = loop_base
        self.loop_modifier = loop_modifier
        self.loop_num_samples = loop_num_samples
        self.loop_offset = loop_offset
        self.tag = tag
        self.total_samples = total_samples
        self.volume_modifier = volume_modifier
