import unittest
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from nr import process
from tests.support import completed


class ProcessTests(unittest.TestCase):
    @patch.object(process.subprocess, "run")
    def test_run(self, mocked_run) -> None:
        mocked_run.return_value = completed()
        output = StringIO()
        with redirect_stdout(output):
            process.run(
                ["git", "status"],
                cwd=Path("/tmp"),
                check=False,
            capture_output=True,
            input="",
        )
        self.assertIn("-> git status", output.getvalue())
        mocked_run.assert_called_once_with(
            ["git", "status"],
            cwd=Path("/tmp"),
            check=False,
            capture_output=True,
            input="",
            text=True,
        )

    @patch.object(process.subprocess, "run")
    def test_run_can_be_quiet(self, mocked_run) -> None:
        mocked_run.return_value = completed()
        output = StringIO()
        with redirect_stdout(output):
            process.run(["git", "status"], announce=False)
        self.assertEqual(output.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
