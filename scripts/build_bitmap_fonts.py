#!/usr/bin/env python3
"""Rebuild audited native bitmap strikes and Ukrainian additions; no outline rendering.

Only the offline OTB decoder needs fonttools==4.60.2. Runtime and asset checks
need no font libraries. Source files, pins, native pixels and licenses are vendored.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / 'assets/fonts'
UKRAINIAN = 'АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯабвгґдеєжзиіїйклмнопрстуфхцчшщьюя'
REQUIRED = UKRAINIAN + "ʼ’'₴–—…"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read_source(spec):
    path = ASSETS / spec['file']
    raw = path.read_bytes()
    if digest(raw) != spec['sha256']:
        raise ValueError(f"Source checksum mismatch: {path.name}")
    if path.suffix == '.json':
        source = json.loads(raw)
        return {int(g['codepoint'][2:], 16): bytes.fromhex(''.join(g['rows']))
                for g in source['glyphData']}
    from fontTools.ttLib import TTFont
    font = TTFont(io.BytesIO(raw))
    strike = font['EBDT'].strikeData[0]
    subtables = font['EBLC'].strikes[0].indexSubTables
    out = {}
    for cp, name in font.getBestCmap().items():
        if cp < 32 or 127 <= cp <= 159:
            continue
        bitmap = strike[name]
        table = next(t for t in subtables if name in t.names)
        bitmap.decompile()
        metrics = bitmap.__dict__.get('metrics') or table.metrics
        x = getattr(metrics, 'horiBearingX', getattr(metrics, 'BearingX', 0))
        top = getattr(metrics, 'horiBearingY', getattr(metrics, 'BearingY', 0))
        y = spec['baseline'] - top
        if x < 0 or y < 0 or x + metrics.width > spec['width'] or y + metrics.height > spec['height']:
            raise ValueError(f"{spec['id']} U+{cp:04X} exceeds its native cell")
        rows = bytearray(spec['height'])
        for gy in range(metrics.height):
            packed = bitmap.getRow(gy, metrics=metrics)
            for gx in range(metrics.width):
                if packed[gx // 8] & (0x80 >> (gx % 8)):
                    rows[y + gy] |= 0x80 >> (x + gx)
        out[cp] = bytes(rows)
    return out


def mirror_ink(rows):
    columns = [x for x in range(8) if any(row & (0x80 >> x) for row in rows)]
    left, right = min(columns), max(columns)
    return bytes(sum(0x80 >> (left + right - x) for x in columns if row & (0x80 >> x))
                 for row in rows)


def adaptations(original, spec):
    glyphs = dict(original)
    edits = []

    def add(char, rows, basis, method):
        if ord(char) in glyphs:
            return
        glyphs[ord(char)] = bytes(rows)
        edits.append(dict(character=char, codepoint=f'U+{ord(char):04X}', basis=basis,
                          method=method, rows=[f'{v:02X}' for v in rows]))

    # Spleen's own reverse-E glyphs retain its stroke weight, baseline and bearings.
    for char, basis in [('Є', 'Э'), ('є', 'э')]:
        if ord(char) not in glyphs:
            add(char, mirror_ink(glyphs[ord(basis)]), basis, 'Mirror ink within original side bearings')
    for char in ['ʼ', '’']:
        add(char, glyphs[ord('’')] if ord('’') in glyphs else glyphs[ord("'")],
            '’' if ord('’') in glyphs else "'", 'Reuse this face’s apostrophe bitmap')
    for char, width in [('–', spec['width'] - 1), ('—', spec['width'])]:
        rows = bytes(((0xff << (8-width)) & 0xff) if row else 0 for row in glyphs[ord('-')])
        add(char, rows, '-', 'Extend the native hyphen horizontally, retaining its scanlines')
    dot = glyphs[ord('.')]
    dot_width = max(bin(row).count("1") for row in dot)
    start = (spec['width'] - (3*dot_width+2)) // 2
    if start < 0:
        raise ValueError('Cell cannot fit three native dots')
    rows = bytes(sum(0x80 >> (start+i*(dot_width+1)+x) for i in range(3) for x in range(dot_width)) if row else 0 for row in dot)
    add('…', rows, '.', 'Repeat three native dots at their original baseline')
    # Hryvnia follows the face's S, with two single-scanline horizontal currency bars.
    rows = bytearray(glyphs[ord('S')])
    occupied = [y for y, row in enumerate(rows) if row]
    # Extend through the side bearing so the two bars remain distinct from S/8.
    bar = sum(0x80 >> x for x in range(spec['width']))
    top, height = min(occupied), max(occupied) - min(occupied) + 1
    for y in [top + height // 3, top + 2 * height // 3]:
        rows[y] = bar
    add('₴', rows, 'S', 'Add two full-cell currency bars through the native S')
    # The visible missing-glyph cell is explicit; no system-font or '?' substitution.
    rows = bytearray(spec['height'])
    left, right = 1, spec['width'] - 2
    for y in range(1, spec['height'] - 1):
        rows[y] = ((0x80 >> left) | (0x80 >> right)) if y not in [1, spec['height'] - 2] else sum(0x80 >> x for x in range(left, right + 1))
    add('\ufffd', rows, 'native cell', 'Outlined missing-character box')
    return glyphs, edits


def encode(glyphs, spec):
    data = bytearray()
    for cp, rows in sorted(glyphs.items()):
        if len(rows) != spec['height'] or any(row & ((1 << (8 - spec['width'])) - 1) for row in rows):
            raise ValueError(f"Invalid bitmap rows for {spec['id']} U+{cp:04X}")
        data.extend(struct.pack('<I', cp))
        data.extend(rows)
    return bytes(data)


def build():
    pins = json.loads((ASSETS / 'sources.json').read_text())
    result, records, patches = {}, [], {}
    for spec in pins['fonts']:
        original = read_source(spec)
        glyphs, edits = adaptations(original, spec)
        missing = [c for c in REQUIRED if ord(c) not in glyphs or not any(glyphs[ord(c)])]
        if missing:
            raise ValueError(f"{spec['id']} missing {missing}")
        assert all(glyphs[cp] == rows for cp, rows in original.items())
        blob = encode(glyphs, spec)
        name = spec['id'] + '.bin'
        result[name] = blob
        records.append(dict(id=spec['id'], width=spec['width'], height=spec['height'],
                            baseline=spec['baseline'], license=spec['license'], glyphs=len(glyphs),
                            source_glyphs=len(original), source_rows_sha256=digest(encode(original, spec)),
                            file=name, sha256=digest(blob), added=[edit['codepoint'] for edit in edits]))
        patches[spec['id']] = edits
    result['manifest.json'] = (json.dumps(dict(format='u32-le codepoint + native MSB-left u8 scanlines', fonts=records), ensure_ascii=False, indent=2) + '\n').encode()
    result['ukrainian-additions.json'] = (json.dumps(patches, ensure_ascii=False, indent=2) + '\n').encode()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    for name, blob in build().items():
        path = ASSETS / name
        if args.check:
            if path.read_bytes() != blob:
                raise SystemExit(f'{name} differs from its pinned sources and documented additions')
        else:
            path.write_bytes(blob)
        print(f'{name}: {len(blob)} bytes {digest(blob)}')


if __name__ == '__main__':
    main()
