from abc import ABC, abstractmethod
import concurrent.futures
from .. import dro_analysis, dro_data, dro_logging
from ..dro_player import DROPlayer, WaveformRenderer
import queue
import threading
import time
import typing
import wx
import wx.lib.newevent

_type_EVT_TASK_RESULT = wx.NewEventType()
EVT_TASK_RESULT = wx.PyEventBinder(_type_EVT_TASK_RESULT)
_type_EVT_TASK_COMPLETED = wx.NewEventType()
EVT_TASK_COMPLETED = wx.PyEventBinder(_type_EVT_TASK_COMPLETED)


class IncrementalTask(ABC):
    """Inherit from this, and implement your processing code as a generator in get_generator().
    Listen to the `EVT_TASK_RESULT` and/or `EVT_TASK_COMPLETED` events. You must then dispatch the event
    based on its name.
    """

    def __init__(self, name: str) -> None:
        self.name: str = name
        self.stop_requested = threading.Event()

        self.log = dro_logging.get_logger(f"IncrementalTask[{self.name}]")

    def run(self) -> None:
        try:
            self.log.debug("Task started")
            generator = self._generate_results()
            self.log.debug("Generator got")
            was_stop_requested = False
            if not self.stop_requested.is_set():  # Don't start if we've already been asked to stop
                self.log.debug("Starting iteration")
                for value in generator:
                    # self.log.debug(f"Value: {value}")
                    # self.log.debug("In iter, checking if stop requested")
                    if self.stop_requested.is_set():
                        self.log.debug("Stopping task")
                        # Don't use the semaphore, in case it was set _after_ we got the last value
                        was_stop_requested = True
                        break
                    # self.log.debug("Dispatching result event")
                    wx.PostEvent(wx.GetApp(), TaskResultEvent(self.name, value))
            self.log.debug("Dispatching completed event")
            wx.PostEvent(wx.GetApp(), TaskCompletedEvent(self.name, was_stop_requested))
        except Exception as e:
            print(e)
            raise e

    @abstractmethod
    def _generate_results(self) -> typing.Iterator[typing.Any]:
        pass

    def request_stop(self) -> None:
        self.stop_requested.set()  # Set the cancellation event


class ExampleTask(IncrementalTask):  # TODO: remove this class
    def _generate_results(self) -> typing.Iterator[int]:
        for i in range(10):
            yield i
            wx.MilliSleep(200)


class DetailedRegisterAnalysisTask(IncrementalTask):
    def __init__(self, drosong: dro_data.DROSong):
        super().__init__("DetailedRegisterAnalysisTask")
        self.drosong = drosong

    def _generate_results(self) -> typing.Iterator[list[tuple[int, str]]]:
        detailed_register_descriptions: list[tuple[int, str]] = []
        detailed_register_analyzer = dro_analysis.DRODetailedRegisterAnalyzer()
        for entry in detailed_register_analyzer.analyze_dro(self.drosong):
            if self.stop_requested.is_set():
                return
            detailed_register_descriptions.append(entry)
        # Turns out, it's faster to just calculate everything and return it at once, than to do it per instruction.
        # (Maybe we could chunk it up, either by number of instructions, or every 100 ms)
        yield detailed_register_descriptions


class WaveformRenderTask(IncrementalTask):
    def __init__(self, dro_player: DROPlayer, drosong: dro_data.DROSong, num_buckets: int):
        super().__init__("WaveformRenderTask")
        self.dro_player = dro_player
        self.drosong = drosong
        self.num_buckets = num_buckets
        self.queue: queue.SimpleQueue = queue.SimpleQueue()
        self.log = dro_logging.get_logger("WaveformRenderTask")

        self.dro_player.stop()

    def _generate_results(self) -> typing.Iterator[list[tuple[int, int]]]:
        # self.log.debug("Starting generate results")
        xy_data = []
        self.dro_player.waveform_renderer = WaveformRenderer(self.queue, self.drosong.ms_length, self.num_buckets)
        self.dro_player.load_song(self.drosong)
        self.dro_player.play()
        # self.log.debug("Starting generate results loop")
        last_update = time.time()
        update_period = 0.1
        while (
                not self.stop_requested.is_set()
                and self.dro_player.is_playing  # TODO: work out why the player is stopped but the task isn't
                and len(xy_data) < self.num_buckets
        ):
            try:
                # self.log.debug("Getting from queue")
                xy_data = self.queue.get(timeout=2)
                # self.log.debug("Got data")
                now = time.time()
                if now - last_update >= update_period:
                    yield xy_data
                    last_update = now
            except queue.Empty:
                self.log.debug("No data, looping")
        # self.log.debug("Finishing up")
        yield xy_data

    def request_stop(self) -> None:
        self.log.debug("Requesting stop")
        super().request_stop()
        self.log.debug("Stopping DRO player")
        self.dro_player.stop()  # TODO: fix this stopping the shared player. One player per task?


