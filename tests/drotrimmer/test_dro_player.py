import array
from unittest import TestCase

from src.drotrimmer import dro_util
from src.drotrimmer.dro_data import (
    DRODataV2,
    DROSongV2,
    DRO_FILE_V2,
    OPLType,
)
from src.drotrimmer.dro_player import DROPlayer

SHORT_DELAY_CODE = 0xFE
LONG_DELAY_CODE = 0xFF
CODEMAP = (0x20, 0x40, 0x60, 0x80)


def create_dro_song_v2(pairs: list[tuple[int, int]]) -> DROSongV2:
    data = DRODataV2(
        array.array("B", [b for pair in pairs for b in pair]),
        CODEMAP,
        SHORT_DELAY_CODE,
        LONG_DELAY_CODE,
    )
    ms_length = 0
    for code, value in pairs:
        if code == SHORT_DELAY_CODE:
            ms_length += value + 1
        elif code == LONG_DELAY_CODE:
            ms_length += (value + 1) << 8
    return DROSongV2(
        DRO_FILE_V2,
        "test.dro",
        data,
        ms_length,
        OPLType.OPL3,
        SHORT_DELAY_CODE,
        LONG_DELAY_CODE,
    )


def create_dro_player() -> DROPlayer:
    player = DROPlayer(sound_on=False)
    # Use a frequency where 1 ms is a fractional number of samples (44.1),
    #  so that rounding behaviour is exercised.
    player.frequency = 44100
    return player


class TestDROPlayer(TestCase):
    def test_full_playback_samples_match_calculated_total(self) -> None:
        # 7 ms + 10 ms = 17 ms -> 749.7 samples at 44100 Hz. The rendered
        #  samples count must match the total calculated from the ms length.
        song = create_dro_song_v2(
            [
                (0x00, 0xAA),
                (SHORT_DELAY_CODE, 6),
                (0x01, 0xBB),
                (SHORT_DELAY_CODE, 9),
            ]
        )
        player = create_dro_player()
        player.load_song(song)
        player.play()
        update_thread = player.update_thread
        assert update_thread is not None
        update_thread.join(timeout=10)

        self.assertFalse(update_thread.is_alive())
        self.assertFalse(player.is_playing)
        self.assertEqual(song.ms_length, player.time_elapsed)
        self.assertEqual(
            dro_util.calculate_playback_samples(
                song.ms_length, player.frequency, player.channels, player.bit_depth
            ),
            player.position_samples,
        )

    def test_stop_waits_for_update_thread(self) -> None:
        # ~61 seconds worth of delays, so playback is still in progress when
        #  stop is called.
        song = create_dro_song_v2([(0x00, 0xAA)] + [(LONG_DELAY_CODE, 0x0F)] * 15)
        player = create_dro_player()
        player.load_song(song)
        player.play()
        update_thread = player.update_thread
        assert update_thread is not None

        player.stop()

        # The update thread must be finished once stop returns, so it can't
        #  interfere with a subsequent seek or playback.
        self.assertFalse(update_thread.is_alive())
        self.assertFalse(player.is_playing)

    def test_seek_while_playing_restarts_and_completes(self) -> None:
        # Mimics clicking the waveform during playback:
        #  stop -> reset -> seek -> play.
        pairs: list[tuple[int, int]] = []
        for i in range(50):
            pairs.append((0x00, i))
            pairs.append((SHORT_DELAY_CODE, 9))  # 10 ms each
        song = create_dro_song_v2(pairs)
        player = create_dro_player()
        player.load_song(song)
        player.play()

        player.stop()
        player.reset()
        player.seek_to_pos(10)
        player.play()
        update_thread = player.update_thread
        assert update_thread is not None
        update_thread.join(timeout=10)

        # The new playback must run to completion, with the same final counters
        #  as an uninterrupted playback.
        self.assertFalse(update_thread.is_alive())
        self.assertFalse(player.is_playing)
        self.assertEqual(song.ms_length, player.time_elapsed)
        self.assertEqual(
            dro_util.calculate_playback_samples(
                song.ms_length, player.frequency, player.channels, player.bit_depth
            ),
            player.position_samples,
        )
