use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DependencyStatus {
    pub gh: bool,
    pub git: bool,
    pub tokei: bool,
    pub gh_authenticated: bool,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GithubUser {
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Repository {
    pub id: i64,
    pub github_id: String,
    pub owner: String,
    pub name: String,
    pub name_with_owner: String,
    pub url: String,
    pub ssh_url: String,
    pub primary_language: Option<String>,
    pub default_branch: String,
    pub is_private: bool,
    pub is_fork: bool,
    pub is_archived: bool,
    pub star_count: i64,
    pub fork_count: i64,
    pub created_at: Option<String>,
    pub github_updated_at: Option<String>,
    pub pushed_at: Option<String>,
    pub local_path: Option<String>,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    pub loc_backfill_complete: bool,
    pub open_pr_count: i64,
    pub open_issue_count: i64,
    pub open_counts_synced: bool,
    pub last_fetched_pushed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepositorySummary {
    pub id: i64,
    pub github_id: String,
    pub owner: String,
    pub name: String,
    pub name_with_owner: String,
    pub url: String,
    pub primary_language: Option<String>,
    pub is_private: bool,
    pub is_fork: bool,
    pub is_archived: bool,
    pub star_count: i64,
    pub fork_count: i64,
    pub total_loc: i64,
    pub source_loc: i64,
    pub test_loc: i64,
    pub loc_change_7d: i64,
    pub loc_change_30d: i64,
    pub loc_change_90d: i64,
    pub loc_change_30d_percent: f64,
    pub open_prs: i64,
    pub open_issues: i64,
    pub last_activity: Option<String>,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    /// False means LOC has not successfully been scanned yet; zero is a valid
    /// value for an empty repository.
    pub loc_available: bool,
    pub loc_baseline_7d_available: bool,
    pub loc_baseline_30d_available: bool,
    pub loc_baseline_90d_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Snapshot {
    pub id: i64,
    pub repository_id: i64,
    pub commit_sha: String,
    pub commit_date: String,
    pub snapshot_date: String,
    pub total_loc: i64,
    pub source_loc: i64,
    pub test_loc: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HistoryPoint {
    pub snapshot_date: String,
    pub total_loc: i64,
    pub source_loc: i64,
    pub test_loc: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PullRequest {
    pub repository_id: i64,
    pub repository: String,
    pub number: i64,
    pub title: String,
    pub state: String,
    pub is_draft: bool,
    pub created_at: String,
    pub updated_at: String,
    pub merged_at: Option<String>,
    pub closed_at: Option<String>,
    pub url: String,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub ci_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Issue {
    pub repository_id: i64,
    pub repository: String,
    pub number: i64,
    pub title: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub url: String,
    pub author: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DashboardTotals {
    pub repositories: i64,
    pub total_loc: i64,
    pub source_loc: i64,
    pub test_loc: i64,
    pub loc_change_30d: i64,
    pub open_prs: i64,
    pub open_issues: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Dashboard {
    pub user: Option<GithubUser>,
    pub repositories: Vec<RepositorySummary>,
    pub totals: DashboardTotals,
    pub history: Vec<HistoryPoint>,
    pub last_sync_at: Option<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocHistory {
    pub repository_id: Option<i64>,
    pub range: String,
    pub points: Vec<HistoryPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivityFeed {
    pub kind: String,
    pub items: Vec<ActivityItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityItem {
    PullRequest(PullRequest),
    Issue(Issue),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncResult {
    pub ok: bool,
    pub message: String,
    pub repositories_synced: i64,
    pub activity_repositories_synced: i64,
    pub loc_repositories_synced: i64,
    pub loc_repositories_skipped: i64,
    pub pull_requests_synced: i64,
    pub issues_synced: i64,
    pub snapshots_created: i64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub activity_refresh_minutes: i64,
    pub lines_refresh_minutes: i64,
    pub refresh_lines_on_change: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            activity_refresh_minutes: 2,
            lines_refresh_minutes: 45,
            refresh_lines_on_change: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncProgress {
    pub running: bool,
    pub phase: String,
    pub current: i64,
    pub total: i64,
    pub repository_name: Option<String>,
    pub message: String,
    pub error: Option<String>,
    pub repository_current: i64,
    pub repository_total: i64,
    pub snapshot_current: i64,
    pub snapshot_total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationConfig {
    #[serde(default)]
    pub test_paths: Vec<String>,
    #[serde(default)]
    pub test_patterns: Vec<String>,
    #[serde(default)]
    pub excluded_directories: Vec<String>,
}

impl Default for ClassificationConfig {
    fn default() -> Self {
        Self {
            test_paths: ["test", "tests", "Tests", "__tests__", "spec", "specs"].into_iter().map(str::to_string).collect(),
            test_patterns: ["*.test.ts", "*.test.tsx", "*.test.js", "*.test.jsx", "*.spec.ts", "*.spec.tsx", "*.spec.js", "*.spec.jsx", "test_*.py", "*_test.py", "*Tests.swift", "*Test.swift", "*_test.go", "*_test.rs"].into_iter().map(str::to_string).collect(),
            excluded_directories: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GithubRepositoryJson {
    #[serde(deserialize_with = "string_or_number")]
    pub id: String,
    pub name: String,
    #[serde(rename = "nameWithOwner")]
    pub name_with_owner: String,
    pub url: String,
    #[serde(rename = "sshUrl")]
    pub ssh_url: String,
    #[serde(rename = "isPrivate")]
    pub is_private: bool,
    #[serde(rename = "isFork")]
    pub is_fork: bool,
    #[serde(rename = "isArchived")]
    pub is_archived: bool,
    #[serde(rename = "stargazerCount", default)]
    pub star_count: i64,
    #[serde(rename = "forkCount", default)]
    pub fork_count: i64,
    #[serde(rename = "defaultBranchRef")]
    pub default_branch_ref: Option<DefaultBranchRef>,
    #[serde(rename = "primaryLanguage")]
    pub primary_language: Option<LanguageRef>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
    #[serde(rename = "pushedAt")]
    pub pushed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DefaultBranchRef {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LanguageRef {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GithubPullRequestJson {
    pub number: i64,
    pub title: String,
    pub state: String,
    #[serde(rename = "isDraft")]
    pub is_draft: bool,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "mergedAt")]
    pub merged_at: Option<String>,
    #[serde(rename = "closedAt")]
    pub closed_at: Option<String>,
    pub url: String,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    #[serde(rename = "changedFiles")]
    pub changed_files: Option<i64>,
    #[serde(rename = "statusCheckRollup")]
    pub status_check_rollup: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GithubIssueJson {
    pub number: i64,
    pub title: String,
    pub state: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "closedAt")]
    pub closed_at: Option<String>,
    pub url: String,
    pub author: Option<GithubActor>,
    pub labels: Vec<GithubLabel>,
    pub assignees: Vec<GithubActor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GithubActor {
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GithubLabel {
    pub name: String,
}

fn string_or_number<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(value) => Ok(value),
        serde_json::Value::Number(value) => Ok(value.to_string()),
        other => Err(serde::de::Error::custom(format!("expected id string or number, got {other}"))),
    }
}
