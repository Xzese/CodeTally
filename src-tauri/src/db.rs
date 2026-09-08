use crate::error::AppResult;
use crate::models::{
    ActivityItem, ClassificationConfig, DashboardTotals, HistoryPoint, Issue, PullRequest,
    Repository, RepositorySummary, Snapshot,
};
use chrono::{Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

const LOC_ANALYSIS_VERSION: &str = "3";
const HISTORY_SAMPLING_VERSION: &str = "1";

pub struct Database {
    path: std::path::PathBuf,
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
        let conn = self.connect()?;
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
            CREATE INDEX IF NOT EXISTS idx_snapshots_commit ON code_snapshots(commit_sha);
            CREATE INDEX IF NOT EXISTS idx_observations_repo_date ON loc_observations(repository_id, observed_at);
            CREATE INDEX IF NOT EXISTS idx_prs_updated ON pull_requests(updated_at);
            CREATE INDEX IF NOT EXISTS idx_prs_repo_state ON pull_requests(repository_id, state);
            CREATE INDEX IF NOT EXISTS idx_issues_updated ON issues(updated_at);
            CREATE INDEX IF NOT EXISTS idx_issues_repo_state ON issues(repository_id, state);
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

    fn connect(&self) -> AppResult<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(conn)
    }

    pub fn set_metadata(&self, key: &str, value: &str) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute("INSERT INTO app_metadata(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])?;
        Ok(())
    }

    pub fn metadata(&self, key: &str) -> AppResult<Option<String>> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT value FROM app_metadata WHERE key=?1", [key], |row| row.get(0)).optional()?)
    }

    pub fn upsert_repository(&self, repo: &Repository) -> AppResult<i64> {
        let conn = self.connect()?;
        conn.execute(
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
                 last_fetched_pushed_at=repositories.last_fetched_pushed_at"#,
            params![
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
            ],
        )?;
        Ok(conn.query_row(
            "SELECT id FROM repositories WHERE github_id=?1",
            [&repo.github_id],
            |row| row.get(0),
        )?)
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
        let mut stmt = conn.prepare("SELECT id,github_id,owner,name,name_with_owner,url,ssh_url,default_branch,primary_language,is_private,is_fork,is_archived,star_count,fork_count,created_at,github_updated_at,pushed_at,local_path,last_sync_at,last_error,loc_backfill_complete,open_pr_count,open_issue_count,open_counts_synced,last_fetched_pushed_at FROM repositories ORDER BY name_with_owner COLLATE NOCASE")?;
        let rows = stmt.query_map([], repository_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
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
            "UPDATE repositories SET last_sync_at=?1, last_error=?2 WHERE id=?3",
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
        let snapshot: Option<Snapshot> = conn.query_row("SELECT id,repository_id,commit_sha,commit_date,snapshot_date,total_loc,source_loc,test_loc,created_at FROM code_snapshots WHERE repository_id=?1 AND snapshot_date<=?2 ORDER BY snapshot_date DESC, id DESC LIMIT 1", params![repository_id, date], snapshot_from_row).optional()?;
        let observation: Option<Snapshot> = conn.query_row("SELECT id,repository_id,commit_sha,observed_at,observed_at,total_loc,source_loc,test_loc,observed_at FROM loc_observations WHERE repository_id=?1 AND observed_at<=?2 ORDER BY observed_at DESC, id DESC LIMIT 1", params![repository_id, date], snapshot_from_row).optional()?;
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
        conn.execute(
            r#"INSERT INTO pull_requests
               (repository_id,number,title,state,is_draft,created_at,updated_at,merged_at,closed_at,url,additions,deletions,changed_files,ci_state)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
               ON CONFLICT(repository_id,number) DO UPDATE SET title=excluded.title,state=excluded.state,is_draft=excluded.is_draft,created_at=excluded.created_at,updated_at=excluded.updated_at,merged_at=excluded.merged_at,closed_at=excluded.closed_at,url=excluded.url,additions=excluded.additions,deletions=excluded.deletions,changed_files=excluded.changed_files,ci_state=excluded.ci_state"#,
            params![item.repository_id,item.number,item.title,item.state,item.is_draft,item.created_at,item.updated_at,item.merged_at,item.closed_at,item.url,item.additions,item.deletions,item.changed_files,item.ci_state],
        )?;
        Ok(())
    }

    pub fn upsert_issue(&self, item: &Issue) -> AppResult<()> {
        let conn = self.connect()?;
        conn.execute(
            r#"INSERT INTO issues
               (repository_id,number,title,state,created_at,updated_at,closed_at,url,author,labels_json,assignees_json)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
               ON CONFLICT(repository_id,number) DO UPDATE SET title=excluded.title,state=excluded.state,created_at=excluded.created_at,updated_at=excluded.updated_at,closed_at=excluded.closed_at,url=excluded.url,author=excluded.author,labels_json=excluded.labels_json,assignees_json=excluded.assignees_json"#,
            params![item.repository_id,item.number,item.title,item.state,item.created_at,item.updated_at,item.closed_at,item.url,item.author,serde_json::to_string(&item.labels)?,serde_json::to_string(&item.assignees)?],
        )?;
        Ok(())
    }

    pub fn pull_requests(&self, repository_id: Option<i64>, state: Option<&str>, limit: usize) -> AppResult<Vec<PullRequest>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare("SELECT p.repository_id,r.name_with_owner,p.number,p.title,p.state,p.is_draft,p.created_at,p.updated_at,p.merged_at,p.closed_at,p.url,p.additions,p.deletions,p.changed_files,p.ci_state FROM pull_requests p JOIN repositories r ON r.id=p.repository_id WHERE r.is_archived=0 AND (?1 IS NULL OR p.repository_id=?1) AND (?2 IS NULL OR lower(p.state)=lower(?2)) ORDER BY p.updated_at DESC LIMIT ?3")?;
        let rows = stmt.query_map(params![repository_id, state, limit as i64], pull_request_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn issues(&self, repository_id: Option<i64>, state: Option<&str>, limit: usize) -> AppResult<Vec<Issue>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare("SELECT i.repository_id,r.name_with_owner,i.number,i.title,i.state,i.created_at,i.updated_at,i.closed_at,i.url,i.author,i.labels_json,i.assignees_json FROM issues i JOIN repositories r ON r.id=i.repository_id WHERE r.is_archived=0 AND (?1 IS NULL OR i.repository_id=?1) AND (?2 IS NULL OR lower(i.state)=lower(?2)) ORDER BY i.updated_at DESC LIMIT ?3")?;
        let rows = stmt.query_map(params![repository_id, state, limit as i64], issue_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn summaries(&self) -> AppResult<Vec<RepositorySummary>> {
        let repos = self.repositories()?;
        let now = Utc::now();
        let day_7 = (now - Duration::days(7)).to_rfc3339();
        let day_30 = (now - Duration::days(30)).to_rfc3339();
        let day_90 = (now - Duration::days(90)).to_rfc3339();
        let mut output = Vec::with_capacity(repos.len());
        for repo in repos {
            let now = Utc::now().to_rfc3339();
            let current = self.measurement_at_or_before(repo.id, &now)?;
            let old_7 = self.measurement_at_or_before(repo.id, &day_7)?;
            let old_30 = self.measurement_at_or_before(repo.id, &day_30)?;
            let old_90 = self.measurement_at_or_before(repo.id, &day_90)?;
            let loc_available = current.is_some();
            let (total, source, tests) = current.as_ref().map(|x| (x.total_loc, x.source_loc, x.test_loc)).unwrap_or_default();
            let base_7 = old_7.as_ref().map(|x| x.total_loc).unwrap_or_default();
            let base_30 = old_30.as_ref().map(|x| x.total_loc).unwrap_or_default();
            let base_90 = old_90.as_ref().map(|x| x.total_loc).unwrap_or_default();
            let change_30 = if loc_available && old_30.is_some() { total - base_30 } else { 0 };
            let conn = self.connect()?;
            let latest_pr: Option<String> = conn.query_row("SELECT MAX(updated_at) FROM pull_requests WHERE repository_id=?1", [repo.id], |row| row.get(0))?;
            let latest_issue: Option<String> = conn.query_row("SELECT MAX(updated_at) FROM issues WHERE repository_id=?1", [repo.id], |row| row.get(0))?;
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
                open_prs: if repo.open_counts_synced { repo.open_pr_count } else { self.count_open_prs(repo.id)? },
                open_issues: if repo.open_counts_synced { repo.open_issue_count } else { self.count_open_issues(repo.id)? },
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

    fn count_open_prs(&self, repository_id: i64) -> AppResult<i64> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT count(*) FROM pull_requests WHERE repository_id=?1 AND upper(state)='OPEN'", [repository_id], |row| row.get(0))?)
    }

    fn count_open_issues(&self, repository_id: i64) -> AppResult<i64> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT count(*) FROM issues WHERE repository_id=?1 AND upper(state)='OPEN'", [repository_id], |row| row.get(0))?)
    }

    pub fn totals(&self, summaries: &[RepositorySummary]) -> DashboardTotals {
        summaries.iter().filter(|r| !r.is_archived).fold(DashboardTotals::default(), |mut totals, repo| {
            totals.repositories += 1;
            if !repo.is_fork {
                totals.total_loc += repo.total_loc;
                totals.source_loc += repo.source_loc;
                totals.test_loc += repo.test_loc;
                totals.loc_change_30d += repo.loc_change_30d;
            }
            totals.open_prs += repo.open_prs;
            totals.open_issues += repo.open_issues;
            totals
        })
    }

    pub fn history(&self, repository_id: Option<i64>) -> AppResult<Vec<HistoryPoint>> {
        let repos: Vec<Repository> = if let Some(id) = repository_id { self.repository(id)?.into_iter().collect() } else { self.repositories()?.into_iter().filter(|r| !r.is_archived && !r.is_fork).collect() };
        let allowed: std::collections::HashSet<i64> = repos.iter().map(|repo| repo.id).collect();
        let conn = self.connect()?;
        let mut dates = BTreeSet::new();
        let mut snapshots: HashMap<i64, Vec<Snapshot>> = HashMap::new();
        let mut stmt = conn.prepare("SELECT id,repository_id,commit_sha,commit_date,snapshot_date,total_loc,source_loc,test_loc,created_at FROM code_snapshots ORDER BY snapshot_date")?;
        for row in stmt.query_map([], snapshot_from_row)? {
            let snapshot = row?;
            if allowed.contains(&snapshot.repository_id) { dates.insert(snapshot.snapshot_date.get(..10).unwrap_or(&snapshot.snapshot_date).to_string()); snapshots.entry(snapshot.repository_id).or_default().push(snapshot); }
        }
        let mut observations: HashMap<i64, Vec<Snapshot>> = HashMap::new();
        let mut stmt = conn.prepare("SELECT id,repository_id,commit_sha,observed_at,observed_at,total_loc,source_loc,test_loc,observed_at FROM loc_observations ORDER BY observed_at")?;
        for row in stmt.query_map([], snapshot_from_row)? {
            let observation = row?;
            if allowed.contains(&observation.repository_id) { dates.insert(observation.snapshot_date.get(..10).unwrap_or(&observation.snapshot_date).to_string()); observations.entry(observation.repository_id).or_default().push(observation); }
        }
        let mut points = Vec::with_capacity(dates.len());
        for date in dates {
            let mut point = HistoryPoint { snapshot_date: date.clone(), ..HistoryPoint::default() };
            for repo in &repos {
                let mut measurement: Option<&Snapshot> = None;
                if let Some(items) = snapshots.get(&repo.id) { measurement = items.iter().filter(|item| item.snapshot_date.get(..10).unwrap_or(&item.snapshot_date) <= date.as_str()).max_by(|a, b| a.snapshot_date.cmp(&b.snapshot_date)); }
                if let Some(items) = observations.get(&repo.id) {
                    if let Some(candidate) = items.iter().filter(|item| item.snapshot_date.get(..10).unwrap_or(&item.snapshot_date) <= date.as_str()).max_by(|a, b| a.snapshot_date.cmp(&b.snapshot_date)) {
                        if measurement.map(|item| candidate.snapshot_date.get(..10).unwrap_or(&candidate.snapshot_date) >= item.snapshot_date.get(..10).unwrap_or(&item.snapshot_date)).unwrap_or(true) { measurement = Some(candidate); }
                    }
                }
                if let Some(snapshot) = measurement {
                    point.total_loc += snapshot.total_loc;
                    point.source_loc += snapshot.source_loc;
                    point.test_loc += snapshot.test_loc;
                }
            }
            points.push(point);
        }
        Ok(points)
    }

    pub fn all_activity(&self, kind: &str, repository_id: Option<i64>, state: Option<&str>, limit: usize) -> AppResult<Vec<ActivityItem>> {
        if kind.eq_ignore_ascii_case("issues") || kind.eq_ignore_ascii_case("issue") {
            Ok(self.issues(repository_id, state, limit)?.into_iter().map(ActivityItem::Issue).collect())
        } else {
            Ok(self.pull_requests(repository_id, state, limit)?.into_iter().map(ActivityItem::PullRequest).collect())
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
    Ok(PullRequest { repository_id: row.get(0)?, repository: row.get(1)?, number: row.get(2)?, title: row.get(3)?, state: row.get(4)?, is_draft: row.get(5)?, created_at: row.get(6)?, updated_at: row.get(7)?, merged_at: row.get(8)?, closed_at: row.get(9)?, url: row.get(10)?, additions: row.get(11)?, deletions: row.get(12)?, changed_files: row.get(13)?, ci_state: row.get(14)?, })
}

fn issue_from_row(row: &Row<'_>) -> rusqlite::Result<Issue> {
    let labels_json: String = row.get(10)?;
    let assignees_json: String = row.get(11)?;
    Ok(Issue { repository_id: row.get(0)?, repository: row.get(1)?, number: row.get(2)?, title: row.get(3)?, state: row.get(4)?, created_at: row.get(5)?, updated_at: row.get(6)?, closed_at: row.get(7)?, url: row.get(8)?, author: row.get(9)?, labels: serde_json::from_str(&labels_json).unwrap_or_default(), assignees: serde_json::from_str(&assignees_json).unwrap_or_default(), })
}
