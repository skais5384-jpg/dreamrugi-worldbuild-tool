#[cfg(windows)]
pub(crate) mod owned;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use super::json::to_deterministic_json_bytes;

const MAX_TEMP_FILE_ATTEMPTS: usize = 128;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 저장 실패 시 대상에 새 내용이 적용되었는지를 구분한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    /// 교체 전 실패이므로 기존 대상은 변경되지 않았다.
    NotApplied,
    /// 교체는 끝났지만 후속 동기화에 실패해 전원 장애 내구성을 확정할 수 없다.
    AppliedDurabilityUncertain,
}

/// 단일 파일 저장 과정에서 실패한 작업 단계다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicWriteStage {
    ValidateTarget,
    CreateTemporary,
    WriteTemporary,
    FlushTemporary,
    SyncTemporary,
    ReplaceTarget,
    SyncAfterReplace,
}

impl fmt::Display for AtomicWriteStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match self {
            Self::ValidateTarget => "validate target path",
            Self::CreateTemporary => "create temporary file",
            Self::WriteTemporary => "write temporary file",
            Self::FlushTemporary => "flush temporary file",
            Self::SyncTemporary => "sync temporary file",
            Self::ReplaceTarget => "replace target file",
            Self::SyncAfterReplace => "sync replaced file metadata",
        };
        formatter.write_str(description)
    }
}

/// 원자적 단일 파일 저장의 진단 정보를 보존하는 오류다.
pub struct AtomicWriteError {
    pub stage: AtomicWriteStage,
    #[expect(
        dead_code,
        reason = "전체 경로는 자동 formatting에서 숨기고 명시적인 private 진단에서만 사용한다"
    )]
    pub target: PathBuf,
    pub temporary_path: Option<PathBuf>,
    pub source: io::Error,
    pub cleanup_error: Option<io::Error>,
    pub outcome: SaveOutcome,
}

impl fmt::Display for AtomicWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "failed to {}; ", self.stage)?;
        format_io_category(formatter, &self.source)?;
        if self.temporary_path.is_some() {
            formatter.write_str("; temporary artifact allocated")?;
        }
        if let Some(cleanup_error) = &self.cleanup_error {
            formatter.write_str("; temporary artifact cleanup also failed (")?;
            format_io_category(formatter, cleanup_error)?;
            formatter.write_str(")")?;
        }
        Ok(())
    }
}

impl fmt::Debug for AtomicWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AtomicWriteError")
            .field("stage", &self.stage)
            .field("target", &"[redacted]")
            .field(
                "temporary_path",
                &self.temporary_path.as_ref().map(|_| "[redacted]"),
            )
            .field("source_kind", &self.source.kind())
            .field("source_os_code", &self.source.raw_os_error())
            .field(
                "cleanup_kind",
                &self.cleanup_error.as_ref().map(io::Error::kind),
            )
            .field(
                "cleanup_os_code",
                &self
                    .cleanup_error
                    .as_ref()
                    .and_then(io::Error::raw_os_error),
            )
            .field("outcome", &self.outcome)
            .finish()
    }
}

impl std::error::Error for AtomicWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // 원본 오류는 typed field로 보존하되, 임의 문자열이 포함될 수 있는 io::Error를
        // 자동 source chain에 노출하지 않는다.
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

/// 결정적 JSON 생성 실패와 파일 저장 실패를 구분한다.
#[derive(Debug)]
pub enum SaveError {
    Serialize(serde_json::Error),
    AtomicWrite(AtomicWriteError),
}

impl fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(error) => {
                write!(formatter, "failed to serialize deterministic JSON: {error}")
            }
            Self::AtomicWrite(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
            Self::AtomicWrite(error) => Some(error),
        }
    }
}

/// 값을 완전히 직렬화한 뒤 검증된 저장 계층 내부에서 단일 파일로 저장한다.
///
/// 원시 경로를 받으므로 crate 밖에는 공개하지 않는다. 향후 프로젝트 경계와
/// 심볼릭 링크를 검증한 경로 타입만 이 함수를 호출하도록 연결해야 한다.
pub fn save_deterministic_json<T>(target: &Path, value: &T) -> Result<(), SaveError>
where
    T: Serialize + ?Sized,
{
    let bytes = to_deterministic_json_bytes(value).map_err(SaveError::Serialize)?;
    write_atomic_with(target, &bytes, &StdFileOperations).map_err(SaveError::AtomicWrite)
}

