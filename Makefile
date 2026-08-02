.PHONY: build release install install-hooks clean test fmt check help

# デフォルトターゲット
.DEFAULT_GOAL := help

# 変数
BINARY_NAME := safe-rm
INSTALL_PATH := /usr/local/bin

## ビルドコマンド

build: ## デバッグビルド
	cargo build

release: ## リリースビルド
	cargo build --locked --release

## インストール

install: release ## リリースビルドして /usr/local/bin にインストール
	cp target/release/$(BINARY_NAME) $(INSTALL_PATH)/

install-hooks: ## Claude Code hook のセットアップ手順を表示
	@echo "Claude Code 連携セットアップ"
	@echo ""
	@echo "1. .claude/settings.json に追加:"
	@echo '   {"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"safe-rm"}]}]}}'
	@echo ""
	@echo "2. CLAUDE.md にファイル削除ルールを追加（README.md 参照）"
	@echo ""
	@echo "3. 権限を付与: claude /permissions add Bash \"safe-rm*\""

## 開発

test: ## 全テストを実行
	cargo test

test-unit: ## ユニットテストのみ実行
	cargo test --lib

test-integration: ## 統合テストのみ実行
	cargo test --test integration_test

fmt: ## コードをフォーマット
	cargo fmt

check: ## clippy と cargo check を実行
	cargo clippy --locked --all-targets --all-features -- -D warnings
	cargo check --locked --all-targets --all-features

clean: ## ビルド成果物を削除
	cargo clean

## ヘルプ

help: ## このヘルプを表示
	@echo "safe-rm ビルドコマンド"
	@echo ""
	@echo "使い方: make [target]"
	@echo ""
	@echo "ターゲット:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "リリース:"
	@echo "  GitHub Actions > Release > Run workflow を使用"
