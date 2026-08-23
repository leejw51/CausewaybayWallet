# Causewaybay Wallet — LÖVE GUI
#
#   make          show this help
#   make build    build the release shared library the game loads
#   make run      open the window
#   make test     run the headless suite
#   make package  build a distributable bundle into ../dist
#
# A window over the same wallet the CLI drives. LÖVE embeds LuaJIT, so the
# binding in ../luacli loads here unchanged — there is no cryptography, no
# store and no argument parsing in this directory.

LOVE   ?= love
LUAJIT ?= luajit

NAME     := cwbwallet-gui
RUST_DIR := ../rustcli
LUA_DIR  := ../luacli
VERSION  := $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(RUST_DIR)/Cargo.toml | head -1)

# The shared library the binding loads at run time. Named per platform, the
# same way ../luacli/Makefile names it.
UNAME := $(shell uname -s)
ifeq ($(UNAME),Darwin)
  FFI_LIB := libcausewaybay_ffi.dylib
else ifeq ($(OS),Windows_NT)
  FFI_LIB := causewaybay_ffi.dll
else
  FFI_LIB := libcausewaybay_ffi.so
endif

DIST_DIR ?= $(abspath $(CURDIR)/../dist)
STAGE    := $(DIST_DIR)/$(NAME)
SIGN     := $(abspath $(CURDIR)/../scripts/codesign-binary.sh)

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@echo "Causewaybay Wallet — LÖVE GUI ($(VERSION))"
	@echo
	@grep -hE '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "  Needs LÖVE 11 (brew install love). The art in assets/ is committed,"
	@echo "  so no API key is needed to run it."
	@echo

# Fail with a usable message rather than a stack trace three targets deep.
.PHONY: require-love
require-love:
	@command -v $(LOVE) > /dev/null 2>&1 || { \
		echo "LÖVE not found. Install it (brew install love) or set LOVE=." >&2; \
		exit 1; }

.PHONY: require-luajit
require-luajit:
	@command -v $(LUAJIT) > /dev/null 2>&1 || { \
		echo "LuaJIT not found. Install it (brew install luajit) or set LUAJIT=." >&2; \
		exit 1; }

# ---------------------------------------------------------------------- build

# Release rather than debug, because this is what `package` ships and what
# `run` should behave like. Cargo decides whether anything actually needs
# rebuilding, so asking every time costs a fraction of a second.
.PHONY: build
build: ## Build the release shared library the game loads
	@$(MAKE) --no-print-directory -C $(RUST_DIR) release
	@echo "  ready: $(RUST_DIR)/target/release/$(FFI_LIB)"

# ------------------------------------------------------------------------ run

# CAUSEWAYBAY_LIB is set so the release library is loaded rather than whichever
# build the binding's search happens to find first — `make run` and
# `make package` should be running the same code.
.PHONY: run
run: require-love build ## Open the window
	@cd .. && CAUSEWAYBAY_LIB="$(abspath $(RUST_DIR)/target/release/$(FFI_LIB))" \
		$(LOVE) lovegui $(ARGS)

# ----------------------------------------------------------------------- test

.PHONY: test
test: require-luajit build ## Run the headless suite (no LÖVE needed)
	@$(LUAJIT) tests/init.lua

