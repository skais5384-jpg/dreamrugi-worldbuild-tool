//! M5-2·3의 저장된 프로젝트 snapshot, 백업, 정확한 관리 집합 복원 경계.
//! frontend 경로 문자열은 권한이 아니며 이 모듈에서 canonical 경계와 manifest를 다시 검증한다.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

#[cfg(test)]
thread_local! {
    static TEST_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static TEST_DELETE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
static ROOT_TEST_FAULTS: std::sync::OnceLock<
    std::sync::Mutex<BTreeMap<PathBuf, Vec<&'static str>>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) fn set_root_test_faults(root: &Path, faults: Vec<&'static str>) {
    let root = fs::canonicalize(root).expect("test fault root must exist");
    ROOT_TEST_FAULTS
        .get_or_init(Default::default)
        .lock()
        .expect("root test fault lock")
        .insert(root, faults);
}

#[cfg(test)]
fn take_root_test_fault(root: &Path, point: &'static str) -> bool {
    let Ok(root) = fs::canonicalize(root) else {
        return false;
    };
    let Some(faults) = ROOT_TEST_FAULTS.get() else {
        return false;
    };
    let mut faults = faults.lock().expect("root test fault lock");
    let Some(points) = faults.get_mut(&root) else {
        return false;
    };
    let Some(index) = points.iter().position(|candidate| *candidate == point) else {
        return false;
    };
    points.remove(index);
    if points.is_empty() {
        faults.remove(&root);
    }
    true
}

#[cfg(not(test))]
fn take_root_test_fault(_: &Path, _: &'static str) -> bool {
    false
}

#[cfg(test)]
fn run_delete_hook() {
    if let Some(hook) = TEST_DELETE_HOOK.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
}

#[cfg(not(test))]
fn run_delete_hook() {}

#[cfg(test)]
fn take_test_fault(point: &'static str) -> bool {
    TEST_FAULT.with(|fault| {
        if fault.get() == Some(point) {
            fault.set(None);
            true
        } else {
            false
        }
    })
}

#[cfg(not(test))]
fn take_test_fault(_: &'static str) -> bool {
    false
}

use super::{atomic_file, json};

const BACKUP_DIRECTORY: &str = "worldbuild-backups";
const DELETED_DIRECTORY: &str = ".deleted-backups";
const RECEIPT_SUFFIX: &str = ".receipt.json";
const PURGING_SUFFIX: &str = ".purging";
const MAX_RECEIPT_BYTES: u64 = 4096;
const MAX_DELETED_BACKUPS: usize = 10;
const MAX_DELETED_BYTES: u64 = 5 * 1024 * 1024 * 1024;

fn deleted_capacity_allows(count: usize, used_bytes: u64, incoming_bytes: u64) -> bool {
    count < MAX_DELETED_BACKUPS
        && incoming_bytes <= MAX_DELETED_BYTES
        && used_bytes <= MAX_DELETED_BYTES - incoming_bytes
}
const MANIFEST_NAME: &str = "worldbuild-backup.json";
const PAYLOAD_DIRECTORY: &str = "payload";
const RESTORE_DIRECTORY: &str = ".worldbuild-restore-v1";
const RESTORE_MANIFEST: &str = "restore.json";
const COMMIT_MARKER: &str = "commit";
const MAX_FILES: usize = 100_000;
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;
const BUFFER_BYTES: usize = 128 * 1024;
const CURRENT_MANIFEST_SCHEMA: u32 = 2;

