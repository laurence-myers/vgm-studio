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
from typing import Any

import wx

from .. import dro_analysis, dro_config, dro_data, dro_globals
from .containers import TextPanel
from .ui_util import gui_id, error_alert


class DTDialogGoto(wx.Dialog):
    def __init__(self, wx_app, parent, max_pos, *args, **kwds):
        # begin wxGlade: DTDialogGoto.__init__
        kwds["style"] = wx.DEFAULT_DIALOG_STYLE
        wx.Dialog.__init__(self, parent, *args, **kwds)
        self.sc_position = wx.SpinCtrl(self, -1, "", min=0, max=max_pos)
        self.btn_go = wx.Button(self, gui_id("BUTTON_GOTO_GO"), "Go")
        self.btn_close = wx.Button(self, wx.ID_CANCEL, "Close")

        self.__set_properties()
        self.__do_layout()
        # end wxGlade
        self.parent = parent
        self.Bind(wx.EVT_BUTTON, wx_app.button_goto, id=gui_id("BUTTON_GOTO_GO"))

    def __set_properties(self):
        # begin wxGlade: DTDialogGoto.__set_properties
        self.SetTitle("Goto Position")
        self.btn_go.SetDefault()
        self.sc_position.SetValue("")
        # end wxGlade

    def __do_layout(self):
        # begin wxGlade: DTDialogGoto.__do_layout
        sz_main = wx.BoxSizer(wx.VERTICAL)
        sz_buttons = wx.BoxSizer(wx.HORIZONTAL)
        # Layout adjusted by Wraithverge to be consistent with Find Reg layout.
        sz_main.Add(self.sc_position, 0, wx.ALL | wx.ALIGN_CENTER, 5)
        sz_buttons.Add((0, 5), 0, 0, 0)
        sz_buttons.Add(self.btn_go, 0, wx.ALL | wx.ALIGN_CENTER_VERTICAL, 5)
        sz_buttons.Add(self.btn_close, 0, wx.ALL | wx.ALIGN_CENTER_VERTICAL, 5)
        sz_main.Add(sz_buttons, 0, wx.ALL, 0)
        self.SetSizer(sz_main)
        sz_main.Fit(self)
        self.Layout()
        # end wxGlade

    def reset(self, max_pos):
        self.sc_position.SetValue(0)
        self.sc_position.SetValue("")
        self.sc_position.SetRange(0, max_pos)


# end of class DTDialogGoto


class DTDialogFindReg(wx.Dialog):
    def __init__(self, wx_app, *args, **kwds):
        # begin wxGlade: DTDialogFindReg.__init__
        kwds["style"] = wx.DEFAULT_DIALOG_STYLE
        wx.Dialog.__init__(self, *args, **kwds)

        self.parent = args[0]
        self.dro_version = args[1]

        # Choices are special values for DRO commands plus registers
        # formerly: [hex(rk) for rk in registers.keys()]
        # but give the option to search for unknown registers (up to 0x105)
        if self.dro_version == dro_data.DRO_FILE_V2:
            # NOTE: could be some confusion with codemaps and low/high banks.
            # Currently looks up the real register value (note the codemap index), and ignores banks.
            self.regchoices = ["DLYS", "DLYL", "DALL"] + [
                ("0x%02X" % rk) for rk in range(0x100)
            ]
        else:
            self.regchoices = ["DLYS", "DLYL", "DALL", "BANK"] + [
                ("0x%02X" % rk) for rk in range(0x100)
            ]

        self.l_register = wx.StaticText(self, -1, "Instruction:")
        self.cb_registers = wx.ComboBox(
            self,
            -1,
            choices=self.regchoices,
            style=wx.CB_DROPDOWN | wx.CB_DROPDOWN | wx.CB_READONLY,
        )
        self.b_find_next = wx.Button(self, gui_id("BUTTON_FINDREG"), "Find Next")
        self.b_find_previous = wx.Button(
            self, gui_id("BUTTON_FINDREGPREV"), "Find Previous"
        )
        self.b_cancel = wx.Button(self, wx.ID_CANCEL, "Close")

        self.Bind(wx.EVT_BUTTON, wx_app.button_find_reg, id=gui_id("BUTTON_FINDREG"))
        self.Bind(
            wx.EVT_BUTTON,
            wx_app.button_find_reg_previous,
            id=gui_id("BUTTON_FINDREGPREV"),
        )

        self.__set_properties()
        self.__do_layout()
        # end wxGlade

    def __set_properties(self):
        # begin wxGlade: DTDialogFindReg.__set_properties
        self.SetTitle("Find Register")
        self.cb_registers.SetSelection(-1)
        # end wxGlade

    def __do_layout(self):
        # Alignment adjustments by Wraithverge
        # begin wxGlade: DTDialogFindReg.__do_layout
        s_main = wx.BoxSizer(wx.VERTICAL)
        s_middle = wx.BoxSizer(wx.HORIZONTAL)
        s_bottom = wx.BoxSizer(wx.HORIZONTAL)
        gs_top = wx.FlexGridSizer(1, 2, 0, 5)
        gs_top.Add(self.l_register, 1, wx.ALIGN_CENTER, 0)
        gs_top.Add(self.cb_registers, 0, 0, 0)
        s_main.Add(gs_top, 0, wx.ALL | wx.ALIGN_CENTER, 2)
        s_middle.Add(self.b_find_previous, 0, 0, 0)
        s_middle.Add(self.b_find_next, 0, 0, 0)
        s_main.Add(s_middle, 0, wx.ALL | wx.ALIGN_RIGHT, 5)
        s_bottom.Add(self.b_cancel, 0, wx.LEFT, 10)
        s_main.Add(s_bottom, 0, wx.ALL | wx.ALIGN_RIGHT, 5)
        self.SetSizer(s_main)
        s_main.Fit(self)
        self.Layout()
        # end wxGlade


