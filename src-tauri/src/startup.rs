#[cfg(target_os = "macos")]
use tauri::Manager;
use tauri::{AppHandle, Runtime};

#[cfg(not(target_os = "macos"))]
use tauri_plugin_autostart::ManagerExt;

/// Refresh an existing registration, but never opt someone into opening at login.
/// Development and screenshot sessions must not replace an installed app's entry.
pub fn repair<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if crate::screenshot_mode() || cfg!(debug_assertions) {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    registration(app)?.enabled(&macos::Launchctl)?;
    #[cfg(not(target_os = "macos"))]
    let _ = app;
    Ok(())
}

#[tauri::command]
pub fn get_open_at_login(app: AppHandle) -> Result<bool, String> {
    if crate::screenshot_mode() {
        return Ok(false);
    }
    #[cfg(target_os = "macos")]
    return registration(&app)?.enabled(&macos::Launchctl);
    #[cfg(not(target_os = "macos"))]
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_open_at_login(app: AppHandle, enabled: bool) -> Result<bool, String> {
    if crate::screenshot_mode() {
        return Err("The login setting is unavailable in screenshot mode.".into());
    }
    #[cfg(target_os = "macos")]
    return registration(&app)?.set_enabled(enabled, &macos::Launchctl);
    #[cfg(not(target_os = "macos"))]
    {
        let manager = app.autolaunch();
        if enabled {
            manager.enable()
        } else {
            manager.disable()
        }
        .map_err(|error| error.to_string())?;
        manager.is_enabled().map_err(|error| error.to_string())
    }
}

#[cfg(target_os = "macos")]
fn registration<R: Runtime>(app: &AppHandle<R>) -> Result<macos::Registration, String> {
    let executable = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("Couldn’t locate CodeTally: {error}"))?;
    let name = app.package_info().name.clone();
    let path = app
        .path()
        .home_dir()
        .map_err(|error| format!("Couldn’t locate the login setting: {error}"))?
        .join("Library/LaunchAgents")
        .join(format!("{name}.plist"));
    Ok(macos::Registration {
        path,
        executable,
        name,
        development: cfg!(debug_assertions),
    })
}

#[cfg(target_os = "macos")]
mod macos {
    use plist::{Dictionary, Value};
    use std::fs::{self, OpenOptions};
    use std::io::ErrorKind;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static REGISTRATION_LOCK: Mutex<()> = Mutex::new(());
    static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

    pub(super) trait LoginService {
        fn disabled_override(&self, label: &str) -> Result<Option<bool>, String>;
        fn set_disabled(&self, label: &str, disabled: bool) -> Result<(), String>;
    }

    pub(super) struct Launchctl;

    impl Launchctl {
        fn domain() -> String {
            // geteuid has no preconditions and only reads this process's user ID.
            format!("gui/{}", unsafe { libc::geteuid() })
        }

