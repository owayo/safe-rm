.PHONY: build release install install-hooks clean test fmt check help

# Default target
.DEFAULT_GOAL := help

# Variables
BINARY_NAME := safe-rm
INSTALL_PATH := /usr/local/bin

## Build Commands

build: ## Build debug version
	cargo build

release: ## Build release version
	cargo build --release

## Installation

install: release ## Build release and install to /usr/local/bin
	cp target/release/$(BINARY_NAME) $(INSTALL_PATH)/

install-hooks: ## Show Claude Code hook setup instructions
	@echo "Claude Code Integration Setup"
	@echo ""
	@echo "1. Add to .claude/settings.json:"
	@echo '   {"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"safe-rm"}]}]}}'
	@echo ""
	@echo "2. Add file deletion rules to CLAUDE.md (see README.md)"
	@echo ""
	@echo "3. Grant permission: claude /permissions add Bash \"safe-rm*\""

## Development

test: ## Run all tests
	cargo test

test-unit: ## Run unit tests only
	cargo test --lib

test-integration: ## Run integration tests only
	cargo test --test integration_test

fmt: ## Format code
	cargo fmt

check: ## Run clippy and check
	cargo clippy -- -D warnings
	cargo check

clean: ## Clean build artifacts
	cargo clean

## Help

help: ## Show this help message
	@echo "safe-rm Build Commands"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Release:"
	@echo "  Use GitHub Actions > Release > Run workflow"
