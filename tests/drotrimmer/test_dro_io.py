import array
import struct
import tempfile
from pathlib import Path
from typing import cast
from unittest import TestCase

from src.drotrimmer.dro_data import DRODataV2, OPLType
from src.drotrimmer.dro_io import DRO_HEADER, DROSongV1, DROSongV2, DroFileIO
from src.drotrimmer.dro_util import DROFileException

FIXTURE_DRO2 = Path(__file__).parent.parent / "lsl3_score_up_dro2.dro"

# A hand-built DRO v1 file: a register write, a short delay, a long delay, both
#  bank switches, an escaped register write, and one more register write.
V1_DATA = bytes(
    (
        0x20,
        0x01,  # register 0x20 = 0x01
        0x00,
        0xB0,  # short delay: 0xB0 + 1 = 177 ms
        0x01,
        0x34,
        0x12,  # long delay: 0x1234 + 1 = 4661 ms
        0x02,  # bank switch, low
        0x03,  # bank switch, high
        0x04,
        0x01,
        0xFF,  # escaped register 0x01 = 0xFF
        0xBD,
        0x20,  # register 0xBD = 0x20
    )
)
V1_MS_LENGTH = 177 + 4661


def build_dro1(char_opl_type: bool = False, trailing: bytes = b"") -> bytes:
    """Old rips wrote the OPL type as one byte; newer ones use four."""
    out = bytearray(DRO_HEADER)
    out += struct.pack("<2H", 0, 1)  # DRO_VERSION_V1_NEW
    out += struct.pack("<L", V1_MS_LENGTH)
    out += struct.pack("<L", len(V1_DATA))
    out += struct.pack("<B" if char_opl_type else "<L", 0)  # OPL2
    out += V1_DATA
    out += trailing
    return bytes(out)


class TestDroFileIOv1(TestCase):
    """Before the `if m != ""` fix, every one of these raised DROFileException:
    `drof.read(1)` returns `bytes`, and `b"" != ""` is True in Python 3."""

    def _read(self, contents: bytes) -> DROSongV1 | DROSongV2:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "test.dro"
            path.write_bytes(contents)
            return DroFileIO().read(str(path))

    def test_read_dro1(self) -> None:
        dro_song = self._read(build_dro1())
        self.assertEqual(dro_song.file_version, 1)
        self.assertEqual(dro_song.opl_type, OPLType.OPL2)
        self.assertEqual(dro_song.ms_length, V1_MS_LENGTH)
        self.assertEqual(len(dro_song.data), 7)

    def test_read_dro1_with_a_one_byte_opl_type(self) -> None:
        dro_song = self._read(build_dro1(char_opl_type=True))
        self.assertEqual(dro_song.opl_type, OPLType.OPL2)
        self.assertEqual(len(dro_song.data), 7)

    def test_read_dro1_still_rejects_trailing_bytes(self) -> None:
        with self.assertRaises(DROFileException):
            self._read(build_dro1(trailing=b"\xde\xad"))


class TestDroFileIOv2(TestCase):
    def test_load_dro2(self) -> None:
        file_name = str(Path(__file__) / ".." / ".." / "lsl3_score_up_dro2.dro")
        dro2_io = DroFileIO()
        dro_song: DROSongV2 = cast(DROSongV2, dro2_io.read(file_name))
        self.assertEqual(dro_song.ms_length, 2683)
        self.assertEqual(dro_song.file_version, 2)
        self.assertEqual(dro_song.long_delay_code, 123)
        self.assertEqual(dro_song.short_delay_code, 122)
        self.assertEqual(dro_song.opl_type, OPLType.OPL2)
        self.assertEqual(dro_song.name, file_name)
        self.assertEqual(
            dro_song.detailed_register_descriptions, None
        )  # not populated yet
        self.assertEqual(
            dro_song.data.codemap[:10], (1, 4, 5, 8, 189, 32, 64, 96, 128, 224)
        )
        self.assertIsNotNone(dro_song.data_lock)

        dro_data = cast(DRODataV2, dro_song.data)
        self.assertEqual(dro_data.codemap, dro_song.data.codemap)
        self.assertEqual(
            dro_data.data[:10], array.array("B", (0, 32, 5, 49, 10, 2, 15, 2, 20, 98))
        )
        self.assertEqual(dro_data._long_delay_code, 123)
        self.assertEqual(dro_data._short_delay_code, 122)

    def test_write_dro2_round_trips(self) -> None:
        """Before the `opl_type.value` fix this raised
        "required argument is not an integer" - and, because `DroFileIO.write`
        opens the destination first, left a 12-byte stub behind."""
        original = FIXTURE_DRO2.read_bytes()
        dro_song = DroFileIO().read(str(FIXTURE_DRO2))

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "out.dro"
            dro_song.name = str(out)
            DroFileIO().write(dro_song)
            self.assertEqual(out.read_bytes(), original)


class TestDroFileIO(TestCase):
    def test_write_does_not_destroy_the_source_on_success(self) -> None:
        """Read, save over the top, read again."""
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "song.dro"
            path.write_bytes(FIXTURE_DRO2.read_bytes())

            dro_song = DroFileIO().read(str(path))
            DroFileIO().write(dro_song)
            reread = DroFileIO().read(str(path))

            self.assertEqual(len(reread.data), len(dro_song.data))
            self.assertEqual(reread.ms_length, dro_song.ms_length)
