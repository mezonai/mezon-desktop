# Mezon Desktop (Rust/GPUI) - Makefile
# Usage: make <target>

APP_NAME    := mezon
CARGO_DIR   := .
BINARY_DEV  := target/debug/$(APP_NAME)
BINARY_REL  := target/release/$(APP_NAME)

# Default target
.DEFAULT_GOAL := help

# -- Help ----------------------------------------------------------------------
.PHONY: help
help:
	@echo ""
	@echo "  Mezon Desktop (Rust/GPUI)"
	@echo ""
	@echo "  Usage: make <target>"
	@echo ""
	@echo "  Development"
	@echo "  ---------------------------------------------"
	@echo "  run          Build (debug) and run the app"
	@echo "  run-release  Build (release) and run the app"
	@echo "  build        Build debug binary"
	@echo "  build-release Build optimised release binary"
	@echo "  watch        Auto-rebuild on file changes (requires cargo-watch)"
	@echo ""
	@echo "  Quality"
	@echo "  ---------------------------------------------"
	@echo "  check        Fast type-check (no codegen)"
	@echo "  clippy       Run Clippy linter"
	@echo "  fmt          Format all Rust code"
	@echo "  fmt-check    Check formatting without applying"
	@echo "  test         Run all tests"
	@echo ""
	@echo "  Maintenance"
	@echo "  ---------------------------------------------"
	@echo "  clean        Remove build artifacts"
	@echo "  update       Update Cargo dependencies"
	@echo "  deps         Install required dev tools"
	@echo ""

# -- Development ---------------------------------------------------------------
.PHONY: run
run:
	@cd $(CARGO_DIR) && RUST_LOG=mezon=debug,info cargo run

.PHONY: run-release
run-release:
	@cd $(CARGO_DIR) && cargo run --release

.PHONY: build
build:
	@cd $(CARGO_DIR) && cargo build
	@echo ""
	@echo "  Binary: $(BINARY_DEV)"

.PHONY: build-release
build-release:
	@cd $(CARGO_DIR) && cargo build --release
	@echo ""
	@echo "  Binary: $(BINARY_REL)"

.PHONY: watch
watch:
	@which cargo-watch > /dev/null 2>&1 || (echo "cargo-watch not found - run: make deps" && exit 1)
	@cd $(CARGO_DIR) && RUST_LOG=mezon=debug,info cargo watch -x run

# -- Quality -------------------------------------------------------------------
.PHONY: check
check:
	@cd $(CARGO_DIR) && cargo check

.PHONY: clippy
clippy:
	@cd $(CARGO_DIR) && cargo clippy -- -D warnings

.PHONY: fmt
fmt:
	@cd $(CARGO_DIR) && cargo fmt

.PHONY: fmt-check
fmt-check:
	@cd $(CARGO_DIR) && cargo fmt -- --check

.PHONY: test
test:
	@cd $(CARGO_DIR) && cargo test

# -- Maintenance ---------------------------------------------------------------
.PHONY: clean
clean:
	@cd $(CARGO_DIR) && cargo clean
	@echo "  Build artifacts removed."

.PHONY: update
update:
	@cd $(CARGO_DIR) && cargo update

.PHONY: deps
deps:
	@echo "  Installing cargo-watch..."
	@cargo install cargo-watch
	@echo "  Done."
