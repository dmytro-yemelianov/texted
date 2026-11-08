#!/usr/bin/env python3
"""Exercise the shared editor through a real PTY; no window or GPU is created."""
import argparse
import fcntl
import os
from pathlib import Path
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import pty


def smoke(binary):
    with tempfile.TemporaryDirectory(prefix="texted-tui-") as directory:
        root = Path(directory)
        original = root / "first.txt"
        original.write_text("Seed\n")
        saved = root / "другий.txt"
        master, slave = pty.openpty()
        before = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        env = dict(os.environ, TERM="xterm-256color", TEXTED_SETTINGS_PATH=str(root / "appearance.json"))
        wrapper = "import pathlib,subprocess,sys,termios; r=subprocess.run(sys.argv[2:]); pathlib.Path(sys.argv[1]).write_text(repr(termios.tcgetattr(0))); sys.exit(r.returncode)"
        restored = root / "restored-termios.txt"
        child = subprocess.Popen([sys.executable, "-c", wrapper, str(restored), str(binary), str(original)], stdin=slave, stdout=slave, stderr=slave,
                                 env=env, preexec_fn=controlling_terminal)
        output = bytearray()

        def drain(seconds=0.2):
            until = time.monotonic() + seconds
            while time.monotonic() < until:
                if select.select([master], [], [], min(0.02, max(0, until - time.monotonic())))[0]:
                    try:
                        output.extend(os.read(master, 1024 * 1024))
                    except OSError:
                        break

        def wait(predicate, label):
            until = time.monotonic() + 8
            while not predicate():
                drain(0.025)
                if child.poll() is not None or time.monotonic() > until:
                    raise AssertionError(f"{label}; exit={child.poll()}; tail={bytes(output[-1000:])!r}")

        def send(data):
            os.write(master, data)
            drain()

        def paste(text):
            send(b"\x1b[200~" + text.encode() + b"\x1b[201~")

        try:
            wait(lambda: b"]0;first.txt - Texted" in output, "initial file operation completed")
            drain()
            paste("Ґава їжак\n")
            send(b"\x1bOQ")  # F2: save existing path
            wait(lambda: original.read_text() == "Ґава їжак\nSeed\n", "existing file saved")
            send(b"\x1b[17~")  # F6: new tab
            paste("Український текст: Її Єє Ґґ")
            send(b"\x1b[21~")  # F10: dirty close
            wait(lambda: b"Save changes?" in output, "shared save confirmation")
            send(b"\x1b")  # Cancel close, keep the tab
            send(b"\x1bOQ")
            wait(lambda: b"Save file" in output, "shared Save As dialog")
            send(b"\x01")  # Ctrl+A in the shared path editor
            paste(str(saved))
            send(b"\r")
            wait(lambda: saved.exists(), "new file saved")
            assert saved.read_text() == "Український текст: Її Єє Ґґ"
            send(b"\x1b[21;2~")  # Shift+F10: shared context menu
            wait(lambda: b"Document details" in output, "shared context menu")
            send(b"\x1b")
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
            os.kill(child.pid, signal.SIGWINCH)
            drain()
            send(b"\x11")  # Ctrl+Q: close both clean tabs
            wait(lambda: child.poll() is not None, "quit completed")
            assert child.returncode == 0, bytes(output[-1000:])
            assert restored.read_text() == repr(before), "PTY termios was not restored"
            assert b"\x1b[?1049l" in output, "alternate screen was not restored"
            assert b"\x1b[?2004l" in output, "bracketed paste was not disabled"
            print("PTY OK: Ukrainian paste, tabs, Save/Save As, Cancel, context menu, resize, quit and termios restoration")
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
            os.close(master)
            os.close(slave)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/texted-tui"))
    smoke(parser.parse_args().binary.resolve())
