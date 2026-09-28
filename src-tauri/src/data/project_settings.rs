//! 프로젝트 본문과 분리된 앱 로컬 기본 프로젝트 설정이다.
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::{
    atomic_file::{save_deterministic_json, SaveError},
    project_file::directory::is_reparse,
};

#[cfg(test)]
use super::atomic_file::{
    save_deterministic_json_with_after_replace_hook, AtomicWriteError, AtomicWriteStage,
    SaveOutcome,
};
#[cfg(test)]
use std::sync::{Arc, Mutex};

const SCHEMA_VERSION: u32 = 1;
const FILE_NAME: &str = "project-preferences.json";
const MAX_BYTES: u64 = 64 * 1024;
const MAX_PATH_LENGTH: usize = 32_768;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preferences {
    schema_version: u32,
    default_project_root: Option<String>,
}

#[derive(Debug)]
pub(crate) enum SettingsError {
    Read(io::Error),
    Invalid,
    Write(SaveError),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "앱 로컬 설정 읽기 실패 ({:?})", error.kind()),
            Self::Invalid => formatter.write_str("앱 로컬 설정 형식이 올바르지 않습니다"),
            Self::Write(error) => write!(formatter, "앱 로컬 설정 저장 실패: {error}"),
        }
    }
}

impl std::error::Error for SettingsError {}

#[derive(Clone)]
pub(crate) struct ProjectSettings {
    directory: PathBuf,
    #[cfg(test)]
    write_fault: Arc<Mutex<Option<TestWriteFault>>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestWriteFault {
    NotApplied,
    AppliedDurabilityUncertain,
    AppliedDurabilityUncertainUnreadable,
}

impl ProjectSettings {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            #[cfg(test)]
            write_fault: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_write_fault(directory: PathBuf, fault: TestWriteFault) -> Self {
        Self {
            directory,
            write_fault: Arc::new(Mutex::new(Some(fault))),
        }
    }

    pub(crate) fn read(&self) -> Result<Option<String>, SettingsError> {
        let path = self.directory.join(FILE_NAME);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(&self.directory) {
                    Err(parent) if parent.kind() == io::ErrorKind::NotFound => return Ok(None),
                    Ok(parent) if parent.is_dir() && !is_reparse(&parent) => return Ok(None),
                    Ok(_) => return Err(SettingsError::Read(error)),
                    Err(parent) => return Err(SettingsError::Read(parent)),
                }
            }
            Err(error) => return Err(SettingsError::Read(error)),
        };
        if !metadata.is_file() || is_reparse(&metadata) || metadata.len() > MAX_BYTES {
            return Err(SettingsError::Invalid);
        }
        let bytes = fs::read(&path).map_err(SettingsError::Read)?;
        let preferences: Preferences =
            serde_json::from_slice(&bytes).map_err(|_| SettingsError::Invalid)?;
        if preferences.schema_version != SCHEMA_VERSION {
            return Err(SettingsError::Invalid);
        }
        if let Some(root) = &preferences.default_project_root {
            validate_root_text(root).map_err(|_| SettingsError::Invalid)?;
        }
        Ok(preferences.default_project_root)
    }

    pub(crate) fn write(&self, root: Option<String>) -> Result<Option<String>, SettingsError> {
        let default_project_root = root.map(|root| canonical_project_root(&root)).transpose()?;
        ensure_directory(&self.directory)?;
        let preferences = Preferences {
            schema_version: SCHEMA_VERSION,
            default_project_root: default_project_root.clone(),
        };
        self.save(&self.directory.join(FILE_NAME), &preferences)?;
        Ok(default_project_root)
    }

    fn save(&self, path: &Path, preferences: &Preferences) -> Result<(), SettingsError> {
        #[cfg(test)]
        if let Some(fault) = self
            .write_fault
            .lock()
            .expect("project settings test fault mutex")
            .take()
        {
            return match fault {
                TestWriteFault::NotApplied => Err(SettingsError::Write(SaveError::AtomicWrite(
                    AtomicWriteError {
                        stage: AtomicWriteStage::ReplaceTarget,
                        target: path.to_path_buf(),
                        temporary_path: None,
                        source: io::Error::new(io::ErrorKind::PermissionDenied, "test fault"),
                        cleanup_error: None,
                        outcome: SaveOutcome::NotApplied,
                    },
                ))),
                TestWriteFault::AppliedDurabilityUncertain => {
                    save_deterministic_json_with_after_replace_hook(path, preferences, |_| {
                        Err(io::Error::new(io::ErrorKind::Other, "test fault"))
                    })
                    .map_err(SettingsError::Write)
                }
                TestWriteFault::AppliedDurabilityUncertainUnreadable => {
                    save_deterministic_json_with_after_replace_hook(path, preferences, |target| {
                        fs::write(target, b"{broken")?;
                        Err(io::Error::new(io::ErrorKind::Other, "test fault"))
                    })
                    .map_err(SettingsError::Write)
                }
            };
        }
        save_deterministic_json(path, preferences).map_err(SettingsError::Write)
    }
}

