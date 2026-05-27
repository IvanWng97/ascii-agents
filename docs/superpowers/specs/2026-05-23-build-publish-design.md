# Build & Publish Flow — Design Spec

**Date:** 2026-05-23
**Status:** Draft

## Goal

Ship `pixtuoid` as a pre-built binary for macOS and Linux via three
distribution channels: GitHub Releases, Homebrew tap, and a `curl|sh`
installer. Triggered by git tag push (`v*`).

## Non-goals

- **crates.io publish** — deferred until API surface stabilizes.
- **Windows** — the hook shim requires Unix sockets; not viable without a
  transport rework.
- **`.deb`/`.rpm` packages** — `curl|sh` covers Linux adequately for now.
- **Docker image** — not meaningful for a TUI tool.
- **macOS code signing / notarization** — can layer on later.

---

## 1. Artifact Matrix

Each release produces **4 tarballs**, each containing both binaries
(`pixtuoid` + `pixtuoid-hook`):

| Rust target                  | Runner / tooling              | Tarball name                                          |
|------------------------------|-------------------------------|-------------------------------------------------------|
| `aarch64-apple-darwin`       | `macos-14` (native ARM)       | `pixtuoid-v{ver}-aarch64-apple-darwin.tar.gz`     |
| `x86_64-apple-darwin`        | `macos-13` (native Intel)     | `pixtuoid-v{ver}-x86_64-apple-darwin.tar.gz`      |
| `x86_64-unknown-linux-gnu`   | `ubuntu-latest` (native)      | `pixtuoid-v{ver}-x86_64-unknown-linux-gnu.tar.gz` |
| `aarch64-unknown-linux-gnu`  | `ubuntu-latest` + `cross`     | `pixtuoid-v{ver}-aarch64-unknown-linux-gnu.tar.gz`|

A `sha256sums.txt` file is attached to every release alongside the tarballs.

### Tarball layout

```
pixtuoid-v0.2.0-aarch64-apple-darwin/
├── pixtuoid
├── pixtuoid-hook
└── LICENSE
```

Flat directory — no nested `bin/`. Both binaries must end up on the user's
`$PATH` for the tool to function (the hook shim is invoked by Claude Code
directly).

### Release profile

Already configured in workspace `Cargo.toml`:

```toml
[profile.release]
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

No changes needed.

---

## 2. GitHub Actions: `release.yml`

Triggered on tag push matching `v*`. Three sequential stages.

### 2.1 Build stage (matrix, 4 jobs in parallel)

```yaml
strategy:
  matrix:
    include:
      - target: aarch64-apple-darwin
        os: macos-14
      - target: x86_64-apple-darwin
        os: macos-13
      - target: x86_64-unknown-linux-gnu
        os: ubuntu-latest
      - target: aarch64-unknown-linux-gnu
        os: ubuntu-latest
        cross: true
```

Each job:

1. Checks out the repo.
2. Installs Rust stable via `dtolnay/rust-toolchain@stable`.
3. For the `cross: true` variant, installs
   [`cross-rs/cross`](https://github.com/cross-rs/cross) and builds with
   `cross build --release --target $TARGET`. All others use native
   `cargo build --release --target $TARGET`.
4. Packages both binaries + LICENSE into a tarball named per the matrix.
5. Uploads the tarball as a workflow artifact.

### 2.2 Release stage (depends on build)

1. Downloads all 4 tarball artifacts.
2. Generates `sha256sums.txt` (one line per tarball, `shasum -a 256`).
3. Runs [`git-cliff`](https://github.com/orhun/git-cliff) to produce a
   changelog from conventional commits since the last tag.
4. Creates a GitHub Release via `softprops/action-gh-release` with:
   - Tag name as the release title.
   - git-cliff output as the release body.
   - All 4 tarballs + `sha256sums.txt` as release assets.

### 2.3 Homebrew stage (depends on release)

1. Computes SHA256 for each tarball from `sha256sums.txt`.
2. Renders the Homebrew formula template with the new version + hashes.
3. Pushes the updated formula to `IvanWng97/homebrew-pixtuoid` via
   a repository dispatch or direct commit using a deploy key / PAT.

---

## 3. git-cliff Configuration

Add `cliff.toml` to repo root. Conventional-commit grouping:

```toml
[changelog]
header = ""
body = """
{% for group, commits in commits | group_by(attribute="group") %}
### {{ group | upper_first }}
{% for commit in commits %}
- {{ commit.message | split(pat="\n") | first }}\
{% endfor %}
{% endfor %}
"""
trim = true

