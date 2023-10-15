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
import ctypes
import optparse
import os.path
import sys
from typing import Any, cast

import wx

from ..dro_data import SongFileType
from ..vgm.vgm_data import VGMSong
from .. import (
    dro_analysis,
    dro_config,
    dro_data,
    dro_globals,
    dro_logging,
    dro_player,
    dro_undo,
    dro_util,
    file_io,
)
from .containers import DTMainFrame, EVT_FILE_DROP, FileDropEvent
from .dialogs import DTDialogGoto, DTDialogFindReg, DROInfoDialog, LoopAnalysisDialog
from .gd3_tag_dialog import GD3TagDialog, EVT_TAG_UPDATE, TagUpdateEvent
from .tables import EVT_FIRST_SELECTED_ITEM_CHANGED, FirstSelectedItemChangedEvent
from .ui_util import (
    gui_id,
    error_alert,
    catch_unhandled_exceptions,
    requires_dro_loaded,
)
from . import tasks, waveform


class UpdateHeaderCommand(dro_undo.UndoableCommand):
    def __init__(
        self, dro_song: dro_data.AbstractSong, opl_type: int, ms_length: int
    ) -> None:
        super().__init__("DRO Header Changes")
        self.dro_song = dro_song
        self.opl_type = opl_type
        self.ms_length = ms_length
        self.original_opl_type = self.dro_song.opl_type
        self.original_ms_length = self.ms_length

    def apply(self) -> None:
        self.dro_song.opl_type = dro_data.OPLType(self.opl_type)
        self.dro_song.ms_length = self.ms_length

    def revert(self) -> None:
        self.dro_song.opl_type = self.original_opl_type
        self.dro_song.ms_length = self.original_ms_length


