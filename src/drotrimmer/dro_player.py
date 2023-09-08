#!/usr/bin/python
#
#    Use, distribution, and modification of the DRO Trimmer binaries, source code,
#    or documentation, is subject to the terms of the MIT license, as below.
#
#    Copyright (c) 2008 - 2023 Laurence Dougal Myers
#
#    Permission is hereby granted, free of charge, to any person obtaining a copy
#    of this software and associated documentation files (the "Software"), to deal
#    in the Software without restriction, including without limitation the rights
#    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
#    copies of the Software, and to permit persons to whom the Software is
#    furnished to do so, subject to the following conditions:
#
#    The above copyright notice and this permission notice shall be included in
#    all copies or substantial portions of the Software.
#
#    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
#    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
#    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
#    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
#    THE SOFTWARE.

"""
DROPlayer initialises:

- output streams: things that take PCM data and write it somewhere else (audio, wav, waveform)
- processing streams: things that take DRO instructions, `write()` instructions, or `render()` something.
  - OPLStream: converts DRO instructions to PCM data, using PyOPL. Accepts a list of output streams.
  - DroCapture: converts DRO -> DRO, used for splitting into separate channels.
- DROPlayerUpdateThread: created on `play()`. Runs in a thread. Directly manipulates the dro_player.

This means that, starting playback is handled in the main thread, but actual rendering occurs in a separate thread.
"""

import math
import optparse
import os
import queue
import struct
import sys
import threading
import time
from typing import Literal
import wave

import pyaudio
import pyopl

from . import (
    dro_analysis,
    dro_capture,
    dro_config,
    dro_data,
    dro_globals,
    dro_logging,
    dro_util,
    dro_io,
)


_log = dro_logging.get_logger("DRO Player")
"""A generic logger for use by the module"""


def stop_player_on_exception(func):
    def inner_func(self, *args, **kwds):
        try:
            func(self, *args, **kwds)
        except:
            self.dro_player.is_playing = False
            raise

    return inner_func


