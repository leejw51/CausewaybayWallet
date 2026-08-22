# Causewaybay Wallet — Python CLI/TUI
#
# `make` lists the targets; `make test` runs everything.
# All targets bootstrap ./.venv on first use.

VENV   := .venv
PYTHON := $(VENV)/bin/python
PIP    := $(VENV)/bin/pip
PYTEST := $(VENV)/bin/pytest
RUFF   := $(VENV)/bin/ruff
BASE_PYTHON ?= python3
CARGO  ?= cargo

NAME    := cwbwallet-python
MODULE  := causewaybay
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' pyproject.toml | head -1)

# Overridden by the root Makefile so both implementations agree on one output
# directory; the default keeps `make -C pythoncli package` working on its own.
DIST_DIR   ?= $(abspath $(CURDIR)/../dist)
BUILD_DIR  := build
WHEEL_DIR  := $(BUILD_DIR)/wheel
PYAPP_ROOT := $(BUILD_DIR)/pyapp
SIGN       := $(abspath $(CURDIR)/../scripts/codesign-binary.sh)

# Pinned: PyApp is pre-1.0 and this Makefile speaks its environment-variable
# contract, which is exactly the kind of thing a minor release may reshape.
PYAPP_VERSION ?= 0.29.0
# The interpreter that gets embedded. Not this machine's python3 —
# python-build-standalone's redistributable build is what makes the result
# portable.
PYAPP_PYTHON_VERSION ?= 3.12

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@echo "Causewaybay Wallet — Python (cwbwallet)"
	@echo
	@grep -hE '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
	@echo

$(PYTHON):
	$(BASE_PYTHON) -m venv $(VENV)
	$(PIP) install --quiet --upgrade pip

.PHONY: venv
venv: $(PYTHON) ## Create the virtualenv

.PHONY: install
install: $(PYTHON) ## Install runtime and development dependencies
	$(PIP) install --quiet -r requirements-dev.txt
	$(PIP) install --quiet -e .

.PHONY: test
test: install ## Run the test suite
	$(PYTEST)

.PHONY: test-verbose
test-verbose: install ## Run the test suite with per-test output
	$(PYTEST) -vv

.PHONY: coverage
coverage: install ## Run the tests with a coverage report
	$(PYTEST) --cov=causewaybay --cov-report=term-missing

.PHONY: test-vectors
test-vectors: install ## Run only the shared-test-vector suite
	$(PYTEST) tests/test_vectors.py

.PHONY: format
format: install ## Format the source, then lint what formatting cannot fix
	@echo "==> ruff format"
	@$(RUFF) format causewaybay tests
	@echo "==> ruff check --fix"
	@$(RUFF) check --fix causewaybay tests
	@# A second pass with no --fix, so anything ruff could not repair still fails.
	@$(RUFF) check causewaybay tests

.PHONY: lint
lint: install ## Lint with ruff, changing nothing
	$(RUFF) check causewaybay tests

.PHONY: fmt
fmt: format ## Alias for `format`

.PHONY: fmt-check
fmt-check: install ## Fail if the source is not formatted
	$(RUFF) format --check causewaybay tests

.PHONY: check
check: lint test ## Everything CI would run

.PHONY: run
run: install ## Run the CLI, e.g. make run ARGS="account list"
	$(PYTHON) -m causewaybay $(ARGS)

.PHONY: tui
tui: install ## Launch the terminal UI
	$(PYTHON) -m causewaybay tui

.PHONY: package
package: wheel ## Build the standalone binary into $(DIST_DIR)
	@$(CARGO) --version >/dev/null 2>&1 \
		|| { echo "ERROR: cargo not found — PyApp is built with it. https://rustup.rs"; exit 1; }
	@echo "==> pyapp $(PYAPP_VERSION) wrapping $(MODULE) $(VERSION)"
	@wheel=$$(ls "$(WHEEL_DIR)"/causewaybay_wallet-*.whl 2>/dev/null | head -1); \
	 test -n "$$wheel" || { echo "ERROR: no wheel in $(WHEEL_DIR)"; exit 1; }; \
	 PYAPP_PROJECT_PATH="$$(cd "$$(dirname "$$wheel")" && pwd)/$$(basename "$$wheel")" \
	 PYAPP_EXEC_MODULE="$(MODULE)" \
	 PYAPP_PYTHON_VERSION="$(PYAPP_PYTHON_VERSION)" \
	 PYAPP_DISTRIBUTION_EMBED=1 \
	 PYAPP_FULL_ISOLATION=1 \
	 $(CARGO) install pyapp \
		--version $(PYAPP_VERSION) --locked --force --quiet \
		--root "$(PYAPP_ROOT)"
	@mkdir -p "$(DIST_DIR)"
	@cp -f "$(PYAPP_ROOT)/bin/pyapp" "$(DIST_DIR)/$(NAME)"
	@chmod +x "$(DIST_DIR)/$(NAME)"
	@"$(SIGN)" "$(DIST_DIR)/$(NAME)"
	@echo "  built $(DIST_DIR)/$(NAME) ($$(du -h "$(DIST_DIR)/$(NAME)" | cut -f1 | tr -d ' '))"

.PHONY: wheel
wheel: install ## Build the wheel PyApp embeds
	@echo "==> wheel causewaybay-wallet $(VERSION)"
	@# Rebuilt from scratch: a stale wheel of an older version would still match
	@# the glob above and get embedded in preference to nothing.
	@rm -rf "$(WHEEL_DIR)"
	@$(PYTHON) -m pip wheel --no-deps --quiet --wheel-dir "$(WHEEL_DIR)" .

.PHONY: demo
demo: install ## Create a throwaway wallet in ./.demo-home and show it off
	@rm -rf .demo-home
	@CAUSEWAYBAY_HOME=$$PWD/.demo-home $(PYTHON) -m causewaybay account import-mnemonic \
		-m "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about" \
		--label demo
	@CAUSEWAYBAY_HOME=$$PWD/.demo-home $(PYTHON) -m causewaybay account list
	@echo
	@echo "Store contents:"
	@cat .demo-home/accounts.jsonl

.PHONY: clean
clean: ## Remove caches, build output and the demo home
	rm -rf .pytest_cache .ruff_cache .coverage htmlcov $(BUILD_DIR) dist *.egg-info .demo-home
	find . -name __pycache__ -type d -prune -exec rm -rf {} +

.PHONY: distclean
distclean: clean ## Also remove the virtualenv
	rm -rf $(VENV)
