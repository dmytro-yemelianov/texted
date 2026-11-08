# Building and delivering Texted

## Download

Public builds are delivered from [GitHub Releases](https://github.com/dmytro-yemelianov/texted/releases).
Each macOS release contains a universal application for Apple Silicon and Intel:

- `Texted-VERSION-macOS-universal.dmg`: open and drag Texted into Applications.
- `Texted-VERSION-macOS-universal.zip`: the same `Texted.app`, preserving executable permissions.
- `.json`: build provenance, version, source commit, toolchain, architectures and distribution metadata.
- `.sha256`: checksums for the three files above.

The build deployment target is macOS 11.0. The workflows execute native tests on macOS 15 on both architectures; older OS versions are not tested by CI. Package metadata records the current ad-hoc signature.

## v0.2.0 preview

[Texted v0.2.0 Preview](https://github.com/dmytro-yemelianov/texted/releases/tag/v0.2.0) integrates PRs #1–#6:

- Native Open/Save As, file drop/Open With handling, and Save/Discard/Cancel guards.
- Metal rendering with a bounded glyph atlas, shared viewport geometry and horizontal scrolling.
- Asynchronous buffer commands, bulk edits and clipboard transfer, with ordered save/close barriers and stale-result rejection.
- Bounded search jobs/checkpoints/coverage, streaming replacement and rope-native word selection.
- Windowless editing/input controllers and a small native bootstrap/router.

Reference measurements and their limits are in the [hardening acceptance report](performance-hardening-results.md). Text still uses Unicode scalar columns; complex-script/grapheme shaping and IME composition remain future work. Exact search counts scan the document in the background, and file loading builds a complete rope. This version is a preview release.

## CI/CD

[Build and deliver](https://github.com/dmytro-yemelianov/texted/actions/workflows/build.yml) runs for pull requests, pushes to `main`, version tags and manual dispatches.

1. Apple Silicon and Intel runners check formatting, deny Clippy warnings, run Rust and packaging tests, smoke-test GUI/terminal entry points and cell-grid JSON, and exercise editing, save/cancel, resize and clean terminal restoration through a real PTY.
2. The packaging job builds both macOS targets with Rust 1.88.0 and `Cargo.lock`, assembles a universal GUI and terminal executables with `lipo`, creates the app icon and app bundle, and produces a ZIP and compressed DMG.
3. Packaging validates bundle metadata, both architectures, icon presence, the local signature, native startup/version, system-library load commands and ZIP round-trip integrity. The DMG is verified and delivery checksums are generated.
4. Workflow artifacts retain the delivery files for 30 days. A tag matching the Cargo version also publishes a GitHub Release with those files. Early `0.x` versions are marked as prereleases.

Actions are pinned to commit SHAs; Dependabot proposes monthly action updates. Only the release job has repository-write permission. Packaging uses no stored credentials.

## Local build

On a Mac with Rust and Xcode command-line tools:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/package_macos.py
(cd dist && shasum -a 256 -c ./*.sha256)
```

Outputs go to ignored `dist/`. The packager refuses to overwrite existing delivery files; pass `--output /path/to/new-directory` for a fresh run. The manifest identifies locally modified source trees. The icon source is [assets/icon.png](../assets/icon.png); the original generation prompt is [assets/icon-prompt.txt](../assets/icon-prompt.txt). Packaging creates the `.icns` sizes from that PNG with macOS `sips` and `iconutil`.

## Cut a release

Update the workspace Cargo version, refresh the lockfile if necessary, commit all intended changes and push `main`. After CI passes:

```sh
# The tag must exactly match the Cargo version, including any prerelease suffix.
git tag v0.2.0
git push origin v0.2.0
```

The tagged source is tested and packaged again before publication. A mismatched tag fails before delivery. Rerunning the tag workflow updates the existing release assets.

## Terminal executable in development packages

Packages built from `feat/terminal-cell-grid` include `Contents/MacOS/texted-tui` as a universal executable, verified after ZIP restoration. The GUI executable also accepts `--terminal`. The published v0.2.0 assets predate this work. To run a locally installed development build:

```sh
/Applications/Texted.app/Contents/MacOS/texted-tui path/to/file.txt
```

Both modes share the application controller, cell scene, tab state, command panel and in-app dialogs. See [terminal delivery](terminal-cell-grid.md) for controls and the JSON format.

Development terminal builds now support `--render=auto|kitty|ansi`, including Texted bitmap fonts and 10% zoom through Kitty graphics. CI also runs the graphics PTY peer in both pixel-mouse and cell-mouse modes; see [graphics delivery](terminal-graphics.md). Published v0.2.0 assets remain unchanged.
