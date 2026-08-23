# Causewaybay Wallet — C CLI, statically linked
#
# One binary with the whole wallet inside it: no shared library to find at run
# time, nothing to install beside it. That is the difference from `luacli/`,
# which loads the cdylib at run time because that is what LÖVE needs.
#
# `make` lists the targets; `make test` builds and runs the suite.

CC      ?= cc
CARGO   ?= cargo
CFLAGS  ?= -std=c11 -O2 -Wall -Wextra -Wpedantic -Wshadow -Wconversion
LDFLAGS ?=

BIN     := cwbwallet-c
RUST_DIR := ../rustcli
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(RUST_DIR)/Cargo.toml | head -1)

# The static archive and the header, both produced by the Rust workspace.
FFI_LIB := $(RUST_DIR)/target/release/libcausewaybay_ffi.a
FFI_INC := $(RUST_DIR)/ffi/include

UNAME := $(shell uname -s)

# What a Rust staticlib needs from the system. The authoritative list comes from
# the toolchain rather than from a list written down here, because a hardcoded
# list is a guess that breaks on the next platform.
#
# Two things make asking fiddly, and both have already gone wrong:
#
#   * The answer is only printed while rustc is actually linking. So it is
#     asked for at link time, not at parse time — which also keeps `make help`
#     from building the workspace.
#   * It is a machine-readable answer, so it is asked for without colour. CI
#     sets CARGO_TERM_COLOR=always, which wraps the note in ANSI escapes; the
#     pattern then matched nothing and the link ran with *no* libraries on it.
#     That is undefined symbols on macOS, and on Linux a link that happens to
#     succeed because cc supplies libc anyway — a latent break waiting for a
#     platform that needs anything more.
#
# The `tr`/`sed` pair strips escapes even so, and the recipe refuses to link on
# an empty answer rather than emitting a command that cannot work.
NATIVE_LIBS_CMD = cd $(RUST_DIR) && CARGO_TERM_COLOR=never $(CARGO) rustc -p causewaybay-ffi \
	--release --crate-type staticlib -- --print native-static-libs 2>&1 \
	| sed -n 's/.*native-static-libs: //p' | head -1 \
	| tr -d '\033' | sed 's/\[[0-9;]*m//g'

# On macOS `-lc` and `-lSystem` name the same library, and cc links it itself
# anyway, so passing either makes the linker report a duplicate on every build.
# An exact-match filter, because `-lc` must not also strip `-lcrypto`.
FILTER_LIBS := cat
ifeq ($(UNAME),Darwin)
  FILTER_LIBS := grep -vx -e -lc -e -lSystem

  # The archive's prebuilt assembly (from `ring`) is compiled against the host
  # SDK, which is newer than the version cc targets by default. Matching them
  # is what stops the linker warning about it once per object file.
  export MACOSX_DEPLOYMENT_TARGET ?= $(shell sw_vers -productVersion)
endif

SOURCES := src/main.c src/json.c
OBJECTS := $(SOURCES:.c=.o)

DIST_DIR ?= $(abspath $(CURDIR)/../dist)
SIGN     := $(abspath $(CURDIR)/../scripts/codesign-binary.sh)

.DEFAULT_GOAL := help

.PHONY: help
help: ## Show this help
	@echo "Causewaybay Wallet — C ($(BIN) $(VERSION))"
	@echo
	@grep -hE '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "  Statically linked against $(FFI_LIB)."
	@echo

# The archive belongs to the Rust workspace, so ask it rather than assume a
# build has already happened.
#
# Phony on purpose. A file target with no prerequisites is only ever built when
# it is *missing*, so an archive built before a change to the Rust source would
# be linked in forever after — silently, since everything still compiles. Cargo
# is the thing that knows whether the archive is current, so it gets asked every
# time and answers in a fraction of a second when nothing moved.
.PHONY: ffi-lib
ffi-lib:
	@$(MAKE) --no-print-directory -C $(RUST_DIR) release

%.o: %.c
	$(CC) $(CFLAGS) -I$(FFI_INC) -c $< -o $@

.PHONY: build
build: $(BIN) ## Build the binary

$(BIN): $(OBJECTS) ffi-lib
	@libs="$$($(NATIVE_LIBS_CMD))"; \
	 if [ -z "$$libs" ]; then \
	   echo "$(BIN): cannot determine the system libraries a Rust staticlib needs." >&2; \
	   echo "  Linking without them produces a binary with undefined symbols, so" >&2; \
	   echo "  this stops here instead. Run the query by hand to see what it says:" >&2; \
	   echo "    cd $(RUST_DIR) && cargo rustc -p causewaybay-ffi --release \\" >&2; \
	   echo "      --crate-type staticlib -- --print native-static-libs" >&2; \
	   exit 1; \
	 fi; \
	 libs=$$(printf '%s\n' $$libs | $(FILTER_LIBS) | tr '\n' ' '); \
	 echo "==> linking $(BIN) ($(VERSION)) statically against$$(printf '%s' " $$libs")"; \
	 $(CC) $(OBJECTS) $(FFI_LIB) $$libs $(LDFLAGS) -o $(BIN)
	@echo "  built ./$(BIN) ($$(du -h $(BIN) | cut -f1 | tr -d ' '))"

.PHONY: test
test: build ## Build, then run the test suite
	@./tests/run.sh ./$(BIN)

# Proves the claim the whole directory rests on: the binary carries the wallet
# rather than reaching for a copy of it on disk.
.PHONY: verify-static
verify-static: build ## Check the binary has no Causewaybay library to load
	@./tests/verify-static.sh ./$(BIN)

.PHONY: lint
lint: ## Compile with warnings as errors, changing nothing
	@$(CC) $(CFLAGS) -Werror -I$(FFI_INC) -fsyntax-only $(SOURCES)
	@echo "  every C file compiles clean"

.PHONY: format
format: ## Format the source if clang-format is installed
	@command -v clang-format > /dev/null 2>&1 \
		&& clang-format -i $(SOURCES) src/json.h && echo "  formatted" \
		|| echo "  clang-format not installed; skipped"

.PHONY: fmt
fmt: format ## Alias for `format`

.PHONY: fmt-check
fmt-check: lint ## Fail if a file does not compile clean

.PHONY: check
check: lint test verify-static ## Everything CI would run

.PHONY: run
run: build ## Run it, e.g. make run ARGS="account list"
	@./$(BIN) $(ARGS)

.PHONY: package
package: build ## Copy the signed binary into $(DIST_DIR)
	@mkdir -p "$(DIST_DIR)"
	@cp -f $(BIN) "$(DIST_DIR)/$(BIN)"
	@chmod +x "$(DIST_DIR)/$(BIN)"
	@"$(SIGN)" "$(DIST_DIR)/$(BIN)"
	@echo "  built $(DIST_DIR)/$(BIN) ($$(du -h "$(DIST_DIR)/$(BIN)" | cut -f1 | tr -d ' '))"

.PHONY: demo
demo: build ## Create a throwaway wallet in ./.demo-home and show it off
	@rm -rf .demo-home
	@CAUSEWAYBAY_HOME=$$PWD/.demo-home ./$(BIN) account import-mnemonic \
		-m "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about" \
		--label demo
	@CAUSEWAYBAY_HOME=$$PWD/.demo-home ./$(BIN) account list

.PHONY: clean
clean: ## Remove build output and the demo home
	rm -f $(OBJECTS) $(BIN)
	rm -rf .demo-home
