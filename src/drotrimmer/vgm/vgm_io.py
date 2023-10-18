import array
import math
import struct
from io import BytesIO
from typing import BinaryIO, Literal

from .vgm_data import VGMSong, GD3Tag, VGMData
from ..dro_analysis import DROTotalDelayCalculator
from ..dro_data import OPLType
from ..dro_util import (
    DROFileException,
    read_int,
    read_char,
    write_int,
    write_char,
    DROTrimmerException,
)

ChipBank = Literal[0, 1]

_CLOCK_OPL2 = 3579545
_CLOCK_DUAL_OPL2 = (
    3579545
    | 0xC0000000  # Spec suggests high bits should be 0x40..., but dro2vgm uses 0xC0...
)
_CLOCK_OPL3 = 14318180
_DUAL_CHIP_FLAG = 0x40000000
_GD3_ENCODING = "utf-16-le"
_GD3_HEADER = b"Gd3 "
_GD3_NULL_TERMINATOR = b"\x00\x00"
_GD3_SUPPORTED_VERSION = 0x00000100
_MINIMUM_SUPPORTED_VERSION = 0x00000151
_VGM_HEADER = b"Vgm "
_VGM_HEADER_OFFSETS = {
    "magic_string": 0x00,
    "eof": 0x04,
    "version": 0x08,
    "gd3": 0x14,
    "total_samples": 0x18,
    "loop_offset": 0x22,
    "loop_num_samples": 0x26,
    "data_offset": 0x34,
    "ym3812_clock": 0x50,
    "ym262_clock": 0x5C,
    # 'volume_modifier': 0x7C,
    # 'loop_base': 0x7E,
    "loop_modifier": 0x7F,
}


def _read_commands(in_file: BinaryIO) -> array.array:
    data = array.array("B")
    while raw := in_file.read(1):
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
    return data


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
    string_blob: str = vgm_file.read(data_length).decode(_GD3_ENCODING)
    # Tag entries are null-terminated, using two byte characters.
    # The encoding is not specified. I have chosen to only support utf-16-le, this seems to be what vgm_tag uses.
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
        _,  # Using .split() gives us one extra empty string, we ignore it
    ) = string_blob.split("\0")
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


def write_gd3_tag(gd3_tag: GD3Tag) -> bytes:
    buffer = BytesIO()
    buffer.write(_GD3_HEADER)
    write_int(buffer, _GD3_SUPPORTED_VERSION)
    header_size_offset = buffer.tell()
    write_int(buffer, 0)
    for field_value in gd3_tag.iter_fields():
        buffer.write(field_value.encode(_GD3_ENCODING))
        buffer.write(_GD3_NULL_TERMINATOR)
    header_size = buffer.tell() - header_size_offset - 4
    buffer.seek(header_size_offset)
    write_int(buffer, header_size)
    out = buffer.getvalue()
    buffer.close()
    return out


