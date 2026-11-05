import plistlib
import unittest
import package_macos


class PackagingTests(unittest.TestCase):
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
