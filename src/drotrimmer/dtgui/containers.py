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
import os
import re
import sys
import wx

from .. import dro_config, dro_util

from .menus import DTMainMenuBar
from .playback_position_panel import PlaybackPositionPanel
from .tables import DTSongDataList
from .ui_util import custom_event, gui_id
from .waveform import WaveformPanel


_type_EVT_FILE_DROP, EVT_FILE_DROP = custom_event()


class FileDropEvent(wx.PyEvent):
    def __init__(self, filename: str) -> None:
        super().__init__(eventType=_type_EVT_FILE_DROP)
        self.filename = filename


class MainFrameDropTarget(wx.FileDropTarget):
    extension_re = re.compile(r"\.dro$", re.IGNORECASE)

    def OnDropFiles(self, _x: int, _y: int, filenames: list[str]):
        # Only allow 1 file, and it must have a supported extension.
        if len(filenames) == 1 and self.extension_re.search(filenames[0]):
            wx.PostEvent(wx.GetApp(), FileDropEvent(filenames[0]))
            return True
        return False


class DTMainFrame(wx.Frame):
    def __init__(self, *args, **kwds) -> None:
        kwds["style"] = wx.DEFAULT_FRAME_STYLE
        tail_length = kwds["tail_length"]
        del kwds["tail_length"]
        wx.Frame.__init__(self, *args, **kwds)

        # Maximize window base on config settings (added by Wraithverge)
        config = dro_config.get_config()
        self.Maximize(config.ui.maximize_window)

        # Set icon, if available
        use_external_icon = True
        exe_name = sys.executable
        # If the program is being run from the Python interpreter (and not
        #  a packged exe), use the external icon file. Otherwise, load the
        #  icon from the packaged exe resources.
        if not os.path.basename(exe_name).startswith("python"):
            icon = wx.Icon(exe_name, wx.BITMAP_TYPE_ICO)
            self.SetIcon(icon)
            use_external_icon = False
        if use_external_icon:
            exe_path = dro_util.get_exe_path()
            ico_name = os.path.join(exe_path, "dt.ico")
            icon = wx.Icon(ico_name, wx.BITMAP_TYPE_ICO)
            self.SetIcon(icon)

        self.statusbar = self.CreateStatusBar()

        self.splitter_1 = wx.SplitterWindow(self)

        self.waveform_panel: WaveformPanel = WaveformPanel(self.splitter_1)

        self.bottom_panel = wx.Panel(self.splitter_1)
        self.dtlist = DTSongDataList(self.bottom_panel, None)
        self.button_panel_1 = wx.Panel(self.bottom_panel)
        self.button_delete = wx.Button(
            self.button_panel_1, gui_id("BUTTON_DELETE"), "Delete instruction"
        )
        self.button_play = wx.Button(
            self.button_panel_1, gui_id("BUTTON_PLAY"), "Play song from current pos."
        )
        self.button_stop = wx.Button(
            self.button_panel_1, gui_id("BUTTON_STOP"), "Stop song"
        )

        tail_in_seconds = tail_length / 1000.0
        if tail_in_seconds % 1:
            tail_str = "%.2f" % (tail_in_seconds,)
        else:
            tail_str = "%d" % (tail_in_seconds,)
        self.button_play_tail = wx.Button(
            self.button_panel_1,
            gui_id("BUTTON_PLAY_TAIL"),
            "Play last %s second%s" % (tail_str, "s" if tail_in_seconds != 1 else ""),
        )

        self.playback_position_panel = PlaybackPositionPanel(self.bottom_panel)

        self.__set_properties()
        self.__do_layout()
        self.__bind_events()

    def __bind_events(self) -> None:
        self.SetDropTarget(MainFrameDropTarget())

    def __do_layout(self) -> None:
        self.splitter_1.SetMinimumPaneSize(100)

        grid_sizer_1 = wx.FlexGridSizer(5, 1, 0, 0)
        grid_sizer_1.Add(self.dtlist, 1, wx.EXPAND, 0)

        sizer_1 = wx.BoxSizer(wx.HORIZONTAL)
        sizer_1.Add(self.button_delete, 0, wx.FIXED_MINSIZE, 0)
        sizer_1.Add(self.button_play, 0, wx.FIXED_MINSIZE, 0)
        sizer_1.Add(self.button_stop, 0, wx.FIXED_MINSIZE, 0)
        sizer_1.Add(self.button_play_tail, 0, wx.FIXED_MINSIZE, 0)
        self.button_panel_1.SetAutoLayout(1)
        self.button_panel_1.SetSizer(sizer_1)
        sizer_1.Fit(self.button_panel_1)
        sizer_1.SetSizeHints(self.button_panel_1)

        grid_sizer_1.Add(self.button_panel_1, 1, wx.EXPAND, 0)
        grid_sizer_1.Add(
            wx.StaticLine(
                self.bottom_panel,
                style=wx.LI_HORIZONTAL,
            ),
            wx.SizerFlags(0).Align(wx.ALIGN_TOP).Expand(),
        )

        # Info panel showing playback position
        grid_sizer_1.Add(self.playback_position_panel, wx.SizerFlags(0).Expand())

        self.bottom_panel.SetAutoLayout(1)
        self.bottom_panel.SetSizer(grid_sizer_1)
        grid_sizer_1.Fit(self.bottom_panel)
        grid_sizer_1.SetSizeHints(self.bottom_panel)

        grid_sizer_1.AddGrowableCol(0, self.dtlist.GetBestSize().width)
        grid_sizer_1.AddGrowableRow(0, 90)

        self.splitter_1.SplitHorizontally(self.waveform_panel, self.bottom_panel, 150)

        self.statusbar.SetStatusWidths([-2, -1])

        self.Layout()
        self.SetSize(self.ToDIP(wx.Size(1900, 1200)))
        self.Centre()

    def __set_properties(self) -> None:
        self.SetMenuBar(DTMainMenuBar())
        self.statusbar.SetFieldsCount(2)

    def set_playback_length(self, ms: int, samples: int) -> None:
        self.playback_position_panel.set_playback_length(ms, samples)

    def set_playback_position(self, position_ms: int, position_samples: int) -> None:
        self.playback_position_panel.set_playback_position(
            position_ms, position_samples
        )


class TextPanel(wx.Panel):
    def __init__(self, parent, text=None):
        wx.Panel.__init__(self, parent)
        if text is None:
            text = ""
        self.textCtrl = wx.TextCtrl(self, -1, text, style=wx.TE_MULTILINE)
        self.__set_properties()
        self.__do_layout()

    def __set_properties(self):
        self.textCtrl.SetEditable(False)

    def __do_layout(self):
        sizer = wx.BoxSizer()
        sizer.Add(self.textCtrl, 1, wx.EXPAND)
        self.SetSizer(sizer)
        sizer.Fit(self)
        self.Layout()

    def set_text(self, text):
        self.textCtrl.SetValue(text)
