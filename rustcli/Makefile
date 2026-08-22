# Causewaybay Wallet — Rust CLI/TUI
#
# `make` lists the targets; `make test` runs everything.

CARGO ?= cargo
BIN     := cwbwallet
NAME    := cwbwallet-rust
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

# Overridden by the root Makefile so both implementations agree on one output
# directory; the default keeps `make -C rustcli package` working on its own.
DIST_DIR ?= $(abspath $(CURDIR)/../dist)
SIGN     := $(abspath $(CURDIR)/../scripts/codesign-binary.sh)

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@echo "Causewaybay Wallet — Rust ($(BIN))"
	@echo
	@grep -hE '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
	@echo

.PHONY: build
build: ## Build the debug binary
	$(CARGO) build

.PHONY: release
release: ## Build the optimised binary
	@echo "==> cargo build --release ($(BIN) $(VERSION))"
	@$(CARGO) build --release --quiet

.PHONY: package
package: release ## Copy the signed release binary into $(DIST_DIR)
	@mkdir -p "$(DIST_DIR)"
	@cp -f target/release/$(BIN) "$(DIST_DIR)/$(NAME)"
	@chmod +x "$(DIST_DIR)/$(NAME)"
	@"$(SIGN)" "$(DIST_DIR)/$(NAME)"
	@echo "  built $(DIST_DIR)/$(NAME) ($$(du -h "$(DIST_DIR)/$(NAME)" | cut -f1 | tr -d ' '))"

.PHONY: test
test: ## Run unit and integration tests
	$(CARGO) test --all-targets

.PHONY: test-verbose
test-verbose: ## Run tests with output from passing tests too
	$(CARGO) test --all-targets -- --nocapture

.PHONY: test-vectors
test-vectors: ## Run only the shared-test-vector suite
	$(CARGO) test --test vectors

.PHONY: format
format: ## Format the source, then lint what formatting cannot fix
	@echo "==> rustfmt"
	@$(CARGO) fmt
	@echo "==> clippy"
	@$(CARGO) clippy --all-targets -- -D warnings

.PHONY: lint
lint: ## Run clippy with warnings denied, changing nothing
	$(CARGO) clippy --all-targets -- -D warnings

.PHONY: fmt
fmt: format ## Alias for `format`

.PHONY: fmt-check
fmt-check: ## Fail if the source is not formatted
	$(CARGO) fmt --check

.PHONY: check
check: fmt-check lint test ## Everything CI would run

.PHONY: run
run: ## Run the CLI, e.g. make run ARGS="account list"
	$(CARGO) run --quiet -- $(ARGS)

.PHONY: tui
tui: ## Launch the terminal UI
	$(CARGO) run --quiet -- tui

.PHONY: install
install: ## Install cwbwallet into ~/.cargo/bin
	$(CARGO) install --path . --force

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
