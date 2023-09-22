import array
from dataclasses import dataclass
from typing import Literal

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


class VGMData:
    pass


class VGMSong:
    def __init__(
        self,
        file_version: int,
        name: str,
        data: array.array,
        instruction_offsets: list[int],
        opl_type: Literal[0, 1, 2],
        total_samples: int,
        loop_offset: int,
        loop_num_samples: int,
        loop_modifier: int,
        tag: GD3Tag | None,
    ) -> None:
        self.data = data
        self.file_version = file_version
        self.instruction_offsets = instruction_offsets
        self.loop_modifier = loop_modifier
        self.loop_num_samples = loop_num_samples
        self.loop_offset = loop_offset
        self.name = name
        self.opl_type = opl_type
        self.tag = tag
        self.total_samples = total_samples