# end of class DTDialogFindReg


class DROInfoDialog(wx.Dialog):
    def __init__(self, parent: wx.Window, dro_song: dro_data.AbstractSong) -> None:
        config = dro_config.get_config()
        dro_info_edit_enabled = config.ui.dro_info_edit_enabled
        # begin wxGlade: MyDialog.__init__
        self.parent = parent
        super().__init__(parent, style=wx.DEFAULT_DIALOG_STYLE)
        self.l_dro_version = wx.StaticText(self, -1, "DRO Version")
        self.tc_dro_version = wx.TextCtrl(self, -1, str(dro_song.file_version))
        self.l_hardware_type = wx.StaticText(self, -1, "Hardware Type")
        self.c_hardware_type = wx.Choice(
            self, -1, choices=[e.name for e in dro_data.OPLType]
        )
        self.c_hardware_type.Select(dro_song.opl_type.value)
        self.l_length_ms = wx.StaticText(self, -1, "Length (MS)")
        self.tc_length_ms = wx.TextCtrl(self, -1, str(dro_song.ms_length))
        self.l_length_ms_calc = wx.StaticText(self, -1, "Calculated Length (MS)")
        calculated_delay = dro_analysis.DROTotalDelayCalculator().sum_delay(dro_song)
        self.tc_kength_ms_calc = wx.TextCtrl(self, -1, str(calculated_delay))
        if dro_info_edit_enabled:
            self.b_edit = wx.Button(self, gui_id("BUTTON_DROINFO_EDIT"), "Edit")
        self.b_close = wx.Button(self, wx.ID_CANCEL, "Close")

        self.__set_properties()
        self.__do_layout(dro_info_edit_enabled)
        # end wxGlade

        self.dro_song = dro_song
        self.edit_mode = False
        if dro_info_edit_enabled:
            self.Bind(
                wx.EVT_BUTTON,
                self.edit_save_button_event,
                id=gui_id("BUTTON_DROINFO_EDIT"),
            )

    def __set_properties(self) -> None:
        # begin wxGlade: MyDialog.__set_properties
        self.SetTitle("DRO Info")
        self.SetSize((330, 242))
        self.tc_dro_version.Disable()
        self.c_hardware_type.Disable()
        self.tc_length_ms.Disable()
        self.tc_kength_ms_calc.Disable()
        self.b_close.SetDefault()
        # end wxGlade

    def __do_layout(self, dro_info_edit_enabled: bool) -> None:
        # begin wxGlade: MyDialog.__do_layout
        s_main = wx.GridSizer(5, 2, 0, 0)
        s_buttons = wx.BoxSizer(wx.HORIZONTAL)
        s_main.Add(self.l_dro_version, 0, wx.ALL, 5)
        s_main.Add(self.tc_dro_version, 0, wx.ALL, 5)
        s_main.Add(self.l_hardware_type, 0, wx.ALL, 5)
        s_main.Add(self.c_hardware_type, 0, wx.ALL, 5)
        s_main.Add(self.l_length_ms, 0, wx.ALL, 5)
        s_main.Add(self.tc_length_ms, 0, wx.ALL, 5)
        s_main.Add(self.l_length_ms_calc, 0, wx.ALL, 5)
        s_main.Add(self.tc_kength_ms_calc, 0, wx.ALL, 5)
        s_main.Add((0, 0), 1, wx.EXPAND, 5)
        if dro_info_edit_enabled:
            s_buttons.Add(self.b_edit, 1, wx.ALL | wx.ALIGN_BOTTOM, 5)
        else:
            s_buttons.Add((0, 0), 1, wx.ALL | wx.ALIGN_BOTTOM, 5)
        s_buttons.Add(self.b_close, 1, wx.ALL | wx.ALIGN_BOTTOM, 5)
        s_main.Add(s_buttons, 1, wx.EXPAND | wx.ALIGN_RIGHT, 5)
        self.SetSizer(s_main)
        self.Layout()

    def edit_save_button_event(self, event: Any) -> None:
        if self.edit_mode:
            self.save_changes(event)
        else:
            self.start_edit_mode(event)

    def start_edit_mode(self, _event: Any) -> None:
        wx.GetApp().set_status_text("DRO Info edit mode enabled.")
        self.edit_mode = True
        self.c_hardware_type.Enable()
        self.tc_length_ms.Enable()
        self.b_edit.SetLabel("Save")
        self.b_close.SetLabel("Cancel")

    def save_changes(self, _event: Any) -> None:
        try:
            opl_type = self.c_hardware_type.GetSelection()
            assert 0 <= opl_type < len(dro_data.OPLType)
            ms_length = int(self.tc_length_ms.GetValue())
        except Exception as _e:
            error_alert(
                self,
                "Error updating DRO info, check that the entered values are correct.",
            )
            return
        wx.GetApp().update_dro_info(opl_type, ms_length)
        md = wx.MessageDialog(
            self,
            "DRO info updated.\n" "Remember to save the file.",
            style=wx.OK | wx.ICON_INFORMATION,
        )
        md.ShowModal()
        md.Destroy()


