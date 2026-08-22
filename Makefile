# Causewaybay Wallet — an educational Cronos/EVM wallet.
#
# Two implementations of one specification (see SPEC.md): a Rust CLI/TUI in
# rustcli/ and a Python CLI/TUI in pythoncli/. They share the same on-disk
# JSONL store, so either can drive a wallet the other created.
#
#   make        show this help
#   make test   run every test in both implementations

RUST_DIR   := rustcli
PYTHON_DIR := pythoncli

RUST_BIN   := $(RUST_DIR)/target/debug/cwbwallet
PYTHON_BIN := $(PYTHON_DIR)/.venv/bin/python

# Where every artifact lands, whichever implementation produced it. Passed down
# so a sub-Makefile invoked from here never has to guess.
DIST_DIR ?= $(abspath $(CURDIR)/dist)

# The version of record: both implementations must agree before anything ships.
RS_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(RUST_DIR)/Cargo.toml | head -1)
PY_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(PYTHON_DIR)/pyproject.toml | head -1)

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@echo
	@echo "  Causewaybay Wallet — Cronos EVM wallet (testnet + mainnet)"
	@echo "  Rust: $(RUST_DIR)    Python: $(PYTHON_DIR)    Store: ~/.causewaybaywallet"
	@echo
	@grep -hE '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "  Per-language targets live in $(RUST_DIR)/Makefile and $(PYTHON_DIR)/Makefile."
	@echo

# ---------------------------------------------------------------------- tests

.PHONY: test
test: test-rust test-python test-vectors test-vector-coverage test-parity ## Run every test in both implementations
	@echo
	@echo "All tests passed."

.PHONY: test-rust
test-rust: ## Run the Rust unit and integration tests
	@echo "==> Rust tests"
	@$(MAKE) --no-print-directory -C $(RUST_DIR) test

.PHONY: test-python
test-python: ## Run the Python test suite
	@echo "==> Python tests"
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) test

.PHONY: test-parity
test-parity: build ## Check that both implementations agree on the store and the CLI
	@echo "==> Cross-implementation parity"
	@./scripts/parity.sh

.PHONY: test-vectors
test-vectors: ## Check the shared test vectors are reproducible
	@echo "==> Test vectors"
	@./scripts/check-vectors.sh

.PHONY: test-vector-coverage
test-vector-coverage: build ## Prove both implementations really read the shared vectors
	@echo "==> Vector coverage (mutation check)"
	@$(PYTHON_BIN) scripts/check-vector-coverage.py

.PHONY: vectors
vectors: build-python ## Regenerate the shared test vectors
	@$(PYTHON_BIN) scripts/gen-vectors.py

# ------------------------------------------------------------------ packaging

.PHONY: package
package: package-versions package-rust package-python ## Build both binaries into ./dist
	@echo
	@echo "  $(DIST_DIR):"
	@ls -1lh "$(DIST_DIR)" 2>/dev/null | tail -n +2 | awk '{printf "    %-22s %s\n", $$9, $$5}'
	@echo
	@echo "  Both speak the same CLI and share ~/.causewaybaywallet."

.PHONY: package-rust
package-rust: ## Only the Rust release binary
	@$(MAKE) --no-print-directory -C $(RUST_DIR) package DIST_DIR="$(DIST_DIR)"

.PHONY: package-python
package-python: ## Only the Python binary (PyApp: embeds CPython and the wheel)
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) package DIST_DIR="$(DIST_DIR)"

# One version across both implementations, checked before anything is built. A
# binary in ./dist stamped with a number nothing verified is the worst artifact
# a release can carry.
.PHONY: package-versions
package-versions:
	@test "$(RS_VERSION)" = "$(PY_VERSION)" || { \
		echo "ERROR: $(RUST_DIR)/Cargo.toml is $(RS_VERSION), $(PYTHON_DIR)/pyproject.toml is $(PY_VERSION)"; \
		exit 1; }
	@echo "==> packaging $(RS_VERSION) into $(DIST_DIR)"

# Prove the shipped artifacts are the ones that were tested, not just that the
# source tree passes. Runs the parity checks against ./dist.
.PHONY: package-verify
package-verify: package ## Package, then run the parity checks against ./dist
	@echo "==> Verifying the packaged binaries"
	@CWB_RUST_BIN="$(DIST_DIR)/cwbwallet-rust" \
	 CWB_PYTHON_BIN="$(DIST_DIR)/cwbwallet-python" \
	 ./scripts/parity.sh

# --------------------------------------------------------------------- builds

.PHONY: build
build: build-rust build-python ## Build both implementations

.PHONY: build-rust
build-rust: ## Build the Rust binary
	@$(MAKE) --no-print-directory -C $(RUST_DIR) build

.PHONY: build-python
build-python: ## Install the Python package into its virtualenv
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) install

# --------------------------------------------------------------------- checks

.PHONY: format
format: ## Format and lint both implementations
	@echo "==> Rust"
	@$(MAKE) --no-print-directory -C $(RUST_DIR) format
	@echo "==> Python"
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) format
	@echo
	@echo "  Formatted and linted: $(RUST_DIR) and $(PYTHON_DIR)."

.PHONY: lint
lint: ## Lint both implementations, changing nothing
	@$(MAKE) --no-print-directory -C $(RUST_DIR) lint
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) lint

.PHONY: fmt
fmt: format ## Alias for `format`

.PHONY: check
check: lint test ## Everything CI would run (lints without rewriting)

# ----------------------------------------------------------------------- demos

.PHONY: demo
demo: build ## Create a throwaway wallet and show both CLIs reading it
	@./scripts/parity.sh --verbose

.PHONY: tui-rust
tui-rust: ## Launch the Rust terminal UI
	@$(MAKE) --no-print-directory -C $(RUST_DIR) tui

.PHONY: tui-python
tui-python: ## Launch the Python terminal UI
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) tui

# ----------------------------------------------------------------------- clean

.PHONY: clean
clean: ## Remove build output from both implementations and ./dist
	@$(MAKE) --no-print-directory -C $(RUST_DIR) clean
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) clean
	@rm -rf "$(DIST_DIR)"
