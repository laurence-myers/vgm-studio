import array
from unittest import TestCase

from src.drotrimmer.dro_data import (
    DROSongV2,
    DRO_FILE_V2,
    DRODataV2,
    DetailedRegisterInfo,
    DeleteInstructionsCommand,
)
from src.drotrimmer.dro_undo import UndoController

SONG_LENGTH = (0xB1 + 0xC100) * 2


def create_detailed_register_descriptions() -> DetailedRegisterInfo:
    return [
        (1, "Foo", 0),
        (0, "Bar", 0),
        (1, "Cmd3", 0),
        (0, "Cmd4", 0),
        (0, "Cmd5", 0),
        (0, "DelayS", 0xB1),
        (0, "DelayL", 0xB1 + 0xC100),
        (1, "Foo", 0xB1 + 0xC100),
        (0, "Bar", 0xB1 + 0xC100),
        (1, "Cmd3", 0xB1 + 0xC100),
        (0, "Cmd4", 0xB1 + 0xC100),
        (0, "Cmd5", 0xB1 + 0xC100),
        (0, "DelayS", 0xB1 + 0xC100 + 0xB1),
        (0, "DelayL", 0xB1 + 0xC100 + 0xB1 + 0xC100),
    ]


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
        SONG_LENGTH,
        2,
        0xFE,
        0xFF,
    )


class TestDeleteInstructionsCommand(TestCase):
    def test_apply_and_revert(self) -> None:
        undo_controller = UndoController()
        dro_song = create_dro_song_v2()
        index_list = [1, 6, 3, 4]
        command = DeleteInstructionsCommand(dro_song, index_list)
        data_slice = slice(0, 8)

        # Initial
        self.assertEqual(dro_song.get_length_data(), 14)
        expected_1 = array.array("B", [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07])
        self.assertEqual(dro_song.data.data[data_slice], expected_1)

        # First apply
        undo_controller.execute(command)
        self.assertEqual(dro_song.get_length_data(), 14 - len(index_list))
        expected_2 = array.array(
            "B",
            [
                0x00,
                0x01,
                # deleted 0x01, 1
                # deleted 0x01, 2
                0x04,
                0x05,
                # deleted 0x03, 1
                # deleted 0x03, 2
                # deleted 0x04, 1
                # deleted 0x04, 2
                0xFE,
                0xB0,
                # deleted 0x06, 1
                # deleted 0x06, 2
                0x00,
                0x01,
            ],
        )
        self.assertEqual(dro_song.data.data[data_slice], expected_2)

        # Delete some more
        undo_controller.execute(DeleteInstructionsCommand(dro_song, [1]))
        self.assertEqual(dro_song.get_length_data(), 14 - len(index_list) - 1)
        expected_3 = array.array(
            "B",
            [
                0x00,
                0x01,
                # deleted 0x01, 1
                # deleted 0x01, 2
                0xFE,
                0xB0,
                0x00,
                0x01,
                0x02,
                0x03,
            ],
        )
        self.assertEqual(dro_song.data.data[data_slice], expected_3)

        # Undo
        undo_controller.undo()
        self.assertEqual(dro_song.data.data[data_slice], expected_2)
        undo_controller.redo()
        self.assertEqual(dro_song.data.data[data_slice], expected_3)
        undo_controller.undo()
        self.assertEqual(dro_song.data.data[data_slice], expected_2)
        undo_controller.undo()
        self.assertEqual(dro_song.data.data[data_slice], expected_1)
        undo_controller.redo()
        self.assertEqual(dro_song.data.data[data_slice], expected_2)
        undo_controller.redo()
        self.assertEqual(dro_song.data.data[data_slice], expected_3)
        undo_controller.undo()
        self.assertEqual(dro_song.data.data[data_slice], expected_2)


