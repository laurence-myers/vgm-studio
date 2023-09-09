from pathlib import Path
from unittest import TestCase

from src.drotrimmer.vgm.vgm_io import VgmFileIO


class TestVgmIO(TestCase):
    def test_read_vgm(self) -> None:
        file_name = str(Path(__file__) / ".." / ".." / "lsl3_score_up.vgm")
        vgm_io = VgmFileIO()
        vgm_song = vgm_io.read_data(file_name)
        self.assertEqual(vgm_song.name, file_name)
        self.assertEqual(vgm_song.opl_type, 0)
        self.assertEqual(vgm_song.total_samples, 118320)
        # TODO: check other attributes, including data
