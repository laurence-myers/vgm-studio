from unittest import TestCase
from src.drotrimmer.dro_undo import UndoController, UndoableCommand

con = UndoController()


class UndoTestCommand(UndoableCommand):
    def __init__(self):
        super().__init__("A test command")

    def apply(self) -> None:
        print("Do an action")

    def revert(self) -> None:
        print("Action undone")


class UndoTestObject(object):
    def an_action(self) -> None:
        con.execute(UndoTestCommand())


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