class TestDROSongV2(TestCase):
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
        dro_song = create_dro_song_v2()
        # No description before detailed analysis has occurred.
        self.assertEqual(dro_song.get_bank_description(0), "?")
        self.assertEqual(dro_song.get_bank_description(1), "?")
        dro_song.detailed_register_descriptions = (
            create_detailed_register_descriptions()
        )
        self.assertEqual(dro_song.get_bank_description(0), "1")
        self.assertEqual(dro_song.get_bank_description(1), "0")
        self.assertEqual(dro_song.get_bank_description(99), "?")

    def test_get_detailed_register_description(self) -> None:
        dro_song = create_dro_song_v2()
        # No description before detailed analysis has occurred.
        self.assertEqual(dro_song.get_detailed_register_description(0), "(unknown)")
        self.assertEqual(
            dro_song.get_detailed_register_description(1),
            "Tremolo / Vibrato / Sustain / KSR / Frequency Multiplication Factor",
        )
        dro_song.detailed_register_descriptions = (
            create_detailed_register_descriptions()
        )
        self.assertEqual(dro_song.get_detailed_register_description(0), "Foo")
        self.assertEqual(dro_song.get_detailed_register_description(1), "Bar")

    def test_get_index_and_ms_offset_by_position_pct(self) -> None:
        dro_song = create_dro_song_v2()

        # No results if detailed register descriptions haven't been generated yet
        result = dro_song.get_index_and_ms_offset_by_position_pct(0.5)
        self.assertEqual(result, None)

        dro_song.detailed_register_descriptions = (
            create_detailed_register_descriptions()
        )
        result = dro_song.get_index_and_ms_offset_by_position_pct(0.5)
        self.assertEqual(result, ((dro_song.get_length_data()) // 2, SONG_LENGTH // 2))
        result = dro_song.get_index_and_ms_offset_by_position_pct(1)
        self.assertEqual(result, (dro_song.get_length_data() - 1, SONG_LENGTH))

    def test_get_instruction_description(self) -> None:
        dro_song = create_dro_song_v2()
        self.assertEqual(dro_song.get_instruction_description(0), "(unknown)")
        self.assertEqual(
            dro_song.get_instruction_description(1),
            "Tremolo / Vibrato / Sustain / KSR / Frequency Multiplication Factor",
        )
        self.assertEqual(
            dro_song.get_instruction_description(2),
            "Key Scale Level / Output Level",
        )

    def test_get_length_data(self) -> None:
        dro_song = create_dro_song_v2()
        self.assertEqual(dro_song.get_length_data(), 14)

    def test_get_length_ms(self) -> None:
        dro_song = create_dro_song_v2()
        self.assertEqual(dro_song.get_length_ms(), SONG_LENGTH)
        self.assertEqual(
            dro_song.get_length_ms(), dro_song.ms_length
        )  # I'm not sure why there's a method _and_ an attribute

    def test_get_register_display(self) -> None:
        dro_song = create_dro_song_v2()
        self.assertEqual(dro_song.get_register_display(0), "0x10")
        self.assertEqual(dro_song.get_register_display(1), "0x30")
        self.assertEqual(dro_song.get_register_display(2), "0x50")
        self.assertEqual(dro_song.get_register_display(5), "DLYS")
        self.assertEqual(dro_song.get_register_display(6), "DLYL")

    def test_get_value_display(self) -> None:
        dro_song = create_dro_song_v2()
        self.assertEqual(dro_song.get_value_display(0), "0x01 (1)")
        self.assertEqual(dro_song.get_value_display(1), "0x03 (3)")
        self.assertEqual(dro_song.get_value_display(2), "0x05 (5)")
        self.assertEqual(dro_song.get_value_display(5), "177 ms")
        self.assertEqual(dro_song.get_value_display(6), "49408 ms")

    def test_pretty_string(self) -> None:
        dro_song = create_dro_song_v2()
        self.assertEqual(
            dro_song.pretty_string(),
            (
                "DRO Song: test.dro\n"
                + "Format: v2\n"
                + "OPL Type: OPL-3\n"
                + f"Length (ms): {SONG_LENGTH}"
            ),
        )

    def test_str(self) -> None:
        dro_song = create_dro_song_v2()
        self.assertEqual(
            str(dro_song),
            f"DRO[name = 'test.dro', ver = '2', opl_type = '2' (OPL-3), ms_length = '{SONG_LENGTH}']",
        )
