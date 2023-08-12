from ..dro_data import DROSong
try:
    from ..dro_player import DROPlayer
except:
    DROPlayer = None
import wx
from wx.lib import plot as wxplot


class WaveformPanel(object):
    def __init__(self, frame: wx.Frame):
        # Create the canvas
        self.panel = wxplot.PlotCanvas(frame)
        self.panel.enableAxes = False
        self.panel.enableAxesValues = False
        self.panel.enableGrid = False
        self.panel.enableLegend = False
        self.panel.SetBackgroundColour(wx.Colour(0x11, 0x11, 0x88))

        frame_width = frame.GetSize()[0]
        self.num_buckets = frame_width

        self.xy_data = []

    def draw(self):
        line = wxplot.PolyLine(
            self.xy_data,
            colour=wx.Colour(0x66, 0xFF, 0x66),
            width=3,
        )
        graphics = wxplot.PlotGraphics([line])
        self.panel.Draw(graphics, yAxis=(-32768, 32767))

    def load_song(self, drosong: DROSong):
        dro_player = DROPlayer(channels=1, sound_on=False, waveform_on=True)
        dro_player.load_song(drosong)
        dro_player.waveform_renderer.callback = self.redraw
        dro_player.waveform_renderer.set_quantization(drosong.ms_length, self.num_buckets)
        dro_player.reset()
        dro_player.play()

    def redraw(self, points: list[(int, int)]):
        self.xy_data = points
        self.draw()
