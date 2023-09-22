import array
import struct
from typing import BinaryIO, Literal

from .vgm_data import VGMSong, GD3Tag
from ..dro_util import DROFileException, read_int, read_char

ChipBank = Literal[0, 1]
OplType = Literal[0, 1, 2]

_DUAL_CHIP_FLAG = 0x40000000
_GD3_HEADER = b"Gd3 "
_GD3_SUPPORTED_VERSION = 0x00000100
_MINIMUM_SUPPORTED_VERSION = 0x00000151
_VGM_HEADER = b"Vgm "


def _read_commands(in_file: BinaryIO) -> tuple[array.array, list[int]]:
    offsets = []
    data = array.array("B")
    while raw := in_file.read(1):
        offsets.append(len(data))
        command = struct.unpack("<B", raw)[0]
        match command:
            case 0x5A:
                """aa dd YM3812, write value dd to register aa"""
                data.append(command)
                data.fromfile(in_file, 2)
            case 0x5E:
                """aa dd	YMF262 port 0, write value dd to register aa"""
                data.append(command)
                data.fromfile(in_file, 2)
            case 0x5F:
                """aa dd	YMF262 port 1, write value dd to register aa"""
                data.append(command)
                data.fromfile(in_file, 2)
            case 0x61:
                """nn nn	Wait n samples, n can range from 0 to 65535 (approx 1.49 seconds).
                Longer pauses than this are represented by multiple wait commands."""
                data.append(command)
                data.fromfile(in_file, 2)
            case 0x62:
                """wait 735 samples (60th of a second), a shortcut for 0x61 0xdf 0x02"""
                data.append(command)
            case 0x63:
                """wait 882 samples (50th of a second), a shortcut for 0x61 0x72 0x03"""
                data.append(command)
            case 0x66:
                """end of sound data"""
                break
            case wait if 0x70 <= wait <= 0x7F:
                """wait n+1 samples, n can range from 0 to 15."""
                data.append(command)
            case 0xAA:
                """aa dd YM3812, write value dd to register aa (chip #2)"""
                data.append(command)
                data.fromfile(in_file, 2)
            case _:
                raise DROFileException(f"Unsupported VGM command: {hex(command)}")
    return (data, offsets)


def parse_gd3_tag(vgm_file: BinaryIO) -> GD3Tag:
    header_name = vgm_file.read(4)
    if header_name != _GD3_HEADER:
        raise DROFileException(
            "Does not appear to be a GD3 tag (invalid header. Expected %s, found %s)."
            % (_GD3_HEADER.decode("ascii"), header_name.decode("ascii"))
        )
    version = read_int(vgm_file)
    if version != _GD3_SUPPORTED_VERSION:
        raise DROFileException("Unsupported GD3 version, only v1.00 is supported.")
    data_length = read_int(vgm_file)
    string_blob: bytes = vgm_file.read(data_length)
    # Tag entries are null-terminated, using two byte characters.
    # The encoding is not specified. I have chosen to only support utf-16.
    (
        track_name_en,
        track_name_native,
        game_name_en,
        game_name_native,
        system_name_en,
        system_name_native,
        track_author_en,
        track_author_native,
        release_date,
        creator,
        notes,
    ) = [entry.decode("utf-16") for entry in string_blob.split(b"\x00\x00")]
    return GD3Tag(
        track_name_en,
        track_name_native,
        game_name_en,
        game_name_native,
        system_name_en,
        system_name_native,
        track_author_en,
        track_author_native,
        release_date,
        creator,
        notes,
    )


class VgmFileIO:
    """Reads or writes VGM data from/to a file."""

    def read_data(self, file_name: str) -> VGMSong:
        with open(file_name, "rb") as vgm_file:
            header_name = vgm_file.read(4)
            if header_name != _VGM_HEADER:
                raise DROFileException(
                    "Does not appear to be a VGM file (invalid header. Expected %s, found %s)."
                    % (_VGM_HEADER.decode("ascii"), header_name.decode("ascii"))
                )
            _eof: int = read_int(vgm_file) + 4  # file length - 4
            version: int = read_int(vgm_file)  # BCD like 0x00000171
            if version < _MINIMUM_SUPPORTED_VERSION:
                raise DROFileException(
                    "Unsupported VGM version, v1.51 is the minimum supported version."
                )
            vgm_file.seek(0x14)
            gd3_offset = read_int(vgm_file)
            if gd3_offset:
                gd3_offset += 0x14
            vgm_file.seek(0x18)
            total_samples = read_int(vgm_file)
            loop_offset = read_int(vgm_file)
            loop_num_samples = read_int(vgm_file)
            vgm_file.seek(0x34)
            vgm_data_offset = read_int(vgm_file) + 0x34
            vgm_file.seek(0x50)
            ym3812_clock_and_dual_bit = read_int(vgm_file)
            is_dual_opl2: bool = bool(ym3812_clock_and_dual_bit & _DUAL_CHIP_FLAG)
            ym3812_clock = ym3812_clock_and_dual_bit & ~_DUAL_CHIP_FLAG
            vgm_file.seek(0x5C)
            ymf262_clock = read_int(vgm_file) & ~_DUAL_CHIP_FLAG  # only support 1 chip
            # 0x7C = volume modifier, 1 byte, v1.60
            # 0x7E = loop base, 1 byte, v1.60
            vgm_file.seek(0x7F)
            loop_modifier = read_char(vgm_file)
            # 0xBC = extra header offset, 4 bytes, v1.70
            opl_type: OplType | None = None
            if is_dual_opl2:
                opl_type = 1
            elif bool(ym3812_clock):
                opl_type = 0
            elif bool(ymf262_clock):
                opl_type = 2

            if opl_type is None:
                raise DROFileException("No OPL2 or OPL3 data detected.")

            # Read the data
            vgm_file.seek(vgm_data_offset)
            (data, instruction_offsets) = _read_commands(vgm_file)

            # Read the GD3 tag
            if gd3_offset:
                vgm_file.seek(gd3_offset)
                tag = parse_gd3_tag(vgm_file)
            else:
                tag = None

            return VGMSong(
                file_version=version,
                name=file_name,
                data=data,
                instruction_offsets=instruction_offsets,
                opl_type=opl_type,
                total_samples=total_samples,
                loop_offset=loop_offset,
                loop_num_samples=loop_num_samples,
                loop_modifier=loop_modifier,
                tag=tag,
            )

    def write_data(self, dro_song: VGMSong) -> None:
        pass
