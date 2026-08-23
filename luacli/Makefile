# Causewaybay Wallet — Lua CLI over the Rust core's C ABI
#
# `make` lists the targets; `make test` runs everything.
#
# There is nothing to compile here: the Lua front end is source, and the only
# build step is the shared library it loads, which belongs to the Rust
# workspace. Every target that needs it delegates to `make -C ../rustcli ffi`
# rather than assuming somebody remembered.

LUAJIT ?= luajit
RUST_DIR := ../rustcli

NAME    := cwbwallet-lua
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(RUST_DIR)/Cargo.toml | head -1)

UNAME := $(shell uname -s)
ifeq ($(UNAME),Darwin)
  FFI_LIB := libcausewaybay_ffi.dylib
else ifeq ($(OS),Windows_NT)
  FFI_LIB := causewaybay_ffi.dll
else
  FFI_LIB := libcausewaybay_ffi.so
endif

DIST_DIR ?= $(abspath $(CURDIR)/../dist)

# The library `make build` produces. Exported by the targets below so a test
# run loads exactly what was just built, rather than whatever the search order
# happens to find first.
FFI_PATH := $(abspath $(RUST_DIR)/target/debug/$(FFI_LIB))
export CAUSEWAYBAY_LIB = $(FFI_PATH)

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@echo "Causewaybay Wallet — Lua ($(NAME) $(VERSION))"
	@echo
	@grep -hE '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "  Needs LuaJIT — LÖVE embeds it, and the wallet reaches Rust through its ffi."
	@echo

# Fails with a usable message rather than a stack trace forty lines into a test.
.PHONY: require-luajit
require-luajit:
	@command -v $(LUAJIT) >/dev/null 2>&1 || { \
		echo "LuaJIT not found. Install it (brew install luajit) or set LUAJIT=." >&2; \
		exit 1; }

.PHONY: build
build: ## Build the shared library the Lua front end loads
	@$(MAKE) --no-print-directory -C $(RUST_DIR) ffi

.PHONY: test
test: require-luajit build ## Run every Lua test
	@$(LUAJIT) tests/init.lua

.PHONY: test-json
test-json: require-luajit ## Only the JSON codec tests (no library needed)
	@$(LUAJIT) tests/init.lua json

.PHONY: test-vectors
test-vectors: require-luajit build ## Only the shared-test-vector suite
	@$(LUAJIT) tests/init.lua vectors

.PHONY: lint
lint: require-luajit ## Byte-compile every file, which is the syntax check
	@status=0; \
	for file in cwbwallet.lua causewaybay/*.lua tests/*.lua; do \
		$(LUAJIT) -bl "$$file" /dev/null || { echo "  syntax error in $$file" >&2; status=1; }; \
	done; \
	if [ $$status -eq 0 ]; then echo "  every Lua file compiles"; fi; \
	exit $$status

# There is no formatter in the dependency-free budget this front end keeps
# (stylua is a Rust binary, luaformatter a C++ one). The style is held by
# review and by `lint` catching what is not even valid Lua.
.PHONY: format
format: lint ## Check the source compiles; there is no formatter to run

.PHONY: fmt
fmt: format ## Alias for `format`

.PHONY: fmt-check
fmt-check: lint ## Fail if a file does not compile

.PHONY: check
check: lint test ## Everything CI would run

.PHONY: run
run: build ## Run the CLI, e.g. make run ARGS="account list"
	@$(LUAJIT) cwbwallet.lua $(ARGS)

.PHONY: interactive
interactive: build ## Open the interactive menu
	@$(LUAJIT) cwbwallet.lua interactive

# The bundle carries its own copy of the shared library, so the directory can
# be moved anywhere and still work — no ./dist and no checkout beside it.
.PHONY: package
package: ## Stage the Lua front end and its library into $(DIST_DIR)
	@$(MAKE) --no-print-directory -C $(RUST_DIR) package DIST_DIR="$(DIST_DIR)"
	@mkdir -p "$(DIST_DIR)/$(NAME)"
	@cp -f cwbwallet.lua bin/cwbwallet-lua "$(DIST_DIR)/$(NAME)/"
	@cp -Rf causewaybay "$(DIST_DIR)/$(NAME)/"
	@cp -f "$(DIST_DIR)/$(FFI_LIB)" "$(DIST_DIR)/$(NAME)/$(FFI_LIB)"
	@chmod +x "$(DIST_DIR)/$(NAME)/cwbwallet-lua"
	@echo "  staged $(DIST_DIR)/$(NAME) (self-contained: carries $(FFI_LIB))"

.PHONY: demo
demo: build ## Create a throwaway wallet in ./.demo-home and show it off
	@rm -rf .demo-home
	@CAUSEWAYBAY_HOME=$$PWD/.demo-home $(LUAJIT) cwbwallet.lua account import-mnemonic \
		-m "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about" \
		--label demo
	@CAUSEWAYBAY_HOME=$$PWD/.demo-home $(LUAJIT) cwbwallet.lua account list

.PHONY: clean
clean: ## Remove the demo home
	rm -rf .demo-home
