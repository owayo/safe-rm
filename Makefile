# safe-rm の開発タスク。引数なしの `make` でターゲット一覧を表示する。
#
# ツールの版は mise.toml で固定する。mise が使えるときは全ツールを `mise exec --`
# 経由で起動し、シェルで mise が有効化されていなくても（IDE や GUI からの起動など）
# 固定版を使う。SYSTEM_TOOLS=1 の場合は PATH 上のツールを使い、版は保証しない。
#
# macOS に付属する GNU Make 3.81 で使える機能だけを使う。
# .ONESHELL、.SHELLFLAGS、$(file ...) および != は使わない。

.DEFAULT_GOAL := help

BINARY_NAME := safe-rm
INSTALL_PATH ?= /usr/local/bin
# Cargo.lock をコミットしているため、CI と同じ依存解決を使う。
# release と、それを経由する install でもこの既定値を維持する。
# tests/project_configuration_test.rs で検査する。
CARGO_FLAGS ?= --locked

# ---- ツールチェーン -------------------------------------------------------------
# mise を PATH と一般的なインストール先から探す。GUI から起動した make はシェルの
# PATH を引き継がない場合がある。make MISE=/path/to/mise で上書きできる。
# mise がない場合の動作確認には MISE_CANDIDATES= で候補を空にする。
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

## 準備

setup: ## ツールチェーン (mise) と依存を取得
	@if [ -n "$(MISE)" ]; then "$(MISE)" install; fi
	$(RUN) cargo fetch $(CARGO_FLAGS)

## ビルド

build: ## デバッグ用バイナリをビルド
	$(RUN) cargo build $(CARGO_FLAGS)

release: ## リリース用バイナリをビルド
	$(RUN) cargo build --release $(CARGO_FLAGS)

## 検査

test: ## 全テストを実行
	$(RUN) cargo test $(CARGO_FLAGS)

test-unit: ## ライブラリのユニットテストだけを実行 (cargo test --lib)
	$(RUN) cargo test $(CARGO_FLAGS) --lib

test-integration: ## 統合テストだけを実行 (tests/integration_test.rs)
	$(RUN) cargo test $(CARGO_FLAGS) --test integration_test

lint: ## 警告をエラーとして clippy を実行
	$(RUN) cargo clippy $(CARGO_FLAGS) --all-targets -- -D warnings

fmt: ## コードを整形 (ファイルを書き換える)
	$(RUN) cargo fmt --all

fmt-check: ## 整形済みか検査 (書き換えない)
	$(RUN) cargo fmt --all -- --check

check: fmt-check lint ## fmt-check と lint を実行 (書き換えない)

ci: check test ## CI と同じ検査を実行 (書き換えない)

## インストール

# バイナリを直接上書きせず、一時ファイルを同じディレクトリに置いて rename する。
# macOS はコード署名の検証結果を inode 単位でキャッシュするため、実行中または直前に
# 実行したバイナリを cp で上書きすると、起動直後に SIGKILL（exit 137）される。
# rename により inode ごと入れ替える。
install: release ## リリース用バイナリを INSTALL_PATH に配置 (既定 /usr/local/bin)
	@mkdir -p "$(INSTALL_PATH)"
	cp "target/release/$(BINARY_NAME)" "$(INSTALL_PATH)/$(BINARY_NAME).new"
	mv -f "$(INSTALL_PATH)/$(BINARY_NAME).new" "$(INSTALL_PATH)/$(BINARY_NAME)"

# 手順の表示のみ行う。フック本体と CLAUDE.md のルールは README.md の
# Claude Code Integration に集約し、更新対象を一箇所にする。
install-hooks: ## Claude Code 連携の設定手順を表示
	@echo "Claude Code integration"
	@echo ""
	@echo "1. Add the PreToolUse hook from README.md (Claude Code Integration > Hook Configuration)"
	@echo "   to the Claude Code settings.json (user or project)"
	@echo ""
	@echo "2. Add the file deletion rules from README.md (Claude Code Integration > CLAUDE.md Instructions)"
	@echo "   to CLAUDE.md"
	@echo ""
	@echo '3. Allow the command: claude /permissions add Bash "safe-rm*"'

uninstall: ## INSTALL_PATH からバイナリを削除
	rm -f "$(INSTALL_PATH)/$(BINARY_NAME)"

clean: ## ビルド成果物を削除
	$(RUN) cargo clean

## ヘルプ

help: ## このヘルプを表示
	@echo "Development tasks for $(BINARY_NAME)"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-17s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Tool versions are pinned in mise.toml. Run make setup first."
	@echo "Release: GitHub Actions > Release > Run workflow"
