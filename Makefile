# Causewaybay Wallet — an educational Cronos/EVM wallet.
#
# One specification (see SPEC.md), four front ends:
#
#   rustcli/    the Rust workspace — core/ (the wallet), ffi/ (a C ABI over
#               it), cli/ (the cwbwallet binary and TUI)
#   pythoncli/  an independent Python implementation of the same spec
#   luacli/     a Lua CLI and LÖVE module, loading the shared library at run
#               time — which is what LÖVE needs
#   ccli/       a C CLI with the static library compiled in, so the binary
#               carries the whole wallet and needs nothing beside it
#
# Rust and Python are two implementations that must agree; Lua and C are front
# ends over the Rust one, reached the two ways a library can be reached. All
# four share ~/.causewaybaywallet, so any of them can drive a wallet another
# created.
#
#   make        show this help
#   make test   run every test in all four

RUST_DIR   := rustcli
PYTHON_DIR := pythoncli
LUA_DIR    := luacli
C_DIR      := ccli

RUST_BIN   := $(RUST_DIR)/target/debug/cwbwallet
PYTHON_BIN := $(PYTHON_DIR)/.venv/bin/python

# Where every artifact lands, whichever front end produced it. Passed down so a
# sub-Makefile invoked from here never has to guess.
DIST_DIR ?= $(abspath $(CURDIR)/dist)

# The version of record: both implementations must agree before anything ships.
# Lua carries no version of its own — it is the Rust core, reached differently.
RS_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(RUST_DIR)/Cargo.toml | head -1)
PY_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(PYTHON_DIR)/pyproject.toml | head -1)

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@echo
	@echo "  Causewaybay Wallet — Cronos EVM wallet (testnet + mainnet)"
	@echo "  Rust: $(RUST_DIR)  Python: $(PYTHON_DIR)  Lua: $(LUA_DIR)  C: $(C_DIR)"
	@echo "  Store: ~/.causewaybaywallet"
	@echo
	@grep -hE '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "  Per-language targets live in each directory's own Makefile."
	@echo

# -------------------------------------------------------------------- version

# The one number a release is allowed to carry, printed only once both
# implementations claim it. Everything that stamps an artifact — packaging, the
# tag check in .github/workflows/release.yml — reads it from here rather than
# picking one manifest and hoping the other kept up.
.PHONY: version
version: ## Print the version both implementations agree on
	@test "$(RS_VERSION)" = "$(PY_VERSION)" || { \
		echo "ERROR: $(RUST_DIR)/Cargo.toml is $(RS_VERSION), $(PYTHON_DIR)/pyproject.toml is $(PY_VERSION)" >&2; \
		exit 1; }
	@echo "$(RS_VERSION)"

# ---------------------------------------------------------------------- tests

.PHONY: test
test: test-rust test-python test-lua test-c test-vectors test-vector-coverage test-parity ## Run every test in all four front ends
	@echo
	@echo "All tests passed."

.PHONY: test-rust
test-rust: ## Run the Rust unit, integration and doc tests
	@echo "==> Rust tests"
	@$(MAKE) --no-print-directory -C $(RUST_DIR) test

.PHONY: test-python
test-python: ## Run the Python test suite
	@echo "==> Python tests"
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) test

.PHONY: test-lua
test-lua: ## Run the Lua test suite (builds the shared library first)
	@echo "==> Lua tests"
	@$(MAKE) --no-print-directory -C $(LUA_DIR) test

.PHONY: test-c
test-c: ## Run the C test suite (builds the static library first)
	@echo "==> C tests"
	@$(MAKE) --no-print-directory -C $(C_DIR) check

.PHONY: test-parity
test-parity: build ## Check that every front end agrees on the store and the CLI
	@echo "==> Cross-implementation parity"
	@./scripts/parity.sh

.PHONY: test-vectors
test-vectors: ## Check the shared test vectors are reproducible
	@echo "==> Test vectors"
	@./scripts/check-vectors.sh

.PHONY: test-vector-coverage
test-vector-coverage: build ## Prove every implementation really reads the shared vectors
	@echo "==> Vector coverage (mutation check)"
	@$(PYTHON_BIN) scripts/check-vector-coverage.py

.PHONY: vectors
vectors: build-python ## Regenerate the shared test vectors
	@$(PYTHON_BIN) scripts/gen-vectors.py

# ------------------------------------------------------------------ packaging

.PHONY: package
package: package-versions package-rust package-python package-lua package-c ## Build every artifact into ./dist
	@echo
	@echo "  $(DIST_DIR):"
	@ls -1lh "$(DIST_DIR)" 2>/dev/null | tail -n +2 | awk '{printf "    %-26s %s\n", $$9, $$5}'
	@echo
	@echo "  All of them speak the same CLI and share ~/.causewaybaywallet."

.PHONY: package-rust
package-rust: ## Only the Rust release binary and shared library
	@$(MAKE) --no-print-directory -C $(RUST_DIR) package DIST_DIR="$(DIST_DIR)"