.PHONY: lint
lint: require-luajit ## Byte-compile every file, which is the syntax check
	@status=0; \
	for file in main.lua conf.lua model.lua worker.lua ui/*.lua tests/*.lua; do \
		$(LUAJIT) -bl "$$file" /dev/null || { echo "  syntax error in $$file" >&2; status=1; }; \
	done; \
	if [ $$status -eq 0 ]; then echo "  every Lua file compiles"; fi; \
	exit $$status

.PHONY: format
format: lint ## Check the source compiles; there is no formatter to run

.PHONY: fmt
fmt: format ## Alias for `format`

.PHONY: fmt-check
fmt-check: lint ## Fail if a file does not compile

.PHONY: check
check: lint test ## Everything CI would run

# -------------------------------------------------------------------- package

# What ships: a .love archive, the shared library beside it, and a launcher.
#
# Not a fused .app. A fused build hides the library inside a bundle where the
# binding cannot reach it without an environment variable a double-click cannot
# set — so the launcher sets it instead, and the result is a directory that can
# be copied anywhere and run. That is the same shape ../luacli packages to, for
# the same reason.
#
# The binding's Lua modules are copied *into* the archive rather than referenced
# at ../luacli, because a bundle that needs a checkout beside it is not a
# bundle. LÖVE's own package loader finds them inside the archive; the
# `../luacli` entry main.lua adds is simply absent there and harmlessly skipped.
.PHONY: package
package: build ## Build a distributable bundle into $(DIST_DIR)
	@rm -rf "$(STAGE)" "$(DIST_DIR)/$(NAME).love"
	@mkdir -p "$(STAGE)"

	@# Stage the game, without the parts a player has no use for.
	@rm -rf build-tmp && mkdir -p build-tmp
	@cp -R main.lua conf.lua model.lua worker.lua ui assets build-tmp/
	@cp -R $(LUA_DIR)/causewaybay build-tmp/
	@cd build-tmp && zip -qr9 "$(STAGE)/$(NAME).love" .
	@rm -rf build-tmp

	@cp -f "$(RUST_DIR)/target/release/$(FFI_LIB)" "$(STAGE)/$(FFI_LIB)"
	@"$(SIGN)" "$(STAGE)/$(FFI_LIB)" 2>/dev/null || true

	@printf '%s\n' \
		'#!/bin/sh' \
		'# Causewaybay Wallet — LÖVE GUI.' \
		'#' \
		'# The binding loads the shared library by path, and a double-clicked' \
		'# bundle has no environment to inherit one from, so it is set here' \
		'# relative to this script. Everything else is in the .love beside it.' \
		'set -eu' \
		'here=$$(CDPATH= cd -- "$$(dirname -- "$$0")" && pwd)' \
		'export CAUSEWAYBAY_LIB="$$here/$(FFI_LIB)"' \
		'exec "$${LOVE:-love}" "$$here/$(NAME).love" "$$@"' \
		> "$(STAGE)/$(NAME)"
	@chmod +x "$(STAGE)/$(NAME)"

	@echo "  staged $(STAGE)"
	@ls -1lh "$(STAGE)" | tail -n +2 | awk '{printf "    %-30s %s\n", $$9, $$5}'
	@echo
	@echo "  Needs LÖVE 11 on the machine that runs it: $(STAGE)/$(NAME)"

# --------------------------------------------------------------------- extras

# The GUI's equivalent of a test for the part tests cannot reach. Drawing can
# only be checked by looking, so this makes a frame reviewable from a terminal.
.PHONY: shots
shots: require-love build ## Write a PNG of each screen into ./shots
	@mkdir -p shots
	@cd .. && CWB_SHOT="$(CURDIR)/shots/wallets.png" CWB_SHOT_AFTER=2.4 $(LOVE) lovegui
	@cd .. && CWB_SHOT="$(CURDIR)/shots/send.png" CWB_SHOT_AFTER=2.6 CWB_SHOT_SCREEN=send \
		CWB_SHOT_KEYS="type:0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0,tab,type:0.5" $(LOVE) lovegui
	@cd .. && CWB_SHOT="$(CURDIR)/shots/network.png" CWB_SHOT_AFTER=2.4 \
		CWB_SHOT_SCREEN=network $(LOVE) lovegui
	@ls -1 shots | sed 's/^/  /'

# Redrawing the art. Needs an API key; running the game does not, because the
# results are committed.
.PHONY: assets
assets: ## Redraw any missing sprites with Grok (needs XAI_API_KEY)
	@python3 tools/generate-assets.py

.PHONY: assets-all
assets-all: ## Redraw every sprite, replacing what is committed
	@python3 tools/generate-assets.py logo wallet coin rocket globe key skull spark

.PHONY: clean
clean: ## Remove screenshots and staged output
	rm -rf shots build-tmp
