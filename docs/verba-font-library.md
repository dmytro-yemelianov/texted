# Verba bitmap library in Texted

Texted reads the complete local Verba catalog and the project's saved edits. It does not automatically apply unselected generator candidates. The corpus and private drafts stay outside this repository.

## Sync

```sh
node scripts/import_verba_fonts.mjs --verba /path/to/verbacorpus-para-fonts
```

The default project is the current corpus snapshot under `~/Library/Application Support/VerbaGlyphEditor/Projects/<snapshot>`. Override with `--project PATH`. The default destination is `~/Library/Application Support/Texted/fonts`; override with `--output PATH`. `--fonts id,id` updates a subset without dropping other imported entries. Restart Texted after syncing: the catalog is validated once before the window event loop. `TEXTED_FONT_LIBRARY` selects a different directory for previews. `texted --list-fonts` prints bundled and local entries, coverage and disabled reasons.

The 2026-11-08 sync contains **2,266 catalog entries: 1,993 ready, 273 disabled**, plus Texted's five embedded faces. Native cells range up to 770×159 in this snapshot. Binary records total 36,390,164 bytes; the manifest is 9,545,844 bytes. The limit is 4,096 faces, a 16 MiB manifest, 2 MiB per binary face and 64 MiB total resident face data.

## Import and validation

The importer calls Verba's own `exportRecord` to reproduce its shared monospace frame and merge saved edits with source glyphs. It checks source checksums, snapshot identity and draft base identity. Every explicit Unicode scalar is eligible; duplicate scalars with conflicting masks are excluded and recorded in `ambiguousUnicode`. Fonts with no unambiguous Unicode glyphs remain visible with a disabled reason. Symbol fonts with explicit Unicode mappings remain usable even without Latin letters.

The common bitmap record is `u32le codepoint` followed by `height × ceil(width/8)` bytes, MSB-left, zero padding. Records are sorted and unique. Each face has space and an explicit visible U+FFFD marker. Texted validates cell bounds, record length, Unicode scalars, sort order, padding, marker ink, filename confinement and CRC32. Corruption disables the affected face and reports the reason. The manifest retains source/snapshot/draft revisions and hashes, license metadata, original dimensions and coverage derived from actual visible glyphs.

Settings store `verba:<stable-source-id>`, not a transient catalog index. If the local face disappears, its typography falls back to embedded VGA without rejecting all appearance settings. Local data is not bundled or uploaded by this change; license records remain attached to the local catalog.

## Rendering and picker performance

The renderer supports multi-byte bitmap rows and caches at most 4,096 decoded glyphs within an 8 MiB CPU budget. The existing GPU atlas retains its eight-page bound. Picker specimens use a separate bounded thumbnail cache: at most 20 font engines and 32 small masks per engine. Font choices are built once per search string and reused across mouse movement, navigation and drawing. Only visible rows produce specimens.

On Apple M5, a release-mode CPU profile with 2,271 total choices measured roughly 0.17 s for cold catalog validation plus initial choices (previously 0.54 s before a table-based CRC32), 7 ns per cached choices/geometry call, and 11.3 µs per warm picker drawing preparation. These are local CPU measurements, not frame latency or GPU execution times. Reproduce with:

```sh
cargo test --release -p texted-app profile_font_picker -- --ignored --nocapture
```

A two-second native `sample` of the running three-tab preview found the main thread waiting in AppKit's event loop; its physical footprint was about 193 MiB. This is an idle sample, not a stress or memory-leak test. Remaining costs include startup checksum validation of thousands of files, document-worker ownership per tab, and independent full-document word counting after revisions; counting is cancellable and never runs inside paint.
