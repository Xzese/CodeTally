use crate::error::{command_error, AppError, AppResult};
use crate::models::{
    DependencyStatus, GithubIssueJson, GithubPullRequestJson, GithubRepositoryJson, GithubUser,
    Issue, PullRequest, Repository,
};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output};

#[derive(Debug, Clone, Copy, Default)]
pub struct OpenCounts {
    pub pull_requests: i64,
    pub issues: i64,
}

pub(crate) fn command_path(name: &str) -> OsString {
    let path_name = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(&path_name);
            if candidate.is_file() {
                return candidate.into_os_string();
            }
        }
    }
    let mut candidates = Vec::<PathBuf>::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/bin").join(&path_name));
        candidates.push(home.join(".cargo/bin").join(&path_name));
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin").join(&path_name),
        PathBuf::from("/usr/local/bin").join(&path_name),
        PathBuf::from("/usr/bin").join(&path_name),
    ]);
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.into_os_string())
        .unwrap_or_else(|| OsString::from(name))
}

pub fn command_exists(name: &str) -> bool {
    Command::new(command_path(name))
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub fn run(program: &str, args: &[String]) -> AppResult<Output> {
    let output = Command::new(command_path(program)).args(args).output()?;
    if !output.status.success() {
        return Err(command_error(program, output));
    }
    Ok(output)
}

pub fn dependency_status() -> DependencyStatus {
    let gh = command_exists("gh");
    let git = command_exists("git");
    let tokei = command_exists("tokei");
    let gh_authenticated = gh && auth_status().is_ok();
    let mut missing = Vec::new();
    if !gh { missing.push("gh".to_string()); }
    if !git { missing.push("git".to_string()); }
    if !tokei { missing.push("tokei".to_string()); }
    if gh && !gh_authenticated { missing.push("gh_auth".to_string()); }
    DependencyStatus { gh, git, tokei, gh_authenticated, missing }
}

pub fn auth_status() -> AppResult<()> {
    let args = vec!["auth".to_string(), "status".to_string()];
    run("gh", &args).map(|_| ())
}

pub fn current_user() -> AppResult<GithubUser> {
    let output = run("gh", &["api".into(), "user".into(), "--jq".into(), ".login".into()])?;
    let login = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if login.is_empty() {
        return Err(AppError::Command { program: "gh api user".into(), message: "GitHub did not return a login".into() });
    }
    Ok(GithubUser { login })
}

pub fn list_repositories(login: &str) -> AppResult<Vec<Repository>> {
    list_repositories_for_owner(login)
}

pub fn list_repositories_for_owner(owner: &str) -> AppResult<Vec<Repository>> {
    let fields = "id,name,nameWithOwner,url,sshUrl,isPrivate,isFork,isArchived,stargazerCount,forkCount,defaultBranchRef,primaryLanguage,createdAt,updatedAt,pushedAt";
    let output = run("gh", &[
        "repo".into(), "list".into(), owner.into(), "--limit".into(), "10000".into(), "--json".into(), fields.into(),
    ])?;
    let parsed: Vec<GithubRepositoryJson> = serde_json::from_slice(&output.stdout)?;
    Ok(parsed.into_iter().map(repository_from_json).collect())
}

/// Return organizations visible to the authenticated gh account. `--paginate`
/// keeps discovery complete for accounts belonging to more than one page of
/// organizations while leaving authentication entirely under gh.
pub fn list_organizations() -> AppResult<Vec<String>> {
    let output = run("gh", &[
        "api".into(), "--paginate".into(), "user/orgs".into(), "--jq".into(), ".[].login".into(),
    ])?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|login| !login.is_empty())
        .map(str::to_string)
        .collect())
}

fn repository_from_json(repo: GithubRepositoryJson) -> Repository {
    let (owner, name) = repo.name_with_owner.split_once('/').unwrap_or(("", repo.name.as_str()));
    Repository {
        id: 0,
        github_id: repo.id,
        owner: owner.to_string(),
        name: name.to_string(),
        name_with_owner: repo.name_with_owner,
        url: repo.url,
        ssh_url: repo.ssh_url,
        primary_language: repo.primary_language.map(|value| value.name),
        default_branch: repo.default_branch_ref.map(|value| value.name).unwrap_or_else(|| "main".into()),
        is_private: repo.is_private,
        is_fork: repo.is_fork,
        is_archived: repo.is_archived,
        star_count: repo.star_count,
        fork_count: repo.fork_count,
        created_at: repo.created_at,
        github_updated_at: repo.updated_at,
        pushed_at: repo.pushed_at,
        local_path: None,
        last_sync_at: None,
        last_error: None,
        loc_backfill_complete: false,
        open_pr_count: 0,
        open_issue_count: 0,
        open_counts_synced: false,
        last_fetched_pushed_at: None,
    }
}

