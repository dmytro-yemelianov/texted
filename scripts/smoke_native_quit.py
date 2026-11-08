#!/usr/bin/env python3
"""Check that a Texted window with nothing to save accepts the system quit request.

Logout, restart and `tell application ... to quit` all send the same Apple Event, which
AppKit turns into applicationShouldTerminate:. Replying Cancel aborts a logout even when
no document needs saving. This opens a real window, so it needs a logged-in macOS GUI
session with Metal and is not part of CI.
"""
import argparse
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import time

BUNDLE_ID = "dev.texted.smoke.native-quit"


def wait_until(predicate, seconds):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
    return False


def running():
    script = f'application id "{BUNDLE_ID}" is running'
    result = subprocess.run(["osascript", "-e", script], capture_output=True, text=True)
    return result.stdout.strip() == "true"


def smoke(binary):
    with tempfile.TemporaryDirectory(prefix="texted-quit-") as directory:
        root = Path(directory)
        app = root / "TextedQuitSmoke.app"
        executable = app / "Contents/MacOS/texted"
        executable.parent.mkdir(parents=True)
        shutil.copy2(binary, executable)
        with (app / "Contents/Info.plist").open("wb") as target:
            plistlib.dump({
                "CFBundleIdentifier": BUNDLE_ID,
                "CFBundleExecutable": "texted",
                "CFBundleName": "TextedQuitSmoke",
                "CFBundlePackageType": "APPL",
                "NSHighResolutionCapable": True,
            }, target)
        env = dict(os.environ, TEXTED_SETTINGS_PATH=str(root / "appearance.json"))
        child = subprocess.Popen([str(executable)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            if not wait_until(running, 20):
                raise AssertionError("Texted did not register with LaunchServices")
            time.sleep(1)  # Let the first frame and the native delegate settle.
            reply = subprocess.run(
                ["osascript", "-e", "with timeout of 15 seconds",
                 "-e", f'tell application id "{BUNDLE_ID}" to quit', "-e", "end timeout"],
                capture_output=True, text=True,
            )
            exit_code = child.wait(timeout=15)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
        if reply.returncode != 0:
            raise AssertionError(f"Quit request was refused: {reply.stderr.strip()}")
        if exit_code != 0:
            raise AssertionError(f"Texted exited with {exit_code}: {child.stderr.read().decode(errors='replace')}")
    print("native quit smoke passed")


def main():
    if sys.platform != "darwin":
        raise SystemExit("The native quit smoke test requires macOS")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/texted"))
    smoke(parser.parse_args().binary.resolve())


if __name__ == "__main__":
    main()
