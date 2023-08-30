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

import datetime
import os
from pathlib import Path
from py2exe import freeze  # type: ignore
import shutil
import subprocess
import zipfile
import drotrimmer.dro_globals as dro_globals
from setup_docs import generate_docs


def filtered_walk(dir: Path):
    for dirpath, dirnames, filenames in os.walk(dir, topdown=True):
        # Filter out some directories
        dirnames[:] = set(dirnames) - {
            ".git",
            "__pycache__",
            "dist",
        }
        # Filter out some files
        filenames[:] = [
            f
            for f in filenames
            if Path(f).suffix
            not in {
                ".bak",
                ".dro",
                ".pyc",
            }
        ]
        for in_file_str in filenames:
            yield Path(dirpath) / in_file_str


# @see https://stackoverflow.com/a/2656405/953887
def on_remove_error(func, path, _exc_info):
    """
    Error handler for ``shutil.rmtree``.

    If the error is due to an access error (read only file)
    it attempts to add write permission and then retries.

    If the error is for another reason it re-raises the error.

    Usage : ``shutil.rmtree(path, onerror=onerror)``
    """
    import stat

    # Is the error an access error?
    if not os.access(path, os.W_OK):
        os.chmod(path, stat.S_IWUSR)
        func(path)
    else:
        raise


def remove_existing_directory(directory: str) -> None:
    path_obj = Path(directory)
    if path_obj.is_dir() and path_obj.exists():
        shutil.rmtree(path_obj, onerror=on_remove_error)


opts = {
    "bundle_files": 2,
    "dll_excludes": ["MSVCP90.dll"],
    "excludes": ["tkinter"],
    "includes": [],
    "packages": [
        "drotrimmer",
        "drotrimmer.dtgui",
    ],
}


def convert_version(in_version: str) -> str:
    ver_bits = [v[1:] for v in in_version.split()]
    if len(ver_bits) == 2:
        ver_bits.append("0")
    return ".".join(ver_bits)


remove_existing_directory("dist/")

freeze(
    version_info={
        "version": convert_version(dro_globals.g_app_version),
        "description": "DRO Trimmer",
        "product_name": "DRO Trimmer",
        "company_name": "Laurence Dougal Myers",
        "comments": "jestarjokin@jestarjokin.net",
    },
    windows=[
        {
            "script": "drotrim.py",
            "icon_resources": [(1, "dt.ico")],
        }
    ],
    data_files=[(".", ["drotrim.ini"])],
    console=[
        {
            "script": "dro_player.py",
        },
        {
            "script": "dro2to1.py",
        },
        {
            "script": "dro_split.py",
        },
    ],
    options=opts,
)

print("Building docs...")
remove_existing_directory("../docs_src")
docs_url = "https://bitbucket.org/jestar_jokin/dro-trimmer/wiki"
subprocess.run(["git", "clone", docs_url, "../docs_src"])
generate_docs("../docs_src", "../src", "../docs", docs_url)

print("Packaging binaries into a zip file...")
remove_existing_directory("../dist")
Path("../dist").mkdir()
today_version = datetime.date.today().isoformat().replace("-", "")
drotrim_path = Path("drotrim")

with zipfile.ZipFile(
    "../dist/drotrim_bin_{0}.zip".format(today_version),
    "w",
    zipfile.ZIP_DEFLATED,
    compresslevel=9,
) as zip:
    for in_file in Path("../docs/").rglob("*"):
        zip.write(in_file, arcname=(drotrim_path / in_file.relative_to("../docs")))

    for in_file in Path("dist/").rglob("*"):
        zip.write(in_file, arcname=(drotrim_path / in_file.relative_to("dist/")))

print("Packaging source files...")
with zipfile.ZipFile(
    "../dist/drotrim_src_{0}.zip".format(today_version),
    "w",
    zipfile.ZIP_DEFLATED,
    compresslevel=9,
) as zip:
    for in_file in filtered_walk(Path("../docs/")):
        zip.write(in_file, arcname=(drotrim_path / in_file.relative_to("../")))

    for in_file in filtered_walk(Path("../docs_src")):
        zip.write(in_file, arcname=(drotrim_path / in_file.relative_to("../")))

    for in_file in filtered_walk(Path("../res/")):
        zip.write(in_file, arcname=(drotrim_path / in_file.relative_to("../")))

    for in_file in filtered_walk(Path("../src/")):
        zip.write(in_file, arcname=(drotrim_path / in_file.relative_to("../")))
