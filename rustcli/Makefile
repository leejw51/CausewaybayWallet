# Causewaybay Wallet — the Rust workspace
#
#   core/  the wallet, with no terminal attached
#   ffi/   a C ABI over core, for the Lua CLI and the LÖVE GUI
#   cli/   the `cwbwallet` binary
#
# `make` lists the targets; `make test` runs everything.

CARGO ?= cargo
BIN     := cwbwallet
NAME    := cwbwallet-rust
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

# The shared library the Lua front ends load. Cargo names a cdylib after the
# platform, so the Makefile has to as well.
UNAME := $(shell uname -s)
ifeq ($(UNAME),Darwin)
  FFI_LIB := libcausewaybay_ffi.dylib
else ifeq ($(OS),Windows_NT)
  FFI_LIB := causewaybay_ffi.dll
else
  FFI_LIB := libcausewaybay_ffi.so
endif

# Overridden by the root Makefile so every implementation agrees on one output
# directory; the default keeps `make -C rustcli package` working on its own.
DIST_DIR ?= $(abspath $(CURDIR)/../dist)
SIGN     := $(abspath $(CURDIR)/../scripts/codesign-binary.sh)

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@echo "Causewaybay Wallet — Rust workspace ($(BIN) $(VERSION))"
	@echo
	@grep -hE '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "  core/ the wallet   ffi/ the C ABI   cli/ the cwbwallet binary"
	@echo

.PHONY: build
build: ## Build every crate in the workspace (debug)
	$(CARGO) build --workspace

.PHONY: ffi
ffi: ## Build only the shared library the Lua front ends load
	@$(CARGO) build -p causewaybay-ffi
	@echo "  built target/debug/$(FFI_LIB)"

.PHONY: release
release: ## Build the optimised binary and shared library
	@echo "==> cargo build --release ($(BIN) $(VERSION))"
	@$(CARGO) build --release --workspace --quiet

.PHONY: package
package: release ## Copy the signed release artefacts into $(DIST_DIR)
	@mkdir -p "$(DIST_DIR)"
	@cp -f target/release/$(BIN) "$(DIST_DIR)/$(NAME)"
	@chmod +x "$(DIST_DIR)/$(NAME)"
	@"$(SIGN)" "$(DIST_DIR)/$(NAME)"
	@echo "  built $(DIST_DIR)/$(NAME) ($$(du -h "$(DIST_DIR)/$(NAME)" | cut -f1 | tr -d ' '))"
	@cp -f target/release/$(FFI_LIB) "$(DIST_DIR)/$(FFI_LIB)"
	@"$(SIGN)" "$(DIST_DIR)/$(FFI_LIB)"
	@cp -f ffi/include/causewaybay.h "$(DIST_DIR)/causewaybay.h"
	@echo "  built $(DIST_DIR)/$(FFI_LIB) ($$(du -h "$(DIST_DIR)/$(FFI_LIB)" | cut -f1 | tr -d ' '))"

.PHONY: test
test: ## Run unit, integration and doc tests across the workspace
	$(CARGO) test --workspace --all-targets
	$(CARGO) test --workspace --doc

.PHONY: test-verbose
test-verbose: ## Run tests with output from passing tests too
	$(CARGO) test --workspace --all-targets -- --nocapture

.PHONY: test-vectors
test-vectors: ## Run only the shared-test-vector suite
	$(CARGO) test -p causewaybay-core --test vectors

.PHONY: test-ffi
test-ffi: ## Run only the C ABI tests
	$(CARGO) test -p causewaybay-ffi

.PHONY: format
format: ## Format the source, then lint what formatting cannot fix
	@echo "==> rustfmt"
	@$(CARGO) fmt --all
	@echo "==> clippy"
	@$(CARGO) clippy --workspace --all-targets -- -D warnings

.PHONY: lint
lint: ## Run clippy with warnings denied, changing nothing
	$(CARGO) clippy --workspace --all-targets -- -D warnings

.PHONY: fmt
fmt: format ## Alias for `format`

.PHONY: fmt-check
fmt-check: ## Fail if the source is not formatted
	$(CARGO) fmt --all --check

.PHONY: check
check: fmt-check lint test ## Everything CI would run

.PHONY: run
run: ## Run the CLI, e.g. make run ARGS="account list"
	$(CARGO) run --quiet -p causewaybay-cli -- $(ARGS)

.PHONY: tui
tui: ## Launch the terminal UI
	$(CARGO) run --quiet -p causewaybay-cli -- tui

.PHONY: install
install: ## Install cwbwallet into ~/.cargo/bin
	$(CARGO) install --path cli --force

.PHONY: demo
demo: build ## Create a throwaway wallet in ./.demo-home and show it off
	@rm -rf .demo-home
	@CAUSEWAYBAY_HOME=$$PWD/.demo-home ./target/debug/$(BIN) account import-mnemonic \
		-m "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about" \
		--label demo
	@CAUSEWAYBAY_HOME=$$PWD/.demo-home ./target/debug/$(BIN) account list
	@echo
	@echo "Store contents:"
	@cat .demo-home/accounts.jsonl

.PHONY: clean
clean: ## Remove build artefacts and the demo home
	$(CARGO) clean
	rm -rf .demo-home