        fn run(arguments: &[&str]) -> Result<String, String> {
            let output = std::process::Command::new("/bin/launchctl")
                .args(arguments)
                .output()
                .map_err(|error| format!("Couldn’t check the macOS login service: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "Couldn’t update or check the macOS login service: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            String::from_utf8(output.stdout)
                .map_err(|_| "The macOS login service returned invalid text.".into())
        }
    }

    impl LoginService for Launchctl {
        fn disabled_override(&self, label: &str) -> Result<Option<bool>, String> {
            let output = Self::run(&["print-disabled", &Self::domain()])?;
            let prefix = format!("\"{label}\" => ");
            for line in output.lines() {
                if let Some(value) = line.trim().strip_prefix(&prefix) {
                    return match value.trim() {
                        "disabled" | "true" => Ok(Some(true)),
                        "enabled" | "false" => Ok(Some(false)),
                        _ => Err(
                            "The macOS login service returned an unknown disabled state.".into(),
                        ),
                    };
                }
            }
            Ok(None)
        }

        fn set_disabled(&self, label: &str, disabled: bool) -> Result<(), String> {
            Self::run(&[
                if disabled { "disable" } else { "enable" },
                &format!("{}/{label}", Self::domain()),
            ])?;
            Ok(())
        }
    }

    pub(super) struct Registration {
        pub path: PathBuf,
        pub executable: PathBuf,
        pub name: String,
        pub development: bool,
    }

    fn is_bundle_executable(path: &Path) -> bool {
        let Some(macos) = path.parent() else {
            return false;
        };
        let Some(contents) = macos.parent() else {
            return false;
        };
        let Some(bundle) = contents.parent() else {
            return false;
        };
        macos.file_name().is_some_and(|name| name == "MacOS")
            && contents.file_name().is_some_and(|name| name == "Contents")
            && bundle
                .extension()
                .is_some_and(|extension| extension == "app")
    }

    impl Registration {
        fn check_app(&self) -> Result<(), String> {
            if self.development || !is_bundle_executable(&self.executable) {
                return Err("Open at login is available in the installed CodeTally app. Development builds cannot change this setting.".into());
            }
            let metadata = fs::metadata(&self.executable).map_err(|error| {
                format!("Couldn’t locate the installed CodeTally executable: {error}")
            })?;
            if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                return Err("The installed CodeTally executable cannot be launched.".into());
            }
            Ok(())
        }

        fn read(&self) -> Result<Option<Dictionary>, String> {
            let bytes = match fs::read(&self.path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(format!("Couldn’t read the login setting: {error}")),
            };
            let value = Value::from_reader(std::io::Cursor::new(bytes))
                .map_err(|error| format!("The login setting is damaged: {error}"))?;
            let dictionary = value
                .into_dictionary()
                .ok_or("The login setting is not a valid LaunchAgent.")?;
            if dictionary.get("Label").and_then(Value::as_string) != Some(&self.name) {
                return Err("The login entry does not belong to CodeTally.".into());
            }
            Ok(Some(dictionary))
        }

        pub(super) fn enabled(&self, service: &impl LoginService) -> Result<bool, String> {
            let _guard = REGISTRATION_LOCK
                .lock()
                .map_err(|_| "Couldn’t access the login setting.")?;
            self.check_app()?;
            let Some(dictionary) = self.read()? else {
                return Ok(false);
            };
            // Honor a disabled entry as well as a missing one.
            if service.disabled_override(&self.name)?.unwrap_or(
                dictionary
                    .get("Disabled")
                    .and_then(Value::as_boolean)
                    .unwrap_or(false),
            ) || dictionary.get("RunAtLoad").and_then(Value::as_boolean) != Some(true)
            {
                return Ok(false);
            }
            self.refresh(dictionary)?;
            Ok(true)
        }

        pub(super) fn set_enabled(
            &self,
            enabled: bool,
            service: &impl LoginService,
        ) -> Result<bool, String> {
            let _guard = REGISTRATION_LOCK
                .lock()
                .map_err(|_| "Couldn’t access the login setting.")?;
            self.check_app()?;
            if !enabled {
                match fs::remove_file(&self.path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("Couldn’t disable opening at login: {error}"))
                    }
                }
                service.set_disabled(&self.name, true)?;
                return Ok(false);
            }
            let mut dictionary = self.read()?.unwrap_or_default();
            dictionary.insert("Label".into(), Value::String(self.name.clone()));
            dictionary.insert("RunAtLoad".into(), Value::Boolean(true));
            dictionary.remove("Disabled");
            self.refresh(dictionary)?;
            service.set_disabled(&self.name, false)?;
            if service.disabled_override(&self.name)? == Some(true) {
                return Err("macOS still has opening at login disabled.".into());
            }
            Ok(true)
        }

        fn refresh(&self, mut dictionary: Dictionary) -> Result<(), String> {
            let executable = self
                .executable
                .to_str()
                .ok_or("CodeTally’s path is not valid text.")?;
            let mut arguments = match dictionary.get("ProgramArguments") {
                Some(Value::Array(arguments))
                    if !arguments.is_empty()
                        && arguments.iter().all(|value| value.as_string().is_some()) =>
                {
                    arguments.clone()
                }
                None => vec![Value::String(executable.into())],
                _ => return Err("The login entry has invalid launch arguments.".into()),
            };
            arguments[0] = Value::String(executable.into());
            dictionary.insert("ProgramArguments".into(), Value::Array(arguments));
            // launchd gives Program precedence when both keys are present.
            if dictionary.contains_key("Program") {
                dictionary.insert("Program".into(), Value::String(executable.into()));
            }
            if self.read()?.as_ref() != Some(&dictionary) {
                self.write(dictionary)?;
            }
            Ok(())
        }

