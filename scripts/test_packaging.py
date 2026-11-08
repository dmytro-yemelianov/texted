import plistlib
import unittest
import tempfile
import shutil
from pathlib import Path
import package_macos


class PackagingTests(unittest.TestCase):
    def test_packages_require_fonts_sources_and_attribution(self):
        with tempfile.TemporaryDirectory() as directory:
            resources = Path(directory)
            shutil.copytree(package_macos.ROOT / "assets/fonts", resources / "fonts")
            package_macos.verify_font_resources(resources)
            notice = resources / "fonts/NOTICE.md"
            notice.unlink()
            with self.assertRaisesRegex(ValueError, "NOTICE.md"):
                package_macos.verify_font_resources(resources)

    def test_tag_must_match_full_cargo_version(self):
        package_macos.validate_tag("0.1.0", "v0.1.0")
        package_macos.validate_tag("0.1.0-preview.1", "v0.1.0-preview.1")
        for tag in ("v0.2.0", "0.1.0", "v0.1.0-preview.1"):
            with self.assertRaises(ValueError):
                package_macos.validate_tag("0.1.0", tag)

    def test_bundle_versions_are_valid_for_prereleases(self):
        info = package_macos.bundle_metadata("0.1.0-preview.1")
        self.assertEqual(info["CFBundleShortVersionString"], "0.1.0")
        self.assertEqual(info["TextedVersion"], "0.1.0-preview.1")
        self.assertEqual(info["CFBundleExecutable"], "texted")
        self.assertEqual(info["CFBundleDocumentTypes"][0]["LSHandlerRank"], "Alternate")
        self.assertEqual(info["LSMinimumSystemVersion"], package_macos.MINIMUM_MACOS)
        self.assertEqual(plistlib.loads(plistlib.dumps(info)), info)
        with self.assertRaises(ValueError):
            package_macos.bundle_metadata("v0.1.0")


if __name__ == "__main__":
    unittest.main()
