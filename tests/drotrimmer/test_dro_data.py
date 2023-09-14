import array
from unittest import TestCase

from src.drotrimmer.dro_data import DROSongV2, DRO_FILE_V2, DRODataV2


def create_dro_data_v2() -> DRODataV2:
    return DRODataV2(
        array.array("B", (list(range(10)) + [0xFE, 0xB0, 0xFF, 0xC0]) * 2),
        (0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0),
        0xFE,
        0xFF,
    )


def create_dro_song_v2() -> DROSongV2:
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
    def test_delete_instructions(self) -> None:
        pass

    def test_find_next_instruction(self) -> None:
        dro_song = create_dro_song_v2()
        index = dro_song.find_next_instruction(0, "0x50")
        self.assertEqual(index, 2)
        index = dro_song.find_next_instruction(0, "0x40")
        self.assertEqual(index, -1)
        index = dro_song.find_next_instruction(3, "0x50")
        self.assertEqual(index, 9)
        index = dro_song.find_next_instruction(3, "0x50", look_backwards=True)
        self.assertEqual(index, 2)
        index = dro_song.find_next_instruction(0, "0x50", look_backwards=True)
        self.assertEqual(index, -1)

        # Special values
        # "DLYS", "DLYL", "DALL", or "BANK"
        index = dro_song.find_next_instruction(0, "DLYS")
        self.assertEqual(index, 5)
        index = dro_song.find_next_instruction(0, "DLYL")
        self.assertEqual(index, 6)
        index = dro_song.find_next_instruction(0, "DALL")
        self.assertEqual(index, 5)
        index = dro_song.find_next_instruction(5, "DALL")
        self.assertEqual(index, 6)
        index = dro_song.find_next_instruction(0, "BANK")
        self.assertEqual(index, -1)  # Bank switches are not supported in DRO v2 files

    def test_get_bank_description(self) -> None:
        pass

    def test_get_detailed_register_description(self) -> None:
        pass

    def test_get_index_and_ms_offset_by_position_pct(self) -> None:
        pass

    def test_get_instruction_description(self) -> None:
        pass

    def test_get_length_data(self) -> None:
        pass

    def test_get_length_ms(self) -> None:
        dro_song = create_dro_song_v2()
        self.assertEqual(dro_song.get_length_ms(), 100)
        self.assertEqual(
            dro_song.get_length_ms(), dro_song.ms_length
        )  # I'm not sure why there's a method _and_ an attribute

    def test_get_register_display(self) -> None:
        pass

    def test_get_value_display(self) -> None:
        pass

    def test_insert_instructions(self) -> None:
        pass

    def test_pretty_string(self) -> None:
        pass

    def test_str(self) -> None:
        pass
