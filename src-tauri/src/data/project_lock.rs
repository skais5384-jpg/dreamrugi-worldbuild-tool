use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::atomic_file::{save_deterministic_json, SaveError};
use super::schema::SchemaVersion;
use super::utc_time::now_utc_milliseconds;

const LOCK_KEY_VERSION: &[u8] = b"worldbuild-lock-v1";
const METADATA_LIMIT: u64 = 16 * 1024;
pub(crate) const PROJECT_LOCK_METADATA_SCHEMA_VERSION: SchemaVersion =
    SchemaVersion::new_unchecked(1);

/// 충돌 화면에 표시할 수 있는 정보만 담고 경로와 사용자 이름은 기록하지 않는다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectLockMetadata {
    pub schema_version: SchemaVersion,
    pub pid: u32,
    pub app_version: String,
    pub acquired_at_utc: String,
    pub project_fingerprint: String,
}

#[derive(Debug)]
pub(crate) enum MetadataReadError {
    Io(io::Error),
    TooLarge { limit: u64 },
    InvalidJson(serde_json::Error),
    UnsupportedSchema { found: u32, supported: u32 },
    FingerprintMismatch,
}

impl fmt::Display for MetadataReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "failed to read lock metadata: {error}"),
            Self::TooLarge { limit } => {
                write!(formatter, "lock metadata exceeds the {limit}-byte limit")
            }
            Self::InvalidJson(error) => write!(formatter, "invalid lock metadata JSON: {error}"),
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "unsupported lock metadata schema {found}; supported schema is {supported}"
            ),
            Self::FingerprintMismatch => formatter.write_str("lock metadata fingerprint differs"),
        }
    }
}

impl std::error::Error for MetadataReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidJson(error) => Some(error),
            Self::TooLarge { .. } | Self::UnsupportedSchema { .. } | Self::FingerprintMismatch => {
                None
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum MetadataUpdateError {
    FormatTime(time::error::Format),
    Save(SaveError),
}

impl fmt::Display for MetadataUpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FormatTime(error) => write!(formatter, "failed to format UTC time: {error}"),
            Self::Save(error) => write!(formatter, "failed to save lock metadata: {error}"),
        }
    }
}

impl std::error::Error for MetadataUpdateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FormatTime(error) => Some(error),
            Self::Save(error) => Some(error),
        }
    }
}

pub(crate) enum ProjectLockError {
    InvalidProjectPath {
        path: PathBuf,
        source: io::Error,
    },
    LockDirectoryUnavailable {
        path: PathBuf,
        source: io::Error,
    },
    LockFileOpenFailed {
        path: PathBuf,
        source: io::Error,
    },
    AlreadyLocked {
        fingerprint: String,
        metadata: Option<ProjectLockMetadata>,
        metadata_error: Option<MetadataReadError>,
    },
    LockOperationFailed {
        fingerprint: String,
        source: io::Error,
    },
    MetadataWriteFailed {
        fingerprint: String,
        source: MetadataUpdateError,
        unlock_error: Option<io::Error>,
    },
    UnlockFailed {
        fingerprint: String,
        source: io::Error,
    },
}

impl fmt::Display for ProjectLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProjectPath { source, .. } => {
                formatter.write_str("invalid project directory; ")?;
                format_io_category(formatter, source)
            }
            Self::LockDirectoryUnavailable { source, .. } => {
                formatter.write_str("lock directory is unavailable; ")?;
                format_io_category(formatter, source)
            }
            Self::LockFileOpenFailed { source, .. } => {
                formatter.write_str("failed to open lock file; ")?;
                format_io_category(formatter, source)
            }
            Self::AlreadyLocked { fingerprint, .. } => {
                write!(formatter, "project {fingerprint} is already locked")
            }
            Self::LockOperationFailed {
                fingerprint,
                source,
            } => {
                write!(formatter, "failed to acquire project lock {fingerprint}; ")?;
                format_io_category(formatter, source)
            }
            Self::MetadataWriteFailed {
                fingerprint,
                source,
                unlock_error,
            } => {
                write!(
                    formatter,
                    "failed to update metadata for lock {fingerprint}; "
                )?;
                format_metadata_update_category(formatter, source)?;
                if let Some(error) = unlock_error {
                    formatter.write_str("; releasing the acquired lock also failed (")?;
                    format_io_category(formatter, error)?;
                    formatter.write_str(")")?;
                }
                Ok(())
            }
            Self::UnlockFailed {
                fingerprint,
                source,
            } => {
                write!(formatter, "failed to release project lock {fingerprint}; ")?;
                format_io_category(formatter, source)
            }
        }
    }
}

