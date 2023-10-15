import wx

from .ui_util import gui_id
from ..vgm.vgm_data import VGMSong, GD3Tag


class GD3TagDialog(wx.Dialog):
    def __init__(self, parent: wx.Window, vgm_song: VGMSong) -> None:
        super().__init__(parent)
        self.vgm_song = vgm_song
        tag = (
            self.vgm_song.tag
            if self.vgm_song.tag is not None
            else GD3Tag("", "", "", "", "", "", "", "", "", "", "")
        )

        self.b_save = wx.Button(self, gui_id("BUTTON_GD3_SAVE"), "Save")
        self.b_close = wx.Button(self, wx.ID_CANCEL, "Close")
        self.SetTitle("GD3 Tag")

        s_main = wx.GridSizer(2, 40, 40)
        s_buttons = wx.BoxSizer(wx.HORIZONTAL)

        self.fields = []
        for field_name, field_value in tag.iter_fields():
            label = wx.StaticText(self, -1, field_name)
            input_ctrl = wx.TextCtrl(self, -1, field_value)
            self.fields.append((label, input_ctrl))
            s_main.Add(label)
            s_main.Add(input_ctrl)

        s_main.Add((0, 0), 1, wx.EXPAND)
        s_buttons.Add(self.b_save, 1, wx.ALL | wx.ALIGN_BOTTOM)
        s_buttons.Add(self.b_close, 1, wx.ALL | wx.ALIGN_BOTTOM)
        s_main.Add(s_buttons, 1, wx.EXPAND | wx.ALIGN_RIGHT)
        self.SetSizer(s_main)
        self.Layout()
