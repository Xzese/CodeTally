use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde_json::Value;
use crate::models::ClassificationConfig;
use std::collections::{BTreeMap, BTreeSet};

/// Returns true for the conventional test directories and file suffixes used by
/// the common languages supported by tokei. The matcher deliberately stays small
/// and predictable so it can later be replaced by per-repository configuration.
pub fn is_test_path(path: &str) -> bool {
    is_test_path_with_config(path, &ClassificationConfig::default())
}

pub fn is_test_path_with_config(path: &str, config: &ClassificationConfig) -> bool {
    let normal = path.replace('\\', "/");
    let lower = normal.to_ascii_lowercase();
    let components: Vec<&str> = lower.split('/').collect();
    for entry in &config.test_paths {
        let configured = normalize_config_path(entry);
        if configured.is_empty() {
            continue;
        }
        let configured_components: Vec<&str> = configured.split('/').collect();
        if configured_components.len() == 1 {
            if components.iter().any(|component| *component == configured) {
                return true;
            }
        } else if components
            .windows(configured_components.len())
            .any(|window| window == configured_components.as_slice())
        {
            return true;
        }
    }
    let file = components.last().copied().unwrap_or("");
    config.test_patterns.iter().any(|pattern| {
        let pattern = pattern.replace('\\', "/");
        if pattern.contains('/') {
            glob_match(&pattern, &lower)
        } else {
            glob_match(&pattern, file)
        }
    })
}

fn glob_match(pattern: &str, value: &str) -> bool {
    // This is deliberately a small, path-independent glob matcher. A `*`
    // consumes zero or more characters and every other character is literal;
    // consequently patterns without a wildcard require an exact match and
    // patterns with a suffix (for example `*Tests.swift`) really do require
    // that suffix. The two-index implementation also handles adjacent or
    // repeated wildcards without accidentally treating an earlier fragment as
    // the final suffix.
    let pattern = pattern.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut star_index = None;
    let mut star_value_index = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && pattern[pattern_index] != b'*'
            && pattern[pattern_index] == value[value_index]
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn normalize_config_path(value: &str) -> String {
    value
        .replace('\\', "/")
        .trim_matches('/')
        .to_ascii_lowercase()
}

pub fn is_documentation_language(language: &str) -> bool {
    matches!(
        language.to_ascii_lowercase().as_str(),
        "markdown"
            | "md"
            | "text"
            | "plaintext"
            | "restructuredtext"
            | "rst"
            | "asciidoc"
            | "adoc"
            | "org"
    )
}

/// Parse `tokei --output json --files` output. Tokei has changed the exact
/// report shape across releases, so both a `reports` array and a map of file
/// reports are accepted. Each file's own `stats.code` and embedded
/// `stats.blobs` are summed so Total LOC always equals Source LOC plus Test LOC.
pub fn classify_tokei_json(value: &Value) -> (i64, i64, i64) {
    classify_tokei_json_with_config(value, &ClassificationConfig::default())
}

pub fn classify_tokei_json_with_config(value: &Value, config: &ClassificationConfig) -> (i64, i64, i64) {
    let Some(languages) = value.as_object() else {
        return (0, 0, 0);
    };
    let mut source = 0_i64;
    let mut tests = 0_i64;
    for (language, summary) in languages {
        if language.eq_ignore_ascii_case("total") || is_documentation_language(language) {
            continue;
        }
        let Some(summary_obj) = summary.as_object() else {
            continue;
        };
        if let Some(reports) = summary_obj.get("reports") {
            match reports {
                Value::Array(entries) => {
                    for entry in entries {
                        let Some(entry_obj) = entry.as_object() else {
                            continue;
                        };
                        let name = entry_obj
                            .get("name")
                            .and_then(Value::as_str)
                            .or_else(|| entry_obj.get("path").and_then(Value::as_str))
                            .unwrap_or("");
                        let (entry_source, entry_tests) = classify_file_report(entry, name, config);
                        source += entry_source;
                        tests += entry_tests;
                    }
                }
                Value::Object(entries) => {
                    for (name, entry) in entries {
                        let (entry_source, entry_tests) = classify_file_report(entry, name, config);
                        source += entry_source;
                        tests += entry_tests;
                    }
                }
                _ => {}
            }
        }
    }
    (source + tests, source, tests)
}

/// Return the file paths represented by Tokei's per-file reports. Unknown
/// extensions are absent from this set and can therefore be counted by the
/// scanner's narrow shell-script fallback without double-counting recognized
/// source files.
pub fn tokei_report_paths(value: &Value) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    let Some(languages) = value.as_object() else { return paths; };
    for summary in languages.values() {
        let Some(summary_obj) = summary.as_object() else { continue; };
        let Some(reports) = summary_obj.get("reports") else { continue; };
        match reports {
            Value::Array(entries) => {
                for entry in entries {
                    if let Some(path) = report_path(entry, "") {
                        paths.insert(normalize_report_path(path));
                    }
                }
            }
            Value::Object(entries) => {
                for (name, entry) in entries {
                    if let Some(path) = report_path(entry, name) {
                        paths.insert(normalize_report_path(path));
                    }
                }
            }
            _ => {}
        }
    }
    paths
}

