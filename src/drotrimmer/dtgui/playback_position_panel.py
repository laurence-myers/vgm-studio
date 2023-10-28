import math
from typing import Any

import wx

from ..dro_config import get_config


class PlaybackPositionPanel(wx.Panel):
    """Shows the current playback (or editing) position, and length,
    of the currently opened song, in milliseconds and samples."""

    def __init__(self, parent: wx.Window) -> None:
        super().__init__(parent=parent)

        self._use_rendering_sample_rate = True
        self.playback_length_ms: int = 0
        self.playback_length_samples: int = 0
        self.playback_position_ms: int = 0
        self.playback_position_samples: int = 0

        self._text_position_ms = wx.StaticText(
            self, style=wx.ALIGN_CENTER | wx.ST_NO_AUTORESIZE
        )
        self._text_position_ms.SetLabelText("0 / 0 ms")
        self._text_position_ms.SetMinSize(self.FromDIP(wx.Size(335, 20)))

        self._text_position_samples = wx.StaticText(
            self, style=wx.ALIGN_CENTER | wx.ST_NO_AUTORESIZE
        )
        self._text_position_samples.SetLabelText("0 / 0 samples")
        self._text_position_samples.SetMinSize(self.FromDIP(wx.Size(335, 20)))

        # Allow the user to choose displayed sample rate values.
        # 1. Always show the option for 44.1 khz
        # 2. Optionally append the rendering sample rate, if it's not 44.1 khz
        # 3. Sort the options
        # 4. Select the rendering sample rate by default
        sample_rate_str = "44.1 khz"
        sample_rate_choices = [sample_rate_str]
        self._rendering_sample_rate = get_config().audio.frequency
        if self._rendering_sample_rate != 44100:
            sample_rate_str = f"{(self._rendering_sample_rate / 1000):.1f} khz"
            sample_rate_choices.append(sample_rate_str)
        sample_rate_choices.sort()
        self._listbox_sample_rate = wx.ComboBox(
            self, choices=sample_rate_choices, style=wx.CB_DROPDOWN | wx.CB_READONLY
        )
        self._listbox_sample_rate.Select(
            self._listbox_sample_rate.FindString(sample_rate_str)
        )
        self._listbox_sample_rate.SetMinSize(self.FromDIP(wx.Size(130, 20)))

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
        info_panel_sizer.Add(
            wx.StaticLine(
                self,
                style=wx.LI_VERTICAL,
            ),
            wx.SizerFlags(0).Expand(),
        )
        info_panel_sizer.Add(
            self._listbox_sample_rate,
            wx.SizerFlags(1).Border(wx.ALL, 10),
        )
        self.SetAutoLayout(1)
        self.SetSizer(info_panel_sizer)
        info_panel_sizer.Fit(self)
        info_panel_sizer.SetSizeHints(self)

        # Bind events
        self.Bind(
            wx.EVT_COMBOBOX, self._on_choose_sample_rate, self._listbox_sample_rate
        )

    def _on_choose_sample_rate(self, _event: wx.CommandEvent) -> None:
        self._use_rendering_sample_rate = (
            self._listbox_sample_rate.GetValue() != "44.1 khz"
        )
        self._update_text()

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
            pos = self.playback_position_samples
            length = self.playback_length_samples
            if not self._use_rendering_sample_rate:
                pos = math.floor(pos / self._rendering_sample_rate * 44100 + 0.5)
                length = math.floor(length / self._rendering_sample_rate * 44100 + 0.5)
            self._text_position_samples.SetLabelText(f"{pos} / {length} samples")
