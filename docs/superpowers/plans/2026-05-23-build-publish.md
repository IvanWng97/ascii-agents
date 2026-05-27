# Build & Publish Flow — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship pre-built binaries for macOS + Linux via GitHub Releases, Homebrew tap, and curl|sh installer — all triggered by `v*` tag push.

**Architecture:** A single `release.yml` workflow with three sequential jobs (build matrix → release → homebrew). Standalone `install.sh` and `cliff.toml` at repo root. Cargo metadata filled in for crates.io readiness.

**Tech Stack:** GitHub Actions, cross-rs/cross (Linux ARM), git-cliff (changelog), softprops/action-gh-release, POSIX sh (installer)

---

### Task 1: Add LICENSE file

The tarball layout requires a LICENSE file. The workspace `Cargo.toml` declares `license = "MIT"` but no LICENSE file exists.

**Files:**
- Create: `LICENSE`

- [ ] **Step 1: Create MIT LICENSE**

```
MIT License

Copyright (c) 2026 Ivan Wang

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- [ ] **Step 2: Commit**

```bash
git add LICENSE
git commit -m "chore: add MIT license file"
```

---

### Task 2: Fill Cargo.toml metadata

Add missing fields (authors, description, homepage, keywords, categories) to workspace and per-crate Cargo.toml files.

**Files:**
- Modify: `Cargo.toml` (workspace)
- Modify: `crates/pixtuoid-core/Cargo.toml`
- Modify: `crates/pixtuoid/Cargo.toml`
- Modify: `crates/pixtuoid-hook/Cargo.toml`

- [ ] **Step 1: Add metadata to workspace Cargo.toml**

Add these fields to `[workspace.package]`:

```toml
authors      = ["Ivan Wang <ivanwng97@icloud.com>"]
description  = "Terminal pixel-art office for AI coding agents"
homepage     = "https://github.com/IvanWng97/pixtuoid"
keywords     = ["terminal", "tui", "pixel-art", "ai-agents", "claude"]
categories   = ["command-line-utilities", "visualization"]
```

- [ ] **Step 2: Add per-crate descriptions and inherit new workspace fields**

In `crates/pixtuoid-core/Cargo.toml`, add:
```toml
description = "Headless engine for pixtuoid — state, sprites, layout"
authors.workspace    = true
homepage.workspace   = true
keywords.workspace   = true
categories.workspace = true
```

In `crates/pixtuoid/Cargo.toml`, add:
```toml
description = "Terminal pixel-art office for AI coding agents"
authors.workspace    = true
homepage.workspace   = true
keywords.workspace   = true
categories.workspace = true
```

In `crates/pixtuoid-hook/Cargo.toml`, add:
```toml
description = "Lightweight hook shim for pixtuoid"
authors.workspace    = true
homepage.workspace   = true
keywords.workspace   = true
categories.workspace = true
```

- [ ] **Step 3: Verify workspace builds**

```bash
cargo check --workspace
```

Expected: success, no errors.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/pixtuoid-core/Cargo.toml crates/pixtuoid/Cargo.toml crates/pixtuoid-hook/Cargo.toml
git commit -m "chore: add Cargo.toml metadata (authors, description, homepage, keywords)"
```

---

### Task 3: Add git-cliff config

**Files:**
- Create: `cliff.toml`

- [ ] **Step 1: Create cliff.toml**

```toml
[changelog]
header = ""
body = """
{% for group, commits in commits | group_by(attribute="group") %}
### {{ group | upper_first }}
{% for commit in commits %}
- {{ commit.message | split(pat="\\n") | first }}\
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

- [ ] **Step 2: Verify locally (optional, requires git-cliff installed)**

```bash
git-cliff --unreleased
```

Expected: grouped list of recent commits. If git-cliff is not installed, skip — CI installs it.

- [ ] **Step 3: Commit**

```bash
git add cliff.toml
git commit -m "chore: add git-cliff changelog config"
```

---

### Task 4: Create release workflow

The core CI/CD pipeline. Three jobs: build (4-way matrix) → release → homebrew.

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Create the workflow file**

```yaml
name: release

on:
  push:
    tags: ["v*"]

permissions:
  contents: write

env:
  CARGO_TERM_COLOR: always

