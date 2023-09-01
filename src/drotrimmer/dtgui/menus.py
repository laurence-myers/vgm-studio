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
from .. import dro_globals
import wx

from .ui_util import gui_id


class DTMainMenuBar(wx.MenuBar):
    def __init__(self, *args, **kwds):
        wx.MenuBar.__init__(self, *args, **kwds)

        # File menu
        self.menu_file = wx.Menu()
        self.menu_file.Append(
            gui_id("MENU_OPENDRO"),
            "&Open DRO...\tCtrl-O",
            "Open a DRO file.",
            wx.ITEM_NORMAL,
        )
        self.menu_file.Append(
            gui_id("MENU_SAVEDRO"),
            "&Save DRO\tCtrl-S",
            "Save the current DRO file.",
            wx.ITEM_NORMAL,
        )
        self.menu_file.Append(
            gui_id("MENU_SAVEDROAS"),
            "Save DRO &As...\tCtrl-Shift-S",
            "Save the current DRO file under a new name.",
            wx.ITEM_NORMAL,
        )
        self.menu_file.AppendSeparator()
        self.menu_file.Append(
            wx.ID_EXIT, "E&xit", "Quit, begone, depart, flee.", wx.ITEM_NORMAL
        )
        self.Append(self.menu_file, "&File")

        self.menu_edit = wx.Menu()
        self.undo_menu_item = self.menu_edit.Append(
            gui_id("MENU_UNDO"),
            "&Undo\tCtrl-Z",
            "Undoes the last change you made to the data.",
            wx.ITEM_NORMAL,
        )
        self.redo_menu_item = self.menu_edit.Append(
            gui_id("MENU_REDO"),
            "&Redo\tCtrl-Y",
            "Redoes the previously undone change you made to the data.",
            wx.ITEM_NORMAL,
        )
        self.menu_edit.AppendSeparator()
        self.menu_edit.Append(
            gui_id("MENU_GOTO"),
            "&Goto...\tCtrl-G",
            "Goes to a specific position.",
            wx.ITEM_NORMAL,
        )
        self.menu_edit.Append(
            gui_id("MENU_FINDREG"),
            "&Find Register...\tCtrl-F",
            "Find the next occurrence of a register.",
            wx.ITEM_NORMAL,
        )
        self.menu_edit.Append(
            gui_id("MENU_LOOPANALYSIS"),
            "&Loop Analysis...\tCtrl-L",
            "Attempts to find sections of data that indicate a loop point.",
            wx.ITEM_NORMAL,
        )
        self.menu_edit.Append(
            gui_id("MENU_DROINFO"),
            "DRO &Info...\tCtrl-I",
            "View or edit the DRO file info (song length, hardware type)",
            wx.ITEM_NORMAL,
        )
        self.menu_edit.AppendSeparator()
        self.menu_edit.Append(
            gui_id("MENU_DELETE"),
            "&Delete Instruction(s)\tDEL",
            "Deletes the currently selected instruction.",
            wx.ITEM_NORMAL,
        )
        self.Append(self.menu_edit, "&Edit")

        # Help menu
        self.menu_help = wx.Menu()
        self.menu_help_help = wx.MenuItem(
            self.menu_help,
            wx.ID_HELP,
            "&Help...\tCtrl-H",
            "Displays a little bit of help.",
            wx.ITEM_NORMAL,
        )
        self.menu_help.Append(self.menu_help_help)
        self.menu_help_about = wx.MenuItem(
            self.menu_help,
            gui_id("MENU_ABOUT"),
            "&About...",
            "Open the about dialog.",
            wx.ITEM_NORMAL,
        )
        self.menu_help.Append(self.menu_help_about)
        self.Append(self.menu_help, "&Help")

        self.__set_properties()
        self.__do_layout()

    def __set_properties(self):
        self.undo_menu_item.Enable(False)
        self.redo_menu_item.Enable(False)

    def __do_layout(self):
        pass

    def update_undo_redo_menu_items(self):
        # Check if there's anything left to undo
        if dro_globals.g_undo_controller.has_something_to_undo():
            self.undo_menu_item.Enable(True)
        else:
            self.undo_menu_item.Enable(False)
            # Check if there's anything left to undo
        if dro_globals.g_undo_controller.has_something_to_redo():
            self.redo_menu_item.Enable(True)
        else:
            self.redo_menu_item.Enable(False)
