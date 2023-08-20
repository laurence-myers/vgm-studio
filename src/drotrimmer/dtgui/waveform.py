from ..dro_logging import get_logger
from ..dro_player import DROPlayer
import math
import wx


class WaveformPanel(wx.Panel):
    log = get_logger("WaveformPanel")

    def __init__(self, parent: wx.Window) -> None:
        super().__init__(parent)
        self.SetBackgroundStyle(wx.BG_STYLE_CUSTOM)
        self.Bind(wx.EVT_SIZE, self.on_size)
        self.Bind(wx.EVT_PAINT, self.on_paint)

        self.dro_player: DROPlayer = DROPlayer(channels=1, sound_on=False)
        #self.dro_player.chip_write_delay = 0  # TODO: should we include this?

        # Set a reasonable default resolution for the waveform.
        # (We could also calculate it from self.GetClientSize()[0], but there's complications.)
        self.x_resolution: int = 768

        self.xy_data: list[(int, int)] = []
        self.playback_start_indicator: int = 0

    def clear(self) -> None:
        self.xy_data = []
        self.playback_start_indicator = 0
        self.Refresh()

    def on_size(self, event) -> None:
        event.Skip()
        self.Refresh()

    def on_paint(self, _event) -> None:
        self.log.debug("Painting")
        width, height = self.GetClientSize()
        dc = wx.AutoBufferedPaintDC(self)
        dc.Clear()
        dc.SetBrush(wx.Brush(wx.Colour(0x11, 0x22, 0x55)))
        dc.DrawRectangle(0, 0, width, height)

        # No data? Don't draw.
        if len(self.xy_data) == 0:
            return

        # Automatically scale to the peak value
        max_value = max(self.xy_data, key=lambda xy: xy[1])[1] or 1
        # Set the pen width relative to the width on screen,
        # so that resizing the window doesn't create gaps between lines.
        pen_width = width // self.x_resolution + 1
        dc.SetPen(wx.Pen(wx.Colour(0x22, 0xFF, 0x22), pen_width))
        for (x, y) in self.xy_data:
            x = math.floor((x / self.x_resolution) * width)
            # Draw from the bottom of the rect to the top, with a small gap at the top for aesthetics.
            dc.DrawLine(x, height, x, height - math.floor((y / max_value) * (height - 10)))

        dc.SetPen(wx.Pen(wx.Colour(0xFF, 0xFF, 0xFF, 0xCC), pen_width))
        x = math.floor((self.playback_start_indicator / self.x_resolution) * width)
        dc.DrawLine(x, height, x, 0)

    def redraw(self, points: list[(int, int)]) -> None:
        self.xy_data = points
        self.Refresh()

    def set_playback_start_indicator(self, ms_offset: int, ms_length: int) -> None:
        frequency = 44100
        total_samples = ms_length * frequency // 1000
        samples_per_line = total_samples // self.x_resolution
        num_samples = ms_offset * frequency // 1000
        self.playback_start_indicator = num_samples // samples_per_line
        self.log.debug(f"Num samples: {num_samples}. New start pos: {self.playback_start_indicator}. xy_data len: {len(self.xy_data)}")
        self.Refresh()

    def stop(self) -> None:
        self.dro_player.stop()
