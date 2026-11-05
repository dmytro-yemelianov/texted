# Building and delivering Texted

## Download

Public builds are delivered from [GitHub Releases](https://github.com/dmytro-yemelianov/texted/releases).
Each macOS release contains a universal application for Apple Silicon and Intel:

- `Texted-VERSION-macOS-universal.dmg`: open and drag Texted into Applications.
- `Texted-VERSION-macOS-universal.zip`: the same `Texted.app`, preserving executable permissions.
- `.json`: build provenance, version, source commit, toolchain, architectures and distribution metadata.
- `.sha256`: checksums for the three files above.

The build deployment target is macOS 11.0. The workflows execute native tests on macOS 15 on both architectures; older OS versions are not tested by CI. Package metadata records the current ad-hoc signature.

## CI/CD

[Build and deliver](https://github.com/dmytro-yemelianov/texted/actions/workflows/build.yml) runs for pull requests, pushes to `main`, version tags and manual dispatches.

1. Apple Silicon and Intel runners check formatting, deny Clippy warnings, run Rust and packaging tests, and smoke-test native `--version` and `--help` commands.
2. The packaging job builds both macOS targets with Rust 1.88.0 and `Cargo.lock`, assembles a universal executable with `lipo`, creates the app icon and app bundle, and produces a ZIP and compressed DMG.
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
git tag v0.1.0
git push origin v0.1.0
```

The tagged source is tested and packaged again before publication. A mismatched tag fails before delivery. Rerunning the tag workflow updates the existing release assets.
