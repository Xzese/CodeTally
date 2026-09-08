use crate::classify::{classify_tokei_json_with_config, excluded_tokei_directories, is_test_path_with_config, is_tokei_report, tokei_report_code_by_path, tokei_report_paths};
use crate::error::{command_error, AppError, AppResult};
use crate::github::command_path;
use crate::models::Repository;
use crate::models::ClassificationConfig;
use ignore::WalkBuilder;
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const EXTERNAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Default)]
pub struct LocScan {
    pub total_loc: i64,
    pub source_loc: i64,
    pub test_loc: i64,
}

pub fn cache_path(cache_root: &Path, repo: &Repository) -> PathBuf {
    cache_root.join(safe_segment(&repo.owner)).join(safe_segment(&repo.name))
}

fn safe_segment(value: &str) -> String {
    value.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect()
}

fn run_git(path: Option<&Path>, args: &[String]) -> AppResult<Output> {
    let gh = shell_quote(&command_path("gh").to_string_lossy());
    // Git can inherit helpers from the user's global/system config. Clear the
    // list before installing the one app-managed helper so private clones use
    // the existing `gh` login without invoking unrelated credential programs.
    let mut full = vec![
        "-c".into(),
        "credential.helper=".into(),
        "-c".into(),
        format!("credential.helper=!{gh} auth git-credential"),
    ];
    if let Some(path) = path {
        full.push("-C".into());
        full.push(path.to_string_lossy().to_string());
    }
    full.extend(args.iter().cloned());
    let mut command = Command::new(command_path("git"));
    command.args(full).env("GIT_TERMINAL_PROMPT", "0");
    let output = run_command_with_timeout(command, "git")?;
    if !output.status.success() {
        return Err(command_error("git", output));
    }
    Ok(output)
}

pub fn ensure_clone(repo: &Repository, cache_root: &Path) -> AppResult<PathBuf> {
    ensure_clone_with_fetch(repo, cache_root, true)
}

/// Ensure the repository exists in the application-managed cache. Callers
/// that already know GitHub metadata is unchanged can skip the network fetch;
/// the existing `ensure_clone` API keeps its historical fetch-by-default
/// behavior for compatibility.
pub fn ensure_clone_with_fetch(repo: &Repository, cache_root: &Path, fetch: bool) -> AppResult<PathBuf> {
    let local = cache_path(cache_root, repo);
    if local.join(".git").is_dir() {
        if fetch {
            run_git(Some(&local), &["fetch".into(), "--all".into(), "--prune".into()])?;
        }
    } else {
        if local.exists() {
            std::fs::remove_dir_all(&local)?;
        }
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let source = https_clone_url(repo);
        run_git(None, &["clone".into(), source, local.to_string_lossy().to_string()])?;
    }
    checkout_branch(&local, &repo.default_branch)?;
    Ok(local)
}

fn https_clone_url(repo: &Repository) -> String {
    let url = repo.url.trim();
    if let Some(path) = url.strip_prefix("git@github.com:") {
        return format!("https://github.com/{path}");
    }
    if let Some(path) = url.strip_prefix("ssh://git@github.com/") {
        return format!("https://github.com/{path}");
    }
    if let Some(path) = url.strip_prefix("http://") {
        return format!("https://{path}");
    }
    if !url.is_empty() {
        return url.to_string();
    }
    format!("https://github.com/{}/{}", repo.owner, repo.name)
}

