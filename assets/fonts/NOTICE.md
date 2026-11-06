# Bundled bitmap font attribution

Texted bundles native bitmap masks, their unchanged sources and reproducible
Ukrainian additions. Application code and font assets retain their separate licenses.

## IBM BIOS, EGA and VGA Unicode Plus strikes

The Ultimate Oldschool PC Font Pack v2.2 by VileR (2020).
Source and attribution: <https://int10h.org/oldschool-pc-fonts/>.
The original IBM designs and the pack's Unicode Plus extensions are credited
as described by the author at <https://int10h.org/oldschool-pc-fonts/readme/>.

`sources/BmPlus_IBM_*.otb` are unchanged files from the official v2.2 archive.
`bios_8x8.bin`, `ega_8x14.bin` and `vga_8x16.bin`, including Texted's added
punctuation/currency/missing-character masks, are distributed under
**Creative Commons Attribution-ShareAlike 4.0 International**.
The complete license is in `licenses/Oldschool-PC-Font-Pack.txt`.
Every existing glyph is preserved. Changes are format conversion and the
additions enumerated in `ukrainian-additions.json`; no endorsement is implied.

## Spleen 6×12 and 8×16

Copyright (c) 2018–2026, Frederic Cambus.
Source: <https://github.com/fcambus/spleen>.
License: **BSD 2-Clause**, reproduced in `licenses/Spleen-BSD-2-Clause.txt`.

The unchanged JSON source strikes are from the local Nasinnia catalog at
commit `fdd545febf4a23048b4d5927c1f70fd649c845c8`. Their metadata records
the upstream BDF hashes. Texted adds Є/є, Ukrainian punctuation and currency
where absent, using only each strike's native pixels. All modifications are
listed in `ukrainian-additions.json`; existing glyphs are preserved.
The derived `spleen_6x12.bin` and `spleen_8x16.bin` retain the BSD 2-Clause license.

## Reproduction

`sources.json` pins source SHA-256 hashes; `manifest.json` pins generated masks
and original glyph rows. From the Texted source checkout, use
`python3 scripts/build_bitmap_fonts.py --check` with `fonttools==4.60.2`.
Neither Texted nor the ordinary integrity tests need fontTools or system fonts.