/// Return code-line totals keyed by each per-file Tokei report path. This is
/// used for files whose contents are temporarily presented to Tokei under a
/// recognized shell extension while retaining their original repository path
/// for classification.
pub fn tokei_report_code_by_path(value: &Value) -> BTreeMap<String, i64> {
    let mut totals = BTreeMap::new();
    let Some(languages) = value.as_object() else { return totals; };
    for summary in languages.values() {
        let Some(summary_obj) = summary.as_object() else { continue; };
        let Some(reports) = summary_obj.get("reports") else { continue; };
        match reports {
            Value::Array(entries) => {
                for entry in entries {
                    if let Some(path) = report_path(entry, "") {
                        totals.insert(normalize_report_path(path), report_code(entry));
                    }
                }
            }
            Value::Object(entries) => {
                for (name, entry) in entries {
                    if let Some(path) = report_path(entry, name) {
                        totals.insert(normalize_report_path(path), report_code(entry));
                    }
                }
            }
            _ => {}
        }
    }
    totals
}

fn report_code(value: &Value) -> i64 {
    let Some(object) = value.as_object() else { return 0; };
    let mut code = object.get("code").and_then(Value::as_i64).unwrap_or(0);
    if let Some(stats) = object.get("stats") {
        code += report_code(stats);
    }
    if let Some(blobs) = object.get("blobs").and_then(Value::as_object) {
        code += blobs.values().map(report_code).sum::<i64>();
    }
    code
}

fn report_path<'a>(value: &'a Value, fallback: &'a str) -> Option<&'a str> {
    let object = value.as_object()?;
    object
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| object.get("path").and_then(Value::as_str))
        .or_else(|| (!fallback.is_empty()).then_some(fallback))
}

fn normalize_report_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

fn classify_file_report(value: &Value, name: &str, config: &ClassificationConfig) -> (i64, i64) {
    let Some(object) = value.as_object() else { return (0, 0); };
    let path = object
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| object.get("path").and_then(Value::as_str))
        .unwrap_or(name);

    classify_stats(object.get("stats").unwrap_or(value), path, config)
}

fn classify_stats(value: &Value, inherited_path: &str, config: &ClassificationConfig) -> (i64, i64) {
    let Some(object) = value.as_object() else { return (0, 0); };
    let mut source = 0_i64;
    let mut tests = 0_i64;

    // CodeStats::summarise includes the current node's code and then adds
    // each embedded language blob. Blob keys are language names (CSS,
    // JavaScript, ...), never path components, so every child keeps the
    // original report path for test classification.
    if let Some(code) = object.get("code").and_then(Value::as_i64) {
        let (code_source, code_tests) = classify_code(inherited_path, code, config);
        source += code_source;
        tests += code_tests;
    }
    if let Some(blobs) = object.get("blobs").and_then(Value::as_object) {
        for (language, blob) in blobs {
            if is_documentation_language(language) {
                continue;
            }
            let (blob_source, blob_tests) = if let Some(code) = blob.as_i64() {
                classify_code(inherited_path, code, config)
            } else {
                classify_stats(blob, inherited_path, config)
            };
            source += blob_source;
            tests += blob_tests;
        }
    }
    (source, tests)
}

fn classify_code(path: &str, code: i64, config: &ClassificationConfig) -> (i64, i64) {
    if is_test_path_with_config(path, config) {
        (0, code)
    } else {
        (code, 0)
    }
}

