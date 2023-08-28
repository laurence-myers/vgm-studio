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
import os.path
import sys
import wx

from .. import (
    dro_analysis,
    dro_config,
    dro_data,
    dro_globals,
    dro_io,
    dro_logging,
    dro_player,
    dro_undo,
    dro_util,
)
from .containers import DTMainFrame
from .dialogs import DTDialogGoto, DTDialogFindReg, DROInfoDialog, LoopAnalysisDialog
from .tables import EVT_FIRST_SELECTED_ITEM_CHANGED, FirstSelectedItemChangedEvent
from .ui_util import guiID, errorAlert, catchUnhandledExceptions, requiresDROLoaded
from . import tasks, waveform


class DTApp(wx.App):
    dro_player: dro_player.DROPlayer
    drosong: dro_data.DROSong | None
    frdialog: DTDialogFindReg | None
    goto_dialog: DTDialogGoto | None
    log: dro_logging.Logger = dro_logging.get_logger("DTApp")
    loop_analysis_dialog: LoopAnalysisDialog | None
    mainframe: DTMainFrame
    _playback_position_timer: wx.Timer
    playback_position_update_interval_ms: int = 10
    tail_length: int
    task_master: tasks.TaskMaster

    def OnInit(self):
        self.drosong: dro_data.DROSong | None = None
        self.dro_player: dro_player.DROPlayer = dro_player.DROPlayer()

        config = dro_config.get_config()
        self.tail_length = config.ui.tail_length
        self.goto_dialog: DTDialogGoto | None = None  # Goto diaog
        self.frdialog: DTDialogFindReg | None = None  # Find Register dialog
        self.loop_analysis_dialog: LoopAnalysisDialog | None = (
            None  # Loop Analysis Dialog
        )
        self.task_master: tasks.TaskMaster = tasks.TaskMaster()

        playback_position_timer_id = guiID("TIMER_PLAYBACK_POSITION")
        self._playback_position_timer: wx.Timer = wx.Timer(
            self, playback_position_timer_id
        )

        self.mainframe: DTMainFrame = DTMainFrame(
            self,
            None,
            -1,
            "DRO Trimmer %s" % (dro_globals.g_app_version,),
            # size=wx.Size(1900, 1200),
            tail_length=self.tail_length,
        )
        self.mainframe.Show(True)
        self.SetTopWindow(self.mainframe)

        self._RegisterEventHandlers()

        return True

    def _RegisterEventHandlers(self):
        self.mainframe.Bind(wx.EVT_MENU, self.menuOpenDRO, id=guiID("MENU_OPENDRO"))
        self.mainframe.Bind(wx.EVT_MENU, self.menuSaveDRO, id=guiID("MENU_SAVEDRO"))
        self.mainframe.Bind(wx.EVT_MENU, self.menuSaveDROAs, id=guiID("MENU_SAVEDROAS"))
        self.mainframe.Bind(wx.EVT_MENU, self.menuExit, id=wx.ID_EXIT)
        self.mainframe.Bind(wx.EVT_MENU, self.menuUndo, id=guiID("MENU_UNDO"))
        self.mainframe.Bind(wx.EVT_MENU, self.menuRedo, id=guiID("MENU_REDO"))
        self.mainframe.Bind(wx.EVT_MENU, self.menuGoto, id=guiID("MENU_GOTO"))
        self.mainframe.Bind(wx.EVT_MENU, self.menuFindReg, id=guiID("MENU_FINDREG"))
        self.mainframe.Bind(wx.EVT_MENU, self.menuDelete, id=guiID("MENU_DELETE"))
        self.mainframe.Bind(wx.EVT_MENU, self.menuDROInfo, id=guiID("MENU_DROINFO"))
        self.mainframe.Bind(
            wx.EVT_MENU, self.menuLoopAnalysis, id=guiID("MENU_LOOPANALYSIS")
        )
        self.mainframe.Bind(wx.EVT_MENU, self.menuHelp, id=wx.ID_HELP)
        self.mainframe.Bind(wx.EVT_MENU, self.menuAbout, id=guiID("MENU_ABOUT"))

        self.mainframe.Bind(wx.EVT_BUTTON, self.buttonDelete, id=guiID("BUTTON_DELETE"))
        self.mainframe.Bind(wx.EVT_BUTTON, self.buttonPlay, id=guiID("BUTTON_PLAY"))
        self.mainframe.Bind(wx.EVT_BUTTON, self.buttonStop, id=guiID("BUTTON_STOP"))
        self.mainframe.Bind(
            wx.EVT_BUTTON, self.buttonPlayTail, id=guiID("BUTTON_PLAY_TAIL")
        )

        self.mainframe.Bind(wx.EVT_CLOSE, self.closeFrame)

        self.Bind(wx.EVT_KEY_DOWN, self.keyListener)
        self.mainframe.Bind(wx.EVT_LIST_KEY_DOWN, self.keyListenerForList)

        self.Bind(
            wx.EVT_TIMER,
            self.onPlaybackPositionTimer,
            id=guiID("TIMER_PLAYBACK_POSITION"),
        )

        # Custom events
        self.Bind(EVT_FIRST_SELECTED_ITEM_CHANGED, self.onListItemSelected)
        self.Bind(waveform.EVT_WAVEFORM_GO_TO, self.onWaveformGoTo)
        self.Bind(waveform.EVT_WAVEFORM_HOVER, self.onWaveformHover)
        self.Bind(tasks.EVT_TASK_RESULT, self.onResult)
        self.Bind(tasks.EVT_TASK_COMPLETED, self.onTaskCompleted)

    # ____________________
    # Start Menu Event Handlers
    @catchUnhandledExceptions
    def menuOpenDRO(self, event):
        od = wx.FileDialog(
            self.mainframe,
            "Open DRO",
            wildcard="DRO files (*.dro)|*.dro|All Files|*.*",
            style=wx.FD_OPEN | wx.FD_FILE_MUST_EXIST | wx.FD_CHANGE_DIR,
        )
        result = od.ShowModal()
        filename = od.GetPath()
        od.Destroy()
        del od
        if result == wx.ID_OK:
            importer = dro_io.DroFileIO()
            self.drosong = importer.read(filename)

            # Delete first instruction if it's a bogus delay (mostly for V1)
            first_delay_analyzer = dro_analysis.DROFirstDelayAnalyzer()
            first_delay_analyzer.analyze_dro(self.drosong)
            if first_delay_analyzer.result:
                self.drosong.delete_instructions([0])
                auto_trimmed = True
            else:
                auto_trimmed = False

            # Check if the total delay calculated doesn't match the delay recorded
            #  in the DRO file header.
            delay_mismatch_analyzer = dro_analysis.DROTotalDelayMismatchAnalyzer()
            delay_mismatch_analyzer.analyze_dro(self.drosong)
            delay_mismatch = delay_mismatch_analyzer.result

            # Load detailed register analysis.
            # Delay running analysis for a fraction of a second, this gives a better user experience. For example,
            # when selecting an instruction and holding down the "delete" key to delete lots of instructions.
            # Also load the waveform
            self.__triggerDetailedRegisterAnalysisAndWaveform(debounce=False)
            self.mainframe.waveform_panel.set_playback_position_pct(0)

            self.dro_player.stop()
            self.dro_player.load_song(self.drosong)

            self.mainframe.dtlist.CreateList(self.drosong)
            self.setStatusText(
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
            dro_globals.get_undo_controller().reset()
            self.mainframe.GetMenuBar().updateUndoRedoMenuItems()

            # Reset the Goto dialog, if it exists.
            if self.goto_dialog is not None:
                self.goto_dialog.reset(len(self.drosong.data) - 1)
            # Reset the loop analysis dialog, if it exists.
            if self.loop_analysis_dialog is not None:
                self.loop_analysis_dialog.load_results(None)

    @catchUnhandledExceptions
    @requiresDROLoaded
    def menuSaveDRO(self, event):
        filename = self.drosong.name
        # Seeing as the filename is stored in the drosong, I should modify
        #  save_dro to only take a DROSong.
        dro_io.DroFileIO().write(filename, self.drosong)
        self.setStatusText("File saved to " + filename + ".")

    @requiresDROLoaded
    def menuSaveDROAs(self, event):
        sd = wx.FileDialog(
            self.mainframe,
            "Save DRO file",
            wildcard="DRO files (*.dro)|*.dro|All Files|*.*",
            style=wx.FD_SAVE | wx.FD_OVERWRITE_PROMPT | wx.FD_CHANGE_DIR,
        )
        if sd.ShowModal() == wx.ID_OK:
            self.drosong.name = sd.GetPath()
            self.menuSaveDRO(event)

    def menuExit(self, event):
        self.mainframe.Close(False)

    @catchUnhandledExceptions
    @requiresDROLoaded
    def menuGoto(self, event):
        if self.goto_dialog is not None:
            self.goto_dialog.Destroy()
        self.goto_dialog = DTDialogGoto(
            self, self.mainframe, len(self.drosong.data) - 1
        )
        self.goto_dialog.Show()

    @catchUnhandledExceptions  # Added by Wraithverge.
    @requiresDROLoaded
    def menuFindReg(self, event):
        if self.frdialog is not None:
            self.frdialog.Destroy()  # TODO: destroy the dialog when it closes normally! (bit of a memory leak)
        self.frdialog = DTDialogFindReg(self, self.mainframe, self.drosong.file_version)
        self.frdialog.Show()

    @catchUnhandledExceptions
    @requiresDROLoaded
    def menuLoopAnalysis(self, event):
        if self.loop_analysis_dialog is not None:
            self.loop_analysis_dialog.Destroy()
        # Create a dummy analyzer so we know how many result pages we need to create.
        analyzer = dro_analysis.DROLoopAnalyzer()
        self.loop_analysis_dialog = LoopAnalysisDialog(self, analyzer, self.mainframe)
        self.loop_analysis_dialog.Show()

    def menuDelete(self, event):
        self.buttonDelete(None)

    @catchUnhandledExceptions
    @requiresDROLoaded
    def menuDROInfo(self, event):
        dro_info_dialog = DROInfoDialog(self.mainframe, self.drosong)
        dro_info_dialog.ShowModal()
        dro_info_dialog.Destroy()

    @catchUnhandledExceptions
    @requiresDROLoaded
    def menuUndo(self, event):
        undo_desc = dro_globals.get_undo_controller().undo()
        if undo_desc:
            self.setStatusText("Undone: %s" % (undo_desc,))
            self.mainframe.dtlist.RefreshItemCount()
            self.mainframe.dtlist.RefreshViewableItems()
            self.mainframe.GetMenuBar().updateUndoRedoMenuItems()
            # Need to refresh detailed analysis and waveform, because instructions may have been re-added.
            self.__triggerDetailedRegisterAnalysisAndWaveform()
        else:
            self.setStatusText("Nothing to undo.")

    @catchUnhandledExceptions
    @requiresDROLoaded
    def menuRedo(self, event):
        redo_desc = dro_globals.get_undo_controller().redo()
        if redo_desc:
            self.setStatusText("Redone: %s" % (redo_desc,))
            self.mainframe.dtlist.RefreshItemCount()
            self.mainframe.dtlist.RefreshViewableItems()
            self.mainframe.GetMenuBar().updateUndoRedoMenuItems()
            # Need to refresh detailed analysis and waveform, because instructions may have been re-added.
            self.__triggerDetailedRegisterAnalysisAndWaveform()
        else:
            self.setStatusText("Nothing to redo.")

    def menuHelp(self, event):
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

    def menuAbout(self, event):
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
    @catchUnhandledExceptions
    @requiresDROLoaded
    def buttonDelete(self, _event):
        if (
            self.mainframe
            and self.mainframe.dtlist
            and self.mainframe.dtlist.HasSelected()
        ):
            self.dro_player.stop()
            # I think all of this should be moved to the dtlist...
            selected_items = self.mainframe.dtlist.GetAllSelected()
            self.drosong.delete_instructions(selected_items)
            self.mainframe.dtlist.RefreshItemCount()
            # Deselect all, and re-select only the first index we deleted,
            # or the last item in the list.
            first_item = selected_items[0]
            self.mainframe.dtlist.Deselect()
            if first_item < self.mainframe.dtlist.GetItemCount():
                newly_selected = first_item
            else:
                # Otherwise, select the list item in the list
                newly_selected = self.mainframe.dtlist.GetItemCount() - 1
            self.mainframe.dtlist.SelectItemManual(newly_selected)
            self.mainframe.dtlist.EnsureVisible(newly_selected)
            self.mainframe.dtlist.RefreshViewableItems()
            # Keep track of Undo buffer.
            # (Crap, requires knowledge that this is an "undoable" action.
            # Might be better to investigate triggering an event, or using
            # observer/listener pattern.)
            self.mainframe.GetMenuBar().updateUndoRedoMenuItems()
            # Also need to update the detailed register descriptions, since deleting an instruction will
            #  change the state of the chip after the deleted instructions.
            #  Unfortunately we need to update the whole lot. Could speed things up by storing "snapshots" of the
            #  chip state and only refreshing the descriptions, from the nearest snapshot before the first deleted
            #  instruction onwards.
            self.__triggerDetailedRegisterAnalysisAndWaveform()

    @requiresDROLoaded
    def buttonPlay(self, event):
        self.dro_player.stop()
        self.dro_player.reset()
        if self.mainframe.dtlist.HasSelected():
            self.dro_player.seek_to_pos(self.mainframe.dtlist.GetFirstSelected())
        self.dro_player.play()
        self._playback_position_timer.Start(self.playback_position_update_interval_ms)

    @requiresDROLoaded
    def buttonStop(self, event):
        self.dro_player.stop()
        self.dro_player.reset()
        self._playback_position_timer.Stop()

    @requiresDROLoaded
    def buttonPlayTail(self, event):
        self.dro_player.stop()
        self.dro_player.reset()
        self.dro_player.seek_to_time(
            max(self.dro_player.current_song.ms_length - self.tail_length, 0)
        )
        self.dro_player.play()
        self._playback_position_timer.Start(self.playback_position_update_interval_ms)

    @catchUnhandledExceptions
    def buttonGoto(self, event):
        position = self.goto_dialog.scPosition.GetValue()
        try:
            position = int(position)
        except Exception:
            self.setStatusText("Invalid position for goto: %s" % position)
            return
        if position < 0 or position >= len(self.drosong.data):
            self.setStatusText("Position for goto is out of range: %s" % position)
            return
        self.mainframe.dtlist.Deselect()
        self.mainframe.dtlist.SelectItemManual(position)
        self.mainframe.dtlist.EnsureVisible(position)
        self.mainframe.dtlist.RefreshViewableItems()
        self.setStatusText("Gone to position: %s" % position)

    @catchUnhandledExceptions
    def buttonFindReg(self, event, look_backwards=False):
        rToFind = self.frdialog.cbRegisters.GetValue()
        if rToFind == "":
            return
        if not self.mainframe.dtlist.HasSelected():
            start = 0
        else:
            start = self.mainframe.dtlist.GetLastSelected() + 1
        i = self.drosong.find_next_instruction(
            start,
            rToFind,
            look_backwards=look_backwards,
        )
        if i == -1:
            self.setStatusText("Could not find another occurrence of " + rToFind + ".")
            return
        self.mainframe.dtlist.Deselect()
        self.mainframe.dtlist.SelectItemManual(i)
        self.mainframe.dtlist.EnsureVisible(i)
        self.mainframe.dtlist.RefreshViewableItems()
        self.setStatusText(
            "Occurrence of " + rToFind + " found at position " + str(i) + "."
        )

    def buttonFindRegPrevious(self, event):
        self.buttonFindReg(event, look_backwards=True)  # blech

    @catchUnhandledExceptions
    @requiresDROLoaded
    def buttonNextNote(self, event, look_backwards=False):
        if not self.mainframe.dtlist.HasSelected():
            start = 0
        else:
            start = self.mainframe.dtlist.GetLastSelected() + 1
        i = self.drosong.find_next_instruction(
            start,
            "DALL",
            look_backwards=look_backwards,
        )
        if i == -1:
            self.setStatusText("No more notes found.")
            return
        self.mainframe.dtlist.Deselect()
        self.mainframe.dtlist.SelectItemManual(i)
        self.mainframe.dtlist.EnsureVisible(i)
        self.mainframe.dtlist.RefreshViewableItems()

    def buttonPreviousNote(self, event):
        self.buttonNextNote(event, look_backwards=True)

    @catchUnhandledExceptions
    @requiresDROLoaded
    def buttonAnalyzeLoop(self, event):
        if self.loop_analysis_dialog is None:
            errorAlert(
                self.mainframe,
                "Loop analysis requires the Loop Analysis dialog to be open, but none found.",
            )
            return
        analyzer = dro_analysis.DROLoopAnalyzer()
        results = analyzer.analyze_dro(self.drosong)
        self.loop_analysis_dialog.load_results(results)
        self.setStatusText("Loop analysis finished.")

    # ____________________
    # Start Misc Event Handlers
    def keyListenerForList(self, event):
        if not self:
            return
        keycode = event.GetKeyCode()
        if keycode in (wx.WXK_DELETE, wx.WXK_BACK):  # delete or backspace
            self.buttonDelete(None)
            event.Veto()
        elif keycode == wx.WXK_LEFT:
            # <-- key. Previous note
            self.buttonPreviousNote(event)
            event.Veto()
        elif keycode == wx.WXK_RIGHT:
            # --> key. Next note
            self.buttonNextNote(event)
            event.Veto()
        else:
            event.Skip()

    def keyListener(self, event):
        if not self:
            return
        keycode = event.GetKeyCode()
        if keycode == 70 and event.CmdDown():  # CTRL-F
            self.menuFindReg(event)
        elif keycode == 71 and event.CmdDown():  # CTRL-G
            self.menuGoto(event)
        elif keycode == 72 and event.CmdDown():  # CTRL-H
            self.menuHelp(event)
        elif keycode == 73 and event.CmdDown():  # CTRL-I
            self.menuDROInfo(event)
        elif keycode == 79 and event.CmdDown():  # CTRL-O
            self.menuOpenDRO(event)
        elif keycode == 83 and event.ShiftDown() and event.CmdDown():  # CTRL-SHIFT-S
            self.menuSaveDROAs(event)
        elif keycode == 83 and event.CmdDown():  # CTRL-S
            self.menuSaveDRO(event)
        elif keycode == 89 and event.CmdDown():  # CTRL-Y
            self.menuRedo(event)
        elif keycode == 90 and event.CmdDown():  # CTRL-Z
            self.menuUndo(event)
        elif keycode == 90:  # Z. TODO: remove this
            self.startTestTask(event)
        elif keycode == 83:  # S. TODO: remove this
            self.cancelTestTask(event)
        elif keycode == 32:  # Spacebar
            self.togglePlayback(event)
        else:
            # print keycode
            event.Skip()

    def startTestTask(self, _event):
        task_name = f"Task {self.task_master.get_num_tasks() + 1}"
        task = tasks.ExampleTask(task_name)
        self.task_master.start_task(task)
        self.log.debug(f"Starting {task_name}\n")

    def cancelTestTask(self, _event):
        task_name = f"Task {self.task_master.get_num_tasks()}"
        self.task_master.cancel_task(task_name)

    def onResult(self, event: tasks.TaskResultEvent):
        # self.log.debug(f"{event.task_name} result\n")

        if event.task_name == "DetailedRegisterAnalysisTask":
            if self.drosong:
                self.drosong.detailed_register_descriptions = event.value
                if self.mainframe.dtlist:
                    self.mainframe.dtlist.RefreshViewableItems()

        elif event.task_name == "WaveformRenderTask":
            self.mainframe.waveform_panel.redraw(event.value)

    def onTaskCompleted(self, event: tasks.TaskCompletedEvent):
        task_name = event.task_name
        self.task_master.remove_completed_task(task_name)
        self.log.debug(f"{task_name} completed\n")

        if event.task_name == "DetailedRegisterAnalysisTask":
            self.setStatusText("", section=1)

    def closeFrame(self, _event):
        self.dro_player.stop()
        self.dro_player.close_audio_output()
        self.mainframe.waveform_panel.stop()
        self.mainframe.Destroy()
        self.task_master.stop()

    def togglePlayback(self, event):
        if self.dro_player.is_playing:
            self.buttonStop(event)
        else:
            self.buttonPlay(event)

    def onListItemSelected(self, event: FirstSelectedItemChangedEvent) -> None:
        item: int | None = event.item_index
        self.log.debug(f"Got an item to select: {item}")
        if self.drosong.detailed_register_descriptions and item is not None:
            ms_offset = self.drosong.detailed_register_descriptions[item][2]
            self.log.debug(f"Selected item's ms offset: {ms_offset}")
            self.mainframe.waveform_panel.set_playback_start_indicator(
                ms_offset,
                self.drosong.ms_length,
            )

    def onPlaybackPositionTimer(self, _event: wx.TimerEvent) -> None:
        if not self.drosong or not self.dro_player.is_playing:
            return
        position_pct = self.dro_player.position_pct
        self.mainframe.waveform_panel.set_playback_position_pct(position_pct)

    def onWaveformGoTo(self, event: waveform.WaveformGoToEvent) -> None:
        pct = event.x_position_pct
        if self.drosong:
            result = self.drosong.get_index_and_ms_offset_by_position_pct(pct)
            if result is not None:
                index, _ = result
                self.mainframe.dtlist.Deselect()
                self.mainframe.dtlist.SelectItemManual(index)
                if self.dro_player.is_playing:
                    self.dro_player.seek_to_pos(index)

    def onWaveformHover(self, event: waveform.WaveformHoverEvent) -> None:
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
    def __updateDROInfoRedo(self, args_list):  # sigh
        self.updateDROInfo(*args_list)

    # @requiresDROLoaded # not really required here
    @dro_undo.undoable(
        "DRO Header Changes",
        dro_globals.get_undo_controller,
        __updateDROInfoRedo,
    )
    def updateDROInfo(self, opl_type, ms_length):
        original_values = [self.drosong.opl_type, self.drosong.ms_length]
        self.drosong.opl_type = opl_type
        self.drosong.ms_length = ms_length
        return original_values

    # Event/threaded stuff. Requires a little more delicacy.
    def setStatusText(self, message, section=0):
        if self.mainframe.statusbar:
            self.mainframe.statusbar.SetStatusText(message, section)

    def __triggerDetailedRegisterAnalysisAndWaveform(self, debounce: bool = True):
        self.__doDetailedRegisterAnalysis()
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

    def __doDetailedRegisterAnalysis(self):
        self.setStatusText("Analyzing registers....", section=1)

        self.task_master.start_task(tasks.DetailedRegisterAnalysisTask(self.drosong))


def start_gui_app():
    # Fix blurriness on Windows
    # @see https://stackoverflow.com/a/54247018/953887
    if sys.platform == "win32":
        try:
            ctypes.windll.shcore.SetProcessDpiAwareness(True)
        except:
            pass

    dro_globals.g_undo_controller = dro_undo.UndoController()
    app = DTApp(0)
    dro_globals.g_wx_app = app
    app.MainLoop()


if __name__ == "__main__":
    start_gui_app()
