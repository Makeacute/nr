import os
import pty
import threading
import unittest
from contextlib import redirect_stdout
from io import StringIO
from unittest.mock import patch

from nr import prompts


class PromptTests(unittest.TestCase):
    @patch("builtins.input", side_effect=["maybe", "yes"])
    def test_confirm_retries(self, _mocked_input) -> None:
        output = StringIO()
        with redirect_stdout(output):
            self.assertTrue(prompts.confirm("Continue?"))
        self.assertIn("Please answer yes or no.", output.getvalue())

    @patch("builtins.input", side_effect=["", ""])
    def test_confirm_defaults(self, _mocked_input) -> None:
        self.assertTrue(prompts.confirm("Continue?", default=True))
        self.assertFalse(prompts.confirm("Continue?", default=False))

    @patch.object(prompts.os, "open", side_effect=OSError)
    def test_commit_message_without_tty(self, _mocked_open) -> None:
        self.assertEqual(prompts.read_commit_message(), prompts.DEFAULT_MESSAGE)

    def test_commit_message_from_tty(self) -> None:
        master_fd, slave_fd = pty.openpty()
        input_fd = os.dup(slave_fd)
        sender = threading.Timer(0.02, os.write, args=(master_fd, b"helx\x7flo\r"))

        try:
            with patch.object(prompts.os, "open", return_value=input_fd):
                sender.start()
                self.assertEqual(prompts.read_commit_message(), "hello")
                sender.join()
        finally:
            os.close(master_fd)
            os.close(slave_fd)


if __name__ == "__main__":
    unittest.main()
