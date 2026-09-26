# Development tasks for safe-rm. Run `make` with no arguments to list the targets.
#
# Tool versions are pinned in mise.toml. When mise is available, every tool runs through
# `mise exec --`, so the pinned versions are used even when mise is not activated in the shell
# (for example when make is started from an IDE or a GUI). SYSTEM_TOOLS=1 uses the tools on PATH
# instead (the versions are then not guaranteed).
#
# Only GNU Make 3.81 features are used (the make that ships with macOS):
# no .ONESHELL, .SHELLFLAGS, $(file ...) or !=.

.DEFAULT_GOAL := help

BINARY_NAME := safe-rm
INSTALL_PATH ?= /usr/local/bin
# Cargo.lock is committed, so resolve dependencies exactly as CI does. release (and install through
# it) must keep this default; tests/project_configuration_test.rs checks it
CARGO_FLAGS ?= --locked

# ---- Toolchain ------------------------------------------------------------------
# Look for mise on PATH, then in the usual install locations (make started from a GUI may not
# inherit the shell's PATH). Override with make MISE=/path/to/mise.
# To try the behavior without mise, empty the candidates with MISE_CANDIDATES=.
MISE_CANDIDATES ?= $(HOME)/.local/bin/mise /opt/homebrew/bin/mise /usr/local/bin/mise
ifeq ($(SYSTEM_TOOLS),1)
RUN :=
else
ifndef MISE
MISE := $(firstword $(shell command -v mise 2>/dev/null) $(wildcard $(MISE_CANDIDATES)))
endif
ifeq ($(MISE),)
ifneq ($(filter-out help,$(or $(MAKECMDGOALS),help)),)
$(error mise was not found. Install it from https://mise.jdx.dev, or add SYSTEM_TOOLS=1 to use the tools on PATH)
endif
endif
RUN := $(if $(MISE),$(MISE) exec --,)
endif

.PHONY: help setup build release test test-unit test-integration lint fmt fmt-check check ci install install-hooks uninstall clean

## Setup

setup: ## Install the toolchain (mise) and dependencies
	@if [ -n "$(MISE)" ]; then "$(MISE)" install; fi
	$(RUN) cargo fetch $(CARGO_FLAGS)

## Build

build: ## Build a debug binary
	$(RUN) cargo build $(CARGO_FLAGS)

release: ## Build a release binary
	$(RUN) cargo build --release $(CARGO_FLAGS)

## Checks

test: ## Run the tests
	$(RUN) cargo test $(CARGO_FLAGS)

test-unit: ## Run only the library unit tests (cargo test --lib)
	$(RUN) cargo test $(CARGO_FLAGS) --lib

test-integration: ## Run only the integration tests (tests/integration_test.rs)
	$(RUN) cargo test $(CARGO_FLAGS) --test integration_test

lint: ## Run clippy with warnings as errors
	$(RUN) cargo clippy $(CARGO_FLAGS) --all-targets -- -D warnings

fmt: ## Format the code (rewrites files)
	$(RUN) cargo fmt --all

fmt-check: ## Check the formatting (no changes)
	$(RUN) cargo fmt --all -- --check

check: fmt-check lint ## Run fmt-check and lint (no changes)

ci: check test ## Run the same checks as CI (no changes)

## Install

# Replace the binary through a temporary file and a rename instead of copying over it. macOS
# caches the code signature check per inode, so a binary copied over one that is running (or ran
# a moment ago) is killed with SIGKILL right after it starts (exit 137). The temporary file sits
# in the same directory so that the rename swaps the inode.
install: release ## Install the release binary to INSTALL_PATH (default /usr/local/bin)
	@mkdir -p "$(INSTALL_PATH)"
	cp "target/release/$(BINARY_NAME)" "$(INSTALL_PATH)/$(BINARY_NAME).new"
	mv -f "$(INSTALL_PATH)/$(BINARY_NAME).new" "$(INSTALL_PATH)/$(BINARY_NAME)"

# Only prints the steps. The hook itself and the CLAUDE.md rules are in README.md
# (Claude Code Integration), so there is a single copy to keep up to date
install-hooks: ## Show how to set up the Claude Code integration
	@echo "Claude Code integration"
	@echo ""
	@echo "1. Add the PreToolUse hook from README.md (Claude Code Integration > Hook Configuration)"
	@echo "   to the Claude Code settings.json (user or project)"
	@echo ""
	@echo "2. Add the file deletion rules from README.md (Claude Code Integration > CLAUDE.md Instructions)"
	@echo "   to CLAUDE.md"
	@echo ""
	@echo '3. Allow the command: claude /permissions add Bash "safe-rm*"'

uninstall: ## Remove the binary from INSTALL_PATH
	rm -f "$(INSTALL_PATH)/$(BINARY_NAME)"

clean: ## Remove build artifacts
	$(RUN) cargo clean

## Help

help: ## Show this help
	@echo "Development tasks for $(BINARY_NAME)"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-17s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Tool versions are pinned in mise.toml. Run make setup first."
	@echo "Release: GitHub Actions > Release > Run workflow"
