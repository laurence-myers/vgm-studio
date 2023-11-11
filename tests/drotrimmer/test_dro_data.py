import array
from unittest import TestCase

from src.drotrimmer.dro_data import (
    DROSongV2,
    DRO_FILE_V2,
    DRODataV2,
    DetailedRegisterInfo,
    DeleteInstructionsCommand,
    DROInstruction,
    DROInstructionType,
    OPLType,
)
from src.drotrimmer.dro_undo import UndoController

SONG_LENGTH = (0xB1 + 0xC100) * 2


def shallow_copy_dro_data_v2(dro_data: DRODataV2, new_values: list[int]) -> DRODataV2:
    return DRODataV2(
        array.array("B", new_values),
        dro_data.codemap,
        dro_data._short_delay_code,
        dro_data._long_delay_code,
    )


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
        OPLType.OPL3,
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


class TestDRODataV2(TestCase):
    def _compare_dro_data_v2(
        self,
        first: DRODataV2,
        second: DRODataV2,
        expected_second_data: list[int] | None = None,
    ) -> None:
        self.assertEqual(first.codemap, second.codemap)
        self.assertEqual(first._short_delay_code, second._short_delay_code)
        self.assertEqual(first._long_delay_code, second._long_delay_code)
        self.assertEqual(
            second.data,
            array.array("B", expected_second_data)
            if expected_second_data is not None
            else first.data,
        )

    def test_del(self) -> None:
        dro_data = create_dro_data_v2()
        test_slice = slice(0, 3)
        self.assertEqual(len(dro_data), 14)
        self.assertEqual(dro_data.raw_len(), 28)
        self._compare_dro_data_v2(
            dro_data[test_slice],
            shallow_copy_dro_data_v2(dro_data, [0x00, 0x01, 0x02, 0x03, 0x04, 0x05]),
        )

        # Delete one instruction
        del dro_data[0]
        self.assertEqual(len(dro_data), 13)
        self.assertEqual(dro_data.raw_len(), 26)
        self._compare_dro_data_v2(
            dro_data[test_slice],
            shallow_copy_dro_data_v2(dro_data, [0x02, 0x03, 0x04, 0x05, 0x06, 0x07]),
        )

        # Delete multiple instructions using a slice
        del dro_data[1:2]
        self.assertEqual(len(dro_data), 11)
        self.assertEqual(dro_data.raw_len(), 22)
        self._compare_dro_data_v2(
            dro_data[test_slice],
            shallow_copy_dro_data_v2(dro_data, [0x02, 0x03, 0x08, 0x09, 0xFE, 0xB0]),
        )

    def test_get_item(self) -> None:
        dro_data = create_dro_data_v2()
        self.assertEqual(
            dro_data[0],
            DROInstruction(DROInstructionType.REGISTER, dro_data.codemap[0], 0x01, 0),
        )
        self.assertEqual(
            dro_data[1],
            DROInstruction(DROInstructionType.REGISTER, dro_data.codemap[2], 0x03, 0),
        )
        self.assertEqual(
            dro_data[2],
            DROInstruction(DROInstructionType.REGISTER, dro_data.codemap[4], 0x05, 0),
        )
        self.assertEqual(
            dro_data[5],
            DROInstruction(
                DROInstructionType.DELAY_MS, dro_data._short_delay_code, 0xB0 + 1, None
            ),
        )
        self._compare_dro_data_v2(
            dro_data[:2],
            shallow_copy_dro_data_v2(dro_data, [0x00, 0x01, 0x02, 0x03]),
        )

    def test_interpret_data(self) -> None:
        dro_data = create_dro_data_v2()

        self.assertEqual(
            dro_data._interpret_data(0),
            DROInstruction(DROInstructionType.REGISTER, dro_data.codemap[0], 0x01, 0),
        )
        self.assertEqual(
            dro_data._interpret_data(2),
            DROInstruction(DROInstructionType.REGISTER, dro_data.codemap[2], 0x03, 0),
        )
        self.assertEqual(
            dro_data._interpret_data(4),
            DROInstruction(DROInstructionType.REGISTER, dro_data.codemap[4], 0x05, 0),
        )
        self.assertEqual(
            dro_data._interpret_data(10),
            DROInstruction(
                DROInstructionType.DELAY_MS, dro_data._short_delay_code, 0xB0 + 1, None
            ),
        )

    def test_is_long_delay(self) -> None:
        dro_data = create_dro_data_v2()
        self.assertTrue(dro_data.is_long_delay(0xFF))
        self.assertFalse(dro_data.is_long_delay(0xFE))

    def test_is_short_delay(self) -> None:
        dro_data = create_dro_data_v2()
        self.assertTrue(dro_data.is_short_delay(0xFE))
        self.assertFalse(dro_data.is_short_delay(0xFF))

    def test_iter(self) -> None:
        dro_data = create_dro_data_v2()
        instructions = []
        for instr in dro_data:
            instructions.append(instr)
        self.assertEqual(len(instructions), 14)
        self.assertEqual(
            instructions[0],
            DROInstruction(DROInstructionType.REGISTER, dro_data.codemap[0], 0x01, 0),
        )

    def test_iter_indexes(self) -> None:
        dro_data = create_dro_data_v2()
        iterator = dro_data._iter_indexes()
        self.assertIsInstance(iterator, range)
        indexes = list(iterator)
        self.assertEqual(indexes[:3], [0, 1, 2])
        self.assertEqual(len(indexes), 14)

    def test_len(self) -> None:
        dro_data = create_dro_data_v2()
        self.assertEqual(len(dro_data), 14)

    def test_shallow_copy(self) -> None:
        dro_data = create_dro_data_v2()
        dro_data_copy = dro_data.shallow_copy()
        self._compare_dro_data_v2(dro_data, dro_data_copy, [])
        dro_data_copy = dro_data.shallow_copy(array.array("B", [1, 2, 3]))
        self._compare_dro_data_v2(dro_data, dro_data_copy, [1, 2, 3])

    def test_translate_index(self) -> None:
        dro_data = create_dro_data_v2()
        self.assertEqual(dro_data._translate_index(0), 0)
        self.assertEqual(dro_data._translate_index(1), 2)
        self.assertEqual(dro_data._translate_index(5), 10)


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
                "Song: test.dro\n"
                + "Format: DRO v2\n"
                + "OPL Type: OPLType.OPL3\n"
                + f"Length (ms): {SONG_LENGTH}"
            ),
        )

    def test_str(self) -> None:
        dro_song = create_dro_song_v2()
        self.assertEqual(
            str(dro_song),
            f"DROSong[name = 'test.dro', ver = '2', opl_type = 'OPLType.OPL3', ms_length = '{SONG_LENGTH}']",
        )
