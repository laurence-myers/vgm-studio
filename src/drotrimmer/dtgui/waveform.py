from ..dro_logging import get_logger
from ..dro_player import DROPlayer
import math
import wx

_type_EVT_WAVEFORM_GO_TO = wx.NewEventType()
EVT_WAVEFORM_GO_TO = wx.PyEventBinder(_type_EVT_WAVEFORM_GO_TO)
_type_EVT_WAVEFORM_HOVER = wx.NewEventType()
EVT_WAVEFORM_HOVER = wx.PyEventBinder(_type_EVT_WAVEFORM_HOVER)


class WaveformGoToEvent(wx.PyEvent):
    def __init__(self, x_position_pct: float) -> None:
        super().__init__(eventType=_type_EVT_WAVEFORM_GO_TO)
        self.x_position_pct = x_position_pct


class WaveformHoverEvent(wx.PyEvent):
    def __init__(self, x_position_pct: float) -> None:
        super().__init__(eventType=_type_EVT_WAVEFORM_HOVER)
        self.x_position_pct = x_position_pct


class WaveformPanel(wx.Panel):
    log = get_logger("WaveformPanel")

    def __init__(self, parent: wx.Window) -> None:
        super().__init__(parent)
        self.SetBackgroundStyle(wx.BG_STYLE_CUSTOM)
        self.Bind(wx.EVT_SIZE, self.on_size)
        self.Bind(wx.EVT_PAINT, self.on_paint)
        # Mouse events
        self.Bind(wx.EVT_LEFT_DOWN, self.on_mouse_left_click)
        self.Bind(wx.EVT_MOTION, self.on_mouse_motion)

        self.dro_player: DROPlayer = DROPlayer(channels=1, sound_on=False)
        #self.dro_player.chip_write_delay = 0  # TODO: should we include this?

        # Set a reasonable default resolution for the waveform.
        # (We could also calculate it from self.GetClientSize()[0], but there's complications.)
        self.x_resolution: int = 768

        self.xy_data: list[(int, int)] = []
        self.playback_start_indicator: int = 0
        self.hover_indicator: int | None = None

    def __calculate_relative_position_from_ms(self, ms_offset: int, ms_length: int) -> int:
        frequency = self.dro_player.frequency
        total_samples: int = ms_length * frequency // 1000
        samples_per_line: float = total_samples / self.x_resolution
        num_samples: int = ms_offset * frequency // 1000
        return math.floor(num_samples / samples_per_line)

    def clear(self) -> None:
        self.xy_data = []
        self.playback_start_indicator = 0
        self.hover_indicator = None
        self.Refresh()

    def on_mouse_left_click(self, event: wx.MouseEvent):
        event.Skip()  # allow default processing, e.g. window focus
        pos = event.GetPosition()
        x_position_pct = pos[0] / self.GetClientSize()[0]
        self.log.debug(f"Clicked in waveform: {pos}, x_position_pct: {x_position_pct}")
        wx.PostEvent(wx.GetApp(), WaveformGoToEvent(x_position_pct))

    def on_mouse_motion(self, event: wx.MouseEvent):
        event.Skip()
        pos = event.GetPosition()
        x_position_pct = pos[0] / self.GetClientSize()[0]
        wx.PostEvent(wx.GetApp(), WaveformHoverEvent(x_position_pct))

    def on_size(self, event: wx.SizeEvent) -> None:
        event.Skip()
        self.Refresh()

    def on_paint(self, _event: wx.PaintEvent) -> None:
        # self.log.debug("Painting")
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

        # Hover, showing where the playback start indicator will snap to
        if self.hover_indicator is not None:
            dc.SetPen(wx.Pen(wx.Colour(0xAA, 0xCC, 0xCC), pen_width))
            x = math.floor((self.hover_indicator / self.x_resolution) * width)
            dc.DrawLine(x, height, x, 0)

        # Playback start indicator
        dc.SetPen(wx.Pen(wx.Colour(0xFF, 0xFF, 0xFF), pen_width))
        playback_start_x = math.floor((self.playback_start_indicator / self.x_resolution) * width)
        dc.DrawLine(playback_start_x, height, playback_start_x, 0)

        # Dim the stuff before the playback position
        # Need to use GraphicsContext to support alpha transparency
        gc = wx.GraphicsContext.Create(dc)
        if gc:
            gc.SetBrush(wx.Brush(wx.Colour(0x00, 0x00, 0x00, 0x7F)))
            gc.DrawRectangle(0, 0, playback_start_x, height)

    def redraw(self, points: list[(int, int)]) -> None:
        self.xy_data = points
        self.Refresh()

    def set_hover_indicator(self, ms_offset: int | None, ms_length: int) -> None:
        self.hover_indicator = None if ms_offset is None else self.__calculate_relative_position_from_ms(
            ms_offset,
            ms_length
        )
        self.Refresh()

    def set_playback_start_indicator(self, ms_offset: int, ms_length: int) -> None:
        self.playback_start_indicator = self.__calculate_relative_position_from_ms(
            ms_offset,
            ms_length
        )
        self.log.debug(f"New start pos: {self.playback_start_indicator}. xy_data len: {len(self.xy_data)}")
        self.Refresh()

    def stop(self) -> None:
        self.dro_player.stop()