class LoopAnalysisDialog(wx.Dialog):
    def __init__(self, wx_app, loop_analyzer, parent, **kwds):
        wx.Dialog.__init__(
            self,
            parent,
            style=wx.DEFAULT_DIALOG_STYLE
            | wx.RESIZE_BORDER
            | wx.MAXIMIZE_BOX
            | wx.MINIMIZE_BOX,
            **kwds
        )
        self.notebook = wx.Notebook(self, size=(400, 300))

        # Create buttons
        self.btn_analyze = wx.Button(self, gui_id("BUTTON_ANALYZE"), "Analyze")
        self.btn_close = wx.Button(self, wx.ID_CANCEL, "Close")

        # Create first page
        info_text = (
            "This is the loop analysis dialog.\n\n"
            "It provides multiple analyses to determine interesting parts of the song data, "
            "hinting at sections that may be loop points.\n\n"
            "Some analysis methods will work better than others, depending on the song, "
            "where the loop occurs, how many times the song loops, how much data exists "
            "after a loop point, etc.\n\n"
            "Please refer to the online documentation for more information."
        )

        page1 = TextPanel(self.notebook, info_text)
        self.notebook.AddPage(page1, "Info")

        # Create as many tabs as there are analysis methods.
        self.result_pages = []
        for i in range(loop_analyzer.num_analyses()):
            page = TextPanel(self.notebook, "No analysis performed yet.")
            self.notebook.AddPage(page, "#%d" % (i + 1,))
            self.result_pages.append(page)

        # Register events
        self.Bind(
            wx.EVT_BUTTON, wx_app.button_analyze_loop, id=gui_id("BUTTON_ANALYZE")
        )

        # Do other UI stuff
        self.__set_properties()
        self.__do_layout()

    def __set_properties(self):
        self.SetTitle("Loop Analysis")

    def __do_layout(self):
        # Lay things out
        sizer_buttons = wx.BoxSizer(wx.HORIZONTAL)
        sizer_buttons.Add(self.btn_analyze, 1, wx.ALIGN_BOTTOM, 0)
        sizer_buttons.Add(self.btn_close, 1, wx.ALIGN_BOTTOM, 0)

        sizer_main = wx.BoxSizer(wx.VERTICAL)
        sizer_main.Add(self.notebook, 1, wx.EXPAND, 0)
        sizer_main.Add(sizer_buttons, 0, wx.EXPAND, 0)
        self.SetSizer(sizer_main)
        sizer_main.Fit(self)
        self.Layout()

    def load_results(self, result_list):
        if result_list is None:
            result_list = ["No analysis performed yet."] * len(self.result_pages)
        for loop_analysis_result, page in zip(result_list, self.result_pages):
            page.set_text(str(loop_analysis_result))