impl fmt::Debug for ProjectLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProjectLockError(")?;
        fmt::Display::fmt(self, formatter)?;
        formatter.write_str(")")
    }
}

impl std::error::Error for ProjectLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // 경로를 포함할 수 있는 원본 오류는 typed variant에 보존하지만 자동 source
        // chain에는 연결하지 않는다. 상위 계층은 안전한 category만 기록한다.
        None
    }
}

fn format_io_category(formatter: &mut fmt::Formatter<'_>, error: &io::Error) -> fmt::Result {
    write!(formatter, "I/O category {:?}", error.kind())?;
    if let Some(code) = error.raw_os_error() {
        write!(formatter, " (OS code {code})")?;
    }
    Ok(())
}

fn format_metadata_update_category(
    formatter: &mut fmt::Formatter<'_>,
    error: &MetadataUpdateError,
) -> fmt::Result {
    match error {
        MetadataUpdateError::FormatTime(_) => formatter.write_str("UTC time formatting category"),
        MetadataUpdateError::Save(SaveError::Serialize(_)) => {
            formatter.write_str("lock metadata serialization category")
        }
        MetadataUpdateError::Save(SaveError::AtomicWrite(source)) => {
            fmt::Display::fmt(source, formatter)
        }
    }
}

/// 프로젝트를 사용하는 동안 파일 핸들을 소유해 OS 잠금을 유지하는 non-Clone guard다.
///
/// 상위 프로젝트 열기 계층은 검증된 `project_root`와 설치 채널 간에 공유되는
/// `lock_root`를 전달해야 한다. 이 타입은 경로 선택이나 UI 수명주기에 결합하지 않는다.
pub(crate) struct ProjectLock {
    file: Option<File>,
    fingerprint: String,
    lock_path: PathBuf,
    metadata_path: PathBuf,
}

impl ProjectLock {
    pub(crate) fn try_acquire(
        project_root: &Path,
        lock_root: &Path,
    ) -> Result<Self, ProjectLockError> {
        let canonical = canonical_project_directory(project_root)?;
        let fingerprint = project_fingerprint(&canonical);
        let lock_directory = lock_root.join("locks");
        fs::create_dir_all(&lock_directory).map_err(|source| {
            ProjectLockError::LockDirectoryUnavailable {
                path: lock_directory.clone(),
                source,
            }
        })?;

        let lock_path = lock_directory.join(format!("{fingerprint}.lock"));
        let metadata_path = lock_directory.join(format!("{fingerprint}.json"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| ProjectLockError::LockFileOpenFailed {
                path: lock_path.clone(),
                source,
            })?;

        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let (metadata, metadata_error) =
                    read_conflict_metadata(&metadata_path, &fingerprint);
                return Err(ProjectLockError::AlreadyLocked {
                    fingerprint,
                    metadata,
                    metadata_error,
                });
            }
            Err(TryLockError::Error(source)) => {
                return Err(ProjectLockError::LockOperationFailed {
                    fingerprint,
                    source,
                });
            }
        }