[git]
conventional_commits = true
commit_parsers = [
    { message = "^feat",     group = "Features" },
    { message = "^fix",      group = "Bug Fixes" },
    { message = "^perf",     group = "Performance" },
    { message = "^refactor", group = "Refactoring" },
    { message = "^style",    group = "Styling" },
    { message = "^chore",    group = "Miscellaneous" },
    { message = "^doc",      group = "Documentation" },
    { message = "^test",     group = "Testing" },
]
```

---

## 4. Homebrew Tap

### Repository: `IvanWng97/homebrew-pixtuoid`

Single formula: `Formula/pixtuoid.rb`

```ruby
class Pixtuoid < Formula
  desc "Terminal pixel-art office for AI coding agents"
  homepage "https://github.com/IvanWng97/pixtuoid"
  version "VERSION"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/IvanWng97/pixtuoid/releases/download/vVERSION/pixtuoid-vVERSION-aarch64-apple-darwin.tar.gz"
      sha256 "SHA256"
    end
    on_intel do
      url "https://github.com/IvanWng97/pixtuoid/releases/download/vVERSION/pixtuoid-vVERSION-x86_64-apple-darwin.tar.gz"
      sha256 "SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/IvanWng97/pixtuoid/releases/download/vVERSION/pixtuoid-vVERSION-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "SHA256"
    end
    on_intel do
      url "https://github.com/IvanWng97/pixtuoid/releases/download/vVERSION/pixtuoid-vVERSION-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "SHA256"
    end
  end

  def install
    bin.install "pixtuoid"
    bin.install "pixtuoid-hook"
  end

  def caveats
    <<~EOS
      To start visualizing your Claude Code sessions:
        pixtuoid install-hooks
        pixtuoid run
    EOS
  end

  test do
    assert_match "pixtuoid", shell_output("#{bin}/pixtuoid --version")
  end
end
```

### Update mechanism

The release workflow's Homebrew stage replaces `VERSION` and `SHA256`
placeholders in a template and commits the result to the tap repo. This
avoids maintaining a separate templating tool — sed on the template is
sufficient.

### Authentication

The release workflow authenticates to the tap repo using a GitHub PAT
stored as a repository secret (`HOMEBREW_TAP_TOKEN`) with `repo` scope on
the `IvanWng97/homebrew-pixtuoid` repository. The PAT is used in a
`git push` or via the GitHub API to commit the updated formula.

### Promotion to homebrew-core

The formula is intentionally structured to match homebrew-core conventions
(no custom tap helpers, standard `on_macos`/`on_linux` blocks, proper
`test` block). When the project meets notability criteria, the formula can
be submitted as-is with minor adjustments (removing the explicit `version`
line in favor of tag-inferred versioning).

---

## 5. Cargo.toml Metadata

Fill in missing fields across all three crates:

```toml
# Workspace Cargo.toml [package] section
authors = ["Ivan Wang <ivanwng97@icloud.com>"]
description = "Terminal pixel-art office for AI coding agents"
homepage = "https://github.com/IvanWng97/pixtuoid"
keywords = ["terminal", "tui", "pixel-art", "ai-agents", "claude"]
categories = ["command-line-utilities", "visualization"]
```

Per-crate descriptions:

- **pixtuoid-core**: `"Headless engine for pixtuoid — state, sprites, layout"`
- **pixtuoid**: `"Terminal pixel-art office for AI coding agents"`
- **pixtuoid-hook**: `"Lightweight hook shim for pixtuoid"`

---

## 6. Version Bumping

Manual. The release flow is:

1. Update `version` in workspace `Cargo.toml`.
2. Commit: `chore: bump version to x.y.z`.
3. Tag: `git tag vx.y.z`.
4. Push: `git push && git push --tags`.
5. CI takes over from here.

No automated version-bump tooling (cargo-release, release-plz) in v1 —
adds complexity for marginal benefit at this stage.

---

## 7. Existing CI (`ci.yml`) Changes

None. The existing CI workflow (fmt + clippy + test on ubuntu-latest)
continues to run on every push/PR. The release workflow is additive and
only triggers on tags.

---

## 8. Files to Create / Modify

| File                              | Action  | Purpose                          |
|-----------------------------------|---------|----------------------------------|
| `.github/workflows/release.yml`  | Create  | Release workflow                 |
| `cliff.toml`                     | Create  | git-cliff changelog config       |
| `Cargo.toml`                     | Modify  | Add metadata fields              |
| `crates/*/Cargo.toml`            | Modify  | Add per-crate descriptions       |

External (separate repo):

| File                              | Action  | Purpose                          |
|-----------------------------------|---------|----------------------------------|
| `IvanWng97/homebrew-pixtuoid` | Create  | New repo with formula template   |
