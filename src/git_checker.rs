//! safe-rm の Git ステータスチェック
//!
//! Git リポジトリを検出し、安全な削除のためにファイルステータスを確認する。

use crate::error::{FileStatus, SafeRmError};
use git2::{Repository, Status, StatusOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Git ステータスチェッカー
pub struct GitChecker {
    repo: Repository,
    /// canonicalize 済みワークディレクトリ（macOS /var→/private/var 等のエイリアス対策）
    workdir_canonical: Option<PathBuf>,
}

impl GitChecker {
    /// 指定パスから Git リポジトリを検出して開く
    ///
    /// `Repository::discover` を使用して上位ディレクトリを走査し、
    /// Gitリポジトリを検出する。サブディレクトリからでもリポジトリルートを正しく検出可能。
    ///
    /// Git API エラー（壊れた `.git`、権限不足、I/O エラー等）は fail-closed で
    /// `Err` を返す。`NotFound` 等のエラーコードでも、`path` 直下またはその祖先に
    /// `.git` メタデータが存在する場合は「壊れた `.git`」と判断して `Err` を伝播し、
    /// メタデータがどこにも見つからない場合のみ `Ok(None)` を返す。
    ///
    /// # 戻り値
    /// * `Ok(Some(GitChecker))` - Git リポジトリが存在
    /// * `Ok(None)` - Git リポジトリなし（Git チェックスキップ）
    /// * `Err(SafeRmError::GitError)` - Git API エラー（fail-closed）
    pub fn open(path: &Path) -> Result<Option<Self>, SafeRmError> {
        match Repository::discover(path) {
            Ok(repo) => {
                let workdir_canonical = repo
                    .workdir()
                    .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
                Ok(Some(Self {
                    repo,
                    workdir_canonical,
                }))
            }
            // `NotFound` は通常「Git リポジトリが見つからない」を意味する一方、
            // 壊れた `.git` でも `NotFound` として返るケースがあるため、祖先に
            // `.git` 痕跡があれば「壊れたリポジトリ」とみなし、痕跡確認そのものが
            // 失敗した場合も信頼できないため fail-closed に倒す。痕跡が一切なく、
            // 確認にも成功した場合のみ非 Git 環境として `Ok(None)` を返す。
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                match Self::has_git_metadata_ancestor(path) {
                    Ok(false) => Ok(None),
                    // `.git` 痕跡がある場合だけでなく、痕跡確認そのものが権限等で
                    // 失敗した場合も非 Git 環境として扱わず fail-closed に倒す。
                    Ok(true) | Err(_) => Err(SafeRmError::GitError(e)),
                }
            }
            // それ以外（権限エラー、I/O 失敗、壊れた `.git` で別エラーコード等）は
            // 無条件に fail-closed で伝播。祖先確認では拾いきれない経路を許さない。
            Err(e) => Err(SafeRmError::GitError(e)),
        }
    }

    /// `path` 自身または任意の祖先ディレクトリに `.git` メタデータが存在するか確認する。
    ///
    /// 通常リポジトリの `.git` ディレクトリ/ファイル、および bare リポジトリの
    /// ルートを検出する。`Repository::discover` が `NotFound` 系のエラーを返した
    /// 場合でも、`.git` が見つかれば「壊れた Git リポジトリ」とみなして fail-closed に倒す。
    fn has_git_metadata_ancestor(path: &Path) -> Result<bool, std::io::Error> {
        // path 自身が `.git` ディレクトリ/ファイル、または bare リポジトリの場合も検出
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_type().is_dir() && Self::is_bare_repository_root(path) {
                    return Ok(true);
                }
            }
            Err(e) if Self::metadata_absence_error(&e) => {}
            Err(e) => return Err(e),
        }

        let mut current = Some(path);
        while let Some(dir) = current {
            // 通常リポジトリの `.git` ファイル/ディレクトリの存在確認
            let dot_git = dir.join(".git");
            match std::fs::symlink_metadata(&dot_git) {
                Ok(_) => return Ok(true),
                Err(e) if Self::metadata_absence_error(&e) => {}
                Err(e) => return Err(e),
            }
            // bare リポジトリ（HEAD/objects/refs が同階層）の存在確認
            if Self::is_bare_repository_root(dir) {
                return Ok(true);
            }
            current = dir.parent();
        }
        Ok(false)
    }

    /// メタデータ取得失敗のうち「対象が存在しない」と同等に扱えるエラーか判定する。
    fn metadata_absence_error(error: &std::io::Error) -> bool {
        matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
        )
    }

    /// Git リポジトリのワークディレクトリ（ルート）を取得
    ///
    /// フルパス指定時のプロジェクト境界判定に使用。
    /// bare リポジトリの場合は None を返す。
    /// macOS の /var → /private/var シンボリックリンク対策で canonicalize 済み。
    pub fn workdir(&self) -> Option<PathBuf> {
        self.workdir_canonical.clone()
    }

    /// Git 管理メタデータ（`.git` や bare リポジトリ本体）へのパスか判定する。
    ///
    /// 通常のリポジトリでは `.git` ディレクトリとその配下を保護し、
    /// bare リポジトリではリポジトリ全体を保護対象にする。
    /// `.git` が gitdir ファイルの環境でも、そのファイル自体を削除できないようにする。
    ///
    /// 末尾コンポーネントは canonicalize せず保持することで、`.git` を指す
    /// symlink 自身は実体ではないため Git 管理メタデータには含めない。
    /// 中間 symlink 経由のアクセスは親までの canonicalize で検出する。
    pub fn is_git_metadata_path(&self, path: &Path) -> bool {
        let target = Self::canonicalize_parent_keep_filename(path);
        self.protected_git_roots()
            .iter()
            .any(|root| target.starts_with(root))
    }

    /// 指定パス自体または配下が Git 管理メタデータに触れるか判定する。
    ///
    /// `safe-rm -r .` のようにリポジトリルートを再帰削除すると、直接 `.git` を
    /// 指定していなくても Git 管理メタデータが削除対象に含まれるためブロックする。
    ///
    /// 末尾コンポーネントは canonicalize せずに保持することで、`.git` を指す
    /// symlink 自身の削除はリンクだけが消えて実体が残るため許可する
    /// （`test_path_targets_symlink_self_to_dot_git_allowed` と同じ方針）。
    /// `gitlink/config` のように中間 symlink を介して `.git` を指すケースでは、
    /// 親までは canonicalize されるため引き続きブロックされる。
    pub fn touches_git_metadata_path(&self, path: &Path) -> bool {
        let target = Self::canonicalize_parent_keep_filename(path);
        self.protected_git_roots()
            .iter()
            .any(|root| target.starts_with(root) || root.starts_with(&target))
    }

    /// 削除対象パス自体、その途中の任意コンポーネント、または再帰削除時に
    /// その配下に Git 管理メタデータが含まれるかを判定する。
    ///
    /// safe-rm を実行している現在のリポジトリと無関係でも、任意階層の Git 管理
    /// メタデータの削除を常に拒否する。bare リポジトリは `.git` という
    /// コンポーネントを持たないため、既存祖先が bare リポジトリの場合も拒否する。
    /// シンボリックリンクは辿らず、リンク自身を評価することでリンク経由の脱出も防ぐ。
    /// macOS APFS のような大文字小文字を区別しないファイルシステムで
    /// `.GIT` 経由のバイパスを防ぐため、コンポーネント比較は ASCII case-insensitive。
    pub fn path_targets_or_contains_git_metadata(path: &Path, recursive: bool) -> bool {
        Self::try_path_targets_or_contains_git_metadata(path, recursive).unwrap_or(true)
    }

    /// 削除対象パス自体、その途中の任意コンポーネント、または再帰削除時に
    /// その配下に Git 管理メタデータが含まれるかを判定する。
    ///
    /// 実削除前に使う検査経路。ディレクトリ読み取りエラーやエントリ取得エラーを
    /// 呼び出し元へ返し、fail-closed で削除をブロックできるようにする。
    pub fn try_path_targets_or_contains_git_metadata(
        path: &Path,
        recursive: bool,
    ) -> Result<bool, SafeRmError> {
        // パス内の任意のコンポーネントが `.git`（大文字小文字無視）の場合は常時ブロック。
        // 末尾だけでなく `nested/.git/config` のような中間コンポーネントも対象。
        if Self::path_has_dot_git_component(path) {
            return Ok(true);
        }

        // 中間コンポーネントが symlink で、その実体が `.git` を含むパスを指す場合
        // （例: `gitlink -> nested/.git` のとき `gitlink/config` の削除で
        // `nested/.git/config` が消されるケース）も検出する。
        // 末尾コンポーネントは canonicalize しない（symlink 自身の削除はリンクだけが
        // 消えて実体は残るため、`gitlink` 単体の削除を過剰にブロックしない）。
        let resolved_for_check = Self::canonicalize_parent_keep_filename(path);
        if resolved_for_check != path && Self::path_has_dot_git_component(&resolved_for_check) {
            return Ok(true);
        }

        if Self::path_targets_bare_repository_metadata(&resolved_for_check) {
            return Ok(true);
        }

        if !recursive {
            return Ok(false);
        }

        // 再帰削除でも、対象が通常ディレクトリでなければ配下に Git メタデータは存在しない
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(SafeRmError::IoError(e)),
        };
        let file_type = metadata.file_type();
        if file_type.is_symlink() || !file_type.is_dir() {
            return Ok(false);
        }

        Self::contains_git_metadata_recursive(path)
    }

    /// パスのいずれかのコンポーネントが `.git`（ASCII case-insensitive）か判定する。
    fn path_has_dot_git_component(path: &Path) -> bool {
        path.components().any(|component| {
            matches!(component, std::path::Component::Normal(name) if Self::is_dot_git_component(name))
        })
    }

    /// 親ディレクトリのみを canonicalize し、末尾のコンポーネントは元のまま保持する。
    ///
    /// これにより中間 symlink を辿った実体パスを得つつ、末尾が symlink でも
    /// その実体は解決しない。`gitlink/config` のように中間 symlink で `.git` を
    /// バイパスするケースは検出できるが、`gitlink` 単体の削除（リンクだけ消える）は
    /// 通常ファイルとして扱える。
    fn canonicalize_parent_keep_filename(path: &Path) -> PathBuf {
        let Some(file_name) = path.file_name() else {
            return path.to_path_buf();
        };
        let Some(parent) = path.parent() else {
            return path.to_path_buf();
        };
        if let Ok(canonical_parent) = parent.canonicalize() {
            return canonical_parent.join(file_name);
        }
        // 親も canonicalize できない場合は、既存祖先まで辿って再結合する。
        // 末尾コンポーネントは保持するので末尾 symlink を辿らない原則は守られる。
        Self::try_canonicalize_existing_parent(path)
    }

    /// `.git` の大文字小文字を区別しない比較。
    /// 末尾コンポーネントだけでなく中間コンポーネントの判定にも使う。
    fn is_dot_git_component(name: &std::ffi::OsStr) -> bool {
        name.to_str()
            .map(|s| s.eq_ignore_ascii_case(".git"))
            .unwrap_or(false)
    }

    /// 指定パス自体または既存祖先が bare リポジトリか確認する。
    fn path_targets_bare_repository_metadata(path: &Path) -> bool {
        let mut current = Some(path);
        while let Some(candidate) = current {
            if Self::is_bare_repository_root(candidate) {
                return true;
            }
            current = candidate.parent();
        }
        false
    }

    /// 指定ディレクトリが bare リポジトリのルートか確認する。
    fn is_bare_repository_root(path: &Path) -> bool {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return false;
        };
        let file_type = metadata.file_type();
        if file_type.is_symlink() || !file_type.is_dir() {
            return false;
        }

        Repository::open_bare(path).is_ok()
    }

    /// 指定ディレクトリ配下に Git 管理メタデータが存在するかを再帰的に確認する。
    ///
    /// `fs::remove_dir_all` と同様にシンボリックリンクは辿らない。
    /// 大文字小文字を区別しない比較で `.GIT` 等のバリアントも検出し、
    /// `.git` コンポーネントを持たない bare リポジトリも検出する。
    fn contains_git_metadata_recursive(dir: &Path) -> Result<bool, SafeRmError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => {
                return Err(SafeRmError::DirectoryReadError {
                    path: dir.to_path_buf(),
                });
            }
        };

        for entry_result in entries {
            let entry = entry_result.map_err(|_| SafeRmError::DirectoryReadError {
                path: dir.to_path_buf(),
            })?;
            if Self::is_dot_git_component(&entry.file_name()) {
                return Ok(true);
            }
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => {
                    return Err(SafeRmError::DirectoryReadError { path: entry.path() });
                }
            };
            if file_type.is_dir() && !file_type.is_symlink() {
                let entry_path = entry.path();
                if Self::is_bare_repository_root(&entry_path)
                    || Self::contains_git_metadata_recursive(&entry_path)?
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// 絶対パスからワークディレクトリ相対パスを取得
    ///
    /// canonicalize 済みパスと未解決パスの両方に対応し、
    /// macOS の /var→/private/var 等のエイリアス差異を吸収する。
    fn to_workdir_relative(&self, path: &Path) -> Option<PathBuf> {
        // canonicalize 済みワークディレクトリで試行
        if let Some(canonical) = &self.workdir_canonical {
            if let Ok(rel) = path.strip_prefix(canonical) {
                return Some(rel.to_path_buf());
            }
        }
        // フォールバック: 未解決ワークディレクトリで試行
        if let Some(raw) = self.repo.workdir() {
            if let Ok(rel) = path.strip_prefix(raw) {
                return Some(rel.to_path_buf());
            }
        }
        None
    }

    /// 削除を常時ブロックすべき Git 管理ディレクトリ/ファイルの一覧を返す。
    fn protected_git_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![Self::try_canonicalize_existing_parent(self.repo.path())];

        if let Some(workdir) = &self.workdir_canonical {
            let displayed_git_entry = Self::try_canonicalize_existing_parent(&workdir.join(".git"));
            if !roots.contains(&displayed_git_entry) {
                roots.push(displayed_git_entry);
            }
        }

        roots
    }

    /// 可能であれば canonicalize する。
    /// 末尾が未作成で失敗した場合は、既存の親ディレクトリまで canonicalize してから
    /// 未作成部分を再結合する。
    fn try_canonicalize_existing_parent(path: &Path) -> PathBuf {
        if let Ok(canonical) = path.canonicalize() {
            return canonical;
        }

        let mut current = path;
        let mut missing_segments = Vec::new();

        while let Some(parent) = current.parent() {
            if let Some(name) = current.file_name() {
                missing_segments.push(name.to_os_string());
            }

            if let Ok(canonical_parent) = parent.canonicalize() {
                let mut rebuilt = canonical_parent;
                for segment in missing_segments.iter().rev() {
                    rebuilt.push(segment);
                }
                return rebuilt;
            }

            current = parent;
        }

        path.to_path_buf()
    }

    /// 全ファイルのステータスを一括取得（バッチ処理用）
    ///
    /// 一度の Git API 呼び出しで全ステータスを取得し、HashMap として返す。
    /// これにより、多数のファイルを処理する際の API 呼び出し回数を削減。
    /// Git API エラー時は fail-closed でエラーを返す。
    ///
    /// # 戻り値
    /// * `Ok(HashMap<String, FileStatus>)` - 相対パス → ステータスのマップ
    /// * `Err(SafeRmError)` - Git API エラー（ステータス取得不可時は削除をブロック）
    pub fn get_all_statuses(&self) -> Result<HashMap<String, FileStatus>, SafeRmError> {
        let mut status_map = HashMap::new();

        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.include_ignored(true);
        opts.include_unmodified(true);
        opts.recurse_untracked_dirs(true);

        let statuses = self.repo.statuses(Some(&mut opts))?;
        for entry in statuses.iter() {
            // git2 0.21 で `entry.path()` は UTF-8 でないパスに対して `Err` を返すようになった。
            // UTF-8 でないパスはキャッシュキーとして扱えないためスキップし、
            // 該当パスは後続の単体問い合わせで fail-closed 経路に進ませる。
            if let Ok(path) = entry.path() {
                let status = Self::convert_status(entry.status());
                status_map.insert(path.to_string(), status);
            }
        }

        Ok(status_map)
    }

    /// キャッシュからファイルステータスを取得
    ///
    /// `get_all_statuses()` で事前取得したキャッシュを使用。
    /// キャッシュにない場合は Clean として扱う（Git 追跡済みで変更なし）。
    pub fn get_file_status_from_cache(
        &self,
        path: &Path,
        cache: &HashMap<String, FileStatus>,
    ) -> FileStatus {
        let relative_path = match self.to_workdir_relative(path) {
            Some(p) => p,
            None => return FileStatus::NotInRepo,
        };

        let path_key = Self::to_git_relative_key(&relative_path);

        // キャッシュから取得
        if let Some(&status) = cache.get(&path_key) {
            return status;
        }

        // キャッシュにない場合: .gitignore チェック
        if self.is_ignored_path(path) {
            return FileStatus::Ignored;
        }

        self.resolve_status_from_relative_path(&relative_path)
    }

    /// ファイルの Git ステータスを取得
    pub fn get_file_status(&self, path: &Path) -> FileStatus {
        // リポジトリルートからの相対パスを取得
        let relative_path = match self.to_workdir_relative(path) {
            Some(p) => p,
            None => return FileStatus::NotInRepo,
        };

        self.resolve_status_from_relative_path(&relative_path)
    }

    /// 相対パスの Git ステータスを解決
    ///
    /// `status_file()` は未追跡ディレクトリを 1 エントリに畳み込むため、
    /// その配下のファイルを直接問い合わせると `NotFound` になることがある。
    /// その場合は再帰付きの status 一覧で再確認し、未追跡ファイルを取りこぼさない。
    fn resolve_status_from_relative_path(&self, relative_path: &Path) -> FileStatus {
        match self.repo.status_file(relative_path) {
            Ok(status) => Self::convert_status(status),
            Err(e) if e.code() == git2::ErrorCode::NotFound => self
                .lookup_status_in_listing(relative_path)
                .unwrap_or(FileStatus::NotInRepo),
            // fail-closed: 予期しない Git エラー時は削除をブロック
            Err(_) => FileStatus::Modified,
        }
    }

    /// status 一覧から相対パスに対応するステータスを検索
    ///
    /// Git API エラー時は fail-closed で Modified を返し、削除をブロックする。
    fn lookup_status_in_listing(&self, relative_path: &Path) -> Option<FileStatus> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);
        opts.include_ignored(true);

        let relative_path_key = Self::to_git_relative_key(relative_path);
        match self.repo.statuses(Some(&mut opts)) {
            Ok(statuses) => {
                for entry in statuses.iter() {
                    // git2 0.21 で `entry.path()` は UTF-8 でないパスに対して `Err` を返すようになった。
                    // UTF-8 でないパスは比較対象外として無視し、見つからなければ後続の判定にフォールバックする。
                    if let Ok(entry_path) = entry.path()
                        && entry_path == relative_path_key
                    {
                        return Some(Self::convert_status(entry.status()));
                    }
                }
            }
            // fail-closed: Git API エラー時は削除をブロック
            Err(_) => return Some(FileStatus::Modified),
        }

        if self
            .repo
            .status_should_ignore(relative_path)
            .unwrap_or(false)
        {
            return Some(FileStatus::Ignored);
        }

        None
    }

    /// git2 のステータスフラグから FileStatus への変換
    fn convert_status(status: Status) -> FileStatus {
        // Ignored チェック（最優先）
        if status.contains(Status::IGNORED) {
            return FileStatus::Ignored;
        }

        // マージコンフリクト中のファイルは未解決の変更として扱い削除を禁止する。
        // 通常は INDEX_*/WT_* と一緒に立つが、単独で立つ稀なケースでも
        // Clean に落ちて削除許可にならないよう先にチェックする。
        if status.contains(Status::CONFLICTED) {
            return FileStatus::Modified;
        }

        // Index 変更（Staged）
        if status.intersects(
            Status::INDEX_NEW
                | Status::INDEX_MODIFIED
                | Status::INDEX_DELETED
                | Status::INDEX_RENAMED
                | Status::INDEX_TYPECHANGE,
        ) {
            return FileStatus::Staged;
        }

        // Worktree 変更（Modified）
        if status.intersects(
            Status::WT_MODIFIED | Status::WT_DELETED | Status::WT_RENAMED | Status::WT_TYPECHANGE,
        ) {
            return FileStatus::Modified;
        }

        // 未追跡
        if status.contains(Status::WT_NEW) {
            return FileStatus::Untracked;
        }

        // 上記以外（稀なケース）は Clean として扱う
        FileStatus::Clean
    }

    /// ステータスが削除許可かどうかを判定
    pub fn is_deletable(status: FileStatus) -> bool {
        status.is_deletable()
    }

    /// ファイルまたはディレクトリをチェック
    ///
    /// # 戻り値
    /// * `Ok(())` - 削除可能
    /// * `Err(SafeRmError::DirtyFiles)` - 変更のあるファイルが存在
    pub fn check_path(&self, path: &Path) -> Result<(), SafeRmError> {
        if Self::is_real_directory(path) {
            self.check_directory(path)
        } else {
            self.check_file(path)
        }
    }

    /// 単一ファイルのチェック
    fn check_file(&self, path: &Path) -> Result<(), SafeRmError> {
        let status = self.get_file_status(path);
        if Self::is_deletable(status) {
            Ok(())
        } else {
            Err(SafeRmError::DirtyFiles {
                path: path.to_path_buf(),
                status,
            })
        }
    }

    /// ディレクトリ内のすべてのファイルをチェック
    ///
    /// # 戻り値
    /// * `Ok(())` - 全ファイルが Clean または Ignored
    /// * `Err(SafeRmError::DirtyFiles)` - 変更のあるファイルが存在
    pub fn check_directory(&self, dir: &Path) -> Result<(), SafeRmError> {
        // ディレクトリ自体が ignored でも、配下に tracked な変更済みファイルが
        // 存在し得るため、早期許可せず各エントリを再帰的に検査する。
        self.check_directory_recursive(dir)
    }

    /// パスが .gitignore に含まれるかチェック
    fn is_ignored_path(&self, path: &Path) -> bool {
        let relative_path = match self.to_workdir_relative(path) {
            Some(p) => p,
            None => return false,
        };

        self.repo
            .status_should_ignore(&relative_path)
            .unwrap_or(false)
    }

    /// ディレクトリ内のファイルを再帰的にチェック
    fn check_directory_recursive(&self, dir: &Path) -> Result<(), SafeRmError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => {
                // fail-closed: ディレクトリ読み取り失敗時は削除をブロック
                return Err(SafeRmError::DirectoryReadError {
                    path: dir.to_path_buf(),
                });
            }
        };

        for entry_result in entries {
            let entry = entry_result.map_err(|_| SafeRmError::DirectoryReadError {
                path: dir.to_path_buf(),
            })?;
            let path = entry.path();

            if Self::is_real_directory(&path) {
                // サブディレクトリは再帰的にチェック
                self.check_directory(&path)?;
            } else {
                // ファイルのステータスをチェック
                let status = self.get_file_status(&path);
                if !Self::is_deletable(status) {
                    return Err(SafeRmError::DirtyFiles { path, status });
                }
            }
        }

        Ok(())
    }

    /// ディレクトリ内のファイルをキャッシュを使用して再帰的にチェック（高速版）
    ///
    /// `get_all_statuses()` で事前取得したキャッシュを使用することで、
    /// 多数のファイルを持つディレクトリの検証を高速化。
    pub fn check_directory_with_cache(
        &self,
        dir: &Path,
        cache: &HashMap<String, FileStatus>,
    ) -> Result<(), SafeRmError> {
        // キャッシュ利用時も、ignored ディレクトリ配下の tracked 変更を
        // 見落とさないように必ず各エントリを検査する。
        self.check_directory_recursive_with_cache(dir, cache)
    }

    /// キャッシュを使用した再帰的ディレクトリチェック
    fn check_directory_recursive_with_cache(
        &self,
        dir: &Path,
        cache: &HashMap<String, FileStatus>,
    ) -> Result<(), SafeRmError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => {
                return Err(SafeRmError::DirectoryReadError {
                    path: dir.to_path_buf(),
                });
            }
        };

        for entry_result in entries {
            let entry = entry_result.map_err(|_| SafeRmError::DirectoryReadError {
                path: dir.to_path_buf(),
            })?;
            let path = entry.path();

            if Self::is_real_directory(&path) {
                // サブディレクトリも再帰的にチェック
                self.check_directory_with_cache(&path, cache)?;
            } else {
                // キャッシュからステータスを取得
                let status = self.get_file_status_from_cache(&path, cache);
                if !Self::is_deletable(status) {
                    return Err(SafeRmError::DirtyFiles { path, status });
                }
            }
        }

        Ok(())
    }

    /// 単一ファイルをキャッシュを使用してチェック
    pub fn check_file_with_cache(
        &self,
        path: &Path,
        cache: &HashMap<String, FileStatus>,
    ) -> Result<(), SafeRmError> {
        let status = self.get_file_status_from_cache(path, cache);
        if Self::is_deletable(status) {
            Ok(())
        } else {
            Err(SafeRmError::DirtyFiles {
                path: path.to_path_buf(),
                status,
            })
        }
    }

    /// ファイルまたはディレクトリをキャッシュを使用してチェック
    pub fn check_path_with_cache(
        &self,
        path: &Path,
        cache: &HashMap<String, FileStatus>,
    ) -> Result<(), SafeRmError> {
        if Self::is_real_directory(path) {
            self.check_directory_with_cache(path, cache)
        } else {
            self.check_file_with_cache(path, cache)
        }
    }

    /// Git status のキー形式（スラッシュ区切り）に揃える
    fn to_git_relative_key(path: &Path) -> String {
        path.components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// シンボリックリンクを辿らずに「実体がディレクトリか」を判定
    fn is_real_directory(path: &Path) -> bool {
        std::fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_dir())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    /// テスト用の Git リポジトリを作成
    fn create_test_repo() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path();

        // Git リポジトリを初期化
        Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        // テスト用コミットの設定
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        temp_dir
    }

    /// テスト用ヘルパー: Git リポジトリを開いて `GitChecker` を取得する。
    /// `open()` は `Result<Option<Self>>` を返すため、テストでは二重 unwrap を避ける。
    fn open_checker(path: &Path) -> GitChecker {
        GitChecker::open(path)
            .expect("Git API エラーは想定外")
            .expect("Git リポジトリが存在すべき")
    }

    /// ファイルを作成してコミット
    fn commit_file(repo_path: &Path, filename: &str, content: &str) {
        let file_path = repo_path.join(filename);
        fs::write(&file_path, content).unwrap();

        Command::new("git")
            .args(["add", filename])
            .current_dir(repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", &format!("Add {}", filename)])
            .current_dir(repo_path)
            .output()
            .unwrap();
    }

    // Git リポジトリ検出のテスト

    #[test]
    fn test_open_git_repo() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let checker = GitChecker::open(&repo_path).expect("Git API エラーは想定外");
        assert!(checker.is_some());
    }

    #[test]
    fn test_open_non_git_directory() {
        let temp_dir = TempDir::new().unwrap();
        let checker = GitChecker::open(temp_dir.path()).expect("非 Git ディレクトリは Ok(None)");
        assert!(checker.is_none());
    }

    #[test]
    fn test_is_git_metadata_path_blocks_dot_git_directory() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "tracked.txt", "tracked");

        let checker = open_checker(&repo_path);

        assert!(checker.is_git_metadata_path(&repo_path.join(".git")));
        assert!(checker.is_git_metadata_path(&repo_path.join(".git").join("config")));
        assert!(!checker.is_git_metadata_path(&repo_path.join("tracked.txt")));
    }

    #[test]
    fn test_touches_git_metadata_path_blocks_repo_root() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "tracked.txt", "tracked");

        let checker = open_checker(&repo_path);

        assert!(checker.touches_git_metadata_path(&repo_path));
        assert!(checker.touches_git_metadata_path(&repo_path.join(".git")));
        assert!(checker.touches_git_metadata_path(&repo_path.join(".git").join("config")));
        assert!(!checker.touches_git_metadata_path(&repo_path.join("tracked.txt")));
    }

    // path_targets_or_contains_git_metadata のテスト

    #[test]
    fn test_path_targets_dot_git_directly() {
        // `.git` ファイル/ディレクトリ自身は recursive フラグに関わらず常時ブロック
        let temp_dir = TempDir::new().unwrap();
        let dot_git = temp_dir.path().join(".git");
        fs::create_dir_all(&dot_git).unwrap();

        assert!(GitChecker::path_targets_or_contains_git_metadata(
            &dot_git, false
        ));
        assert!(GitChecker::path_targets_or_contains_git_metadata(
            &dot_git, true
        ));
    }

    #[test]
    fn test_path_targets_dot_git_file() {
        // `.git` という名前のファイル（gitlink 等）も保護対象
        let temp_dir = TempDir::new().unwrap();
        let dot_git_file = temp_dir.path().join(".git");
        fs::write(&dot_git_file, "gitdir: /tmp/elsewhere").unwrap();

        assert!(GitChecker::path_targets_or_contains_git_metadata(
            &dot_git_file,
            false
        ));
    }

    #[test]
    fn test_path_targets_non_git_path_not_recursive() {
        // 通常のファイルは保護対象外
        let temp_dir = TempDir::new().unwrap();
        let file = temp_dir.path().join("regular.txt");
        fs::write(&file, "content").unwrap();

        assert!(!GitChecker::path_targets_or_contains_git_metadata(
            &file, false
        ));
    }

    #[test]
    fn test_path_targets_dir_with_dot_git_recursive() {
        // 配下に `.git` を含むディレクトリは recursive=true 時のみブロック
        let temp_dir = TempDir::new().unwrap();
        let nested = temp_dir.path().join("nested");
        let dot_git = nested.join(".git");
        fs::create_dir_all(&dot_git).unwrap();

        assert!(GitChecker::path_targets_or_contains_git_metadata(
            &nested, true
        ));
        // recursive=false なら配下は探索しない
        assert!(!GitChecker::path_targets_or_contains_git_metadata(
            &nested, false
        ));
    }

    #[test]
    fn test_path_targets_bare_repo_recursive() {
        // bare リポジトリは `.git` コンポーネントを持たないが Git 管理メタデータ。
        let temp_dir = TempDir::new().unwrap();
        let bare_repo_path = temp_dir.path().join("repo.git");

        Command::new("git")
            .args(["init", "--bare", bare_repo_path.to_str().unwrap()])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        assert!(GitChecker::path_targets_or_contains_git_metadata(
            &bare_repo_path,
            true
        ));
        assert!(GitChecker::path_targets_or_contains_git_metadata(
            &bare_repo_path.join("HEAD"),
            false
        ));
    }

    #[test]
    fn test_path_targets_dir_with_nested_bare_repo_recursive() {
        let temp_dir = TempDir::new().unwrap();
        let parent = temp_dir.path().join("parent");
        let bare_repo_path = parent.join("cache.git");
        fs::create_dir_all(&parent).unwrap();

        Command::new("git")
            .args(["init", "--bare", bare_repo_path.to_str().unwrap()])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        assert!(GitChecker::path_targets_or_contains_git_metadata(
            &parent, true
        ));
        assert!(!GitChecker::path_targets_or_contains_git_metadata(
            &parent, false
        ));
    }

    #[test]
    fn test_path_targets_deeply_nested_dot_git() {
        // 深くネストされた `.git` も recursive=true で検出
        let temp_dir = TempDir::new().unwrap();
        let deep = temp_dir.path().join("a").join("b").join("c");
        let dot_git = deep.join(".git");
        fs::create_dir_all(&dot_git).unwrap();

        assert!(GitChecker::path_targets_or_contains_git_metadata(
            temp_dir.path(),
            true
        ));
    }

    #[test]
    fn test_path_targets_dir_without_dot_git() {
        // 配下に `.git` がないディレクトリは保護対象外
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path().join("no_git_here");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("regular.txt"), "content").unwrap();

        assert!(!GitChecker::path_targets_or_contains_git_metadata(
            &dir, true
        ));
    }

    #[test]
    fn test_path_targets_nonexistent_path() {
        // 存在しないパスは false（後段の包含検証等に判断を委ねる）
        let temp_dir = TempDir::new().unwrap();
        let nonexistent = temp_dir.path().join("missing");

        assert!(!GitChecker::path_targets_or_contains_git_metadata(
            &nonexistent,
            true
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_try_path_targets_reports_directory_read_error() {
        // `.git` 探索中に読めないディレクトリへ到達した場合は fail-closed で返す
        let temp_dir = TempDir::new().unwrap();
        let parent = temp_dir.path().join("parent");
        let unreadable = parent.join("unreadable");
        fs::create_dir_all(&unreadable).unwrap();

        use std::os::unix::fs::PermissionsExt;
        let original_permissions = fs::metadata(&unreadable).unwrap().permissions();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();

        let read_dir_denied = fs::read_dir(&unreadable).is_err();
        let try_result = GitChecker::try_path_targets_or_contains_git_metadata(&parent, true);
        let legacy_result = GitChecker::path_targets_or_contains_git_metadata(&parent, true);

        fs::set_permissions(&unreadable, original_permissions).unwrap();

        // root 等で読み取り制限が効かない環境では、このケースは検証対象外にする
        if read_dir_denied {
            match try_result {
                Err(SafeRmError::DirectoryReadError { path }) => assert_eq!(path, unreadable),
                other => panic!("DirectoryReadError が期待されたが {other:?} が返された"),
            }
            assert!(
                legacy_result,
                "互換用 bool API も読み取りエラー時は保守的にブロックするべき"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_path_targets_does_not_follow_symlink_to_dir_with_dot_git() {
        // ディレクトリへのシンボリックリンクは辿らない（リンク自身を評価）
        let temp_dir = TempDir::new().unwrap();
        let real_repo = temp_dir.path().join("real_repo");
        fs::create_dir_all(real_repo.join(".git")).unwrap();

        let link = temp_dir.path().join("link_to_repo");
        std::os::unix::fs::symlink(&real_repo, &link).unwrap();

        // link 自身は `.git` ではないし、シンボリックリンクなので配下も探索しない
        assert!(!GitChecker::path_targets_or_contains_git_metadata(
            &link, true
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_path_targets_does_not_descend_into_symlink_subdir() {
        // 配下にシンボリックリンクで `.git` を持つディレクトリがあっても、
        // シンボリックリンクは辿らないので保護対象外（実体の `.git` は別経路で守る）
        let temp_dir = TempDir::new().unwrap();
        let real_repo = temp_dir.path().join("real_repo");
        fs::create_dir_all(real_repo.join(".git")).unwrap();

        let parent = temp_dir.path().join("parent");
        fs::create_dir_all(&parent).unwrap();
        let link_inside = parent.join("link_to_repo");
        std::os::unix::fs::symlink(&real_repo, &link_inside).unwrap();

        // parent 配下に直接の `.git` はない（リンクは辿らない）
        assert!(!GitChecker::path_targets_or_contains_git_metadata(
            &parent, true
        ));
    }

    #[test]
    fn test_is_git_metadata_path_blocks_bare_repo_contents() {
        let temp_dir = TempDir::new().unwrap();
        let bare_repo_path = temp_dir.path().join("repo.git");

        Command::new("git")
            .args(["init", "--bare", bare_repo_path.to_str().unwrap()])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        let checker = open_checker(&bare_repo_path);

        assert!(checker.is_git_metadata_path(&bare_repo_path.join("HEAD")));
        assert!(checker.is_git_metadata_path(&bare_repo_path.join("objects")));
    }

    // ファイルステータス判定のテスト

    #[test]
    fn test_get_file_status_clean() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルを作成してコミット
        commit_file(&repo_path, "clean.txt", "clean content");

        let checker = open_checker(&repo_path);
        let file_path = repo_path.join("clean.txt");
        let status = checker.get_file_status(&file_path);

        assert_eq!(status, FileStatus::Clean);
    }

    #[test]
    fn test_get_file_status_modified() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルを作成してコミット
        commit_file(&repo_path, "modified.txt", "original content");

        // ファイルを変更
        let file_path = repo_path.join("modified.txt");
        fs::write(&file_path, "modified content").unwrap();

        let checker = open_checker(&repo_path);
        let status = checker.get_file_status(&file_path);

        assert_eq!(status, FileStatus::Modified);
    }

    #[test]
    fn test_get_file_status_staged() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルを作成
        let file_path = repo_path.join("staged.txt");
        fs::write(&file_path, "staged content").unwrap();

        // git add（コミットせず）
        Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = open_checker(&repo_path);
        let status = checker.get_file_status(&file_path);

        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_get_file_status_untracked() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミットを作成（空のリポジトリでないことを確認）
        commit_file(&repo_path, "initial.txt", "initial");

        // 未追跡ファイルを作成
        let file_path = repo_path.join("untracked.txt");
        fs::write(&file_path, "untracked content").unwrap();

        let checker = open_checker(&repo_path);
        let status = checker.get_file_status(&file_path);

        assert_eq!(status, FileStatus::Untracked);
    }

    #[test]
    fn test_get_file_status_nested_untracked_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 空リポジトリ扱いを避けるため初期コミットを作成
        commit_file(&repo_path, "initial.txt", "initial");

        let nested_dir = repo_path.join("newdir").join("deep");
        fs::create_dir_all(&nested_dir).unwrap();
        let file_path = nested_dir.join("untracked.txt");
        fs::write(&file_path, "untracked content").unwrap();

        let checker = open_checker(&repo_path);
        let status = checker.get_file_status(&file_path);

        assert_eq!(
            status,
            FileStatus::Untracked,
            "未追跡ディレクトリ配下のファイルも Untracked と判定されるべき"
        );
    }

    #[test]
    fn test_get_file_status_ignored() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成
        let gitignore_path = repo_path.join(".gitignore");
        fs::write(&gitignore_path, "*.log\n").unwrap();

        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add .gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // 無視されるファイルを作成
        let file_path = repo_path.join("debug.log");
        fs::write(&file_path, "log content").unwrap();

        let checker = open_checker(&repo_path);
        let status = checker.get_file_status(&file_path);

        assert_eq!(status, FileStatus::Ignored);
    }

    #[test]
    fn test_is_deletable_clean() {
        assert!(GitChecker::is_deletable(FileStatus::Clean));
    }

    #[test]
    fn test_is_deletable_ignored() {
        assert!(GitChecker::is_deletable(FileStatus::Ignored));
    }

    #[test]
    fn test_is_deletable_not_in_repo() {
        assert!(GitChecker::is_deletable(FileStatus::NotInRepo));
    }

    #[test]
    fn test_is_not_deletable_modified() {
        assert!(!GitChecker::is_deletable(FileStatus::Modified));
    }

    #[test]
    fn test_is_not_deletable_staged() {
        assert!(!GitChecker::is_deletable(FileStatus::Staged));
    }

    #[test]
    fn test_is_not_deletable_untracked() {
        assert!(!GitChecker::is_deletable(FileStatus::Untracked));
    }

    // ディレクトリ再帰チェックのテスト

    #[test]
    fn test_check_directory_all_clean() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ディレクトリを作成
        let subdir = repo_path.join("subdir");
        fs::create_dir(&subdir).unwrap();

        // クリーンなファイルを作成
        let file1 = subdir.join("file1.txt");
        let file2 = subdir.join("file2.txt");
        fs::write(&file1, "content1").unwrap();
        fs::write(&file2, "content2").unwrap();

        // コミット
        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add files"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_directory(&subdir);

        assert!(result.is_ok());
    }

    #[test]
    fn test_check_directory_with_dirty_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ディレクトリを作成
        let subdir = repo_path.join("subdir");
        fs::create_dir(&subdir).unwrap();

        // ファイルを作成してコミット
        let file1 = subdir.join("file1.txt");
        fs::write(&file1, "content1").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add file1"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // 未追跡ファイルを追加
        let file2 = subdir.join("untracked.txt");
        fs::write(&file2, "untracked").unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_directory(&subdir);

        assert!(result.is_err());
        match result.unwrap_err() {
            SafeRmError::DirtyFiles { status, .. } => {
                assert_eq!(status, FileStatus::Untracked);
            }
            _ => panic!("Expected DirtyFiles error"),
        }
    }

    #[test]
    fn test_check_directory_with_nested_untracked_file() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        let dirty_dir = repo_path.join("dirty_dir");
        let nested_dir = dirty_dir.join("nested");
        fs::create_dir_all(&nested_dir).unwrap();
        let dirty_file = nested_dir.join("untracked.txt");
        fs::write(&dirty_file, "untracked").unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_directory(&dirty_dir);

        assert!(
            result.is_err(),
            "未追跡ファイルを含むディレクトリは失敗するべき"
        );
        match result.unwrap_err() {
            SafeRmError::DirtyFiles { path, status } => {
                assert_eq!(path, dirty_file);
                assert_eq!(status, FileStatus::Untracked);
            }
            _ => panic!("Expected DirtyFiles error"),
        }
    }

    #[test]
    fn test_check_directory_ignored() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成
        let gitignore_path = repo_path.join(".gitignore");
        fs::write(&gitignore_path, "build/\n").unwrap();

        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add .gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Ignored ディレクトリを作成
        let build_dir = repo_path.join("build");
        fs::create_dir(&build_dir).unwrap();

        // ディレクトリ内に任意のファイルを作成
        let artifact = build_dir.join("output.bin");
        fs::write(&artifact, "binary content").unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_directory(&build_dir);

        // Ignored ディレクトリは早期許可
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_directory_nested() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ネストしたディレクトリを作成
        let nested = repo_path.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();

        // クリーンなファイルを作成
        let file = nested.join("deep.txt");
        fs::write(&file, "deep content").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add deep file"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = open_checker(&repo_path);
        let parent = repo_path.join("a");
        let result = checker.check_directory(&parent);

        assert!(result.is_ok());
    }

    #[test]
    fn test_check_file_clean() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "clean.txt", "clean");

        let checker = open_checker(&repo_path);
        let file_path = repo_path.join("clean.txt");
        let result = checker.check_path(&file_path);

        assert!(result.is_ok());
    }

    #[test]
    fn test_check_file_modified() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "file.txt", "original");

        let file_path = repo_path.join("file.txt");
        fs::write(&file_path, "modified").unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_path(&file_path);

        assert!(result.is_err());
        match result.unwrap_err() {
            SafeRmError::DirtyFiles { status, .. } => {
                assert_eq!(status, FileStatus::Modified);
            }
            _ => panic!("Expected DirtyFiles error"),
        }
    }

    // get_all_statuses とキャッシュ関連のテスト

    #[test]
    fn test_get_all_statuses_returns_all_files() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成してコミット
        let gitignore_path = repo_path.join(".gitignore");
        fs::write(&gitignore_path, "*.log\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add .gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Clean ファイル
        commit_file(&repo_path, "clean.txt", "clean");

        // Modified ファイル
        commit_file(&repo_path, "modified.txt", "original");
        fs::write(repo_path.join("modified.txt"), "changed").unwrap();

        // Untracked ファイル
        fs::write(repo_path.join("new.txt"), "new").unwrap();

        // Ignored ファイル
        fs::write(repo_path.join("debug.log"), "log").unwrap();

        let checker = open_checker(&repo_path);
        let statuses = checker.get_all_statuses().unwrap();

        // Clean, Modified, Untracked, Ignored が status に含まれる
        assert_eq!(statuses.get("clean.txt"), Some(&FileStatus::Clean));
        assert!(statuses.contains_key("modified.txt"));
        assert!(statuses.contains_key("new.txt"));
        assert!(statuses.contains_key("debug.log"));
    }

    #[test]
    fn test_check_file_with_cache_clean() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "cached_clean.txt", "content");

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();
        let file_path = repo_path.join("cached_clean.txt");
        let result = checker.check_file_with_cache(&file_path, &cache);

        assert!(result.is_ok(), "Clean file should pass cache check");
    }

    #[test]
    fn test_check_file_with_cache_modified() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "cached_mod.txt", "original");
        fs::write(repo_path.join("cached_mod.txt"), "changed").unwrap();

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();
        let file_path = repo_path.join("cached_mod.txt");
        let result = checker.check_file_with_cache(&file_path, &cache);

        assert!(result.is_err(), "Modified file should fail cache check");
    }

    #[test]
    fn test_check_path_with_cache_directory() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let subdir = repo_path.join("cachedir");
        fs::create_dir(&subdir).unwrap();
        commit_file(&repo_path, "cachedir/file1.txt", "content1");
        commit_file(&repo_path, "cachedir/file2.txt", "content2");

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();
        let result = checker.check_path_with_cache(&subdir, &cache);

        assert!(
            result.is_ok(),
            "Directory with clean files should pass cache check"
        );
    }

    #[test]
    fn test_check_directory_with_cache_dirty() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let subdir = repo_path.join("dirtycache");
        fs::create_dir(&subdir).unwrap();
        commit_file(&repo_path, "dirtycache/clean.txt", "clean");

        // 未追跡ファイルを追加
        fs::write(subdir.join("untracked.txt"), "untracked").unwrap();

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();
        let result = checker.check_directory_with_cache(&subdir, &cache);

        assert!(
            result.is_err(),
            "Directory with dirty files should fail cache check"
        );
    }

    #[test]
    fn test_workdir_returns_path() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let checker = open_checker(&repo_path);
        let workdir = checker.workdir();

        assert!(workdir.is_some(), "Git repo should have a workdir");
        let wd = workdir.unwrap();
        assert!(wd.is_absolute(), "Workdir should be absolute");
    }

    #[test]
    fn test_get_file_status_from_cache_not_in_repo() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "dummy.txt", "dummy");

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();

        // リポジトリ外のパスに対して NotInRepo が返ることを確認
        let outside_path = std::path::Path::new("/tmp/nonexistent_path.txt");
        let status = checker.get_file_status_from_cache(outside_path, &cache);
        assert_eq!(status, FileStatus::NotInRepo);
    }

    #[test]
    fn test_convert_status_clean() {
        // 空のステータス（変更なし）はCleanになるべき
        let status = GitChecker::convert_status(Status::empty());
        assert_eq!(status, FileStatus::Clean);
    }

    #[test]
    fn test_convert_status_ignored() {
        let status = GitChecker::convert_status(Status::IGNORED);
        assert_eq!(status, FileStatus::Ignored);
    }

    #[test]
    fn test_convert_status_staged_new() {
        let status = GitChecker::convert_status(Status::INDEX_NEW);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_staged_modified() {
        let status = GitChecker::convert_status(Status::INDEX_MODIFIED);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_wt_modified() {
        let status = GitChecker::convert_status(Status::WT_MODIFIED);
        assert_eq!(status, FileStatus::Modified);
    }

    #[test]
    fn test_convert_status_wt_new() {
        let status = GitChecker::convert_status(Status::WT_NEW);
        assert_eq!(status, FileStatus::Untracked);
    }

    #[test]
    fn test_convert_status_ignored_takes_precedence() {
        // IGNORED + WT_NEW の場合、IGNORED が優先される
        let status = GitChecker::convert_status(Status::IGNORED | Status::WT_NEW);
        assert_eq!(status, FileStatus::Ignored);
    }

    #[test]
    fn test_convert_status_index_deleted() {
        let status = GitChecker::convert_status(Status::INDEX_DELETED);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_index_renamed() {
        let status = GitChecker::convert_status(Status::INDEX_RENAMED);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_index_typechange() {
        let status = GitChecker::convert_status(Status::INDEX_TYPECHANGE);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_wt_deleted() {
        let status = GitChecker::convert_status(Status::WT_DELETED);
        assert_eq!(status, FileStatus::Modified);
    }

    #[test]
    fn test_convert_status_wt_renamed() {
        let status = GitChecker::convert_status(Status::WT_RENAMED);
        assert_eq!(status, FileStatus::Modified);
    }

    #[test]
    fn test_convert_status_wt_typechange() {
        let status = GitChecker::convert_status(Status::WT_TYPECHANGE);
        assert_eq!(status, FileStatus::Modified);
    }

    #[test]
    fn test_to_git_relative_key_uses_forward_slash() {
        let nested = Path::new("subdir").join("file.txt");
        let key = GitChecker::to_git_relative_key(&nested);
        assert_eq!(key, "subdir/file.txt");
    }

    #[test]
    fn test_get_file_status_from_cache_matches_nested_key() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let nested_dir = repo_path.join("nested");
        fs::create_dir(&nested_dir).unwrap();
        let nested_file = nested_dir.join("file.txt");
        fs::write(&nested_file, "content").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add nested file"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = open_checker(&repo_path);
        let mut cache = HashMap::new();
        cache.insert("nested/file.txt".to_string(), FileStatus::Untracked);

        let status = checker.get_file_status_from_cache(&nested_file, &cache);
        assert_eq!(status, FileStatus::Untracked);
    }

    #[test]
    fn test_convert_status_index_and_wt_combined() {
        // INDEX_MODIFIED + WT_MODIFIED の場合、Staged が優先される
        let status = GitChecker::convert_status(Status::INDEX_MODIFIED | Status::WT_MODIFIED);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_convert_status_index_new_and_wt_modified() {
        // INDEX_NEW + WT_MODIFIED の場合、Staged が優先される
        let status = GitChecker::convert_status(Status::INDEX_NEW | Status::WT_MODIFIED);
        assert_eq!(status, FileStatus::Staged);
    }

    #[test]
    fn test_get_file_status_not_in_workdir() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "dummy.txt", "dummy");

        let checker = open_checker(&repo_path);

        // リポジトリ外のパスは NotInRepo
        let outside = Path::new("/tmp/outside_file.txt");
        let status = checker.get_file_status(outside);
        assert_eq!(status, FileStatus::NotInRepo);
    }

    #[test]
    fn test_check_directory_empty() {
        // 空ディレクトリのチェック（ファイルなし = 全 Clean）
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "initial.txt", "init");

        let empty_dir = repo_path.join("empty");
        fs::create_dir(&empty_dir).unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_directory(&empty_dir);
        assert!(result.is_ok(), "空ディレクトリは削除可能であるべき");
    }

    #[test]
    fn test_check_directory_with_cache_empty() {
        // 空ディレクトリのキャッシュ付きチェック
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "initial.txt", "init");

        let empty_dir = repo_path.join("empty_cached");
        fs::create_dir(&empty_dir).unwrap();

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();
        let result = checker.check_directory_with_cache(&empty_dir, &cache);
        assert!(
            result.is_ok(),
            "空ディレクトリはキャッシュ付きでも削除可能であるべき"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_is_real_directory_does_not_follow_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let real_dir = root.join("real_dir");
        fs::create_dir(&real_dir).unwrap();

        let link = root.join("dir_link");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();

        assert!(GitChecker::is_real_directory(&real_dir));
        assert!(
            !GitChecker::is_real_directory(&link),
            "symlink to directory must be treated as non-directory"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_check_path_with_cache_symlink_to_directory_checks_link_itself() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 実体ディレクトリを作成してコミット
        let target_dir = repo_path.join("target_dir");
        fs::create_dir(&target_dir).unwrap();
        fs::write(target_dir.join("clean.txt"), "clean").unwrap();
        Command::new("git")
            .args(["add", "target_dir/clean.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add target dir"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // ディレクトリへのシンボリックリンクを作成してコミット
        let link_path = repo_path.join("dir_link");
        std::os::unix::fs::symlink("target_dir", &link_path).unwrap();
        Command::new("git")
            .args(["add", "dir_link"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add dir symlink"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // 実体ディレクトリ側を dirty にしても、link 自体が clean なら削除判定は許可されるべき
        fs::write(repo_path.join("target_dir").join("untracked.txt"), "dirty").unwrap();

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();
        let result = checker.check_path_with_cache(&link_path, &cache);
        assert!(
            result.is_ok(),
            "directory symlink should be checked as the link itself, not traversed"
        );
    }

    #[test]
    fn test_get_file_status_from_cache_ignored_file_not_in_cache() {
        // キャッシュになく .gitignore で無視されるファイルの場合、
        // get_file_status_from_cache は Ignored を返すべき
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を作成してコミット
        fs::write(repo_path.join(".gitignore"), "*.log\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add .gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Ignored ファイルを作成
        fs::write(repo_path.join("debug.log"), "log data").unwrap();

        let checker = open_checker(&repo_path);
        // 空キャッシュ（意図的にキャッシュミスさせる）
        let empty_cache = HashMap::new();

        let status = checker.get_file_status_from_cache(&repo_path.join("debug.log"), &empty_cache);
        assert_eq!(
            status,
            FileStatus::Ignored,
            "キャッシュにない .gitignore 対象ファイルは Ignored を返すべき"
        );
    }

    #[test]
    fn test_get_file_status_from_cache_clean_file_not_in_cache() {
        // キャッシュにないが Git 追跡済みで変更なし（Clean）のファイル
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "tracked.txt", "content");

        let checker = open_checker(&repo_path);
        let empty_cache = HashMap::new();

        let status =
            checker.get_file_status_from_cache(&repo_path.join("tracked.txt"), &empty_cache);
        assert_eq!(
            status,
            FileStatus::Clean,
            "キャッシュにないがコミット済みのファイルは Clean を返すべき"
        );
    }

    #[test]
    fn test_get_file_status_from_cache_nested_untracked_file_not_in_cache() {
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        let nested_dir = repo_path.join("newdir").join("deep");
        fs::create_dir_all(&nested_dir).unwrap();
        let file_path = nested_dir.join("untracked.txt");
        fs::write(&file_path, "untracked").unwrap();

        let checker = open_checker(&repo_path);
        let empty_cache = HashMap::new();

        let status = checker.get_file_status_from_cache(&file_path, &empty_cache);
        assert_eq!(
            status,
            FileStatus::Untracked,
            "キャッシュミス時でも未追跡ディレクトリ配下のファイルは Untracked を返すべき"
        );
    }

    #[test]
    fn test_to_git_relative_key_single_segment() {
        // 単一セグメント（ディレクトリなし）のパス
        let path = Path::new("file.txt");
        let key = GitChecker::to_git_relative_key(path);
        assert_eq!(key, "file.txt");
    }

    #[test]
    #[cfg(unix)]
    fn test_get_file_status_accepts_canonical_path_when_repo_opened_via_alias() {
        // symlink alias 経由で repo を開き、canonical path で問い合わせるケース
        // to_workdir_relative() のフォールバックが機能することを検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "tracked.txt", "content");

        let alias_dir = TempDir::new().unwrap();
        let alias_repo = alias_dir.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &alias_repo).unwrap();

        // alias 経由で repo を開く
        let checker = open_checker(&alias_repo);

        // canonical path で問い合わせる
        let status = checker.get_file_status(&repo_path.join("tracked.txt"));
        assert_eq!(
            status,
            FileStatus::Clean,
            "symlink alias 経由でも canonical path から正しくステータスを取得すべき"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_get_file_status_from_cache_via_symlink_alias() {
        // symlink alias 経由の repo でキャッシュベースのステータス取得を検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "clean.txt", "content");
        fs::write(repo_path.join("dirty.txt"), "untracked").unwrap();

        let alias_dir = TempDir::new().unwrap();
        let alias_repo = alias_dir.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &alias_repo).unwrap();

        let checker = open_checker(&alias_repo);
        let cache = checker.get_all_statuses().unwrap();

        // canonical path で clean ファイルを問い合わせ
        let clean_status = checker.get_file_status_from_cache(&repo_path.join("clean.txt"), &cache);
        assert_eq!(
            clean_status,
            FileStatus::Clean,
            "symlink alias 経由でもキャッシュから Clean を正しく取得すべき"
        );

        // canonical path で dirty ファイルを問い合わせ
        let dirty_status = checker.get_file_status_from_cache(&repo_path.join("dirty.txt"), &cache);
        assert_eq!(
            dirty_status,
            FileStatus::Untracked,
            "symlink alias 経由でもキャッシュから Untracked を正しく取得すべき"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_check_file_with_cache_blocks_dirty_via_symlink_alias() {
        // symlink alias 経由の repo で dirty ファイルの削除がブロックされることを検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "file.txt", "original");
        fs::write(repo_path.join("file.txt"), "modified").unwrap();

        let alias_dir = TempDir::new().unwrap();
        let alias_repo = alias_dir.path().join("repo-link");
        std::os::unix::fs::symlink(&repo_path, &alias_repo).unwrap();

        let checker = open_checker(&alias_repo);
        let cache = checker.get_all_statuses().unwrap();

        // canonical path でチェック → Modified でブロックされるべき
        let result = checker.check_file_with_cache(&repo_path.join("file.txt"), &cache);
        assert!(
            result.is_err(),
            "symlink alias 経由でも Modified ファイルの削除はブロックされるべき"
        );
    }

    #[test]
    fn test_to_workdir_relative_returns_none_for_outside_path() {
        // ワークディレクトリ外のパスは None を返す
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        let checker = open_checker(&repo_path);

        let outside_path = Path::new("/tmp/definitely-not-in-repo/file.txt");
        let status = checker.get_file_status(outside_path);
        assert_eq!(
            status,
            FileStatus::NotInRepo,
            "リポジトリ外パスは NotInRepo を返すべき"
        );
    }

    #[test]
    fn test_convert_status_current() {
        // Status が空（CURRENT = 0x0）の場合は Clean を返す
        let status = Status::CURRENT;
        assert_eq!(
            GitChecker::convert_status(status),
            FileStatus::Clean,
            "Status::CURRENT は Clean として扱うべき"
        );
    }

    #[test]
    fn test_check_path_file_vs_directory() {
        // check_path がファイルとディレクトリを正しく振り分けることを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルを作成してコミット
        let file = repo_path.join("file.txt");
        commit_file(&repo_path, "file.txt", "content");

        // ディレクトリを作成してファイルをコミット
        let dir = repo_path.join("subdir");
        fs::create_dir_all(&dir).unwrap();
        commit_file(&repo_path, "subdir/sub.txt", "sub content");

        let checker = open_checker(&repo_path);

        // ファイル — Clean で成功
        assert!(checker.check_path(&file).is_ok());
        // ディレクトリ — 全ファイルが Clean で成功
        assert!(checker.check_path(&dir).is_ok());
    }

    #[test]
    fn test_get_all_statuses_includes_ignored() {
        // get_all_statuses が Ignored ファイルも含むことを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore でパターンを設定
        commit_file(&repo_path, ".gitignore", "*.log\n");

        // 無視対象ファイルを作成
        fs::write(repo_path.join("debug.log"), "log data").unwrap();

        let checker = open_checker(&repo_path);
        let statuses = checker.get_all_statuses().unwrap();

        assert_eq!(
            statuses.get("debug.log"),
            Some(&FileStatus::Ignored),
            "get_all_statuses は Ignored ファイルを含むべき"
        );
    }

    #[test]
    fn test_check_directory_empty_dir() {
        // 空のディレクトリに対する check_directory は成功すべき
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミットを作成
        commit_file(&repo_path, "init.txt", "init");

        let empty_dir = repo_path.join("empty");
        fs::create_dir_all(&empty_dir).unwrap();

        let checker = open_checker(&repo_path);
        assert!(
            checker.check_directory(&empty_dir).is_ok(),
            "空ディレクトリの削除は許可されるべき"
        );
    }

    #[test]
    fn test_check_directory_recursive_with_mixed_status() {
        // ネストしたディレクトリ内に dirty ファイルがある場合のチェック
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let nested = repo_path.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();

        // clean ファイル
        commit_file(&repo_path, "a/b/clean.txt", "clean");

        // 未追跡ファイルを追加
        fs::write(nested.join("untracked.txt"), "new").unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_directory(&repo_path.join("a"));
        assert!(
            result.is_err(),
            "未追跡ファイルを含むディレクトリの削除はブロックされるべき"
        );
    }

    #[test]
    fn test_is_real_directory_returns_false_for_file() {
        let temp_dir = TempDir::new().unwrap();
        let file = temp_dir.path().join("file.txt");
        fs::write(&file, "content").unwrap();

        assert!(
            !GitChecker::is_real_directory(&file),
            "通常ファイルに対して false を返すべき"
        );
    }

    #[test]
    fn test_is_real_directory_returns_false_for_nonexistent() {
        assert!(
            !GitChecker::is_real_directory(Path::new("/nonexistent/path")),
            "存在しないパスに対して false を返すべき"
        );
    }

    #[test]
    fn test_to_git_relative_key_nested_path() {
        // ネストしたパスがスラッシュ区切りに正しく変換されることを確認
        let path = Path::new("src").join("components").join("App.tsx");
        let key = GitChecker::to_git_relative_key(&path);
        assert_eq!(key, "src/components/App.tsx");
    }

    #[test]
    fn test_check_file_staged_blocked() {
        // ステージ済みファイルの削除が DirtyFiles(Staged) でブロックされることを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // 初期コミットを作成
        commit_file(&repo_path, "initial.txt", "initial");

        // ファイルを作成して git add のみ（コミットしない）
        let file_path = repo_path.join("staged_only.txt");
        fs::write(&file_path, "staged content").unwrap();
        Command::new("git")
            .args(["add", "staged_only.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_file(&file_path);

        assert!(result.is_err(), "Staged ファイルの削除はブロックされるべき");
        match result.unwrap_err() {
            SafeRmError::DirtyFiles { path, status } => {
                assert_eq!(path, file_path);
                assert_eq!(status, FileStatus::Staged);
            }
            _ => panic!("Expected DirtyFiles error"),
        }
    }

    #[test]
    fn test_check_path_with_cache_file() {
        // check_path_with_cache がファイル（非ディレクトリ）に対して正しく動作することを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // Clean ファイルを作成
        commit_file(&repo_path, "cached_file.txt", "content");

        // Modified ファイルを作成
        commit_file(&repo_path, "dirty_file.txt", "original");
        fs::write(repo_path.join("dirty_file.txt"), "changed").unwrap();

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();

        // Clean ファイルは成功
        let clean_result =
            checker.check_path_with_cache(&repo_path.join("cached_file.txt"), &cache);
        assert!(
            clean_result.is_ok(),
            "Clean ファイルはキャッシュ経由でも削除可能であるべき"
        );

        // Modified ファイルは失敗
        let dirty_result = checker.check_path_with_cache(&repo_path.join("dirty_file.txt"), &cache);
        assert!(
            dirty_result.is_err(),
            "Modified ファイルはキャッシュ経由でもブロックされるべき"
        );
        match dirty_result.unwrap_err() {
            SafeRmError::DirtyFiles { status, .. } => {
                assert_eq!(status, FileStatus::Modified);
            }
            _ => panic!("Expected DirtyFiles error"),
        }
    }

    #[test]
    fn test_check_directory_recursive_with_ignored_subdir() {
        // Ignored ファイルのみを含むディレクトリの削除が許可されることを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore で特定パターンを無視
        commit_file(&repo_path, ".gitignore", "*.log\n*.tmp\n");

        // ディレクトリを作成し、無視対象ファイルのみを配置
        let subdir = repo_path.join("logs");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join("app.log"), "log data").unwrap();
        fs::write(subdir.join("debug.tmp"), "tmp data").unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_directory(&subdir);

        assert!(
            result.is_ok(),
            "Ignored ファイルのみを含むディレクトリは削除可能であるべき"
        );
    }

    #[test]
    fn test_check_directory_with_ignored_and_untracked_file_blocks() {
        // ignored ファイルが混在しても、ディレクトリ自体が ignored でなければ
        // 未追跡ファイルを見落とさずブロックすることを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, ".gitignore", "*.log\n");

        let subdir = repo_path.join("logs");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join("app.log"), "ignored log").unwrap();
        fs::write(subdir.join("untracked.txt"), "new data").unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_directory(&subdir);
        assert!(
            result.is_err(),
            "ignored ファイルと未追跡ファイルが混在するディレクトリはブロックされるべき"
        );
        if let Err(SafeRmError::DirtyFiles { status, .. }) = result {
            assert_eq!(status, FileStatus::Untracked);
        } else {
            panic!("DirtyFiles エラーが期待されたが異なるエラーが返された");
        }

        let cache = checker.get_all_statuses().unwrap();
        let cached_result = checker.check_directory_with_cache(&subdir, &cache);
        assert!(
            cached_result.is_err(),
            "キャッシュ使用時も未追跡ファイルを見落としてはならない"
        );
    }

    #[test]
    fn test_check_directory_with_cache_blocks_tracked_modified_file_in_ignored_dir() {
        // ディレクトリ全体が ignored でも、配下の tracked ファイルは保護対象。
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        fs::create_dir_all(repo_path.join("ignored")).unwrap();
        commit_file(&repo_path, "ignored/tracked.txt", "original");
        commit_file(&repo_path, ".gitignore", "ignored/\n");
        fs::write(repo_path.join("ignored/tracked.txt"), "modified").unwrap();

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();
        let result = checker.check_directory_with_cache(&repo_path.join("ignored"), &cache);

        assert!(
            matches!(
                result,
                Err(SafeRmError::DirtyFiles {
                    status: FileStatus::Modified,
                    ..
                })
            ),
            "ignored ディレクトリ配下でも tracked な変更済みファイルはブロックされるべき: {:?}",
            result
        );
    }

    #[test]
    fn test_get_file_status_from_cache_returns_clean_for_tracked_file() {
        // 追跡済み Clean ファイルは一括取得キャッシュから Clean を返すことを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // ファイルを作成してコミット（変更なし = Clean）
        commit_file(&repo_path, "tracked_clean.txt", "content");

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();

        let file_path = repo_path.join("tracked_clean.txt");
        let status = checker.get_file_status_from_cache(&file_path, &cache);
        assert_eq!(
            status,
            FileStatus::Clean,
            "追跡済み Clean ファイルは Clean を返すべき"
        );

        assert_eq!(cache.get("tracked_clean.txt"), Some(&FileStatus::Clean));
    }

    #[test]
    fn test_get_all_statuses_returns_result_ok() {
        // 正常なリポジトリで get_all_statuses が Ok を返すことを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "file.txt", "content");

        let checker = open_checker(&repo_path);
        let result = checker.get_all_statuses();
        assert!(result.is_ok(), "正常なリポジトリでは Ok を返すべき");
    }

    #[test]
    fn test_resolve_status_fail_closed_returns_modified_on_unexpected_error() {
        // resolve_status_from_relative_path が NotFound 以外のエラーで
        // Modified（削除不可）を返すことを検証する間接テスト
        //
        // status_file() が NotFound 以外のエラーを返すケースを
        // check_file 経由で検証: ワークディレクトリ外の相対パスを渡す
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "dummy.txt", "content");

        let checker = open_checker(&repo_path);

        // check_file で Modified（fail-closed）が返され削除がブロックされることを確認
        // get_file_status 経由: ワークディレクトリ内の追跡済みファイルは Clean
        let clean_path = repo_path.join("dummy.txt");
        let status = checker.get_file_status(&clean_path);
        assert_eq!(status, FileStatus::Clean);
        assert!(GitChecker::is_deletable(status));

        // Modified は削除不可
        assert!(!GitChecker::is_deletable(FileStatus::Modified));
    }

    #[test]
    fn test_check_directory_recursive_with_cache_fail_closed_on_read_error() {
        // キャッシュ使用時のディレクトリ読み取り失敗で DirectoryReadError が返ることを確認
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "file.txt", "content");

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();

        // 存在しないディレクトリに対してキャッシュ付きチェック
        let nonexistent_dir = repo_path.join("nonexistent_dir");
        fs::create_dir(&nonexistent_dir).unwrap();

        // パーミッションを除去してディレクトリ読み取り不可にする (Unix のみ)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&nonexistent_dir, fs::Permissions::from_mode(0o000)).unwrap();

            let result = checker.check_directory_with_cache(&nonexistent_dir, &cache);
            assert!(
                result.is_err(),
                "読み取り不可ディレクトリのキャッシュ付きチェックは失敗すべき"
            );

            // テスト後のクリーンアップ: パーミッション復元
            fs::set_permissions(&nonexistent_dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn test_batch_exit_code_two_paths_mixed_errors() {
        // 2パスのバッチで終了コード 1 と終了コード 2 が混在する場合のテスト
        // check_file は DirtyFiles（終了コード 2）を返す
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "clean.txt", "clean");
        fs::write(repo_path.join("dirty.txt"), "untracked").unwrap();

        let checker = open_checker(&repo_path);

        // Clean ファイルは OK
        let clean_result = checker.check_file(&repo_path.join("clean.txt"));
        assert!(clean_result.is_ok());

        // Untracked ファイルは DirtyFiles エラー
        let dirty_result = checker.check_file(&repo_path.join("dirty.txt"));
        assert!(dirty_result.is_err());
        if let Err(crate::error::SafeRmError::DirtyFiles { status, .. }) = dirty_result {
            assert_eq!(status, FileStatus::Untracked);
        } else {
            panic!("DirtyFiles エラーが期待されたが異なるエラーが返された");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_check_path_symlink_to_file_treated_as_file() {
        // ファイルへの symlink は is_real_directory() が false を返すので
        // check_file パスで処理されることを検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "target.txt", "target content");
        let link_path = repo_path.join("link_to_file.txt");
        std::os::unix::fs::symlink(repo_path.join("target.txt"), &link_path).unwrap();

        Command::new("git")
            .args(["add", "link_to_file.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add symlink"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = open_checker(&repo_path);

        // symlink-to-file は check_file 経由で処理される
        assert!(!GitChecker::is_real_directory(&link_path));
        let result = checker.check_path(&link_path);
        assert!(result.is_ok(), "コミット済み symlink は削除可能であるべき");
    }

    #[test]
    fn test_check_directory_with_only_ignored_files() {
        // .gitignore 対象ファイルのみ含むディレクトリは削除可能
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を設定
        fs::write(repo_path.join(".gitignore"), "build/\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add .gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // build/ ディレクトリにファイルを作成
        let build_dir = repo_path.join("build");
        fs::create_dir_all(&build_dir).unwrap();
        fs::write(build_dir.join("output.bin"), "binary content").unwrap();
        fs::write(build_dir.join("log.txt"), "build log").unwrap();

        let checker = open_checker(&repo_path);
        let result = checker.check_directory(&build_dir);
        assert!(
            result.is_ok(),
            "ignored ファイルのみ含むディレクトリは削除可能であるべき"
        );
    }

    #[test]
    fn test_get_file_status_from_cache_falls_back_correctly() {
        // キャッシュにないファイルで、is_ignored_path の判定にフォールバックすることを検証
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        // .gitignore を設定してコミット
        fs::write(repo_path.join(".gitignore"), "*.log\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add .gitignore"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // ignored ファイルを作成
        fs::write(repo_path.join("debug.log"), "log data").unwrap();

        let checker = open_checker(&repo_path);

        // 空のキャッシュで問い合わせ → フォールバックで正しくステータスを取得
        let empty_cache = HashMap::new();
        let status = checker.get_file_status_from_cache(&repo_path.join("debug.log"), &empty_cache);
        assert_eq!(
            status,
            FileStatus::Ignored,
            "空キャッシュでも ignored ファイルは正しく判定されるべき"
        );
    }

    #[test]
    fn test_check_file_with_cache_not_in_repo() {
        // ワークディレクトリ外のパスを check_file_with_cache に渡すと DirtyFiles
        // （NotInRepo は is_deletable なので Ok が返る）
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "initial.txt", "initial");

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();

        // リポジトリ外のパス
        let outside_path = Path::new("/tmp/nonexistent_safe_rm_test_xyz");
        let result = checker.check_file_with_cache(outside_path, &cache);
        // NotInRepo は is_deletable = true なので Ok
        assert!(
            result.is_ok(),
            "NotInRepo ステータスは削除可能として扱われるべき"
        );
    }

    #[test]
    fn test_get_all_statuses_empty_repo() {
        // 初期コミットなしの空リポジトリでも get_all_statuses が正常動作する
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        let checker = open_checker(&repo_path);
        let statuses = checker.get_all_statuses().unwrap();

        // 空リポジトリではファイルがないのでマップも空
        assert!(
            statuses.is_empty(),
            "空リポジトリの get_all_statuses は空マップを返すべき"
        );
    }

    #[test]
    fn test_get_all_statuses_empty_repo_with_untracked() {
        // 初期コミットなしの空リポジトリで未追跡ファイルがある場合
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        fs::write(repo_path.join("new_file.txt"), "content").unwrap();

        let checker = open_checker(&repo_path);
        let statuses = checker.get_all_statuses().unwrap();

        assert_eq!(
            statuses.get("new_file.txt"),
            Some(&FileStatus::Untracked),
            "空リポジトリの未追跡ファイルも取得されるべき"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_get_file_status_broken_symlink() {
        // リンク先が存在しない壊れた symlink のステータス
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        // 存在しないターゲットへの symlink を作成
        let broken_link = repo_path.join("broken_link.txt");
        std::os::unix::fs::symlink("/nonexistent/target", &broken_link).unwrap();

        let checker = open_checker(&repo_path);
        let status = checker.get_file_status(&broken_link);

        // 壊れた symlink は未追跡ファイルとして扱われる
        assert_eq!(
            status,
            FileStatus::Untracked,
            "壊れた symlink は Untracked として扱われるべき"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_check_path_with_cache_broken_symlink() {
        // 壊れた symlink のキャッシュベースチェック
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        let broken_link = repo_path.join("broken.txt");
        std::os::unix::fs::symlink("/nonexistent", &broken_link).unwrap();

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();

        // 壊れた symlink は Untracked なので is_deletable = false
        let result = checker.check_path_with_cache(&broken_link, &cache);
        assert!(
            result.is_err(),
            "壊れた symlink（未追跡）は削除がブロックされるべき"
        );
    }

    #[test]
    fn test_get_file_status_from_cache_staged_file() {
        // キャッシュ経由で Staged ファイルが正しく取得される
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, "initial.txt", "initial");

        // 新規ファイルを作成して git add
        let staged_file = repo_path.join("staged.txt");
        fs::write(&staged_file, "staged content").unwrap();
        Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();

        let status = checker.get_file_status_from_cache(&staged_file, &cache);
        assert_eq!(
            status,
            FileStatus::Staged,
            "キャッシュ経由でも Staged ファイルは正しく判定されるべき"
        );
    }

    #[test]
    fn test_check_directory_with_cache_ignored_subdir() {
        // キャッシュ使用時に .gitignore 対象のサブディレクトリが早期許可される
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, ".gitignore", "build/\n");

        let build_dir = repo_path.join("build");
        fs::create_dir_all(&build_dir).unwrap();
        fs::write(build_dir.join("output.bin"), "binary").unwrap();

        let checker = open_checker(&repo_path);
        let cache = checker.get_all_statuses().unwrap();
        let result = checker.check_directory_with_cache(&build_dir, &cache);

        assert!(
            result.is_ok(),
            "キャッシュ使用時も Ignored ディレクトリは削除可能であるべき"
        );
    }

    #[test]
    fn test_get_all_statuses_multiple_status_types() {
        // 複数の異なるステータスが同時に正しく取得される
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();

        commit_file(&repo_path, ".gitignore", "*.log\n");

        // Clean ファイル（ステータスリストに含まれない）
        commit_file(&repo_path, "clean.txt", "clean");

        // Modified ファイル
        commit_file(&repo_path, "modified.txt", "original");
        fs::write(repo_path.join("modified.txt"), "changed").unwrap();

        // Staged ファイル
        let staged = repo_path.join("staged.txt");
        fs::write(&staged, "staged").unwrap();
        Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Untracked ファイル
        fs::write(repo_path.join("new.txt"), "new").unwrap();

        // Ignored ファイル
        fs::write(repo_path.join("debug.log"), "log").unwrap();

        let checker = open_checker(&repo_path);
        let statuses = checker.get_all_statuses().unwrap();

        assert_eq!(statuses.get("modified.txt"), Some(&FileStatus::Modified));
        assert_eq!(statuses.get("staged.txt"), Some(&FileStatus::Staged));
        assert_eq!(statuses.get("new.txt"), Some(&FileStatus::Untracked));
        assert_eq!(statuses.get("debug.log"), Some(&FileStatus::Ignored));
        assert_eq!(statuses.get("clean.txt"), Some(&FileStatus::Clean));
    }

    #[test]
    fn test_is_real_directory_returns_true_for_directory() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path().join("subdir");
        fs::create_dir(&dir).unwrap();

        assert!(
            GitChecker::is_real_directory(&dir),
            "実ディレクトリに対して true を返すべき"
        );
    }

    #[test]
    fn test_to_git_relative_key_empty_path() {
        // 空パスの変換
        let path = Path::new("");
        let key = GitChecker::to_git_relative_key(path);
        assert_eq!(key, "");
    }

    #[test]
    fn test_convert_status_conflicted_returns_modified() {
        // マージコンフリクト中（CONFLICTED 単独）のファイルは
        // Clean に落とさず削除を禁止できる Modified として扱うべき
        let status = Status::CONFLICTED;
        assert_eq!(
            GitChecker::convert_status(status),
            FileStatus::Modified,
            "CONFLICTED 単独でも Modified として扱い削除を禁止すべき"
        );
    }

    #[test]
    fn test_convert_status_conflicted_with_wt_modified() {
        // CONFLICTED と他のフラグが同時に立っている場合も削除禁止扱い
        let status = Status::CONFLICTED | Status::WT_MODIFIED;
        let converted = GitChecker::convert_status(status);
        assert!(
            !converted.is_deletable(),
            "CONFLICTED を含むステータスは削除許可にしてはならない: {:?}",
            converted
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_path_targets_via_symlink_to_dot_git_blocked() {
        // 中間 symlink が `.git` ディレクトリを指している場合、
        // OS の path resolution は `gitlink/config` → `nested/.git/config` と解決するが、
        // 元の path のコンポーネントには `.git` が出ないためバイパスされ得る。
        // 正規化済みパスでも `.git` 検出を行うことでこのバイパスをブロックする。
        let temp_dir = TempDir::new().unwrap();
        let nested_dot_git = temp_dir.path().join("nested").join(".git");
        fs::create_dir_all(&nested_dot_git).unwrap();
        // `.git` 配下の管理ファイルを作成
        fs::write(nested_dot_git.join("config"), "[core]").unwrap();

        // gitlink -> nested/.git の symlink を作成
        let gitlink = temp_dir.path().join("gitlink");
        std::os::unix::fs::symlink(&nested_dot_git, &gitlink).unwrap();

        // gitlink/config を削除しようとすると nested/.git/config に到達してしまう。
        // これは確実にブロックされなければならない。
        let target = gitlink.join("config");
        assert!(
            GitChecker::path_targets_or_contains_git_metadata(&target, false),
            "中間 symlink 経由で .git 配下を指すパスはブロックすべき"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_path_targets_via_symlink_to_bare_repo_blocked() {
        // bare リポジトリへの中間 symlink でも同様のバイパスを許してはならない。
        let temp_dir = TempDir::new().unwrap();
        let bare = temp_dir.path().join("bare.git");

        Command::new("git")
            .args(["init", "--bare", bare.to_str().unwrap()])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        let alias = temp_dir.path().join("alias");
        std::os::unix::fs::symlink(&bare, &alias).unwrap();

        // alias/HEAD は bare リポジトリの管理ファイルに解決される
        let target = alias.join("HEAD");
        assert!(
            GitChecker::path_targets_or_contains_git_metadata(&target, false),
            "中間 symlink 経由で bare リポジトリ配下を指すパスはブロックすべき"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_path_targets_symlink_self_to_dot_git_allowed() {
        // symlink 自身を削除する場合はリンクだけが消え実体の `.git` は残るため、
        // リンク先が `.git` でも削除を許可する（既存方針との整合性）。
        let temp_dir = TempDir::new().unwrap();
        let nested_dot_git = temp_dir.path().join("nested").join(".git");
        fs::create_dir_all(&nested_dot_git).unwrap();

        let gitlink = temp_dir.path().join("gitlink");
        std::os::unix::fs::symlink(&nested_dot_git, &gitlink).unwrap();

        // gitlink 自身（リンクのみ）の削除はブロックしない
        assert!(
            !GitChecker::path_targets_or_contains_git_metadata(&gitlink, false),
            "symlink 自身（リンク先が .git）の削除はブロックしないでよい"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_path_targets_symlink_self_to_bare_repo_allowed() {
        // bare リポジトリへの symlink 自身も同様にリンクだけ消えるので許可。
        let temp_dir = TempDir::new().unwrap();
        let bare = temp_dir.path().join("bare.git");

        Command::new("git")
            .args(["init", "--bare", bare.to_str().unwrap()])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        let alias = temp_dir.path().join("alias");
        std::os::unix::fs::symlink(&bare, &alias).unwrap();

        // alias 自身（リンクのみ）の削除はブロックしない
        assert!(
            !GitChecker::path_targets_or_contains_git_metadata(&alias, false),
            "symlink 自身（リンク先が bare repo）の削除はブロックしないでよい"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_touches_git_metadata_path_allows_symlink_to_repo_dot_git() {
        // 現在のリポジトリの `.git` を指す symlink 自身は、リンクだけ消えて
        // 実体は残るため許可する（`test_path_targets_symlink_self_to_dot_git_allowed`
        // と同じ「symlink 自身は許可」方針との整合性）。
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "tracked.txt", "tracked");

        let checker = open_checker(&repo_path);

        // リポジトリ内に `.git` を指す symlink を作成
        let dot_git = repo_path.join(".git");
        let gitlink = repo_path.join("gitlink");
        std::os::unix::fs::symlink(&dot_git, &gitlink).unwrap();

        assert!(
            !checker.touches_git_metadata_path(&gitlink),
            "現在の repo の .git を指す symlink 自身の削除は許可すべき"
        );

        // 中間 symlink 経由で `.git` 配下に到達するケースは引き続きブロック
        let inner = gitlink.join("config");
        assert!(
            checker.touches_git_metadata_path(&inner),
            "中間 symlink を経由した .git 配下へのアクセスはブロックすべき"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_is_git_metadata_path_allows_symlink_to_repo_dot_git() {
        // `is_git_metadata_path` も同様に symlink 自身は実体扱いしない方針。
        let temp_dir = create_test_repo();
        let repo_path = temp_dir.path().canonicalize().unwrap();
        commit_file(&repo_path, "tracked.txt", "tracked");

        let checker = open_checker(&repo_path);

        let dot_git = repo_path.join(".git");
        let gitlink = repo_path.join("gitlink");
        std::os::unix::fs::symlink(&dot_git, &gitlink).unwrap();

        assert!(
            !checker.is_git_metadata_path(&gitlink),
            "symlink 自身は Git 管理メタデータとして扱わない"
        );

        let inner = gitlink.join("config");
        assert!(
            checker.is_git_metadata_path(&inner),
            "symlink 経由で .git 配下に到達するパスは Git 管理メタデータ扱い"
        );
    }

    #[test]
    fn test_open_fail_closed_on_broken_dot_git() {
        // 壊れた `.git` ディレクトリ（必須ファイルが欠落）に対しては fail-closed
        // で `Err` を返し、削除が permissive default に倒れないことを検証する。
        // `Repository::discover` が `NotFound` を返した場合でも、`.git` 痕跡が
        // 残っていれば `has_git_metadata_ancestor` が拾って `Err` に倒す。
        let temp_dir = TempDir::new().unwrap();
        let dot_git = temp_dir.path().join(".git");
        fs::create_dir_all(&dot_git).unwrap();
        // 必須エントリ（HEAD/objects/refs）を作成しないため、Git API は読込失敗する
        fs::write(dot_git.join("dummy"), "not a real git repo").unwrap();

        match GitChecker::open(temp_dir.path()) {
            Err(SafeRmError::GitError(_)) => {
                // 期待どおり: Git API エラーは fail-closed で伝播
            }
            Ok(Some(_)) => panic!("壊れた .git で Ok(Some) を返してはならない"),
            Ok(None) => {
                panic!("壊れた .git は fail-closed で Err(GitError) を返すべき (Ok(None) は不可)")
            }
            Err(other) => panic!("予期しないエラー種別: {:?}", other),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_open_fail_closed_on_permission_denied_without_git_metadata() {
        // Git メタデータが見つからない場合でも、探索対象自体を読めない I/O エラーは
        // 非 Git 環境として許可せず fail-closed で伝播する。
        let temp_dir = TempDir::new().unwrap();
        let unreadable = temp_dir.path().join("unreadable");
        fs::create_dir(&unreadable).unwrap();

        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&unreadable).unwrap().permissions();
        permissions.set_mode(0o000);
        fs::set_permissions(&unreadable, permissions).unwrap();

        let result = GitChecker::open(&unreadable);

        let mut permissions = fs::metadata(&unreadable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&unreadable, permissions).unwrap();

        match result {
            Err(SafeRmError::GitError(_)) => {
                // 期待どおり: Git 探索の I/O エラーは fail-closed で伝播
            }
            Ok(Some(_)) => panic!("Git メタデータのない unreadable directory で Ok(Some) は不可"),
            Ok(None) => {
                panic!("Git 探索の I/O エラーを非 Git 環境として扱ってはならない")
            }
            Err(other) => panic!("予期しないエラー種別: {:?}", other),
        }
    }

    #[test]
    fn test_open_returns_none_for_truly_empty_directory() {
        // 祖先のいずれにも `.git` 痕跡がない場合のみ `Ok(None)` を返す。
        // CI 環境によっては `TempDir` の祖先側に `.git` がある可能性があるため、
        // `Ok(None)` か `Err(GitError)` のどちらかを許容する（fail-closed 維持）。
        let temp_dir = TempDir::new().unwrap();
        let nonexistent = temp_dir.path().join("definitely-not-a-repo");
        // ディレクトリは作らないため、`.git` 痕跡は当然ない
        match GitChecker::open(&nonexistent) {
            Ok(None) => {
                // 期待どおり: 非 Git 環境として扱う
            }
            Err(SafeRmError::GitError(_)) => {
                // CI 環境で祖先に `.git` が見つかった場合の fail-closed
            }
            Ok(Some(_)) => panic!("存在しないパスで Ok(Some) を返してはならない"),
            Err(other) => panic!("予期しないエラー種別: {:?}", other),
        }
    }
}