        let acquired_at_utc = match now_utc_milliseconds() {
            Ok(value) => value,
            Err(source) => {
                return Err(metadata_failure(
                    file,
                    fingerprint,
                    MetadataUpdateError::FormatTime(source),
                ));
            }
        };
        let metadata = ProjectLockMetadata {
            schema_version: PROJECT_LOCK_METADATA_SCHEMA_VERSION,
            pid: std::process::id(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            acquired_at_utc,
            project_fingerprint: fingerprint.clone(),
        };
        if let Err(source) = save_deterministic_json(&metadata_path, &metadata) {
            return Err(metadata_failure(
                file,
                fingerprint,
                MetadataUpdateError::Save(source),
            ));
        }

        Ok(Self {
            file: Some(file),
            fingerprint,
            lock_path,
            metadata_path,
        })
    }

    /// 명시적 해제가 필요한 전환 흐름에서는 OS 오류를 호출자에게 돌려준다.
    pub(crate) fn release(mut self) -> Result<(), ProjectLockError> {
        let Some(file) = self.file.take() else {
            return Ok(());
        };
        file.unlock()
            .map_err(|source| ProjectLockError::UnlockFailed {
                fingerprint: self.fingerprint.clone(),
                source,
            })
    }

    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    pub(crate) fn metadata_path(&self) -> &Path {
        &self.metadata_path
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        // File을 닫으면 정상 종료와 unwind 모두에서 OS가 잠금을 해제한다.
        // 파일 삭제는 Unix inode 교체 경쟁으로 이중 잠금을 만들 수 있어 하지 않는다.
        drop(self.file.take());
    }
}

fn metadata_failure(
    file: File,
    fingerprint: String,
    source: MetadataUpdateError,
) -> ProjectLockError {
    let unlock_error = file.unlock().err();
    ProjectLockError::MetadataWriteFailed {
        fingerprint,
        source,
        unlock_error,
    }
}

pub(super) fn canonical_project_identity(project_root: &Path) -> io::Result<(PathBuf, String)> {
    let canonical = fs::canonicalize(project_root)?;
    let metadata = fs::metadata(&canonical)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "project root is not a directory",
        ));
    }
    let fingerprint = project_fingerprint(&canonical);
    Ok((canonical, fingerprint))
}

fn canonical_project_directory(project_root: &Path) -> Result<PathBuf, ProjectLockError> {
    canonical_project_identity(project_root)
        .map(|(canonical, _fingerprint)| canonical)
        .map_err(|source| ProjectLockError::InvalidProjectPath {
            path: project_root.to_path_buf(),
            source,
        })
}

fn project_fingerprint(canonical: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(LOCK_KEY_VERSION);
    hasher.update([0]);
    hasher.update(std::env::consts::OS.as_bytes());
    hasher.update([0]);
    update_hasher_with_path(&mut hasher, canonical);
    lowercase_hex(&hasher.finalize())
}

#[cfg(unix)]
fn update_hasher_with_path(hasher: &mut Sha256, path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    hasher.update(path.as_os_str().as_bytes());
}

#[cfg(windows)]
fn update_hasher_with_path(hasher: &mut Sha256, path: &Path) {
    use std::os::windows::ffi::OsStrExt;

    let mut units: Vec<u16> = path.as_os_str().encode_wide().collect();
    for unit in &mut units {
        if *unit == b'/' as u16 {
            *unit = b'\\' as u16;
        } else if (*unit >= b'A' as u16) && (*unit <= b'Z' as u16) {
            *unit += (b'a' - b'A') as u16;
        }
    }
    const EXTENDED: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const UNC: &[u16] = &[b'u' as u16, b'n' as u16, b'c' as u16, b'\\' as u16];
    let units = if let Some(rest) = units.strip_prefix(EXTENDED) {
        if let Some(unc_rest) = rest.strip_prefix(UNC) {
            // `\\?\UNC\server`와 `\\server`가 같은 입력이 되도록 UNC 루트를 복원한다.
            for unit in [b'\\' as u16, b'\\' as u16] {
                hasher.update(unit.to_le_bytes());
            }
            unc_rest
        } else {
            rest
        }
    } else {
        &units
    };
    for unit in units {
        hasher.update(unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, windows)))]