/// Return whether a parsed value has the structure emitted by
/// `tokei --output json --files`. An empty object is the valid report for an
/// empty repository; a non-empty value must contain per-file reports. This
/// keeps callers from silently turning an error payload or unrelated JSON
/// object into a zero-line snapshot.
pub fn is_tokei_report(value: &Value) -> bool {
    let Some(languages) = value.as_object() else { return false; };
    if languages.is_empty() {
        return true;
    }
    let valid_languages = languages.iter().all(|(language, summary)| {
        if language.eq_ignore_ascii_case("total") {
            return true;
        }
        let Some(summary_obj) = summary.as_object() else { return false; };
        if is_documentation_language(language) {
            return summary_obj
                .get("reports")
                .map(valid_reports)
                .unwrap_or(false);
        }
        summary_obj
            .get("reports")
            .map(valid_reports)
            .unwrap_or(false)
    });
    if !valid_languages {
        return false;
    }
    if languages.keys().any(|language| !language.eq_ignore_ascii_case("total")) {
        return true;
    }

    // Tokei emits a Total-only report for an empty repository or a tree that
    // contains no recognised language files.
    languages
        .get("Total")
        .or_else(|| languages.get("total"))
        .and_then(Value::as_object)
        .map(|total| {
            total.get("reports").map(valid_reports).unwrap_or(false)
                && total.get("children").map(Value::is_object).unwrap_or(false)
        })
        .unwrap_or(false)
}

fn valid_reports(value: &Value) -> bool {
    match value {
        Value::Array(entries) => entries.iter().all(valid_report_entry),
        Value::Object(entries) => entries
            .iter()
            .all(|(name, entry)| valid_report_entry_with_name(entry, name)),
        _ => false,
    }
}

fn valid_report_entry(value: &Value) -> bool {
    valid_report_entry_with_name(value, "")
}

fn valid_report_entry_with_name(value: &Value, fallback_name: &str) -> bool {
    let Some(object) = value.as_object() else { return false; };
    let has_path = !fallback_name.is_empty()
        || object.get("name").and_then(Value::as_str).is_some()
        || object.get("path").and_then(Value::as_str).is_some();
    let has_code = object.get("code").and_then(Value::as_i64).is_some()
        || object.get("stats").and_then(Value::as_object).and_then(|stats| stats.get("code")).and_then(Value::as_i64).is_some();
    let has_blobs = object.get("blobs").map(has_blob_stats).unwrap_or(false)
        || object.get("stats").map(has_blob_stats).unwrap_or(false);
    has_path && (has_code || has_blobs)
}

fn has_blob_stats(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|object| object.get("blobs"))
        .and_then(Value::as_object)
        .map(|blobs| !blobs.is_empty())
        .unwrap_or(false)
}

pub fn month_sample_dates(start: Option<&str>, end: DateTime<Utc>) -> Vec<String> {
    let start_date = start
        .and_then(parse_date)
        .unwrap_or_else(|| end.date_naive());
    if start_date > end.date_naive() {
        return Vec::new();
    }
    let mut year = start_date.year();
    let mut month = start_date.month();
    let end_month = (end.year(), end.month());
    let mut dates = Vec::new();
    while (year, month) <= end_month {
        let next_month = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap()
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1).unwrap()
        };
        let month_end = next_month - Duration::days(1);
        if (year, month) == end_month {
            // The current month's month-end is in the future until the month
            // closes. Use the actual sync instant so snapshots never claim a
            // future sample date.
            dates.push(end.to_rfc3339());
        } else {
            dates.push(format!("{}T23:59:59Z", month_end.format("%Y-%m-%d")));
        }
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }
    }
    dates
}

pub fn parse_date(value: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.date_naive())
        .or_else(|_| NaiveDate::parse_from_str(value, "%Y-%m-%d"))
        .ok()
}

pub fn range_start(range: Option<&str>, now: DateTime<Utc>) -> Option<String> {
    let days = match range.unwrap_or("all").to_ascii_lowercase().as_str() {
        "3m" => Some(92),
        "1y" => Some(366),
        "3y" => Some(366 * 3),
        _ => None,
    };
    days.map(|days| (now - Duration::days(days)).format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

pub fn excluded_tokei_directories() -> BTreeSet<&'static str> {
    [
        ".git",
        ".repowise",
        "node_modules",
        "vendor",
        "dist",
        "build",
        "coverage",
        ".next",
        ".venv",
        "venv",
        "target",
        "DerivedData",
    ]
    .into_iter()
    .collect()
}
