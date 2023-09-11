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

from .dro_data import (
    DRO_FILE_V1,
    DRO_FILE_V2,
    DROSong,
    DROSongV2,
    DRODataV1,
    DRODataV2,
    DROInstructionType,
)
from .dro_util import *

DRO_HEADER = b"DBRAWOPL"
DRO_VERSION_V1_OLD = (1, 0)
DRO_VERSION_V1_NEW = (
    0,
    1,
)  # the DOSBox devs really screwed the versioning up, didn't they?
DRO_VERSION_V2 = (2, 0)

# This var is just for backwards compatability - it seems old versions of DRO
#  files wrote the OPL type as a char, whereas newer versions write it as a 4-byte
#  int. This program supports either, but it's hard coded.
WRITE_CHAR_OPL = False


class DroFileIO(object):
    def read(self, file_name: str) -> DROSong:
        """Accepts a file name (string). Returns a DROSong object and whether it was auto-trimmed (boolean).

        Raises DROFileException on invalid file data/version."""
        with open(file_name, "rb") as drof:
            header_name = drof.read(8)
            if header_name != DRO_HEADER:
                raise DROFileException(
                    "Does not appear to be a DRO file (invalid header. Expected %s, found %s)."
                    % (DRO_HEADER.decode("ascii"), header_name.decode("ascii"))
                )

            header_version = struct.unpack("<2H", drof.read(4))
            if header_version in (DRO_VERSION_V1_OLD, DRO_VERSION_V1_NEW):
                reader: DroFileIOv1 | DroFileIOv2 = DroFileIOv1()
            elif header_version == DRO_VERSION_V2:
                reader = DroFileIOv2()
            else:
                raise DROFileException(
                    "Unsupported version of the DRO file format. Supported: v1 or v2. Found: %s"
                    % (header_version,)
                )

            dro_song = reader.read_data(file_name, drof)
            return dro_song

    def write(self, file_name: str, dro_song: DROSong | DROSongV2) -> None:
        with open(file_name, "wb") as drof:
            drof.write(DRO_HEADER)
            if dro_song.file_version == DRO_FILE_V1:
                writer_v1: DroFileIOv1 = DroFileIOv1()
                drof.write(
                    struct.pack("<2H", *DRO_VERSION_V1_NEW)
                )  # hmm, maybe shouldn't be here
                writer_v1.write_data(drof, dro_song)
            elif dro_song.file_version == DRO_FILE_V2 and isinstance(
                dro_song, DROSongV2  # keep mypy happy
            ):
                writer_v2: DroFileIOv2 = DroFileIOv2()
                drof.write(
                    struct.pack("<2H", *DRO_VERSION_V2)
                )  # hmm, maybe shouldn't be here
                writer_v2.write_data(drof, dro_song)
            else:
                # Should never get here.
                raise DROFileException(
                    "Tried to save an unsupported version of the DRO file format. Support v1 or v2, found: %s"
                    % (dro_song.file_version,)
                )