fn shell_quote(value: &str) -> String {
    // Git invokes `!` helpers through a shell. Single-quote the executable
    // path and escape embedded quotes so a path with spaces or shell
    // metacharacters remains one literal executable name.
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn checkout_branch(path: &Path, branch: &str) -> AppResult<()> {
    let branch = if branch.trim().is_empty() { "main" } else { branch };
    let remote = format!("origin/{branch}");
    match run_git(Some(path), &["checkout".into(), "--force".into(), "-B".into(), branch.to_string(), remote]) {
        Ok(_) => Ok(()),
        Err(remote_error) => {
            // A freshly cloned empty repository has no HEAD and no remote
            // branch to check out yet. It is a valid cache entry; callers can
            // report that it has no snapshots instead of treating this as a
            // generic Git failure. Other checkout errors still get a chance
            // to restore an existing local branch, then are returned.
            if is_empty_repository(path)? {
                return Ok(());
            }
            run_git(Some(path), &["checkout".into(), "--force".into(), branch.to_string()])?;
            let _ = remote_error;
            Ok(())
        }
    }
}

pub fn current_commit(path: &Path) -> AppResult<(String, String)> {
    current_commit_optional(path)?.ok_or_else(|| AppError::Command {
        program: "git".into(),
        message: "repository has no commits".into(),
    })
}

/// Return the current commit when the repository has one. A missing HEAD is
/// represented as `None`; unrelated Git failures still return an error.
pub fn current_commit_optional(path: &Path) -> AppResult<Option<(String, String)>> {
    let output = run_git_raw(Some(path), &["rev-parse".into(), "--verify".into(), "HEAD".into()])?;
    if !output.status.success() {
        if is_missing_head(&output) {
            return Ok(None);
        }
        return Err(command_error("git", output));
    }
    let sha = output_text(output)?;
    let date = output_text(run_git(Some(path), &["show".into(), "-s".into(), "--format=%cI".into(), sha.clone()])?)?;
    Ok(Some((sha, date)))
}

/// Return the oldest reachable commit on the repository's default branch.
/// GitHub creation timestamps can be later than an imported repository's first
/// commit, so historical sampling must use this timestamp when available.
pub fn earliest_commit(path: &Path, branch: &str) -> AppResult<Option<(String, String)>> {
    if current_commit_optional(path)?.is_none() {
        return Ok(None);
    }
    // `rev-list --reverse` follows Git's traversal order, which is not
    // guaranteed to be chronological when merges are present. Compare the
    // committer epoch of every commit reachable from the default branch so the
    // returned date is the actual earliest reachable commit date.
    let output = output_text(run_git(Some(path), &["log".into(), "--format=%H%x00%ct".into(), branch.to_string()])?)?;
    let mut earliest: Option<(String, i64)> = None;
    for line in output.lines() {
        let Some((sha, timestamp)) = line.split_once('\0') else { continue; };
        let Ok(timestamp) = timestamp.trim().parse::<i64>() else { continue; };
        if earliest.as_ref().map(|(_, current)| timestamp < *current).unwrap_or(true) {
            earliest = Some((sha.trim().to_string(), timestamp));
        }
    }
    let Some((sha, _)) = earliest else { return Ok(None); };
    if sha.is_empty() {
        return Ok(None);
    }
    let date = output_text(run_git(Some(path), &["show".into(), "-s".into(), "--format=%cI".into(), sha.clone()])?)?;
    Ok(Some((sha, date)))
}

pub fn is_empty_repository(path: &Path) -> AppResult<bool> {
    Ok(current_commit_optional(path)?.is_none())
}

pub fn commit_at_or_before(path: &Path, branch: &str, date: &str) -> AppResult<Option<(String, String)>> {
    if is_empty_repository(path)? {
        return Ok(None);
    }
    let output = run_git(Some(path), &["rev-list".into(), "-n".into(), "1".into(), format!("--before={date}"), branch.to_string()])?;
    let sha = output_text(output)?;
    if sha.is_empty() { return Ok(None); }
    let commit_date = output_text(run_git(Some(path), &["show".into(), "-s".into(), "--format=%cI".into(), sha.clone()])?)?;
    Ok(Some((sha, commit_date)))
}

pub fn scan_at_commit(path: &Path, commit: &str, restore_branch: &str) -> AppResult<LocScan> {
    scan_at_commit_with_config(path, commit, restore_branch, None)
}

pub fn scan_at_commit_with_config(path: &Path, commit: &str, restore_branch: &str, config: Option<&ClassificationConfig>) -> AppResult<LocScan> {
    run_git(Some(path), &["checkout".into(), "--force".into(), commit.to_string()])?;
    let result = scan_worktree_with_config(path, config);
    let restore_result = checkout_branch(path, restore_branch);
    if let Err(error) = restore_result {
        return Err(error);
    }
    result
}

pub fn scan_worktree(path: &Path) -> AppResult<LocScan> {
    scan_worktree_with_config(path, None)
}

pub fn scan_worktree_with_config(path: &Path, config: Option<&ClassificationConfig>) -> AppResult<LocScan> {
    // Keep cache paths out of tokei's report names. Running from the clone and
    // scanning `.` also avoids exposing absolute local paths in diagnostics.
    let mut args = vec![".".into(), "--output".into(), "json".into(), "--files".into(), "--hidden".into()];
    for directory in excluded_tokei_directories() {
        args.push("--exclude".into());
        args.push(directory.to_string());
    }
    if let Some(config) = config {
        for directory in &config.excluded_directories {
            args.push("--exclude".into());
            args.push(directory.clone());
        }
    }
    let mut command = Command::new(command_path("tokei"));
    command.current_dir(path).args(args);
    let output = run_command_with_timeout(command, "tokei")?;
    if !output.status.success() {
        return Err(command_error("tokei", output));
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    if !is_tokei_report(&value) {
        return Err(AppError::InvalidArgument("tokei returned a non-report JSON value".into()));
    }
    let default_config = ClassificationConfig::default();
    let config = config.unwrap_or(&default_config);
    let (mut total, mut source, mut tests) = classify_tokei_json_with_config(&value, config);
    let (fallback_source, fallback_tests) = fallback_omitted_files(path, &value, config)?;
    source += fallback_source;
    tests += fallback_tests;
    total += fallback_source + fallback_tests;
    Ok(LocScan { total_loc: total, source_loc: source, test_loc: tests })
}

/// Count tracked text files omitted by Tokei. Shell candidates are presented
/// under a temporary `.sh` name so Tokei keeps its shell parser; other unknown
/// text uses the same nonblank, full-line-comment rule as the fallback only.
fn fallback_omitted_files(path: &Path, report: &Value, config: &ClassificationConfig) -> AppResult<(i64, i64)> {
    let recognized = tokei_report_paths(report);
    let Ok(output) = run_git(Some(path), &["ls-files".into(), "-z".into()]) else {
        // Keep scan_worktree useful for callers that pass a non-Git directory;
        // normal application clones are Git repositories and use the fallback.
        return Ok((0, 0));
    };
    let tracked = output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|raw| std::str::from_utf8(raw).ok())
        .map(normalize_relative_path)
        .collect::<BTreeSet<_>>();
    let mut walker = WalkBuilder::new(path);
    walker
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .add_custom_ignore_filename(".tokeignore");
    let mut source = 0_i64;
    let mut tests = 0_i64;
    let mut shell_candidates = Vec::new();
    for entry in walker.build() {
        let entry = entry.map_err(|error| AppError::InvalidArgument(format!("could not walk repository for LOC fallback: {error}")))?;
        let relative = normalize_relative_path(entry.path().strip_prefix(path).ok().and_then(|value| value.to_str()).unwrap_or(""));
        if relative.is_empty()
            || !tracked.contains(&relative)
            || recognized.contains(&relative)
            || should_skip_fallback_path(&relative, config)
        {
            continue;
        }
        let full_path = entry.path();
        if !is_regular_file(full_path) {
            continue;
        }
        let bytes = std::fs::read(full_path)?;
        if is_documentation_path(&relative) || bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            continue;
        }
        if is_shell_fallback_candidate(&full_path, &bytes) {
            shell_candidates.push((relative, bytes));
            continue;
        }
        let code = count_fallback_text_lines(&bytes);
        if is_test_path_with_config(&relative, config) {
            tests += code;
        } else {
            source += code;
        }
    }
    let (shell_source, shell_tests) = count_shell_candidates_with_tokei(&shell_candidates, config)?;
    source += shell_source;
    tests += shell_tests;
    Ok((source, tests))
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|metadata| metadata.file_type().is_file()).unwrap_or(false)
}

