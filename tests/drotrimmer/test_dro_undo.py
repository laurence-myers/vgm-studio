from typing import Any
from unittest import TestCase
from src.drotrimmer.dro_undo import UndoController, undoable

con = UndoController()


def get_controller():
    return con


class UndoTestObject(object):
    def an_undo_action(self, _original_state: Any) -> None:
        print("Action undone")

    @undoable("meow", get_controller, an_undo_action)
    def an_action(self) -> None:
        print("Do an action")


class TestDroUndo(TestCase):
    def test_undo_and_redo(self):
        obj = UndoTestObject()

        self.assertEqual(len(con.buffer), 0)
        self.assertEqual(con.position, -1)
        obj.an_action()
        self.assertEqual(len(con.buffer), 1)
        self.assertEqual(con.position, 0)
        obj.an_action()
        self.assertEqual(len(con.buffer), 2)
        self.assertEqual(con.position, 1)
        con.undo()
        self.assertEqual(len(con.buffer), 2)
        self.assertEqual(con.position, 0)
        obj.an_action()
        self.assertEqual(len(con.buffer), 2)
        self.assertEqual(con.position, 1)
        obj.an_action()
        self.assertEqual(len(con.buffer), 3)
        self.assertEqual(con.position, 2)
        con.undo()
        self.assertEqual(len(con.buffer), 3)
        self.assertEqual(con.position, 1)
        con.undo()
        self.assertEqual(len(con.buffer), 3)
        self.assertEqual(con.position, 0)
        con.redo()
        self.assertEqual(len(con.buffer), 3)
        self.assertEqual(con.position, 1)
