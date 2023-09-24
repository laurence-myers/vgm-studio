import array
from dataclasses import dataclass
from typing import Literal, Iterable

from ..dro_data import DROData, DROInstruction, DROInstructionType
from ..dro_util import DROTrimmerException

OPL_TYPE_MAP = ["OPL-2", "Dual OPL-2", "OPL-3"]


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
    release_date: str  # should this be a struct?
    creator: str
    notes: str


class VGMData(DROData):
    def __init__(self, data: array.array, offsets: list[int]) -> None:
        super().__init__(data)
        self._offsets = offsets

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
                inst_type = DROInstructionType.DELAY
                # TODO: convert samples to ms
                val = (self.data[real_index + 1] << 8) + self.data[real_index + 1]
            case 0x62:
                """wait 735 samples (60th of a second), a shortcut for 0x61 0xdf 0x02"""
                inst_type = DROInstructionType.DELAY
                val = 1000 // 60
            case 0x63:
                """wait 882 samples (50th of a second), a shortcut for 0x61 0x72 0x03"""
                inst_type = DROInstructionType.DELAY
                val = 1000 // 50
            case wait if 0x70 <= wait <= 0x7F:
                """wait n+1 samples, n can range from 0 to 15."""
                inst_type = DROInstructionType.DELAY
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
            offsets=[],  # TODO: generate offsets
        )
        return new_copy

    def is_long_delay(self, command: int) -> bool:
        return command == 0x61

    def is_short_delay(self, command: int) -> bool:
        return command == 0x62 or command == 0x63 or 0x70 <= command <= 0x7F


class VGMSong:
    def __init__(
        self,
        file_version: int,
        name: str,
        data: VGMData,
        opl_type: Literal[0, 1, 2],
        total_samples: int,
        loop_offset: int,
        loop_num_samples: int,
        loop_modifier: int,
        tag: GD3Tag | None,
    ) -> None:
        self.data = data
        self.file_version = file_version
        self.loop_modifier = loop_modifier
        self.loop_num_samples = loop_num_samples
        self.loop_offset = loop_offset
        self.name = name
        self.opl_type = opl_type
        self.tag = tag
        self.total_samples = total_samples
