import array
from unittest import TestCase

from src.drotrimmer.dro_data import DROInstruction, DROInstructionType
from src.drotrimmer.vgm.vgm_data import VGMData


def create_all_vgm_data() -> VGMData:
    init_data = []

    command = 0x5A
    for i in range(5):
        init_data.append(command)
        init_data.append(i * 2)
        init_data.append((i * 2) + 1)

    init_data.extend([0x61, 0xB0, 0x00])  # long wait
    init_data.extend([0x62, 0x63])  # 1/60 and 1/50 waits
    init_data.extend(range(0x70, 0x7F))  # short waits

    for command in (0x5E, 0x5F, 0xAA):
        for i in range(5):
            init_data.append(command)
            init_data.append(i * 2)
            init_data.append((i * 2) + 1)

    return VGMData(
        array.array("B", init_data),
    )


def create_vgm_data() -> VGMData:
    init_data = []

    command = 0x5A
    for i in range(5):
        init_data.append(command)
        init_data.append(i * 2)
        init_data.append((i * 2) + 1)

    init_data.extend([0x61, 0xB0, 0x00])  # long wait
    init_data.append(0x70)

    init_data *= 2

    return VGMData(
        array.array("B", init_data),
    )


def shallow_copy_vgm_data(new_values: list[int]) -> VGMData:
    return VGMData(array.array("B", new_values))


class TestVgmData(TestCase):
    def _compare_vgm_data(
        self,
        first: VGMData,
        second: VGMData,
        expected_second_data: list[int] | None = None,
    ) -> None:
        self.assertEqual(
            second.data,
            (
                array.array("B", expected_second_data)
                if expected_second_data is not None
                else first.data
            ),
        )

    def test_del(self) -> None:
        dro_data = create_vgm_data()
        test_slice = slice(0, 3)
        self.assertEqual(len(dro_data), 14)
        self.assertEqual(dro_data.raw_len(), 10 * 3 + 4 * 2)
        self._compare_vgm_data(
            dro_data[test_slice],
            shallow_copy_vgm_data(
                [0x5A, 0x00, 0x01, 0x5A, 0x02, 0x03, 0x5A, 0x04, 0x05]
            ),
        )

        # Delete one instruction
        del dro_data[0]
        dro_data._generate_offsets()  # Not great
        self.assertEqual(len(dro_data), 13)
        self.assertEqual(dro_data.raw_len(), (5 * 3 + (3 + 1)) * 2 - (1 * 3))
        self._compare_vgm_data(
            dro_data[test_slice],
            shallow_copy_vgm_data(
                [0x5A, 0x02, 0x03, 0x5A, 0x04, 0x05, 0x5A, 0x06, 0x07]
            ),
        )

        # Delete multiple instructions using a slice
        del dro_data[1:2]
        dro_data._generate_offsets()  # Not great
        self.assertEqual(len(dro_data), 11)
        self.assertEqual(dro_data.raw_len(), (5 * 3 + (3 + 1)) * 2 - (3 * 3))
        self._compare_vgm_data(
            dro_data[test_slice],
            shallow_copy_vgm_data(
                [0x5A, 0x02, 0x03, 0x5A, 0x08, 0x09, 0x61, 0xB0, 0x00]
            ),
        )

    def test_get_item(self) -> None:
        dro_data = create_vgm_data()
        self.assertEqual(
            dro_data[0],
            DROInstruction(DROInstructionType.REGISTER, 0x00, 0x01, 0),
        )
        self.assertEqual(
            dro_data[1],
            DROInstruction(DROInstructionType.REGISTER, 0x02, 0x03, 0),
        )
        self.assertEqual(
            dro_data[2],
            DROInstruction(DROInstructionType.REGISTER, 0x04, 0x05, 0),
        )
        self.assertEqual(
            dro_data[5],
            DROInstruction(DROInstructionType.DELAY_SMP, 0x61, 0xB0, None),
        )
        self._compare_vgm_data(
            dro_data[:2],
            shallow_copy_vgm_data([0x5A, 0x00, 0x01, 0x5A, 0x02, 0x03]),
        )

    def test_interpret_data(self) -> None:
        dro_data = create_vgm_data()

        self.assertEqual(
            dro_data._interpret_data(0),
            DROInstruction(DROInstructionType.REGISTER, 0x00, 0x01, 0),
        )
        self.assertEqual(
            dro_data._interpret_data(1 * 3),
            DROInstruction(DROInstructionType.REGISTER, 0x02, 0x03, 0),
        )
        self.assertEqual(
            dro_data._interpret_data(2 * 3),
            DROInstruction(DROInstructionType.REGISTER, 0x04, 0x05, 0),
        )
        self.assertEqual(
            dro_data._interpret_data(5 * 3),
            DROInstruction(DROInstructionType.DELAY_SMP, 0x61, 0xB0, None),
        )

    def test_is_long_delay(self) -> None:
        dro_data = create_vgm_data()
        self.assertTrue(dro_data.is_long_delay(0x61))
        self.assertFalse(dro_data.is_long_delay(0x70))

    def test_is_short_delay(self) -> None:
        dro_data = create_vgm_data()
        self.assertTrue(dro_data.is_short_delay(0x70))
        self.assertFalse(dro_data.is_short_delay(0x61))

    def test_iter(self) -> None:
        dro_data = create_vgm_data()
        instructions = []
        for instr in dro_data:
            instructions.append(instr)
        self.assertEqual(len(instructions), 14)
        self.assertEqual(
            instructions[0],
            DROInstruction(DROInstructionType.REGISTER, 0x00, 0x01, 0),
        )

    def test_iter_indexes(self) -> None:
        dro_data = create_vgm_data()
        iterator = dro_data._iter_indexes()
        self.assertIsInstance(iterator, range)
        indexes = list(iterator)
        self.assertEqual(indexes[:3], [0, 1, 2])
        self.assertEqual(len(indexes), 14)

    def test_len(self) -> None:
        dro_data = create_vgm_data()
        self.assertEqual(len(dro_data), 14)

    def test_shallow_copy(self) -> None:
        dro_data = create_vgm_data()
        dro_data_copy = dro_data.shallow_copy()
        self._compare_vgm_data(dro_data, dro_data_copy, [])
        dro_data_copy = dro_data.shallow_copy(array.array("B", [0x5A, 2, 3]))
        self._compare_vgm_data(dro_data, dro_data_copy, [0x5A, 2, 3])

    def test_translate_index(self) -> None:
        dro_data = create_vgm_data()
        self.assertEqual(dro_data._translate_index(0), 0)
        self.assertEqual(dro_data._translate_index(1), 3)
        self.assertEqual(dro_data._translate_index(5), 5 * 3)
