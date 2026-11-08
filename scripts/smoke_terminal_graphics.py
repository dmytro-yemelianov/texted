#!/usr/bin/env python3
"""Emulate a Kitty peer in a PTY and verify shared font/zoom/edit/save behavior."""
import argparse
import ast
import base64
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
import zlib


def smoke(binary, pixel_mouse=True):
    with tempfile.TemporaryDirectory(prefix="texted-kitty-") as folder:
        root = Path(folder)
        document = root / "source.txt"
        document.write_text("Ґава Її Єє Іі ґ\n")
        settings = root / "appearance.json"
        master, slave = pty.openpty()
        before = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 800, 480))
        def setup():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        restored = root / "restored.txt"
        wrapper = "import pathlib,subprocess,sys,termios; r=subprocess.run(sys.argv[2:]); pathlib.Path(sys.argv[1]).write_text(repr(termios.tcgetattr(0))); sys.exit(r.returncode)"
        child = subprocess.Popen([sys.executable, "-c", wrapper, str(restored), str(binary), "--render=kitty", str(document)],
                                 stdin=slave, stdout=slave, stderr=slave, preexec_fn=setup,
                                 env=dict(os.environ, TERM="xterm-kitty", TEXTED_SETTINGS_PATH=str(settings)))
        output = bytearray()
        parsed = bytearray()
        query_seen = False
        mouse_query_seen = False
        images = {}
        image_data = bytearray()
        image_params = None
        uploaded = 0
        deleted = 0
        def drain(seconds=0.2):
            nonlocal query_seen, mouse_query_seen, image_data, image_params, uploaded, deleted
            until = time.monotonic() + seconds
            while time.monotonic() < until:
                if not select.select([master], [], [], min(0.02, max(0, until-time.monotonic())))[0]:
                    continue
                data = os.read(master, 1024 * 1024)
                output.extend(data)
                if pixel_mouse and not mouse_query_seen and b"\x1b[?1016$p" in output:
                    mouse_query_seen = True
                    os.write(master,b"\x1b[?1016;2$y")
                parsed.extend(data)
                while True:
                    start = parsed.find(b"\x1b_G")
                    if start < 0:
                        if len(parsed) > 32768:
                            del parsed[:-2]
                        break
                    end = parsed.find(b"\x1b\\", start)
                    if end < 0:
                        break
                    sequence = bytes(parsed[start+3:end])
                    del parsed[:end+2]
                    control, _, payload = sequence.partition(b";")
                    p = dict(s.split(b"=", 1) for s in control.split(b",") if b"=" in s)
                    if p.get(b"a") == b"q":
                        query_seen = True
                        os.write(master, b"\x1b_Gi="+p[b"i"]+b";OK\x1b\\\x1b[6;16;8t")
                    elif p.get(b"a") == b"d":
                        images.pop(int(p[b"i"]), None)
                        deleted += 1
                    else:
                        if p.get(b"a") == b"T":
                            image_data = bytearray()
                            image_params = p
                        image_data.extend(payload)
                        assert len(payload) <= 4096
                        if p.get(b"m", b"0") == b"0" and image_params:
                            raw = zlib.decompress(base64.b64decode(image_data))
                            w, h = int(image_params[b"s"]), int(image_params[b"v"])
                            assert len(raw) == w*h*3
                            images[int(image_params[b"i"])] = raw
                            uploaded += 1
                            image_params = None
        def wait(predicate, description):
            until = time.monotonic()+8
            while not predicate():
                drain(0.025)
                if child.poll() is not None or time.monotonic() > until:
                    raise AssertionError(f"{description}; exit={child.poll()}, tail={bytes(output[-200:])!r}")
        def send(data):
            os.write(master, data)
            drain()
        try:
            wait(lambda: b"]0;source.txt - Texted" in output and uploaded > 0, "graphics startup")
            assert query_seen and images
            drain()
            count = uploaded
            send(b"\x1b[20~")  # F9: shared Appearance
            if pixel_mouse:
                wait(lambda: b"\x1b[?1016h" in output,"pixel mouse enabled")
                send(b"\x1b[<0;301;185M\x1b[<0;301;185m")
            else:
                send(b"\x1b[<0;38;12M\x1b[<0;38;12m")  # Cell mouse fallback at source (300,184)
            send(b"\x1b[H\x1b[B\r")  # EGA, selected from our bitmap catalog
            send(b"\x1bOQ")  # F2: Keep
            wait(lambda: settings.exists() and json.loads(settings.read_text())["typography"]["family"] == "ega_8x14", "bitmap font preference")
            assert uploaded > count
            send(b"\x1b[61;5u")  # Ctrl+= : Texted zoom, terminal cell dimensions unchanged
            wait(lambda: json.loads(settings.read_text())["typography"]["zoom_percent"] == 110, "independent 10% zoom")
            send("\x1b[200~Україна\n\x1b[201~".encode())
            send(b"\x1bOQ")
            wait(lambda: document.read_text().startswith("Україна\n"), "Unicode paste/save")
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 128, 1024, 560))
            wait(lambda: len(images) == 144, "graphics resize and old image reclamation")
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 8, 20, 160, 128))
            wait(lambda: not images, "small terminal suspends graphics without discarding text")
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 128, 1024, 560))
            wait(lambda: len(images) == 144, "graphics restored after resize")
            send(b"\x11")
            wait(lambda: child.poll() is not None, "graphics quit")
            drain()
            assert child.returncode == 0
            after = ast.literal_eval(restored.read_text())
            before[3] &= ~getattr(termios, "PENDIN", 0)
            after[3] &= ~getattr(termios, "PENDIN", 0)
            assert before == after
            assert not images, "Owned images were not reclaimed"
            assert deleted > 0
            assert b"\x1b[?2026l" in output and b"\x1b[?1049l" in output
            print(f"Kitty PTY OK: bitmap font, independent zoom, Ukrainian save, mouse, resize, bounded tiles, image cleanup and termios ({uploaded} tile uploads)")
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
            os.close(master)
            os.close(slave)


def rejection(binary):
    """Forced graphics must fail cleanly when the peer never confirms support."""
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 640, 384))
    before = termios.tcgetattr(slave)
    def setup():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    with tempfile.TemporaryDirectory(prefix="texted-no-graphics-") as folder:
        restored = Path(folder) / "termios.txt"
        wrapper = "import pathlib,subprocess,sys,termios; r=subprocess.run(sys.argv[2:]); pathlib.Path(sys.argv[1]).write_text(repr(termios.tcgetattr(0))); sys.exit(r.returncode)"
        child = subprocess.Popen([sys.executable,"-c",wrapper,str(restored),str(binary),"--render=kitty"],stdin=slave,stdout=slave,stderr=slave,preexec_fn=setup)
        try:
            deadline = time.monotonic()+3
            while child.poll() is None and time.monotonic()<deadline:
                if select.select([master],[],[],0.02)[0]:
                    os.read(master,65536)
            assert child.poll() is not None, "Unsupported graphics probe did not time out"
            assert child.returncode == 1
            after = ast.literal_eval(restored.read_text())
            before[3] &= ~getattr(termios,"PENDIN",0)
            after[3] &= ~getattr(termios,"PENDIN",0)
            assert before == after
            print("Kitty rejection OK: unsupported peer exits cleanly and restores termios")
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=3)
            os.close(master)
            os.close(slave)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/texted-tui"))
    binary = parser.parse_args().binary.resolve()
    smoke(binary)
    smoke(binary, pixel_mouse=False)
    rejection(binary)
