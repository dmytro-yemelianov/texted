"""Draw native-pixel review sheets from the shipped masks (Pillow required only here)."""
import json
from pathlib import Path
import struct

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / 'assets/fonts'
OUTPUT = ROOT / 'docs/font-review'
LINES = [
    'ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789',
    'АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯ',
    'abcdefghijklmnopqrstuvwxyz 0123456789',
    'абвгґдеєжзиіїйклмнопрстуфхцчшщьюя',
    'ГҐ гґ ІЇ ії ЕЄ еє ИЙ ий ЬЗ ьз',
    "ʼ ’ ' ₴ – — … ! ? , . : ; « »",
    'Ґава, їжак і єнот: Україна — 123 ₴.',
    'П’ять ґудзиків. Обʼєкт. Сім’я. Є/є.',
]


def main():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    for font in json.loads((ASSETS / 'manifest.json').read_text())['fonts']:
        w, h = font['width'], font['height']
        blob = (ASSETS / font['file']).read_bytes()
        stride = 4 + h
        glyphs = {struct.unpack('<I', blob[i:i+4])[0]: blob[i+4:i+stride]
                  for i in range(0, len(blob), stride)}
        added = {int(cp[2:], 16) for cp in font['added']}
        lines = [font['id'].upper()] + LINES
        image = Image.new('RGB', (16 + max(map(len, lines))*w, 12 + len(lines)*(h+5)), '#141820')
        for row, line in enumerate(lines):
            for col, char in enumerate(line):
                bitmap = glyphs.get(ord(char), glyphs[0xfffd])
                color = (101, 224, 219) if ord(char) in added else (236, 230, 211)
                for y, bits in enumerate(bitmap):
                    for x in range(w):
                        if bits & (0x80 >> x):
                            image.putpixel((8+col*w+x, 6+row*(h+5)+y), color)
        image.resize((image.width*4, image.height*4), Image.Resampling.NEAREST).save(OUTPUT / (font['id']+'.png'))


if __name__ == '__main__':
    main()