#[cfg(test)]
pub(crate) fn save_deterministic_json_with_after_replace_hook<T>(
    target: &Path,
    value: &T,
    after_replace: impl Fn(&Path) -> io::Result<()>,
) -> Result<(), SaveError>
where
    T: Serialize + ?Sized,
{
    let bytes = to_deterministic_json_bytes(value).map_err(SaveError::Serialize)?;
    write_atomic_with(
        target,
        &bytes,
        &AfterReplaceTestOperations { after_replace },
    )
    .map_err(SaveError::AtomicWrite)
}

/// 이미 검증한 backup 원본 바이트를 M1-3과 같은 원자 교체 절차로 복원한다.
pub(crate) fn save_bytes_atomic(target: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    write_atomic_with(target, bytes, &StdFileOperations)
}

trait FileOperations {
    type Handle: Write;

    fn create_new(&self, path: &Path) -> io::Result<Self::Handle>;
    fn sync_all(&self, handle: &Self::Handle) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove_temporary(&self, path: &Path) -> io::Result<()>;
    #[cfg(test)]
    fn after_replace_before_sync(&self, _target: &Path) -> io::Result<()> {
        Ok(())
    }
    fn sync_after_replace(&self, target: &Path, parent: &Path) -> io::Result<()>;
}

struct StdFileOperations;

#[cfg(test)]
struct AfterReplaceTestOperations<F> {
    after_replace: F,
}

impl FileOperations for StdFileOperations {
    type Handle = File;

    fn create_new(&self, path: &Path) -> io::Result<Self::Handle> {
        OpenOptions::new().write(true).create_new(true).open(path)
    }

    fn sync_all(&self, handle: &Self::Handle) -> io::Result<()> {
        handle.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn remove_temporary(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    #[cfg(unix)]
    fn sync_after_replace(&self, _target: &Path, parent: &Path) -> io::Result<()> {
        File::open(parent)?.sync_all()
    }

    #[cfg(windows)]
    fn sync_after_replace(&self, target: &Path, _parent: &Path) -> io::Result<()> {
        // Windows std에는 부모 디렉터리 엔트리를 동기화하는 이식성 있는 API가 없다.
        // 교체된 파일을 다시 동기화해 가능한 범위만 강화한다.
        OpenOptions::new().write(true).open(target)?.sync_all()
    }

    #[cfg(not(any(unix, windows)))]
    fn sync_after_replace(&self, target: &Path, _parent: &Path) -> io::Result<()> {
        File::open(target)?.sync_all()
    }
}

#[cfg(test)]
impl<F> FileOperations for AfterReplaceTestOperations<F>
where
    F: Fn(&Path) -> io::Result<()>,
{
    type Handle = File;

    fn create_new(&self, path: &Path) -> io::Result<Self::Handle> {
        StdFileOperations.create_new(path)
    }

    fn sync_all(&self, handle: &Self::Handle) -> io::Result<()> {
        StdFileOperations.sync_all(handle)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        StdFileOperations.rename(from, to)
    }

    fn remove_temporary(&self, path: &Path) -> io::Result<()> {
        StdFileOperations.remove_temporary(path)
    }

    fn after_replace_before_sync(&self, target: &Path) -> io::Result<()> {
        (self.after_replace)(target)
    }

    fn sync_after_replace(&self, target: &Path, parent: &Path) -> io::Result<()> {
        StdFileOperations.sync_after_replace(target, parent)
    }
}

fn write_atomic_with<O>(target: &Path, bytes: &[u8], operations: &O) -> Result<(), AtomicWriteError>
where
    O: FileOperations,
{
    let parent = validate_target(target)?;
    let (temporary_path, mut temporary_file) = create_temporary(target, &parent, operations)?;

    if let Err(source) = temporary_file.write_all(bytes) {
        drop(temporary_file);
        return Err(error_with_cleanup(
            AtomicWriteStage::WriteTemporary,
            target,
            &temporary_path,
            source,
            operations,
        ));
    }
    if let Err(source) = temporary_file.flush() {
        drop(temporary_file);
        return Err(error_with_cleanup(
            AtomicWriteStage::FlushTemporary,
            target,
            &temporary_path,
            source,
            operations,
        ));
    }
    if let Err(source) = operations.sync_all(&temporary_file) {
        drop(temporary_file);
        return Err(error_with_cleanup(
            AtomicWriteStage::SyncTemporary,
            target,
            &temporary_path,
            source,
            operations,
        ));
    }

    // Windows에서도 열린 핸들이 교체를 방해하지 않도록 rename 전에 명시적으로 닫는다.
    drop(temporary_file);

    if let Err(source) = operations.rename(&temporary_path, target) {
        return Err(error_with_cleanup(
            AtomicWriteStage::ReplaceTarget,
            target,
            &temporary_path,
            source,
            operations,
        ));
    }

    #[cfg(test)]
    if let Err(source) = operations.after_replace_before_sync(target) {
        return Err(AtomicWriteError {
            stage: AtomicWriteStage::SyncAfterReplace,
            target: target.to_path_buf(),
            temporary_path: Some(temporary_path),
            source,
            cleanup_error: None,
            outcome: SaveOutcome::AppliedDurabilityUncertain,
        });
    }

    if let Err(source) = operations.sync_after_replace(target, &parent) {
        return Err(AtomicWriteError {
            stage: AtomicWriteStage::SyncAfterReplace,
            target: target.to_path_buf(),
            temporary_path: Some(temporary_path),
            source,
            cleanup_error: None,
            outcome: SaveOutcome::AppliedDurabilityUncertain,
        });
    }

    Ok(())
}

fn validate_target(target: &Path) -> Result<PathBuf, AtomicWriteError> {
    if target.file_name().is_none() {
        return Err(simple_error(
            AtomicWriteStage::ValidateTarget,
            target,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "target path must contain a file name",
            ),
        ));
    }

    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    match fs::metadata(parent) {
        Ok(metadata) if metadata.is_dir() => Ok(parent.to_path_buf()),
        Ok(_) => Err(simple_error(
            AtomicWriteStage::ValidateTarget,
            target,
            io::Error::new(
                io::ErrorKind::NotADirectory,
                "target parent is not a directory",
            ),
        )),
        Err(source) => Err(simple_error(
            AtomicWriteStage::ValidateTarget,
            target,
            source,
        )),
    }
}

fn create_temporary<O>(
    target: &Path,
    parent: &Path,
    operations: &O,
) -> Result<(PathBuf, O::Handle), AtomicWriteError>
where
    O: FileOperations,
{
    let file_name = target.file_name().ok_or_else(|| {
        simple_error(
            AtomicWriteStage::ValidateTarget,
            target,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "target path must contain a file name",
            ),
        )
    })?;

