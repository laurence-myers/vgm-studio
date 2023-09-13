import array
from unittest import TestCase

from src.drotrimmer.dro_data import DROSongV2, DRO_FILE_V2, DRODataV2


def create_dro_data_v2():
    return DRODataV2(
        array.array("B"),
        (0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90),
        0xFE,
        0xFF,
    )


def create_dro_song_v2():
    data = create_dro_data_v2()
    return DROSongV2(
        DRO_FILE_V2,
        "test.dro",
        data,
        100,
        2,
        0xFE,
        0xFF,
    )


class TestDROSongV2(TestCase):
    def test_get_length_ms(self):
        dro_song = create_dro_song_v2()
        self.assertEqual(dro_song.get_length_ms(), 100)
        self.assertEqual(
            dro_song.get_length_ms(), dro_song.ms_length
        )  # I'm not sure why there's a method _and_ an attribute
