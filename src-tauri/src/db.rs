use crate::error::AppResult;
use crate::models::{
    ActivityItem, ClassificationConfig, Dashboard, DashboardTotals, GithubUser, HistoryPoint, Issue, PullRequest,
    Repository, RepositorySummary, Snapshot,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension, OpenFlags, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const LOC_ANALYSIS_VERSION: &str = "3";
const HISTORY_SAMPLING_VERSION: &str = "1";
const ACTIVITY_ACTOR_FIELDS_VERSION: &str = "1";

pub struct Database {
    path: std::path::PathBuf,
}

/// The observer must remain open: SQLite data_version is comparable only on
/// the same connection. Normal writers use separate connections.
#[derive(Default)]
pub struct DashboardCache {
    observer: Option<(PathBuf, Connection)>,
    cached: Option<CachedDashboard>,
}

struct CachedDashboard {
    version: i64,
    built_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    dashboard: Arc<Dashboard>,
}

impl Database {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn init(&self) -> AppResult<()> {
        let mut conn = self.connect()?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS repositories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                github_id TEXT NOT NULL UNIQUE,
                owner TEXT NOT NULL,
                name TEXT NOT NULL,
                name_with_owner TEXT NOT NULL UNIQUE,
                url TEXT NOT NULL,
                ssh_url TEXT NOT NULL,
                default_branch TEXT NOT NULL DEFAULT 'main',
                primary_language TEXT,
                is_private INTEGER NOT NULL DEFAULT 0,
                is_fork INTEGER NOT NULL DEFAULT 0,
                is_archived INTEGER NOT NULL DEFAULT 0,
                star_count INTEGER NOT NULL DEFAULT 0,
                fork_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT,
                github_updated_at TEXT,
                pushed_at TEXT,
                local_path TEXT,
                last_sync_at TEXT,
                last_error TEXT,
                loc_backfill_complete INTEGER NOT NULL DEFAULT 0,
                open_pr_count INTEGER NOT NULL DEFAULT 0,
                open_issue_count INTEGER NOT NULL DEFAULT 0,
                open_counts_synced INTEGER NOT NULL DEFAULT 0,
                last_fetched_pushed_at TEXT
            );
            CREATE TABLE IF NOT EXISTS code_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                commit_sha TEXT NOT NULL,
                commit_date TEXT NOT NULL,
                snapshot_date TEXT NOT NULL,
                total_loc INTEGER NOT NULL DEFAULT 0,
                source_loc INTEGER NOT NULL DEFAULT 0,
                test_loc INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                UNIQUE(repository_id, commit_sha)
            );
            CREATE TABLE IF NOT EXISTS loc_observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                commit_sha TEXT NOT NULL,
                observed_at TEXT NOT NULL,
                observation_date TEXT NOT NULL,
                total_loc INTEGER NOT NULL DEFAULT 0,
                source_loc INTEGER NOT NULL DEFAULT 0,
                test_loc INTEGER NOT NULL DEFAULT 0,
                UNIQUE(repository_id, observation_date)
            );
            CREATE TABLE IF NOT EXISTS pull_requests (
                repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                number INTEGER NOT NULL,
                title TEXT NOT NULL,
                state TEXT NOT NULL,
                is_draft INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                merged_at TEXT,
                closed_at TEXT,
                url TEXT NOT NULL,
                author TEXT,
                assignees_json TEXT NOT NULL DEFAULT '[]',
                additions INTEGER NOT NULL DEFAULT 0,
                deletions INTEGER NOT NULL DEFAULT 0,
                changed_files INTEGER NOT NULL DEFAULT 0,
                ci_state TEXT,
                PRIMARY KEY(repository_id, number)
            );
            CREATE TABLE IF NOT EXISTS issues (
                repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                number INTEGER NOT NULL,
                title TEXT NOT NULL,
                state TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                closed_at TEXT,
                url TEXT NOT NULL,
                author TEXT,
                labels_json TEXT NOT NULL DEFAULT '[]',
                assignees_json TEXT NOT NULL DEFAULT '[]',
                PRIMARY KEY(repository_id, number)
            );
            CREATE TABLE IF NOT EXISTS classification_rules (
                repository_id INTEGER PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,
                test_paths_json TEXT NOT NULL,
                test_patterns_json TEXT NOT NULL,
                excluded_directories_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS app_metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_snapshots_repo_date ON code_snapshots(repository_id, snapshot_date);
            CREATE INDEX IF NOT EXISTS idx_snapshots_date ON code_snapshots(snapshot_date);
            CREATE INDEX IF NOT EXISTS idx_observations_date ON loc_observations(observed_at);
            CREATE INDEX IF NOT EXISTS idx_snapshots_commit ON code_snapshots(commit_sha);
            CREATE INDEX IF NOT EXISTS idx_observations_repo_date ON loc_observations(repository_id, observed_at);
            CREATE INDEX IF NOT EXISTS idx_prs_updated ON pull_requests(updated_at);
            CREATE INDEX IF NOT EXISTS idx_prs_repo_state ON pull_requests(repository_id, state);
            CREATE INDEX IF NOT EXISTS idx_issues_updated ON issues(updated_at);
            CREATE INDEX IF NOT EXISTS idx_issues_repo_state ON issues(repository_id, state);
            CREATE INDEX IF NOT EXISTS idx_prs_repo_updated ON pull_requests(repository_id, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_issues_repo_updated ON issues(repository_id, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_prs_state_updated ON pull_requests(lower(state), updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_issues_state_updated ON issues(lower(state), updated_at DESC);
            "#,
        )?;
        // Keep databases created by an early development build usable.
        let _ = conn.execute("ALTER TABLE repositories ADD COLUMN loc_backfill_complete INTEGER NOT NULL DEFAULT 0", []);
        let _ = conn.execute("ALTER TABLE repositories ADD COLUMN open_pr_count INTEGER NOT NULL DEFAULT 0", []);
        let _ = conn.execute("ALTER TABLE repositories ADD COLUMN open_issue_count INTEGER NOT NULL DEFAULT 0", []);
        let _ = conn.execute("ALTER TABLE repositories ADD COLUMN open_counts_synced INTEGER NOT NULL DEFAULT 0", []);
        let _ = conn.execute("ALTER TABLE repositories ADD COLUMN last_fetched_pushed_at TEXT", []);
        let _ = conn.execute("ALTER TABLE repositories ADD COLUMN star_count INTEGER NOT NULL DEFAULT 0", []);
        let _ = conn.execute("ALTER TABLE repositories ADD COLUMN fork_count INTEGER NOT NULL DEFAULT 0", []);
        let _ = conn.execute("ALTER TABLE pull_requests ADD COLUMN author TEXT", []);
        let _ = conn.execute("ALTER TABLE pull_requests ADD COLUMN assignees_json TEXT NOT NULL DEFAULT '[]'", []);
        // Kanban data has its own additive schema version. Keep it independent
        // from LOC analysis migrations and from the disposable GitHub cache.
        let kanban_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if kanban_version < 1 {
            let tx = conn.transaction()?;
            tx.execute_batch(r#"
                ALTER TABLE pull_requests ADD COLUMN node_id TEXT;
                ALTER TABLE issues ADD COLUMN node_id TEXT;
                ALTER TABLE issues ADD COLUMN completion_reason TEXT;
                CREATE TABLE kanban_item_metadata (
                    account_scope TEXT NOT NULL,
                    item_key TEXT NOT NULL,
                    manual_column TEXT,
                    priority TEXT NOT NULL DEFAULT 'None',
                    notes TEXT NOT NULL DEFAULT '',
                    sort_rank INTEGER NOT NULL DEFAULT 0,
                    revision INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY(account_scope, item_key)
                );
                CREATE TABLE kanban_account_repositories (
                    account_scope TEXT NOT NULL,
                    github_repository_id TEXT NOT NULL,
                    PRIMARY KEY(account_scope, github_repository_id)
                );
                CREATE TABLE kanban_activity_status (
                    account_scope TEXT NOT NULL,
                    github_repository_id TEXT NOT NULL,
                    last_successful_at TEXT,
                    partial INTEGER NOT NULL DEFAULT 1,
                    error TEXT,
                    PRIMARY KEY(account_scope, github_repository_id)
                );
                CREATE TABLE kanban_links_cache (
                    account_scope TEXT NOT NULL,
                    item_key TEXT NOT NULL,
                    links_json TEXT NOT NULL,
                    partial INTEGER NOT NULL,
                    message TEXT,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY(account_scope, item_key)
                );
                INSERT OR IGNORE INTO kanban_account_repositories(account_scope, github_repository_id)
                    SELECT 'github.com:' || lower(value), github_id FROM repositories
                    CROSS JOIN app_metadata WHERE key='github_login' AND value<>'';
                PRAGMA user_version = 1;
            "#)?;
            tx.commit()?;
        }
        if kanban_version < 2 {
            let tx = conn.transaction()?;
            tx.execute_batch(r#"
                CREATE TABLE IF NOT EXISTS kanban_preferences (
                    account_scope TEXT PRIMARY KEY,
                    kind TEXT NOT NULL DEFAULT 'prs',
                    repository_scope TEXT NOT NULL DEFAULT 'all',
                    relationship TEXT NOT NULL DEFAULT 'author',
                    search TEXT NOT NULL DEFAULT '',
                    show_completed INTEGER NOT NULL DEFAULT 0,
                    updated_at TEXT NOT NULL
                );
                PRAGMA user_version = 2;
            "#)?;
            tx.commit()?;
        }
        if kanban_version < 3 {
            let tx = conn.transaction()?;
            tx.execute_batch("ALTER TABLE kanban_links_cache ADD COLUMN next_cursor TEXT; PRAGMA user_version = 3;")?;
            tx.commit()?;
        }
        if kanban_version < 4 {
            let tx = conn.transaction()?;
            tx.execute_batch(r#"
                CREATE TABLE kanban_discussion_cache (
                    account_scope TEXT NOT NULL,
                    item_key TEXT NOT NULL,
                    comments_json TEXT NOT NULL DEFAULT '[]',
                    comment_count INTEGER NOT NULL DEFAULT 0,
                    comments_cursor TEXT,
                    checks_json TEXT NOT NULL DEFAULT '[]',
                    check_count INTEGER NOT NULL DEFAULT 0,
                    checks_cursor TEXT,
                    checks_commit_oid TEXT,
                    refreshed_at TEXT,
                    PRIMARY KEY(account_scope, item_key)
                );
                PRAGMA user_version = 4;
            "#)?;
            tx.commit()?;
        }
        if kanban_version < 5 {
            let tx = conn.transaction()?;
            tx.execute_batch("ALTER TABLE kanban_discussion_cache ADD COLUMN commits_json TEXT NOT NULL DEFAULT '[]';
                              ALTER TABLE kanban_discussion_cache ADD COLUMN commit_count INTEGER NOT NULL DEFAULT 0;
                              ALTER TABLE kanban_discussion_cache ADD COLUMN commits_cursor TEXT;
                              ALTER TABLE kanban_discussion_cache ADD COLUMN last_error TEXT;
                              UPDATE kanban_discussion_cache SET refreshed_at=NULL;
                              PRAGMA user_version = 5;")?;
            tx.commit()?;
        }
        let stored_activity_actor_version: Option<String> = conn.query_row("SELECT value FROM app_metadata WHERE key='activity_actor_fields_version'", [], |row| row.get(0)).optional()?;
        if stored_activity_actor_version.as_deref() != Some(ACTIVITY_ACTOR_FIELDS_VERSION) {
            // Actor fields were added after activity was already cached. Reset
            // only closed-PR checkpoints to the oldest cached PR so the next
            // activity refresh hydrates actor data without dropping rows.
            let oldest_closed_prs = {
                let mut statement = conn.prepare("SELECT repository_id,MIN(updated_at) FROM pull_requests WHERE upper(state) IN ('CLOSED','MERGED') GROUP BY repository_id")?;
                let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            for (repository_id, updated_at) in oldest_closed_prs {
                let checkpoint = serde_json::json!({
                    "completed_at": updated_at,
                    "started_at": null,
                    "after": null,
                }).to_string();
                conn.execute(
                    "INSERT INTO app_metadata(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![format!("github_activity_v2:{repository_id}:pullRequests:false"), checkpoint],
                )?;
            }
            conn.execute("INSERT INTO app_metadata(key,value) VALUES ('activity_actor_fields_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [ACTIVITY_ACTOR_FIELDS_VERSION])?;
        }
        let stored_version: Option<String> = conn.query_row("SELECT value FROM app_metadata WHERE key='loc_analysis_version'", [], |row| row.get(0)).optional()?;
        if stored_version.as_deref() != Some(LOC_ANALYSIS_VERSION) {
            // LOC counts are derived artifacts. A scanner/parser change must
            // rebuild them automatically while retaining GitHub feeds, repo
            // metadata, and the managed clone cache.
            conn.execute("DELETE FROM code_snapshots", [])?;
            conn.execute("DELETE FROM loc_observations", [])?;
            conn.execute("UPDATE repositories SET loc_backfill_complete=0", [])?;
            conn.execute("INSERT INTO app_metadata(key,value) VALUES ('loc_analysis_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [LOC_ANALYSIS_VERSION])?;
        }
        let stored_history_version: Option<String> = conn.query_row("SELECT value FROM app_metadata WHERE key='history_sampling_version'", [], |row| row.get(0)).optional()?;
        if stored_history_version.as_deref() != Some(HISTORY_SAMPLING_VERSION) {
            // Historical samples used to begin at GitHub's repository creation
            // timestamp. Re-run the sampling pass from the first reachable
            // commit while preserving already scanned counts, observations, and
            // managed clone paths.
            conn.execute("UPDATE repositories SET loc_backfill_complete=0", [])?;
            conn.execute("INSERT INTO app_metadata(key,value) VALUES ('history_sampling_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [HISTORY_SAMPLING_VERSION])?;
        }
        Ok(())
    }

    pub(crate) fn connect(&self) -> AppResult<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(conn)
    }

    pub fn set_metadata(&self, key: &str, value: &str) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute("INSERT INTO app_metadata(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE app_metadata.value IS NOT excluded.value", params![key, value])?;
        Ok(())
    }

    pub fn metadata(&self, key: &str) -> AppResult<Option<String>> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT value FROM app_metadata WHERE key=?1", [key], |row| row.get(0)).optional()?)
    }

    pub fn upsert_repository(&self, repo: &Repository) -> AppResult<i64> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        Self::upsert_repository_on(&tx, repo)?;
        let id = tx.query_row("SELECT id FROM repositories WHERE github_id=?1", [&repo.github_id], |row| row.get(0))?;
        tx.commit()?;
        Ok(id)
    }

    pub fn upsert_repositories(&self, repositories: &[Repository]) -> AppResult<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        for repo in repositories { Self::upsert_repository_on(&tx, repo)?; }
        tx.commit()?;
        Ok(())
    }

    fn upsert_repository_on(conn: &Connection, repo: &Repository) -> AppResult<()> {
        // A deleted repository can be recreated under the same owner/name with
        // a new GitHub ID. Keep the old row and its snapshots/activity attached
        // to that old identity, but retire its display name before inserting the
        // new repository. Names are reusable; GitHub IDs are the cache key.
        let displaced_id: Option<i64> = conn.query_row(
            "SELECT id FROM repositories WHERE name_with_owner=?1 AND github_id<>?2",
            params![repo.name_with_owner, repo.github_id],
            |row| row.get(0),
        ).optional()?;
        if let Some(id) = displaced_id {
            conn.execute(
                "UPDATE repositories SET name_with_owner=?1,is_archived=1,last_error=?2 WHERE id=?3",
                params![
                    format!("{} [replaced local #{id}]", repo.name_with_owner),
                    "Repository name now belongs to a different GitHub repository",
                    id
                ],
            )?;
        }
        conn.prepare_cached(
            r#"INSERT INTO repositories
               (github_id, owner, name, name_with_owner, url, ssh_url, default_branch,
                primary_language, is_private, is_fork, is_archived, star_count, fork_count, created_at,
                github_updated_at, pushed_at, local_path, last_sync_at, last_error, loc_backfill_complete,
                open_pr_count, open_issue_count, open_counts_synced, last_fetched_pushed_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)
               ON CONFLICT(github_id) DO UPDATE SET
                 owner=excluded.owner, name=excluded.name, name_with_owner=excluded.name_with_owner,
                 url=excluded.url, ssh_url=excluded.ssh_url, default_branch=excluded.default_branch,
                 primary_language=excluded.primary_language, is_private=excluded.is_private,
                 is_fork=excluded.is_fork, is_archived=excluded.is_archived, created_at=excluded.created_at,
                 star_count=excluded.star_count, fork_count=excluded.fork_count,
                 github_updated_at=excluded.github_updated_at, pushed_at=excluded.pushed_at,
                 local_path=COALESCE(excluded.local_path, repositories.local_path),
                 last_sync_at=COALESCE(excluded.last_sync_at, repositories.last_sync_at),
                 last_error=COALESCE(excluded.last_error, repositories.last_error),
                 loc_backfill_complete=repositories.loc_backfill_complete,
                 open_pr_count=repositories.open_pr_count,
                 open_issue_count=repositories.open_issue_count,
                 open_counts_synced=repositories.open_counts_synced,
                 last_fetched_pushed_at=repositories.last_fetched_pushed_at
               WHERE repositories.owner IS NOT excluded.owner OR repositories.name IS NOT excluded.name OR repositories.name_with_owner IS NOT excluded.name_with_owner OR repositories.url IS NOT excluded.url OR repositories.ssh_url IS NOT excluded.ssh_url OR repositories.default_branch IS NOT excluded.default_branch OR repositories.primary_language IS NOT excluded.primary_language OR repositories.is_private IS NOT excluded.is_private OR repositories.is_fork IS NOT excluded.is_fork OR repositories.is_archived IS NOT excluded.is_archived OR repositories.created_at IS NOT excluded.created_at OR repositories.star_count IS NOT excluded.star_count OR repositories.fork_count IS NOT excluded.fork_count OR repositories.github_updated_at IS NOT excluded.github_updated_at OR repositories.pushed_at IS NOT excluded.pushed_at OR repositories.local_path IS NOT COALESCE(excluded.local_path,repositories.local_path) OR repositories.last_sync_at IS NOT COALESCE(excluded.last_sync_at,repositories.last_sync_at) OR repositories.last_error IS NOT COALESCE(excluded.last_error,repositories.last_error)"#)?.execute(params![
                repo.github_id,
                repo.owner,
                repo.name,
                repo.name_with_owner,
                repo.url,
                repo.ssh_url,
                repo.default_branch,
                repo.primary_language,
                repo.is_private,
                repo.is_fork,
                repo.is_archived,
                repo.star_count,
                repo.fork_count,
                repo.created_at,
                repo.github_updated_at,
                repo.pushed_at,
                repo.local_path,
                repo.last_sync_at,
                repo.last_error,
                repo.loc_backfill_complete,
                repo.open_pr_count,
                repo.open_issue_count,
                repo.open_counts_synced,
                repo.last_fetched_pushed_at,
            ])?;
        Ok(())
    }

    pub fn repository(&self, id: i64) -> AppResult<Option<Repository>> {
        let conn = self.connect()?;
        Ok(conn
            .query_row(
                "SELECT id,github_id,owner,name,name_with_owner,url,ssh_url,default_branch,primary_language,is_private,is_fork,is_archived,star_count,fork_count,created_at,github_updated_at,pushed_at,local_path,last_sync_at,last_error,loc_backfill_complete,open_pr_count,open_issue_count,open_counts_synced,last_fetched_pushed_at FROM repositories WHERE id=?1",
                [id],
                repository_from_row,
            )
            .optional()?)
    }

    pub fn repositories(&self) -> AppResult<Vec<Repository>> {
        let conn = self.connect()?;
        Self::repositories_on(&conn)
    }

    fn repositories_on(conn: &Connection) -> AppResult<Vec<Repository>> {
        let mut stmt = conn.prepare("SELECT id,github_id,owner,name,name_with_owner,url,ssh_url,default_branch,primary_language,is_private,is_fork,is_archived,star_count,fork_count,created_at,github_updated_at,pushed_at,local_path,last_sync_at,last_error,loc_backfill_complete,open_pr_count,open_issue_count,open_counts_synced,last_fetched_pushed_at FROM repositories ORDER BY name_with_owner COLLATE NOCASE")?;
        let rows = stmt.query_map([], repository_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The inventory remains unfiltered so disabled repositories can be reenabled.
    pub fn repository_selection(&self) -> AppResult<Vec<crate::models::RepositorySelection>> {
        let login = self.metadata("github_login")?.unwrap_or_default();
        Ok(self.repositories()?.into_iter().filter(|repo| !repo.is_archived).map(|repo| {
            let group = if repo.owner.eq_ignore_ascii_case(&login) { "personal" } else { "company" }.to_string();
            crate::models::RepositorySelection { github_id: repo.github_id, name_with_owner: repo.name_with_owner, owner: repo.owner, group }
        }).collect())
    }

    pub fn delete_repository(&self, repo: &Repository) -> AppResult<()> {
        let mut conn = self.connect()?;
        let transaction = conn.transaction()?;
        transaction.execute("DELETE FROM repositories WHERE id=?1", [repo.id])?;
        transaction.execute(
            "DELETE FROM app_metadata WHERE key=?1 OR key LIKE ?2",
            params![format!("github_repository_unavailable:{}", repo.github_id), format!("github_activity_v2:{}:%", repo.id)],
        )?;
        transaction.execute(
            "DELETE FROM app_metadata WHERE key='github_last_attempted_repository' AND value=?1",
            [repo.id.to_string()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn repository_enabled(&self, repo: &Repository) -> AppResult<bool> {
        let settings = crate::sync::app_settings(self)?;
        let login = self.metadata("github_login")?.unwrap_or_default();
        Ok(repository_selected(repo, &settings, &login))
    }

    pub fn selected_repositories(&self) -> AppResult<Vec<Repository>> {
        let settings = crate::sync::app_settings(self)?;
        let login = self.metadata("github_login")?.unwrap_or_default();
        Ok(self.repositories()?.into_iter().filter(|repo| repository_selected(repo, &settings, &login)).collect())
    }

    fn activity_repository_ids_json(&self, scope: Option<&[i64]>) -> AppResult<String> {
        let ids: Vec<i64> = self.selected_repositories()?.iter().map(|repo| repo.id)
            .filter(|id| scope.is_none_or(|scope| scope.contains(id))).collect();
        Ok(serde_json::to_string(&ids)?)
    }

    pub fn set_local_path(&self, id: i64, path: &str) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute("UPDATE repositories SET local_path=?1, last_error=NULL WHERE id=?2", params![path, id])?;
        Ok(())
    }

    pub fn rebase_local_paths(&self, old_root: &Path, new_root: &Path) -> AppResult<()> {
        if old_root == new_root {
            return Ok(());
        }
        let conn = self.connect()?;
        let paths = {
            let mut statement = conn.prepare("SELECT id, local_path FROM repositories WHERE local_path IS NOT NULL")?;
            let rows = statement
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        for (id, local_path) in paths {
            let Some(relative) = Path::new(&local_path).strip_prefix(old_root).ok() else { continue; };
            let rebased = new_root.join(relative).to_string_lossy().into_owned();
            conn.execute(
                "UPDATE repositories SET local_path=?1 WHERE id=?2",
                params![rebased, id],
            )?;
        }
        Ok(())
    }

    pub fn set_fetched_pushed_at(&self, id: i64, pushed_at: Option<&str>) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute("UPDATE repositories SET last_fetched_pushed_at=?1 WHERE id=?2", params![pushed_at, id])?;
        Ok(())
    }

    pub fn set_backfill_complete(&self, id: i64, complete: bool) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute("UPDATE repositories SET loc_backfill_complete=?1 WHERE id=?2", params![complete, id])?;
        Ok(())
    }

    pub fn set_open_counts(&self, id: i64, pull_requests: i64, issues: i64) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute("UPDATE repositories SET open_pr_count=?1, open_issue_count=?2, open_counts_synced=1 WHERE id=?3", params![pull_requests, issues, id])?;
        Ok(())
    }

    pub fn classification_config(&self, repository_id: i64) -> AppResult<ClassificationConfig> {
        let conn = self.connect()?;
        let row: Option<(String, String, String)> = conn.query_row(
            "SELECT test_paths_json,test_patterns_json,excluded_directories_json FROM classification_rules WHERE repository_id=?1",
            [repository_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        let Some((paths, patterns, excluded)) = row else { return Ok(ClassificationConfig::default()); };
        Ok(ClassificationConfig {
            test_paths: serde_json::from_str(&paths).unwrap_or_default(),
            test_patterns: serde_json::from_str(&patterns).unwrap_or_default(),
            excluded_directories: serde_json::from_str(&excluded).unwrap_or_default(),
        })
    }

    pub fn set_classification_config(&self, repository_id: i64, config: &ClassificationConfig) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO classification_rules(repository_id,test_paths_json,test_patterns_json,excluded_directories_json) VALUES (?1,?2,?3,?4) ON CONFLICT(repository_id) DO UPDATE SET test_paths_json=excluded.test_paths_json,test_patterns_json=excluded.test_patterns_json,excluded_directories_json=excluded.excluded_directories_json",
            params![repository_id, serde_json::to_string(&config.test_paths)?, serde_json::to_string(&config.test_patterns)?, serde_json::to_string(&config.excluded_directories)?],
        )?;
        // Classification changes invalidate every derived LOC count. Commit
        // deduplication must not preserve counts produced by the old rules.
        conn.execute("DELETE FROM code_snapshots WHERE repository_id=?1", [repository_id])?;
        conn.execute("DELETE FROM loc_observations WHERE repository_id=?1", [repository_id])?;
        conn.execute("UPDATE repositories SET loc_backfill_complete=0 WHERE id=?1", [repository_id])?;
        Ok(())
    }

    pub fn mark_sync(&self, id: i64, error: Option<&str>) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE repositories SET last_sync_at=CASE WHEN ?2 IS NULL THEN ?1 ELSE last_sync_at END, last_error=?2 WHERE id=?3",
            params![Utc::now().to_rfc3339(), error, id],
        )?;
        Ok(())
    }

    pub fn upsert_snapshot(&self, snapshot: &Snapshot) -> AppResult<bool> {
        let conn = self.connect()?;
        let changed = conn.execute(
            r#"INSERT INTO code_snapshots
               (repository_id,commit_sha,commit_date,snapshot_date,total_loc,source_loc,test_loc,created_at)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
               ON CONFLICT(repository_id,commit_sha) DO NOTHING"#,
            params![snapshot.repository_id, snapshot.commit_sha, snapshot.commit_date, snapshot.snapshot_date, snapshot.total_loc, snapshot.source_loc, snapshot.test_loc, snapshot.created_at],
        )?;
        Ok(changed > 0)
    }

    pub fn upsert_observation(&self, observation: &Snapshot) -> AppResult<bool> {
        let conn = self.connect()?;
        let observed_date = observation.snapshot_date.get(..10).unwrap_or(&observation.snapshot_date);
        let changed = conn.execute(
            r#"INSERT INTO loc_observations
               (repository_id,commit_sha,observed_at,observation_date,total_loc,source_loc,test_loc)
               VALUES (?1,?2,?3,?4,?5,?6,?7)
               ON CONFLICT(repository_id,observation_date) DO UPDATE SET
                 commit_sha=excluded.commit_sha, observed_at=excluded.observed_at,
                 total_loc=excluded.total_loc, source_loc=excluded.source_loc, test_loc=excluded.test_loc"#,
            params![observation.repository_id, observation.commit_sha, observation.snapshot_date, observed_date, observation.total_loc, observation.source_loc, observation.test_loc],
        )?;
        Ok(changed > 0)
    }

    pub fn has_snapshots(&self, repository_id: i64) -> AppResult<bool> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM code_snapshots WHERE repository_id=?1)", [repository_id], |row| row.get(0))?)
    }

    pub fn has_commit_snapshot(&self, repository_id: i64, sha: &str) -> AppResult<bool> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM code_snapshots WHERE repository_id=?1 AND commit_sha=?2)", params![repository_id, sha], |row| row.get(0))?)
    }

    /// Move an existing commit sample to an earlier historical date without
    /// rescanning its commit. This lets a history migration discover a month
    /// before the old GitHub creation-date range while reusing cached counts.
    pub fn move_snapshot_date_earlier(&self, repository_id: i64, sha: &str, date: &str) -> AppResult<bool> {
        let conn = self.connect()?;
        let changed = conn.execute(
            "UPDATE code_snapshots SET snapshot_date=?1 WHERE repository_id=?2 AND commit_sha=?3 AND snapshot_date>?1",
            params![date, repository_id, sha],
        )?;
        Ok(changed > 0)
    }

    pub fn snapshot_for_commit(&self, repository_id: i64, sha: &str) -> AppResult<Option<Snapshot>> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT id,repository_id,commit_sha,commit_date,snapshot_date,total_loc,source_loc,test_loc,created_at FROM code_snapshots WHERE repository_id=?1 AND commit_sha=?2 LIMIT 1", params![repository_id, sha], snapshot_from_row).optional()?)
    }

    pub fn latest_snapshot(&self, repository_id: i64) -> AppResult<Option<Snapshot>> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT id,repository_id,commit_sha,commit_date,snapshot_date,total_loc,source_loc,test_loc,created_at FROM code_snapshots WHERE repository_id=?1 ORDER BY snapshot_date DESC, id DESC LIMIT 1", [repository_id], snapshot_from_row).optional()?)
    }

    pub fn latest_observation(&self, repository_id: i64) -> AppResult<Option<Snapshot>> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT id,repository_id,commit_sha,observed_at,observed_at,total_loc,source_loc,test_loc,observed_at FROM loc_observations WHERE repository_id=?1 ORDER BY observed_at DESC, id DESC LIMIT 1", [repository_id], snapshot_from_row).optional()?)
    }

    pub fn snapshot_at_or_before(&self, repository_id: i64, date: &str) -> AppResult<Option<Snapshot>> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT id,repository_id,commit_sha,commit_date,snapshot_date,total_loc,source_loc,test_loc,created_at FROM code_snapshots WHERE repository_id=?1 AND snapshot_date<=?2 ORDER BY snapshot_date DESC, id DESC LIMIT 1", params![repository_id, date], snapshot_from_row).optional()?)
    }

    pub fn measurement_at_or_before(&self, repository_id: i64, date: &str) -> AppResult<Option<Snapshot>> {
        let conn = self.connect()?;
        Self::measurement_on(&conn, repository_id, date)
    }

    fn measurement_on(conn: &Connection, repository_id: i64, date: &str) -> AppResult<Option<Snapshot>> {
        let snapshot: Option<Snapshot> = conn.prepare_cached("SELECT id,repository_id,commit_sha,commit_date,snapshot_date,total_loc,source_loc,test_loc,created_at FROM code_snapshots WHERE repository_id=?1 AND snapshot_date<=?2 ORDER BY snapshot_date DESC, id DESC LIMIT 1")?.query_row(params![repository_id, date], snapshot_from_row).optional()?;
        let observation: Option<Snapshot> = conn.prepare_cached("SELECT id,repository_id,commit_sha,observed_at,observed_at,total_loc,source_loc,test_loc,observed_at FROM loc_observations WHERE repository_id=?1 AND observed_at<=?2 ORDER BY observed_at DESC, id DESC LIMIT 1")?.query_row(params![repository_id, date], snapshot_from_row).optional()?;
        Ok(match (snapshot, observation) {
            (Some(snapshot), Some(observation)) => if observation.snapshot_date >= snapshot.snapshot_date { Some(observation) } else { Some(snapshot) },
            (Some(snapshot), None) => Some(snapshot),
            (None, Some(observation)) => Some(observation),
            _ => None,
        })
    }

    pub fn snapshot_dates(&self, repository_id: Option<i64>) -> AppResult<Vec<String>> {
        let conn = self.connect()?;
        let mut dates = std::collections::BTreeSet::new();
        if let Some(id) = repository_id {
            let mut stmt = conn.prepare("SELECT DISTINCT snapshot_date FROM code_snapshots WHERE repository_id=?1 ORDER BY snapshot_date")?;
            for date in stmt.query_map([id], |row| row.get::<_, String>(0))? {
                dates.insert(date?);
            }
            let mut stmt = conn.prepare("SELECT DISTINCT observation_date FROM loc_observations WHERE repository_id=?1 ORDER BY observation_date")?;
            for date in stmt.query_map([id], |row| row.get::<_, String>(0))? { dates.insert(date?); }
        } else {
            let mut stmt = conn.prepare("SELECT DISTINCT snapshot_date FROM code_snapshots ORDER BY snapshot_date")?;
            for date in stmt.query_map([], |row| row.get::<_, String>(0))? {
                dates.insert(date?);
            }
            let mut stmt = conn.prepare("SELECT DISTINCT observation_date FROM loc_observations ORDER BY observation_date")?;
            for date in stmt.query_map([], |row| row.get::<_, String>(0))? { dates.insert(date?); }
        }
        Ok(dates.into_iter().collect())
    }

    pub fn upsert_pull_request(&self, item: &PullRequest) -> AppResult<()> {
        let conn = self.connect()?;
        Self::upsert_pull_request_on(&conn, item)
    }

    fn upsert_pull_request_on(conn: &Connection, item: &PullRequest) -> AppResult<()> {
        conn.prepare_cached(
            r#"INSERT INTO pull_requests
               (repository_id,number,title,state,is_draft,created_at,updated_at,merged_at,closed_at,url,author,assignees_json,additions,deletions,changed_files,ci_state)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
               ON CONFLICT(repository_id,number) DO UPDATE SET title=excluded.title,state=excluded.state,is_draft=excluded.is_draft,created_at=excluded.created_at,updated_at=excluded.updated_at,merged_at=excluded.merged_at,closed_at=excluded.closed_at,url=excluded.url,author=excluded.author,assignees_json=excluded.assignees_json,additions=excluded.additions,deletions=excluded.deletions,changed_files=excluded.changed_files,ci_state=excluded.ci_state
               WHERE pull_requests.title IS NOT excluded.title OR pull_requests.state IS NOT excluded.state OR pull_requests.is_draft IS NOT excluded.is_draft OR pull_requests.created_at IS NOT excluded.created_at OR pull_requests.updated_at IS NOT excluded.updated_at OR pull_requests.merged_at IS NOT excluded.merged_at OR pull_requests.closed_at IS NOT excluded.closed_at OR pull_requests.url IS NOT excluded.url OR pull_requests.additions IS NOT excluded.additions OR pull_requests.deletions IS NOT excluded.deletions OR pull_requests.changed_files IS NOT excluded.changed_files OR pull_requests.ci_state IS NOT excluded.ci_state OR pull_requests.author IS NOT excluded.author OR pull_requests.assignees_json IS NOT excluded.assignees_json"#)?.execute(params![item.repository_id,item.number,item.title,item.state,item.is_draft,item.created_at,item.updated_at,item.merged_at,item.closed_at,item.url,item.author,serde_json::to_string(&item.assignees)?,item.additions,item.deletions,item.changed_files,item.ci_state])?;
        Ok(())
    }

    pub fn upsert_issue(&self, item: &Issue) -> AppResult<()> {
        let conn = self.connect()?;
        Self::upsert_issue_on(&conn, item)
    }

    fn upsert_issue_on(conn: &Connection, item: &Issue) -> AppResult<()> {
        conn.prepare_cached(
            r#"INSERT INTO issues
               (repository_id,number,title,state,created_at,updated_at,closed_at,url,author,labels_json,assignees_json)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
               ON CONFLICT(repository_id,number) DO UPDATE SET title=excluded.title,state=excluded.state,created_at=excluded.created_at,updated_at=excluded.updated_at,closed_at=excluded.closed_at,url=excluded.url,author=excluded.author,labels_json=excluded.labels_json,assignees_json=excluded.assignees_json
               WHERE issues.title IS NOT excluded.title OR issues.state IS NOT excluded.state OR issues.created_at IS NOT excluded.created_at OR issues.updated_at IS NOT excluded.updated_at OR issues.closed_at IS NOT excluded.closed_at OR issues.url IS NOT excluded.url OR issues.author IS NOT excluded.author OR issues.labels_json IS NOT excluded.labels_json OR issues.assignees_json IS NOT excluded.assignees_json"#)?.execute(params![item.repository_id,item.number,item.title,item.state,item.created_at,item.updated_at,item.closed_at,item.url,item.author,serde_json::to_string(&item.labels)?,serde_json::to_string(&item.assignees)?])?;
        Ok(())
    }

    /// Publish a validated feed page and its checkpoint together. A failed
    /// commit leaves both data and cursor unchanged, so replay remains safe.
    pub fn apply_activity_page(&self, prs: &[PullRequest], issues: &[Issue], repository_id: i64, open_prs: i64, open_issues: i64, cursor_key: &str, cursor: &str) -> AppResult<()> {
        self.apply_activity_page_with_identities(prs, issues, repository_id, open_prs, open_issues, cursor_key, cursor, |_| Ok(()))
    }

    pub(crate) fn apply_activity_page_with_identities(&self, prs: &[PullRequest], issues: &[Issue], repository_id: i64, open_prs: i64, open_issues: i64, cursor_key: &str, cursor: &str, update_identities: impl FnOnce(&Connection) -> AppResult<()>) -> AppResult<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        for item in prs { Self::upsert_pull_request_on(&tx, item)?; }
        for item in issues { Self::upsert_issue_on(&tx, item)?; }
        update_identities(&tx)?;
        tx.execute("UPDATE repositories SET open_pr_count=?1, open_issue_count=?2, open_counts_synced=1 WHERE id=?3 AND (open_counts_synced<>1 OR open_pr_count<>?1 OR open_issue_count<>?2)", params![open_prs, open_issues, repository_id])?;
        tx.execute("INSERT INTO app_metadata(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE app_metadata.value IS NOT excluded.value", params![cursor_key, cursor])?;
        tx.commit()?;
        Ok(())
    }

    pub fn pull_requests(&self, repository_id: Option<i64>, state: Option<&str>, limit: usize) -> AppResult<Vec<PullRequest>> {
        self.scoped_pull_requests(repository_id, None, state, limit)
    }

    fn scoped_pull_requests(&self, repository_id: Option<i64>, scope: Option<&[i64]>, state: Option<&str>, limit: usize) -> AppResult<Vec<PullRequest>> {
        self.scoped_pull_requests_for_relationship(repository_id, scope, state, limit, "everyone", None)
    }

    fn scoped_pull_requests_for_relationship(&self, repository_id: Option<i64>, scope: Option<&[i64]>, state: Option<&str>, limit: usize, relationship: &str, login: Option<&str>) -> AppResult<Vec<PullRequest>> {
        let relationship = normalize_relationship(relationship)?;
        let relationship_filter = relationship_filter("p", relationship);
        let conn = self.connect()?;
        let repository_filter = if repository_id.is_some() { " AND p.repository_id=?1" } else { "" };
        let state_filter = match state {
            Some(value) if value.eq_ignore_ascii_case("closed") => " AND (lower(p.state) IN ('closed','merged') OR p.merged_at IS NOT NULL)",
            Some(_) => " AND lower(p.state)=lower(?2)",
            None => "",
        };
        let query = format!("SELECT p.repository_id,r.name_with_owner,p.number,p.title,p.state,p.is_draft,p.created_at,p.updated_at,p.merged_at,p.closed_at,p.url,p.author,p.assignees_json,p.additions,p.deletions,p.changed_files,p.ci_state FROM pull_requests p JOIN repositories r ON r.id=p.repository_id WHERE r.is_archived=0 AND r.id IN (SELECT value FROM json_each(?4)){repository_filter}{state_filter} AND ({relationship_filter}) ORDER BY p.updated_at DESC LIMIT ?3");
        let mut stmt = conn.prepare(&query)?;
        let rows = stmt.query_map(params![repository_id, state, limit as i64, self.activity_repository_ids_json(scope)?, login], pull_request_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn issues(&self, repository_id: Option<i64>, state: Option<&str>, limit: usize) -> AppResult<Vec<Issue>> {
        self.scoped_issues(repository_id, None, state, limit)
    }

    fn scoped_issues(&self, repository_id: Option<i64>, scope: Option<&[i64]>, state: Option<&str>, limit: usize) -> AppResult<Vec<Issue>> {
        self.scoped_issues_for_relationship(repository_id, scope, state, limit, "everyone", None)
    }

    fn scoped_issues_for_relationship(&self, repository_id: Option<i64>, scope: Option<&[i64]>, state: Option<&str>, limit: usize, relationship: &str, login: Option<&str>) -> AppResult<Vec<Issue>> {
        let relationship = normalize_relationship(relationship)?;
        let relationship_filter = relationship_filter("i", relationship);
        let conn = self.connect()?;
        let repository_filter = if repository_id.is_some() { " AND i.repository_id=?1" } else { "" };
        let state_filter = if state.is_some() { " AND lower(i.state)=lower(?2)" } else { "" };
        let query = format!("SELECT i.repository_id,r.name_with_owner,i.number,i.title,i.state,i.created_at,i.updated_at,i.closed_at,i.url,i.author,i.labels_json,i.assignees_json FROM issues i JOIN repositories r ON r.id=i.repository_id WHERE r.is_archived=0 AND r.id IN (SELECT value FROM json_each(?4)){repository_filter}{state_filter} AND ({relationship_filter}) ORDER BY i.updated_at DESC LIMIT ?3");
        let mut stmt = conn.prepare(&query)?;
        let rows = stmt.query_map(params![repository_id, state, limit as i64, self.activity_repository_ids_json(scope)?, login], issue_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn summaries(&self) -> AppResult<Vec<RepositorySummary>> {
        self.summaries_at(Utc::now())
    }

    fn summaries_at(&self, now: DateTime<Utc>) -> AppResult<Vec<RepositorySummary>> {
        let settings = crate::sync::app_settings(self)?;
        let login = self.metadata("github_login")?.unwrap_or_default();
        let conn = self.connect()?;
        let tx = conn.unchecked_transaction()?;
        let repos: Vec<_> = Self::repositories_on(&tx)?.into_iter().filter(|repo| repository_selected(repo, &settings, &login)).collect();
        let day_7 = (now - Duration::days(7)).to_rfc3339();
        let day_30 = (now - Duration::days(30)).to_rfc3339();
        let day_90 = (now - Duration::days(90)).to_rfc3339();
        let now = now.to_rfc3339();
        let mut output = Vec::with_capacity(repos.len());
        for repo in repos {
            let current = Self::measurement_on(&tx, repo.id, &now)?;
            let old_7 = Self::measurement_on(&tx, repo.id, &day_7)?;
            let old_30 = Self::measurement_on(&tx, repo.id, &day_30)?;
            let old_90 = Self::measurement_on(&tx, repo.id, &day_90)?;
            let loc_available = current.is_some();
            let (total, source, tests) = current.as_ref().map(|x| (x.total_loc, x.source_loc, x.test_loc)).unwrap_or_default();
            let base_7 = old_7.as_ref().map(|x| x.total_loc).unwrap_or_default();
            let base_30 = old_30.as_ref().map(|x| x.total_loc).unwrap_or_default();
            let base_90 = old_90.as_ref().map(|x| x.total_loc).unwrap_or_default();
            let change_30 = if loc_available && old_30.is_some() { total - base_30 } else { 0 };
            let (latest_pr, latest_issue, open_prs, open_issues): (Option<String>, Option<String>, i64, i64) = tx.prepare_cached(
                "SELECT (SELECT MAX(updated_at) FROM pull_requests WHERE repository_id=?1),
                        (SELECT MAX(updated_at) FROM issues WHERE repository_id=?1),
                        CASE WHEN ?2 THEN ?3 ELSE (SELECT count(*) FROM pull_requests WHERE repository_id=?1 AND upper(state)='OPEN') END,
                        CASE WHEN ?2 THEN ?4 ELSE (SELECT count(*) FROM issues WHERE repository_id=?1 AND upper(state)='OPEN') END"
            )?.query_row(params![repo.id, repo.open_counts_synced, repo.open_pr_count, repo.open_issue_count], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
            let last_activity = [repo.pushed_at.clone(), repo.github_updated_at.clone(), latest_pr, latest_issue].into_iter().flatten().max();
            output.push(RepositorySummary {
                id: repo.id,
                github_id: repo.github_id,
                owner: repo.owner,
                name: repo.name,
                name_with_owner: repo.name_with_owner,
                url: repo.url,
                primary_language: repo.primary_language,
                is_private: repo.is_private,
                is_fork: repo.is_fork,
                is_archived: repo.is_archived,
                star_count: repo.star_count,
                fork_count: repo.fork_count,
                total_loc: total,
                source_loc: source,
                test_loc: tests,
                loc_change_7d: if loc_available && old_7.is_some() { total - base_7 } else { 0 },
                loc_change_30d: change_30,
                loc_change_90d: if loc_available && old_90.is_some() { total - base_90 } else { 0 },
                loc_change_30d_percent: if loc_available && base_30 > 0 && old_30.is_some() { change_30 as f64 / base_30 as f64 * 100.0 } else { 0.0 },
                open_prs,
                open_issues,
                last_activity,
                last_sync_at: repo.last_sync_at,
                last_error: repo.last_error,
                loc_available,
                loc_baseline_7d_available: loc_available && old_7.is_some(),
                loc_baseline_30d_available: loc_available && old_30.is_some(),
                loc_baseline_90d_available: loc_available && old_90.is_some(),
            });
        }
        Ok(output)
    }

    pub fn dashboard_cached(&self, cache: &mut DashboardCache) -> AppResult<Arc<Dashboard>> {
        self.dashboard_cached_at(cache, Utc::now())
    }

    pub fn dashboard_cached_at(&self, cache: &mut DashboardCache, now: DateTime<Utc>) -> AppResult<Arc<Dashboard>> {
        if cache.observer.as_ref().map(|(path, _)| path) != Some(&self.path) {
            let observer = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            observer.busy_timeout(std::time::Duration::from_secs(10))?;
            cache.observer = Some((self.path.clone(), observer));
            cache.cached = None;
        }
        let observer = &cache.observer.as_ref().expect("observer initialized").1;
        let version: i64 = observer.query_row("PRAGMA data_version", [], |row| row.get(0))?;
        if let Some(cached) = &cache.cached {
            if cached.version == version && now >= cached.built_at && now < cached.expires_at {
                return Ok(Arc::clone(&cached.dashboard));
            }
        }
        let repositories: Vec<_> = self.summaries_at(now)?.into_iter().filter(|repo| !repo.is_archived).collect();
        let dashboard = Arc::new(Dashboard {
            totals: self.totals(&repositories)?,
            history: self.history(None)?,
            last_sync_at: repositories.iter().filter_map(|repo| repo.last_sync_at.clone()).max(),
            user: self.metadata("github_login")?.map(|login| GithubUser { login }),
            errors: self.metadata("org_discovery_errors")?.filter(|_| crate::sync::app_settings(self).map(|settings| settings.include_company_repositories).unwrap_or(false)).map(|value| value.lines().map(str::to_string).collect()).unwrap_or_default(),
            last_lines_refresh_at: self.metadata(crate::sync::LAST_LOC_REFRESH_METADATA_KEY)?,
            last_full_refresh_at: self.metadata(crate::sync::LAST_FULL_REFRESH_METADATA_KEY)?,
            last_activity_refresh_at: self.metadata(crate::sync::LAST_ACTIVITY_REFRESH_METADATA_KEY)?,
            last_personal_refresh_at: self.metadata(crate::sync::LAST_PERSONAL_REFRESH_METADATA_KEY)?,
            repositories,
        });
        let expires_at = Self::dashboard_expiry(observer, now)?;
        let after: i64 = observer.query_row("PRAGMA data_version", [], |row| row.get(0))?;
        // A concurrent page commit may span our independent reads. Do not admit
        // that result to the cache; the next poll will rebuild from fresh data.
        cache.cached = if version == after {
            Some(CachedDashboard { version, built_at: now, expires_at, dashboard: Arc::clone(&dashboard) })
        } else { None };
        Ok(dashboard)
    }

    fn dashboard_expiry(conn: &Connection, now: DateTime<Utc>) -> AppResult<DateTime<Utc>> {
        // Baselines move without writes. Expire at the next measurement crossing
        // any cutoff, with a short upper bound for unusual imported timestamps.
        let mut expires = now + Duration::minutes(1);
        for days in [0, 7, 30, 90] {
            let cutoff = (now - Duration::days(days)).to_rfc3339();
            for (table, column) in [("code_snapshots", "snapshot_date"), ("loc_observations", "observed_at")] {
                let query = format!("SELECT {column} FROM {table} WHERE {column}>?1 ORDER BY {column} LIMIT 1");
                let value: Option<String> = conn.prepare_cached(&query)?.query_row([&cutoff], |row| row.get(0)).optional()?;
                if let Some(value) = value {
                    let timestamp = value.parse::<DateTime<Utc>>().ok().or_else(|| {
                        NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok().map(|date| date.and_hms_opt(0, 0, 0).unwrap().and_utc())
                    });
                    let boundary = timestamp.map(|time| time + Duration::days(days));
                    // Raw SQLite date ordering can keep a Z timestamp excluded
                    // for the rest of its second versus a +00:00 cutoff. In
                    // that ambiguous second, keep rebuilding rather than reuse.
                    let next = match boundary {
                        Some(time) if time > now => time,
                        _ => now,
                    };
                    expires = expires.min(next);
                }
            }
        }
        Ok(expires)
    }

    pub fn totals(&self, summaries: &[RepositorySummary]) -> AppResult<DashboardTotals> {
        let settings = crate::sync::app_settings(self)?;
        Ok(summaries.iter().filter(|r| !r.is_archived).fold(DashboardTotals::default(), |mut totals, repo| {
            totals.repositories += 1;
            if settings.include_forks_in_totals || !repo.is_fork {
                totals.total_loc += repo.total_loc;
                totals.source_loc += repo.source_loc;
                totals.test_loc += repo.test_loc;
                totals.loc_change_30d += repo.loc_change_30d;
            }
            totals.open_prs += repo.open_prs;
            totals.open_issues += repo.open_issues;
            totals
        }))
    }

    pub fn history(&self, repository_id: Option<i64>) -> AppResult<Vec<HistoryPoint>> {
        let settings = crate::sync::app_settings(self)?;
        let repos: Vec<Repository> = self.selected_repositories()?.into_iter().filter(|repo| repository_id.map(|id| repo.id == id).unwrap_or(settings.include_forks_in_totals || !repo.is_fork)).collect();
        let allowed = serde_json::to_string(&repos.iter().map(|repo| repo.id).collect::<Vec<_>>())?;
        let conn = self.connect()?;
        // Scope in SQLite, then sweep measurements once. Observations win over
        // commit samples on the same day, even if their timestamp is earlier.
        let scope = "r.id IN (SELECT value FROM json_each(?1))";
        let query = format!(
            "SELECT day,repository_id,total_loc,source_loc,test_loc FROM (
                SELECT substr(s.snapshot_date,1,10) AS day,s.repository_id,s.total_loc,s.source_loc,s.test_loc,
                       0 AS observation,s.snapshot_date AS measured_at,s.id
                FROM code_snapshots s JOIN repositories r ON r.id=s.repository_id WHERE {scope}
                UNION ALL
                SELECT substr(o.observed_at,1,10),o.repository_id,o.total_loc,o.source_loc,o.test_loc,
                       1,o.observed_at,o.id
                FROM loc_observations o JOIN repositories r ON r.id=o.repository_id WHERE {scope}
             ) ORDER BY day,repository_id,observation,measured_at,id"
        );
        let mut stmt = conn.prepare(&query)?;
        let mut rows = stmt.query([allowed])?;
        let mut latest = HashMap::<i64, (i64, i64, i64)>::new();
        let mut current = HistoryPoint::default();
        let mut points = Vec::new();
        while let Some(row) = rows.next()? {
            let day: String = row.get(0)?;
            if current.snapshot_date != day {
                if !current.snapshot_date.is_empty() { points.push(current.clone()); }
                current.snapshot_date = day;
            }
            let repo_id: i64 = row.get(1)?;
            let counts = (row.get::<_, i64>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?);
            let previous = latest.insert(repo_id, counts).unwrap_or_default();
            current.total_loc += counts.0 - previous.0;
            current.source_loc += counts.1 - previous.1;
            current.test_loc += counts.2 - previous.2;
        }
        if !current.snapshot_date.is_empty() { points.push(current); }
        Ok(points)
    }

    pub fn all_activity(&self, kind: &str, repository_id: Option<i64>, state: Option<&str>, limit: usize) -> AppResult<Vec<ActivityItem>> {
        self.scoped_activity(kind, repository_id, None, state, limit)
    }

    pub fn scoped_activity(&self, kind: &str, repository_id: Option<i64>, scope: Option<&[i64]>, state: Option<&str>, limit: usize) -> AppResult<Vec<ActivityItem>> {
        self.scoped_activity_for_relationship(kind, repository_id, scope, state, limit, "everyone", None)
    }

    pub fn scoped_activity_for_relationship(&self, kind: &str, repository_id: Option<i64>, scope: Option<&[i64]>, state: Option<&str>, limit: usize, relationship: &str, login: Option<&str>) -> AppResult<Vec<ActivityItem>> {
        if kind.eq_ignore_ascii_case("issues") || kind.eq_ignore_ascii_case("issue") {
            Ok(self.scoped_issues_for_relationship(repository_id, scope, state, limit, relationship, login)?.into_iter().map(ActivityItem::Issue).collect())
        } else {
            Ok(self.scoped_pull_requests_for_relationship(repository_id, scope, state, limit, relationship, login)?.into_iter().map(ActivityItem::PullRequest).collect())
        }
    }
}

fn repository_from_row(row: &Row<'_>) -> rusqlite::Result<Repository> {
    Ok(Repository {
        id: row.get(0)?, github_id: row.get(1)?, owner: row.get(2)?, name: row.get(3)?, name_with_owner: row.get(4)?, url: row.get(5)?, ssh_url: row.get(6)?, default_branch: row.get(7)?, primary_language: row.get(8)?, is_private: row.get(9)?, is_fork: row.get(10)?, is_archived: row.get(11)?, star_count: row.get(12)?, fork_count: row.get(13)?, created_at: row.get(14)?, github_updated_at: row.get(15)?, pushed_at: row.get(16)?, local_path: row.get(17)?, last_sync_at: row.get(18)?, last_error: row.get(19)?, loc_backfill_complete: row.get(20)?, open_pr_count: row.get(21)?, open_issue_count: row.get(22)?, open_counts_synced: row.get(23)?, last_fetched_pushed_at: row.get(24)?,
    })
}

fn snapshot_from_row(row: &Row<'_>) -> rusqlite::Result<Snapshot> {
    Ok(Snapshot { id: row.get(0)?, repository_id: row.get(1)?, commit_sha: row.get(2)?, commit_date: row.get(3)?, snapshot_date: row.get(4)?, total_loc: row.get(5)?, source_loc: row.get(6)?, test_loc: row.get(7)?, created_at: row.get(8)? })
}

fn pull_request_from_row(row: &Row<'_>) -> rusqlite::Result<PullRequest> {
    let assignees_json: Option<String> = row.get(12)?;
    Ok(PullRequest { repository_id: row.get(0)?, repository: row.get(1)?, number: row.get(2)?, title: row.get(3)?, state: row.get(4)?, is_draft: row.get(5)?, created_at: row.get(6)?, updated_at: row.get(7)?, merged_at: row.get(8)?, closed_at: row.get(9)?, url: row.get(10)?, author: row.get(11)?, assignees: assignees_json.as_deref().and_then(|json| serde_json::from_str(json).ok()).unwrap_or_default(), additions: row.get(13)?, deletions: row.get(14)?, changed_files: row.get(15)?, ci_state: row.get(16)?, })
}

fn issue_from_row(row: &Row<'_>) -> rusqlite::Result<Issue> {
    let labels_json: String = row.get(10)?;
    let assignees_json: String = row.get(11)?;
    Ok(Issue { repository_id: row.get(0)?, repository: row.get(1)?, number: row.get(2)?, title: row.get(3)?, state: row.get(4)?, created_at: row.get(5)?, updated_at: row.get(6)?, closed_at: row.get(7)?, url: row.get(8)?, author: row.get(9)?, labels: serde_json::from_str(&labels_json).unwrap_or_default(), assignees: serde_json::from_str(&assignees_json).unwrap_or_default(), })
}

fn normalize_relationship(relationship: &str) -> AppResult<&str> {
    if relationship.eq_ignore_ascii_case("everyone") { Ok("everyone") }
    else if relationship.eq_ignore_ascii_case("author") { Ok("author") }
    else if relationship.eq_ignore_ascii_case("assignee") { Ok("assignee") }
    else if relationship.eq_ignore_ascii_case("author_or_assignee") { Ok("author_or_assignee") }
    else { Err(crate::error::AppError::InvalidArgument("relationship must be one of everyone, author, assignee, author_or_assignee".into())) }
}

fn relationship_filter(alias: &str, relationship: &str) -> String {
    match relationship {
        // Keep the login placeholder present so all relationship variants use
        // the same positional parameter list.
        "everyone" => "?5 IS NULL OR ?5 IS NOT NULL".into(),
        "author" => format!("?5 IS NOT NULL AND lower({alias}.author)=lower(?5)"),
        "assignee" => format!("?5 IS NOT NULL AND EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid({alias}.assignees_json) THEN {alias}.assignees_json ELSE '[]' END) AS assignee WHERE lower(CAST(assignee.value AS TEXT))=lower(?5))"),
        "author_or_assignee" => format!("?5 IS NOT NULL AND (lower({alias}.author)=lower(?5) OR EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid({alias}.assignees_json) THEN {alias}.assignees_json ELSE '[]' END) AS assignee WHERE lower(CAST(assignee.value AS TEXT))=lower(?5)) )"),
        _ => unreachable!("relationship validated before SQL generation"),
    }
}

fn repository_selected(repo: &Repository, settings: &crate::models::AppSettings, login: &str) -> bool {
    !repo.is_archived
        && (if repo.owner.eq_ignore_ascii_case(login) { settings.include_personal_repositories } else { settings.include_company_repositories })
        && !settings.excluded_repository_ids.contains(&repo.github_id)
}