    for _ in 0..MAX_TEMP_FILE_ATTEMPTS {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = OsString::from(".");
        temporary_name.push(file_name);
        temporary_name.push(format!(".tmp-{}-{counter}", std::process::id()));
        let temporary_path = parent.join(temporary_name);

        match operations.create_new(&temporary_path) {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(AtomicWriteError {
                    stage: AtomicWriteStage::CreateTemporary,
                    target: target.to_path_buf(),
                    temporary_path: Some(temporary_path),
                    source,
                    cleanup_error: None,
                    outcome: SaveOutcome::NotApplied,
                });
            }
        }
    }

    Err(simple_error(
        AtomicWriteStage::CreateTemporary,
        target,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique temporary file name",
        ),
    ))
}

fn error_with_cleanup<O>(
    stage: AtomicWriteStage,
    target: &Path,
    temporary_path: &Path,
    source: io::Error,
    operations: &O,
) -> AtomicWriteError
where
    O: FileOperations,
{
    let cleanup_error = operations.remove_temporary(temporary_path).err();
    AtomicWriteError {
        stage,
        target: target.to_path_buf(),
        temporary_path: Some(temporary_path.to_path_buf()),
        source,
        cleanup_error,
        outcome: SaveOutcome::NotApplied,
    }
}

fn simple_error(stage: AtomicWriteStage, target: &Path, source: io::Error) -> AtomicWriteError {
    AtomicWriteError {
        stage,
        target: target.to_path_buf(),
        temporary_path: None,
        source,
        cleanup_error: None,
        outcome: SaveOutcome::NotApplied,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    use serde::{ser::Error as _, Serialize, Serializer};
    use serde_json::json;

    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FailurePoint {
        Write,
        Flush,
        SyncTemporary,
        Rename,
        SyncAfterReplace,
    }

    struct InjectedHandle {
        file: File,
        failure: Option<FailurePoint>,
    }

    impl Write for InjectedHandle {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.failure == Some(FailurePoint::Write) {
                return Err(injected_error("write"));
            }
            self.file.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.failure == Some(FailurePoint::Flush) {
                return Err(injected_error("flush"));
            }
            self.file.flush()
        }
    }

    struct InjectedOperations {
        failure: Option<FailurePoint>,
        cleanup_failure: bool,
        created_paths: RefCell<Vec<PathBuf>>,
        rename_count: Cell<usize>,
        cleanup_count: Cell<usize>,
    }

    impl InjectedOperations {
        fn new(failure: Option<FailurePoint>) -> Self {
            Self {
                failure,
                cleanup_failure: false,
                created_paths: RefCell::new(Vec::new()),
                rename_count: Cell::new(0),
                cleanup_count: Cell::new(0),
            }
        }

        fn with_cleanup_failure(failure: FailurePoint) -> Self {
            Self {
                failure: Some(failure),
                cleanup_failure: true,
                created_paths: RefCell::new(Vec::new()),
                rename_count: Cell::new(0),
                cleanup_count: Cell::new(0),
            }
        }
    }

    impl FileOperations for InjectedOperations {
        type Handle = InjectedHandle;

        fn create_new(&self, path: &Path) -> io::Result<Self::Handle> {
            self.created_paths.borrow_mut().push(path.to_path_buf());
            let file = OpenOptions::new().write(true).create_new(true).open(path)?;
            Ok(InjectedHandle {
                file,
                failure: self.failure,
            })
        }

        fn sync_all(&self, handle: &Self::Handle) -> io::Result<()> {
            if self.failure == Some(FailurePoint::SyncTemporary) {
                return Err(injected_error("temporary sync"));
            }
            handle.file.sync_all()
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.rename_count.set(self.rename_count.get() + 1);
            if self.failure == Some(FailurePoint::Rename) {
                return Err(injected_error("rename"));
            }
            fs::rename(from, to)
        }

        fn remove_temporary(&self, path: &Path) -> io::Result<()> {
            self.cleanup_count.set(self.cleanup_count.get() + 1);
            if self.cleanup_failure {
                return Err(injected_error("cleanup"));
            }
            fs::remove_file(path)
        }

        fn sync_after_replace(&self, target: &Path, _parent: &Path) -> io::Result<()> {
            if self.failure == Some(FailurePoint::SyncAfterReplace) {
                return Err(injected_error("post-replace sync"));
            }
            OpenOptions::new().write(true).open(target)?.sync_all()
        }
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> io::Result<Self> {
            let root = std::env::temp_dir();
            for _ in 0..MAX_TEMP_FILE_ATTEMPTS {
                let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = root.join(format!(
                    "worldbuild-tool-atomic-test-{}-{counter}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self { path }),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not create unique test directory",
            ))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn saves_new_file_with_exact_deterministic_bytes_in_same_directory(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let target = directory.path.join("project.json");
        let value = json!({"z": 1, "a": "세계"});
        let expected = to_deterministic_json_bytes(&value)?;
        let operations = InjectedOperations::new(None);

        write_atomic_with(&target, &expected, &operations)?;

        assert_eq!(fs::read(&target)?, expected);
        assert_eq!(operations.rename_count.get(), 1);
        assert_eq!(operations.cleanup_count.get(), 0);
        assert!(operations
            .created_paths
            .borrow()
            .iter()
            .all(|path| path.parent() == Some(directory.path.as_path())));
        assert_no_temporary_files(&directory.path)?;
        Ok(())
    }

    #[test]
    fn replaces_existing_file_without_removing_it_first() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = TestDirectory::new()?;
        let target = directory.path.join("project.json");
        fs::write(&target, b"old")?;
        let operations = InjectedOperations::new(None);

        write_atomic_with(&target, b"new", &operations)?;

        assert_eq!(fs::read(&target)?, b"new");
        assert_eq!(operations.rename_count.get(), 1);
        assert_eq!(operations.cleanup_count.get(), 0);
        assert_no_temporary_files(&directory.path)?;
        Ok(())
    }

    struct AlwaysFails;

    impl Serialize for AlwaysFails {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(S::Error::custom("intentional serialization failure"))
        }
    }

    #[test]
    fn serialization_failure_does_not_touch_filesystem() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let target = directory.path.join("project.json");
        fs::write(&target, b"old")?;

        let result = save_deterministic_json(&target, &AlwaysFails);

        assert!(matches!(result, Err(SaveError::Serialize(_))));
        assert_eq!(fs::read(&target)?, b"old");
        assert_no_temporary_files(&directory.path)?;
        Ok(())
    }

    #[test]
    fn pre_replace_failures_preserve_target_and_clean_temporary_file(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for (failure, expected_stage) in [
            (FailurePoint::Write, AtomicWriteStage::WriteTemporary),
            (FailurePoint::Flush, AtomicWriteStage::FlushTemporary),
            (FailurePoint::SyncTemporary, AtomicWriteStage::SyncTemporary),
            (FailurePoint::Rename, AtomicWriteStage::ReplaceTarget),
        ] {
            let directory = TestDirectory::new()?;
            let target = directory.path.join("project.json");
            fs::write(&target, b"old")?;
            let operations = InjectedOperations::new(Some(failure));

            let error = write_atomic_with(&target, b"new", &operations)
                .expect_err("injected operation should fail");

            assert_eq!(error.stage, expected_stage);
            assert_eq!(error.outcome, SaveOutcome::NotApplied);
            assert!(error.cleanup_error.is_none());
            assert_eq!(fs::read(&target)?, b"old");
            assert_eq!(operations.cleanup_count.get(), 1);
            assert_no_temporary_files(&directory.path)?;
        }
        Ok(())
    }

    #[test]
    fn cleanup_failure_does_not_hide_original_error() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let target = directory.path.join("project.json");
        fs::write(&target, b"old")?;
        let operations = InjectedOperations::with_cleanup_failure(FailurePoint::Write);

        let error = write_atomic_with(&target, b"new", &operations)
            .expect_err("write and cleanup should fail");

        assert_eq!(error.stage, AtomicWriteStage::WriteTemporary);
        assert!(error.source.to_string().contains("injected write failure"));
        assert!(error.cleanup_error.is_some());
        let temporary_path = error
            .temporary_path
            .as_deref()
            .ok_or_else(|| io::Error::other("temporary path should be recorded"))?;
        assert!(temporary_path.exists());
        fs::remove_file(temporary_path)?;
        Ok(())
    }

    #[test]
    fn post_replace_sync_failure_reports_applied_uncertain(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let target = directory.path.join("project.json");
        fs::write(&target, b"old")?;
        let operations = InjectedOperations::new(Some(FailurePoint::SyncAfterReplace));

        let error = write_atomic_with(&target, b"new", &operations)
            .expect_err("post-replace sync should fail");

        assert_eq!(error.stage, AtomicWriteStage::SyncAfterReplace);
        assert_eq!(error.outcome, SaveOutcome::AppliedDurabilityUncertain);
        assert_eq!(fs::read(&target)?, b"new");
        assert_no_temporary_files(&directory.path)?;
        Ok(())
    }

    #[test]
    fn error_formatting_and_source_chain_redact_absolute_paths(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let sentinel = directory.path.join("private-user-name/project.json");
        let temporary = directory
            .path
            .join("private-user-name/.project.json.tmp-sentinel");
        let error = AtomicWriteError {
            stage: AtomicWriteStage::ReplaceTarget,
            target: sentinel.clone(),
            temporary_path: Some(temporary.clone()),
            source: io::Error::other(format!("secret source at {}", sentinel.display())),
            cleanup_error: Some(io::Error::other(format!(
                "secret cleanup at {}",
                temporary.display()
            ))),
            outcome: SaveOutcome::NotApplied,
        };
        let diagnostic = format!("{error}\n{error:?}");

        assert!(diagnostic.contains("replace target file"));
        assert!(diagnostic.contains("I/O category"));
        assert!(!diagnostic.contains(&sentinel.display().to_string()));
        assert!(!diagnostic.contains(&temporary.display().to_string()));
        assert!(std::error::Error::source(&error).is_none());
        Ok(())
    }

    #[test]
    fn rejects_missing_parent_without_creating_it() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDirectory::new()?;
        let missing_parent = directory.path.join("missing");
        let target = missing_parent.join("project.json");

        let error = write_atomic_with(&target, b"new", &InjectedOperations::new(None))
            .expect_err("missing parent should fail validation");

        assert_eq!(error.stage, AtomicWriteStage::ValidateTarget);
        assert_eq!(error.outcome, SaveOutcome::NotApplied);
        assert!(!missing_parent.exists());
        Ok(())
    }

    fn assert_no_temporary_files(directory: &Path) -> io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let name = entry?.file_name();
            if name.to_string_lossy().contains(".tmp-") {
                return Err(io::Error::other(format!(
                    "temporary file remained: {}",
                    name.to_string_lossy()
                )));
            }
        }
        Ok(())
    }

    fn injected_error(operation: &str) -> io::Error {
        io::Error::other(format!("injected {operation} failure"))
    }
}