        fn write(&self, dictionary: Dictionary) -> Result<(), String> {
            let parent = self
                .path
                .parent()
                .ok_or("The login setting has no parent directory.")?;
            fs::create_dir_all(parent)
                .map_err(|error| format!("Couldn’t create the login entry: {error}"))?;
            let mode = match fs::metadata(&self.path) {
                Ok(metadata) => metadata.permissions().mode() & 0o777,
                Err(error) if error.kind() == ErrorKind::NotFound => 0o644,
                Err(error) => {
                    return Err(format!("Couldn’t read login entry permissions: {error}"))
                }
            };
            let sequence = NEXT_WRITE.fetch_add(1, Ordering::Relaxed);
            let temporary = parent.join(format!(
                ".codetally-login-{}-{sequence}.tmp",
                std::process::id()
            ));
            let result = (|| {
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(mode)
                    .open(&temporary)?;
                file.set_permissions(fs::Permissions::from_mode(mode))?;
                Value::Dictionary(dictionary)
                    .to_writer_xml(&file)
                    .map_err(std::io::Error::other)?;
                file.sync_all()?;
                fs::rename(&temporary, &self.path)
            })();
            let _ = fs::remove_file(&temporary);
            // Do not bootout/restart the agent here: it may own this running app.
            // launchd reads the updated entry on the next login.
            result.map_err(|error| format!("Couldn’t update the login entry: {error}"))
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::macos::{LoginService, Registration};
    use plist::{Dictionary, Value};
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    #[derive(Default)]
    struct TestService {
        disabled: Cell<Option<bool>>,
        fail_enable: Cell<bool>,
    }

    impl LoginService for TestService {
        fn disabled_override(&self, _: &str) -> Result<Option<bool>, String> {
            Ok(self.disabled.get())
        }
        fn set_disabled(&self, _: &str, disabled: bool) -> Result<(), String> {
            if !disabled && self.fail_enable.get() {
                return Err("macOS rejected enabling the login item".into());
            }
            self.disabled.set(Some(disabled));
            Ok(())
        }
    }

    struct Fixture {
        root: PathBuf,
        registration: Registration,
        service: TestService,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "codetally-login-test-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            let executable = root.join("CodeTally.app/Contents/MacOS/codetally");
            fs::create_dir_all(executable.parent().unwrap()).unwrap();
            fs::write(&executable, b"fixture executable").unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
            let registration = Registration {
                path: root.join("LaunchAgents/CodeTally.plist"),
                executable,
                name: "CodeTally".into(),
                development: false,
            };
            Self {
                root,
                registration,
                service: TestService::default(),
            }
        }

        fn save(&self, dictionary: Dictionary) {
            fs::create_dir_all(self.registration.path.parent().unwrap()).unwrap();
            Value::Dictionary(dictionary)
                .to_file_xml(&self.registration.path)
                .unwrap();
        }

        fn read(&self) -> Dictionary {
            Value::from_file(&self.registration.path)
                .unwrap()
                .into_dictionary()
                .unwrap()
        }

        fn stale_entry(&self) -> Dictionary {
            let mut entry = Dictionary::new();
            entry.insert("Label".into(), Value::String("CodeTally".into()));
            entry.insert("RunAtLoad".into(), Value::Boolean(true));
            entry.insert(
                "ProgramArguments".into(),
                Value::Array(vec![
                    Value::String(
                        self.root
                            .join("deleted-worktree/target/debug/codetally")
                            .display()
                            .to_string(),
                    ),
                    Value::String("--hidden".into()),
                ]),
            );
            entry
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn repairs_deleted_development_target_and_preserves_arguments_and_other_settings() {
        let fixture = Fixture::new();
        let mut stale = fixture.stale_entry();
        stale.insert(
            "Program".into(),
            Value::String("/missing/old-codetally".into()),
        );
        stale.insert("ProcessType".into(), Value::String("Interactive".into()));
        fixture.save(stale);
        fs::set_permissions(
            &fixture.registration.path,
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(fixture.registration.enabled(&fixture.service).unwrap());
        assert_eq!(
            fs::metadata(&fixture.registration.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let repaired = fixture.read();
        let expected = fixture.registration.executable.to_str().unwrap();
        assert_eq!(repaired["Program"].as_string(), Some(expected));
        let args = repaired["ProgramArguments"].as_array().unwrap();
        assert_eq!(args[0].as_string(), Some(expected));
        assert_eq!(args[1].as_string(), Some("--hidden"));
        assert_eq!(repaired["ProcessType"].as_string(), Some("Interactive"));
        let bytes = fs::read(&fixture.registration.path).unwrap();
        assert!(fixture.registration.enabled(&fixture.service).unwrap());
        assert_eq!(bytes, fs::read(&fixture.registration.path).unwrap());
    }

    #[test]
    fn respects_disabled_entries_and_supports_explicit_enable_disable() {
        let fixture = Fixture::new();
        assert!(!fixture.registration.enabled(&fixture.service).unwrap());
        assert!(!fixture.registration.path.exists());
        for disabled_key in ["Disabled", "RunAtLoad"] {
            fixture.service.disabled.set(None);
            let mut disabled = fixture.stale_entry();
            disabled.insert(
                disabled_key.into(),
                Value::Boolean(disabled_key == "Disabled"),
            );
            fixture.save(disabled.clone());
            assert!(!fixture.registration.enabled(&fixture.service).unwrap());
            assert_eq!(fixture.read(), disabled);
            assert!(fixture
                .registration
                .set_enabled(true, &fixture.service)
                .unwrap());
            assert!(fixture.registration.enabled(&fixture.service).unwrap());
            assert!(!fixture.read().contains_key("Disabled"));
            assert!(!fixture
                .registration
                .set_enabled(false, &fixture.service)
                .unwrap());
            assert!(!fixture.registration.path.exists());
        }
        assert!(fixture
            .registration
            .set_enabled(true, &fixture.service)
            .unwrap());
        assert!(fixture.registration.enabled(&fixture.service).unwrap());
    }

    #[test]
    fn development_and_unbundled_builds_cannot_hijack_or_remove_production_registration() {
        let mut fixture = Fixture::new();
        fixture.save(fixture.stale_entry());
        let before = fs::read(&fixture.registration.path).unwrap();
        fixture.registration.development = true;
        for requested in [true, false] {
            assert!(fixture
                .registration
                .set_enabled(requested, &fixture.service)
                .is_err());
        }
        assert!(fixture.registration.enabled(&fixture.service).is_err());
        assert_eq!(before, fs::read(&fixture.registration.path).unwrap());
        fixture.registration.development = false;
        fixture.registration.executable = fixture.root.join("target/release/codetally");
        assert!(fixture
            .registration
            .set_enabled(true, &fixture.service)
            .is_err());
        assert_eq!(before, fs::read(&fixture.registration.path).unwrap());
    }

    #[test]
    fn externally_disabled_entries_stay_untouched_until_explicit_enable_succeeds() {
        let fixture = Fixture::new();
        fixture.save(fixture.stale_entry());
        let before = fs::read(&fixture.registration.path).unwrap();
        fixture.service.disabled.set(Some(true));
        assert!(!fixture.registration.enabled(&fixture.service).unwrap());
        assert_eq!(before, fs::read(&fixture.registration.path).unwrap());
        fixture.service.fail_enable.set(true);
        assert!(fixture
            .registration
            .set_enabled(true, &fixture.service)
            .is_err());
        assert!(!fixture.registration.enabled(&fixture.service).unwrap());
        fixture.service.fail_enable.set(false);
        assert!(fixture
            .registration
            .set_enabled(true, &fixture.service)
            .unwrap());
        assert_eq!(fixture.service.disabled.get(), Some(false));
        assert!(fixture.registration.enabled(&fixture.service).unwrap());
    }

    #[test]
    fn invalid_entries_and_write_failures_are_reported_without_claiming_enabled() {
        let mut fixture = Fixture::new();
        fs::create_dir_all(fixture.registration.path.parent().unwrap()).unwrap();
        fs::write(&fixture.registration.path, b"broken plist").unwrap();
        assert!(fixture
            .registration
            .enabled(&fixture.service)
            .unwrap_err()
            .contains("damaged"));
        assert_eq!(
            fs::read(&fixture.registration.path).unwrap(),
            b"broken plist"
        );
        let mut invalid = fixture.stale_entry();
        invalid.insert(
            "ProgramArguments".into(),
            Value::Array(vec![Value::Boolean(true)]),
        );
        fixture.save(invalid.clone());
        assert!(fixture
            .registration
            .enabled(&fixture.service)
            .unwrap_err()
            .contains("launch arguments"));
        assert_eq!(fixture.read(), invalid);
        let blocker = fixture.root.join("not-a-directory");
        fs::write(&blocker, b"blocker").unwrap();
        fixture.registration.path = blocker.join("CodeTally.plist");
        assert!(fixture
            .registration
            .set_enabled(true, &fixture.service)
            .is_err());
    }
}
