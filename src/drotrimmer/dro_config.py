import configparser
from dataclasses import dataclass
import os

from .dro_logging import get_logger
from .dro_util import get_exe_path, DROTrimmerException

__config = None
__log = get_logger("Config")


@dataclass(frozen=True, slots=True)
class DROConfigAudio:
    bit_depth: int = 16
    buffer_size: int = 512
    chip_write_delay: float = 0
    frequency: int = 48000


@dataclass(frozen=True, slots=True)
class DROConfigUI:
    dro_info_edit_enabled: bool = False
    maximize_window: bool = False
    tail_length: int = 3000


@dataclass(frozen=True, slots=True)
class DROConfig:
    audio: DROConfigAudio = DROConfigAudio()
    ui: DROConfigUI = DROConfigUI()


def get_config() -> DROConfig:
    global __config
    global __log
    if __config is None:
        try:
            __config = __read_config()
        except Exception as e:
            __log.warning(
                "Could not read config from drotrim.ini, using default values. (Error: %s)"
                % e
            )
            __config = DROConfig()
        __log.debug(__config)
    return __config


def __read_config() -> DROConfig:
    parsed_config = configparser.ConfigParser()
    # Mitigate issue #4 by always searching for a config file in the same
    #  path as the executable.
    exe_path = get_exe_path()
    config_files_parsed = parsed_config.read(
        ["drotrim.ini", os.path.join(exe_path, "drotrim.ini")]
    )
    if not len(config_files_parsed):
        raise DROTrimmerException("Could not read drotrim.ini.")

    default_config = DROConfig()
    audio_config = DROConfigAudio(
        bit_depth=parsed_config.getint(
            "audio",
            "bit_depth",
            fallback=default_config.audio.bit_depth,
        ),
        buffer_size=parsed_config.getint(
            "audio",
            "buffer_size",
            fallback=default_config.audio.buffer_size,
        ),
        chip_write_delay=parsed_config.getfloat(
            "audio",
            "chip_write_delay",
            fallback=default_config.audio.chip_write_delay,
        ),
        frequency=parsed_config.getint(
            "audio",
            "frequency",
            fallback=default_config.audio.frequency,
        ),
    )
    ui_config = DROConfigUI(
        dro_info_edit_enabled=parsed_config.getboolean(
            "ui",
            "dro_info_edit_enabled",
            fallback=default_config.ui.dro_info_edit_enabled,
        ),
        maximize_window=parsed_config.getboolean(
            "ui",
            "maximize_window",
            fallback=default_config.ui.maximize_window,
        ),
        tail_length=parsed_config.getint(
            "ui",
            "tail_length",
            fallback=default_config.ui.tail_length,
        ),
    )
    return DROConfig(audio_config, ui_config)