jobs:
  build:
    name: build (${{ matrix.target }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
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
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.target }}

      - name: Install cross
        if: matrix.cross
        run: cargo install cross --git https://github.com/cross-rs/cross

      - name: Build
        run: |
          if [ "${{ matrix.cross }}" = "true" ]; then
            cross build --release --target ${{ matrix.target }}
          else
            cargo build --release --target ${{ matrix.target }}
          fi

      - name: Package
        run: |
          VERSION="${GITHUB_REF_NAME}"
          DIR="pixtuoid-${VERSION}-${{ matrix.target }}"
          mkdir "$DIR"
          cp "target/${{ matrix.target }}/release/pixtuoid" "$DIR/"
          cp "target/${{ matrix.target }}/release/pixtuoid-hook" "$DIR/"
          cp LICENSE "$DIR/"
          tar czf "${DIR}.tar.gz" "$DIR"

      - uses: actions/upload-artifact@v4
        with:
          name: artifact-${{ matrix.target }}
          path: "*.tar.gz"
          if-no-files-found: error

  release:
    name: release
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Download artifacts
        uses: actions/download-artifact@v4
        with:
          path: artifacts
          merge-multiple: true

      - name: Generate checksums
        run: |
          cd artifacts
          sha256sum *.tar.gz > sha256sums.txt

      - name: Generate changelog
        id: changelog
        uses: orhun/git-cliff-action@v4
        with:
          config: cliff.toml
          args: --latest --strip header

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          body: ${{ steps.changelog.outputs.content }}
          files: |
            artifacts/*.tar.gz
            artifacts/sha256sums.txt

  homebrew:
    name: update homebrew tap
    needs: release
    runs-on: ubuntu-latest
    steps:
      - name: Download checksums
        uses: actions/download-artifact@v4
        with:
          name: artifact-aarch64-apple-darwin
          path: artifacts

      - name: Download all artifacts for checksums
        uses: actions/download-artifact@v4
        with:
          path: artifacts
          merge-multiple: true

      - name: Compute checksums and update formula
        env:
          HOMEBREW_TAP_TOKEN: ${{ secrets.HOMEBREW_TAP_TOKEN }}
        run: |
          VERSION="${GITHUB_REF_NAME#v}"
          TAG="${GITHUB_REF_NAME}"

          SHA_MACOS_ARM=$(sha256sum "artifacts/pixtuoid-${TAG}-aarch64-apple-darwin.tar.gz" | cut -d' ' -f1)
          SHA_MACOS_INTEL=$(sha256sum "artifacts/pixtuoid-${TAG}-x86_64-apple-darwin.tar.gz" | cut -d' ' -f1)
          SHA_LINUX_ARM=$(sha256sum "artifacts/pixtuoid-${TAG}-aarch64-unknown-linux-gnu.tar.gz" | cut -d' ' -f1)
          SHA_LINUX_INTEL=$(sha256sum "artifacts/pixtuoid-${TAG}-x86_64-unknown-linux-gnu.tar.gz" | cut -d' ' -f1)

          FORMULA=$(cat <<RUBY
          class Pixtuoid < Formula
            desc "Terminal pixel-art office for AI coding agents"
            homepage "https://github.com/IvanWng97/pixtuoid"
            version "${VERSION}"
            license "MIT"

            on_macos do
              on_arm do
                url "https://github.com/IvanWng97/pixtuoid/releases/download/${TAG}/pixtuoid-${TAG}-aarch64-apple-darwin.tar.gz"
                sha256 "${SHA_MACOS_ARM}"
              end
              on_intel do
                url "https://github.com/IvanWng97/pixtuoid/releases/download/${TAG}/pixtuoid-${TAG}-x86_64-apple-darwin.tar.gz"
                sha256 "${SHA_MACOS_INTEL}"
              end
            end

            on_linux do
              on_arm do
                url "https://github.com/IvanWng97/pixtuoid/releases/download/${TAG}/pixtuoid-${TAG}-aarch64-unknown-linux-gnu.tar.gz"
                sha256 "${SHA_LINUX_ARM}"
              end
              on_intel do
                url "https://github.com/IvanWng97/pixtuoid/releases/download/${TAG}/pixtuoid-${TAG}-x86_64-unknown-linux-gnu.tar.gz"
                sha256 "${SHA_LINUX_INTEL}"
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
          RUBY
          )

          git clone "https://x-access-token:${HOMEBREW_TAP_TOKEN}@github.com/IvanWng97/homebrew-pixtuoid.git" tap
          mkdir -p tap/Formula
          echo "$FORMULA" > tap/Formula/pixtuoid.rb
          cd tap
          git config user.name "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git add Formula/pixtuoid.rb
          git commit -m "pixtuoid ${VERSION}"
          git push
```

- [ ] **Step 2: Validate YAML syntax**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))" && echo "OK"
```

Expected: `OK`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: add release workflow (build matrix, GitHub Releases, Homebrew tap)"
```

---

### Task 5: Create install.sh

POSIX-sh shell installer that detects OS/arch, downloads the matching tarball from GitHub Releases, verifies checksum, installs both binaries.

**Files:**
- Create: `install.sh`

- [ ] **Step 1: Write install.sh**

```sh
#!/bin/sh
set -eu

REPO="IvanWng97/pixtuoid"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

main() {
    parse_args "$@"
    detect_platform
    resolve_version
    download_and_verify
    install_binaries
    print_success
}

parse_args() {
    for arg in "$@"; do
        case "$arg" in
            --global)
                INSTALL_DIR="/usr/local/bin"
                ;;
            --help|-h)
                echo "Usage: install.sh [--global]"
                echo ""
                echo "Install pixtuoid to \$INSTALL_DIR (default: ~/.local/bin)"
                echo "  --global    Install to /usr/local/bin"
                echo ""
                echo "Environment variables:"
                echo "  INSTALL_DIR   Override install location"
                echo "  VERSION       Pin a specific version (e.g. 0.2.0)"
                exit 0
                ;;
            *)
                err "Unknown argument: $arg"
                ;;
        esac
    done
}

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Darwin) ;;
        Linux)  ;;
        *)      err "Unsupported OS: $OS (only Darwin and Linux are supported)" ;;
    esac

    case "$ARCH" in
        x86_64)         TARGET_ARCH="x86_64" ;;
        aarch64|arm64)  TARGET_ARCH="aarch64" ;;
        *)              err "Unsupported architecture: $ARCH" ;;
    esac

    case "$OS" in
        Darwin) TARGET="${TARGET_ARCH}-apple-darwin" ;;
        Linux)  TARGET="${TARGET_ARCH}-unknown-linux-gnu" ;;
    esac

    if command -v curl >/dev/null 2>&1; then
        FETCH="curl -fsSL"
    elif command -v wget >/dev/null 2>&1; then
        FETCH="wget -qO-"
    else
        err "Neither curl nor wget found. Install one and retry."
    fi

    echo "Detected platform: $TARGET"
}

resolve_version() {
    if [ -n "${VERSION:-}" ]; then
        TAG="v${VERSION}"
    else
        TAG=$($FETCH "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | head -1 | cut -d'"' -f4)
        if [ -z "$TAG" ]; then
            err "Could not determine latest release. Set VERSION= to install a specific version."
        fi
    fi
    echo "Installing pixtuoid ${TAG}..."
}

download_and_verify() {
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"
    TARBALL="pixtuoid-${TAG}-${TARGET}.tar.gz"

    echo "Downloading ${TARBALL}..."
    $FETCH "${BASE_URL}/${TARBALL}" > "${TMPDIR}/${TARBALL}"
    $FETCH "${BASE_URL}/sha256sums.txt" > "${TMPDIR}/sha256sums.txt"

    echo "Verifying checksum..."
    EXPECTED=$(grep "${TARBALL}" "${TMPDIR}/sha256sums.txt" | cut -d' ' -f1)
    if [ -z "$EXPECTED" ]; then
        err "Tarball ${TARBALL} not found in sha256sums.txt"
    fi

    if command -v sha256sum >/dev/null 2>&1; then
        ACTUAL=$(sha256sum "${TMPDIR}/${TARBALL}" | cut -d' ' -f1)
    elif command -v shasum >/dev/null 2>&1; then
        ACTUAL=$(shasum -a 256 "${TMPDIR}/${TARBALL}" | cut -d' ' -f1)
    else
        err "Neither sha256sum nor shasum found. Cannot verify checksum."
    fi

    if [ "$EXPECTED" != "$ACTUAL" ]; then
        err "Checksum mismatch! Expected: ${EXPECTED}, got: ${ACTUAL}"
    fi
    echo "Checksum verified."

    tar xzf "${TMPDIR}/${TARBALL}" -C "${TMPDIR}"
}

install_binaries() {
    mkdir -p "$INSTALL_DIR"

    EXTRACT_DIR="${TMPDIR}/pixtuoid-${TAG}-${TARGET}"
    cp "${EXTRACT_DIR}/pixtuoid" "${INSTALL_DIR}/pixtuoid"
    cp "${EXTRACT_DIR}/pixtuoid-hook" "${INSTALL_DIR}/pixtuoid-hook"
    chmod +x "${INSTALL_DIR}/pixtuoid" "${INSTALL_DIR}/pixtuoid-hook"
}

print_success() {
    echo ""
    echo "pixtuoid installed successfully to ${INSTALL_DIR}"

    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            echo ""
            echo "WARNING: ${INSTALL_DIR} is not in your \$PATH."
            echo "Add it with:"
            echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
            ;;
    esac

    echo ""
    echo "Get started:"
    echo "  pixtuoid install-hooks"
    echo "  pixtuoid run"
}

err() {
    echo "Error: $1" >&2
    exit 1
}

main "$@"
```

- [ ] **Step 2: Make executable**

```bash
chmod +x install.sh
```

- [ ] **Step 3: Run shellcheck**

```bash
shellcheck install.sh
```

Expected: no warnings or errors.

- [ ] **Step 4: Commit**

```bash
git add install.sh
git commit -m "feat: add curl|sh installer for pre-built binaries"
```

---

### Task 6: Verify everything builds cleanly

Final sanity check — make sure nothing was broken.

**Files:** none (read-only)

- [ ] **Step 1: Full workspace check**

```bash
cargo check --workspace
cargo test --workspace --features pixtuoid-core/test-renderer
```

Expected: all pass, no warnings.

- [ ] **Step 2: Verify YAML parses**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))" && echo "OK"
```

Expected: `OK`

- [ ] **Step 3: Verify shellcheck passes**

```bash
shellcheck install.sh
```

Expected: clean.
