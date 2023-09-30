import array
from pathlib import Path
from unittest import TestCase

from src.drotrimmer.vgm.vgm_data import VGMSong
from src.drotrimmer.vgm.vgm_io import VgmFileIO


class TestVgmIO(TestCase):
    def test_read_vgm(self) -> None:
        file_name = str(Path(__file__) / ".." / ".." / "lsl3_score_up.vgm")
        vgm_io = VgmFileIO()
        vgm_song: VGMSong = vgm_io.read_data(file_name)
        self.assertEqual(vgm_song.name, file_name)
        self.assertEqual(vgm_song.opl_type, 0)
        self.assertEqual(vgm_song.total_samples, 118320)
        self.assertEqual(
            vgm_song.data.data[:6],
            array.array("B", [0x5A, 0x01, 0x20, 0x5A, 0x20, 0x31]),
        )
        self.assertEqual(vgm_song.data._offsets[0], 0x00)
        self.assertEqual(vgm_song.data._offsets[1], 0x03)
        # TODO: check other attributes
