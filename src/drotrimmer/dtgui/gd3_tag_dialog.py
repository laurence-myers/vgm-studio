import wx

from .ui_util import gui_id, custom_event
from ..vgm.vgm_data import VGMSong, GD3Tag


_type_EVT_TAG_UPDATE, EVT_TAG_UPDATE = custom_event()


class TagUpdateEvent(wx.PyEvent):
    def __init__(self, tag: GD3Tag) -> None:
        super().__init__(eventType=_type_EVT_TAG_UPDATE)
        self.tag = tag


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

        # Events
        self.Bind(wx.EVT_BUTTON, self._on_gd3_save, id=gui_id("BUTTON_GD3_SAVE"))

        # Layout etc
        self.SetTitle("GD3 Tag")
        self.SetSize(self.FromDIP(wx.Size(400, 550)))

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
            input.SetMinSize(self.FromDIP((250, 30)))
            s_main.Add(input, proportion=3, flag=wx.EXPAND)
            return input

        create_label("Track Name (EN):")
        self.input_track_name_en = create_input(tag.track_name_en)
        create_label("Track Name (orig):")
        self.input_track_name_native = create_input(tag.track_name_native)

        create_label("Game Name (EN):")
        self.input_game_name_en = create_input(tag.game_name_en)
        create_label("Game Name (orig):")
        self.input_game_name_native = create_input(tag.game_name_native)

        # TODO: maybe derive this from AbstractSong.opl_type. Make it a dropdown
        create_label("System Name (EN):")
        self.input_system_name_en = create_input(tag.system_name_en)
        create_label("System Name (orig):")
        self.input_system_name_native = create_input(tag.system_name_native)

        create_label("Track Author (EN):")
        self.input_track_author_en = create_input(tag.track_author_en)
        create_label("Track Author (orig):")
        self.input_track_author_native = create_input(tag.track_author_native)

        create_label("Release Date:")
        self.input_release_date = create_input(tag.release_date)

        create_label("Creator:")
        self.input_creator = create_input(tag.creator)

        create_label("Notes:")
        self.input_notes = wx.TextCtrl(
            self,
            -1,
            tag.notes,
            style=wx.TE_MULTILINE,
        )
        self.input_notes.SetMinSize(self.FromDIP((250, 90)))
        s_main.Add(self.input_notes, proportion=3, flag=wx.EXPAND)

        s_main.Add((0, 0), 1, wx.EXPAND)
        s_buttons.Add(self.b_save, 1, wx.ALL | wx.ALIGN_BOTTOM)
        s_buttons.Add(self.b_close, 1, wx.ALL | wx.ALIGN_BOTTOM)
        s_main.Add(s_buttons, 3, wx.EXPAND | wx.ALIGN_RIGHT)
        self.SetSizer(s_main)
        self.Layout()

    def _on_gd3_save(self, _event: wx.Event) -> None:
        new_tag = GD3Tag(
            self.input_track_name_en.Value,
            self.input_track_name_native.Value,
            self.input_game_name_en.Value,
            self.input_game_name_native.Value,
            self.input_system_name_en.Value,
            self.input_system_name_native.Value,
            self.input_track_author_en.Value,
            self.input_track_author_native.Value,
            self.input_release_date.Value,
            self.input_creator.Value,
            self.input_notes.Value,
        )
        wx.PostEvent(wx.GetApp(), TagUpdateEvent(new_tag))
        self.Close()
