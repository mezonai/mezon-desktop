# ------------------------------------------------------------------------------
# GENERAL
# ------------------------------------------------------------------------------

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
    @echo "  run             Build (debug) and run the app"
    @echo "  watch           Hot-reload development (requires cargo-watch)"
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

# ------------------------------------------------------------------------------
# DEVELOPMENT
# ------------------------------------------------------------------------------

# Install all necessary CLI tools via cargo-binstall
install:
    @echo "Installing development tools..."
    cargo install cargo-binstall || true
    cargo binstall -y cargo-watch cargo-nextest cargo-deny cargo-outdated cargo-llvm-cov

# Run the project with optional arguments
run *args:
    cargo run {{args}}

# Hot-reload development (requires cargo-watch)
watch:
    cargo watch -x run

# Fast check for errors during development
check:
    cargo clippy --workspace -- -D warnings

# Strict linting (Use before commit/push)
lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    cargo fmt --all -- --check

# Auto-fix formatting and clippy suggestions
fix:
    cargo fmt --all
    cargo clippy --workspace --fix --allow-dirty --allow-staged

# ------------------------------------------------------------------------------
# TESTING (Nextest)
# ------------------------------------------------------------------------------

# Run all tests in the workspace, or pass args straight to cargo-nextest
test *args:
    sh -c 'if [ "$#" -eq 0 ]; then exec cargo nextest run --workspace --all-targets; fi; exec cargo nextest run "$@"' sh {{args}}

# ------------------------------------------------------------------------------
# CODE COVERAGE (llvm-cov)
# ------------------------------------------------------------------------------

# Generate and open HTML coverage report
cov:
    cargo llvm-cov --workspace --all-features --open

# Run coverage and show summary in terminal
cov-summary:
    cargo llvm-cov --workspace --all-features

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

# Build production release
release:
    cargo build --release

# Clean build artifacts
clean:
    cargo clean
    @echo "Cleaned target directory."
    


