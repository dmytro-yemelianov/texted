#!/usr/bin/env python3
"""Build a universal Texted.app and verified ZIP/DMG delivery artifacts using macOS tools."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin")
MINIMUM_MACOS = "11.0"


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], cwd=ROOT, check=True, **kwargs)


def capture(*args):
    return run(*args, stdout=subprocess.PIPE, text=True).stdout.strip()


def project_version():
    metadata = json.loads(capture("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"))
    return next(p["version"] for p in metadata["packages"] if p["name"] == "texted-app")


def validate_tag(version, tag):
    if tag is not None and tag != "v" + version:
        raise ValueError(f"Tag {tag!r} must match Cargo version v{version}")


def bundle_metadata(version):
    match = re.fullmatch(r"(\d+\.\d+\.\d+)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version)
    if not match:
        raise ValueError(f"Invalid application version: {version}")
    with (ROOT / "packaging/macos/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    info.update(CFBundleShortVersionString=match[1], CFBundleVersion=match[1], TextedVersion=version)
    return info


def build_icon(destination, work):
    iconset = work / "Texted.iconset"
    iconset.mkdir()
    for size in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            name = f"icon_{size}x{size}" + ("@2x" if scale == 2 else "") + ".png"
            run("sips", "-z", size * scale, size * scale, ROOT / "assets/icon.png", "--out", iconset / name,
                stdout=subprocess.DEVNULL)
    run("iconutil", "-c", "icns", iconset, "-o", destination)


def verify_font_resources(resources):
    fonts = resources / "fonts"
    # Ship the complete reproducible source/attribution set, also after ZIP restoration.
    for source in (ROOT / "assets/fonts").rglob("*"):
        if source.is_file():
            relative = source.relative_to(ROOT / "assets/fonts")
            delivered = fonts / relative
            if not delivered.is_file() or delivered.read_bytes() != source.read_bytes():
                raise ValueError(f"Missing or changed bundled font resource: {relative}")


def verify_bundle(app):
    with (app / "Contents/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    binary = app / "Contents/MacOS" / info["CFBundleExecutable"]
    if not os.access(binary, os.X_OK):
        raise ValueError("Bundle executable is missing or not executable")
    if set(capture("lipo", "-archs", binary).split()) != {"arm64", "x86_64"}:
        raise ValueError("Expected both arm64 and x86_64 in the universal executable")
    if not (app / "Contents/Resources" / info["CFBundleIconFile"]).is_file():
        raise ValueError("Bundle icon is missing")
    verify_font_resources(app / "Contents/Resources")
    run("plutil", "-lint", app / "Contents/Info.plist")
    run("codesign", "--verify", "--deep", "--strict", app)
    expected = "Texted " + info["TextedVersion"]
    if capture(binary, "--version") != expected:
        raise ValueError("Packaged binary version does not match Info.plist")
    # Fail packaging if a local/Homebrew library accidentally enters the bundle's load commands.
    dependencies = capture("otool", "-L", binary)
    for line in dependencies.splitlines():
        if not line.startswith("\t"):
            continue
        library = line.strip().split(" (", 1)[0]
        if library.startswith("/") and not library.startswith(("/usr/lib/", "/System/Library/")):
            raise ValueError(f"Unexpected non-system library: {library}")


def package(output, tag=None):
    if sys.platform != "darwin":
        raise RuntimeError("macOS packaging requires macOS with Xcode command-line tools")
    version = project_version()
    validate_tag(version, tag)
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    stem = f"Texted-{version}-macOS-universal"
    paths = [output / f"{stem}{suffix}" for suffix in (".zip", ".dmg", ".json", ".sha256")]
    if any(path.exists() for path in paths):
        raise FileExistsError("Delivery files already exist; use a new output directory")
    env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET=MINIMUM_MACOS)
    for target in TARGETS:
        run("cargo", "build", "--locked", "--release", "--bin", "texted", "--target", target, env=env)
    with tempfile.TemporaryDirectory(prefix="texted-package-") as directory:
        work = Path(directory)
        staging = work / "delivery"
        app = staging / "Texted.app"
        resources = app / "Contents/Resources"
        executable = app / "Contents/MacOS/texted"
        resources.mkdir(parents=True)
        executable.parent.mkdir(parents=True)
        # Honor Cargo's target-directory configuration rather than guessing where binaries landed.
        metadata = json.loads(capture("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"))
        target_dir = Path(metadata["target_directory"])
        run("lipo", "-create", *(target_dir / target / "release/texted" for target in TARGETS), "-output", executable)
        executable.chmod(0o755)
        with (app / "Contents/Info.plist").open("wb") as target:
            plistlib.dump(bundle_metadata(version), target)
        build_icon(resources / "Texted.icns", work)
        manifest = {
            "name": "Texted", "version": version, "architectures": ["arm64", "x86_64"],
            "minimum_macos": MINIMUM_MACOS, "commit": capture("git", "rev-parse", "HEAD"),
            "working_tree_dirty": bool(capture("git", "status", "--porcelain")),
            "rustc": capture("rustc", "--version"), "signature": "ad-hoc",
        }
        manifest_text = json.dumps(manifest, indent=2) + "\n"
        (resources / "build-info.json").write_text(manifest_text)
        for name in ("LICENSE-MIT", "LICENSE-APACHE"):
            shutil.copy2(ROOT / name, resources / name)
        shutil.copytree(ROOT / "assets/fonts", resources / "fonts")
        # Give arm64 executables a valid local signature after lipo assembles the two slices.
        run("codesign", "--force", "--sign", "-", "--timestamp=none", app)
        verify_bundle(app)
        run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", app, paths[0])
        (staging / "Applications").symlink_to("/Applications", target_is_directory=True)
        run("hdiutil", "create", "-volname", "Texted " + version, "-srcfolder", staging,
            "-format", "UDZO", "-ov", paths[1])
        run("hdiutil", "verify", paths[1])
        # Check the ZIP actually delivers the bundle, permissions and icon that were verified above.
        restored = work / "restored"
        run("ditto", "-x", "-k", paths[0], restored)
        verify_bundle(restored / "Texted.app")
        paths[2].write_text(manifest_text)
    checksums = []
    for path in paths[:3]:
        digest = hashlib.sha256()
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
        checksums.append(f"{digest.hexdigest()}  {path.name}\n")
    paths[3].write_text("".join(checksums))
    print("\nDelivery files:")
    for path in paths:
        print(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    parser.add_argument("--version-tag", help="Require an exact v<Cargo version> release tag")
    args = parser.parse_args()
    package(args.output, args.version_tag)


if __name__ == "__main__":
    main()
