import wx


class PlaybackPositionPanel(wx.Panel):
    """Shows the current playback (or editing) position, and length,
    of the currently opened song, in milliseconds and samples."""

    def __init__(self, parent: wx.Window) -> None:
        super().__init__(parent=parent)

        self.playback_length_ms: int = 0
        self.playback_length_samples: int = 0
        self.playback_position_ms: int = 0
        self.playback_position_samples: int = 0

        self._text_position_ms = wx.StaticText(
            self, style=wx.ALIGN_CENTER | wx.ST_NO_AUTORESIZE
        )
        self._text_position_ms.SetLabelText("0 / 0 ms")
        self._text_position_ms.SetMinSize(self.FromDIP(wx.Size(400, 20)))

        self._text_position_samples = wx.StaticText(
            self, style=wx.ALIGN_CENTER | wx.ST_NO_AUTORESIZE
        )
        self._text_position_samples.SetLabelText("0 / 0 samples")
        self._text_position_samples.SetMinSize(self.FromDIP(wx.Size(400, 20)))

        # Do layout
        info_panel_sizer = wx.BoxSizer(wx.HORIZONTAL)
        info_panel_sizer.Add(
            self._text_position_ms,
            wx.SizerFlags(1).Border(wx.ALL, 10),
        )
        info_panel_sizer.Add(
            wx.StaticLine(
                self,
                style=wx.LI_VERTICAL,
            ),
            wx.SizerFlags(0).Expand(),
        )
        info_panel_sizer.Add(
            self._text_position_samples,
            wx.SizerFlags(1).Border(wx.ALL, 10),
        )
        self.SetAutoLayout(1)
        self.SetSizer(info_panel_sizer)
        info_panel_sizer.Fit(self)
        info_panel_sizer.SetSizeHints(self)

    def set_playback_length(self, ms: int, samples: int) -> None:
        self.playback_length_ms = ms
        self.playback_length_samples = samples
        self._update_text()

    def set_playback_position(self, ms: int, samples: int) -> None:
        self.playback_position_ms = ms
        self.playback_position_samples = samples
        self._update_text()

    def _update_text(self) -> None:
        if self._text_position_ms:
            self._text_position_ms.SetLabelText(
                f"{self.playback_position_ms} / {self.playback_length_ms} ms"
            )
        if self._text_position_samples:
            self._text_position_samples.SetLabelText(
                f"{self.playback_position_samples} / {self.playback_length_samples} samples"
            )
