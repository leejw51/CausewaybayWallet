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
	@# A glob, not a list: the hand-written list silently stopped covering
	@# boot.lua and login.lua when they were added, and a syntax error in
	@# login.lua reached a run because of it.
	@status=0; \
	for file in *.lua ui/*.lua tests/*.lua; do \
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
check: lint sfx-check test ## Everything CI would run

# Not in `check`: it copies the tree and runs the suite twenty-odd times, which
# is a thing to do when the tests change rather than on every build.
.PHONY: mutate
mutate: require-luajit build ## Break the code on purpose and check the tests notice
	@python3 tools/mutate.py $(ARGS)

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
	@# A glob, not a list. The hand-written one went stale twice without
	@# anybody noticing: it was still copying four files after `boot.lua` and
	@# `login.lua` were added, so every bundle `package` produced since the
	@# login screen existed was broken on startup. Nothing here is optional —
	@# if a `.lua` sits beside `main.lua`, the game needs it.
	@cp -R *.lua ui assets build-tmp/
	@cp -R $(LUA_DIR)/causewaybay build-tmp/
	@cd build-tmp && zip -qr9 "$(STAGE)/$(NAME).love" .
	@rm -rf build-tmp
	@$(MAKE) --no-print-directory verify-archive ARCHIVE="$(STAGE)/$(NAME).love"

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

# ------------------------------------------------------------------ macOS app

# A real double-clickable .app, with LÖVE inside it.
#
# `package` above produces a directory that needs LÖVE installed. This produces
# something you can hand to a person who has never heard of LÖVE: the runtime is
# downloaded and embedded, the shared library goes in Frameworks where a signed
# bundle keeps its nested binaries, the icon is built from the game's own logo,
# and the whole thing is signed and zipped.
#
# Two things about this are not optional and both are LuaJIT's fault:
#
#   * the hardened runtime needs `allow-jit` and
#     `allow-unsigned-executable-memory`, or the app dies the moment LuaJIT
#     compiles anything;
#   * `disable-library-validation`, because the wallet's own .dylib is not
#     signed by whoever signed LÖVE, and without it dlopen refuses to load it.
#
# Signing uses a Developer ID if the machine has one and falls back to ad-hoc,
# which is enough to run locally and not enough to hand to somebody else
# without them right-clicking Open.

LOVE_VERSION ?= 11.5
APP_NAME     := CausewaybayBank
BUNDLE_ID    := com.causewaybay.wallet
LOVE_URL     := https://github.com/love2d/love/releases/download/$(LOVE_VERSION)/love-$(LOVE_VERSION)-macos.zip
BUILD        := build
APP          := $(BUILD)/$(APP_NAME).app

.PHONY: app
app: build ## Build a double-clickable macOS .app with LÖVE embedded
	@# Single quotes, not backticks: sh would treat `make app` as a command
	@# substitution and re-enter this target on every platform that is not macOS.
	@test "$$(uname -s)" = "Darwin" || { echo "  'make app' is macOS only." >&2; exit 1; }
	@rm -rf "$(APP)" "$(BUILD)/love-macos" "$(BUILD)/$(APP_NAME)-macos.zip"
	@mkdir -p "$(BUILD)"

	@# The game, as one archive. Same contents as `package`, and for the same
	@# reason: the binding's Lua goes *inside* rather than being referenced at
	@# ../luacli, because a bundle that needs a checkout beside it is not one.
	@rm -rf "$(BUILD)/stage" && mkdir -p "$(BUILD)/stage"
	@cp -R *.lua ui assets "$(BUILD)/stage/"
	@cp -R $(LUA_DIR)/causewaybay "$(BUILD)/stage/"
	@cd "$(BUILD)/stage" && zip -qr9 "../game.love" .
	@rm -rf "$(BUILD)/stage"
	@$(MAKE) --no-print-directory verify-archive ARCHIVE="$(BUILD)/game.love"

	@echo "  fetching LÖVE $(LOVE_VERSION)"
	@curl -fsSL -o "$(BUILD)/love-macos.zip" "$(LOVE_URL)"
	@unzip -q -o "$(BUILD)/love-macos.zip" -d "$(BUILD)/love-macos"
	@mv "$(BUILD)/love-macos/love.app" "$(APP)"

	@cp "$(BUILD)/game.love" "$(APP)/Contents/Resources/game.love"
	@cp -f "$(RUST_DIR)/target/release/$(FFI_LIB)" "$(APP)/Contents/Frameworks/$(FFI_LIB)"

	@# Without these the bundle is still called "LÖVE" everywhere macOS shows a
	@# name, and two different games installed at once collide on the bundle id.
	@plutil -replace CFBundleName -string "Causewaybay Bank" "$(APP)/Contents/Info.plist"
	@plutil -replace CFBundleDisplayName -string "Causewaybay Bank" "$(APP)/Contents/Info.plist"
	@plutil -replace CFBundleIdentifier -string "$(BUNDLE_ID)" "$(APP)/Contents/Info.plist"
	@plutil -replace CFBundleShortVersionString -string "$(VERSION)" "$(APP)/Contents/Info.plist"
	@plutil -replace CFBundleVersion -string "$(VERSION)" "$(APP)/Contents/Info.plist"
	@# LÖVE ships this as an editable document type so a .love dropped on the
	@# icon runs. This bundle *is* one game and must not open somebody else's.
	@plutil -remove CFBundleDocumentTypes "$(APP)/Contents/Info.plist" 2>/dev/null || true
	@plutil -remove UTExportedTypeDeclarations "$(APP)/Contents/Info.plist" 2>/dev/null || true
	@plutil -replace CFBundleExecutable -string "love" "$(APP)/Contents/Info.plist"
	@plutil -replace NSHighResolutionCapable -bool true "$(APP)/Contents/Info.plist"

	@$(MAKE) --no-print-directory app-icon
	@$(MAKE) --no-print-directory app-sign

	@$(MAKE) --no-print-directory app-smoke

	@cd "$(BUILD)" && ditto -c -k --sequesterRsrc --keepParent "$(APP_NAME).app" "$(APP_NAME)-macos.zip"
	@rm -rf "$(BUILD)/love-macos" "$(BUILD)/love-macos.zip" "$(BUILD)/game.love"
	@echo
	@echo "  $(APP)"
	@du -sh "$(APP)" "$(BUILD)/$(APP_NAME)-macos.zip" | sed 's/^/    /'
	@echo
	@echo "  Open it with: open \"$(APP)\""

# Every `.lua` beside main.lua has to be in the archive.
#
# This is the check that would have caught both times the list went stale. A
# missing module is not a subtle failure — the bundle dies on the first
# `require` — but it dies on *somebody else's* machine, because the checkout
# it was built from has the file sitting right there and `make run` never
# touches the archive.
.PHONY: verify-archive
verify-archive:
	@missing=""; \
	for file in *.lua; do \
		unzip -l "$(ARCHIVE)" 2>/dev/null | grep -qE " $$file$$" || missing="$$missing $$file"; \
	done; \
	if [ -n "$$missing" ]; then \
		echo "  the archive is missing:$$missing" >&2; \
		echo "  (staging is a glob; if this fires, something excluded it)" >&2; \
		exit 1; \
	fi; \
	echo "  archive carries every module ($$(ls *.lua | wc -l | tr -d ' ') files)"

# The icon, from the game's own logo — the vault sprite, which is already the
# thing on the title card. `sips` refuses to upscale cleanly, so the source is
# padded to a square first and every size is generated from that.
# Start the thing that was built and see whether it draws.
#
# The archive check above proves every file is present; this proves they add
# up to a program. It is the only test in here that runs the *bundle* rather
# than the checkout, and the difference is the whole point: `make run` loads
# modules from the working tree, where everything always exists.
#
# A missing module dies on the first `require`, which is instant — so if no
# frame has been captured after twenty seconds, it did not start.
.PHONY: app-smoke
app-smoke:
	@test -d "$(APP)" || { echo "  no $(APP); run 'make app'" >&2; exit 1; }
	@rm -f "$(BUILD)/smoke.png"
	@CWB_SHOT="$(abspath $(BUILD))/smoke.png" CWB_SHOT_AFTER=2 \
		CAUSEWAYBAY_HOME="$(abspath $(BUILD))/smoke-home" \
		"$(APP)/Contents/MacOS/love" > "$(BUILD)/smoke.log" 2>&1 & \
	pid=$$!; \
	for _ in $$(seq 1 40); do \
		[ -f "$(BUILD)/smoke.png" ] && break; \
		sleep 0.5; \
	done; \
	kill $$pid 2>/dev/null || true; \
	wait $$pid 2>/dev/null || true; \
	if [ ! -f "$(BUILD)/smoke.png" ]; then \
		echo "  the bundle did not draw a frame:" >&2; \
		grep -v CapsLock "$(BUILD)/smoke.log" | head -20 >&2; \
		exit 1; \
	fi
	@rm -rf "$(BUILD)/smoke-home" "$(BUILD)/smoke.log"
	@echo "  the bundle starts and draws"

.PHONY: app-icon
app-icon:
	@test -f assets/logo.png || { echo "  no assets/logo.png; keeping LÖVE's icon"; exit 0; }
	@rm -rf "$(BUILD)/icon.iconset" && mkdir -p "$(BUILD)/icon.iconset"
	@for size in 16 32 64 128 256 512 1024; do \
		sips -s format png -z $$size $$size assets/logo.png \
			--out "$(BUILD)/icon.iconset/icon_$${size}x$${size}.png" > /dev/null 2>&1; \
	done
	@for size in 16 32 128 256 512; do \
		double=$$((size * 2)); \
		cp "$(BUILD)/icon.iconset/icon_$${double}x$${double}.png" \
			"$(BUILD)/icon.iconset/icon_$${size}x$${size}@2x.png" 2>/dev/null || true; \
	done
	@rm -f "$(BUILD)/icon.iconset/icon_1024x1024.png"
	@iconutil -c icns "$(BUILD)/icon.iconset" -o "$(BUILD)/icon.icns"
	@cp "$(BUILD)/icon.icns" "$(APP)/Contents/Resources/OS X AppIcon.icns" 2>/dev/null || true
	@cp "$(BUILD)/icon.icns" "$(APP)/Contents/Resources/Love.icns" 2>/dev/null || true
	@rm -rf "$(BUILD)/icon.iconset" "$(BUILD)/icon.icns"
	@echo "  icon from assets/logo.png"

# Signed inside-out: nested binaries first, then the executable, then the
# bundle. Signing the outer bundle first invalidates itself the moment anything
# inside it is signed afterwards.
#
# Every stage gets the entitlements, which follows bfvegas — a LÖVE game that
# is signed, notarised and shipping. Apple's own guidance is that entitlements
# belong on the main executable and putting them on a framework does nothing at
# run time, so this is redundant rather than wrong; it is kept because matching
# a recipe known to pass notarisation is worth more than tidiness in a place
# where the failures are opaque and arrive days later.
.PHONY: app-sign
app-sign:
	@test -d "$(APP)" || { echo "  no $(APP); run \`make app\`" >&2; exit 1; }
	@rm -f "$(BUILD)/entitlements.plist"
	@printf '%s\n' \
		'<?xml version="1.0" encoding="UTF-8"?>' \
		'<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
		'<plist version="1.0"><dict>' \
		'  <key>com.apple.security.cs.allow-jit</key><true/>' \
		'  <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>' \
		'  <key>com.apple.security.cs.disable-library-validation</key><true/>' \
		'</dict></plist>' \
		> "$(BUILD)/entitlements.plist"
	@# `APPLE_SIGNING_IDENTITY` wins when it is set, because that is what the
	@# release workflow exports after importing the certificate into a keychain
	@# it made for the job. Falling back to a keychain search is for a laptop.
	@identity="$${APPLE_SIGNING_IDENTITY:-}"; \
	if [ -z "$$identity" ] || [ "$$identity" = "-" ]; then \
		identity=$$(security find-identity -v -p codesigning 2>/dev/null \
			| grep "Developer ID Application" | head -1 | awk -F'"' '{print $$2}'); \
	fi; \
	if [ -z "$$identity" ]; then \
		identity="-"; \
		echo "  no Developer ID found - signing ad-hoc (runs here, not elsewhere)"; \
	else \
		echo "  signing as $$identity"; \
	fi; \
	stamp=--timestamp; \
	[ "$$identity" = "-" ] && stamp=""; \
	find "$(APP)/Contents/Frameworks" -type f 2>/dev/null | while read -r file; do \
		if file "$$file" | grep -q "Mach-O"; then \
			codesign --force --options runtime --sign "$$identity" \
				--entitlements "$(BUILD)/entitlements.plist" $$stamp "$$file" || exit 1; \
		fi; \
	done; \
	find "$(APP)/Contents/Frameworks" -maxdepth 1 -name "*.framework" 2>/dev/null | while read -r bundle; do \
		codesign --force --options runtime --sign "$$identity" \
			--entitlements "$(BUILD)/entitlements.plist" $$stamp "$$bundle" || exit 1; \
	done; \
	codesign --force --options runtime --sign "$$identity" \
		--entitlements "$(BUILD)/entitlements.plist" $$stamp \
		"$(APP)/Contents/MacOS/love" || exit 1; \
	codesign --force --options runtime --sign "$$identity" \
		--entitlements "$(BUILD)/entitlements.plist" $$stamp "$(APP)" || exit 1
	@rm -f "$(BUILD)/entitlements.plist"
	@codesign --verify --deep --strict "$(APP)" && echo "  signature verifies"

# ---------------------------------------------------------------- notarize

# Signing is not enough on its own, and this is the part that finishes it.
#
# A Developer ID signature with the hardened runtime and a secure timestamp is
# necessary and not sufficient: since macOS 10.15 anything distributed outside
# the App Store must also be notarised by Apple. Without it, a *downloaded*
# copy is refused —
#
#     spctl -a -t open --context context:primary-signature CausewaybayBank.app
#     rejected  (source=Unnotarized Developer ID)
#
# — and the person who downloaded it gets "Apple could not verify…".
#
# Not part of `make app`, for two reasons: it needs credentials this repository
# must not carry, and it uploads the bundle to Apple and waits, which is not
# something a build should do without being asked.
#
# The variables are `scripts/codesign-binary.sh`'s, because that script already
# notarises the four CLI binaries in this repository and a second set of names
# for the same three secrets is a trap. The release workflow exports them from
# `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD` and `TEAM_ID`, which is how bfvegas
# and bfagent name them too — so a machine set up for either already works.
#
#     export APPLE_ID=you@example.com
#     export APPLE_PASSWORD=abcd-efgh-ijkl-mnop   # app-specific
#     export APPLE_TEAM_ID=ABCDE12345
#     make notarize
.PHONY: notarize
notarize: ## Notarize and staple the built .app (needs Apple credentials)
	@test -d "$(APP)" || { echo "  no $(APP); run 'make app' first" >&2; exit 1; }
	@for name in APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do \
		eval "value=\$$$$name"; \
		[ -n "$$value" ] || { \
			echo "  $$name is not set." >&2; \
			echo "  Notarising needs an Apple ID, an app-specific password" >&2; \
			echo "  from appleid.apple.com, and the team id. None is kept here." >&2; \
			exit 1; }; \
	done

	@# Ad-hoc cannot be notarised, and finding that out from Apple takes
	@# minutes. The signature says which it is, so ask it first.
	@codesign -dv --verbose=2 "$(APP)" 2>&1 | grep -q "Authority=Developer ID Application" || { 		echo "  $(APP) is not signed with a Developer ID." >&2; 		echo "  Apple will refuse an ad-hoc signature; there is nothing to submit." >&2; 		exit 1; }

	@echo "  submitting to Apple - this takes minutes, not seconds"
	@ditto -c -k --sequesterRsrc --keepParent "$(APP)" "$(BUILD)/notarize.zip"
	@xcrun notarytool submit "$(BUILD)/notarize.zip" \
		--apple-id "$$APPLE_ID" \
		--password "$$APPLE_PASSWORD" \
		--team-id "$$APPLE_TEAM_ID" \
		--wait --timeout 20m
	@rm -f "$(BUILD)/notarize.zip"

	@# Stapled, so the ticket travels with the bundle and Gatekeeper can
	@# check it with no network at all.
	@xcrun stapler staple "$(APP)"
	@xcrun stapler validate "$(APP)"

	@# The distribution zip was made before any of this and carries an
	@# unstapled copy. Rebuilt, or the download is exactly what it was.
	@rm -f "$(BUILD)/$(APP_NAME)-macos.zip"
	@cd "$(BUILD)" && ditto -c -k --sequesterRsrc --keepParent \
		"$(APP_NAME).app" "$(APP_NAME)-macos.zip"

	@$(MAKE) --no-print-directory gatekeeper

# What a person who downloads it will actually get.
#
# `--context context:primary-signature` is the one that matters: plain
# `spctl -a` accepts an un-notarised Developer ID bundle, and Finder does not.
# Asking the wrong question is how "it is signed" gets mistaken for "it opens".
.PHONY: gatekeeper
gatekeeper: ## Assess the bundle the way Finder does
	@test -d "$(APP)" || { echo "  no $(APP); run 'make app' first" >&2; exit 1; }
	@codesign --verify --deep --strict "$(APP)" && echo "  signature: valid"
	@if xcrun stapler validate "$(APP)" > /dev/null 2>&1; then 		echo "  notarised: yes, and stapled"; 	else 		echo "  notarised: NO - a downloaded copy will be blocked"; 	fi
	@spctl -a -vvv -t open --context context:primary-signature "$(APP)" 2>&1 		| sed 's/^/  /' || true

# --------------------------------------------------------------------- extras

# Stand-ins for what a person would type. The mnemonic is BIP-39's published
# all-zeros test vector — the one in every implementation's fixtures — so it is
# not a secret and never was.
SHOT_ADDRESS := 0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0
SHOT_WORDS   := abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about

# The GUI's equivalent of a test for the part tests cannot reach. Drawing can
# only be checked by looking, so this makes a frame reviewable from a terminal.
.PHONY: shots
shots: require-love build ## Write a PNG of each screen into ./shots
	@mkdir -p shots
	@cd .. && CWB_SHOT="$(CURDIR)/shots/wallets.png" CWB_SHOT_AFTER=2.4 $(LOVE) lovegui
	@cd .. && CWB_SHOT="$(CURDIR)/shots/send.png" CWB_SHOT_AFTER=2.6 CWB_SHOT_SCREEN=send \
		CWB_SHOT_KEYS="type:$(SHOT_ADDRESS),tab,type:0.5" $(LOVE) lovegui
	@cd .. && CWB_SHOT="$(CURDIR)/shots/network.png" CWB_SHOT_AFTER=2.4 \
		CWB_SHOT_SCREEN=network $(LOVE) lovegui
	@# The boot sequence and its title card — reachable now that the harness can
	@# be told to stay on it rather than dismissing it first.
	@cd .. && CWB_SHOT="$(CURDIR)/shots/boot.png" CWB_SHOT_AFTER=1.4 \
		CWB_SHOT_SCREEN=boot $(LOVE) lovegui
	@cd .. && CWB_SHOT="$(CURDIR)/shots/title.png" CWB_SHOT_AFTER=3.2 \
		CWB_SHOT_SCREEN=boot $(LOVE) lovegui
	@# Its own throwaway home. Unlocking with a phrase the store does not know
	@# *imports* it, so a login shot against the real home would quietly add an
	@# account to somebody's wallet every time this ran.
	@cd .. && CAUSEWAYBAY_HOME="$(CURDIR)/shots/.home" \
		CWB_SHOT="$(CURDIR)/shots/login.png" CWB_SHOT_AFTER=1.8 \
		CWB_SHOT_SCREEN=login CWB_SHOT_KEYS="type:$(SHOT_WORDS)" $(LOVE) lovegui
	@# Two frames of the flight rather than one, because the whole point of
	@# expo_in is that the first half and the second half look nothing alike.
	@cd .. && CWB_SHOT="$(CURDIR)/shots/launch-ignition.png" CWB_SHOT_AFTER=0.6 \
		CWB_SHOT_SCREEN=send CWB_SHOT_KEYS="type:$(SHOT_ADDRESS),tab,type:0.5,launch" $(LOVE) lovegui
	@cd .. && CWB_SHOT="$(CURDIR)/shots/launch-away.png" CWB_SHOT_AFTER=1.5 \
		CWB_SHOT_SCREEN=send CWB_SHOT_KEYS="type:$(SHOT_ADDRESS),tab,type:0.5,launch" $(LOVE) lovegui
	@ls -1 shots | sed 's/^/  /'

# Redrawing the art. Needs an API key; running the game does not, because the
# results are committed.
.PHONY: assets
assets: ## Redraw any missing sprites with Grok (needs XAI_API_KEY)
	@python3 tools/generate-assets.py

.PHONY: assets-all
assets-all: ## Redraw every sprite, replacing what is committed
	@python3 tools/generate-assets.py logo wallet coin rocket globe key skull spark

# The sounds, unlike the art, need nothing but Python — they are square waves
# and a shift register, so this is safe to run at any time by anyone.
.PHONY: sfx
sfx: ## Re-synthesise the sound effects (no API key, no network)
	@python3 tools/generate-sfx.py

# Checked for shape rather than for bytes. Byte-comparing against a fresh
# render would be the stronger test and is the wrong one to run in CI: the
# waveforms come out of libm, and a sine that differs in its last bit between
# macOS and Linux would fail a build over something no ear could detect. What
# can go wrong and matters is a file that is missing, truncated, silent, or at
# the wrong sample rate — so that is what this asserts.
.PHONY: sfx-check
sfx-check: ## Verify every committed sound is present and well-formed
	@python3 tools/check-sfx.py

.PHONY: clean
clean: ## Remove screenshots, the app bundle, and staged output
	rm -rf shots build build-tmp