fn manifest_size_allowed(size: u64) -> bool {
    size <= MAX_MANIFEST_BYTES
}
const FORMAT_HISTORY_DIRECTORY: &str = "format-history";
const MANAGED_NAMESPACES: [&str; 4] = ["templates", "documents", "workspace", "assets"];
const EXCLUDED_ROOT_ENTRIES: [&str; 4] = [".git", ".svn", ".worldbuild", RESTORE_DIRECTORY];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackupKind {
    Manual,
    PreRestore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupRow {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) created_at_utc: String,
    pub(crate) kind: BackupKind,
    pub(crate) size: String,
    pub(crate) status: &'static str,
    pub(crate) coverage: &'static str,
    pub(crate) unnamed_ordinal: Option<usize>,
    /// UI 기본 목록에는 표시하지 않는 opaque native locator다. 실행 때 manifest를 다시 검증한다.
    pub(crate) locator: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotResult {
    pub(crate) root: PathBuf,
    pub(crate) row: Option<BackupRow>,
    pub(crate) safety: Option<BackupRow>,
    pub(crate) outcome: &'static str,
    pub(crate) warning: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupPage {
    pub(crate) backups: Vec<BackupRow>,
    pub(crate) next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeletedBackupRow {
    pub(crate) backup: BackupRow,
    pub(crate) deleted_at_utc: String,
    pub(crate) expires_at_utc: String,
    pub(crate) operation: String,
    pub(crate) status: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteReceipt {
    schema_version: u32,
    id: String,
    project_fingerprint: String,
    operation: String,
    deleted_at_utc: String,
    expires_at_utc: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeleteBackupResult {
    pub(crate) outcome: &'static str,
    pub(crate) warning: Option<&'static str>,
    pub(crate) failure: Option<BackupCategory>,
}

#[derive(Debug)]
pub(crate) struct BackupError {
    category: BackupCategory,
    io: Option<io::Error>,
    safety: Option<BackupRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupCategory {
    InvalidInput,
    SourceChanged,
    DestinationOccupied,
    Corrupt,
    Unsupported,
    TooLarge,
    RetentionFull,
    Cancelled,
    Io,
    RecoveryRequired,
}

impl BackupError {
    fn new(category: BackupCategory) -> Self {
        Self {
            category,
            io: None,
            safety: None,
        }
    }
    fn io(error: io::Error) -> Self {
        Self {
            category: BackupCategory::Io,
            io: Some(error),
            safety: None,
        }
    }
    pub(crate) fn category(&self) -> BackupCategory {
        self.category
    }
    pub(crate) fn io_kind(&self) -> Option<io::ErrorKind> {
        self.io.as_ref().map(io::Error::kind)
    }
    pub(crate) fn safety(&self) -> Option<BackupRow> {
        self.safety.clone()
    }
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "project snapshot operation failed ({:?})",
            self.category
        )
    }
}
impl std::error::Error for BackupError {}
impl From<io::Error> for BackupError {
    fn from(value: io::Error) -> Self {
        Self::io(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    id: String,
    project_fingerprint: String,
    created_at_utc: String,
    kind: BackupKind,
    label: String,
    directories: Vec<String>,
    files: Vec<ManifestFile>,
    total_bytes: u64,
    complete: bool,
    #[serde(default)]
    format_history_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestFile {
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreJournal {
    schema_version: u32,
    operation_id: String,
    target_files: Vec<ManifestFile>,
    target_directories: Vec<String>,
}

fn invalid() -> BackupError {
    BackupError::new(BackupCategory::InvalidInput)
}

fn checkpoint(file: bool) -> Result<(), BackupError> {
    super::repository::progress::checkpoint(file)
        .map_err(|_| BackupError::new(BackupCategory::Cancelled))
}

fn preparing() -> Result<(), BackupError> {
    super::repository::progress::preparing()
        .map_err(|_| BackupError::new(BackupCategory::Cancelled))
}

#[cfg(test)]
fn process_crash_checkpoint(point: &str) {
    if std::env::var("WB_M523_CRASH_POINT").ok().as_deref() != Some(point) {
        return;
    }
    let ready = std::env::var_os("WB_M523_CRASH_READY")
        .map(PathBuf::from)
        .expect("crash checkpoint requires a handshake path");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(ready)
        .expect("create crash handshake");
    file.write_all(point.as_bytes()).expect("write handshake");
    file.sync_all().expect("sync handshake");
    loop {
        std::thread::park_timeout(std::time::Duration::from_secs(60));
    }
}

#[cfg(not(test))]
fn process_crash_checkpoint(_: &str) {}

fn now() -> Result<String, BackupError> {
    time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .map_err(|_| BackupError::new(BackupCategory::Io))
}

fn safe_label(value: &str) -> bool {
    value.len() <= 120 && !value.contains(['\0', '\r', '\n']) && value.trim() == value
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value == value.trim()
        && !value.chars().any(|c| {
            c <= '\u{1f}' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        && !value.ends_with(['.', ' '])
        && !matches!(value, "." | "..")
        && {
            let stem = value
                .split('.')
                .next()
                .unwrap_or(value)
                .to_ascii_uppercase();
            !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        }
}

fn canonical_existing_directory(path: &Path) -> Result<PathBuf, BackupError> {
    if !path.is_absolute() {
        return Err(invalid());
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || super::project_file::directory::is_reparse(&metadata) {
        return Err(invalid());
    }
    fs::canonicalize(path).map_err(Into::into)
}

fn normalized(path: &Path) -> String {
    let mut value = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = value.strip_prefix("//?/UNC/") {
        value = format!("//{rest}");
    } else if let Some(rest) = value.strip_prefix("//?/") {
        value = rest.into();
    }
    value.trim_end_matches('/').to_ascii_lowercase()
}

fn disjoint(left: &Path, right: &Path) -> bool {
    let left = normalized(left);
    let right = normalized(right);
    !(left == right || left.starts_with(&(right.clone() + "/")) || right.starts_with(&(left + "/")))
}

fn validate_relative(value: &str) -> Result<PathBuf, BackupError> {
    if value.is_empty() || value.contains('\\') || value.contains('\0') || value.contains(':') {
        return Err(invalid());
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid());
    }
    let parts = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().ok_or_else(invalid),
            _ => Err(invalid()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let managed = parts
        .first()
        .is_some_and(|value| MANAGED_NAMESPACES.contains(value));
    let history =
        parts.len() >= 2 && parts[0] == ".worldbuild" && parts[1] == FORMAT_HISTORY_DIRECTORY;
    if !managed && !history && value != crate::svn::policy::FILE {
        return Err(invalid());
    }
    Ok(path.to_owned())
}

#[cfg(windows)]
fn hard_link_count(metadata: &fs::Metadata) -> u64 {
    // MetadataExt::number_of_links는 현재 toolchain에서 unstable이므로 이 값만으로
    // 안전을 낮추지 않는다. 실제 열린 file handle의 Win32 정보를 아래 helper에서 확인한다.
    let _ = metadata;
    1
}

#[cfg(windows)]
fn opened_hard_link_count(file: &File) -> io::Result<u64> {
    super::edit_recovery::native::link_count(file).map(u64::from)
}

#[cfg(not(windows))]
fn opened_hard_link_count(file: &File) -> io::Result<u64> {
    Ok(hard_link_count(&file.metadata()?))
}
#[cfg(unix)]
fn hard_link_count(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink()
}
#[cfg(not(any(windows, unix)))]
fn hard_link_count(_: &fs::Metadata) -> u64 {
    2
}

fn relative_string(root: &Path, path: &Path) -> Result<String, BackupError> {
    let relative = path.strip_prefix(root).map_err(|_| invalid())?;
    let mut parts = Vec::new();
    for part in relative.components() {
        let Component::Normal(part) = part else {
            return Err(invalid());
        };
        let part = part.to_str().ok_or_else(invalid)?;
        if part.is_empty() || matches!(part, "." | "..") || part.contains(['/', '\\', ':', '\0']) {
            return Err(invalid());
        }
        parts.push(part);
    }
    let value = parts.join("/");
    validate_relative(&value)?;
    Ok(value)
}

fn collect(root: &Path) -> Result<(Vec<String>, Vec<ManifestFile>), BackupError> {
    collect_with_root_entries(root, true)
}

fn collect_with_root_entries(
    root: &Path,
    require_backup_root_entries: bool,
) -> Result<(Vec<String>, Vec<ManifestFile>), BackupError> {
    checkpoint(false)?;
    let root = canonical_existing_directory(root)?;
    if super::asset_maintenance::has_pending(&root)
        .map_err(|_| BackupError::new(BackupCategory::Corrupt))?
    {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    if require_backup_root_entries {
        validate_root_entries(&root)?;
    }
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut total = 0_u64;
    for namespace in MANAGED_NAMESPACES {
        checkpoint(false)?;
        let path = root.join(namespace);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_dir() || super::project_file::directory::is_reparse(&metadata) {
            return Err(BackupError::new(BackupCategory::Corrupt));
        }
        collect_directory(&root, &path, &mut directories, &mut files, &mut total)?;
    }
    let policy = root.join(crate::svn::policy::FILE);
    match fs::symlink_metadata(&policy) {
        Ok(_) => {
            let (size, sha256) = hash_file(&policy)?;
            if size > 16 * 1024 {
                return Err(BackupError::new(BackupCategory::TooLarge));
            }
            files.push(ManifestFile {
                path: crate::svn::policy::FILE.into(),
                size,
                sha256,
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let history = root.join(".worldbuild").join(FORMAT_HISTORY_DIRECTORY);
    match fs::symlink_metadata(&history) {
        Ok(metadata) => {
            if !metadata.is_dir() || super::project_file::directory::is_reparse(&metadata) {
                return Err(BackupError::new(BackupCategory::Corrupt));
            }
            collect_directory(&root, &history, &mut directories, &mut files, &mut total)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    directories.sort();
    directories.dedup();
    files.sort();
    Ok((directories, files))
}

/// Reuse the snapshot's guarded walk for SVN registration. The caller still
/// decides which managed paths are shareable; format history remains local.
pub(crate) fn snapshot_paths(root: &Path) -> Result<(Vec<String>, Vec<String>), BackupError> {
    let (directories, files) = collect_with_root_entries(root, false)?;
    Ok((
        directories,
        files.into_iter().map(|file| file.path).collect(),
    ))
}

fn validate_root_entries(root: &Path) -> Result<(), BackupError> {
    let guard = super::project_file::directory::ProjectDirectory::open_root(root)?;
    let entries = guard.read_dir()?.collect::<Result<Vec<_>, _>>()?;
    for entry in entries {
        checkpoint(false)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(invalid)?;
        if !MANAGED_NAMESPACES.contains(&name)
            && !EXCLUDED_ROOT_ENTRIES.contains(&name)
            && name != crate::svn::policy::FILE
        {
            return Err(BackupError::new(BackupCategory::InvalidInput));
        }
    }
    let system = root.join(".worldbuild");
    match fs::symlink_metadata(&system) {
        Ok(metadata) => {
            if !metadata.is_dir() || super::project_file::directory::is_reparse(&metadata) {
                return Err(BackupError::new(BackupCategory::Corrupt));
            }
            let guard = super::project_file::directory::ProjectDirectory::open_root(&system)?;
            for entry in guard.read_dir()? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_str().ok_or_else(invalid)?;
                if name == "transactions" {
                    let metadata = fs::symlink_metadata(entry.path())?;
                    if !metadata.is_dir()
                        || super::project_file::directory::is_reparse(&metadata)
                        || fs::read_dir(entry.path())?.next().transpose()?.is_some()
                    {
                        return Err(BackupError::new(BackupCategory::SourceChanged));
                    }
                } else if name != FORMAT_HISTORY_DIRECTORY {
                    return Err(BackupError::new(BackupCategory::InvalidInput));
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn collect_directory(
    root: &Path,
    directory: &Path,
    directories: &mut Vec<String>,
    files: &mut Vec<ManifestFile>,
    total: &mut u64,
) -> Result<(), BackupError> {
    if files.len() >= MAX_FILES || directories.len() >= MAX_FILES {
        return Err(BackupError::new(BackupCategory::TooLarge));
    }
    directories.push(relative_string(root, directory)?);
    let guard = super::project_file::directory::ProjectDirectory::open_root(directory)?;
    let mut entries = guard.read_dir()?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        checkpoint(false)?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if super::project_file::directory::is_reparse(&metadata) {
            return Err(BackupError::new(BackupCategory::Corrupt));
        }
        if metadata.is_dir() {
            collect_directory(root, &path, directories, files, total)?;
            continue;
        }
        if !metadata.is_file() || hard_link_count(&metadata) != 1 {
            return Err(BackupError::new(BackupCategory::Corrupt));
        }
        if files.len() >= MAX_FILES {
            return Err(BackupError::new(BackupCategory::TooLarge));
        }
        *total = total
            .checked_add(metadata.len())
            .ok_or_else(|| BackupError::new(BackupCategory::TooLarge))?;
        if *total > MAX_TOTAL_BYTES {
            return Err(BackupError::new(BackupCategory::TooLarge));
        }
        let (size, sha256) = hash_file(&path)?;
        if size != metadata.len() {
            return Err(BackupError::new(BackupCategory::SourceChanged));
        }
        files.push(ManifestFile {
            path: relative_string(root, &path)?,
            size,
            sha256,
        });
        checkpoint(true)?;
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<(u64, String), BackupError> {
    let before = fs::symlink_metadata(path)?;
    if !before.is_file()
        || super::project_file::directory::is_reparse(&before)
        || hard_link_count(&before) != 1
    {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    let mut file = super::edit_recovery::native::open(path, false, false)?;
    if opened_hard_link_count(&file).map_err(|_| BackupError::new(BackupCategory::Corrupt))? != 1 {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    let result = hash_opened_file(&mut file)?;
    if result.0 != before.len() {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    Ok(result)
}

fn hash_opened_file(file: &mut File) -> Result<(u64, String), BackupError> {
    let before = file.metadata()?;
    if !before.is_file()
        || super::project_file::directory::is_reparse(&before)
        || opened_hard_link_count(file)? != 1
    {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut sha = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    let mut size = 0_u64;
    loop {
        checkpoint(false)?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| BackupError::new(BackupCategory::TooLarge))?;
        sha.update(&buffer[..read]);
    }
    let after = file.metadata()?;
    if before.len() != after.len() || after.len() != size {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    Ok((size, format!("{:x}", sha.finalize())))
}

fn copy_file_verified(
    source: &Path,
    destination: &Path,
    expected: &ManifestFile,
) -> Result<(), BackupError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let source_metadata = fs::symlink_metadata(source)?;
    if !source_metadata.is_file()
        || super::project_file::directory::is_reparse(&source_metadata)
        || hard_link_count(&source_metadata) != 1
    {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    let source_file = super::edit_recovery::native::open(source, false, false)?;
    if opened_hard_link_count(&source_file)? != 1 {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    let mut input = BufReader::with_capacity(BUFFER_BYTES, source_file);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut sha = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    let mut size = 0_u64;
    loop {
        checkpoint(false)?;
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        sha.update(&buffer[..read]);
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| BackupError::new(BackupCategory::TooLarge))?;
    }
    output.flush()?;
    output.sync_all()?;
    let digest = format!("{:x}", sha.finalize());
    if size != expected.size || digest != expected.sha256 {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    checkpoint(true)?;
    Ok(())
}

fn materialize(
    source: &Path,
    destination: &Path,
    directories: &[String],
    files: &[ManifestFile],
) -> Result<(), BackupError> {
    fs::create_dir(destination)?;
    for directory in directories {
        fs::create_dir_all(destination.join(validate_relative(directory)?))?;
    }
    for file in files {
        let relative = validate_relative(&file.path)?;
        copy_file_verified(&source.join(&relative), &destination.join(&relative), file)?;
    }
    Ok(())
}

fn cleanup_owned(path: &Path) -> Result<(), BackupError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn publish_directory(stage: &Path, destination: &Path) -> Result<(), BackupError> {
    if destination.exists() {
        let metadata = fs::symlink_metadata(destination)?;
        if !metadata.is_dir() || super::project_file::directory::is_reparse(&metadata) {
            return Err(BackupError::new(BackupCategory::DestinationOccupied));
        }
        let mut entries = fs::read_dir(destination)?;
        if entries.next().transpose()?.is_some() {
            return Err(BackupError::new(BackupCategory::DestinationOccupied));
        }
        drop(entries);
        fs::remove_dir(destination)?;
    }
    fs::rename(stage, destination)?;
    Ok(())
}

fn confirm_unchanged(
    root: &Path,
    directories: &[String],
    files: &[ManifestFile],
) -> Result<(), BackupError> {
    let (current_directories, current_files) = collect(root)?;
    if current_directories != directories || current_files != files {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    Ok(())
}

fn admit_snapshot_storage(
    anchor: &Path,
    targets: &[PathBuf],
    files: &[ManifestFile],
    owned_apply: bool,
) -> Result<(), BackupError> {
    let sizes = files.iter().map(|file| file.size).collect::<Vec<_>>();
    let backups = vec![None; sizes.len()];
    let names = files
        .iter()
        .map(|file| file.path.len() as u64)
        .collect::<Vec<_>>();
    let input = super::storage_estimate::TransactionStorageInput {
        staged_sizes: &sizes,
        backup_sizes: &backups,
        target_name_bytes: &names,
        minimum_required_bytes: super::storage_estimate::TRANSACTION_OPERATIONAL_RESERVE_BYTES,
    };
    let admitted = if owned_apply {
        super::storage_estimate::admit_owned_transaction_storage(anchor, targets, input)
    } else {
        super::storage_estimate::admit_transaction_storage(anchor, targets, input)
    };
    admitted
        .map(|_| ())
        .map_err(|_| BackupError::new(BackupCategory::TooLarge))
}

pub(crate) fn export_project(
    root: &Path,
    parent: &Path,
    name: &str,
) -> Result<SnapshotResult, BackupError> {
    if !safe_name(name) {
        return Err(invalid());
    }
    let source = canonical_existing_directory(root)?;
    let parent = canonical_existing_directory(parent)?;
    {
        let parent_guard = super::project_file::directory::ProjectDirectory::open_root(&parent)?;
        parent_guard.validate()?;
    }
    let destination = parent.join(name);
    if !disjoint(&source, &destination) {
        return Err(invalid());
    }
    let (directories, files) = collect(&source)?;
    let stage = parent.join(format!(".worldbuild-copy-{}", Uuid::new_v4()));
    admit_snapshot_storage(
        &parent,
        &[stage.clone(), destination.clone()],
        &files,
        false,
    )?;
    let result = (|| {
        materialize(&source, &stage, &directories, &files)?;
        confirm_unchanged(&source, &directories, &files)?;
        preparing()?;
        publish_directory(&stage, &destination)?;
        let verified = if take_test_fault("copy_after_publish") {
            false
        } else {
            collect(&destination)
                .map(|(copied_directories, copied_files)| {
                    copied_directories == directories && copied_files == files
                })
                .unwrap_or(false)
        };
        Ok(SnapshotResult {
            root: destination,
            row: None,
            safety: None,
            outcome: if verified {
                "published_verified"
            } else {
                "published_verification_uncertain"
            },
            warning: (!verified).then_some("verification_failed_after_publish"),
        })
    })();
    if result.is_err() {
        let _ = cleanup_owned(&stage);
    }
    result
}

fn open_or_create_plain_child(
    parent: &Path,
    parent_guard: &super::project_file::directory::ProjectDirectory,
    name: &str,
) -> Result<(PathBuf, super::project_file::directory::ProjectDirectory), BackupError> {
    parent_guard.validate()?;
    let child = parent.join(name);
    match fs::create_dir(&child) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&child)?;
    if !metadata.is_dir() || super::project_file::directory::is_reparse(&metadata) {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    let canonical = fs::canonicalize(&child)?;
    if canonical.parent().map(normalized) != Some(normalized(&parent)) {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    let child_guard =
        super::project_file::directory::ProjectDirectory::open_mutable_root(&canonical)?;
    parent_guard.validate()?;
    child_guard.validate()?;
    Ok((canonical, child_guard))
}

struct BackupParent {
    path: PathBuf,
    _guards: Vec<super::project_file::directory::ProjectDirectory>,
}

fn backup_parent(
    storage: &Path,
    fingerprint: &str,
    create: bool,
) -> Result<Option<BackupParent>, BackupError> {
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let storage = canonical_existing_directory(storage)?;
    let storage_guard =
        super::project_file::directory::ProjectDirectory::open_mutable_root(&storage)?;
    let backup_root = storage.join(BACKUP_DIRECTORY);
    if !create && !backup_root.exists() {
        return Ok(None);
    }
    let (backup_root, backup_root_guard) =
        open_or_create_plain_child(&storage, &storage_guard, BACKUP_DIRECTORY)?;
    let parent = backup_root.join(fingerprint);
    if !create && !parent.exists() {
        return Ok(None);
    }
    let (path, fingerprint_guard) =
        open_or_create_plain_child(&backup_root, &backup_root_guard, fingerprint)?;
    Ok(Some(BackupParent {
        path,
        _guards: vec![storage_guard, backup_root_guard, fingerprint_guard],
    }))
}

fn deleted_parent(parent: &BackupParent, create: bool) -> Result<Option<PathBuf>, BackupError> {
    let path = parent.path.join(DELETED_DIRECTORY);
    if !create && !path.exists() {
        return Ok(None);
    }
    let guard = parent._guards.last().ok_or_else(invalid)?;
    let (path, _) = open_or_create_plain_child(&parent.path, guard, DELETED_DIRECTORY)?;
    Ok(Some(path))
}

fn receipt_path(deleted: &Path, id: &str) -> PathBuf {
    deleted.join(format!("{id}{RECEIPT_SUFFIX}"))
}

fn purging_path(deleted: &Path, id: &str) -> PathBuf {
    deleted.join(format!("{id}{PURGING_SUFFIX}"))
}

fn deleted_package_path(deleted: &Path, id: &str) -> PathBuf {
    deleted.join(format!("{id}.worldbuild-backup"))
}

fn require_absent(path: &Path) -> Result<(), BackupError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(BackupError::new(BackupCategory::DestinationOccupied)),
        Err(error) => Err(error.into()),
    }
}

fn receipt_time(value: &str) -> Result<time::OffsetDateTime, BackupError> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|_| BackupError::new(BackupCategory::Corrupt))
}

#[cfg(test)]
thread_local! {
    static TEST_DELETED_CLOCK: std::cell::Cell<Option<time::OffsetDateTime>> = const { std::cell::Cell::new(None) };
}

fn deleted_clock() -> time::OffsetDateTime {
    #[cfg(test)]
    if let Some(now) = TEST_DELETED_CLOCK.with(std::cell::Cell::get) {
        return now;
    }
    time::OffsetDateTime::now_utc()
}

fn deleted_status(
    receipt: &DeleteReceipt,
    now: time::OffsetDateTime,
) -> Result<&'static str, BackupError> {
    let deleted_at = receipt_time(&receipt.deleted_at_utc)?;
    let expires_at = receipt_time(&receipt.expires_at_utc)?;
    Ok(if deleted_at > now {
        "uncertain"
    } else if expires_at <= now {
        "expired"
    } else {
        "verification_required"
    })
}

fn read_receipt(deleted: &Path, id: &str, fingerprint: &str) -> Result<DeleteReceipt, BackupError> {
    let guard = super::project_file::directory::ProjectDirectory::open_root(deleted)?;
    let name = format!("{id}{RECEIPT_SUFFIX}");
    let file = guard.open_child_candidate(&name)?;
    guard.validate_file(&file, &name)?;
    let metadata = file.metadata()?;
    if metadata.len() > MAX_RECEIPT_BYTES
        || opened_hard_link_count(&file)? != 1
        || super::project_file::directory::is_reparse(&metadata)
    {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECEIPT_BYTES + 1).read_to_end(&mut bytes)?;
    json::parse_strict_json_object(&bytes)
        .map_err(|_| BackupError::new(BackupCategory::Corrupt))?;
    let receipt: DeleteReceipt =
        serde_json::from_slice(&bytes).map_err(|_| BackupError::new(BackupCategory::Corrupt))?;
    let deleted_at = receipt_time(&receipt.deleted_at_utc)?;
    let expires_at = receipt_time(&receipt.expires_at_utc)?;
    if receipt.schema_version != 1
        || receipt.id != id
        || receipt.project_fingerprint != fingerprint
        || Uuid::parse_str(&receipt.operation).is_err()
        || expires_at - deleted_at != time::Duration::days(7)
    {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    guard.validate()?;
    Ok(receipt)
}

fn write_receipt(deleted: &Path, receipt: &DeleteReceipt) -> Result<(), BackupError> {
    let guard = super::project_file::directory::ProjectDirectory::open_mutable_root(deleted)?;
    let path = receipt_path(deleted, &receipt.id);
    let bytes = serde_json::to_vec(receipt).map_err(|_| BackupError::new(BackupCategory::Io))?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err(BackupError::new(BackupCategory::TooLarge));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let written = (|| {
        guard.validate_file(&file, &format!("{}{RECEIPT_SUFFIX}", receipt.id))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        guard.validate()?;
        Ok::<(), BackupError>(())
    })();
    drop(file);
    drop(guard);
    if written.is_err() {
        let _ = remove_receipt(deleted, &receipt.id);
    }
    written
}

fn remove_receipt(deleted: &Path, id: &str) -> Result<(), BackupError> {
    remove_owned_deleted_child(deleted, &format!("{id}{RECEIPT_SUFFIX}"))
}

fn remove_owned_deleted_child(deleted: &Path, name: &str) -> Result<(), BackupError> {
    let guard = super::project_file::directory::ProjectDirectory::open_mutable_root(deleted)?;
    let file = super::edit_recovery::native::open_for_discard(&deleted.join(name))?;
    guard.validate_file(&file, name)?;
    super::edit_recovery::native::cleanup(&file)?;
    guard.validate()?;
    Ok(())
}

fn create_purging_marker(deleted: &Path, id: &str) -> Result<(), BackupError> {
    let guard = super::project_file::directory::ProjectDirectory::open_mutable_root(deleted)?;
    let name = format!("{id}{PURGING_SUFFIX}");
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(deleted.join(&name))?;
    guard.validate_file(&file, &name)?;
    file.sync_all()?;
    guard.validate()?;
    Ok(())
}

fn deleted_row(
    path: &Path,
    manifest: &Manifest,
    receipt: &DeleteReceipt,
    status: &'static str,
) -> DeletedBackupRow {
    DeletedBackupRow {
        backup: row(path, manifest, "verification_required"),
        deleted_at_utc: receipt.deleted_at_utc.clone(),
        expires_at_utc: receipt.expires_at_utc.clone(),
        operation: receipt.operation.clone(),
        status,
    }
}

fn uncertain_deleted_row(id: &str, locator: &Path) -> DeletedBackupRow {
    DeletedBackupRow {
        backup: BackupRow {
            id: id.to_owned(),
            label: String::new(),
            created_at_utc: String::new(),
            kind: BackupKind::Manual,
            size: "0".into(),
            status: "corrupt",
            coverage: "unknown",
            unnamed_ordinal: None,
            locator: locator.to_string_lossy().into_owned(),
        },
        deleted_at_utc: String::new(),
        expires_at_utc: String::new(),
        operation: String::new(),
        status: "uncertain",
    }
}

pub(crate) fn list_deleted_backups(
    fingerprint: &str,
    storage: &Path,
) -> Result<Vec<DeletedBackupRow>, BackupError> {
    let Some(parent) = backup_parent(storage, fingerprint, false)? else {
        return Ok(Vec::new());
    };
    let Some(deleted) = deleted_parent(&parent, false)? else {
        return Ok(Vec::new());
    };
    let guard = super::project_file::directory::ProjectDirectory::open_root(&deleted)?;
    let entries = guard.read_dir()?.collect::<Result<Vec<_>, _>>()?;
    if entries.len() > MAX_FILES {
        return Err(BackupError::new(BackupCategory::TooLarge));
    }
    let names = entries
        .iter()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    guard.validate()?;
    drop(guard);
    let mut rows = Vec::new();
    for entry in entries {
        checkpoint(false)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(RECEIPT_SUFFIX) else {
            continue;
        };
        if Uuid::parse_str(id).is_err() {
            rows.push(uncertain_deleted_row(&name, &deleted.join(&name)));
            continue;
        }
        let path = deleted_package_path(&deleted, id);
        let active = parent.path.join(format!("{id}.worldbuild-backup"));
        let purging = purging_path(&deleted, id);
        if fs::symlink_metadata(&purging).is_ok()
            && read_receipt(&deleted, id, fingerprint).is_ok()
            && require_absent(&path).is_ok()
            && require_absent(&active).is_ok()
            && remove_receipt(&deleted, id).is_ok()
            && remove_owned_deleted_child(&deleted, &format!("{id}{PURGING_SUFFIX}")).is_ok()
        {
            continue;
        }
        if fs::symlink_metadata(&purging).is_ok()
            && read_receipt(&deleted, id, fingerprint).is_ok()
            && read_manifest(&path, Some(fingerprint))
                .and_then(|manifest| verify_payload(&path, &manifest))
                .is_ok()
        {
            let _ = remove_owned_deleted_child(&deleted, &format!("{id}{PURGING_SUFFIX}"));
        }
        if matches!(fs::symlink_metadata(&path), Err(error) if error.kind() == io::ErrorKind::NotFound)
            && require_absent(&purging_path(&deleted, id)).is_ok()
            && read_receipt(&deleted, id, fingerprint).is_ok()
            && read_manifest(&active, Some(fingerprint))
                .and_then(|manifest| verify_payload(&active, &manifest))
                .is_ok()
            && remove_receipt(&deleted, id).is_ok()
        {
            continue;
        }
        let entry = (|| {
            let receipt = read_receipt(&deleted, id, fingerprint)?;
            let manifest = read_manifest(&path, Some(fingerprint))?;
            let status = if require_absent(&purging_path(&deleted, id)).is_err() {
                "uncertain"
            } else {
                deleted_status(&receipt, deleted_clock())?
            };
            Ok::<_, BackupError>(deleted_row(&path, &manifest, &receipt, status))
        })();
        rows.push(entry.unwrap_or_else(|_| uncertain_deleted_row(id, &path)));
    }
    for name in &names {
        if name.ends_with(RECEIPT_SUFFIX) {
            continue;
        }
        if let Some(id) = name.strip_suffix(".worldbuild-backup") {
            if Uuid::parse_str(id).is_ok() && names.contains(&format!("{id}{RECEIPT_SUFFIX}")) {
                continue;
            }
        }
        if let Some(id) = name.strip_suffix(PURGING_SUFFIX) {
            if Uuid::parse_str(id).is_ok() && names.contains(&format!("{id}{RECEIPT_SUFFIX}")) {
                continue;
            }
            if Uuid::parse_str(id).is_ok()
                && require_absent(&receipt_path(&deleted, id)).is_ok()
                && require_absent(&deleted_package_path(&deleted, id)).is_ok()
                && require_absent(&parent.path.join(format!("{id}.worldbuild-backup"))).is_ok()
                && remove_owned_deleted_child(&deleted, &name).is_ok()
            {
                continue;
            }
        }
        rows.push(uncertain_deleted_row(name, &deleted.join(name)));
    }
    rows.sort_by(|a, b| b.deleted_at_utc.cmp(&a.deleted_at_utc));
    Ok(rows)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuarantineOutcome {
    pub(crate) outcome: &'static str,
    pub(crate) warning: Option<&'static str>,
    pub(crate) deleted: Option<DeletedBackupRow>,
}

pub(crate) fn quarantine_backup(
    storage: &Path,
    locator: &Path,
    fingerprint: &str,
) -> Result<QuarantineOutcome, BackupError> {
    let parent = backup_parent(storage, fingerprint, false)?.ok_or_else(invalid)?;
    let source = canonical_existing_directory(locator)?;
    if source.parent().map(normalized) != Some(normalized(&parent.path)) {
        return Err(invalid());
    }
    let manifest = read_manifest(&source, Some(fingerprint))?;
    if manifest.kind == BackupKind::PreRestore {
        return Err(BackupError::new(BackupCategory::RecoveryRequired));
    }
    verify_payload(&source, &manifest)?;
    let deleted = deleted_parent(&parent, true)?.ok_or_else(invalid)?;
    if cleanup_one_expired_backup(storage, fingerprint)?
        .is_some_and(|result| result.outcome != "deleted")
    {
        return Err(BackupError::new(BackupCategory::RecoveryRequired));
    }
    let existing = list_deleted_backups(fingerprint, storage)?;
    if existing.iter().any(|row| row.status == "uncertain") {
        return Err(BackupError::new(BackupCategory::RecoveryRequired));
    }
    let existing_count = existing.len();
    let mut used_bytes = 0_u64;
    for entry in existing {
        let checked = read_manifest(Path::new(&entry.backup.locator), Some(fingerprint))?;
        verify_payload(Path::new(&entry.backup.locator), &checked)?;
        used_bytes = used_bytes
            .checked_add(checked.total_bytes)
            .ok_or_else(|| BackupError::new(BackupCategory::TooLarge))?;
    }
    if !deleted_capacity_allows(existing_count, used_bytes, manifest.total_bytes) {
        return Err(BackupError::new(BackupCategory::RetentionFull));
    }
    let target = deleted_package_path(&deleted, &manifest.id);
    require_absent(&target)?;
    require_absent(&receipt_path(&deleted, &manifest.id))?;
    let source_guard = super::project_file::directory::ProjectDirectory::open_root(&source)?;
    let source_identity = source_guard.identity();
    source_guard.validate()?;
    drop(source_guard);
    let deleted_at = deleted_clock();
    let expires_at = deleted_at + time::Duration::days(7);
    let format = time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
    );
    let receipt = DeleteReceipt {
        schema_version: 1,
        id: manifest.id.clone(),
        project_fingerprint: fingerprint.into(),
        operation: Uuid::new_v4().to_string(),
        deleted_at_utc: deleted_at
            .format(format)
            .map_err(|_| BackupError::new(BackupCategory::Io))?,
        expires_at_utc: expires_at
            .format(format)
            .map_err(|_| BackupError::new(BackupCategory::Io))?,
    };
    write_receipt(&deleted, &receipt)?;
    process_crash_checkpoint("quarantine_receipt_before_move");
    let source_guard = super::project_file::directory::ProjectDirectory::open_owned_root(&source)?;
    if source_guard.identity() != source_identity {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    let destination =
        super::project_file::directory::ProjectDirectory::open_move_destination(&deleted)?;
    let moved =
        source_guard.rename_owned_into(&destination, &format!("{}.worldbuild-backup", manifest.id));
    drop(destination);
    if let Err(error) = moved {
        if fs::symlink_metadata(&source).is_ok()
            && matches!(fs::symlink_metadata(&target), Err(error) if error.kind() == io::ErrorKind::NotFound)
        {
            let _ = remove_receipt(&deleted, &manifest.id);
            return Err(error.into());
        }
    }
    process_crash_checkpoint("quarantine_move_before_readback");
    let verified = (|| {
        let checked = read_receipt(&deleted, &manifest.id, fingerprint)?;
        let published = read_manifest(&target, Some(fingerprint))?;
        verify_payload(&target, &published)?;
        if checked != receipt || published != manifest || require_absent(&source).is_err() {
            return Err(BackupError::new(BackupCategory::SourceChanged));
        }
        Ok(deleted_row(
            &target,
            &published,
            &receipt,
            "verification_required",
        ))
    })();
    match verified {
        Ok(row) => Ok(QuarantineOutcome {
            outcome: "quarantined",
            warning: None,
            deleted: Some(row),
        }),
        Err(_) => Ok(QuarantineOutcome {
            outcome: "quarantined_unverified",
            warning: Some("backup_quarantine_readback_required"),
            deleted: None,
        }),
    }
}

pub(crate) fn restore_deleted_backup(
    storage: &Path,
    fingerprint: &str,
    id: &str,
    operation: &str,
) -> Result<QuarantineOutcome, BackupError> {
    if Uuid::parse_str(id).is_err() || Uuid::parse_str(operation).is_err() {
        return Err(invalid());
    }
    let parent = backup_parent(storage, fingerprint, false)?.ok_or_else(invalid)?;
    let deleted = deleted_parent(&parent, false)?.ok_or_else(invalid)?;
    let receipt = read_receipt(&deleted, id, fingerprint)?;
    if receipt.operation != operation {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    require_absent(&purging_path(&deleted, id))?;
    let now = deleted_clock();
    if receipt_time(&receipt.deleted_at_utc)? > now || receipt_time(&receipt.expires_at_utc)? <= now
    {
        return Err(BackupError::new(BackupCategory::RecoveryRequired));
    }
    let source = deleted_package_path(&deleted, id);
    let target = parent.path.join(format!("{id}.worldbuild-backup"));
    require_absent(&target)?;
    let manifest = read_manifest(&source, Some(fingerprint))?;
    verify_payload(&source, &manifest)?;
    let source_guard = super::project_file::directory::ProjectDirectory::open_root(&source)?;
    let identity = source_guard.identity();
    source_guard.validate()?;
    drop(source_guard);
    let source_guard = super::project_file::directory::ProjectDirectory::open_owned_root(&source)?;
    if source_guard.identity() != identity {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    let destination =
        super::project_file::directory::ProjectDirectory::open_move_destination(&parent.path)?;
    let moved = source_guard.rename_owned_into(&destination, &format!("{id}.worldbuild-backup"));
    drop(destination);
    if let Err(error) = moved {
        if fs::symlink_metadata(&source).is_ok()
            && matches!(fs::symlink_metadata(&target), Err(error) if error.kind() == io::ErrorKind::NotFound)
        {
            return Err(error.into());
        }
        return Ok(QuarantineOutcome {
            outcome: "restore_unverified",
            warning: Some("backup_restore_readback_required"),
            deleted: None,
        });
    }
    process_crash_checkpoint("quarantine_restore_before_receipt_cleanup");
    if require_absent(&source).is_err()
        || read_manifest(&target, Some(fingerprint))
            .and_then(|checked| {
                if checked != manifest {
                    return Err(BackupError::new(BackupCategory::SourceChanged));
                }
                verify_payload(&target, &checked)
            })
            .is_err()
    {
        return Ok(QuarantineOutcome {
            outcome: "restore_unverified",
            warning: Some("backup_restore_readback_required"),
            deleted: None,
        });
    }
    if read_receipt(&deleted, id, fingerprint).ok().as_ref() != Some(&receipt) {
        return Ok(QuarantineOutcome {
            outcome: "restored_receipt_cleanup_required",
            warning: Some("backup_receipt_cleanup_required"),
            deleted: None,
        });
    }
    match remove_receipt(&deleted, id) {
        Ok(()) => Ok(QuarantineOutcome {
            outcome: "restored",
            warning: None,
            deleted: None,
        }),
        Err(_) => Ok(QuarantineOutcome {
            outcome: "restored_receipt_cleanup_required",
            warning: Some("backup_receipt_cleanup_required"),
            deleted: None,
        }),
    }
}

pub(crate) fn create_backup(
    root: &Path,
    fingerprint: &str,
    storage: &Path,
    label: Option<&str>,
    kind: BackupKind,
) -> Result<BackupRow, BackupError> {
    let label = label.unwrap_or("");
    if !safe_label(label) {
        return Err(invalid());
    }
    let source = canonical_existing_directory(root)?;
    let storage = canonical_existing_directory(storage)?;
    if !disjoint(&source, &storage) {
        return Err(invalid());
    }
    let parent = backup_parent(&storage, fingerprint, true)?.ok_or_else(invalid)?;
    let id = Uuid::new_v4().to_string();
    let stage = parent.path.join(format!(".staging-{id}"));
    let destination = parent.path.join(format!("{id}.worldbuild-backup"));
    let (directories, files) = collect(&source)?;
    admit_snapshot_storage(
        &parent.path,
        &[stage.clone(), destination.clone()],
        &files,
        false,
    )?;
    let created_at_utc = now()?;
    let total_bytes = files
        .iter()
        .try_fold(0_u64, |sum, file| sum.checked_add(file.size))
        .ok_or_else(|| BackupError::new(BackupCategory::TooLarge))?;
    let manifest = Manifest {
        schema_version: CURRENT_MANIFEST_SCHEMA,
        id: id.clone(),
        project_fingerprint: fingerprint.into(),
        created_at_utc: created_at_utc.clone(),
        kind,
        label: label.into(),
        directories: directories.clone(),
        files: files.clone(),
        total_bytes,
        complete: true,
        format_history_complete: true,
    };
    let result = (|| {
        fs::create_dir(&stage)?;
        materialize(
            &source,
            &stage.join(PAYLOAD_DIRECTORY),
            &directories,
            &files,
        )?;
        confirm_unchanged(&source, &directories, &files)?;
        atomic_file::save_deterministic_json(&stage.join(MANIFEST_NAME), &manifest)
            .map_err(|_| BackupError::new(BackupCategory::Io))?;
        let checked = read_manifest(&stage, Some(fingerprint))?;
        verify_payload(&stage, &checked)?;
        checkpoint(false)?;
        fs::rename(&stage, &destination)?;
        Ok(row(&destination, &manifest, "verified"))
    })();
    if result.is_err() {
        let _ = cleanup_owned(&stage);
    }
    result
}

fn read_manifest(directory: &Path, fingerprint: Option<&str>) -> Result<Manifest, BackupError> {
    let directory = canonical_existing_directory(directory)?;
    validate_package_shape(&directory)?;
    let path = directory.join(MANIFEST_NAME);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file()
        || super::project_file::directory::is_reparse(&metadata)
        || !manifest_size_allowed(metadata.len())
        || hard_link_count(&metadata) != 1
    {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    let mut file = super::edit_recovery::native::open(&path, false, false)
        .map_err(|_| BackupError::new(BackupCategory::Corrupt))?;
    read_manifest_file(&directory, &mut file, fingerprint)
}

fn read_manifest_file(
    directory: &Path,
    file: &mut File,
    fingerprint: Option<&str>,
) -> Result<Manifest, BackupError> {
    let metadata = file
        .metadata()
        .map_err(|_| BackupError::new(BackupCategory::Corrupt))?;
    if !metadata.is_file()
        || super::project_file::directory::is_reparse(&metadata)
        || !manifest_size_allowed(metadata.len())
    {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    if opened_hard_link_count(&file).map_err(|_| BackupError::new(BackupCategory::Corrupt))? != 1 {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.seek(SeekFrom::Start(0))?;
    file.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    json::parse_strict_json_object(&bytes)
        .map_err(|_| BackupError::new(BackupCategory::Corrupt))?;
    let mut manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| BackupError::new(BackupCategory::Corrupt))?;
    if !matches!(manifest.schema_version, 1 | CURRENT_MANIFEST_SCHEMA) {
        return Err(BackupError::new(BackupCategory::Unsupported));
    }
    if manifest.schema_version == CURRENT_MANIFEST_SCHEMA && !manifest.format_history_complete {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    if manifest.schema_version == 1 {
        manifest.format_history_complete = false;
    }
    let directory_name = directory.file_name().and_then(|name| name.to_str());
    validate_manifest(&manifest, directory_name, fingerprint)?;
    Ok(manifest)
}

fn validate_manifest(
    manifest: &Manifest,
    directory_name: Option<&str>,
    fingerprint: Option<&str>,
) -> Result<(), BackupError> {
    let final_name = format!("{}.worldbuild-backup", manifest.id);
    let staging_name = format!(".staging-{}", manifest.id);
    if !manifest.complete
        || Uuid::parse_str(&manifest.id).is_err()
        || !safe_label(&manifest.label)
        || fingerprint.is_some_and(|value| value != manifest.project_fingerprint)
        || !matches!(directory_name, Some(name) if name == final_name || name == staging_name)
    {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    if manifest.directories.len() > MAX_FILES || manifest.files.len() > MAX_FILES {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    if !manifest
        .directories
        .windows(2)
        .all(|pair| pair[0] < pair[1])
        || !manifest
            .files
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path)
    {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    let mut directories = BTreeSet::new();
    for directory in &manifest.directories {
        validate_relative(directory).map_err(|_| BackupError::new(BackupCategory::Corrupt))?;
        if !directories.insert(directory.as_str()) {
            return Err(BackupError::new(BackupCategory::Corrupt));
        }
        let path = Path::new(directory);
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            let parent = parent.to_string_lossy();
            if parent != ".worldbuild" || directory != ".worldbuild/format-history" {
                if !directories.contains(parent.as_ref()) {
                    return Err(BackupError::new(BackupCategory::Corrupt));
                }
            }
        }
    }
    let mut paths = BTreeSet::new();
    let mut total = 0_u64;
    for file in &manifest.files {
        validate_relative(&file.path).map_err(|_| BackupError::new(BackupCategory::Corrupt))?;
        let parent = Path::new(&file.path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.to_string_lossy())
            .ok_or_else(|| BackupError::new(BackupCategory::Corrupt))?;
        if !paths.insert(file.path.clone())
            || directories.contains(file.path.as_str())
            || !directories.contains(parent.as_ref())
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(BackupError::new(BackupCategory::Corrupt));
        }
        total = total
            .checked_add(file.size)
            .ok_or_else(|| BackupError::new(BackupCategory::TooLarge))?;
    }
    if total != manifest.total_bytes || total > MAX_TOTAL_BYTES {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    Ok(())
}

fn validate_package_shape(directory: &Path) -> Result<(), BackupError> {
    let guard = super::project_file::directory::ProjectDirectory::open_root(directory)?;
    let mut names = BTreeSet::new();
    for entry in guard.read_dir()? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(invalid)?;
        if !matches!(name, MANIFEST_NAME | PAYLOAD_DIRECTORY) || !names.insert(name.to_owned()) {
            return Err(BackupError::new(BackupCategory::Corrupt));
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if super::project_file::directory::is_reparse(&metadata)
            || (name == MANIFEST_NAME && !metadata.is_file())
            || (name == PAYLOAD_DIRECTORY && !metadata.is_dir())
        {
            return Err(BackupError::new(BackupCategory::Corrupt));
        }
    }
    if names != BTreeSet::from([MANIFEST_NAME.to_owned(), PAYLOAD_DIRECTORY.to_owned()]) {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    Ok(())
}

fn verify_payload(directory: &Path, manifest: &Manifest) -> Result<(), BackupError> {
    let payload = directory.join(PAYLOAD_DIRECTORY);
    let (directories, files) = collect(&payload).map_err(|error| {
        if error.category == BackupCategory::Cancelled {
            error
        } else {
            BackupError::new(BackupCategory::Corrupt)
        }
    })?;
    if directories != manifest.directories || files != manifest.files {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    Ok(())
}

fn row(directory: &Path, manifest: &Manifest, status: &'static str) -> BackupRow {
    BackupRow {
        id: manifest.id.clone(),
        label: manifest.label.clone(),
        created_at_utc: manifest.created_at_utc.clone(),
        kind: manifest.kind,
        size: manifest.total_bytes.to_string(),
        status,
        coverage: if manifest.format_history_complete {
            "complete"
        } else {
            "legacy_unknown"
        },
        unnamed_ordinal: None,
        locator: directory.to_string_lossy().into_owned(),
    }
}

pub(crate) fn list_backups(
    fingerprint: &str,
    storage: &Path,
    cursor: Option<&str>,
) -> Result<BackupPage, BackupError> {
    #[cfg(test)]
    let measure_started = std::time::Instant::now();
    #[cfg(test)]
    let mut manifest_ns = 0_u128;
    checkpoint(false)?;
    let Some(parent) = backup_parent(storage, fingerprint, false)? else {
        return Ok(BackupPage {
            backups: Vec::new(),
            next_cursor: None,
        });
    };
    let mut rows = Vec::new();
    let mut entries = fs::read_dir(&parent.path)?.collect::<Result<Vec<_>, _>>()?;
    if entries.len() > MAX_FILES {
        return Err(BackupError::new(BackupCategory::TooLarge));
    }
    entries.sort_by_key(|entry| entry.file_name());
    #[cfg(test)]
    let enumeration_ns = measure_started.elapsed().as_nanos();
    for entry in entries {
        checkpoint(false)?;
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".worldbuild-backup") {
            continue;
        }
        let path = entry.path();
        #[cfg(test)]
        let manifest_started = std::time::Instant::now();
        let result = read_manifest(&path, Some(fingerprint));
        #[cfg(test)]
        {
            manifest_ns += manifest_started.elapsed().as_nanos();
        }
        match result {
            Ok(manifest) => rows.push(row(&path, &manifest, "verification_required")),
            Err(error) if error.category == BackupCategory::Unsupported => rows.push(BackupRow {
                id: name.to_string_lossy().into_owned(),
                label: String::new(),
                created_at_utc: String::new(),
                kind: BackupKind::Manual,
                size: "0".into(),
                status: "unsupported",
                coverage: "unknown",
                unnamed_ordinal: None,
                locator: path.to_string_lossy().into_owned(),
            }),
            Err(_) => rows.push(BackupRow {
                id: name.to_string_lossy().into_owned(),
                label: String::new(),
                created_at_utc: String::new(),
                kind: BackupKind::Manual,
                size: "0".into(),
                status: "corrupt",
                coverage: "unknown",
                unnamed_ordinal: None,
                locator: path.to_string_lossy().into_owned(),
            }),
        }
        checkpoint(true)?;
    }
    rows.sort_by(|left, right| {
        right
            .created_at_utc
            .cmp(&left.created_at_utc)
            .then_with(|| left.id.cmp(&right.id))
    });
    let unnamed_total = rows
        .iter()
        .filter(|row| {
            row.label.is_empty() && matches!(row.status, "verification_required" | "verified")
        })
        .count();
    let mut unnamed_seen = 0;
    for row in &mut rows {
        if row.label.is_empty() && matches!(row.status, "verification_required" | "verified") {
            row.unnamed_ordinal = Some(unnamed_total - unnamed_seen);
            unnamed_seen += 1;
        }
    }
    let start = match cursor {
        Some(cursor) => rows
            .iter()
            .position(|row| row.id == cursor)
            .map(|index| index + 1)
            .ok_or_else(invalid)?,
        None => 0,
    };
    const PAGE_SIZE: usize = 100;
    let end = start.saturating_add(PAGE_SIZE).min(rows.len());
    let backups = rows[start..end].to_vec();
    let next_cursor = (end < rows.len())
        .then(|| backups.last().map(|row| row.id.clone()))
        .flatten();
    #[cfg(test)]
    if std::env::var_os("M56_BACKUP_TIMING").is_some() {
        println!(
            "M56_BACKUP_PHASE {}",
            serde_json::json!({
                "enumeration_ms": enumeration_ns as f64 / 1_000_000.0,
                "manifest_read_decode_ms": manifest_ns as f64 / 1_000_000.0,
                "sort_page_ms": (measure_started.elapsed().as_nanos() - enumeration_ns - manifest_ns) as f64 / 1_000_000.0,
                "total_ms": measure_started.elapsed().as_secs_f64() * 1000.0,
                "rows": rows.len(),
                "page": backups.len()
            })
        );
    }
    Ok(BackupPage {
        backups,
        next_cursor,
    })
}

pub(crate) fn inspect_backup(locator: &Path) -> Result<BackupRow, BackupError> {
    let manifest = read_manifest(locator, None)?;
    verify_payload(locator, &manifest)?;
    Ok(row(locator, &manifest, "verified"))
}

pub(crate) fn restore_new(
    locator: &Path,
    parent: &Path,
    name: &str,
) -> Result<SnapshotResult, BackupError> {
    if !safe_name(name) {
        return Err(invalid());
    }
    let backup = canonical_existing_directory(locator)?;
    let manifest = read_manifest(&backup, None)?;
    verify_payload(&backup, &manifest)?;
    let parent = canonical_existing_directory(parent)?;
    {
        let parent_guard = super::project_file::directory::ProjectDirectory::open_root(&parent)?;
        parent_guard.validate()?;
    }
    let destination = parent.join(name);
    if !disjoint(&backup, &destination) {
        return Err(invalid());
    }
    let stage = parent.join(format!(".worldbuild-restore-{}", Uuid::new_v4()));
    admit_snapshot_storage(
        &parent,
        &[stage.clone(), destination.clone()],
        &manifest.files,
        false,
    )?;
    let result = (|| {
        materialize(
            &backup.join(PAYLOAD_DIRECTORY),
            &stage,
            &manifest.directories,
            &manifest.files,
        )?;
        preparing()?;
        publish_directory(&stage, &destination)?;
        let verified = if take_test_fault("restore_new_after_publish") {
            false
        } else {
            collect(&destination)
                .map(|(directories, files)| {
                    directories == manifest.directories && files == manifest.files
                })
                .unwrap_or(false)
        };
        Ok(SnapshotResult {
            root: destination,
            row: Some(row(&backup, &manifest, "verified")),
            safety: None,
            outcome: if verified {
                "published_verified"
            } else {
                "published_verification_uncertain"
            },
            warning: (!verified).then_some("verification_failed_after_publish"),
        })
    })();
    if result.is_err() {
        let _ = cleanup_owned(&stage);
    }
    result
}

fn exact_managed_files(root: &Path) -> Result<BTreeMap<String, ManifestFile>, BackupError> {
    let (_, files) = collect(root)?;
    Ok(files
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect())
}

fn checkpoint_after_first_apply_mutation(observed: &mut bool) {
    if !*observed {
        *observed = true;
        process_crash_checkpoint("during_apply");
    }
}

fn apply_staged(root: &Path, staged: &Path, journal: &RestoreJournal) -> Result<(), BackupError> {
    // 적용 전에 현재 관리 집합 전체를 strict 열거해 reparse/alias를 거절한다.
    let _ = collect(root)?;
    let expected: BTreeMap<_, _> = journal
        .target_files
        .iter()
        .map(|file| (file.path.clone(), file))
        .collect();
    let mut mutation_observed = false;
    for directory in &journal.target_directories {
        let target = root.join(validate_relative(directory)?);
        let existed = target.exists();
        fs::create_dir_all(&target)?;
        if !existed {
            checkpoint_after_first_apply_mutation(&mut mutation_observed);
        }
    }
    for file in &journal.target_files {
        let relative = validate_relative(&file.path)?;
        let target = root.join(&relative);
        if target.exists() {
            let (size, digest) = hash_file(&target)?;
            if size == file.size && digest == file.sha256 {
                continue;
            }
            fs::remove_file(&target)?;
        }
        copy_file_verified(&staged.join(&relative), &target, file)?;
        checkpoint_after_first_apply_mutation(&mut mutation_observed);
    }
    let current = exact_managed_files(root)?;
    for (path, _) in current {
        if !expected.contains_key(&path) {
            fs::remove_file(root.join(validate_relative(&path)?))?;
            checkpoint_after_first_apply_mutation(&mut mutation_observed);
        }
    }
    for namespace in MANAGED_NAMESPACES {
        let namespace_root = root.join(namespace);
        remove_empty_managed_directories(&namespace_root, &namespace_root)?;
        if !journal
            .target_directories
            .iter()
            .any(|directory| directory == namespace)
        {
            if let Ok(mut entries) = fs::read_dir(&namespace_root) {
                if entries.next().transpose()?.is_none() {
                    fs::remove_dir(&namespace_root)?;
                    checkpoint_after_first_apply_mutation(&mut mutation_observed);
                }
            }
        }
    }
    let history = root.join(".worldbuild").join(FORMAT_HISTORY_DIRECTORY);
    remove_empty_managed_directories(&history, &history)?;
    if let Ok(mut entries) = fs::read_dir(&history) {
        if entries.next().transpose()?.is_none() {
            fs::remove_dir(&history)?;
            checkpoint_after_first_apply_mutation(&mut mutation_observed);
        }
    }
    let _ = fs::remove_dir(root.join(".worldbuild"));
    // 예상 밖 빈 디렉터리를 정리한 뒤 manifest가 보존 대상으로 선언한 빈
    // 디렉터리를 다시 만들어 최종 exact-set을 snapshot과 일치시킨다.
    for directory in &journal.target_directories {
        fs::create_dir_all(root.join(validate_relative(directory)?))?;
    }
    let (directories, files) = collect(root)?;
    if directories != journal.target_directories || files != journal.target_files {
        return Err(BackupError::new(BackupCategory::RecoveryRequired));
    }
    Ok(())
}

fn remove_empty_managed_directories(root: &Path, current: &Path) -> Result<(), BackupError> {
    let metadata = match fs::symlink_metadata(current) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || super::project_file::directory::is_reparse(&metadata) {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    let children = fs::read_dir(current)?.collect::<Result<Vec<_>, _>>()?;
    for child in children {
        if child.file_type()?.is_dir() {
            remove_empty_managed_directories(root, &child.path())?;
        }
    }
    if current != root && fs::read_dir(current)?.next().transpose()?.is_none() {
        fs::remove_dir(current)?;
    }
    Ok(())
}

pub(crate) fn restore_current(
    root: &Path,
    fingerprint: &str,
    locator: &Path,
    safety_storage: &Path,
) -> Result<SnapshotResult, BackupError> {
    let root = canonical_existing_directory(root)?;
    let backup = canonical_existing_directory(locator)?;
    let manifest = read_manifest(&backup, Some(fingerprint))?;
    if !manifest.format_history_complete {
        return Err(BackupError::new(BackupCategory::Unsupported));
    }
    verify_payload(&backup, &manifest)?;
    let restore_parent = root.join(RESTORE_DIRECTORY);
    let operation_id = Uuid::new_v4().to_string();
    let operation = restore_parent.join(&operation_id);
    let staged = operation.join("new");
    admit_snapshot_storage(
        &root,
        &[operation.clone(), staged.clone()],
        &manifest.files,
        true,
    )?;
    let safety = create_backup(
        &root,
        fingerprint,
        safety_storage,
        Some("복원 전 안전 백업"),
        BackupKind::PreRestore,
    )?;
    let selected = row(&backup, &manifest, "verified");
    let journal = RestoreJournal {
        schema_version: 1,
        operation_id,
        target_files: manifest.files.clone(),
        target_directories: manifest.directories.clone(),
    };
    let prepared = (|| {
        fs::create_dir_all(&restore_parent)?;
        fs::create_dir(&operation)?;
        if take_test_fault("restore_prepare_after_safety") {
            return Err(BackupError::new(BackupCategory::Io));
        }
        materialize(
            &backup.join(PAYLOAD_DIRECTORY),
            &staged,
            &manifest.directories,
            &manifest.files,
        )?;
        atomic_file::save_deterministic_json(&operation.join(RESTORE_MANIFEST), &journal)
            .map_err(|_| BackupError::new(BackupCategory::Io))?;
        preparing()?;
        File::create(operation.join(COMMIT_MARKER))?.sync_all()?;
        process_crash_checkpoint("after_commit_marker");
        Ok::<(), BackupError>(())
    })();
    if let Err(error) = prepared {
        let _ = cleanup_owned(&operation);
        let _ = fs::remove_dir(&restore_parent);
        return Ok(SnapshotResult {
            root,
            row: Some(selected),
            safety: Some(safety),
            outcome: "not_applied_safety_created",
            warning: Some(match error.category() {
                BackupCategory::Cancelled => "cancelled_before_commit",
                _ => "prepare_failed_before_commit",
            }),
        });
    }
    let applied = apply_staged(&root, &staged, &journal)
        .and_then(|()| {
            if take_test_fault("restore_cleanup_after_apply")
                || take_root_test_fault(&root, "restore_cleanup_after_apply")
            {
                Err(BackupError::new(BackupCategory::Io))
            } else {
                cleanup_owned(&operation)
            }
        })
        .and_then(|()| {
            let _ = fs::remove_dir(&restore_parent);
            Ok(())
        });
    if applied.is_ok() {
        return Ok(SnapshotResult {
            root,
            row: Some(selected),
            safety: Some(safety),
            outcome: "applied_verified",
            warning: None,
        });
    }
    match recover_pending(&root) {
        Ok(()) => Ok(SnapshotResult {
            root,
            row: Some(selected),
            safety: Some(safety),
            outcome: "applied_recovered",
            warning: Some("post_commit_recovery_completed"),
        }),
        Err(recovery) => Err(BackupError {
            category: BackupCategory::RecoveryRequired,
            io: recovery
                .io
                .or_else(|| applied.err().and_then(|error| error.io)),
            safety: Some(safety),
        }),
    }
}

pub(crate) fn recover_pending(root: &Path) -> Result<(), BackupError> {
    if take_root_test_fault(root, "recover_pending_once") {
        return Err(BackupError::new(BackupCategory::RecoveryRequired));
    }
    let restore_parent = root.join(RESTORE_DIRECTORY);
    let metadata = match fs::symlink_metadata(&restore_parent) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || super::project_file::directory::is_reparse(&metadata) {
        return Err(BackupError::new(BackupCategory::RecoveryRequired));
    }
    let entries = fs::read_dir(&restore_parent)?.collect::<Result<Vec<_>, _>>()?;
    for entry in entries {
        let operation = entry.path();
        let operation_name = operation
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| BackupError::new(BackupCategory::RecoveryRequired))?;
        if Uuid::parse_str(operation_name).is_err() {
            return Err(BackupError::new(BackupCategory::RecoveryRequired));
        }
        let metadata = fs::symlink_metadata(&operation)?;
        if !metadata.is_dir() || super::project_file::directory::is_reparse(&metadata) {
            return Err(BackupError::new(BackupCategory::RecoveryRequired));
        }
        let journal_path = operation.join(RESTORE_MANIFEST);
        if !journal_path.exists() && !operation.join(COMMIT_MARKER).exists() {
            cleanup_owned(&operation)?;
            continue;
        }
        let metadata = fs::symlink_metadata(&journal_path)?;
        if !metadata.is_file()
            || super::project_file::directory::is_reparse(&metadata)
            || metadata.len() > MAX_MANIFEST_BYTES
        {
            return Err(BackupError::new(BackupCategory::RecoveryRequired));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        super::edit_recovery::native::open(&journal_path, false, false)?
            .take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)?;
        json::parse_strict_json_object(&bytes)
            .map_err(|_| BackupError::new(BackupCategory::RecoveryRequired))?;
        let journal: RestoreJournal = serde_json::from_slice(&bytes)
            .map_err(|_| BackupError::new(BackupCategory::RecoveryRequired))?;
        if journal.schema_version != 1
            || operation.file_name().and_then(|value| value.to_str()) != Some(&journal.operation_id)
        {
            return Err(BackupError::new(BackupCategory::RecoveryRequired));
        }
        if operation.join(COMMIT_MARKER).exists() {
            apply_staged(root, &operation.join("new"), &journal)?;
        }
        cleanup_owned(&operation)?;
    }
    let _ = fs::remove_dir(&restore_parent);
    Ok(())
}

struct DeleteEvidence {
    directories: BTreeMap<String, super::project_file::directory::DirectoryIdentity>,
    files: BTreeMap<String, super::project_file::directory::DirectoryIdentity>,
}

fn relative_prefixes(path: &Path) -> Result<Vec<String>, BackupError> {
    let mut result = Vec::new();
    let mut current = PathBuf::new();
    for component in path.components() {
        let Component::Normal(component) = component else {
            return Err(BackupError::new(BackupCategory::Corrupt));
        };
        let component = component
            .to_str()
            .ok_or_else(|| BackupError::new(BackupCategory::Corrupt))?;
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(BackupError::new(BackupCategory::Corrupt));
        }
        current.push(component);
        result.push(current.to_string_lossy().replace('\\', "/"));
    }
    Ok(result)
}

fn deletion_directory_paths(manifest: &Manifest) -> Result<Vec<String>, BackupError> {
    let mut paths = BTreeSet::new();
    for directory in &manifest.directories {
        for prefix in relative_prefixes(&validate_relative(directory)?)? {
            paths.insert(prefix);
        }
    }
    for file in &manifest.files {
        let relative = validate_relative(&file.path)?;
        let parent = relative
            .parent()
            .ok_or_else(|| BackupError::new(BackupCategory::Corrupt))?;
        for prefix in relative_prefixes(parent)? {
            paths.insert(prefix);
        }
    }
    if paths.len() > MAX_FILES + 1 {
        return Err(BackupError::new(BackupCategory::TooLarge));
    }
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    paths.sort_by_key(|path| (path.matches('/').count(), path.clone()));
    Ok(paths)
}

fn open_directory_chain(
    payload: &Path,
    payload_guard: &super::project_file::directory::ProjectDirectory,
    relative: &Path,
    identities: &BTreeMap<String, super::project_file::directory::DirectoryIdentity>,
    owned: bool,
) -> Result<Vec<super::project_file::directory::ProjectDirectory>, BackupError> {
    let mut guards: Vec<super::project_file::directory::ProjectDirectory> = Vec::new();
    let mut current = PathBuf::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(BackupError::new(BackupCategory::Corrupt));
        };
        current.push(component);
        let key = current.to_string_lossy().replace('\\', "/");
        let expected = identities
            .get(&key)
            .ok_or_else(|| BackupError::new(BackupCategory::Corrupt))?;
        let path = payload.join(&current);
        let guard = if owned {
            super::project_file::directory::ProjectDirectory::open_owned_root(&path)?
        } else {
            super::project_file::directory::ProjectDirectory::open_root(&path)?
        };
        if guard.identity() != *expected {
            return Err(BackupError::new(BackupCategory::SourceChanged));
        }
        if let Some(parent) = guards.last() {
            parent.validate()?;
        } else {
            payload_guard.validate()?;
        }
        guards.push(guard);
    }
    Ok(guards)
}

fn capture_delete_evidence(
    directory: &Path,
    payload: &Path,
    package_guard: &super::project_file::directory::ProjectDirectory,
    payload_guard: &super::project_file::directory::ProjectDirectory,
    manifest: &Manifest,
) -> Result<DeleteEvidence, BackupError> {
    verify_payload(directory, manifest)?;
    let mut evidence = DeleteEvidence {
        directories: BTreeMap::new(),
        files: BTreeMap::new(),
    };
    for relative in deletion_directory_paths(manifest)? {
        let relative_path = Path::new(&relative);
        let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
        let parents =
            open_directory_chain(payload, payload_guard, parent, &evidence.directories, false)?;
        let guard = super::project_file::directory::ProjectDirectory::open_root(
            &payload.join(relative_path),
        )?;
        if let Some(parent) = parents.last() {
            parent.validate()?;
        } else {
            payload_guard.validate()?;
        }
        evidence.directories.insert(relative, guard.identity());
    }
    for expected in &manifest.files {
        let relative = validate_relative(&expected.path)?;
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        let parents =
            open_directory_chain(payload, payload_guard, parent, &evidence.directories, false)?;
        let mut file = super::edit_recovery::native::open(&payload.join(&relative), false, false)?;
        let name = relative
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| BackupError::new(BackupCategory::Corrupt))?;
        if let Some(parent) = parents.last() {
            parent.validate_file(&file, name)?;
        } else {
            return Err(BackupError::new(BackupCategory::Corrupt));
        }
        let identity = super::project_file::directory::file_identity(&file)?;
        let (size, digest) = hash_opened_file(&mut file)?;
        if size != expected.size || digest != expected.sha256 {
            return Err(BackupError::new(BackupCategory::SourceChanged));
        }
        evidence.files.insert(expected.path.clone(), identity);
    }
    package_guard.validate()?;
    payload_guard.validate()?;
    Ok(evidence)
}

fn after_delete_mutation(mutations: &mut usize) -> Result<(), BackupError> {
    *mutations += 1;
    if *mutations == 1 {
        process_crash_checkpoint("quarantine_purge_after_first_mutation");
    }
    if take_test_fault("delete_after_first_mutation") {
        return Err(BackupError::io(io::Error::other(
            "test fault after owned delete mutation",
        )));
    }
    Ok(())
}

fn partial_delete_warning(category: BackupCategory) -> &'static str {
    match category {
        BackupCategory::SourceChanged => "backup_delete_incomplete_source_changed",
        BackupCategory::Corrupt => "backup_delete_incomplete_corrupt",
        BackupCategory::Io => "backup_delete_incomplete_io",
        BackupCategory::Cancelled => "backup_delete_incomplete_cancelled",
        _ => "backup_delete_incomplete",
    }
}

#[cfg(test)]
pub(crate) fn delete_backup(
    storage: &Path,
    locator: &Path,
    fingerprint: &str,
) -> Result<DeleteBackupResult, BackupError> {
    let parent = backup_parent(storage, fingerprint, false)?
        .ok_or_else(|| BackupError::new(BackupCategory::InvalidInput))?;
    delete_backup_in_parent(&parent.path, locator, fingerprint)
}

fn delete_backup_in_parent(
    parent: &Path,
    locator: &Path,
    fingerprint: &str,
) -> Result<DeleteBackupResult, BackupError> {
    let directory = canonical_existing_directory(locator)?;
    if directory.parent().map(normalized) != Some(normalized(parent)) {
        return Err(BackupError::new(BackupCategory::InvalidInput));
    }
    let name = directory
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(invalid)?;
    let id = name
        .strip_suffix(".worldbuild-backup")
        .ok_or_else(invalid)?;
    if Uuid::parse_str(id).is_err() {
        return Err(invalid());
    }
    let package_guard =
        super::project_file::directory::ProjectDirectory::open_mutable_root(&directory)?;
    let payload = directory.join(PAYLOAD_DIRECTORY);
    let payload_guard =
        super::project_file::directory::ProjectDirectory::open_mutable_root(&payload)?;
    validate_package_shape(&directory)?;
    let mut manifest_file =
        super::edit_recovery::native::open_for_discard(&directory.join(MANIFEST_NAME))?;
    package_guard.validate_file(&manifest_file, MANIFEST_NAME)?;
    let manifest = read_manifest_file(&directory, &mut manifest_file, Some(fingerprint))?;
    if manifest.id != id {
        return Err(BackupError::new(BackupCategory::Corrupt));
    }
    let evidence = capture_delete_evidence(
        &directory,
        &payload,
        &package_guard,
        &payload_guard,
        &manifest,
    )?;
    let mut mutations = 0_usize;
    let deleted = (|| {
        for expected in &manifest.files {
            let relative = validate_relative(&expected.path)?;
            let parent = relative.parent().unwrap_or_else(|| Path::new(""));
            let parents = open_directory_chain(
                &payload,
                &payload_guard,
                parent,
                &evidence.directories,
                true,
            )?;
            let mut file =
                super::edit_recovery::native::open_for_discard(&payload.join(&relative))?;
            let name = relative
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| BackupError::new(BackupCategory::Corrupt))?;
            let parent = parents
                .last()
                .ok_or_else(|| BackupError::new(BackupCategory::Corrupt))?;
            parent.validate_file(&file, name)?;
            if super::project_file::directory::file_identity(&file)?
                != *evidence
                    .files
                    .get(&expected.path)
                    .ok_or_else(|| BackupError::new(BackupCategory::Corrupt))?
            {
                return Err(BackupError::new(BackupCategory::SourceChanged));
            }
            let (size, digest) = hash_opened_file(&mut file)?;
            if size != expected.size || digest != expected.sha256 {
                return Err(BackupError::new(BackupCategory::SourceChanged));
            }
            run_delete_hook();
            super::edit_recovery::native::cleanup(&file)?;
            drop(file);
            after_delete_mutation(&mut mutations)?;
        }
        let mut directories = evidence.directories.keys().cloned().collect::<Vec<_>>();
        directories.sort_by_key(|path| {
            (
                std::cmp::Reverse(path.matches('/').count()),
                std::cmp::Reverse(path.clone()),
            )
        });
        for relative in directories {
            let mut chain = open_directory_chain(
                &payload,
                &payload_guard,
                Path::new(&relative),
                &evidence.directories,
                true,
            )?;
            let target = chain
                .pop()
                .ok_or_else(|| BackupError::new(BackupCategory::Corrupt))?;
            run_delete_hook();
            target.delete_owned()?;
            after_delete_mutation(&mut mutations)?;
        }
        let payload_identity = payload_guard.identity();
        drop(payload_guard);
        let payload_delete =
            super::project_file::directory::ProjectDirectory::open_owned_root(&payload)?;
        if payload_delete.identity() != payload_identity {
            return Err(BackupError::new(BackupCategory::SourceChanged));
        }
        payload_delete.delete_owned()?;
        after_delete_mutation(&mut mutations)?;
        super::edit_recovery::native::cleanup(&manifest_file)?;
        drop(manifest_file);
        after_delete_mutation(&mut mutations)?;
        let package_identity = package_guard.identity();
        drop(package_guard);
        let package_delete =
            super::project_file::directory::ProjectDirectory::open_owned_root(&directory)?;
        if package_delete.identity() != package_identity {
            return Err(BackupError::new(BackupCategory::SourceChanged));
        }
        package_delete.delete_owned()?;
        after_delete_mutation(&mut mutations)?;
        Ok::<(), BackupError>(())
    })();
    match deleted {
        Ok(()) => Ok(DeleteBackupResult {
            outcome: "deleted",
            warning: None,
            failure: None,
        }),
        Err(error) if mutations > 0 => {
            let category = error.category();
            Ok(DeleteBackupResult {
                outcome: "partially_deleted",
                warning: Some(partial_delete_warning(category)),
                failure: Some(category),
            })
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn purge_deleted_backup(
    storage: &Path,
    fingerprint: &str,
    id: &str,
    operation: &str,
    explicit: bool,
) -> Result<DeleteBackupResult, BackupError> {
    if Uuid::parse_str(id).is_err() || Uuid::parse_str(operation).is_err() {
        return Err(invalid());
    }
    let parent = backup_parent(storage, fingerprint, false)?.ok_or_else(invalid)?;
    let deleted = deleted_parent(&parent, false)?.ok_or_else(invalid)?;
    let receipt = read_receipt(&deleted, id, fingerprint)?;
    if receipt.operation != operation {
        return Err(BackupError::new(BackupCategory::SourceChanged));
    }
    require_absent(&purging_path(&deleted, id))?;
    let now = deleted_clock();
    let deleted_at = receipt_time(&receipt.deleted_at_utc)?;
    let expires_at = receipt_time(&receipt.expires_at_utc)?;
    if deleted_at > now || (!explicit && expires_at > now) {
        return Err(BackupError::new(BackupCategory::RecoveryRequired));
    }
    let package = deleted_package_path(&deleted, id);
    create_purging_marker(&deleted, id)?;
    let result = delete_backup_in_parent(&deleted, &package, fingerprint)?;
    if result.outcome != "deleted" {
        return Ok(result);
    }
    if read_receipt(&deleted, id, fingerprint).ok().as_ref() != Some(&receipt) {
        return Ok(DeleteBackupResult {
            outcome: "deleted_receipt_cleanup_required",
            warning: Some("backup_receipt_cleanup_required"),
            failure: None,
        });
    }
    match remove_receipt(&deleted, id)
        .and_then(|_| remove_owned_deleted_child(&deleted, &format!("{id}{PURGING_SUFFIX}")))
    {
        Ok(()) => Ok(result),
        Err(_) => Ok(DeleteBackupResult {
            outcome: "deleted_receipt_cleanup_required",
            warning: Some("backup_receipt_cleanup_required"),
            failure: None,
        }),
    }
}

pub(crate) fn cleanup_one_expired_backup(
    storage: &Path,
    fingerprint: &str,
) -> Result<Option<DeleteBackupResult>, BackupError> {
    for entry in list_deleted_backups(fingerprint, storage)? {
        if entry.status == "expired" {
            return purge_deleted_backup(
                storage,
                fingerprint,
                &entry.backup.id,
                &entry.operation,
                false,
            )
            .map(Some);
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command, sync::Arc, thread, time::Duration};

    const FOLLOW_UP_DOCUMENT: &str = "77777777-7777-4777-8777-777777777777";

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("worldbuild-m523-{name}-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        root
    }

    fn project(root: &Path) {
        fs::create_dir(root.join("templates")).unwrap();
        fs::create_dir(root.join("documents")).unwrap();
        fs::create_dir(root.join("workspace")).unwrap();
        fs::create_dir_all(root.join("assets/a")).unwrap();
        fs::write(root.join("templates/t.json"), b"{\"unknown\":1}").unwrap();
        fs::write(root.join("documents/d.json"), b"document-bytes").unwrap();
        fs::write(root.join("assets/a/metadata.json"), b"asset-meta").unwrap();
        fs::write(root.join("assets/a/original.bin"), b"asset-bytes").unwrap();
        fs::create_dir_all(root.join(".worldbuild/format-history/document-d")).unwrap();
        fs::write(
            root.join(format!(
                ".worldbuild/format-history/document-d/{}.json",
                "a".repeat(64)
            )),
            b"permanent-format-history",
        )
        .unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/canary"), b"keep").unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn quarantined_backup_restores_same_package_and_survives_relisting() {
        let base = fixture("quarantine-roundtrip");
        let source = base.join("source");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        let fingerprint = "c".repeat(64);
        let backup = create_backup(
            &source,
            &fingerprint,
            &storage,
            Some("roundtrip"),
            BackupKind::Manual,
        )
        .unwrap();
        let original = PathBuf::from(&backup.locator);
        let result = quarantine_backup(&storage, &original, &fingerprint).unwrap();
        assert_eq!(result.outcome, "quarantined");
        assert!(!original.exists());
        let deleted = list_deleted_backups(&fingerprint, &storage).unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].backup.id, backup.id);
        assert!(list_backups(&fingerprint, &storage, None)
            .unwrap()
            .backups
            .is_empty());
        let restored =
            restore_deleted_backup(&storage, &fingerprint, &backup.id, &deleted[0].operation)
                .unwrap();
        assert_eq!(restored.outcome, "restored");
        assert!(list_deleted_backups(&fingerprint, &storage)
            .unwrap()
            .is_empty());
        assert_eq!(inspect_backup(&original).unwrap().id, backup.id);
        let second = quarantine_backup(&storage, &original, &fingerprint).unwrap();
        assert_eq!(second.outcome, "quarantined");
        let repeated = list_deleted_backups(&fingerprint, &storage).unwrap();
        assert_eq!(repeated.len(), 1);
        assert_eq!(repeated[0].backup.id, backup.id);
        assert_ne!(repeated[0].operation, deleted[0].operation);
        assert_eq!(fs::read(source.join(".git/canary")).unwrap(), b"keep");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn deleted_capacity_checks_count_and_bytes_without_allocating_large_payloads() {
        assert!(deleted_capacity_allows(9, MAX_DELETED_BYTES - 72, 72));
        assert!(!deleted_capacity_allows(10, 0, 1));
        assert!(!deleted_capacity_allows(0, 0, MAX_DELETED_BYTES + 1));
        assert!(!deleted_capacity_allows(0, MAX_DELETED_BYTES, 1));
    }

    #[cfg(windows)]
    #[test]
    fn completed_purge_recovery_clears_only_its_receipt_and_marker() {
        let base = fixture("quarantine-purge-cleanup");
        let source = base.join("source");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        let fingerprint = "e".repeat(64);
        let first = create_backup(
            &source,
            &fingerprint,
            &storage,
            Some("first"),
            BackupKind::Manual,
        )
        .unwrap();
        let other = create_backup(
            &source,
            &fingerprint,
            &storage,
            Some("other"),
            BackupKind::Manual,
        )
        .unwrap();
        quarantine_backup(&storage, Path::new(&first.locator), &fingerprint).unwrap();
        let deleted = backup_parent(&storage, &fingerprint, false)
            .unwrap()
            .and_then(|parent| deleted_parent(&parent, false).unwrap())
            .unwrap();
        create_purging_marker(&deleted, &first.id).unwrap();
        fs::remove_dir_all(deleted_package_path(&deleted, &first.id)).unwrap();
        assert!(list_deleted_backups(&fingerprint, &storage)
            .unwrap()
            .is_empty());
        assert!(!receipt_path(&deleted, &first.id).exists());
        assert!(!purging_path(&deleted, &first.id).exists());
        assert_eq!(
            inspect_backup(Path::new(&other.locator)).unwrap().id,
            other.id
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn future_clock_and_unknown_receipt_never_authorize_early_cleanup() {
        struct ResetClock;
        impl Drop for ResetClock {
            fn drop(&mut self) {
                TEST_DELETED_CLOCK.with(|clock| clock.set(None));
            }
        }
        let _reset = ResetClock;
        let start = receipt_time("2026-09-24T00:00:00.000Z").unwrap();
        TEST_DELETED_CLOCK.with(|clock| clock.set(Some(start)));
        let base = fixture("quarantine-clock-uncertain");
        let source = base.join("source");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        let fingerprint = "a".repeat(64);
        let backup =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        let deleted = quarantine_backup(&storage, Path::new(&backup.locator), &fingerprint)
            .unwrap()
            .deleted
            .unwrap();
        TEST_DELETED_CLOCK.with(|clock| clock.set(Some(start - time::Duration::seconds(1))));
        assert_eq!(
            list_deleted_backups(&fingerprint, &storage).unwrap()[0].status,
            "uncertain"
        );
        assert!(cleanup_one_expired_backup(&storage, &fingerprint)
            .unwrap()
            .is_none());
        let archive = PathBuf::from(&deleted.backup.locator);
        assert!(archive.exists());
        TEST_DELETED_CLOCK.with(|clock| clock.set(Some(start)));
        let parent = backup_parent(&storage, &fingerprint, false)
            .unwrap()
            .unwrap();
        let deleted_parent = deleted_parent(&parent, false).unwrap().unwrap();
        fs::write(
            receipt_path(&deleted_parent, &backup.id),
            b"{\"schema_version\":999}",
        )
        .unwrap();
        assert_eq!(
            list_deleted_backups(&fingerprint, &storage).unwrap()[0].status,
            "uncertain"
        );
        assert!(cleanup_one_expired_backup(&storage, &fingerprint)
            .unwrap()
            .is_none());
        assert!(archive.exists());
        drop(parent);
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn deleted_backup_expires_at_exact_seven_days_and_is_purged_once() {
        struct ResetClock;
        impl Drop for ResetClock {
            fn drop(&mut self) {
                TEST_DELETED_CLOCK.with(|clock| clock.set(None));
            }
        }
        let _reset = ResetClock;
        let start = receipt_time("2026-09-24T00:00:00.000Z").unwrap();
        TEST_DELETED_CLOCK.with(|clock| clock.set(Some(start)));
        let base = fixture("quarantine-expiry");
        let source = base.join("source");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        let fingerprint = "d".repeat(64);
        let backup =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        let deleted = quarantine_backup(&storage, Path::new(&backup.locator), &fingerprint)
            .unwrap()
            .deleted
            .unwrap();
        TEST_DELETED_CLOCK.with(|clock| {
            clock.set(Some(
                start + time::Duration::days(7) - time::Duration::milliseconds(1),
            ))
        });
        assert_eq!(
            list_deleted_backups(&fingerprint, &storage).unwrap()[0].status,
            "verification_required"
        );
        assert!(cleanup_one_expired_backup(&storage, &fingerprint)
            .unwrap()
            .is_none());
        TEST_DELETED_CLOCK.with(|clock| clock.set(Some(start + time::Duration::days(7))));
        assert_eq!(
            list_deleted_backups(&fingerprint, &storage).unwrap()[0].status,
            "expired"
        );
        assert_eq!(
            restore_deleted_backup(&storage, &fingerprint, &backup.id, &deleted.operation)
                .unwrap_err()
                .category(),
            BackupCategory::RecoveryRequired
        );
        assert_eq!(
            cleanup_one_expired_backup(&storage, &fingerprint)
                .unwrap()
                .unwrap()
                .outcome,
            "deleted"
        );
        assert!(list_deleted_backups(&fingerprint, &storage)
            .unwrap()
            .is_empty());
        assert_eq!(fs::read(source.join(".git/canary")).unwrap(), b"keep");
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn deleted_backup_restore_rejects_occupied_original_without_touching_archive() {
        let base = fixture("quarantine-occupied");
        let source = base.join("source");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        let fingerprint = "e".repeat(64);
        let backup =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        let deleted = quarantine_backup(&storage, Path::new(&backup.locator), &fingerprint)
            .unwrap()
            .deleted
            .unwrap();
        fs::create_dir(&backup.locator).unwrap();
        fs::write(Path::new(&backup.locator).join("canary"), b"occupied").unwrap();
        assert_eq!(
            restore_deleted_backup(&storage, &fingerprint, &backup.id, &deleted.operation)
                .unwrap_err()
                .category(),
            BackupCategory::DestinationOccupied
        );
        assert_eq!(
            fs::read(Path::new(&backup.locator).join("canary")).unwrap(),
            b"occupied"
        );
        assert_eq!(
            list_deleted_backups(&fingerprint, &storage).unwrap().len(),
            1
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn quarantine_rejects_other_storage_and_pre_restore_safety_backup() {
        let base = fixture("quarantine-scope");
        let source = base.join("source");
        let storage = base.join("storage");
        let other = base.join("other");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        fs::create_dir(&other).unwrap();
        project(&source);
        let fingerprint = "f".repeat(64);
        let ordinary =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        assert_eq!(
            quarantine_backup(&other, Path::new(&ordinary.locator), &fingerprint)
                .unwrap_err()
                .category(),
            BackupCategory::InvalidInput
        );
        assert!(Path::new(&ordinary.locator).exists());
        let protected = create_backup(
            &source,
            &fingerprint,
            &storage,
            None,
            BackupKind::PreRestore,
        )
        .unwrap();
        assert_eq!(
            quarantine_backup(&storage, Path::new(&protected.locator), &fingerprint)
                .unwrap_err()
                .category(),
            BackupCategory::RecoveryRequired
        );
        assert!(Path::new(&protected.locator).exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn prepared_receipt_without_move_keeps_active_backup_and_recovers_listing() {
        let base = fixture("quarantine-prepared");
        let source = base.join("source");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        let fingerprint = "a".repeat(64);
        let backup =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        let parent = backup_parent(&storage, &fingerprint, false)
            .unwrap()
            .unwrap();
        let deleted = deleted_parent(&parent, true).unwrap().unwrap();
        let receipt = DeleteReceipt {
            schema_version: 1,
            id: backup.id.clone(),
            project_fingerprint: fingerprint.clone(),
            operation: Uuid::new_v4().to_string(),
            deleted_at_utc: "2026-09-24T00:00:00.000Z".into(),
            expires_at_utc: "2026-10-01T00:00:00.000Z".into(),
        };
        write_receipt(&deleted, &receipt).unwrap();
        assert!(list_deleted_backups(&fingerprint, &storage)
            .unwrap()
            .is_empty());
        assert!(receipt_path(&deleted, &backup.id).metadata().is_err());
        assert_eq!(
            inspect_backup(Path::new(&backup.locator)).unwrap().id,
            backup.id
        );
        drop(parent);
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn quarantine_ten_item_limit_preserves_eleventh_original() {
        let base = fixture("quarantine-count");
        let source = base.join("source");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        let fingerprint = "b".repeat(64);
        for _ in 0..MAX_DELETED_BACKUPS {
            let backup =
                create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
            assert_eq!(
                quarantine_backup(&storage, Path::new(&backup.locator), &fingerprint)
                    .unwrap()
                    .outcome,
                "quarantined"
            );
        }
        let next =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        assert_eq!(
            quarantine_backup(&storage, Path::new(&next.locator), &fingerprint)
                .unwrap_err()
                .category(),
            BackupCategory::RetentionFull
        );
        assert_eq!(
            inspect_backup(Path::new(&next.locator)).unwrap().id,
            next.id
        );
        assert_eq!(
            list_deleted_backups(&fingerprint, &storage).unwrap().len(),
            MAX_DELETED_BACKUPS
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn m6_child_quarantine_driver() {
        let Some(mode) = std::env::var_os("WB_M6_CHILD_MODE") else {
            return;
        };
        let root = PathBuf::from(std::env::var_os("WB_M6_ROOT").unwrap());
        let storage = PathBuf::from(std::env::var_os("WB_M6_STORAGE").unwrap());
        let locks = PathBuf::from(std::env::var_os("WB_M6_LOCKS").unwrap());
        let locator = PathBuf::from(std::env::var_os("WB_M6_LOCATOR").unwrap());
        let fingerprint = std::env::var("WB_M6_FINGERPRINT").unwrap();
        let id = std::env::var("WB_M6_BACKUP_ID").unwrap();
        if mode == "delete" {
            let _ = quarantine_backup(&storage, &locator, &fingerprint);
            panic!("delete child reached terminal state before checkpoint");
        }
        if mode == "restore" {
            let deleted = list_deleted_backups(&fingerprint, &storage).unwrap();
            let operation = deleted.first().unwrap().operation.clone();
            let _ = restore_deleted_backup(&storage, &fingerprint, &id, &operation);
            panic!("restore child reached terminal state before checkpoint");
        }
        if mode == "purge" {
            let deleted = list_deleted_backups(&fingerprint, &storage).unwrap();
            let operation = deleted.first().unwrap().operation.clone();
            let _ = purge_deleted_backup(&storage, &fingerprint, &id, &operation, true);
            panic!("purge child reached terminal state before checkpoint");
        }
        let mut runtime =
            super::super::project_runtime::ProjectRuntime::acquire(&root, &locks).unwrap();
        runtime.recover().unwrap();
        runtime.close().unwrap();
        let deleted = list_deleted_backups(&fingerprint, &storage).unwrap();
        if mode == "verify_deleted" {
            assert_eq!(deleted.len(), 1);
            assert_eq!(deleted[0].backup.id, id);
            assert!(!locator.exists());
        } else if mode == "verify_partial" {
            assert_eq!(deleted.len(), 1);
            assert_eq!(deleted[0].status, "uncertain");
            assert!(!locator.exists());
        } else {
            assert!(deleted.is_empty());
            assert_eq!(inspect_backup(&locator).unwrap().id, id);
        }
    }

    #[cfg(windows)]
    #[test]
    fn quarantine_real_child_kill_reclassifies_each_move_boundary() {
        let test_name = "data::project_backup::tests::m6_child_quarantine_driver";
        for (point, mode) in [
            ("quarantine_receipt_before_move", "delete"),
            ("quarantine_move_before_readback", "delete"),
            ("quarantine_restore_before_receipt_cleanup", "restore"),
            ("quarantine_purge_after_first_mutation", "purge"),
        ] {
            let base = fixture(point);
            let root = base.join("project");
            let storage = base.join("storage");
            let locks = base.join("locks");
            fs::create_dir(&root).unwrap();
            fs::create_dir(&storage).unwrap();
            fs::create_dir(&locks).unwrap();
            project(&root);
            let fingerprint = "c".repeat(64);
            let backup =
                create_backup(&root, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
            if mode == "restore" || mode == "purge" {
                assert_eq!(
                    quarantine_backup(&storage, Path::new(&backup.locator), &fingerprint)
                        .unwrap()
                        .outcome,
                    "quarantined"
                );
            }
            let ready = base.join("checkpoint-ready");
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test_name, "--nocapture"])
                .env("WB_M6_CHILD_MODE", mode)
                .env("WB_M523_CRASH_POINT", point)
                .env("WB_M523_CRASH_READY", &ready)
                .env("WB_M6_ROOT", &root)
                .env("WB_M6_STORAGE", &storage)
                .env("WB_M6_LOCKS", &locks)
                .env("WB_M6_LOCATOR", &backup.locator)
                .env("WB_M6_FINGERPRINT", &fingerprint)
                .env("WB_M6_BACKUP_ID", &backup.id)
                .spawn()
                .unwrap();
            for _ in 0..1500 {
                if ready.exists() {
                    break;
                }
                if let Some(status) = child.try_wait().unwrap() {
                    panic!("child exited before {point}: {status}");
                }
                thread::sleep(Duration::from_millis(20));
            }
            assert!(ready.exists(), "child did not reach {point}");
            child.kill().unwrap();
            child.wait().unwrap();
            let expected = if point == "quarantine_move_before_readback" {
                "verify_deleted"
            } else if mode == "purge" {
                "verify_partial"
            } else {
                "verify_active"
            };
            let status = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test_name, "--nocapture"])
                .env("WB_M6_CHILD_MODE", expected)
                .env("WB_M6_ROOT", &root)
                .env("WB_M6_STORAGE", &storage)
                .env("WB_M6_LOCKS", &locks)
                .env("WB_M6_LOCATOR", &backup.locator)
                .env("WB_M6_FINGERPRINT", &fingerprint)
                .env("WB_M6_BACKUP_ID", &backup.id)
                .status()
                .unwrap();
            assert!(status.success(), "restart failed after {point}");
            fs::remove_dir_all(base).unwrap();
        }
    }

    fn follow_up_document() -> super::super::artifact::DocumentArtifact {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "artifactType":"document",
            "schemaVersion":1,
            "documentId":FOLLOW_UP_DOCUMENT,
            "templateId":"11111111-1111-4111-8111-111111111111",
            "templateRevision":1,
            "name":"recovery follow-up",
            "fieldValues":{},
            "orphanedFieldDefinitions":{},
            "createdAtUtc":"2026-09-21T00:00:00.000Z",
            "updatedAtUtc":"2026-09-21T00:00:00.000Z",
            "future":null
        }))
        .unwrap();
        super::super::artifact::decode_document(&bytes).unwrap()
    }

    fn repository_follow_up(
        runtime: &mut super::super::project_runtime::ProjectRuntime,
        create: bool,
    ) {
        use super::super::{
            collaboration_lock::{LockCoordinator, LockService, LockSessionId, NoLockService},
            repository::{ArtifactRepository, CanonicalWritePlan},
        };
        let ready = runtime.ready().expect("reader is issued after recovery");
        let repository = ArtifactRepository::new(&ready).expect("repository opens after recovery");
        let id = FOLLOW_UP_DOCUMENT.parse().unwrap();
        if create {
            let plan = CanonicalWritePlan::new()
                .create_document(&follow_up_document())
                .unwrap();
            let targets = plan.targets().cloned().collect();
            let service: Arc<dyn LockService> = Arc::new(NoLockService::new());
            let coordinator = LockCoordinator::new(service);
            let mut locks = coordinator
                .acquire_all(
                    repository.write_project().fingerprint(),
                    &LockSessionId::generate().unwrap(),
                    targets,
                )
                .unwrap();
            let prepared = plan
                .prepare(&repository, locks.write_permit().unwrap())
                .unwrap();
            prepared.commit().unwrap();
            locks.release_all().unwrap();
        }
        let loaded = repository.load_document(id).unwrap();
        assert_eq!(loaded.artifact().name(), "recovery follow-up");
    }

    #[test]
    fn copy_and_backup_preserve_bytes_and_exclude_scm() {
        let base = fixture("copy");
        let source = base.join("source");
        let output = base.join("output");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&output).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        fs::create_dir(source.join(".svn")).unwrap();
        fs::write(source.join(".svn/entries"), b"working-copy metadata").unwrap();
        let copied = export_project(&source, &output, "copy").unwrap();
        assert_eq!(
            fs::read(copied.root.join("templates/t.json")).unwrap(),
            b"{\"unknown\":1}"
        );
        assert!(!copied.root.join(".git").exists());
        assert!(!copied.root.join(".svn").exists());
        assert_eq!(
            fs::read(copied.root.join(format!(
                ".worldbuild/format-history/document-d/{}.json",
                "a".repeat(64)
            )))
            .unwrap(),
            b"permanent-format-history"
        );
        let fingerprint = "a".repeat(64);
        let backup = create_backup(
            &source,
            &fingerprint,
            &storage,
            Some("manual"),
            BackupKind::Manual,
        )
        .unwrap();
        let page = list_backups(&fingerprint, &storage, None).unwrap();
        assert_eq!(page.backups.len(), 1);
        assert_eq!(page.backups[0].id, backup.id);
        assert_eq!(page.backups[0].status, "verification_required");
        assert!(page.next_cursor.is_none());
        assert_eq!(
            inspect_backup(Path::new(&page.backups[0].locator))
                .unwrap()
                .status,
            "verified"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn two_complete_backups_list_and_restore_the_selected_snapshot() {
        let base = fixture("two-complete-backups");
        let source = base.join("source");
        let storage = base.join("storage");
        let output = base.join("output");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        fs::create_dir(&output).unwrap();
        project(&source);
        let fingerprint = "c".repeat(64);
        let first = create_backup(
            &source,
            &fingerprint,
            &storage,
            Some("first"),
            BackupKind::Manual,
        )
        .unwrap();
        fs::write(source.join("templates/t.json"), b"{\"second\":2}").unwrap();
        let second = create_backup(
            &source,
            &fingerprint,
            &storage,
            Some("second"),
            BackupKind::Manual,
        )
        .unwrap();
        let page = list_backups(&fingerprint, &storage, None).unwrap();
        assert_eq!(page.backups.len(), 2);
        assert!(page.next_cursor.is_none());
        for backup in [&first, &second] {
            let row = page.backups.iter().find(|row| row.id == backup.id).unwrap();
            assert_eq!(row.coverage, "complete");
            assert_eq!(inspect_backup(Path::new(&row.locator)).unwrap().id, row.id);
        }
        let selected = page.backups.iter().find(|row| row.id == first.id).unwrap();
        let restored = restore_new(Path::new(&selected.locator), &output, "selected").unwrap();
        assert_eq!(restored.outcome, "published_verified");
        assert_eq!(
            fs::read(output.join("selected/templates/t.json")).unwrap(),
            b"{\"unknown\":1}"
        );
        assert_eq!(
            fs::read(source.join("templates/t.json")).unwrap(),
            b"{\"second\":2}"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn copy_cancelled_at_file_checkpoint_never_publishes_destination() {
        let base = fixture("cancel-copy");
        let source = base.join("source");
        let output = base.join("output");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&output).unwrap();
        project(&source);
        let progress = Arc::new(super::super::repository::progress::Progress::default());
        let (entered, resume) = progress.hold_checkpoint();
        let task_progress = progress.clone();
        let task_source = source.clone();
        let task_output = output.clone();
        let task = thread::spawn(move || {
            let _scope = super::super::repository::progress::scope(task_progress);
            export_project(&task_source, &task_output, "copy")
        });
        entered
            .recv_timeout(std::time::Duration::from_secs(20))
            .unwrap();
        progress.cancel();
        resume.send(()).unwrap();
        let error = task.join().unwrap().unwrap_err();
        assert_eq!(error.category(), BackupCategory::Cancelled);
        assert!(!output.join("copy").exists());
        assert_eq!(fs::read(source.join(".git/canary")).unwrap(), b"keep");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn unclassified_root_entry_rejects_snapshot_instead_of_silently_omitting_it() {
        let base = fixture("unclassified");
        let source = base.join("source");
        let output = base.join("output");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&output).unwrap();
        project(&source);
        fs::write(source.join("unknown-user-file.bin"), b"preserve me").unwrap();
        let error = export_project(&source, &output, "copy").unwrap_err();
        assert_eq!(error.category(), BackupCategory::InvalidInput);
        assert!(!output.join("copy").exists());
        assert_eq!(
            fs::read(source.join("unknown-user-file.bin")).unwrap(),
            b"preserve me"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn restore_current_removes_old_managed_files_and_keeps_unmanaged_canary() {
        let base = fixture("restore");
        let source = base.join("source");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        let fingerprint = "b".repeat(64);
        let backup =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        fs::write(source.join("templates/t.json"), b"changed").unwrap();
        fs::write(source.join("documents/extra.json"), b"extra").unwrap();
        let newer_history = source.join(format!(
            ".worldbuild/format-history/document-d/{}.json",
            "b".repeat(64)
        ));
        fs::write(&newer_history, b"newer-history").unwrap();
        let restored =
            restore_current(&source, &fingerprint, Path::new(&backup.locator), &storage).unwrap();
        assert_eq!(
            fs::read(source.join("templates/t.json")).unwrap(),
            b"{\"unknown\":1}"
        );
        assert!(!source.join("documents/extra.json").exists());
        assert_eq!(fs::read(source.join(".git/canary")).unwrap(), b"keep");
        assert!(!newer_history.exists());
        let safety = restored.safety.unwrap();
        assert_eq!(
            fs::read(
                Path::new(&safety.locator)
                    .join(PAYLOAD_DIRECTORY)
                    .join(format!(
                        ".worldbuild/format-history/document-d/{}.json",
                        "b".repeat(64)
                    ))
            )
            .unwrap(),
            b"newer-history"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn restart_recovery_discards_uncommitted_prepare_and_rolls_committed_plan_forward() {
        let base = fixture("restart");
        let root = base.join("project");
        let target = base.join("target");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&target).unwrap();
        project(&root);
        project(&target);
        fs::write(target.join("templates/t.json"), b"target bytes").unwrap();
        fs::remove_file(target.join("documents/d.json")).unwrap();
        let (target_directories, target_files) = collect(&target).unwrap();

        let prepare = |operation_id: &str, committed: bool| {
            let parent = root.join(RESTORE_DIRECTORY);
            fs::create_dir_all(&parent).unwrap();
            let operation = parent.join(operation_id);
            fs::create_dir(&operation).unwrap();
            materialize(
                &target,
                &operation.join("new"),
                &target_directories,
                &target_files,
            )
            .unwrap();
            atomic_file::save_deterministic_json(
                &operation.join(RESTORE_MANIFEST),
                &RestoreJournal {
                    schema_version: 1,
                    operation_id: operation_id.into(),
                    target_files: target_files.clone(),
                    target_directories: target_directories.clone(),
                },
            )
            .unwrap();
            if committed {
                File::create(operation.join(COMMIT_MARKER)).unwrap();
            }
        };

        let before = fs::read(root.join("templates/t.json")).unwrap();
        prepare("11111111-1111-4111-8111-111111111111", false);
        recover_pending(&root).unwrap();
        assert_eq!(fs::read(root.join("templates/t.json")).unwrap(), before);

        prepare("22222222-2222-4222-8222-222222222222", true);
        fs::write(root.join("templates/t.json"), b"partial mixed state").unwrap();
        recover_pending(&root).unwrap();
        assert_eq!(
            fs::read(root.join("templates/t.json")).unwrap(),
            b"target bytes"
        );
        assert!(!root.join("documents/d.json").exists());
        assert_eq!(fs::read(root.join(".git/canary")).unwrap(), b"keep");
        assert!(!root.join(RESTORE_DIRECTORY).exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn delete_requires_selected_owned_storage_and_preserves_unexpected_entries() {
        let base = fixture("delete-owner");
        let source = base.join("source");
        let storage = base.join("storage");
        let other = base.join("other");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        fs::create_dir(&other).unwrap();
        project(&source);
        let fingerprint = "c".repeat(64);
        let backup =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        let manifest_alias = base.join("manifest-hardlink.json");
        fs::hard_link(
            Path::new(&backup.locator).join(MANIFEST_NAME),
            &manifest_alias,
        )
        .unwrap();
        assert_eq!(
            inspect_backup(Path::new(&backup.locator))
                .unwrap_err()
                .category(),
            BackupCategory::Corrupt
        );
        fs::remove_file(manifest_alias).unwrap();
        assert_eq!(
            delete_backup(&other, Path::new(&backup.locator), &fingerprint)
                .unwrap_err()
                .category(),
            BackupCategory::InvalidInput
        );
        let canary = Path::new(&backup.locator).join("external-canary");
        fs::write(&canary, b"keep").unwrap();
        assert_eq!(
            delete_backup(&storage, Path::new(&backup.locator), &fingerprint)
                .unwrap_err()
                .category(),
            BackupCategory::Corrupt
        );
        assert_eq!(fs::read(canary).unwrap(), b"keep");
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn delete_holds_package_and_nested_directory_ownership_until_exact_handle_cleanup() {
        let base = fixture("delete-owned-handles");
        let source = base.join("source");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        let fingerprint = "4".repeat(64);
        let backup =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        let sibling = create_backup(
            &source,
            &fingerprint,
            &storage,
            Some("sibling"),
            BackupKind::Manual,
        )
        .unwrap();
        let package = PathBuf::from(&backup.locator);
        let nested = package
            .join(PAYLOAD_DIRECTORY)
            .join(".worldbuild/format-history/document-d");
        let moved_package = base.join("swapped-package");
        let moved_nested = base.join("swapped-nested");
        let hook_package = package.clone();
        let hook_nested = nested.clone();
        TEST_DELETE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let package_attempt =
                    thread::spawn(move || fs::rename(&hook_package, &moved_package));
                let nested_attempt = thread::spawn(move || fs::rename(&hook_nested, &moved_nested));
                assert!(package_attempt.join().unwrap().is_err());
                assert!(nested_attempt.join().unwrap().is_err());
            }));
        });

        let result = delete_backup(&storage, &package, &fingerprint).unwrap();
        assert_eq!(result.outcome, "deleted");
        assert!(result.failure.is_none());
        assert!(!package.exists());
        assert!(Path::new(&sibling.locator).is_dir());
        assert_eq!(fs::read(source.join(".git/canary")).unwrap(), b"keep");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn delete_reports_partial_outcome_after_first_owned_mutation() {
        let base = fixture("delete-partial");
        let source = base.join("source");
        let storage = base.join("storage");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        project(&source);
        let fingerprint = "5".repeat(64);
        let backup =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        let sibling = create_backup(
            &source,
            &fingerprint,
            &storage,
            Some("sibling"),
            BackupKind::Manual,
        )
        .unwrap();
        let external = base.join("external-canary");
        fs::write(&external, b"keep").unwrap();
        TEST_FAULT.with(|fault| fault.set(Some("delete_after_first_mutation")));

        let result = delete_backup(&storage, Path::new(&backup.locator), &fingerprint).unwrap();
        assert_eq!(result.outcome, "partially_deleted");
        assert_eq!(result.warning, Some("backup_delete_incomplete_io"));
        assert_eq!(result.failure, Some(BackupCategory::Io));
        assert!(Path::new(&backup.locator).is_dir());
        assert!(Path::new(&sibling.locator).is_dir());
        assert_eq!(fs::read(&external).unwrap(), b"keep");
        assert_eq!(
            inspect_backup(Path::new(&backup.locator))
                .unwrap_err()
                .category(),
            BackupCategory::Corrupt
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn backup_storage_rejects_reparse_hierarchy_without_touching_target() {
        use std::os::windows::fs::symlink_dir;
        let base = fixture("storage-reparse");
        let source = base.join("source");
        let storage = base.join("storage");
        let external = base.join("external");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        fs::create_dir(&external).unwrap();
        fs::write(external.join("canary"), b"keep").unwrap();
        project(&source);
        if symlink_dir(&external, storage.join(BACKUP_DIRECTORY)).is_err() {
            fs::remove_dir_all(base).unwrap();
            return;
        }
        assert_eq!(
            create_backup(&source, &"9".repeat(64), &storage, None, BackupKind::Manual,)
                .unwrap_err()
                .category(),
            BackupCategory::Corrupt
        );
        assert_eq!(fs::read(external.join("canary")).unwrap(), b"keep");
        fs::remove_dir(storage.join(BACKUP_DIRECTORY)).unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn legacy_backup_is_visible_and_new_restorable_but_current_restore_is_blocked() {
        let base = fixture("legacy-coverage");
        let source = base.join("source");
        let storage = base.join("storage");
        let output = base.join("output");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        fs::create_dir(&output).unwrap();
        project(&source);
        let fingerprint = "f".repeat(64);
        let backup =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        let manifest_path = Path::new(&backup.locator).join(MANIFEST_NAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["schemaVersion"] = 1.into();
        manifest
            .as_object_mut()
            .unwrap()
            .remove("formatHistoryComplete");
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        let inspected = inspect_backup(Path::new(&backup.locator)).unwrap();
        assert_eq!(inspected.status, "verified");
        assert_eq!(inspected.coverage, "legacy_unknown");
        let restored = restore_new(Path::new(&backup.locator), &output, "legacy-copy").unwrap();
        assert_eq!(restored.outcome, "published_verified");
        assert_eq!(
            restore_current(&source, &fingerprint, Path::new(&backup.locator), &storage,)
                .unwrap_err()
                .category(),
            BackupCategory::Unsupported
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn publication_and_commit_boundaries_return_typed_outcomes() {
        let base = fixture("typed-outcomes");
        let source = base.join("source");
        let storage = base.join("storage");
        let output = base.join("output");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&storage).unwrap();
        fs::create_dir(&output).unwrap();
        project(&source);
        let fingerprint = "1".repeat(64);
        let backup =
            create_backup(&source, &fingerprint, &storage, None, BackupKind::Manual).unwrap();

        TEST_FAULT.with(|fault| fault.set(Some("copy_after_publish")));
        let copied = export_project(&source, &output, "copy").unwrap();
        assert_eq!(copied.outcome, "published_verification_uncertain");
        assert!(copied.root.is_dir());

        TEST_FAULT.with(|fault| fault.set(Some("restore_new_after_publish")));
        let restored_new = restore_new(Path::new(&backup.locator), &output, "restored").unwrap();
        assert_eq!(restored_new.outcome, "published_verification_uncertain");
        assert!(restored_new.root.is_dir());

        fs::write(source.join("templates/t.json"), b"before-not-applied").unwrap();
        TEST_FAULT.with(|fault| fault.set(Some("restore_prepare_after_safety")));
        let not_applied =
            restore_current(&source, &fingerprint, Path::new(&backup.locator), &storage).unwrap();
        assert_eq!(not_applied.outcome, "not_applied_safety_created");
        assert_eq!(
            fs::read(source.join("templates/t.json")).unwrap(),
            b"before-not-applied"
        );
        assert!(not_applied.safety.is_some());

        TEST_FAULT.with(|fault| fault.set(Some("restore_cleanup_after_apply")));
        let recovered =
            restore_current(&source, &fingerprint, Path::new(&backup.locator), &storage).unwrap();
        assert_eq!(recovered.outcome, "applied_recovered");
        assert_eq!(
            fs::read(source.join("templates/t.json")).unwrap(),
            b"{\"unknown\":1}"
        );
        assert!(!source.join(RESTORE_DIRECTORY).exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn list_paginates_all_backups_after_non_backup_entries() {
        let base = fixture("paging");
        let storage = base.join("storage");
        fs::create_dir(&storage).unwrap();
        let fingerprint = "d".repeat(64);
        let parent = backup_parent(&storage, &fingerprint, true)
            .unwrap()
            .unwrap();
        for index in 0..1000 {
            fs::create_dir(parent.path.join(format!("non-backup-{index:04}"))).unwrap();
        }
        for index in 0..1001 {
            let id = Uuid::new_v4().to_string();
            let package = parent.path.join(format!("{id}.worldbuild-backup"));
            fs::create_dir(&package).unwrap();
            fs::create_dir(package.join(PAYLOAD_DIRECTORY)).unwrap();
            fs::write(
                package.join(MANIFEST_NAME),
                serde_json::to_vec(&Manifest {
                    schema_version: CURRENT_MANIFEST_SCHEMA,
                    id,
                    project_fingerprint: fingerprint.clone(),
                    created_at_utc: format!(
                        "2026-09-20T00:{:02}:{:02}.000Z",
                        index / 60,
                        index % 60
                    ),
                    kind: BackupKind::Manual,
                    label: String::new(),
                    directories: vec![],
                    files: vec![],
                    total_bytes: 0,
                    complete: true,
                    format_history_complete: true,
                })
                .unwrap(),
            )
            .unwrap();
        }
        let mut cursor = None;
        let mut count = 0;
        let mut ordinals = BTreeSet::new();
        loop {
            let page = list_backups(&fingerprint, &storage, cursor.as_deref()).unwrap();
            count += page.backups.len();
            ordinals.extend(
                page.backups
                    .iter()
                    .map(|row| row.unnamed_ordinal.expect("unnamed row ordinal")),
            );
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(count, 1001);
        assert_eq!(ordinals.len(), 1001);
        assert_eq!(ordinals.first(), Some(&1));
        assert_eq!(ordinals.last(), Some(&1001));
        drop(parent);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[ignore = "explicit M5-6 metadata-only backup listing measurement"]
    fn m56_backup_list_measurement() {
        let base =
            PathBuf::from(std::env::var_os("M56_BACKUP_BASE").expect("owned benchmark base"));
        let storage = base.join("storage");
        let marker = base.join("M56-BACKUP-FIXTURE");
        let fingerprint = "d".repeat(64);
        let count: usize = std::env::var("M56_BACKUP_COUNT")
            .unwrap_or_else(|_| "1001".into())
            .parse()
            .unwrap();
        let files_per_manifest: usize = std::env::var("M56_BACKUP_FILES")
            .unwrap_or_else(|_| "0".into())
            .parse()
            .unwrap();
        let noise = if count > 100 { 1000 } else { 0 };
        let marker_bytes = format!("m56-backup-v2-{count}-{files_per_manifest}-{noise}");
        if !marker.exists() {
            fs::create_dir_all(&storage).unwrap();
            let parent = backup_parent(&storage, &fingerprint, true)
                .unwrap()
                .unwrap();
            for index in 0..noise {
                fs::create_dir(parent.path.join(format!("non-backup-{index:04}"))).unwrap();
            }
            let files = (0..files_per_manifest)
                .map(|index| ManifestFile {
                    path: format!("documents/f{index:06}.json"),
                    size: 0,
                    sha256: "0".repeat(64),
                })
                .collect::<Vec<_>>();
            for index in 0..count {
                let id = Uuid::from_u128(index as u128 + 1).to_string();
                let package = parent.path.join(format!("{id}.worldbuild-backup"));
                fs::create_dir(&package).unwrap();
                fs::create_dir(package.join(PAYLOAD_DIRECTORY)).unwrap();
                fs::write(
                    package.join(MANIFEST_NAME),
                    serde_json::to_vec(&Manifest {
                        schema_version: CURRENT_MANIFEST_SCHEMA,
                        id,
                        project_fingerprint: fingerprint.clone(),
                        created_at_utc: format!(
                            "2026-09-20T00:{:02}:{:02}.000Z",
                            index / 60,
                            index % 60
                        ),
                        kind: BackupKind::Manual,
                        label: String::new(),
                        directories: if files_per_manifest == 0 {
                            vec![]
                        } else {
                            vec!["documents".into()]
                        },
                        files: files.clone(),
                        total_bytes: 0,
                        complete: true,
                        format_history_complete: true,
                    })
                    .unwrap(),
                )
                .unwrap();
            }
            drop(parent);
            fs::write(&marker, marker_bytes.as_bytes()).unwrap();
        }
        assert_eq!(fs::read(&marker).unwrap(), marker_bytes.as_bytes());
        let start = std::time::Instant::now();
        let first = list_backups(&fingerprint, &storage, None).unwrap();
        let first_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(first.backups.len(), count.min(100));
        let next_ms = first.next_cursor.as_deref().map(|cursor| {
            let start = std::time::Instant::now();
            let next = list_backups(&fingerprint, &storage, Some(cursor)).unwrap();
            assert_eq!(next.backups.len(), (count - 100).min(100));
            start.elapsed().as_secs_f64() * 1000.0
        });
        let start = std::time::Instant::now();
        let again = list_backups(&fingerprint, &storage, None).unwrap();
        let again_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(again.backups, first.backups);
        println!(
            "M56_BACKUP_JSON {}",
            serde_json::json!({
                "backups": count,
                "raw_entries": count + noise,
                "files_per_manifest": files_per_manifest,
                "first_ms": first_ms,
                "next_ms": next_ms,
                "again_ms": again_ms,
                "page_size": first.backups.len(),
                "status": first.backups[0].status,
            })
        );
    }

    fn manifest_with_directory_count(count: usize) -> Manifest {
        let id = Uuid::new_v4().to_string();
        let mut directories = Vec::with_capacity(count);
        if count > 0 {
            directories.push("documents".to_owned());
            directories.extend((0..count - 1).map(|index| format!("documents/d{index:06}")));
        }
        Manifest {
            schema_version: CURRENT_MANIFEST_SCHEMA,
            id,
            project_fingerprint: "6".repeat(64),
            created_at_utc: "2026-09-21T00:00:00.000Z".into(),
            kind: BackupKind::Manual,
            label: String::new(),
            directories,
            files: vec![],
            total_bytes: 0,
            complete: true,
            format_history_complete: true,
        }
    }

    #[test]
    fn manifest_directory_cap_accepts_99999_and_100000_but_rejects_100001() {
        for count in [99_999, 100_000] {
            let manifest = manifest_with_directory_count(count);
            let name = format!("{}.worldbuild-backup", manifest.id);
            validate_manifest(&manifest, Some(&name), Some(&manifest.project_fingerprint)).unwrap();
        }
        let manifest = manifest_with_directory_count(100_001);
        let name = format!("{}.worldbuild-backup", manifest.id);
        assert_eq!(
            validate_manifest(&manifest, Some(&name), Some(&manifest.project_fingerprint))
                .unwrap_err()
                .category(),
            BackupCategory::Corrupt
        );
        assert!(manifest_size_allowed(MAX_MANIFEST_BYTES));
        assert!(!manifest_size_allowed(MAX_MANIFEST_BYTES + 1));
    }

    #[test]
    fn manifest_rejects_unsorted_duplicate_or_incomplete_directory_topology() {
        let mut manifest = manifest_with_directory_count(0);
        let name = format!("{}.worldbuild-backup", manifest.id);
        for directories in [
            vec!["documents/z".into(), "documents".into()],
            vec!["documents".into(), "documents".into()],
            vec!["documents/child".into()],
        ] {
            manifest.directories = directories;
            assert_eq!(
                validate_manifest(&manifest, Some(&name), Some(&manifest.project_fingerprint))
                    .unwrap_err()
                    .category(),
                BackupCategory::Corrupt
            );
        }
        manifest.directories = vec!["documents".into(), "documents/collision".into()];
        manifest.files = vec![ManifestFile {
            path: "documents/collision".into(),
            size: 0,
            sha256: "0".repeat(64),
        }];
        assert_eq!(
            validate_manifest(&manifest, Some(&name), Some(&manifest.project_fingerprint))
                .unwrap_err()
                .category(),
            BackupCategory::Corrupt
        );

        manifest.directories = vec!["documents".into()];
        manifest.files = (0..=MAX_FILES)
            .map(|index| ManifestFile {
                path: format!("documents/{index:06}.json"),
                size: 0,
                sha256: "0".repeat(64),
            })
            .collect();
        assert_eq!(
            validate_manifest(&manifest, Some(&name), Some(&manifest.project_fingerprint))
                .unwrap_err()
                .category(),
            BackupCategory::Corrupt
        );

        manifest.files = vec![
            ManifestFile {
                path: "documents/a.json".into(),
                size: u64::MAX,
                sha256: "0".repeat(64),
            },
            ManifestFile {
                path: "documents/b.json".into(),
                size: 1,
                sha256: "0".repeat(64),
            },
        ];
        manifest.total_bytes = 0;
        assert_eq!(
            validate_manifest(&manifest, Some(&name), Some(&manifest.project_fingerprint))
                .unwrap_err()
                .category(),
            BackupCategory::TooLarge
        );
    }

    #[test]
    fn oversized_decoded_directory_manifest_is_corrupt_in_inspect_and_list() {
        let base = fixture("manifest-directory-cap");
        let storage = base.join("storage");
        fs::create_dir(&storage).unwrap();
        let fingerprint = "6".repeat(64);
        let parent = backup_parent(&storage, &fingerprint, true)
            .unwrap()
            .unwrap();
        let manifest = manifest_with_directory_count(100_001);
        let package = parent
            .path
            .join(format!("{}.worldbuild-backup", manifest.id));
        fs::create_dir(&package).unwrap();
        fs::create_dir(package.join(PAYLOAD_DIRECTORY)).unwrap();
        fs::write(
            package.join(MANIFEST_NAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert_eq!(
            inspect_backup(&package).unwrap_err().category(),
            BackupCategory::Corrupt
        );
        let page = list_backups(&fingerprint, &storage, None).unwrap();
        assert_eq!(page.backups.len(), 1);
        assert_eq!(page.backups[0].status, "corrupt");
        drop(parent);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn m523_child_restore_driver() {
        let Some(mode) = std::env::var_os("WB_M523_CHILD_MODE") else {
            return;
        };
        let root = PathBuf::from(std::env::var_os("WB_M523_ROOT").unwrap());
        let locks = PathBuf::from(std::env::var_os("WB_M523_LOCKS").unwrap());
        if mode == "restore" {
            let storage = PathBuf::from(std::env::var_os("WB_M523_STORAGE").unwrap());
            let locator = PathBuf::from(std::env::var_os("WB_M523_LOCATOR").unwrap());
            let fingerprint = std::env::var("WB_M523_FINGERPRINT").unwrap();
            let _ = restore_current(&root, &fingerprint, &locator, &storage);
            panic!("restore child reached terminal state instead of checkpoint");
        }
        let mut runtime = super::super::project_runtime::ProjectRuntime::acquire(&root, &locks)
            .expect("restart acquires project");
        runtime.recover().expect("startup recovery succeeds");
        repository_follow_up(&mut runtime, mode == "recover");
        runtime.close().expect("restart closes cleanly");
    }

    fn crash_restart_case(point: &str) {
        let base = fixture(point);
        let root = base.join("project");
        let storage = base.join("storage");
        let locks = base.join("locks");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&storage).unwrap();
        fs::create_dir(&locks).unwrap();
        project(&root);
        let fingerprint = "e".repeat(64);
        let backup =
            create_backup(&root, &fingerprint, &storage, None, BackupKind::Manual).unwrap();
        fs::write(root.join("templates/t.json"), b"changed-before-crash").unwrap();
        fs::write(root.join("documents/extra.json"), b"remove-on-restore").unwrap();
        let ready = base.join("checkpoint-ready");
        let test_name = "data::project_backup::tests::m523_child_restore_driver";
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env("WB_M523_CHILD_MODE", "restore")
            .env("WB_M523_CRASH_POINT", point)
            .env("WB_M523_CRASH_READY", &ready)
            .env("WB_M523_ROOT", &root)
            .env("WB_M523_STORAGE", &storage)
            .env("WB_M523_LOCATOR", &backup.locator)
            .env("WB_M523_FINGERPRINT", &fingerprint)
            .env("WB_M523_LOCKS", &locks)
            .spawn()
            .unwrap();
        for _ in 0..1500 {
            if ready.exists() {
                break;
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!("restore child exited before checkpoint: {status}");
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(ready.exists(), "child did not reach {point}");
        if point == "after_commit_marker" {
            assert_eq!(
                fs::read(root.join("templates/t.json")).unwrap(),
                b"changed-before-crash"
            );
        } else {
            assert_eq!(
                fs::read(root.join("templates/t.json")).unwrap(),
                b"{\"unknown\":1}"
            );
        }
        assert!(root.join("documents/extra.json").exists());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(root.join(RESTORE_DIRECTORY).exists());

        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env("WB_M523_CHILD_MODE", "recover")
            .env("WB_M523_ROOT", &root)
            .env("WB_M523_LOCKS", &locks)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            fs::read(root.join("templates/t.json")).unwrap(),
            b"{\"unknown\":1}"
        );
        assert!(!root.join("documents/extra.json").exists());
        assert_eq!(fs::read(root.join(".git/canary")).unwrap(), b"keep");
        assert!(root
            .join(format!("documents/{FOLLOW_UP_DOCUMENT}.json"))
            .is_file());
        assert!(!root.join(RESTORE_DIRECTORY).exists());
        let second_restart = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env("WB_M523_CHILD_MODE", "verify")
            .env("WB_M523_ROOT", &root)
            .env("WB_M523_LOCKS", &locks)
            .status()
            .unwrap();
        assert!(second_restart.success());
        let page = list_backups(&fingerprint, &storage, None).unwrap();
        assert!(page
            .backups
            .iter()
            .any(|row| row.kind == BackupKind::PreRestore));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn killed_child_recovers_after_commit_marker_and_during_apply() {
        crash_restart_case("after_commit_marker");
        crash_restart_case("during_apply");
    }
}

#[cfg(test)]
mod collaboration_policy_snapshot_tests {
    use super::*;
    #[test]
    fn policy_is_an_exact_snapshot_path_and_cannot_authorize_an_arbitrary_root_file() {
        let root = std::env::temp_dir().join(format!("policy-snapshot-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let bytes = b"preserve policy bytes independently of the artifact codec";
        fs::write(root.join(crate::svn::policy::FILE), bytes).unwrap();
        let (_, files) = collect(&root).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, crate::svn::policy::FILE);
        assert_eq!(files[0].sha256, format!("{:x}", Sha256::digest(bytes)));
        assert!(validate_relative(crate::svn::policy::FILE).is_ok());
        assert!(validate_relative("private.json").is_err());
        fs::write(root.join("private.json"), b"private").unwrap();
        assert!(collect(&root).is_err());
        fs::remove_file(root.join("private.json")).unwrap();
        fs::remove_file(root.join(crate::svn::policy::FILE)).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
