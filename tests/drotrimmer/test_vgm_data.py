import array
import struct
from pathlib import Path
from unittest import TestCase

from src.drotrimmer.dro_data import (
    DRO_FILE_V1,
    DRODataV1,
    DROInstruction,
    DROInstructionType,
    DROSongV1,
    OPLType,
)
from src.drotrimmer.dro_io import DroFileIO
from src.drotrimmer.vgm.vgm_data import VGMData, VGMSong

FIXTURE_DIR = Path(__file__).parent.parent
FIXTURE_DRO2 = FIXTURE_DIR / "lsl3_score_up_dro2.dro"
FIXTURE_VGM = FIXTURE_DIR / "lsl3_score_up.vgm"


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


class TestVgmSongFromSong(TestCase):
    """lsl3_score_up.vgm was produced from lsl3_score_up_dro2.dro by dro2vgm, an
    independent tool, so a correct conversion has to reproduce its command stream.

    Before the fix, from_song emitted delays of 15 ms or less as "0x70 | ms" - a
    wait of ms + 1 *samples* rather than ms milliseconds - and counted those
    milliseconds as samples, reporting 118125 samples instead of 118320."""

    @staticmethod
    def _vgm_fixture_data() -> tuple[bytes, int]:
        """The fixture's command stream (minus the 0x66 end marker), and the
        total sample count its header records."""
        raw = FIXTURE_VGM.read_bytes()
        data_offset = struct.unpack_from("<L", raw, 0x34)[0] + 0x34
        total_samples = struct.unpack_from("<L", raw, 0x18)[0]
        assert raw[-1] == 0x66, "fixture should end with the end-of-data marker"
        return raw[data_offset:-1], total_samples

    def test_from_song_reproduces_dro2vgm(self) -> None:
        expected_data, expected_samples = self._vgm_fixture_data()
        dro_song = DroFileIO().read(str(FIXTURE_DRO2))
        vgm_song = VGMSong.from_song(dro_song)

        self.assertEqual(vgm_song.total_samples, expected_samples)
        self.assertEqual(vgm_song.total_samples, 118320)
        self.assertEqual(bytes(vgm_song.data.data), expected_data)
        self.assertEqual(len(vgm_song.data), len(dro_song.data))

    def test_from_song_rounds_the_running_total_not_each_delay(self) -> None:
        """Two identical 16 ms delays become 706 and 705 samples. No per-delay
        rounding can produce that."""
        dro_song = DroFileIO().read(str(FIXTURE_DRO2))
        vgm_song = VGMSong.from_song(dro_song)

        waits = [
            inst.value
            for inst in vgm_song.data
            if inst.inst_type == DROInstructionType.DELAY_SMP
        ]
        self.assertEqual(waits[:9], [4410, 706, 8820, 749, 750, 706, 4410, 44, 705])
        self.assertEqual(sum(waits), 118320)

    def test_from_song_repeats_the_wait_opcode_for_long_delays(self) -> None:
        """A wait of more than 65535 samples needs a second 0x61 command. The old
        code appended a bare 0xFF 0xFF pair, which decoded as garbage."""
        # A single long delay of 2000 ms = 88200 samples.
        data = DRODataV1(array.array("B", [0x01, 0xCF, 0x07]))
        dro_song = DROSongV1(DRO_FILE_V1, "t.dro", data, 2000, OPLType.OPL2)
        vgm_song = VGMSong.from_song(dro_song)

        self.assertEqual(vgm_song.total_samples, 88200)
        self.assertEqual(len(vgm_song.data), 2, "one wait cannot express 88200 samples")
        self.assertEqual(vgm_song.data.data[0], 0x61)
        self.assertEqual(vgm_song.data.data[3], 0x61, "the opcode repeats")
        self.assertEqual(
            [inst.value for inst in vgm_song.data], [0xFFFF, 88200 - 0xFFFF]
        )
