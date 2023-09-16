#!/usr/bin/python
#
#    Use, distribution, and modification of the DRO Trimmer binaries, source code,
#    or documentation, is subject to the terms of the MIT license, as below.
#
#    Copyright (c) 2008 - 2023 Laurence Dougal Myers
#
#    Permission is hereby granted, free of charge, to any person obtaining a copy
#    of this software and associated documentation files (the "Software"), to deal
#    in the Software without restriction, including without limitation the rights
#    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
#    copies of the Software, and to permit persons to whom the Software is
#    furnished to do so, subject to the following conditions:
#
#    The above copyright notice and this permission notice shall be included in
#    all copies or substantial portions of the Software.
#
#    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
#    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
#    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
#    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
#    THE SOFTWARE.
import threading
from abc import ABC, abstractmethod


class UndoableCommand(ABC):
    def __init__(self, description: str) -> None:
        self.description = description

    @abstractmethod
    def apply(self) -> None:
        ...

    @abstractmethod
    def revert(self) -> None:
        ...


class UndoController(object):
    buffer: list[UndoableCommand]
    position: int
    _lock: threading.Lock

    def __init__(self) -> None:
        self.reset()

    def reset(self) -> None:
        self.buffer: list[UndoableCommand] = []
        self.position: int = -1
        self._lock: threading.Lock = threading.Lock()

    def is_buffer_empty(self) -> bool:
        return len(self.buffer) == 0

    def has_something_to_undo(self) -> bool:
        return not self.is_buffer_empty() and self.position != -1

    def has_something_to_redo(self) -> bool:
        return not self.is_buffer_empty() and self.position < len(self.buffer) - 1

    def execute(self, value: UndoableCommand) -> None:
        with self._lock:
            value.apply()
            # If we've already tried undoing, truncate the list
            if self.has_something_to_redo():
                del self.buffer[self.position + 1 :]
            self.buffer.append(value)
            self.position += 1

    def undo(self) -> str | None:
        """Perform an undo action, using the entry in the undo buffer
        pointed to from the current position.

        Returns a string if an undo was performed, described the action
        that was undone, otherwise returns None.

        If there have been no previous calls to "undo", the current
        position will be the last entry in the buffer.
        If buffer is emtpy, will do nothing.
        """
        with self._lock:
            if (
                self.has_something_to_undo()
            ):  # silently ignore calls if nothing to undo.
                memo = self.buffer[self.position]
                memo.revert()
                self.position -= 1
                return memo.description
        return None

    def redo(self) -> str | None:
        """
        Returns a string if an redo was performed, described the action
        that was redone, otherwise returns None.
        """
        with self._lock:
            if (
                self.has_something_to_redo()
            ):  # silently ignore calls if nothing to redo.
                command = self.buffer[self.position + 1]
                command.apply()
                self.position += 1
                return command.description
        return None
