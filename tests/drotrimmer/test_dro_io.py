import array
from pathlib import Path
from typing import cast
from unittest import TestCase

from src.drotrimmer.dro_data import DRODataV2
from src.drotrimmer.dro_io import DROSongV2, DroFileIO


class TestDroUndo(TestCase):
    def test_load_dro2(self) -> None:
        file_name = str(Path(__file__) / ".." / ".." / "lsl3_score_up_dro2.dro")
        dro2_io = DroFileIO()
        dro_song: DROSongV2 = cast(DROSongV2, dro2_io.read(file_name))
        self.assertEqual(dro_song.ms_length, 2683)
        self.assertEqual(dro_song.file_version, 2)
        self.assertEqual(dro_song.long_delay_code, 123)
        self.assertEqual(dro_song.short_delay_code, 122)
        self.assertEqual(dro_song.opl_type, 0)
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
        self.assertEqual(dro_data.delay_codes, (122, 123))
        self.assertEqual(dro_data.long_delay_code, 123)
        self.assertEqual(dro_data.short_delay_code, 122)
