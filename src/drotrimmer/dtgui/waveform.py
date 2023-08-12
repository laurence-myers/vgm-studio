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
        self.panel.SetBackgroundColour(wx.Colour(0xEE, 0xEE, 0xEE))

        # Do an initial draw
        self.draw(frame)

    def draw(self, frame: wx.Frame):
        # Generate some Data
        frame_width = frame.GetSize()[0]
        x_data = [x1 for x2 in range(frame_width) for x1 in (x2, x2)]
        y_data = [0, 65535, 0, 65535/2, 0, 12000, 0, 6000, 0, 20000] * (frame_width // 8) * 2

        # most items require data as a list of (x, y) pairs:
        #    [[1x, y1], [x2, y2], [x3, y3], ..., [xn, yn]]
        xy_data = list(zip(x_data, y_data))

        # Create your Poly object(s).
        # Use keyword args to set display properties.
        line = wxplot.PolyLine(
            xy_data,
            colour=wx.Colour(0x55, 0x55, 0xCC),
            width=3,
        )

        # create your graphics object
        graphics = wxplot.PlotGraphics([line])

        # draw the graphics object on the canvas
        self.panel.Draw(graphics)