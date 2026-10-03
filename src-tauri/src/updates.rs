use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const RELEASE_API_PATH: &str = "repos/Xzese/CodeTally/releases/latest";
const RELEASE_URL_PREFIX: &str = "https://github.com/Xzese/CodeTally/releases/tag/";
const GH_TIMEOUT: Duration = Duration::from_secs(20);
const GH_POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_GH_OUTPUT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateCheck {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub update_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedVersion {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: bool,
    build: bool,
}

impl ParsedVersion {
    fn core(self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

#[derive(Debug)]
struct ParsedRelease {
    tag: String,
    version: ParsedVersion,
}

/// Check the installed version against the latest published stable release.
/// The caller is responsible for running this blocking operation away from an
/// async executor thread.
pub fn check_for_updates(current_version: &str) -> Result<UpdateCheck, String> {
    let current = parse_version(current_version)
        .map_err(|error| format!("Installed app version is invalid: {error}"))?;
    let release = parse_release_response(&run_release_query()?)?;
    let latest_version = release.version.core();
    let release_url = format!("{RELEASE_URL_PREFIX}{}", release.tag);

    Ok(UpdateCheck {
        current_version: current_version.to_string(),
        latest_version,
        release_url,
        update_available: compare_versions(&current, &release.version) == Ordering::Less,
    })
}

fn parse_version(input: &str) -> Result<ParsedVersion, String> {
    if input.is_empty() || input.trim() != input {
        return Err("version must not be empty or contain surrounding whitespace".into());
    }

    let without_prefix = input
        .strip_prefix('v')
        .or_else(|| input.strip_prefix('V'))
        .unwrap_or(input);
    let (without_build, build) = without_prefix
        .split_once('+')
        .map_or((without_prefix, None), |(core, metadata)| (core, Some(metadata)));
    if let Some(metadata) = build {
        validate_identifiers(metadata, false, "build metadata")?;
    }

    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, None), |(core, value)| (core, Some(value)));
    if let Some(value) = prerelease {
        validate_identifiers(value, true, "prerelease")?;
    }

    let mut components = core.split('.');
    let major = parse_component(components.next(), "major")?;
    let minor = parse_component(components.next(), "minor")?;
    let patch = parse_component(components.next(), "patch")?;
    if components.next().is_some() {
        return Err("version must contain exactly major.minor.patch".into());
    }

    Ok(ParsedVersion {
        major,
        minor,
        patch,
        prerelease: prerelease.is_some(),
        build: build.is_some(),
    })
}

fn parse_stable_version(input: &str) -> Result<ParsedVersion, String> {
    let version = parse_version(input)?;
    if version.prerelease || version.build {
        return Err("release tag must be a stable major.minor.patch version".into());
    }
    Ok(version)
}

fn parse_component(value: Option<&str>, name: &str) -> Result<u64, String> {
    let value = value.ok_or_else(|| format!("version is missing its {name} component"))?;
    if value.is_empty()
        || !value.chars().all(|character| character.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(format!("version has an invalid {name} component"));
    }
    value
        .parse::<u64>()
        .map_err(|_| format!("version {name} component is too large"))
}

fn validate_identifiers(value: &str, reject_numeric_leading_zero: bool, kind: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{kind} must not be empty"));
    }
    for identifier in value.split('.') {
        if identifier.is_empty()
            || !identifier
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
            || (reject_numeric_leading_zero
                && identifier.len() > 1
                && identifier.chars().all(|character| character.is_ascii_digit())
                && identifier.starts_with('0'))
        {
            return Err(format!("{kind} contains an invalid identifier"));
        }
    }
    Ok(())
}

fn compare_versions(left: &ParsedVersion, right: &ParsedVersion) -> Ordering {
    (left.major, left.minor, left.patch)
        .cmp(&(right.major, right.minor, right.patch))
        .then_with(|| match (left.prerelease, right.prerelease) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => Ordering::Equal,
        })
}

fn parse_release_response(body: &str) -> Result<ParsedRelease, String> {
    let release: GithubRelease = serde_json::from_str(body)
        .map_err(|error| format!("latest release response is invalid JSON: {error}"))?;
    if release.draft {
        return Err("latest release is a draft".into());
    }
    if release.prerelease {
        return Err("latest release is a prerelease".into());
    }

    let version = parse_stable_version(&release.tag_name)
        .map_err(|error| format!("latest release tag is invalid: {error}"))?;
    Ok(ParsedRelease {
        tag: release.tag_name,
        version,
    })
}