fn is_shell_fallback_candidate(path: &Path, bytes: &[u8]) -> bool {
    let command_file = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("command"))
        .unwrap_or(false);
    command_file || (path.extension().is_none() && has_shell_shebang(bytes))
}

fn has_shell_shebang(bytes: &[u8]) -> bool {
    let first_line = bytes.split(|byte| *byte == b'\n').next().unwrap_or_default();
    let line_value = String::from_utf8_lossy(first_line);
    let line = line_value.trim_end_matches('\r');
    let Some(shebang) = line.strip_prefix("#!") else { return false; };
    let mut tokens = shebang.split_whitespace();
    let Some(command) = tokens.next() else { return false; };
    let command_name = Path::new(command).file_name().and_then(|value| value.to_str()).unwrap_or(command);
    if command_name == "env" {
        for token in tokens {
            if token == "--" || token.starts_with('-') { continue; }
            return is_shell_interpreter(token);
        }
        false
    } else {
        is_shell_interpreter(command_name)
    }
}

fn is_shell_interpreter(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "ash" | "bash" | "dash" | "fish" | "ksh" | "sh" | "zsh")
}

fn count_fallback_text_lines(bytes: &[u8]) -> i64 {
    std::str::from_utf8(bytes)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !is_full_line_comment(line))
                .count() as i64
        })
        .unwrap_or(0)
}

