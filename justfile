# ------------------------------------------------------------------------------
# GENERAL
# ------------------------------------------------------------------------------

# Crates we own — vendored Zed crates are excluded (we don't lint/test their code;
# some of their test targets don't even compile against our pinned deps).
pkgs := "-p mezon-app -p mezon-ui -p mezon-store -p mezon-client -p mezon-native -p mezon-proto -p mezon-i18n -p mezon-theme -p mezon-widgets -p mezon-canvas -p mezon-updater -p mezon-audio -p mezon-cli -p mezon-mcp -p mezon-voice -p mezon-stream -p mezon-record  -p mezon-call -p mmn-client"

# Formatting scope lives in scripts/fmt.sh — the one place the justfile, the
# pre-commit hook and CI all read it from, so they cannot drift apart.

# List available recipes
default:
    @just help

help:
    @echo ""
    @echo "  Mezon Desktop (Rust/GPUI)"
    @echo ""
    @echo "  Usage: just <recipe>"
    @echo ""
    @echo "  Development"
    @echo "  ---------------------------------------------"
    @echo "  install           Install development tools (via cargo-binstall)"
    @echo "  install-linux-deps Install Linux system libraries for GPUI/GTK"
    @echo "  run             Build (debug) and run the app (loads .env)"
    @echo "  watch           Hot-reload development (requires cargo-watch, loads .env)"
    @echo "  check           Fast clippy checks"
    @echo "  lint            Strict linting before commit"
    @echo "  fix             Auto-fix formatting and clippy suggestions"
    @echo ""
    @echo "  Testing"
    @echo "  ---------------------------------------------"
    @echo "  test            Run all tests in the workspace"
    @echo "  test <args>     Forward args to cargo-nextest"
    @echo "                  e.g. just test -p my_crate"
    @echo "                  e.g. just test my_test_name"
    @echo ""
    @echo "  Coverage"
    @echo "  ---------------------------------------------"
    @echo "  cov             Generate and open HTML coverage report"
    @echo "  cov-summary     Show coverage summary in terminal"
    @echo ""
    @echo "  Security & Maintenance"
    @echo "  ---------------------------------------------"
    @echo "  safety          Run security and license checks"
    @echo "  audit           Audit dependencies for advisories"
    @echo "  outdated        Check for outdated dependencies"
    @echo "  update          Update Cargo dependencies"
    @echo ""
    @echo "  Packaging"
    @echo "  ---------------------------------------------"
    @echo "  bundle          Build macOS Mezon.app bundle"
    @echo "  build-deb       Build Linux .deb package"
    @echo "  update-feed     Package auto-update artifact + manifest"
    @echo ""

# ------------------------------------------------------------------------------
# DEVELOPMENT
# ------------------------------------------------------------------------------

# Install all necessary CLI tools via cargo-binstall
install:
    @echo "Installing development tools..."
    cargo install cargo-binstall || true
    cargo binstall -y cargo-watch cargo-nextest cargo-deny cargo-outdated cargo-llvm-cov

# Install Linux system libraries required to build GPUI, GTK tray, and accessibility
install-linux-deps:
    @bash scripts/linux-deps

# Run the project with optional arguments (loads .env when present)
run *args:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/load-env.sh
    exec cargo run {{args}}

# Hot-reload development (requires cargo-watch)
watch:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/load-env.sh
    exec cargo watch -x run

# Profile with Tracy (open Tracy 0.11.x GUI to connect; CPU + memory + frames)
tracy:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/load-env.sh
    exec cargo run --profile profiling --features tracy

# Fast check for errors during development
check:
    cargo clippy {{pkgs}} -- -D warnings

# Formatting gate — the single source of truth for the fmt scope.
# The pre-commit hook and CI both call this, so they can never drift apart.
fmt-check:
    ./scripts/fmt.sh check

# Strict linting (Use before commit/push)
lint: _ensure-hooks
    cargo clippy {{pkgs}} --all-targets --all-features --locked -- -D warnings
    @just fmt-check

# Auto-fix formatting and clippy suggestions
fix:
    ./scripts/fmt.sh fix
    cargo clippy {{pkgs}} --fix --allow-dirty --allow-staged

# Point git at the repo's tracked hooks. Idempotent; every dev gets the same
# pre-commit gate without having to remember to install anything.
setup-hooks:
    git config core.hooksPath .githooks
    @echo "git hooks -> .githooks (pre-commit runs 'just fmt-check')"

# Installs the hooks on first use of any common recipe, so a fresh clone is
# formatted-by-default rather than only caught in CI.
_ensure-hooks:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(git config --get core.hooksPath || true)" != ".githooks" ]; then
        git config core.hooksPath .githooks
        echo "installed git hooks -> .githooks"
    fi

# ------------------------------------------------------------------------------
# TESTING (Nextest)
# ------------------------------------------------------------------------------

# Run all tests in the workspace, or pass args straight to cargo-nextest
test *args:
    sh -c 'if [ "$#" -eq 0 ]; then exec cargo nextest run {{pkgs}} --all-targets; fi; exec cargo nextest run "$@"' sh {{args}}

# ------------------------------------------------------------------------------
# CODE COVERAGE (llvm-cov)
# ------------------------------------------------------------------------------

# Generate and open HTML coverage report
cov:
    cargo llvm-cov {{pkgs}} --all-features --open

# Run coverage and show summary in terminal
cov-summary:
    cargo llvm-cov {{pkgs}} --all-features

# ------------------------------------------------------------------------------
# SECURITY & MAINTENANCE
# ------------------------------------------------------------------------------

# Run all security and license checks
safety:
    cargo deny check

# Audit dependencies for security vulnerabilities
audit:
    cargo deny check advisories

bans:
    cargo deny check bans

# Check for outdated dependencies
outdated:
    cargo outdated -R

# Update dependencies
update:
    cargo update

# ------------------------------------------------------------------------------
# BUILD & CLEAN
# ------------------------------------------------------------------------------

# Build production release (loads .env when present)
release:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/load-env.sh
    exec cargo build --release

# Bundle the release binary into a macOS Mezon.app
bundle: release
    #!/usr/bin/env bash
    set -euo pipefail
    app="target/release/bundle/Mezon.app"
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp crates/mezon-app/Info.plist "$app/Contents/Info.plist"
    cp target/release/mezon "$app/Contents/MacOS/mezon"
    chmod +x "$app/Contents/MacOS/mezon"
    for icon in assets/icons/app.icns crates/mezon-app/app.icns assets/app.icns; do
        if [ -f "$icon" ]; then cp "$icon" "$app/Contents/Resources/app.icns"; break; fi
    done
    codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
    echo "Built $app"
    echo "Run: open \"$app\"  (or double-click in Finder)"

# Build a Linux .deb package (requires Linux; run install-linux-deps first)
build-deb:
    @bash scripts/build-deb.sh

update-feed platform artifact="":
    @bash scripts/make-update-feed.sh {{platform}} {{artifact}}

# Clean build artifacts
clean:
    cargo clean
    @echo "Cleaned target directory."
    


