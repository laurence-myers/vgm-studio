from ..dro_logging import get_logger
from ..dro_player import DROPlayer
import math
import wx


class WaveformPanel(wx.Panel):
    __LOG = get_logger("WaveformPanel")

    def __init__(self, parent: wx.Frame):
        super().__init__(parent)
        self.SetBackgroundStyle(wx.BG_STYLE_CUSTOM)
        self.Bind(wx.EVT_SIZE, self.on_size)
        self.Bind(wx.EVT_PAINT, self.on_paint)

        self.dro_player: DROPlayer = DROPlayer(channels=1, sound_on=False)
        #self.dro_player.chip_write_delay = 0  # TODO: should we include this?

        frame_width = parent.GetClientSize()[0]
        self.num_buckets: int = frame_width

        self.xy_data: list[(int, int)] = []

    def clear(self):
        self.xy_data = []
        self.Refresh()

    def on_size(self, event):
        event.Skip()
        self.Refresh()

    def on_paint(self, _event):
        WaveformPanel.__LOG.debug("Painting")
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
        dc.SetPen(wx.Pen(wx.Colour(0x22, 0xFF, 0x22), width // self.num_buckets + 1))
        for (x, y) in self.xy_data:
            x = math.floor((x / self.num_buckets) * width)
            # Draw from the bottom of the rect to the top, with a small gap at the top for aesthetics.
            dc.DrawLine(x, height, x, height - math.floor((y / max_value) * (height - 10)))

    def redraw(self, points: list[(int, int)]):
        self.xy_data = points
        self.Refresh()

    def stop(self):
        self.dro_player.stop()