fn is_full_line_comment(line: &str) -> bool {
    line.starts_with('#') || line.starts_with("//") || line.starts_with("/*") || line.starts_with('*') || line.starts_with("<!--")
}

fn is_documentation_path(relative: &str) -> bool {
    let name = relative.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
    if matches!(name.as_str(), "readme" | "license" | "copying" | "changelog") {
        return true;
    }
    let extension = Path::new(&name).extension().and_then(|value| value.to_str()).unwrap_or("");
    matches!(extension, "adoc" | "asciidoc" | "markdown" | "md" | "mdown" | "mkd" | "org" | "rst" | "text" | "textile" | "txt")
}

fn count_shell_candidates_with_tokei(candidates: &[(String, Vec<u8>)], config: &ClassificationConfig) -> AppResult<(i64, i64)> {
    if candidates.is_empty() {
        return Ok((0, 0));
    }
    let temp_dir = create_shell_temp_dir()?;
    let result = (|| -> AppResult<(i64, i64)> {
        for (index, (_, bytes)) in candidates.iter().enumerate() {
            std::fs::write(temp_dir.join(format!("file-{index:08}.sh")), bytes)?;
        }
        let mut command = Command::new(command_path("tokei"));
        command.current_dir(&temp_dir).args([".", "--output", "json", "--files"]);
        let output = run_command_with_timeout(command, "tokei")?;
        if !output.status.success() {
            return Err(command_error("tokei", output));
        }
        let value: Value = serde_json::from_slice(&output.stdout)?;
        if !is_tokei_report(&value) {
            return Err(AppError::InvalidArgument("tokei returned a non-report JSON value while counting shell fallback files".into()));
        }
        let code_by_path = tokei_report_code_by_path(&value);
        let mut source = 0_i64;
        let mut tests = 0_i64;
        for (index, (relative, _)) in candidates.iter().enumerate() {
            let name = format!("file-{index:08}.sh");
            let Some(code) = code_by_path
                .iter()
                .find(|(path, _)| path == &&name || path.ends_with(&format!("/{name}")))
                .map(|(_, code)| *code)
            else {
                return Err(AppError::InvalidArgument(format!("tokei omitted shell fallback candidate {name}")));
            };
            if is_test_path_with_config(relative, config) {
                tests += code;
            } else {
                source += code;
            }
        }
        Ok((source, tests))
    })();
    let _ = std::fs::remove_dir_all(&temp_dir);
    result
}