fn update_hasher_with_path(hasher: &mut Sha256, path: &Path) {
    hasher.update(path.to_string_lossy().as_bytes());
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

fn read_conflict_metadata(
    path: &Path,
    expected_fingerprint: &str,
) -> (Option<ProjectLockMetadata>, Option<MetadataReadError>) {
    match read_metadata(path, expected_fingerprint) {
        Ok(metadata) => (Some(metadata), None),
        Err(MetadataReadError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            (None, None)
        }
        Err(error) => (None, Some(error)),
    }
}

fn read_metadata(
    path: &Path,
    expected_fingerprint: &str,
) -> Result<ProjectLockMetadata, MetadataReadError> {
    let file = File::open(path).map_err(MetadataReadError::Io)?;
    let mut bytes = Vec::new();
    file.take(METADATA_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(MetadataReadError::Io)?;
    if bytes.len() as u64 > METADATA_LIMIT {
        return Err(MetadataReadError::TooLarge {
            limit: METADATA_LIMIT,
        });
    }
    let metadata: ProjectLockMetadata =
        serde_json::from_slice(&bytes).map_err(MetadataReadError::InvalidJson)?;
    if metadata.schema_version != PROJECT_LOCK_METADATA_SCHEMA_VERSION {
        return Err(MetadataReadError::UnsupportedSchema {
            found: metadata.schema_version.get(),
            supported: PROJECT_LOCK_METADATA_SCHEMA_VERSION.get(),
        });
    }
    if metadata.project_fingerprint != expected_fingerprint {
        return Err(MetadataReadError::FingerprintMismatch);
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::super::atomic_file::{AtomicWriteError, AtomicWriteStage, SaveOutcome};
    use super::super::json::to_deterministic_json_bytes;
    use super::*;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> io::Result<Self> {
            for _ in 0..128 {
                let count = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "worldbuild-project-lock-{}-{count}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "test directory collision",
            ))
        }

        fn project(&self, name: &str) -> io::Result<PathBuf> {
            let path = self.0.join(name);
            fs::create_dir(&path)?;
            Ok(path)
        }

        fn lock_root(&self) -> PathBuf {
            self.0.join("app-data")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _cleanup_result = fs::remove_dir_all(&self.0);
        }
    }

    fn already_locked(error: ProjectLockError) -> ProjectLockError {
        assert!(matches!(error, ProjectLockError::AlreadyLocked { .. }));
        error
    }

    #[test]
    fn first_lock_succeeds_and_duplicate_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let first = ProjectLock::try_acquire(&project, &temp.lock_root())?;
        let error = ProjectLock::try_acquire(&project, &temp.lock_root())
            .err()
            .ok_or("second acquisition unexpectedly succeeded")?;
        already_locked(error);
        drop(first);
        Ok(())
    }

    #[test]
    fn error_formatting_and_source_chain_redact_absolute_paths() -> io::Result<()> {
        let temp = TestDirectory::new()?;
        let sentinel = temp.0.join("private-user-name/project");
        let errors = [
            ProjectLockError::InvalidProjectPath {
                path: sentinel.clone(),
                source: io::Error::other(format!("secret at {}", sentinel.display())),
            },
            ProjectLockError::LockDirectoryUnavailable {
                path: sentinel.clone(),
                source: io::Error::other(format!("secret at {}", sentinel.display())),
            },
            ProjectLockError::LockFileOpenFailed {
                path: sentinel.clone(),
                source: io::Error::other(format!("secret at {}", sentinel.display())),
            },
            ProjectLockError::LockOperationFailed {
                fingerprint: "a".repeat(64),
                source: io::Error::other(format!("secret at {}", sentinel.display())),
            },
            ProjectLockError::MetadataWriteFailed {
                fingerprint: "b".repeat(64),
                source: MetadataUpdateError::Save(SaveError::AtomicWrite(AtomicWriteError {
                    stage: AtomicWriteStage::ReplaceTarget,
                    target: sentinel.clone(),
                    temporary_path: Some(sentinel.join("temporary")),
                    source: io::Error::other(format!("secret at {}", sentinel.display())),
                    cleanup_error: None,
                    outcome: SaveOutcome::NotApplied,
                })),
                unlock_error: Some(io::Error::other(format!(
                    "secret at {}",
                    sentinel.display()
                ))),
            },
            ProjectLockError::UnlockFailed {
                fingerprint: "c".repeat(64),
                source: io::Error::other(format!("secret at {}", sentinel.display())),
            },
        ];

        for error in errors {
            let diagnostic = format!("{error}\n{error:?}");
            assert!(diagnostic.contains("I/O category"));
            assert!(!diagnostic.contains(&sentinel.display().to_string()));
            assert!(std::error::Error::source(&error).is_none());
        }
        Ok(())
    }

    #[test]
    fn drop_and_explicit_release_allow_reacquisition() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        drop(ProjectLock::try_acquire(&project, &temp.lock_root())?);
        ProjectLock::try_acquire(&project, &temp.lock_root())?.release()?;
        ProjectLock::try_acquire(&project, &temp.lock_root())?.release()?;
        Ok(())
    }

    #[test]
    fn distinct_projects_can_be_locked_together() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let first = temp.project("first")?;
        let second = temp.project("second")?;
        let first_lock = ProjectLock::try_acquire(&first, &temp.lock_root())?;
        let second_lock = ProjectLock::try_acquire(&second, &temp.lock_root())?;
        assert_ne!(first_lock.fingerprint(), second_lock.fingerprint());
        Ok(())
    }

    #[test]
    fn dot_and_parent_aliases_share_the_same_lock() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        fs::create_dir(project.join("child"))?;
        let current = std::env::current_dir()?;
        let relative = relative_path(&current, &project).ok_or("test paths use different roots")?;
        let first = ProjectLock::try_acquire(&relative.join("."), &temp.lock_root())?;
        let alias = project.join("child").join("..");
        already_locked(
            ProjectLock::try_acquire(&alias, &temp.lock_root())
                .err()
                .ok_or("alias acquisition unexpectedly succeeded")?,
        );
        drop(first);
        Ok(())
    }

    fn relative_path(from: &Path, to: &Path) -> Option<PathBuf> {
        let from_components: Vec<_> = from.components().collect();
        let to_components: Vec<_> = to.components().collect();
        let shared = from_components
            .iter()
            .zip(&to_components)
            .take_while(|(left, right)| left == right)
            .count();
        if shared == 0 {
            return None;
        }
        let mut relative = PathBuf::new();
        for _ in &from_components[shared..] {
            relative.push("..");
        }
        for component in &to_components[shared..] {
            relative.push(component.as_os_str());
        }
        Some(relative)
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_alias_shares_the_same_lock() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let alias = temp.0.join("alias");
        symlink(&project, &alias)?;
        let first = ProjectLock::try_acquire(&project, &temp.lock_root())?;
        already_locked(
            ProjectLock::try_acquire(&alias, &temp.lock_root())
                .err()
                .ok_or("symlink alias acquisition unexpectedly succeeded")?,
        );
        drop(first);
        Ok(())
    }

    #[test]
    fn korean_project_path_is_supported() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("한글 세계")?;
        ProjectLock::try_acquire(&project, &temp.lock_root())?.release()?;
        Ok(())
    }

    #[test]
    fn stale_files_do_not_prevent_acquisition_and_are_never_deleted(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let first = ProjectLock::try_acquire(&project, &temp.lock_root())?;
        let lock_path = first.lock_path().to_path_buf();
        let metadata_path = first.metadata_path().to_path_buf();
        drop(first);
        assert!(lock_path.exists() && metadata_path.exists());
        let second = ProjectLock::try_acquire(&project, &temp.lock_root())?;
        drop(second);
        assert!(lock_path.exists() && metadata_path.exists());
        Ok(())
    }

    #[test]
    fn conflict_returns_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let first = ProjectLock::try_acquire(&project, &temp.lock_root())?;
        let error = ProjectLock::try_acquire(&project, &temp.lock_root())
            .err()
            .ok_or("duplicate acquisition unexpectedly succeeded")?;
        match error {
            ProjectLockError::AlreadyLocked {
                metadata,
                metadata_error,
                ..
            } => {
                let metadata = metadata.ok_or("metadata missing")?;
                assert_eq!(metadata.pid, std::process::id());
                assert!(metadata_error.is_none());
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }
        drop(first);
        Ok(())
    }

    #[test]
    fn malformed_conflict_metadata_does_not_change_lock_result(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let first = ProjectLock::try_acquire(&project, &temp.lock_root())?;
        fs::write(first.metadata_path(), b"not json")?;
        let error = ProjectLock::try_acquire(&project, &temp.lock_root())
            .err()
            .ok_or("duplicate acquisition unexpectedly succeeded")?;
        match error {
            ProjectLockError::AlreadyLocked {
                metadata,
                metadata_error,
                ..
            } => {
                assert!(metadata.is_none());
                assert!(matches!(
                    metadata_error,
                    Some(MetadataReadError::InvalidJson(_))
                ));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }
        drop(first);
        Ok(())
    }

    #[test]
    fn unsupported_lock_metadata_schema_is_reported_without_changing_lock_result(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let first = ProjectLock::try_acquire(&project, &temp.lock_root())?;
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(first.metadata_path())?)?;
        metadata["schemaVersion"] = serde_json::Value::from(2);
        fs::write(
            first.metadata_path(),
            to_deterministic_json_bytes(&metadata)?,
        )?;

        let error = ProjectLock::try_acquire(&project, &temp.lock_root())
            .err()
            .ok_or("duplicate acquisition unexpectedly succeeded")?;
        match error {
            ProjectLockError::AlreadyLocked {
                metadata,
                metadata_error,
                ..
            } => {
                assert!(metadata.is_none());
                assert!(matches!(
                    metadata_error,
                    Some(MetadataReadError::UnsupportedSchema {
                        found: 2,
                        supported: 1
                    })
                ));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }
        drop(first);
        Ok(())
    }

    #[test]
    fn metadata_write_failure_releases_lock() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let canonical = canonical_project_directory(&project)?;
        let fingerprint = project_fingerprint(&canonical);
        let locks = temp.lock_root().join("locks");
        fs::create_dir_all(&locks)?;
        fs::create_dir(locks.join(format!("{fingerprint}.json")))?;

        let error = ProjectLock::try_acquire(&project, &temp.lock_root())
            .err()
            .ok_or("metadata failure unexpectedly succeeded")?;
        assert!(matches!(
            error,
            ProjectLockError::MetadataWriteFailed { .. }
        ));

        fs::remove_dir(locks.join(format!("{fingerprint}.json")))?;
        ProjectLock::try_acquire(&project, &temp.lock_root())?.release()?;
        Ok(())
    }

    #[test]
    fn files_live_only_under_app_data() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let guard = ProjectLock::try_acquire(&project, &temp.lock_root())?;
        assert!(fs::read_dir(&project)?.next().is_none());
        assert!(guard.lock_path().starts_with(temp.lock_root()));
        assert!(guard.metadata_path().starts_with(temp.lock_root()));
        Ok(())
    }

    #[test]
    fn metadata_is_deterministic_and_contains_no_project_path(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("비공개 프로젝트")?;
        let guard = ProjectLock::try_acquire(&project, &temp.lock_root())?;
        let bytes = fs::read(guard.metadata_path())?;
        let metadata: ProjectLockMetadata = serde_json::from_slice(&bytes)?;
        assert_eq!(bytes, to_deterministic_json_bytes(&metadata)?);
        assert_eq!(PROJECT_LOCK_METADATA_SCHEMA_VERSION.get(), 1);
        assert_eq!(
            metadata.schema_version,
            PROJECT_LOCK_METADATA_SCHEMA_VERSION
        );
        assert_eq!(serde_json::to_value(&metadata)?["schemaVersion"], 1);
        let text = String::from_utf8(bytes)?;
        assert!(!text.contains("비공개 프로젝트"));
        assert!(!text.contains(&project.to_string_lossy().into_owned()));
        assert_eq!(metadata.app_version, env!("CARGO_PKG_VERSION"));
        Ok(())
    }

    #[test]
    fn utc_timestamp_has_exact_millisecond_z_shape() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let guard = ProjectLock::try_acquire(&project, &temp.lock_root())?;
        let metadata = read_metadata(guard.metadata_path(), guard.fingerprint())?;
        let bytes = metadata.acquired_at_utc.as_bytes();
        assert_eq!(bytes.len(), 24);
        for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 21, 22] {
            assert!(bytes[index].is_ascii_digit());
        }
        assert_eq!(&metadata.acquired_at_utc[4..5], "-");
        assert_eq!(&metadata.acquired_at_utc[10..11], "T");
        assert!(metadata.acquired_at_utc.ends_with('Z'));
        Ok(())
    }

    #[test]
    fn fingerprint_is_stable_and_lowercase_sha256() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let canonical = canonical_project_directory(&project)?;
        let first = project_fingerprint(&canonical);
        let second = project_fingerprint(&canonical_project_directory(&project.join("."))?);
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        Ok(())
    }

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _kill_result = self.0.kill();
            let _wait_result = self.0.wait();
        }
    }

    #[test]
    fn forced_child_exit_releases_os_lock_but_leaves_files(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = TestDirectory::new()?;
        let project = temp.project("project")?;
        let ready = temp.0.join("child-ready");
        let executable = std::env::current_exe()?;
        let child = Command::new(executable)
            .arg("--ignored")
            .arg("--exact")
            .arg("data::project_lock::tests::project_lock_child_helper")
            .arg("--nocapture")
            .env("WORLDBUILD_LOCK_CHILD_PROJECT", &project)
            .env("WORLDBUILD_LOCK_CHILD_ROOT", temp.lock_root())
            .env("WORLDBUILD_LOCK_CHILD_READY", &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut child = ChildGuard(child);
        wait_until(Duration::from_secs(10), || ready.exists())?;

        already_locked(
            ProjectLock::try_acquire(&project, &temp.lock_root())
                .err()
                .ok_or("parent unexpectedly acquired child lock")?,
        );
        child.0.kill()?;
        child.0.wait()?;

        let guard = retry_acquire(&project, &temp.lock_root(), Duration::from_secs(5))?;
        assert!(guard.lock_path().exists());
        assert!(guard.metadata_path().exists());
        Ok(())
    }

    #[test]
    #[ignore = "helper launched by forced_child_exit_releases_os_lock_but_leaves_files"]
    fn project_lock_child_helper() -> Result<(), Box<dyn std::error::Error>> {
        let Some(project) = std::env::var_os("WORLDBUILD_LOCK_CHILD_PROJECT") else {
            return Ok(());
        };
        let lock_root =
            std::env::var_os("WORLDBUILD_LOCK_CHILD_ROOT").ok_or("missing child lock root")?;
        let ready =
            std::env::var_os("WORLDBUILD_LOCK_CHILD_READY").ok_or("missing child ready path")?;
        let _guard = ProjectLock::try_acquire(Path::new(&project), Path::new(&lock_root))?;
        fs::write(ready, b"ready")?;
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }

    fn wait_until(
        timeout: Duration,
        condition: impl Fn() -> bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if condition() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        Err("timed out waiting for child process".into())
    }

    fn retry_acquire(
        project: &Path,
        lock_root: &Path,
        timeout: Duration,
    ) -> Result<ProjectLock, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + timeout;
        loop {
            match ProjectLock::try_acquire(project, lock_root) {
                Ok(guard) => return Ok(guard),
                Err(error @ ProjectLockError::AlreadyLocked { .. })
                    if Instant::now() < deadline =>
                {
                    let _diagnostic = error;
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}
