from typing import cast

from .dro_data import AbstractSong, SongFileType, DROSongV2, DROSongV1
from .dro_io import DroFileIO
from .dro_util import DROFileException
from .vgm.vgm_data import VGMSong
from .vgm.vgm_io import VgmFileIO


def read_song_from_file(file_name: str) -> AbstractSong:
    if file_name.lower().endswith(".dro"):
        return DroFileIO().read(file_name)
    elif file_name.lower().endswith(".vgm"):
        return VgmFileIO().read(file_name)
    else:
        raise DROFileException(f"Tried to read an unsupported file format: {file_name}")


def write_song_to_file(dro_song: AbstractSong) -> None:
    match dro_song.file_type:
        case SongFileType.DRO:
            DroFileIO().write(cast(DROSongV1 | DROSongV2, dro_song))
        case SongFileType.VGM:
            VgmFileIO().write(cast(VGMSong, dro_song))
        case _:
            raise DROFileException(
                f"Tried to save an unsupported file format: {dro_song.file_type}"
            )