fn create_shell_temp_dir() -> AppResult<PathBuf> {
    let base = std::env::temp_dir();
    let seed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    for attempt in 0..16 {
        let candidate = base.join(format!("codetally-shell-{seed}-{}-{attempt}", std::process::id()));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "could not allocate a temporary shell scan directory").into())
}

fn should_skip_fallback_path(relative: &str, config: &ClassificationConfig) -> bool {
    let components: Vec<String> = relative
        .split('/')
        .filter(|component| !component.is_empty())
        .map(|component| component.to_ascii_lowercase())
        .collect();
    if components.iter().any(|component| component == ".git" || component == ".repowise") {
        return true;
    }
    let metadata = [".gitignore", ".gitattributes", ".gitmodules", ".ignore", ".tokeignore", ".ds_store"];
    if components.iter().any(|component| metadata.contains(&component.as_str())) {
        return true;
    }
    let built_in = excluded_tokei_directories();
    if components.iter().any(|component| built_in.contains(component.as_str())) {
        return true;
    }
    config.excluded_directories.iter().any(|configured| {
        let configured: Vec<String> = configured
            .replace('\\', "/")
            .trim_matches('/')
            .to_ascii_lowercase()
            .split('/')
            .filter(|component| !component.is_empty())
            .map(str::to_string)
            .collect();
        !configured.is_empty() && components.windows(configured.len()).any(|window| window.iter().zip(configured.iter()).all(|(actual, expected)| actual == expected))
    })
}

fn normalize_relative_path(value: &str) -> String {
    value.replace('\\', "/").trim_start_matches("./").to_string()
}

fn run_git_raw(path: Option<&Path>, args: &[String]) -> AppResult<Output> {
    let gh = shell_quote(&command_path("gh").to_string_lossy());
    let mut full = vec![
        "-c".into(),
        "credential.helper=".into(),
        "-c".into(),
        format!("credential.helper=!{gh} auth git-credential"),
    ];
    if let Some(path) = path {
        full.push("-C".into());
        full.push(path.to_string_lossy().to_string());
    }
    full.extend(args.iter().cloned());
    let mut command = Command::new(command_path("git"));
    command.args(full).env("GIT_TERMINAL_PROMPT", "0");
    run_command_with_timeout(command, "git")
}

fn run_command_with_timeout(mut command: Command, program: &str) -> AppResult<Output> {
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    });
    let stderr = child.stderr.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    });
    let deadline = Instant::now() + EXTERNAL_COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout.and_then(|reader| reader.join().ok());
                let _ = stderr.and_then(|reader| reader.join().ok());
                return Err(AppError::Command {
                    program: program.to_string(),
                    message: format!("timed out after {} seconds", EXTERNAL_COMMAND_TIMEOUT.as_secs()),
                });
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    };
    let stdout = stdout.and_then(|reader| reader.join().ok()).unwrap_or_default();
    let stderr = stderr.and_then(|reader| reader.join().ok()).unwrap_or_default();
    Ok(Output { status, stdout, stderr })
}

fn is_missing_head(output: &Output) -> bool {
    let message = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    is_missing_head_message(&message)
}

fn is_missing_head_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("ambiguous argument 'head'")
        || (message.contains("unknown revision") && message.contains("head"))
        || message.contains("not a valid object name 'head'")
        || message.contains("needed a single revision")
        || message.contains("does not have any commits")
        || message.contains("bad object head")
}

fn output_text(output: Output) -> AppResult<String> {
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
