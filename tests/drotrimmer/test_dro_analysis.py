import array
from unittest import TestCase

from src.drotrimmer.dro_analysis import (
    DROFirstDelayAnalyzer,
    DRORegisterUsageAnalyzer,
    DROSimpleNoteAnalyser,
    DROTotalDelayCalculator,
)
from src.drotrimmer.dro_data import (
    DRO_FILE_V1,
    DRO_FILE_V2,
    DRODataV1,
    DRODataV2,
    DROSongV1,
    DROSongV2,
    OPLType,
)

# A "key on" write: bit 0x20 sets the key, 0x1C is the octave (4), 0x03 the
#  top two bits of the pitch.
KEY_ON = 0x31
KEY_OFF = 0x11
PITCH_LOW = 0x98


def dro_song_v1(data: bytes) -> DROSongV1:
    return DROSongV1(
        DRO_FILE_V1,
        "test.dro",
        DRODataV1(array.array("B", data)),
        0,
        OPLType.OPL3,
    )


def dro_song_v2(data: bytes, codemap: tuple[int, ...]) -> DROSongV2:
    return DROSongV2(
        DRO_FILE_V2,
        "test.dro",
        DRODataV2(array.array("B", data), codemap, 0xFE, 0xFF),
        0,
        OPLType.OPL3,
        0xFE,
        0xFF,
    )


class TestDROSimpleNoteAnalyser(TestCase):
    def test_analyze_dro_v1_tracks_bank_switch_instructions(self) -> None:
        """DRO v1 register writes carry no bank of their own; the bank comes from
        separate bank switch instructions. Reading it off the instruction raised
        `TypeError: unsupported operand type(s) for *: 'NoneType' and 'int'`."""
        dro_song = dro_song_v1(
            bytes(
                (
                    0xA0,
                    PITCH_LOW,
                    0xB0,
                    KEY_ON,  # low bank, channel 1
                    0x03,  # switch to the high bank
                    0xA0,
                    PITCH_LOW,
                    0xB0,
                    KEY_ON,  # high bank, channel 10
                    0x02,  # back to the low bank
                    0xA1,
                    PITCH_LOW,
                    0xB1,
                    KEY_ON,  # low bank, channel 2
                )
            )
        )

        output = DROSimpleNoteAnalyser().analyze_dro(dro_song)

        self.assertEqual(len(output), 18)
        note_counts = [len(notes) for notes in output]
        expected = [0] * 18
        expected[0] = 1  # low bank, channel 1
        expected[1] = 1  # low bank, channel 2
        expected[9] = 1  # high bank, channel 10
        self.assertEqual(note_counts, expected)

        self.assertEqual(output[0][0].channel, 1)
        self.assertEqual(output[9][0].channel, 10)
        self.assertEqual(output[0][0].octave, 4)
        self.assertEqual(output[0][0].pitch, ((KEY_ON & 0x03) << 8) | PITCH_LOW)

    def test_analyze_dro_v1_without_any_bank_switch_stays_on_the_low_bank(self) -> None:
        dro_song = dro_song_v1(bytes((0xA8, PITCH_LOW, 0xB8, KEY_ON)))
        output = DROSimpleNoteAnalyser().analyze_dro(dro_song)
        self.assertEqual([len(notes) for notes in output], [0] * 8 + [1] + [0] * 9)
        self.assertEqual(output[8][0].channel, 9)

    def test_analyze_dro_v2_uses_the_bank_on_each_register_write(self) -> None:
        # codemap[0] = 0xA0, codemap[1] = 0xB0. The high bit of a code selects
        #  the high bank.
        dro_song = dro_song_v2(
            bytes(
                (
                    0x00,
                    PITCH_LOW,  # low bank, register 0xA0
                    0x01,
                    KEY_ON,  # low bank, register 0xB0
                    0x80,
                    PITCH_LOW,  # high bank, register 0xA0
                    0x81,
                    KEY_ON,  # high bank, register 0xB0
                )
            ),
            (0xA0, 0xB0),
        )

        output = DROSimpleNoteAnalyser().analyze_dro(dro_song)

        expected = [0] * 18
        expected[0] = 1
        expected[9] = 1
        self.assertEqual([len(notes) for notes in output], expected)

    def test_a_note_is_only_recorded_when_the_key_goes_down(self) -> None:
        dro_song = dro_song_v1(
            bytes(
                (
                    0xA0,
                    PITCH_LOW,
                    0xB0,
                    KEY_ON,  # note on
                    0xB0,
                    KEY_ON,  # still on, no new note
                    0xB0,
                    KEY_OFF,  # note off
                    0xB0,
                    KEY_ON,  # on again, a second note
                )
            )
        )
        output = DROSimpleNoteAnalyser().analyze_dro(dro_song)
        self.assertEqual(len(output[0]), 2)


class TestDRORegisterUsageAnalyzer(TestCase):
    def test_bank_switches_steer_the_usage_keys(self) -> None:
        """This analyzer already tracked the bank correctly; lock that in."""
        dro_song = dro_song_v1(bytes((0x20, 0x01, 0x03, 0x20, 0x02, 0x02, 0x20, 0x03)))
        usage, perc_usage = DRORegisterUsageAnalyzer().analyze_dro(dro_song)
        self.assertEqual(usage[0x020], 2)  # low bank, twice
        self.assertEqual(usage[0x120], 1)  # high bank, once
        self.assertEqual(perc_usage, {})

    def test_detailed_percussion_analysis(self) -> None:
        # Register 0xBD, with percussion mode (0x20), BD (0x10) and HH (0x01).
        dro_song = dro_song_v1(bytes((0xBD, 0x31)))
        _usage, perc_usage = DRORegisterUsageAnalyzer(
            detailed_percussion_analysis=True
        ).analyze_dro(dro_song)
        self.assertEqual(
            sorted(perc_usage.keys()), sorted((0x20, 0x10, 0x01))  # perc mode, BD, HH
        )
        self.assertTrue(all(perc_usage.values()))


class TestDelayAnalyzers(TestCase):
    def test_total_delay_calculator(self) -> None:
        # A short delay of 0xB0 + 1 ms, and a long delay of 0x1234 + 1 ms.
        dro_song = dro_song_v1(bytes((0x00, 0xB0, 0x01, 0x34, 0x12)))
        self.assertEqual(DROTotalDelayCalculator().sum_delay(dro_song), 177 + 4661)

    def test_first_delay_analyzer(self) -> None:
        analyzer = DROFirstDelayAnalyzer()
        analyzer.analyze_dro(dro_song_v1(bytes((0x00, 0xB0, 0x20, 0x01))))
        self.assertTrue(analyzer.result)

        analyzer = DROFirstDelayAnalyzer()
        analyzer.analyze_dro(dro_song_v1(bytes((0x20, 0x01, 0x00, 0xB0))))
        self.assertFalse(analyzer.result)