class VgmFileIO:
    """Reads or writes VGM data from/to a file."""

    def read(self, file_name: str) -> VGMSong:
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
            vgm_file.seek(_VGM_HEADER_OFFSETS["gd3"])
            gd3_offset = read_int(vgm_file)
            if gd3_offset:
                gd3_offset += _VGM_HEADER_OFFSETS["gd3"]
            vgm_file.seek(_VGM_HEADER_OFFSETS["total_samples"])
            total_samples = read_int(vgm_file)
            loop_offset = read_int(vgm_file)
            loop_num_samples = read_int(vgm_file)
            vgm_file.seek(_VGM_HEADER_OFFSETS["data_offset"])
            vgm_data_offset = read_int(vgm_file) + _VGM_HEADER_OFFSETS["data_offset"]
            vgm_file.seek(_VGM_HEADER_OFFSETS["ym3812_clock"])
            ym3812_clock_and_dual_bit = read_int(vgm_file)
            is_dual_opl2: bool = bool(ym3812_clock_and_dual_bit & _DUAL_CHIP_FLAG)
            ym3812_clock = ym3812_clock_and_dual_bit & ~_DUAL_CHIP_FLAG
            vgm_file.seek(_VGM_HEADER_OFFSETS["ym262_clock"])
            ymf262_clock = read_int(vgm_file) & ~_DUAL_CHIP_FLAG  # only support 1 chip
            # 0x7C = volume modifier, 1 byte, v1.60
            # 0x7E = loop base, 1 byte, v1.60
            vgm_file.seek(_VGM_HEADER_OFFSETS["loop_modifier"])
            loop_modifier = read_char(vgm_file)
            # 0xBC = extra header offset, 4 bytes, v1.70
            opl_type: OPLType | None = None
            if is_dual_opl2:
                opl_type = OPLType.DUAL_OPL2
            elif bool(ym3812_clock):
                opl_type = OPLType.OPL2
            elif bool(ymf262_clock):
                opl_type = OPLType.OPL3

            if opl_type is None:
                raise DROFileException("No OPL2 or OPL3 data detected.")

            # Read the data
            vgm_file.seek(vgm_data_offset)
            data = _read_commands(vgm_file)

            # Read the GD3 tag
            if gd3_offset:
                vgm_file.seek(gd3_offset)
                tag = parse_gd3_tag(vgm_file)
            else:
                tag = None

            return VGMSong(
                file_version=version,
                name=file_name,
                data=VGMData(data),
                opl_type=opl_type,
                total_samples=total_samples,
                loop_offset=loop_offset,
                loop_num_samples=loop_num_samples,
                loop_modifier=loop_modifier,
                tag=tag,
            )

    def write(self, dro_song: VGMSong) -> None:
        length_ms = DROTotalDelayCalculator().sum_delay(dro_song)
        gd3_tag = write_gd3_tag(dro_song.tag) if dro_song.tag else None
        with open(dro_song.name, "wb") as vgm_file:
            header_size = 0xFF
            vgm_file.write(b"\x00" * header_size)

            vgm_file.seek(_VGM_HEADER_OFFSETS["magic_string"])
            vgm_file.write(_VGM_HEADER)

            vgm_file.seek(_VGM_HEADER_OFFSETS["eof"])
            gd3_size = len(gd3_tag) if gd3_tag else 0
            end_of_data_marker_size = 1
            eof = (
                header_size
                + dro_song.data.raw_len()
                + end_of_data_marker_size
                + gd3_size
                + 1  # needs a little bit extra
            )
            write_int(vgm_file, eof - _VGM_HEADER_OFFSETS["eof"])

            vgm_file.seek(_VGM_HEADER_OFFSETS["version"])
            version = 0x00000151
            write_int(vgm_file, version)

            if gd3_tag:
                vgm_file.seek(_VGM_HEADER_OFFSETS["gd3"])
                write_int(vgm_file, eof - gd3_size - _VGM_HEADER_OFFSETS["gd3"])

            vgm_file.seek(_VGM_HEADER_OFFSETS["data_offset"])
            data_offset = 0x100
            write_int(vgm_file, data_offset - _VGM_HEADER_OFFSETS["data_offset"])

            match dro_song.opl_type:
                case OPLType.OPL2:
                    vgm_file.seek(_VGM_HEADER_OFFSETS["ym3812_clock"])
                    write_int(vgm_file, _CLOCK_OPL2)
                case OPLType.DUAL_OPL2:
                    vgm_file.seek(_VGM_HEADER_OFFSETS["ym3812_clock"])
                    write_int(vgm_file, _CLOCK_DUAL_OPL2)
                case OPLType.OPL3:
                    vgm_file.seek(_VGM_HEADER_OFFSETS["ym262_clock"])
                    write_int(vgm_file, _CLOCK_OPL3)
                case _:
                    raise DROTrimmerException(
                        f"Unrecognised OPL chip type: {dro_song.opl_type}"
                    )

            # TODO: sum samples, don't sum it from ms
            # TODO: investigate and fix difference in samples from original
            vgm_file.seek(_VGM_HEADER_OFFSETS["total_samples"])
            write_int(vgm_file, math.ceil(length_ms * 44.1))

            vgm_file.seek(header_size + 1)  # go to end of header
            dro_song.data.tofile(vgm_file)
            write_char(vgm_file, 0x66)  # end of sound data

            # Write the GD3 tag after the data
            if gd3_tag:
                vgm_file.write(gd3_tag)

            # TODO: investigate diffs:
            #  0x24: rate, 0x230 (560). dro2vgm seems to write it as 1000 (0x3E8)
