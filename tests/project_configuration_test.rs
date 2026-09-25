//! プロジェクト設定の回帰テスト

const MAKEFILE: &str = include_str!("../Makefile");

/// `target:` で始まるターゲット行の直後にある、最初の空でないレシピ行を返す。
fn first_recipe_line(target: &str) -> &'static str {
    let header = format!("{target}:");
    let mut lines = MAKEFILE.lines();
    lines
        .find(|line| line.starts_with(&header))
        .unwrap_or_else(|| panic!("Makefile に {target} ターゲットが存在する必要がある"));
    lines
        .find(|line| !line.trim().is_empty())
        .unwrap_or_else(|| panic!("{target} ターゲットにレシピが存在する必要がある"))
}

#[test]
fn release_target_uses_locked_dependencies() {
    // cargo に渡すフラグの既定値が --locked であること。`make release CARGO_FLAGS=` のように
    // 明示して外さない限り、コミット済みの Cargo.lock を強制する
    assert!(
        MAKEFILE
            .lines()
            .any(|line| line == "CARGO_FLAGS ?= --locked"),
        "Makefile の CARGO_FLAGS の既定値は --locked である必要がある"
    );

    assert_eq!(
        first_recipe_line("release"),
        "\t$(RUN) cargo build --release $(CARGO_FLAGS)",
        "release ビルドは CARGO_FLAGS (既定 --locked) でコミット済みの Cargo.lock を強制する必要がある"
    );
}
