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
from pathlib import Path
from py2exe import freeze
import shutil
import zipfile
import drotrimmer.dro_globals as dro_globals


def remove_existing_directory(directory: str) -> None:
    path_obj = Path(directory)
    if path_obj.is_dir() and path_obj.exists():
        shutil.rmtree(path_obj)


opts = {
    "bundle_files": 2,
    "dll_excludes": ["MSVCP90.dll"],
    "excludes": ["tkinter"],
    "includes": [],
    "packages": ["drotrimmer", "drotrimmer.dtgui"]
}


def convert_version(in_version: str) -> str:
    ver_bits = [v[1:] for v in in_version.split()]
    if len(ver_bits) == 2:
        ver_bits.append("0")
    return '.'.join(ver_bits)


remove_existing_directory('dist/')

freeze(
    version_info={
        "version": convert_version(dro_globals.g_app_version),
        "description": "DRO Trimmer",
        "product_name": "DRO Trimmer",
        "company_name": "Laurence Dougal Myers",
        "comments": "jestarjokin@jestarjokin.net"
    },
    windows=[
        {
            "script": "drotrim.py",
            "icon_resources": [(1, "drotrimmer/dt.ico")],
        }
    ],
    data_files=[(".", ["drotrimmer/drotrim.ini"])],
    console=[
        {
            "script": "dro_player.py"
        },
        {
            "script": "dro2to1.py"
        },
        {
            "script": "dro_split.py"
        },
    ],
    options=opts
)

print("Packaging into a zip file...")

# TODO:
# - Build HTML docs from wiki src (Creole)

remove_existing_directory('../dist')
Path('../dist').mkdir()
today_version = datetime.date.today().isoformat().replace('-', '')
with zipfile.ZipFile(
        '../dist/drotrim_bin_{0}.zip'.format(today_version),
        'w',
        zipfile.ZIP_DEFLATED,
        compresslevel=9) as zip:
    for in_file in Path('../docs/').glob('*'):
        zip.write(in_file, arcname=('drotrim' / in_file.relative_to('../docs')))
    for in_file in Path('dist/').glob('*'):
        zip.write(in_file, arcname=('drotrim' / in_file.relative_to('dist/')))

# TODO: package the src