class DTApp(wx.App):
    dro_player: dro_player.DROPlayer
    drosong: dro_data.AbstractSong | None
    frdialog: DTDialogFindReg | None
    goto_dialog: DTDialogGoto | None
    log: dro_logging.Logger = dro_logging.get_logger("DTApp")
    loop_analysis_dialog: LoopAnalysisDialog | None
    mainframe: DTMainFrame
    _playback_position_timer: wx.Timer
    playback_position_update_interval_ms: int = 10
    tail_length: int
    task_master: tasks.TaskMaster
    undo_controller: dro_undo.UndoController

    def OnInit(self) -> bool:
        self.undo_controller = dro_undo.UndoController()
        self.drosong: dro_data.AbstractSong | None = None
        self.dro_player: dro_player.DROPlayer = dro_player.DROPlayer()

        config = dro_config.get_config()
        self.tail_length = config.ui.tail_length
        self.goto_dialog: DTDialogGoto | None = None  # Goto diaog
        self.frdialog: DTDialogFindReg | None = None  # Find Register dialog
        self.loop_analysis_dialog: LoopAnalysisDialog | None = (
            None  # Loop Analysis Dialog
        )
        self.task_master: tasks.TaskMaster = tasks.TaskMaster()

        playback_position_timer_id = gui_id("TIMER_PLAYBACK_POSITION")
        self._playback_position_timer: wx.Timer = wx.Timer(
            self, playback_position_timer_id
        )

        self.mainframe: DTMainFrame = DTMainFrame(
            None,
            -1,
            "DRO Trimmer %s" % (dro_globals.g_app_version,),
            # size=wx.Size(1900, 1200),
            tail_length=self.tail_length,
        )
        self.mainframe.Show(True)
        self.SetTopWindow(self.mainframe)

        self._register_event_handlers()

        return True

    def _register_event_handlers(self):
        self.mainframe.Bind(wx.EVT_MENU, self.menu_open_dro, id=gui_id("MENU_OPENDRO"))
        self.mainframe.Bind(wx.EVT_MENU, self.menu_save_dro, id=gui_id("MENU_SAVEDRO"))
        self.mainframe.Bind(
            wx.EVT_MENU, self.menu_save_dro_as, id=gui_id("MENU_SAVEDROAS")
        )
        self.mainframe.Bind(wx.EVT_MENU, self.menu_exit, id=wx.ID_EXIT)
        self.mainframe.Bind(wx.EVT_MENU, self.menu_undo, id=gui_id("MENU_UNDO"))
        self.mainframe.Bind(wx.EVT_MENU, self.menu_redo, id=gui_id("MENU_REDO"))
        self.mainframe.Bind(wx.EVT_MENU, self.menu_goto, id=gui_id("MENU_GOTO"))
        self.mainframe.Bind(wx.EVT_MENU, self.menu_find_reg, id=gui_id("MENU_FINDREG"))
        self.mainframe.Bind(wx.EVT_MENU, self.menu_delete, id=gui_id("MENU_DELETE"))
        self.mainframe.Bind(wx.EVT_MENU, self.menu_dro_info, id=gui_id("MENU_DROINFO"))
        self.mainframe.Bind(wx.EVT_MENU, self.menu_edit_tag, id=gui_id("MENU_EDIT_TAG"))
        self.mainframe.Bind(
            wx.EVT_MENU, self.menu_loop_analysis, id=gui_id("MENU_LOOPANALYSIS")
        )
        self.mainframe.Bind(
            wx.EVT_MENU, self.menu_convert_to_vgm, id=gui_id("MENU_CONVERT_TO_VGM")
        )
        self.mainframe.Bind(wx.EVT_MENU, self.menu_help, id=wx.ID_HELP)
        self.mainframe.Bind(wx.EVT_MENU, self.menu_about, id=gui_id("MENU_ABOUT"))

        self.mainframe.Bind(
            wx.EVT_BUTTON, self.button_delete, id=gui_id("BUTTON_DELETE")
        )
        self.mainframe.Bind(wx.EVT_BUTTON, self.button_play, id=gui_id("BUTTON_PLAY"))
        self.mainframe.Bind(wx.EVT_BUTTON, self.button_stop, id=gui_id("BUTTON_STOP"))
        self.mainframe.Bind(
            wx.EVT_BUTTON, self.button_play_tail, id=gui_id("BUTTON_PLAY_TAIL")
        )

        self.mainframe.Bind(wx.EVT_CLOSE, self.close_frame)

        self.Bind(wx.EVT_KEY_DOWN, self._key_listener)
        self.mainframe.Bind(wx.EVT_LIST_KEY_DOWN, self._key_listener_for_list)
        self._register_accelerators()

        self.Bind(
            wx.EVT_TIMER,
            self.on_playback_position_timer,
            id=gui_id("TIMER_PLAYBACK_POSITION"),
        )

        # Custom events
        self.Bind(EVT_FILE_DROP, self.on_file_drop)
        self.Bind(EVT_FIRST_SELECTED_ITEM_CHANGED, self.on_list_item_selected)
        self.Bind(EVT_TAG_UPDATE, self.on_tag_update)
        self.Bind(waveform.EVT_WAVEFORM_GO_TO, self.on_waveform_go_to)
        self.Bind(waveform.EVT_WAVEFORM_HOVER, self.on_waveform_hover)
        self.Bind(tasks.EVT_TASK_RESULT, self.on_result)
        self.Bind(tasks.EVT_TASK_COMPLETED, self.on_task_completed)

    # ____________________
    # Start Menu Event Handlers
    @catch_unhandled_exceptions
    def menu_open_dro(self, _event) -> None:
        od = wx.FileDialog(
            self.mainframe,
            "Open DRO",
            wildcard="DRO or VGM (*.dro;*.vgm)|*.dro;*.vgm|"
            + "DRO files (*.dro)|*.dro|"
            + "VGM files (*.vgm)|*.vgm|"
            + "All Files|*.*",
            style=wx.FD_OPEN | wx.FD_FILE_MUST_EXIST | wx.FD_CHANGE_DIR,
        )
        result = od.ShowModal()
        filename = od.GetPath()
        od.Destroy()
        del od
        if result == wx.ID_OK:
            self.__load_file(filename)

    def __load_file(self, filename: str) -> None:
        try:
            self.drosong = file_io.read_song_from_file(filename)
            if not self.drosong:  # Just to keep mypy happy
                return

            if self.drosong.file_type == dro_data.SongFileType.DRO:
                # Delete first instruction if it's a bogus delay (mostly for V1)
                first_delay_analyzer = dro_analysis.DROFirstDelayAnalyzer()
                first_delay_analyzer.analyze_dro(self.drosong)
                if first_delay_analyzer.result:
                    self.undo_controller.execute(
                        dro_data.DeleteInstructionsCommand(self.drosong, [0])
                    )
                    auto_trimmed = True
                else:
                    auto_trimmed = False

                # Check if the total delay calculated doesn't match the delay recorded
                #  in the DRO file header.
                delay_mismatch_analyzer = dro_analysis.DROTotalDelayMismatchAnalyzer()
                delay_mismatch_analyzer.analyze_dro(self.drosong)
                delay_mismatch = delay_mismatch_analyzer.result
            else:
                auto_trimmed = False
                delay_mismatch = None

            # Load detailed register analysis.
            # Delay running analysis for a fraction of a second, this gives a better user experience. For example,
            # when selecting an instruction and holding down the "delete" key to delete lots of instructions.
            # Also load the waveform
            self.__trigger_detailed_register_analysis_and_waveform(debounce=False)
            self.mainframe.waveform_panel.set_playback_position_pct(0)

            self.dro_player.stop()
            self.dro_player.load_song(self.drosong)

            self.mainframe.dtlist.create_list(self.drosong)
            self.set_status_text(
                "Successfully opened " + os.path.basename(filename) + "."
            )

            # File was auto-trimmed, notify user
            dats = "T"  # despite auto-trimming string
            if auto_trimmed:
                dats = "Despite auto-trimming, t"
                md = wx.MessageDialog(
                    self.mainframe,
                    "The DRO was found to contain a bogus delay as\n"
                    + "its first instruction. It has been automatically\n"
                    + "removed. (Don't forget to save!)",
                    "DRO auto-trimmed",
                    style=wx.OK | wx.ICON_INFORMATION,
                )
                md.ShowModal()
                # File has mismatch between measured and reported
            if delay_mismatch:
                msg = (
                    dats
                    + "here was a mismatch between\n"
                    + "the measured length of the song in milliseconds,\n"
                    + "and the length stored in the DRO file.\n"
                )
                if self.drosong.file_version == dro_data.DRO_FILE_V1:
                    msg += "Please re-save the file to use the calculated value."
                else:
                    msg += (
                        'Please set "dro_info_edit_enabled" to "true"\n'
                        + "in drotrim.ini, then edit the song length on\n"
                        + "the DRO Info screen."
                    )
                md = wx.MessageDialog(
                    self.mainframe,
                    msg,
                    "DRO timing mismatch",
                    style=wx.OK | wx.ICON_INFORMATION,
                )
                md.ShowModal()

            # Reset undo history when a new file is opened.
            self.undo_controller.reset()
            self.mainframe.GetMenuBar().update_undo_redo_menu_items()

            # Reset the Goto dialog, if it exists.
            if self.goto_dialog is not None:
                self.goto_dialog.reset(len(self.drosong.data) - 1)
            # Reset the loop analysis dialog, if it exists.
            if self.loop_analysis_dialog is not None:
                self.loop_analysis_dialog.load_results(None)
        except dro_util.DROFileException as e:
            error_alert(self.mainframe, str(e), "Failed to load file")
        except FileNotFoundError as e:
            error_alert(self.mainframe, str(e), "Failed to open file")

    @catch_unhandled_exceptions
    @requires_dro_loaded
    def menu_save_dro(self, _event: Any) -> None:
        if not self.drosong:
            return
        file_io.write_song_to_file(self.drosong)
        self.set_status_text(f"File saved to {self.drosong.name}.")

    @requires_dro_loaded
    def menu_save_dro_as(self, event):
        sd = wx.FileDialog(
            self.mainframe,
            "Save DRO file",
            wildcard="DRO or VGM (*.dro;*.vgm)|*.dro;*.vgm|"
            + "DRO files (*.dro)|*.dro|"
            + "VGM files (*.vgm)|*.vgm|"
            + "All Files|*.*",
            style=wx.FD_SAVE | wx.FD_OVERWRITE_PROMPT | wx.FD_CHANGE_DIR,
        )
        if sd.ShowModal() == wx.ID_OK:
            self.drosong.name = sd.GetPath()
            self.menu_save_dro(event)

    def menu_exit(self, _event):
        self.mainframe.Close(False)

    @catch_unhandled_exceptions
    @requires_dro_loaded
    def menu_goto(self, _event):
        if self.goto_dialog is not None:
            self.goto_dialog.Destroy()
        self.goto_dialog = DTDialogGoto(
            self, self.mainframe, len(self.drosong.data) - 1
        )
        self.goto_dialog.Show()

    @catch_unhandled_exceptions  # Added by Wraithverge.
    @requires_dro_loaded
    def menu_find_reg(self, _event):
        if self.frdialog is not None:
            self.frdialog.Destroy()  # TODO: destroy the dialog when it closes normally! (bit of a memory leak)
        self.frdialog = DTDialogFindReg(self, self.mainframe, self.drosong.file_version)
        self.frdialog.Show()

    @catch_unhandled_exceptions
    @requires_dro_loaded
    def menu_loop_analysis(self, _event):
        if self.loop_analysis_dialog is not None:
            self.loop_analysis_dialog.Destroy()
        # Create a dummy analyzer so we know how many result pages we need to create.
        analyzer = dro_analysis.DROLoopAnalyzer()
        self.loop_analysis_dialog = LoopAnalysisDialog(self, analyzer, self.mainframe)
        self.loop_analysis_dialog.Show()

    def menu_delete(self, _event):
        self.button_delete(None)

    @catch_unhandled_exceptions
    @requires_dro_loaded
    def menu_dro_info(self, _event):
        dro_info_dialog = DROInfoDialog(self.mainframe, self.drosong)
        dro_info_dialog.ShowModal()
        dro_info_dialog.Destroy()

    @catch_unhandled_exceptions
    @requires_dro_loaded
    def menu_edit_tag(self, _event: Any) -> None:
        if self.drosong.file_type != SongFileType.VGM:
            self.set_status_text("Only VGMs support tag editing")
            return
        tag_edit_dialog = GD3TagDialog(self.mainframe, cast(VGMSong, self.drosong))
        tag_edit_dialog.Show()
        # tag_edit_dialog.Destroy()

    @catch_unhandled_exceptions
    @requires_dro_loaded
    def menu_undo(self, _event):
        undo_desc = self.undo_controller.undo()
        if undo_desc:
            self.set_status_text("Undone: %s" % (undo_desc,))
            self.mainframe.dtlist.refresh_item_count()
            self.mainframe.dtlist.refresh_viewable_items()
            self.mainframe.GetMenuBar().update_undo_redo_menu_items()
            # Need to refresh detailed analysis and waveform, because instructions may have been re-added.
            self.__trigger_detailed_register_analysis_and_waveform()
        else:
            self.set_status_text("Nothing to undo.")

    @catch_unhandled_exceptions
    @requires_dro_loaded
    def menu_redo(self, _event):
        redo_desc = self.undo_controller.redo()
        if redo_desc:
            self.set_status_text("Redone: %s" % (redo_desc,))
            self.mainframe.dtlist.refresh_item_count()
            self.mainframe.dtlist.refresh_viewable_items()
            self.mainframe.GetMenuBar().update_undo_redo_menu_items()
            # Need to refresh detailed analysis and waveform, because instructions may have been re-added.
            self.__trigger_detailed_register_analysis_and_waveform()
        else:
            self.set_status_text("Nothing to redo.")

    @catch_unhandled_exceptions
    @requires_dro_loaded
    def menu_convert_to_vgm(self, _event):
        if self.drosong.file_type == SongFileType.VGM:
            self.set_status_text("File is already in VGM format")
            return
        self.drosong = VGMSong.from_song(self.drosong)
        self.undo_controller.reset()  # Can't undo this operation, so just wipe the history.
        self.__trigger_detailed_register_analysis_and_waveform()  # Need to re-analyse the file, just to be safe.
        self.set_status_text("Successfully converted to VGM")

    def menu_help(self, _event):
        hd = wx.MessageDialog(
            self.mainframe,
            "Full instructions are available online.\n"
            + "https://bitbucket.org/jestar_jokin/dro-trimmer/wiki/Home\n"
            "\n"
            + "1) Select an instruction.\n"
            + "2) Delete via button or the Del key.\n"
            + "3) Profit!\n\n"
            + "If you're trimming a looping song, look for a\n"
            + "whole bunch of instructions with no delays, as\n"
            + "this might be where the instruments are set up.",
            "Help",
            style=wx.OK | wx.ICON_INFORMATION,
        )
        hd.ShowModal()
        hd.Destroy()

    def menu_about(self, _event):
        ad = wx.MessageDialog(
            self.mainframe,
            (
                "DRO Trimmer " + dro_globals.g_app_version + "\n"
                "Laurence Dougal Myers\n"
                + "Web: http://www.jestarjokin.net/apps/drotrimmer\n"
                + "Web: https://bitbucket.org/jestar_jokin/dro-trimmer/\n"
                + "E-Mail: jestarjokin@jestarjokin.net\n\n"
                + "Thanks to:\n"
                + "The DOSBOX team\n"
                + "The AdPlug team\n"
                + "Adam Nielsen for PyOPL\n"
                + "Wraithverge for testing, feedback and contributions\n"
                + "pi-r-squared for their original attempt at a DRO editor"
            ),
            "About",
            style=wx.OK | wx.ICON_INFORMATION,
        )
        ad.ShowModal()
        ad.Destroy()

    # ____________________
    # Start Button Event Handlers
    @catch_unhandled_exceptions
    @requires_dro_loaded
    def button_delete(self, _event):
        if (
            self.mainframe
            and self.mainframe.dtlist
            and self.mainframe.dtlist.has_selected()
        ):
            self.dro_player.stop()
            # I think all of this should be moved to the dtlist...
            selected_items = self.mainframe.dtlist.get_all_selected()
            self.undo_controller.execute(
                dro_data.DeleteInstructionsCommand(self.drosong, selected_items)
            )
            self.mainframe.dtlist.refresh_item_count()
            # Deselect all, and re-select only the first index we deleted,
            # or the last item in the list.
            first_item = selected_items[0]
            self.mainframe.dtlist.deselect()
            if first_item < self.mainframe.dtlist.GetItemCount():
                newly_selected = first_item
            else:
                # Otherwise, select the list item in the list
                newly_selected = self.mainframe.dtlist.GetItemCount() - 1
            self.mainframe.dtlist.select_item_manual(newly_selected)
            self.mainframe.dtlist.EnsureVisible(newly_selected)
            self.mainframe.dtlist.refresh_viewable_items()
            # Keep track of Undo buffer.
            # (Crap, requires knowledge that this is an "undoable" action.
            # Might be better to investigate triggering an event, or using
            # observer/listener pattern.)
            self.mainframe.GetMenuBar().update_undo_redo_menu_items()
            # Also need to update the detailed register descriptions, since deleting an instruction will
            #  change the state of the chip after the deleted instructions.
            #  Unfortunately we need to update the whole lot. Could speed things up by storing "snapshots" of the
            #  chip state and only refreshing the descriptions, from the nearest snapshot before the first deleted
            #  instruction onwards.
            self.__trigger_detailed_register_analysis_and_waveform()

    @requires_dro_loaded
    def button_play(self, _event):
        self.dro_player.stop()
        self.dro_player.reset()
        if self.mainframe.dtlist.has_selected():
            self.dro_player.seek_to_pos(self.mainframe.dtlist.GetFirstSelected())
        self.dro_player.play()
        self._playback_position_timer.Start(self.playback_position_update_interval_ms)

    @requires_dro_loaded
    def button_stop(self, _event):
        self.dro_player.stop()
        self.dro_player.reset()
        self._playback_position_timer.Stop()

    @requires_dro_loaded
    def button_play_tail(self, _event):
        self.dro_player.stop()
        self.dro_player.reset()
        self.dro_player.seek_to_time(
            max(self.dro_player.current_song.ms_length - self.tail_length, 0)
        )
        self.dro_player.play()
        self._playback_position_timer.Start(self.playback_position_update_interval_ms)

    @catch_unhandled_exceptions
    def button_goto(self, _event):
        position = self.goto_dialog.sc_position.GetValue()
        try:
            position = int(position)
        except Exception:
            self.set_status_text("Invalid position for goto: %s" % position)
            return
        if position < 0 or position >= len(self.drosong.data):
            self.set_status_text("Position for goto is out of range: %s" % position)
            return
        self.mainframe.dtlist.deselect()
        self.mainframe.dtlist.select_item_manual(position)
        self.mainframe.dtlist.EnsureVisible(position)
        self.mainframe.dtlist.refresh_viewable_items()
        self.set_status_text("Gone to position: %s" % position)

    @catch_unhandled_exceptions
    def button_find_reg(self, _event, look_backwards=False):
        r_to_find = self.frdialog.cb_registers.GetValue()
        if r_to_find == "":
            return
        if not self.mainframe.dtlist.has_selected():
            start = 0
        else:
            start = self.mainframe.dtlist.get_last_selected()
        i = self.drosong.find_next_instruction(
            start,
            r_to_find,
            look_backwards=look_backwards,
        )
        if i == -1:
            self.set_status_text(
                "Could not find another occurrence of " + r_to_find + "."
            )
            return
        self.mainframe.dtlist.deselect()
        self.mainframe.dtlist.select_item_manual(i)
        self.mainframe.dtlist.EnsureVisible(i)
        self.mainframe.dtlist.refresh_viewable_items()
        self.set_status_text(
            "Occurrence of " + r_to_find + " found at position " + str(i) + "."
        )

    def button_find_reg_previous(self, event):
        self.button_find_reg(event, look_backwards=True)  # blech

    @catch_unhandled_exceptions
    @requires_dro_loaded
    def button_next_delay(self, _event, look_backwards=False):
        if not self.mainframe.dtlist.has_selected():
            start = 0
        else:
            start = self.mainframe.dtlist.get_last_selected()
        i = self.drosong.find_next_instruction(
            start,
            "DALL",
            look_backwards=look_backwards,
        )
        if i == -1:
            self.set_status_text("No more delays found.")
            return
        self.mainframe.dtlist.deselect()
        self.mainframe.dtlist.select_item_manual(i)
        self.mainframe.dtlist.EnsureVisible(i)
        self.mainframe.dtlist.refresh_viewable_items()

    def button_previous_delay(self, event):
        self.button_next_delay(event, look_backwards=True)

    @catch_unhandled_exceptions
    @requires_dro_loaded
    def button_analyze_loop(self, _event):
        if self.loop_analysis_dialog is None:
            error_alert(
                self.mainframe,
                "Loop analysis requires the Loop Analysis dialog to be open, but none found.",
            )
            return
        analyzer = dro_analysis.DROLoopAnalyzer()
        results = analyzer.analyze_dro(self.drosong)
        self.loop_analysis_dialog.load_results(results)
        self.set_status_text("Loop analysis finished.")

    # ____________________
    # Start Misc Event Handlers
    def _key_listener_for_list(self, event: Any) -> None:
        if not self:
            return
        keycode = event.GetKeyCode()
        if keycode == wx.WXK_LEFT:
            # <-- key. Previous delay
            self.button_previous_delay(event)
            event.Veto()
        elif keycode == wx.WXK_RIGHT:
            # --> key. Next delay
            self.button_next_delay(event)
            event.Veto()
        elif keycode == 32:  # Spacebar
            self.toggle_playback(event)
        else:
            # print keycode
            event.Skip()

    def _key_listener(self, event: wx.KeyEvent) -> None:
        keycode = event.GetKeyCode()
        # On spacebar events, we don't want to catch text entry in the tag editor.
        # So, we only toggle playback if the main list or waveform panel has focus.
        # This also allows users to use spacebar to trigger buttons via standard keyboard navigation.
        if (
            keycode == 32
            and self.mainframe.dtlist.HasFocus()
            or self.mainframe.waveform_panel.HasFocus()
        ):
            self.toggle_playback(event)
        else:
            event.Skip()

    def _register_accelerators(self) -> None:
        accelerator_entries = [
            # Buttons
            wx.AcceleratorEntry(
                wx.ACCEL_NORMAL, wx.WXK_DELETE, gui_id("BUTTON_DELETE")
            ),
            wx.AcceleratorEntry(wx.ACCEL_NORMAL, wx.WXK_BACK, gui_id("BUTTON_DELETE")),
            # wx.AcceleratorEntry(
            #     wx.ACCEL_NORMAL, wx.WXK_LEFT, gui_id("BUTTON_PREVIOUS_DELAY")
            # ),
            # wx.AcceleratorEntry(
            #     wx.ACCEL_NORMAL, wx.WXK_RIGHT, gui_id("BUTTON_NEXT_DELAY")
            # ),
            # Menu items
            wx.AcceleratorEntry(wx.ACCEL_CTRL, ord("F"), gui_id("MENU_FINDREG")),
            wx.AcceleratorEntry(wx.ACCEL_CTRL, ord("G"), gui_id("MENU_GOTO")),
            wx.AcceleratorEntry(wx.ACCEL_CTRL, ord("H"), wx.ID_HELP),
            wx.AcceleratorEntry(wx.ACCEL_CTRL, ord("I"), gui_id("MENU_DROINFO")),
            wx.AcceleratorEntry(wx.ACCEL_CTRL, ord("O"), gui_id("MENU_OPENDRO")),
            wx.AcceleratorEntry(
                wx.ACCEL_CTRL | wx.ACCEL_SHIFT, ord("S"), gui_id("MENU_SAVEDROAS")
            ),
            wx.AcceleratorEntry(wx.ACCEL_CTRL, ord("S"), gui_id("MENU_SAVEDRO")),
            wx.AcceleratorEntry(wx.ACCEL_CTRL, ord("Y"), gui_id("MENU_REDO")),
            wx.AcceleratorEntry(wx.ACCEL_CTRL, ord("Z"), gui_id("MENU_UNDO")),
            # Custom
            # wx.AcceleratorEntry(
            #     wx.ACCEL_NORMAL, wx.WXK_SPACE, gui_id("BUTTON_TOGGLE_PLAYBACK")
            # ),
        ]
        self.mainframe.SetAcceleratorTable(wx.AcceleratorTable(accelerator_entries))

    def on_result(self, event: tasks.TaskResultEvent):
        # self.log.debug(f"{event.task_name} result\n")

        if event.task_name == "DetailedRegisterAnalysisTask":
            if self.drosong:
                self.drosong.detailed_register_descriptions = event.value
                if self.mainframe.dtlist:
                    self.mainframe.dtlist.refresh_viewable_items()

        elif event.task_name == "WaveformRenderTask":
            self.mainframe.waveform_panel.redraw(event.value)

    def on_task_completed(self, event: tasks.TaskCompletedEvent):
        task_name = event.task_name
        self.task_master.remove_completed_task(task_name)
        self.log.debug(f"{task_name} completed\n")

        if event.task_name == "DetailedRegisterAnalysisTask":
            self.set_status_text("", section=1)

    def close_frame(self, _event):
        self.dro_player.stop()
        self.dro_player.close_audio_output()
        self.mainframe.waveform_panel.stop()
        self.mainframe.Destroy()
        self.task_master.stop()

    def toggle_playback(self, event):
        if self.dro_player.is_playing:
            self.button_stop(event)
        else:
            self.button_play(event)

    @catch_unhandled_exceptions
    def on_file_drop(self, event: FileDropEvent):
        self.log.debug(f"File drop event received. Filename: {event.filename}")
        self.__load_file(event.filename)

    @catch_unhandled_exceptions
    def on_tag_update(self, event: TagUpdateEvent) -> None:
        if not self.drosong or self.drosong.file_type != SongFileType.VGM:
            return
        cast(VGMSong, self.drosong).tag = event.tag

    def on_list_item_selected(self, event: FirstSelectedItemChangedEvent) -> None:
        if not self.drosong:
            return
        item: int | None = event.item_index
        self.log.debug(f"Got an item to select: {item}")
        if self.drosong.detailed_register_descriptions and item is not None:
            ms_offset = self.drosong.detailed_register_descriptions[item][2]
            self.log.debug(f"Selected item's ms offset: {ms_offset}")
            self.mainframe.waveform_panel.set_playback_start_indicator(
                ms_offset,
                self.drosong.ms_length,
            )
            self._update_playback_position_info(ms_offset)

    def on_playback_position_timer(self, _event: wx.TimerEvent) -> None:
        if not self.drosong:
            return
        self._update_playback_position_info()
        if self.dro_player.is_playing:
            position_pct = self.dro_player.position_pct
            self.mainframe.waveform_panel.set_playback_position_pct(position_pct)

    def on_waveform_go_to(self, event: waveform.WaveformGoToEvent) -> None:
        pct = event.x_position_pct
        if self.drosong:
            result = self.drosong.get_index_and_ms_offset_by_position_pct(pct)
            if result is not None:
                index, ms = result
                self.mainframe.dtlist.deselect()
                self.mainframe.dtlist.select_item_manual(index)
                if self.dro_player.is_playing:
                    self.dro_player.seek_to_pos(index)
                self._update_playback_position_info(ms)

    def on_waveform_hover(self, event: waveform.WaveformHoverEvent) -> None:
        pct = event.x_position_pct
        if pct is None:
            self.mainframe.waveform_panel.clear_hover_indicator()
        elif self.drosong:
            result = self.drosong.get_index_and_ms_offset_by_position_pct(pct)
            ms_offset: int | None = None if result is None else result[1]
            if ms_offset is None:
                self.mainframe.waveform_panel.clear_hover_indicator()
            else:
                self.mainframe.waveform_panel.set_hover_indicator(
                    ms_offset,
                    self.drosong.ms_length,
                )

    # Other stuff

    def update_dro_info(self, opl_type: int, ms_length: int) -> None:
        if self.drosong:
            self.undo_controller.execute(
                UpdateHeaderCommand(self.drosong, opl_type, ms_length)
            )

    # Event/threaded stuff. Requires a little more delicacy.
    def set_status_text(self, message: str, section: int = 0) -> None:
        if self.mainframe.statusbar:
            self.mainframe.statusbar.SetStatusText(message, section)

    def __trigger_detailed_register_analysis_and_waveform(
        self, debounce: bool = True
    ) -> None:
        if not self.drosong:
            return
        self.__do_detailed_register_analysis()
        # TODO: don't reach into the waveform panel to get the player or num_buckets
        self.mainframe.waveform_panel.clear()
        self.task_master.start_task(
            tasks.WaveformRenderTask(
                self.mainframe.waveform_panel.dro_player,
                self.drosong,
                self.mainframe.waveform_panel.x_resolution,
            ),
            debounce_sec=1 if debounce else None,
        )
        self._update_playback_length_info()

    def __do_detailed_register_analysis(self) -> None:
        if not self.drosong:
            return
        self.set_status_text("Analyzing registers....", section=1)
        self.task_master.start_task(tasks.DetailedRegisterAnalysisTask(self.drosong))

    def _update_playback_length_info(self) -> None:
        if not self.drosong:
            return
        self.mainframe.set_playback_length(
            self.drosong.ms_length,
            dro_util.calculate_playback_samples(
                self.drosong.ms_length,
                self.dro_player.frequency,
                self.dro_player.channels,
                self.dro_player.bit_depth,
            ),
        )

    def _update_playback_position_info(self, ms: int | None = None) -> None:
        if ms is None:
            ms = self.dro_player.time_elapsed
            samples = self.dro_player.position_samples
        else:
            samples = dro_util.calculate_playback_samples(
                ms,
                self.dro_player.frequency,
                self.dro_player.channels,
                self.dro_player.bit_depth,
            )
        self.mainframe.set_playback_position(
            ms,
            samples,
        )


def __parse_arguments():
    usage = (
        "Usage: %prog [dro_file]\n\n"
        + "Opens a GUI to edit a DRO song. Optionally pass the name of a file to open."
    )
    version = dro_globals.g_app_version
    oparser = optparse.OptionParser(usage, version=version)
    options, args = oparser.parse_args()
    return oparser, options, args


def start_gui_app():
    _oparser, _options, args = __parse_arguments()

    # Fix blurriness on Windows
    # @see https://stackoverflow.com/a/54247018/953887
    if sys.platform == "win32":
        try:
            ctypes.windll.shcore.SetProcessDpiAwareness(True)
        except:
            pass

    app = DTApp(0)
    dro_globals.g_wx_app = app

    # If we were passed a file name arg, queue it up to be loaded in the GUI.
    if len(args) > 0:
        initial_file_name = args[0]
        wx.PostEvent(
            app,
            FileDropEvent(  # re-use the existing file drop event class
                initial_file_name
            ),
        )

    app.MainLoop()


if __name__ == "__main__":
    start_gui_app()