class WavRenderer(object):
    def __init__(self, frequency: int, bit_depth: int, channels: int) -> None:
        self.frequency = frequency
        self.bit_depth = bit_depth
        self.channels = channels
        self.wav: wave.Wave_write | None = None
        self.wav_fname: str | None = None
        self.wav_lock = threading.RLock()

    def open(self, dro_song: dro_data.DROSong) -> None:
        if self.wav_fname is None:
            self.wav_fname = "{}.wav".format(dro_song.name)
        self.wav = wave.open(self.wav_fname, "wb")
        self.wav.setnchannels(self.channels)
        self.wav.setsampwidth(self.bit_depth // 8)
        self.wav.setframerate(self.frequency)

    def close(self) -> None:
        with self.wav_lock:
            if self.wav is not None:
                self.wav.close()
                self.wav = None  # Hm, maybe should leave it hanging around?
                self.wav_fname = None

    def write(self, data: bytes):
        with self.wav_lock:
            if self.wav is not None:
                self.wav.writeframes(data)

    def set_output_fname(self, output_fname: str) -> None:
        self.wav_fname = "{}.wav".format(output_fname)

    def is_active(self):
        return (
            self.wav and self.wav._file
        )  # a bid dodgy, accessing a "private" property.


class WaveformRenderer(object):
    def __init__(
        self,
        frequency: int,
        points_queue: queue.SimpleQueue,
        total_length_ms: int,
        num_buckets: int,
    ):
        self.frequency: int = frequency
        self.bit_depth: int = 16
        self.channels: int = 1
        self.points: list[tuple[int, int]] = []
        self.samples_written: int = 0
        self.quantized_samples_written: int = 0
        self.samples_per_bucket: float = 0
        self.queue: queue.SimpleQueue[list[tuple[int, int]]] = points_queue
        self.curr_max_sample: int = 0
        self.expected_total_samples: int = 0
        self.set_quantization(total_length_ms, num_buckets)
        self._last_wait: float = 0.0
        self._wait_period: float = 0.1

    def write(self, data: bytes):
        for (sample,) in struct.iter_unpack("h", data):
            bucket = math.floor(self.samples_written / self.samples_per_bucket)
            self.curr_max_sample = max(sample, abs(self.curr_max_sample))
            next_bucket = math.floor(
                (self.samples_written + 1) / self.samples_per_bucket
            )
            if bucket != next_bucket:
                self.points.append((bucket, self.curr_max_sample))
                self.curr_max_sample = 0
            self.samples_written += 1

        self.queue.put(self.points)
        if time.time() - self._last_wait > self._wait_period:
            time.sleep(0.01)  # Avoid smashing the CPU, so the UI is more responsive
            self._last_wait = time.time()

    def is_active(self):
        return True

    def set_quantization(self, total_length_ms: int, num_buckets: int):
        self.expected_total_samples = math.floor(
            total_length_ms * (self.frequency / 1000.0)
        )
        self.samples_per_bucket = self.expected_total_samples / num_buckets


class ProcessingStreamsList(list):
    def __init__(self):
        super(ProcessingStreamsList, self).__init__()
        self._bank = 0

    @property
    def bank(self):
        return self._bank

    @bank.setter
    def bank(self, value: int):
        self._bank = value
        for stream in self:
            stream.bank = value

    def open(self, dro_song: dro_data.DROSong):
        for stream in self:
            stream.open(dro_song)

    def set_output_fname(self, output_fname: str):
        for stream in self:
            stream.set_output_fname(output_fname)

    def write(self, register: int, value: int):
        for stream in self:
            stream.write(register, value)

    def render(self, ms_to_render: int):
        for stream in self:
            stream.render(ms_to_render)

    def render_chip_delay(self):
        for stream in self:
            stream.render_chip_delay()

    def clear_chip_delay_drift(self):
        for stream in self:
            stream.clear_chip_delay_drift()

    def stop(self):
        for stream in self:
            stream.stop()


class OPLStream(object):
    """Based on demo.py that comes with the PyOPL library.

    Also accounts for chip-write delays:
     "The AdLib manual gives the wait times in microseconds: three point three
     (3.3) microseconds for the address, and twenty-three (23) microseconds
     for the data."

    The OPL3 (YMF262) spec suggests that an address write and data write both need a wait of 32 master clock cycles.
    The master clock runs at 14.32 MHz. 64 cycles is 4.469273743016759776536312849162 microseconds... approximately ;)

    This page:
    http://repetae.net/computer/opledit/tech/opl3.txt
    or, if that's dead, from the archive:
    https://web.archive.org/web/20141013120106/http://www.ugcs.caltech.edu/~john/computer/opledit/tech/opl3.txt
    Says:
     "Unlike Adlib (OPL2), OPL3 doesn't need delay between register writes.
     With OPL2 you had to wait 3.3 [microseconds] after index register write and another
     23 [microseconds] after data register write. On the contrary OPL3 doesn't need
     (almost) any delay after index register write and only 0.28 [microseconds] after data
     register write. This means you can neglect the delays and slightly speed up
     your music driver. But using reasonable delays will certainly do no harm."

    A post on VOGONS mentions it could be 3.3us... sigh.

    Anyway, basically we need to make it configurable.

    """

    def __init__(
        self,
        frequency: int,
        buffer_size: int,
        bit_depth: int,
        channels: int,
        chip_write_delay: float,
        output_streams: list[pyaudio.Stream | WavRenderer | WaveformRenderer],
    ):
        self.frequency = frequency  # Changing this to be different to the audio rate produces a tempo-shifting effect
        self.buffer_size = buffer_size
        self.bit_depth = bit_depth
        self.channels = channels
        self.chip_write_delay = chip_write_delay
        self.output_streams = output_streams
        self.opl: pyopl.opl = pyopl.opl(
            frequency,
            sampleSize=(self.bit_depth // 8),
            channels=self.channels,
        )
        self.buffer: bytearray = self.__create_bytearray(buffer_size)
        self.stop_requested: bool = False  # required so we don't keep rendering obsolete data after stopping playback.
        self._bank: Literal[0, 1] = 0
        # OPL2/OPL3 need microsecond delays writing to registers, we need to account for it.
        self.chip_delay_drift: float = 0.0
        self.sample_overflow: float = (
            0  # fraction of samples that still need to be rendered.
        )
        self.samples_rendered: int = 0
        self.reset()

    @property
    def bank(self) -> Literal[0, 1]:
        return self._bank

    @bank.setter
    def bank(self, value: Literal[0, 1]):
        self._bank = value

    def reset(self):
        """
        The OPL emulator will retain it state, we need to make sure that we can clear its state
        (e.g. when creating a new OPL stream).
        """
        orig_bank = self.bank
        for bank in range(2):
            self.bank = bank
            for reg in range(0x100):
                self.write(reg, 0x00)
        self.bank = orig_bank
        self.chip_delay_drift = 0
        self.sample_overflow = 0
        self.samples_rendered = 0

    def open(self, dro_song: dro_data.DROSong):
        for ostream in self.output_streams:
            if isinstance(ostream, WavRenderer):  # blech
                ostream.open(dro_song)

    def set_output_fname(self, output_fname: str):
        for ostream in self.output_streams:
            if isinstance(ostream, WavRenderer):  # blech
                ostream.set_output_fname(output_fname)

    def stop(self):
        self.stop_requested = True
        for ostream in self.output_streams:
            if isinstance(ostream, WavRenderer):  # blech
                ostream.close()

    def __create_bytearray(self, size: int):
        return bytearray(size * (self.bit_depth // 8) * self.channels)

    def write(self, register: int, value: int):
        if self.bank:
            register |= 0x100
            # Could be re-written as "register |= self.bank << 2"
        self.opl.writeReg(register, value)
        self.chip_delay_drift += self.chip_write_delay

    def render(self, length_ms: int | float):
        # Taken from PyOPL 1.0 and 1.2. Accurate rendering, though a bit inefficient.
        samples_to_render = length_ms * self.frequency / 1000.0
        samples_to_render += self.sample_overflow
        self.sample_overflow = samples_to_render % 1
        if samples_to_render < 2:
            # Limitation of PyOPL: needs a minimum of two samples.
            return
        samples_to_render = int(samples_to_render // 1)
        while samples_to_render > 1 and not self.stop_requested:
            if samples_to_render < self.buffer_size:
                tmp_buffer = self.__create_bytearray(
                    (samples_to_render % self.buffer_size)
                )
                samples_to_render = 0
            else:
                tmp_buffer = self.buffer
                samples_to_render -= self.buffer_size
            self.opl.getSamples(tmp_buffer)
            for ostream in self.output_streams:
                try:
                    if hasattr(ostream, "is_active") and ostream.is_active():
                        ostream.write(bytes(tmp_buffer))
                except IOError:
                    return
            self.samples_rendered += len(tmp_buffer)

    def render_chip_delay(self):
        if self.chip_delay_drift > 0:
            self.render(self.chip_delay_drift / 1000.0)
            self.chip_delay_drift = 0

    def clear_chip_delay_drift(self):
        self.chip_delay_drift = 0


class DROPlayer(object):
    CHANNEL_REGISTERS = frozenset(
        list(range(0xB0, 0xB8 + 1)) + list(range(0x1B0, 0x1B8 + 1))
    )
    PERCUSSION_REGISTER = 0xBD
    # PERCUSSION_VALUES = frozenset(map(lambda i: 2 ** i, range(5)))

    def __init__(
        self,
        capture_dro=False,
        channels: int = 2,
        recording_on=False,
        sound_on=True,
    ):
        # TODO: separate frequency etc for opl rendering
        #  (similar to DOSBox's mixer vs opl settings)
        config = dro_config.get_config()
        self.frequency = config.audio.frequency
        self.buffer_size = config.audio.buffer_size
        self.bit_depth = config.audio.bit_depth
        self.chip_write_delay = config.audio.chip_write_delay

        self.capture_dro = capture_dro
        self.channels: int = channels  # crap
        self.recording_on = recording_on
        self.sound_on = sound_on

        if self.sound_on:
            self.audio: pyaudio.PyAudio | None = pyaudio.PyAudio()
        self.audio_stream: pyaudio.Stream | None = None
        # Set up the WAV Renderer
        if self.recording_on:
            self.wav_renderer: WavRenderer | None = WavRenderer(
                self.frequency,
                self.bit_depth,
                self.channels,
            )
        self.waveform_renderer: WaveformRenderer | None = None
        """Used in the UI. Produces a series of (x,y) points."""

        # Set up other stuff
        self.processing_streams = ProcessingStreamsList()
        """A list of processing streams, which accepts instructions and produce some output (PCM data, or DRO data)."""
        self.current_song: dro_data.DROSong | None = None
        self.is_playing: bool = False
        self.pos: int = 0
        """The current index in the DROSong data."""
        self.time_elapsed: int = 0
        """The time elapsed in ms. Note this based on delay instructions execute, not playback time."""
        self.update_thread = None
        self.active_channels = set(self.CHANNEL_REGISTERS)
        """Allows muting channels, useful for dro_split."""
        self.active_percussion = [0xFF, 0xFF]
        self.writes_elapsed = 0
        """Used for calculating chip write delay."""

    def __init_audio_output(self) -> pyaudio.Stream:
        if not self.audio:
            raise dro_util.DROTrimmerException(
                "Can't init audio stream, PyAudio was not initialised."
            )
        return self.audio.open(
            format=self.audio.get_format_from_width(self.bit_depth // 8),
            channels=self.channels,
            rate=self.frequency,
            output=True,
        )

    def close_audio_output(self):
        if self.audio_stream is not None:
            try:
                self.audio_stream.close()
            except Exception as e:
                _log.exception(e)

    def load_song(self, new_song: dro_data.DROSong):
        self.is_playing = False
        self.current_song = new_song
        self.reset()

    def reset(self) -> None:
        self.is_playing = False
        self.pos = 0
        self.time_elapsed = 0
        self.writes_elapsed = 0
        if self.update_thread is not None:
            self.update_thread.stop_request.set()
        self.update_thread = (
            None  # This thread gets created only when playing actually begins.
        )
        output_streams: list[pyaudio.Stream | WavRenderer | WaveformRenderer] = []
        if self.sound_on and self.audio:
            if not self.audio_stream:
                self.audio_stream = self.__init_audio_output()
            output_streams.append(self.audio_stream)
        if self.recording_on and self.wav_renderer:
            output_streams.append(self.wav_renderer)
        if self.waveform_renderer:
            output_streams.append(self.waveform_renderer)
        opl_stream = OPLStream(
            self.frequency,
            self.buffer_size,
            self.bit_depth,
            self.channels,
            self.chip_write_delay,
            output_streams,
        )
        if self.current_song is not None:
            if self.current_song.file_version == dro_data.DRO_FILE_V1:
                # Hack. DRO V1 files don't seem to set the "Waveform select" register
                # correctly, so OPL-2 songs sound very wrong. Doesn't affect V2 files.
                opl_stream.write(1, 32)
        self.processing_streams = ProcessingStreamsList()
        self.processing_streams.append(opl_stream)
        if self.capture_dro:
            dro_out_stream = dro_capture.DroCapture()
            self.processing_streams.append(dro_out_stream)
        self.active_channels = set(self.CHANNEL_REGISTERS)
        self.active_percussion = [0xFF, 0xFF]

    def set_output_fname(self, output_fname: str):
        self.processing_streams.set_output_fname(output_fname)

    def play(self):
        self.is_playing = True
        self.processing_streams.open(self.current_song)
        self.update_thread = DROPlayerUpdateThread(self, self.current_song)
        self.update_thread.start()

    @property
    def position_pct(self) -> float:
        # Prefer samples rendered, which is more accurate. If it's not set, check time elapsed.
        if not self.current_song:
            return 0
        samples_rendered = 0
        for ps in self.processing_streams:
            if isinstance(ps, OPLStream):
                samples_rendered = ps.samples_rendered
                break
        if not samples_rendered and self.time_elapsed:
            return self.time_elapsed / self.current_song.ms_length
        total_samples = dro_util.calculate_playback_samples(
            self.current_song.ms_length, self.frequency, self.channels, self.bit_depth
        )
        return samples_rendered / total_samples

    @property
    def position_samples(self) -> int:
        if not self.current_song:
            return 0
        samples_rendered = 0
        for ps in self.processing_streams:
            if isinstance(ps, OPLStream):
                samples_rendered = ps.samples_rendered
                break
        return samples_rendered

    def __set_samples_rendered_from_time_elapsed(self):
        samples_elapsed = dro_util.calculate_playback_samples(
            self.time_elapsed, self.frequency, self.channels, self.bit_depth
        )
        # Yucky, why do we reach into processing streams and OPLStream? Not very SOLID
        for ps in self.processing_streams:
            if isinstance(ps, OPLStream):
                ps.samples_rendered = samples_elapsed
                break

    def stop(self):
        self.is_playing = False
        if self.update_thread is not None:
            self.update_thread.stop_request.set()
        self.processing_streams.stop()

    def seek_to_time(self, seek_time):
        seeker = DROSeeker(self)
        seeker.seek_to_time(seek_time)
        self.__set_samples_rendered_from_time_elapsed()

    def seek_to_pos(self, seek_pos):
        seeker = DROSeeker(self)
        seeker.seek_to_pos(seek_pos)
        self.__set_samples_rendered_from_time_elapsed()

    @property
    def write_delay_elapsed(self):
        return self.writes_elapsed * self.chip_write_delay // 1000

    @property
    def time_with_write_delay_elapsed(self):
        return self.time_elapsed + self.write_delay_elapsed


class DROSeeker(object):
    """Helper class to seek in DRO songs.
    Externalised from the player so the player class remains DRO-version neutral."""

    def __init__(self, dro_player: DROPlayer):
        self.dro_player = dro_player  # circular reference, yuck

    # Could potentially merge with the updater thread, and have a flag to skip "rendering" of any sound.
    @stop_player_on_exception
    def seek_to_time(self, seek_time_ms: int) -> None:
        """Seeks to the specified time.
        Seek time is clamped between 0 and the song's recorded ms_length."""
        if not self.dro_player.current_song:
            return

        seek_time_ms = min(max(seek_time_ms, 0), self.dro_player.current_song.ms_length)

        self.dro_player.pos = 0
        self.dro_player.time_elapsed = 0
        self.dro_player.writes_elapsed = 0
        while self.dro_player.time_elapsed < seek_time_ms and self.dro_player.pos < len(
            self.dro_player.current_song.data
        ):
            inst = self.dro_player.current_song.data[self.dro_player.pos]
            if inst.inst_type == dro_data.DROInstruction.T_DELAY:
                delay = inst.value
                # If we go past the intended seek time, don't increment the position counter. This way we end up
                #  before the seek time, rather than after it.
                if self.dro_player.time_elapsed + delay > seek_time_ms:
                    break
                self.dro_player.time_elapsed += delay
            elif inst.inst_type == dro_data.DROInstruction.T_BANK_SWITCH:
                self.dro_player.processing_streams.bank = inst.value  # DRO v1
            # elif inst.inst_type == dro_data.DROInstruction.T_REGISTER:
            else:
                if inst.bank is not None:  # DRO v2
                    self.dro_player.processing_streams.bank = inst.bank
                self.dro_player.processing_streams.write(inst.command, inst.value)
                self.dro_player.writes_elapsed += 1
            self.dro_player.pos += 1
        self.dro_player.processing_streams.clear_chip_delay_drift()

    @stop_player_on_exception
    def seek_to_pos(self, seek_pos: int) -> None:
        """Seeks to a particular instruction position.
        This method is useful for playing a song from an instruction highlighted in the table editor.
        Note the position has no real bearing on the length of the song in ms - for a song with 200 instructions,
        40 of them might be initializing registers/operators.
        """
        if not self.dro_player.current_song:
            return
        seek_pos = min(
            seek_pos, len(self.dro_player.current_song.data)
        )  # make sure seek_pos is within bounds
        self.dro_player.pos = 0
        self.dro_player.time_elapsed = 0
        self.dro_player.writes_elapsed = 0
        while self.dro_player.pos < seek_pos:
            inst = self.dro_player.current_song.data[self.dro_player.pos]
            if inst.inst_type == dro_data.DROInstruction.T_DELAY:
                self.dro_player.time_elapsed += inst.value
            elif inst.inst_type == dro_data.DROInstruction.T_BANK_SWITCH:
                self.dro_player.processing_streams.bank = inst.value  # DRO v1
            # elif inst.inst_type == dro_data.DROInstruction.T_REGISTER:
            else:
                if inst.bank is not None:  # DRO v2
                    self.dro_player.processing_streams.bank = inst.bank
                self.dro_player.processing_streams.write(inst.command, inst.value)
                self.dro_player.writes_elapsed += 1
            self.dro_player.pos += 1
        self.dro_player.processing_streams.clear_chip_delay_drift()


class DROPlayerUpdateThread(threading.Thread):
    PERCUSSION_REGISTER = 0xBD

    def __init__(self, dro_player: DROPlayer, current_song: dro_data.DROSong):
        super(DROPlayerUpdateThread, self).__init__()
        self.dro_player = dro_player  # circular reference, yuck
        self.current_song = current_song
        self.stop_request = threading.Event()
        self.active_channels = set(self.dro_player.active_channels)
        self.active_percussion = set(self.dro_player.active_percussion)

    @stop_player_on_exception
    def run(self):
        while (
            self.dro_player.pos < len(self.current_song.data)
            and self.dro_player.is_playing
            and not self.stop_request.is_set()
        ):
            # First, check if we need to mute a channel register.
            new_muted_channels = self.active_channels - self.dro_player.active_channels
            for channel in new_muted_channels:
                self.dro_player.processing_streams.bank = (channel & 0x100) >> 8
                self.dro_player.processing_streams.write(channel & 0xFF, 0x00)
            # Check if we need to unmute a channel register (also remove muted channels).
            if self.dro_player.active_channels ^ self.active_channels:
                self.active_channels = set(self.dro_player.active_channels)

            # Process the instruction.
            inst = self.dro_player.current_song.data[self.dro_player.pos]
            if inst.inst_type == dro_data.DROInstruction.T_DELAY:
                self.dro_player.processing_streams.render(inst.value)
                self.dro_player.time_elapsed += inst.value
            elif inst.inst_type == dro_data.DROInstruction.T_BANK_SWITCH:
                self.dro_player.processing_streams.bank = inst.value  # DRO v1
            # elif inst.inst_type == dro_data.DROInstruction.T_REGISTER:
            else:
                if inst.bank is not None:  # DRO v2
                    self.dro_player.processing_streams.bank = inst.bank
                # Check if this is a channel register, and if so, if it should be muted.
                # Percussion channel is handled separately
                if inst.command == self.PERCUSSION_REGISTER:
                    # Need to pass through the 3 high bits of the percussion channel.
                    # We rely on the bitmask to handle this.
                    mask = self.dro_player.active_percussion[
                        self.dro_player.processing_streams.bank
                    ]
                    val = inst.value & mask
                    self.dro_player.processing_streams.write(inst.command, val)
                # Non-channel registers get a pass.
                elif inst.command not in self.dro_player.CHANNEL_REGISTERS:
                    self.dro_player.processing_streams.write(inst.command, inst.value)
                # Only write to channel registers if they are active.
                elif (
                    self.dro_player.processing_streams.bank << 8
                ) | inst.command in self.active_channels:
                    self.dro_player.processing_streams.write(inst.command, inst.value)
                self.dro_player.writes_elapsed += 1
                self.dro_player.processing_streams.render_chip_delay()
            # Update position and stop if no more instructions.
            self.dro_player.pos += 1
            if self.dro_player.pos >= len(self.current_song.data):
                self.dro_player.is_playing = False
        self.dro_player.stop()


class _TimerUpdateThread(threading.Thread):
    def __init__(self, calc_ms_length: int):
        super(_TimerUpdateThread, self).__init__()
        self.time_elapsed = 0
        self.calc_ms_length = calc_ms_length
        self.stop_request = threading.Event()

    def run(self):
        calc_ms_length_string = dro_util.ms_to_timestr(self.calc_ms_length)
        last_run_time = time.monotonic()
        last_minutes = 0
        last_seconds = 0
        while not self.stop_request.is_set():
            curr_time = time.monotonic()
            delta = curr_time - last_run_time
            last_run_time = curr_time
            self.time_elapsed += delta
            minutes, seconds = divmod(math.floor(self.time_elapsed), 60)
            if minutes != last_minutes or seconds != last_seconds:
                last_minutes = minutes
                last_seconds = seconds
                sys.stdout.write(
                    "\r{} / {}".format(
                        dro_util.to_timestr(minutes, seconds), calc_ms_length_string
                    )
                )
                sys.stdout.flush()
            time.sleep(0.01)


def __parse_arguments():
    usage = (
        "Usage: %prog [options] dro_file\n\n"
        + "Plays a DRO song. Can also be used to render a song to a single WAV file.\n\n"
        + "Keyboard shorcuts:\n"
        + "  0-9: solo channel\n"
        + "  ~: unmute all channels\n"
        + "  -: switch to the low bank\n"
        + "  +: switch to the high bank (OPL-3)\n"
        "  CTRL-C: cancel playback"
    )
    version = dro_globals.g_app_version
    oparser = optparse.OptionParser(usage, version=version)
    oparser.add_option(
        "-r",
        "--render",
        action="store_true",
        dest="render",
        default=False,
        help="Render the song to a WAV file. Sound output is disabled.",
    )
    options, args = oparser.parse_args()
    return oparser, options, args


def main():
    """As a bonus, this module can be used as a standalone program to play a DRO song!"""
    oparser, options, args = __parse_arguments()
    if len(args) < 1:
        print("Please pass the name of the song to play as the first argument.")
        oparser.print_help()
        return 1
    song_to_play = args[0]
    if not os.path.isfile(song_to_play):
        print("Song does not appear to exist, or is not a file: %s" % song_to_play)
        return 3

    file_reader = dro_io.DroFileIO()
    dro_song = file_reader.read(song_to_play)
    dro_player = DROPlayer(
        recording_on=options.render,
        sound_on=not options.render,
    )
    dro_player.load_song(dro_song)
    print(dro_song.pretty_string())

    timer_thread = None
    try:
        calc_ms_length = dro_analysis.DROTotalDelayWithWriteDelayCalculator().sum_delay(
            dro_song
        )
        calc_ms_length_string = dro_util.ms_to_timestr(calc_ms_length)
        if options.render:
            dro_player.play()
            while dro_player.is_playing:
                sys.stdout.write(
                    "\r{} / {}".format(
                        dro_util.ms_to_timestr(
                            dro_player.time_with_write_delay_elapsed
                        ),
                        calc_ms_length_string,
                    )
                )
                sys.stdout.flush()
                time.sleep(0.05)
            # Print the end time too (but cheat)
            sys.stdout.write(
                "\r{} / {}".format(calc_ms_length_string, calc_ms_length_string)
            )
        else:
            dro_player.play()
            timer_thread = _TimerUpdateThread(calc_ms_length)
            timer_thread.start()
            bank = 0
            while dro_player.is_playing:
                # Check for user input.
                chin = dro_util.getch()
                if chin:
                    if 48 <= ord(chin) <= 57:  # solo channels
                        if int(chin) == 0:
                            channel = 0xBD
                        else:
                            channel = 0xB0 + int(chin) - 1
                        channel |= bank << 8
                        dro_player.active_channels = set([channel])
                    elif chin == b"`" or chin == b"~":  # reset
                        dro_player.active_channels = set(dro_player.CHANNEL_REGISTERS)
                    elif chin == b"-" or chin == b"_":  # switch to bank 0
                        bank = 0
                    elif chin == b"=" or chin == b"+":  # switch to bank 1
                        bank = 1
                time.sleep(0.01)
            # Print the end time too (but cheat)
            sys.stdout.write(
                "\r{} / {}".format(calc_ms_length_string, calc_ms_length_string)
            )
    except KeyboardInterrupt:
        pass
    except Exception as e:
        print(e)
        return 2
    finally:
        dro_player.close_audio_output()
        if dro_player.is_playing:
            dro_player.stop()
        if timer_thread is not None:
            timer_thread.stop_request.set()
            if timer_thread.is_alive():  # not quite right, but meh.
                timer_thread.join()
    return 0


if __name__ == "__main__":
    sys.exit(main())
