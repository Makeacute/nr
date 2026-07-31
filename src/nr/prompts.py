import codecs
import os
import select
import termios
import time
import tty

TIMEOUT_SECONDS = 60
DEFAULT_MESSAGE = "rebuild"
PROMPT = "[commit]: "


def confirm(prompt: str, *, default: bool = False) -> bool:
    suffix = " [Y/n] " if default else " [y/N] "

    while True:
        try:
            answer = input(prompt + suffix).strip().lower()
        except (EOFError, KeyboardInterrupt):
            print()
            return False

        if not answer:
            return default
        if answer in {"y", "yes"}:
            return True
        if answer in {"n", "no"}:
            return False

        print("Please answer yes or no.")


def choose(
    prompt: str,
    choices: list[tuple[str, str]],
    *,
    default: str | None = None,
) -> str | None:
    keys = {key for key, _label in choices}
    if default is not None and default not in keys:
        raise ValueError(f"invalid default choice: {default}")

    for key, label in choices:
        marker = " (default)" if key == default else ""
        print(f"  {key}: {label}{marker}")

    suffix = f" [{default}] " if default else " "
    while True:
        try:
            answer = input(prompt + suffix).strip()
        except (EOFError, KeyboardInterrupt):
            print()
            return None

        if not answer and default is not None:
            return default
        if answer in keys:
            return answer

        print(f"Please choose one of: {', '.join(sorted(keys))}.")


def read_line(prompt: str, *, default: str | None = None) -> str | None:
    suffix = f" [{default}] " if default is not None else " "
    try:
        answer = input(prompt + suffix)
    except (EOFError, KeyboardInterrupt):
        print()
        return None

    answer = answer.strip()
    if not answer and default is not None:
        return default
    return answer


def read_commit_message() -> str:
    try:
        terminal_fd = os.open("/dev/tty", os.O_RDWR)
    except OSError:
        return DEFAULT_MESSAGE

    old_settings = termios.tcgetattr(terminal_fd)
    buffer: list[str] = []
    decoder = codecs.getincrementaldecoder("utf-8")()
    deadline = time.monotonic() + TIMEOUT_SECONDS

    os.write(terminal_fd, PROMPT.encode())

    try:
        tty.setraw(terminal_fd)

        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break

            readable, _, _ = select.select([terminal_fd], [], [], remaining)
            if not readable:
                break

            byte = os.read(terminal_fd, 1)
            if byte in (b"\r", b"\n"):
                decoder.reset()
                break
            if byte == b"\x03":
                raise KeyboardInterrupt
            if byte in (b"\x7f", b"\x08"):
                decoder.reset()
                if buffer:
                    buffer.pop()
                    os.write(terminal_fd, b"\b \b")
                continue
            if byte < b"\x20" or byte == b"\x7f":
                decoder.reset()
                continue

            try:
                character = decoder.decode(byte)
            except UnicodeDecodeError:
                decoder.reset()
                continue

            if character and character.isprintable():
                buffer.append(character)
                os.write(terminal_fd, character.encode())
    finally:
        termios.tcsetattr(terminal_fd, termios.TCSADRAIN, old_settings)
        os.write(terminal_fd, b"\n")
        os.close(terminal_fd)

    return "".join(buffer).strip() or DEFAULT_MESSAGE