fn run_release_query() -> Result<String, String> {
    let mut child = Command::new(crate::github::command_path("gh"))
        .args([
            "api",
            "--hostname",
            "github.com",
            RELEASE_API_PATH,
            "--jq",
            "{tag_name, draft, prerelease}",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start GitHub CLI: {error}"))?;

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("GitHub CLI did not provide stdout".into());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("GitHub CLI did not provide stderr".into());
        }
    };
    let stdout_reader = thread::spawn(move || read_capped(stdout));
    let stderr_reader = thread::spawn(move || read_capped(stderr));

    let mut status = None;
    let deadline = Instant::now() + GH_TIMEOUT;
    while status.is_none() {
        match child.try_wait() {
            Ok(Some(exit_status)) => status = Some(exit_status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err("GitHub release check timed out after 20 seconds".into());
            }
            Ok(None) => thread::sleep(GH_POLL_INTERVAL),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("could not wait for GitHub CLI: {error}"));
            }
        }
    }

    let stdout = stdout_reader
        .join()
        .map_err(|_| "GitHub CLI stdout reader failed".to_string())?
        .map_err(|error| format!("could not read GitHub CLI output: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "GitHub CLI stderr reader failed".to_string())?
        .map_err(|error| format!("could not read GitHub CLI error output: {error}"))?;
    let status = status.expect("status set before readers are joined");
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("GitHub CLI release check failed ({status})")
        } else {
            format!("GitHub CLI release check failed: {detail}")
        });
    }

    String::from_utf8(stdout).map_err(|_| "GitHub CLI returned non-UTF-8 output".into())
}

fn read_capped<R: Read>(reader: R) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader
        .take(MAX_GH_OUTPUT_BYTES + 1)
        .read_to_end(&mut output)?;
    if output.len() as u64 > MAX_GH_OUTPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GitHub CLI output exceeded the size limit",
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optional_v_and_current_prerelease() {
        let stable = parse_version("v1.2.3").expect("stable version");
        let prerelease = parse_version("1.2.4-rc.1").expect("prerelease version");
        assert_eq!(stable.core(), "1.2.3");
        assert!(prerelease.prerelease);
        assert_eq!(prerelease.core(), "1.2.4");
    }

    #[test]
    fn compares_numeric_components_and_stable_over_current_prerelease() {
        let current = parse_version("1.2.3-rc.1").expect("current version");
        let stable = parse_version("1.2.3").expect("stable version");
        let newer = parse_version("1.2.4").expect("newer version");
        let one_nine = parse_version("1.9.0").expect("1.9.0");
        let one_ten = parse_version("1.10.0").expect("1.10.0");
        assert_eq!(compare_versions(&current, &stable), Ordering::Less);
        assert_eq!(compare_versions(&stable, &newer), Ordering::Less);
        assert_eq!(compare_versions(&stable, &current), Ordering::Greater);
        assert_eq!(compare_versions(&one_nine, &one_ten), Ordering::Less);
    }

    #[test]
    fn rejects_malformed_versions() {
        for value in ["1.2", "1.2.3.4", "v1.02.3", "1.2.3-", "1.2.3+bad space"] {
            assert!(parse_version(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn accepts_published_stable_release_and_uses_tag_only_for_trusted_url() {
        let release = parse_release_response(
            r#"{"tag_name":"v1.2.3","draft":false,"prerelease":false,"html_url":"https://evil.example/release"}"#,
        )
        .expect("release response");
        assert_eq!(release.version.core(), "1.2.3");
        assert_eq!(release.tag, "v1.2.3");
    }

    #[test]
    fn rejects_draft_prerelease_and_malformed_release_responses() {
        for body in [
            r#"{"tag_name":"v1.2.3","draft":true,"prerelease":false}"#,
            r#"{"tag_name":"v1.2.3","draft":false,"prerelease":true}"#,
            r#"{"tag_name":"v1.2.3-beta","draft":false,"prerelease":false}"#,
            r#"{"tag_name":"v1.2.3","draft":false}"#,
            "not json",
        ] {
            assert!(parse_release_response(body).is_err(), "accepted {body}");
        }
    }
}
