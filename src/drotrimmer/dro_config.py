import configparser
import os

from .dro_util import get_exe_path, DROTrimmerException

__config = None


def get_config():
    global __config
    if __config is None:
        __config = configparser.SafeConfigParser()
        # Mitigate issue #4 by always searching for a config file in the same
        #  path as the executable.
        exe_path = get_exe_path()
        config_files_parsed = __config.read(['drotrim.ini', os.path.join(exe_path, 'drotrim.ini')])
        if not len(config_files_parsed):
            raise DROTrimmerException("Could not read drotrim.ini.")
    return __config