fn validate_root_text(root: &str) -> Result<(), ()> {
    let path = Path::new(root);
    if root.is_empty() || root.len() > MAX_PATH_LENGTH || root.contains('\0') || !path.is_absolute()
    {
        return Err(());
    }
    Ok(())
}

fn canonical_project_root(root: &str) -> Result<String, SettingsError> {
    validate_root_text(root).map_err(|_| SettingsError::Invalid)?;
    let metadata = fs::symlink_metadata(root).map_err(SettingsError::Read)?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(SettingsError::Invalid);
    }
    let canonical = fs::canonicalize(root).map_err(SettingsError::Read)?;
    canonical
        .to_str()
        .map(str::to_owned)
        .ok_or(SettingsError::Invalid)
}

fn ensure_directory(directory: &Path) -> Result<(), SettingsError> {
    fs::create_dir_all(directory).map_err(SettingsError::Read)?;
    let metadata = fs::symlink_metadata(directory).map_err(SettingsError::Read)?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(SettingsError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> io::Result<Self> {
            for _ in 0..128 {
                let path = std::env::temp_dir().join(format!(
                    "worldbuild-project-settings-{}-{}",
                    std::process::id(),
                    COUNTER.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "test collision",
            ))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_setting_is_unset_and_round_trips_canonical_project(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = Temp::new()?;
        let project = temp.0.join("project");
        fs::create_dir(&project)?;
        let settings = ProjectSettings::new(temp.0.join("settings"));
        assert_eq!(settings.read()?, None);
        let saved = settings.write(Some(project.to_string_lossy().into_owned()))?;
        assert_eq!(settings.read()?, saved);
        assert_eq!(settings.write(None)?, None);
        assert_eq!(settings.read()?, None);
        Ok(())
    }

    #[test]
    fn malformed_setting_is_not_replaced_by_a_read() -> Result<(), Box<dyn std::error::Error>> {
        let temp = Temp::new()?;
        let directory = temp.0.join("settings");
        fs::create_dir(&directory)?;
        let path = directory.join(FILE_NAME);
        fs::write(&path, b"{broken")?;
        let settings = ProjectSettings::new(directory);
        assert!(matches!(settings.read(), Err(SettingsError::Invalid)));
        assert_eq!(fs::read(path)?, b"{broken");
        Ok(())
    }

    #[test]
    fn unsupported_schema_is_rejected_without_replacing_bytes(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = Temp::new()?;
        let directory = temp.0.join("settings");
        fs::create_dir(&directory)?;
        let path = directory.join(FILE_NAME);
        let bytes = br#"{"schemaVersion":2,"defaultProjectRoot":null}"#;
        fs::write(&path, bytes)?;
        let settings = ProjectSettings::new(directory);
        assert!(matches!(settings.read(), Err(SettingsError::Invalid)));
        assert_eq!(fs::read(path)?, bytes);
        Ok(())
    }

    #[test]
    fn settings_directory_io_failure_is_not_treated_as_unset(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = Temp::new()?;
        let not_directory = temp.0.join("settings-is-a-file");
        fs::write(&not_directory, b"occupied")?;
        let settings = ProjectSettings::new(not_directory);
        assert!(matches!(settings.read(), Err(SettingsError::Read(_))));
        Ok(())
    }

    #[test]
    fn rejected_write_preserves_the_previous_default() -> Result<(), Box<dyn std::error::Error>> {
        let temp = Temp::new()?;
        let project = temp.0.join("project");
        fs::create_dir(&project)?;
        let settings = ProjectSettings::new(temp.0.join("settings"));
        let saved = settings.write(Some(project.to_string_lossy().into_owned()))?;

        let missing = temp.0.join("missing");
        assert!(settings
            .write(Some(missing.to_string_lossy().into_owned()))
            .is_err());
        assert_eq!(settings.read()?, saved);
        Ok(())
    }
}
