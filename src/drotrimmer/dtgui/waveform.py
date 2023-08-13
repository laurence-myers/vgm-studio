from ..dro_data import DROSong
try:
    from ..dro_player import DROPlayer, WaveformRenderer
except:
    DROPlayer = None
    WaveformRenderer = None
import time
import wx
from wx.lib import plot as wxplot


class WaveformPanel(object):
    def __init__(self, frame: wx.Frame):
        # Create the canvas
        self.panel = wxplot.PlotCanvas(frame, size=frame.ToDIP(wx.Size(0, 200)))
        self.panel.enableAxes = False
        self.panel.enableAxesValues = False
        self.panel.enableGrid = False
        self.panel.enableLegend = False
        self.panel.SetBackgroundColour(wx.Colour(0x11, 0x22, 0x55))
        self.dro_player: DROPlayer = DROPlayer(channels=1, sound_on=False)
        #self.dro_player.chip_write_delay = 0  # TODO: should we include this?

        frame_width = frame.GetSize()[0]
        self.num_buckets: int = frame_width

        self.xy_data: list[(int, int)] = []
        self.last_drawn_at: float = time.time()
        self.draw_rate_secs: float = 0.100

    def draw(self):
        if self.panel:  # don't let DROPlayer to update waveform when we're shuttind down
            line = wxplot.PolyLine(
                self.xy_data,
                colour=wx.Colour(0x22, 0xFF, 0x22),
                width=3,
            )
            graphics = wxplot.PlotGraphics([line])
            self.panel.Draw(graphics, xAxis=(0, self.num_buckets))

    def load_song(self, drosong: DROSong):
        self.xy_data = []
        self.stop()
        self.dro_player.waveform_renderer = WaveformRenderer(self.redraw, drosong.ms_length, self.num_buckets)
        self.dro_player.load_song(drosong)
        self.dro_player.play()

    def redraw(self, points: list[(int, int)], bucket: int):
        self.xy_data = points
        # Re-draw every few seconds, or when
        # we've calculated all the points.
        now = time.time()
        if now - self.last_drawn_at >= self.draw_rate_secs \
                or bucket >= self.num_buckets:
            self.last_drawn_at = now
            self.draw()

    def stop(self):
        self.dro_player.stop()