.PHONY: package-python
package-python: ## Only the Python binary (PyApp: embeds CPython and the wheel)
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) package DIST_DIR="$(DIST_DIR)"

.PHONY: package-lua
package-lua: package-rust ## Only the Lua front end (source, plus the library above)
	@$(MAKE) --no-print-directory -C $(LUA_DIR) package DIST_DIR="$(DIST_DIR)"

# One version across both implementations, checked before anything is built. A
# binary in ./dist stamped with a number nothing verified is the worst artifact
# a release can carry.
.PHONY: package-c
package-c: ## Only the C binary (the static library compiled in)
	@$(MAKE) --no-print-directory -C $(C_DIR) package DIST_DIR="$(DIST_DIR)"

.PHONY: package-versions
package-versions: version
	@echo "==> packaging $(RS_VERSION) into $(DIST_DIR)"

# Prove the shipped artifacts are the ones that were tested, not just that the
# source tree passes. Runs the parity checks against ./dist.
.PHONY: package-verify
package-verify: package ## Package, then run the parity checks against ./dist
	@echo "==> Verifying the packaged binaries"
	@CWB_RUST_BIN="$(DIST_DIR)/cwbwallet-rust" \
	 CWB_PYTHON_BIN="$(DIST_DIR)/cwbwallet-python" \
	 CWB_LUA_BIN="$(DIST_DIR)/cwbwallet-lua/cwbwallet-lua" \
	 CWB_C_BIN="$(DIST_DIR)/cwbwallet-c" \
	 ./scripts/parity.sh

# --------------------------------------------------------------------- builds

# Builds what the parity script needs to check all four. The C binary wants
# the release static library, which is why this is not only a debug build —
# and why leaving it out would let parity quietly cover less than it looks.
.PHONY: build
build: build-rust build-python ## Build every front end
	@$(MAKE) --no-print-directory -C $(RUST_DIR) ffi
	@$(MAKE) --no-print-directory -C $(C_DIR) build

.PHONY: build-rust
build-rust: ## Build the Rust workspace
	@$(MAKE) --no-print-directory -C $(RUST_DIR) build

.PHONY: build-python
build-python: ## Install the Python package into its virtualenv
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) install

.PHONY: build-lua
build-lua: ## Build the shared library the Lua front end loads
	@$(MAKE) --no-print-directory -C $(LUA_DIR) build

.PHONY: build-c
build-c: ## Build the C binary, static library and all
	@$(MAKE) --no-print-directory -C $(C_DIR) build

# --------------------------------------------------------------------- checks

.PHONY: format
format: ## Format and lint every front end
	@echo "==> Rust"
	@$(MAKE) --no-print-directory -C $(RUST_DIR) format
	@echo "==> Python"
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) format
	@echo "==> Lua"
	@$(MAKE) --no-print-directory -C $(LUA_DIR) format
	@echo "==> C"
	@$(MAKE) --no-print-directory -C $(C_DIR) format
	@echo
	@echo "  Formatted and linted every front end."

.PHONY: lint
lint: ## Lint every front end, changing nothing
	@$(MAKE) --no-print-directory -C $(RUST_DIR) lint
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) lint
	@$(MAKE) --no-print-directory -C $(LUA_DIR) lint
	@$(MAKE) --no-print-directory -C $(C_DIR) lint

# Formatting is checked separately from linting because the two tools disagree
# about what they are for: clippy and ruff-check find mistakes, rustfmt and
# ruff-format find diffs. CI runs both, so `check` has to as well — leaving it
# out is how a branch passes locally and fails on a wrapped line.
.PHONY: fmt-check
fmt-check: ## Fail if any front end is not formatted
	@$(MAKE) --no-print-directory -C $(RUST_DIR) fmt-check
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) fmt-check
	@$(MAKE) --no-print-directory -C $(LUA_DIR) fmt-check
	@$(MAKE) --no-print-directory -C $(C_DIR) fmt-check

.PHONY: fmt
fmt: format ## Alias for `format`

.PHONY: check
check: fmt-check lint test ## Everything CI would run (checks without rewriting)

# ----------------------------------------------------------------------- demos

.PHONY: demo
demo: build ## Create a throwaway wallet and show every CLI reading it
	@./scripts/parity.sh --verbose

.PHONY: tui-rust
tui-rust: ## Launch the Rust terminal UI
	@$(MAKE) --no-print-directory -C $(RUST_DIR) tui

.PHONY: tui-python
tui-python: ## Launch the Python terminal UI
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) tui

# ----------------------------------------------------------------------- clean

.PHONY: clean
clean: ## Remove build output from every front end and ./dist
	@$(MAKE) --no-print-directory -C $(RUST_DIR) clean
	@$(MAKE) --no-print-directory -C $(PYTHON_DIR) clean
	@$(MAKE) --no-print-directory -C $(LUA_DIR) clean
	@$(MAKE) --no-print-directory -C $(C_DIR) clean
	@rm -rf "$(DIST_DIR)"