pub fn list_pull_requests(repo: &Repository) -> AppResult<Vec<PullRequest>> {
    let fields = "number,title,state,isDraft,createdAt,updatedAt,mergedAt,closedAt,url,additions,deletions,changedFiles,statusCheckRollup";
    let output = run("gh", &[
        "pr".into(), "list".into(), "--repo".into(), repo.name_with_owner.clone(), "--state".into(), "all".into(), "--search".into(), "sort:updated-desc".into(), "--limit".into(), "1000".into(), "--json".into(), fields.into(),
    ])?;
    let parsed: Vec<GithubPullRequestJson> = serde_json::from_slice(&output.stdout)?;
    Ok(parsed.into_iter().map(|value| PullRequest {
        repository_id: repo.id,
        repository: repo.name_with_owner.clone(),
        number: value.number,
        title: value.title,
        state: value.state.to_ascii_uppercase(),
        is_draft: value.is_draft,
        created_at: value.created_at,
        updated_at: value.updated_at,
        merged_at: value.merged_at,
        closed_at: value.closed_at,
        url: value.url,
        additions: value.additions.unwrap_or(0),
        deletions: value.deletions.unwrap_or(0),
        changed_files: value.changed_files.unwrap_or(0),
        ci_state: status_state(value.status_check_rollup.as_ref()),
    }).collect())
}

pub fn list_issues(repo: &Repository) -> AppResult<Vec<Issue>> {
    let fields = "number,title,state,createdAt,updatedAt,closedAt,url,author,labels,assignees";
    let output = run("gh", &[
        "issue".into(), "list".into(), "--repo".into(), repo.name_with_owner.clone(), "--state".into(), "all".into(), "--search".into(), "sort:updated-desc".into(), "--limit".into(), "1000".into(), "--json".into(), fields.into(),
    ])?;
    let parsed: Vec<GithubIssueJson> = serde_json::from_slice(&output.stdout)?;
    Ok(parsed.into_iter().map(|value| Issue {
        repository_id: repo.id,
        repository: repo.name_with_owner.clone(),
        number: value.number,
        title: value.title,
        state: value.state.to_ascii_uppercase(),
        created_at: value.created_at,
        updated_at: value.updated_at,
        closed_at: value.closed_at,
        url: value.url,
        author: value.author.map(|actor| actor.login),
        labels: value.labels.into_iter().map(|label| label.name).collect(),
        assignees: value.assignees.into_iter().map(|actor| actor.login).collect(),
    }).collect())
}

/// Query the server-side totals separately from the activity feed. The list
/// endpoints are intentionally capped for a responsive feed, while dashboard
/// open counts must remain accurate for repositories with more than 1,000 items.
pub fn open_counts(repo: &Repository) -> AppResult<OpenCounts> {
    let query = "query($owner:String!,$name:String!){repository(owner:$owner,name:$name){pullRequests(states:OPEN){totalCount} issues(states:OPEN){totalCount}}}";
    let output = run("gh", &[
        "api".into(), "graphql".into(), "-f".into(), format!("query={query}"),
        "-F".into(), format!("owner={}", repo.owner),
        "-F".into(), format!("name={}", repo.name),
    ])?;
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let repository = value.get("data").and_then(|data| data.get("repository"));
    Ok(OpenCounts {
        pull_requests: repository.and_then(|repo| repo.get("pullRequests")).and_then(|items| items.get("totalCount")).and_then(serde_json::Value::as_i64).unwrap_or(0),
        issues: repository.and_then(|repo| repo.get("issues")).and_then(|items| items.get("totalCount")).and_then(serde_json::Value::as_i64).unwrap_or(0),
    })
}

fn status_state(value: Option<&serde_json::Value>) -> Option<String> {
    let Some(value) = value else { return None };
    let entries = value.as_array()?;
    if entries.is_empty() { return Some("none".into()); }
    let mut pending = false;
    let mut failed = false;
    for entry in entries {
        let state = entry.get("state").and_then(serde_json::Value::as_str).unwrap_or("").to_ascii_uppercase();
        let status = entry.get("status").and_then(serde_json::Value::as_str).unwrap_or("").to_ascii_uppercase();
        let conclusion = entry.get("conclusion").and_then(serde_json::Value::as_str).unwrap_or("").to_ascii_uppercase();
        if matches!(state.as_str(), "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT") || matches!(conclusion.as_str(), "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT") { failed = true; }
        if matches!(state.as_str(), "PENDING" | "EXPECTED") || matches!(status.as_str(), "QUEUED" | "IN_PROGRESS" | "PENDING") { pending = true; }
    }
    Some(if failed { "failure" } else if pending { "pending" } else { "success" }.into())
}
