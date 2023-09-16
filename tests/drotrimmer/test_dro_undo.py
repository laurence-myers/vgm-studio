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
        self.assertEqual(con.has_something_to_undo(), False)
        self.assertEqual(con.has_something_to_redo(), False)

        obj.an_action()
        self.assertEqual(len(con.buffer), 1)
        self.assertEqual(con.position, 0)
        self.assertEqual(con.has_something_to_undo(), True)
        self.assertEqual(con.has_something_to_redo(), False)

        obj.an_action()
        self.assertEqual(len(con.buffer), 2)
        self.assertEqual(con.position, 1)
        self.assertEqual(con.has_something_to_undo(), True)
        self.assertEqual(con.has_something_to_redo(), False)

        con.undo()
        self.assertEqual(len(con.buffer), 2)
        self.assertEqual(con.position, 0)
        self.assertEqual(con.has_something_to_undo(), True)
        self.assertEqual(con.has_something_to_redo(), True)

        obj.an_action()
        self.assertEqual(len(con.buffer), 2)
        self.assertEqual(con.position, 1)
        self.assertEqual(con.has_something_to_undo(), True)
        self.assertEqual(con.has_something_to_redo(), False)

        obj.an_action()
        self.assertEqual(len(con.buffer), 3)
        self.assertEqual(con.position, 2)
        self.assertEqual(con.has_something_to_undo(), True)
        self.assertEqual(con.has_something_to_redo(), False)

        con.undo()
        self.assertEqual(len(con.buffer), 3)
        self.assertEqual(con.position, 1)
        self.assertEqual(con.has_something_to_undo(), True)
        self.assertEqual(con.has_something_to_redo(), True)

        con.undo()
        self.assertEqual(len(con.buffer), 3)
        self.assertEqual(con.position, 0)
        self.assertEqual(con.has_something_to_undo(), True)
        self.assertEqual(con.has_something_to_redo(), True)

        con.redo()
        self.assertEqual(len(con.buffer), 3)
        self.assertEqual(con.position, 1)
        self.assertEqual(con.has_something_to_undo(), True)
        self.assertEqual(con.has_something_to_redo(), True)
