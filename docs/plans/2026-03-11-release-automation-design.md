# Release & Local Build Automation Design

**Goal:** Automate building and distributing Aura with two separate pipelines — local dev and GitHub Releases via `prod` branch.

**Architecture:** Single version source of truth in workspace Cargo.toml. Local pipeline for fast iteration. GitHub Actions on `prod` branch for public releases.

---

## Version Management

- Single source of truth: `version` field in workspace `Cargo.toml`
- `bundle.sh` reads version from Cargo.toml (no more hardcoded `VERSION="1.0.0"`)
- Info.plist `CFBundleVersion` / `CFBundleShortVersionString` derived from it
- `install.sh` also reads from Cargo.toml (currently hardcodes `0.2.0`)
- Manual bumps only — developer updates Cargo.toml before merging to `prod`

## Local Pipeline (`scripts/dev.sh`)

- Trigger: `bash scripts/dev.sh`
- Steps:
  1. `cargo build --release -p aura-daemon`
  2. `bash scripts/bundle.sh`
  3. Kill running Aura processes
  4. Copy `target/release/Aura.app` to `/Applications/`
  5. Relaunch `open /Applications/Aura.app`
- No version bump, no DMG, no git operations

## Release Pipeline (`.github/workflows/release.yml`)

- Trigger: push to `prod` branch
- Runner: `macos-latest` (GitHub-hosted, has Xcode + Rust available)
- Steps:
  1. Checkout code
  2. Install Rust toolchain (stable)
  3. `cargo fmt --all --check`
  4. `cargo clippy --workspace`
  5. `cargo test --workspace`
  6. `cargo build --release -p aura-daemon`
  7. Build SwiftUI app via `swiftc`
  8. Run `scripts/bundle.sh` to create Aura.app
  9. Create DMG (`hdiutil create`)
  10. Ad-hoc code sign (same as current approach)
  11. Read version from Cargo.toml
  12. Create GitHub Release `v{version}` with DMG attached
  13. Auto-generate release notes from commits since last release

## Not Included (YAGNI)

- No Apple Developer ID / notarization
- No Sparkle auto-updates
- No Homebrew cask
- No CHANGELOG file (GitHub Release notes suffice)