class DroFileIOv1(object):
    def read_data(self, file_name: str, drof: BinaryIO) -> DROSong:
        """Accepts an open DRO file. Returns a DROSong object and whether it was auto-trimmed (boolean).

        Raises DROFileException on invalid file data/version."""
        # Code interpreted from the adplug source code.
        dro_byte_length = 0
        dro_ms_length = 0
        dro_opl_type = 0

        # Actually load some data
        dro_ms_length = read_int(drof)  # Total milliseconds in file (not used)
        dro_byte_length = read_int(drof)  # Total data bytes in file (not used)

        # Looking at the samurai.dro file in the adplug testing dir, it uses a char for
        #  the OPL type, but my rips use words.
        #  Looks like there's two different file formats, with the same version number.
        dro_opl_type = read_int(drof)  # Type of opl data this can contain

        # To avoid the char/word problem, we'll just assume if the word we read in is
        #  too large (say, more than 0xFF), we probably meant to read a char, so go back
        #  and try again. Obviously this will cause problems if for some reason the DOSBox
        #  guys want to use an opl_type of e.g. 1893647, but I think that's unlikely.
        if dro_opl_type > 0xFF:
            drof.seek(-4, 1)
            dro_opl_type = read_char(drof)

        raw_data = array.array("B")
        raw_data.fromfile(drof, dro_byte_length)
        dro_data = DRODataV1(raw_data)
        dro_data.generate_index_map()

        # If we haven't reached the EOF we must have an error somewhere in the code.
        m = drof.read(1)
        if m != "":
            raise DROFileException(
                "Tried to read the specified number of bytes in the data stream, but there were some bytes left over!"
            )

        return DROSong(DRO_FILE_V1, file_name, dro_data, dro_ms_length, dro_opl_type)

    def write_data(self, drof: BinaryIO, dro_song: DROSong) -> None:
        """Accepts a file name (string), and a DROSong object. Saves the DROSong
        data to a file."""

        header_start = drof.tell()

        self.write_header(drof, 0, 0, 0)  # write a dummy header

        total_size = dro_song.data.raw_len()
        total_delay = 0
        # Would be nice to use the DROTotalDelayCalculator, but that
        # introduces a circular import.
        # (Why don't we use the value stored in the dro_song object? Seems
        #  to be a discrepancy between how V1 and V2 files write this value)
        for inst in dro_song.data:
            if inst.inst_type == DROInstructionType.DELAY:
                total_delay += inst.value
        dro_song.data.tofile(drof)

        # rewind and rewrite the header
        drof.seek(header_start)
        self.write_header(drof, total_delay, total_size, dro_song.opl_type)

        print(
            "DRO file saved. total_delay: "
            + str(total_delay)
            + " total_size: "
            + str(total_size)
        )

    def write_header(
        self,
        in_f: BinaryIO,
        length: int,
        size: int,
        opl_type: int,
    ) -> None:
        write_int(in_f, length)
        write_int(in_f, size)
        if WRITE_CHAR_OPL:  # I guess for backwards compatibility?
            write_char(in_f, opl_type)
        else:
            write_int(in_f, opl_type)


class DroFileIOv2(object):
    def read_data(self, file_name: str, drof: BinaryIO) -> DROSongV2:
        (
            iLengthPairs,
            iLengthMS,
            iHardwareType,
            iFormat,
            iCompression,
            iShortDelayCode,
            iLongDelayCode,
            iCodemapLength,
        ) = struct.unpack("<2L6B", drof.read(14))
        codemap = struct.unpack(str(iCodemapLength) + "B", drof.read(iCodemapLength))
        if iFormat != 0:
            raise DROFileException(
                "Unsupported DRO v2 format. Only 0 is supported, found format ID %s"
                % iFormat
            )
        if iCompression != 0:
            raise DROFileException(
                "Unsupported DRO v2 compression. Only 0 is supported, found compression ID %s"
                % iFormat
            )
        if len(codemap) > 128:
            raise DROFileException(
                "DRO v2 file has too many entries in the codemap. Maximum 128, found %s. Is the file corrupt?"
                % len(codemap)
            )

        raw_data = array.array("B")
        raw_data.fromfile(drof, iLengthPairs * 2)
        dro_data = DRODataV2(
            raw_data,
            codemap,
            iShortDelayCode,
            iLongDelayCode,
        )

        # NOTE: iHardwareType value is different compared to V1. Really should cater for it better by converting to another value.
        return DROSongV2(
            DRO_FILE_V2,
            file_name,
            dro_data,
            iLengthMS,
            iHardwareType,
            iShortDelayCode,
            iLongDelayCode,
        )

    def write_data(self, drof: BinaryIO, dro_song: DROSongV2) -> None:
        # Write the header
        drof.write(
            struct.pack(
                "<2L6B",
                len(dro_song.data),  # length in reg/val pairs
                dro_song.ms_length,  # length in MS
                dro_song.opl_type,  # hardware type
                0,  # format
                0,  # compression
                dro_song.short_delay_code,
                dro_song.long_delay_code,
                len(dro_song.data.codemap),  # length of codemap
            )
        )
        # Write the codemap
        drof.write(
            struct.pack(str(len(dro_song.data.codemap)) + "B", *dro_song.data.codemap)
        )
        # Write the data
        dro_song.data.tofile(drof)