class TaskResultEvent(wx.PyEvent):
    def __init__(self, task_name: str, value: int) -> None:
        super().__init__(eventType=_type_EVT_TASK_RESULT)
        self.task_name = task_name
        self.value = value


class TaskCompletedEvent(wx.PyEvent):
    def __init__(self, task_name: str, was_stop_requested: bool) -> None:
        super().__init__(eventType=_type_EVT_TASK_COMPLETED)
        self.task_name = task_name
        self.was_stop_requested = was_stop_requested


class TaskMaster:
    task_futures: dict[str, (concurrent.futures.Future, IncrementalTask)]

    def __init__(self) -> None:
        self.executor = concurrent.futures.ThreadPoolExecutor(max_workers=2)  # TODO: catch and log/display errors
        self.scheduled_tasks: dict[str, tuple[threading.Timer, IncrementalTask]] = {}  # Store for debouncing
        self.task_futures: dict[str, tuple[concurrent.futures.Future, IncrementalTask]] = {}  # Store for cancellation

    def _cancel_all_tasks(self) -> None:
        for _, (timer, task) in self.scheduled_tasks.items():
            timer.cancel()
            task.request_stop()
        for _, (timer, task) in self.task_futures.items():
            timer.cancel()
            task.request_stop()

    def cancel_task(self, task_name: str) -> bool:
        was_cancelled = False
        if task_name in self.scheduled_tasks:
            timer, task = self.scheduled_tasks[task_name]
            timer.cancel()
            task.request_stop()  # Shouldn't have started, but just in case...
            self.__remove_scheduled_task(task_name)
            was_cancelled = True
        if task_name in self.task_futures:
            future, task = self.task_futures[task_name]
            future.cancel()  # If the task is pending, cancel it
            task.request_stop()  # Nicely ask the task to stop, in case it's running
            self.remove_completed_task(task_name)
            was_cancelled = True
        return was_cancelled

    def get_num_tasks(self) -> int:
        return len(self.task_futures)

    def remove_completed_task(self, task_name: str) -> None:
        if task_name in self.task_futures:
            del self.task_futures[task_name]

    def __remove_scheduled_task(self, task_name: str) -> None:
        if task_name in self.scheduled_tasks:
            del self.scheduled_tasks[task_name]

    def __start_scheduled_task(self, task: IncrementalTask):
        self.__submit_task(task)
        self.__remove_scheduled_task(task.name)

    def start_task(self, task: IncrementalTask, debounce_sec: float | None = None) -> None:
        """Queues a task to be run. Cancels any existing running or pending instance of a task with the same name."""
        self.cancel_task(task.name)
        if debounce_sec:
            # Debounce by starting a timer. (We already cancelled any scheduled execution)
            timer = threading.Timer(debounce_sec, self.__start_scheduled_task, args=[task])
            self.scheduled_tasks[task.name] = (timer, task)
            timer.start()
        else:
            self.__submit_task(task)

    def stop(self) -> None:
        self._cancel_all_tasks()
        self.executor.shutdown(cancel_futures=True)

    def __submit_task(self, task: IncrementalTask) -> None:
        future = self.executor.submit(task.run)
        # future.add_done_callback(lambda f: print(f.result()))
        self.task_futures[task.name] = (future, task)
