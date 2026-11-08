"""Offline asset integrity/coverage checks; no optional font libraries required."""
import json
import struct
import unittest

import build_bitmap_fonts as fonts


class BitmapFontTests(unittest.TestCase):
    def test_pinned_sources_native_pixels_and_documented_additions(self):
        pins = json.loads((fonts.ASSETS / 'sources.json').read_text())
        manifest = json.loads((fonts.ASSETS / 'manifest.json').read_text())
        patches = json.loads((fonts.ASSETS / 'ukrainian-additions.json').read_text())
        for spec, record in zip(pins['fonts'], manifest['fonts']):
            with self.subTest(font=spec['id']):
                self.assertEqual(spec['id'], record['id'])
                self.assertEqual(fonts.digest((fonts.ASSETS / spec['file']).read_bytes()), spec['sha256'])
                blob = (fonts.ASSETS / record['file']).read_bytes()
                self.assertEqual(fonts.digest(blob), record['sha256'])
                stride = 4 + record['height']
                self.assertEqual(len(blob), record['glyphs'] * stride)
                pairs = [(struct.unpack('<I', blob[i:i+4])[0], blob[i+4:i+stride])
                         for i in range(0, len(blob), stride)]
                self.assertEqual([cp for cp, _ in pairs], sorted(set(cp for cp, _ in pairs)))
                glyphs = dict(pairs)
                for char in fonts.REQUIRED:
                    self.assertTrue(any(glyphs[ord(char)]), char)
                for cp, rows in pairs:
                    self.assertFalse(0xd800 <= cp <= 0xdfff or cp > 0x10ffff)
                    self.assertTrue(all(row & ((1 << (8 - record['width'])) - 1) == 0 for row in rows))
                additions = patches[spec['id']]
                self.assertEqual(record['added'], [edit['codepoint'] for edit in additions])
                for edit in additions:
                    cp = int(edit['codepoint'][2:], 16)
                    self.assertEqual(glyphs.pop(cp), bytes.fromhex(''.join(edit['rows'])))
                self.assertEqual(len(glyphs), record['source_glyphs'])
                self.assertEqual(fonts.digest(fonts.encode(glyphs, spec)), record['source_rows_sha256'])
                # JSON source strikes can also be compared directly without the OTB decoder.
                if spec['file'].endswith('.json'):
                    self.assertEqual(glyphs, fonts.read_source(spec))
                rebuilt, edits = fonts.adaptations(glyphs, spec)
                self.assertEqual(edits, additions)
                self.assertEqual(fonts.encode(rebuilt, spec), blob)


if __name__ == '__main__':
    unittest.main()
