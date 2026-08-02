//! プロジェクト設定の回帰テスト

#[test]
fn release_target_uses_locked_dependencies() {
    let mut lines = include_str!("../Makefile").lines();

    lines
        .find(|line| line.starts_with("release:"))
        .expect("Makefile に release ターゲットが存在する必要がある");
    let recipe = lines
        .find(|line| !line.trim().is_empty())
        .expect("release ターゲットにレシピが存在する必要がある");

    assert_eq!(
        recipe, "\tcargo build --locked --release",
        "release ビルドはコミット済みの Cargo.lock を強制する必要がある"
    );
}
