import wx

from .ui_util import gui_id
from ..vgm.vgm_data import VGMSong


class VgmMetadataDialog(wx.Dialog):
    def __init__(self, parent: wx.Window, vgm_song: VGMSong) -> None:
        super().__init__(parent)
        self.vgm_song = vgm_song

        self.b_save = wx.Button(self, gui_id("BUTTON_GD3_SAVE"), "Save")
        self.b_close = wx.Button(self, wx.ID_CANCEL, "Close")

        # Events
        self.Bind(wx.EVT_BUTTON, self._on_save, id=gui_id("BUTTON_VGM_METADATA_SAVE"))

        # Layout etc
        self.SetTitle("VGM Metadata")
        self.SetSize(self.FromDIP(wx.Size(400, 250)))

        s_main = wx.FlexGridSizer(2, 10, 10)
        s_buttons = wx.BoxSizer(wx.HORIZONTAL)

        def create_label(label_text: str) -> None:
            label = wx.StaticText(self, -1, label_text)
            s_main.Add(label, proportion=1)

        def create_input(value: str) -> wx.TextCtrl:
            input = wx.TextCtrl(
                self,
                -1,
                value,
            )
            input.SetMinSize(self.FromDIP((280, 30)))
            s_main.Add(input, proportion=3, flag=wx.EXPAND)
            return input

        create_label("Loop start:")
        self.input_loop_offset = create_input(str(self.vgm_song.loop_offset))
        create_label("Loop length:")
        self.input_loop_num_samples = create_input(str(self.vgm_song.loop_num_samples))
        create_label("Loop base:")
        self.input_loop_base = create_input(str(self.vgm_song.loop_base))
        create_label("Loop modifier:")
        self.input_loop_modifier = create_input(str(self.vgm_song.loop_modifier))
        create_label("Volume modifier:")
        self.input_volume_modifier = create_input(str(self.vgm_song.volume_modifier))

        s_main.Add((0, 0), 1, wx.EXPAND)
        s_buttons.Add(self.b_save, 1, wx.ALL | wx.ALIGN_BOTTOM)
        s_buttons.Add(self.b_close, 1, wx.ALL | wx.ALIGN_BOTTOM)
        s_main.Add(s_buttons, 3, wx.EXPAND | wx.ALIGN_RIGHT)
        self.SetSizer(s_main)
        self.Layout()

    def _on_save(self, _event: wx.Event) -> None:
        self.vgm_song.loop_offset = int(self.input_loop_offset.Value)
        self.vgm_song.loop_num_samples = int(self.input_loop_num_samples.Value)
        self.vgm_song.loop_base = int(self.input_loop_base.Value)
        self.vgm_song.loop_modifier = int(self.input_loop_modifier.Value)
        self.vgm_song.volume_modifier = int(self.input_volume_modifier.Value)
        self.Close()
