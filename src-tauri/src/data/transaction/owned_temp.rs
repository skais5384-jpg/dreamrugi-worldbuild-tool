//! Canonical transaction만 쓰는 소유 임시 파일 프로토콜이다.
//! 생성 시 삭제 예약을 하고 실제 ID receipt를 sync한 뒤 같은 handle에서 해제한다.
//! 프로세스 종료 시 OS handle 정리에 의존하며 전원 장애 자동 삭제를 보장하지 않는다.
use super::prepare::sha256;
use super::{
    LockedProject, ProjectRelativePath, TransactionId, TransactionManifest, TransactionOperation,
};
use crate::data::project_file::{
    self,
    directory::{is_reparse, ProjectDirectory},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const DIRECTORY: &str = "owned-temp-v1";
#[derive(Debug, Clone, Copy)]
pub(crate) enum OwnedStage {
    Intent,
    Create,
    Identity,
    Acquired,
    ClearDeleteOnClose,
    WritePayload,
    Rename,
    Sync,
    Verify,
    Cleanup,
}
#[derive(Debug)]
struct OwnedFailure {
    stage: OwnedStage,
    source: io::Error,
}
impl std::fmt::Display for OwnedFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "owned temporary {:?}: {:?} (OS {:?})",
            self.stage,
            self.source.kind(),
            self.source.raw_os_error()
        )
    }
}
impl std::error::Error for OwnedFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
fn at(stage: OwnedStage, source: io::Error) -> io::Error {
    io::Error::new(source.kind(), OwnedFailure { stage, source })
}
pub(super) fn diagnostic(error: &io::Error) -> Option<(OwnedStage, &io::Error)> {
    let failure = error.get_ref()?.downcast_ref::<OwnedFailure>()?;
    Some((failure.stage, &failure.source))
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Phase {
    Apply,
    Restore,
}
impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Restore => "restore",
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    transaction: TransactionId,
    operation: u32,
    phase: Phase,
    target: ProjectRelativePath,
    temporary: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectId {
    volume: u64,
    index: [u8; 16],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    intent: Intent,
    object: ObjectId,
}

fn invalid(category: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, category)
}
fn regular(f: &File) -> io::Result<()> {
    let m = f.metadata()?;
    if !m.is_file() || is_reparse(&m) {
        return Err(invalid("owned temporary: non-regular object"));
    }
    Ok(())
}
fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}
pub(super) fn enabled(directory: &Path) -> io::Result<bool> {
    super::protocol::active(directory)
}
pub(super) fn initialize(directory: &Path) -> io::Result<()> {
    fs::create_dir(directory.join(DIRECTORY))?;
    super::prepare::sync_directory(directory)
}

/// 필드는 비공개이며 호출자에게 raw path/ID/bytes를 조립하는 API를 제공하지 않는다.
pub(super) struct GuardedTarget {
    root: ProjectDirectory,
    namespace: ProjectDirectory,
    path: PathBuf,
    name: String,
}
impl GuardedTarget {
    fn open(project: &LockedProject<'_>, op: &TransactionOperation) -> io::Result<Self> {
        Self::open_path(project, &op.target_path)
    }
    fn open_path(project: &LockedProject<'_>, target: &ProjectRelativePath) -> io::Result<Self> {
        let (namespace, name) = target
            .as_str()
            .split_once('/')
            .ok_or_else(|| invalid("owned temporary: target is not flat"))?;
        // 동일한 정규 artifact ID 경계를 쓰되 배치는 고정 파일 하나만 허용한다.
        if crate::data::repository::ArtifactSourceId::from_target(target).is_none() {
            return Err(invalid("owned temporary: noncanonical target"));
        }
        let root = ProjectDirectory::open_root(project.canonical_root())?;
        let ns = root
            .open_namespace(namespace)?
            .ok_or_else(|| invalid("owned temporary: namespace missing"))?;
        Ok(Self {
            root,
            namespace: ns,
            path: project.canonical_root().join(namespace),
            name: name.to_owned(),
        })
    }
    fn validate(&self, f: &File, name: &str) -> io::Result<()> {
        self.root.validate()?;
        regular(f)?;
        self.namespace.validate_file(f, name)?;
        if !self.namespace.same_volume(f)? {
            return Err(invalid("owned temporary: volume mismatch"));
        }
        Ok(())
    }
}
pub(super) fn guards(
    project: &LockedProject<'_>,
    directory: &Path,
    manifest: &TransactionManifest,
) -> io::Result<Vec<GuardedTarget>> {
    let journal = super::protocol::inspect(directory)?;
    journal.validate_project(project)?;
    if journal.manifest.as_ref() != Some(manifest) {
        return Err(invalid("owned temporary: in-memory manifest differs"));
    }
    if !enabled(directory)? {
        return Ok(Vec::new());
    }
    manifest
        .operations
        .iter()
        .map(|op| GuardedTarget::open(project, op))
        .collect()
}
fn intent(id: &TransactionId, op: &TransactionOperation, phase: Phase) -> Intent {
    Intent {
        version: 1,
        transaction: id.clone(),
        operation: op.index,
        phase,
        target: op.target_path.clone(),
        temporary: format!(".wb-{}-{:06}-{}.tmp", id.as_str(), op.index, phase.name()),
    }
}
fn record_path(directory: &Path, i: &Intent, receipt: bool) -> PathBuf {
    directory.join(DIRECTORY).join(format!(
        "{:06}-{}.{}.json",
        i.operation,
        i.phase.name(),
        if receipt { "acquired" } else { "intent" }
    ))
}
fn read_record(directory: &Path, path: &Path) -> io::Result<Option<Vec<u8>>> {
    match project_file::open_existing_private_file(&fs::canonicalize(directory)?, path) {
        Ok(mut f) => {
            let b = super::protocol::bounded_read(
                &mut f,
                super::protocol::RECORD_LIMIT,
                super::protocol::RecordStage::OwnershipRead,
            )?;
            Ok(Some(b))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn write_record<T: Serialize>(path: &Path, value: &T, point: &str, op: u32) -> io::Result<File> {
    (|| {
        let b =
            serde_json::to_vec(value).map_err(|_| invalid("owned temporary: journal encoding"))?;
        if b.len() as u64 > super::protocol::RECORD_LIMIT {
            return Err(invalid(
                "owned temporary: ownership record exceeds admitted bound",
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1).custom_flags(0x00200000);
        }
        let mut f = options.open(path)?;
        let half = b.len() / 2;
        f.write_all(&b[..half])?;
        checkpoint(&format!("{point}-partial"), op)?;
        f.write_all(&b[half..])?;
        f.flush()?;
        checkpoint(&format!("{point}-before-sync"), op)?;
        f.sync_all()?;
        f.seek(SeekFrom::Start(0))?;
        let actual = super::protocol::bounded_read(
            &mut f,
            super::protocol::RECORD_LIMIT,
            super::protocol::RecordStage::OwnershipVerify,
        )?;
        if actual != b {
            return Err(invalid("owned temporary: journal verification"));
        }
        Ok(f)
    })()
    .map_err(|source| {
        at(
            if point == "intent" {
                OwnedStage::Intent
            } else {
                OwnedStage::Acquired
            },
            source,
        )
    })
}
fn receipt(directory: &Path, i: &Intent) -> io::Result<Option<Receipt>> {
    let Some(b) = read_record(directory, &record_path(directory, i, true))? else {
        return Ok(None);
    };
    match serde_json::from_slice::<Receipt>(&b) {
        Ok(r) if r.intent == *i => Ok(Some(r)),
        _ => Ok(None),
    }
}
fn valid_intent(directory: &Path, i: &Intent) -> io::Result<bool> {
    Ok(read_record(directory, &record_path(directory, i, false))?
        .and_then(|b| serde_json::from_slice::<Intent>(&b).ok())
        .is_some_and(|v| v == *i))
}
fn source(
    project: &LockedProject<'_>,
    directory: &Path,
    op: &TransactionOperation,
    phase: Phase,
) -> io::Result<(File, Vec<u8>)> {
    let (path, size, hash, schema) = match phase {
        Phase::Apply => (
            op.staged_path.as_str(),
            op.staged_size,
            op.staged_sha256.as_str(),
            op.staged_schema_version,
        ),
        Phase::Restore => (
            op.backup_path
                .as_deref()
                .ok_or_else(|| invalid("owned temporary: backup missing"))?,
            op.original_size
                .ok_or_else(|| invalid("owned temporary: backup size missing"))?,
            op.original_sha256
                .as_deref()
                .ok_or_else(|| invalid("owned temporary: backup hash missing"))?,
            op.original_schema_version,
        ),
    };
    let canonical = fs::canonicalize(directory)?;
    if !canonical.starts_with(project.canonical_root()) {
        return Err(invalid("owned temporary: journal outside project"));
    }
    let mut f = project_file::open_existing_private_file(&canonical, &directory.join(path))?;
    let mut b = Vec::new();
    f.read_to_end(&mut b)?;
    if b.len() as u64 != size || sha256(&b) != hash {
        return Err(invalid("owned temporary: source bytes changed"));
    }
    if !(phase == Phase::Restore && op.original_raw) {
        let actual = super::prepare::managed_schema_version(&b)
            .map_err(|_| invalid("owned temporary: source schema invalid"))?;
        if schema.is_some_and(|s| s != actual) {
            return Err(invalid("owned temporary: source schema changed"));
        }
    }
    Ok((f, b))
}
pub(super) struct Replacement {
    file: File,
    guard: GuardedTarget,
    _source: File,
    _intent: File,
    _receipt: File,
    object: ObjectId,
    hash: String,
    size: u64,
    temporary: String,
    index: u32,
    moved: bool,
}
impl Replacement {
    pub(super) fn apply(
        project: &LockedProject<'_>,
        id: &TransactionId,
        directory: &Path,
        op: &TransactionOperation,
    ) -> io::Result<Self> {
        Self::begin(project, id, directory, op, Phase::Apply)
    }
    fn begin(
        project: &LockedProject<'_>,
        id: &TransactionId,
        directory: &Path,
        op: &TransactionOperation,
        phase: Phase,
    ) -> io::Result<Self> {
        let guard = GuardedTarget::open(project, op)?;
        let i = intent(id, op, phase);
        let (source, b) = source(project, directory, op, phase)?;
        let intent_path = record_path(directory, &i, false);
        if exists(&intent_path)? && !valid_intent(directory, &i)? {
            if phase != Phase::Restore || exists(&guard.path.join(&i.temporary))? {
                return Err(invalid("owned temporary: invalid previous intent"));
            }
            // recovery preflight가 target 무변경과 잔여물 부재를 확인한 부분 intent만 다시 쓴다.
            fs::remove_file(&intent_path)?;
        }
        let intent_pin = if !exists(&intent_path)? {
            write_record(&intent_path, &i, "intent", op.index)?
        } else {
            project_file::open_existing_private_file(&fs::canonicalize(directory)?, &intent_path)?
        };
        let acquired = record_path(directory, &i, true);
        if exists(&acquired)? {
            // 복원 retry는 이전 임시 객체를 먼저 지운 뒤에만 receipt를 갱신한다. 적용 receipt는 보존한다.
            if phase != Phase::Restore || exists(&guard.path.join(&i.temporary))? {
                return Err(invalid("owned temporary: outstanding ownership receipt"));
            }
            fs::remove_file(&acquired)?;
        }
        checkpoint("intent-durable", op.index)?;
        let mut file = native::create(&guard.path.join(&i.temporary))
            .map_err(|e| at(OwnedStage::Create, e))?;
        checkpoint("created", op.index)?;
        guard
            .validate(&file, &i.temporary)
            .map_err(|e| at(OwnedStage::Identity, e))?;
        let object = native::identity(&file).map_err(|e| at(OwnedStage::Identity, e))?;
        checkpoint("identity", op.index)?;
        let receipt_pin = write_record(
            &acquired,
            &Receipt {
                intent: i.clone(),
                object: object.clone(),
            },
            "receipt",
            op.index,
        )?;
        checkpoint("acquired", op.index)?;
        native::disposition(&file, 8).map_err(|e| at(OwnedStage::ClearDeleteOnClose, e))?;
        checkpoint("cleared", op.index)?;
        (|| {
            let half = b.len() / 2;
            file.write_all(&b[..half])?;
            checkpoint("payload-partial", op.index)?;
            file.write_all(&b[half..])?;
            file.flush()?;
            file.sync_all()
        })()
        .map_err(|e| at(OwnedStage::WritePayload, e))?;
        // 소유권 기록도 적용이 끝날 때까지 pin하여 생성 handle과 durable 증거의 연결을 유지한다.
        let mut result = Self {
            file,
            guard,
            _source: source,
            _intent: intent_pin,
            _receipt: receipt_pin,
            object,
            hash: sha256(&b),
            size: b.len() as u64,
            temporary: i.temporary,
            index: op.index,
            moved: false,
        };
        result.verify()?;
        checkpoint("payload-synced", op.index)?;
        Ok(result)
    }
    pub(super) fn rename(&mut self, replace: bool) -> io::Result<()> {
        self.guard
            .validate(&self.file, &self.temporary)
            .map_err(|e| at(OwnedStage::Verify, e))?;
        native::rename(&self.file, &self.guard.name, replace)
            .map_err(|e| at(OwnedStage::Rename, e))?;
        self.moved = true;
        checkpoint("renamed", self.index)
    }
    pub(super) fn sync(&self) -> io::Result<()> {
        self.file.sync_all().map_err(|e| at(OwnedStage::Sync, e))
    }
    pub(super) fn verify(&mut self) -> io::Result<()> {
        let name = if self.moved {
            &self.guard.name
        } else {
            &self.temporary
        };
        self.guard.validate(&self.file, name)?;
        if native::identity(&self.file)? != self.object {
            return Err(invalid("owned temporary: live identity changed"));
        }
        self.file.seek(SeekFrom::Start(0))?;
        let mut b = Vec::new();
        self.file.read_to_end(&mut b)?;
        if b.len() as u64 != self.size || sha256(&b) != self.hash {
            return Err(invalid("owned temporary: live bytes changed"));
        }
        Ok(())
    }
}
pub(super) fn restore(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    op: &TransactionOperation,
) -> io::Result<()> {
    let mut replacement = Replacement::begin(project, id, directory, op, Phase::Restore)?;
    // 검증된 backup을 복사하며 backup 자체를 최종 target의 alias로 만들지 않는다.
    let current =
        super::recovery::classify_target(&replacement.guard.path.join(&replacement.guard.name), op)
            .map_err(io::Error::other)?;
    if !matches!(
        current,
        super::recovery::TargetState::Staged | super::recovery::TargetState::Missing
    ) {
        return Err(invalid("owned temporary: restore precondition changed"));
    }
    replacement.rename(true)?;
    replacement.sync()?;
    replacement.verify()
}

pub(super) fn validate_tree(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    manifest: &TransactionManifest,
) -> io::Result<()> {
    let journal = super::protocol::inspect(directory)?;
    journal.validate_project(project)?;
    if journal.manifest.as_ref() != Some(manifest) {
        return Err(invalid("owned temporary: in-memory manifest differs"));
    }
    if !enabled(directory)? {
        return Ok(());
    }
    let mut allowed = std::collections::BTreeSet::new();
    for op in &manifest.operations {
        for phase in [Phase::Apply, Phase::Restore] {
            let i = intent(id, op, phase);
            for r in [false, true] {
                allowed.insert(record_path(directory, &i, r));
            }
        }
    }
    for entry in fs::read_dir(directory.join(DIRECTORY))? {
        let entry = entry?;
        if !allowed.contains(&entry.path()) {
            return Err(invalid("owned temporary: unknown ownership artifact"));
        }
        read_record(directory, &entry.path())?
            .ok_or_else(|| invalid("owned temporary: record disappeared"))?;
    }
    for op in &manifest.operations {
        inspect_operation(project, id, directory, op, false)?;
    }
    Ok(())
}
fn inspect_operation(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    op: &TransactionOperation,
    remove: bool,
) -> io::Result<()> {
    let guard = GuardedTarget::open(project, op)?;
    for phase in [Phase::Apply, Phase::Restore] {
        let i = intent(id, op, phase);
        let path = guard.path.join(&i.temporary);
        let has_intent = exists(&record_path(directory, &i, false))?;
        let has_receipt = exists(&record_path(directory, &i, true))?;
        let r = receipt(directory, &i)?;
        let valid = valid_intent(directory, &i)?;
        if exists(&path)? {
            let r = r.filter(|_| valid).ok_or_else(|| {
                invalid("owned temporary: residue has no valid acquired identity")
            })?;
            let file = native::open_owned(&path)?;
            guard.validate(&file, &i.temporary)?;
            if native::identity(&file)? != r.object {
                return Err(invalid(
                    "owned temporary: residue belongs to another object",
                ));
            }
            if remove {
                checkpoint("cleanup-owned", op.index)?;
                native::delete_owned(&file).map_err(|e| at(OwnedStage::Cleanup, e))?;
                drop(file);
                if exists(&path)? {
                    return Err(invalid("owned temporary: deletion incomplete"));
                }
            }
        } else if (has_intent && !valid) || (has_receipt && (r.is_none() || !valid)) {
            // 부분 기록과 손상을 구분할 수 없는 경우, 이 단계가 target을 바꾸지 않았다는
            // 기존 M1 판정까지 성립할 때만 정상 중단으로 취급한다. 객체가 남으면 위에서 차단한다.
            let state = super::recovery::classify_target(&guard.path.join(&guard.name), op)
                .map_err(io::Error::other)?;
            let safe = match phase {
                Phase::Apply => matches!(
                    state,
                    super::recovery::TargetState::Original | super::recovery::TargetState::Missing
                ),
                Phase::Restore => matches!(
                    state,
                    super::recovery::TargetState::Staged | super::recovery::TargetState::Missing
                ),
            };
            if !safe {
                return Err(invalid(
                    "owned temporary: incomplete record after possible mutation",
                ));
            }
        }
    }
    let state = super::recovery::classify_target(&guard.path.join(&guard.name), op)
        .map_err(io::Error::other)?;
    if matches!(state, super::recovery::TargetState::Staged) {
        if !valid_intent(directory, &intent(id, op, Phase::Apply))? {
            return Err(invalid("owned temporary: applied target intent missing"));
        }
        let r = receipt(directory, &intent(id, op, Phase::Apply))?
            .ok_or_else(|| invalid("owned temporary: applied target receipt missing"))?;
        let file = native::open_owned(&guard.path.join(&guard.name))?;
        guard.validate(&file, &guard.name)?;
        if native::identity(&file)? != r.object {
            return Err(invalid("owned temporary: applied target identity changed"));
        }
    }
    Ok(())
}
pub(super) fn cleanup(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    manifest: &TransactionManifest,
) -> io::Result<()> {
    let journal = super::protocol::inspect(directory)?;
    journal.validate_project(project)?;
    if journal.manifest.as_ref() != Some(manifest) {
        return Err(invalid("owned temporary: in-memory manifest differs"));
    }
    if !enabled(directory)? {
        return Ok(());
    }
    validate_tree(project, id, directory, manifest)?;
    for op in &manifest.operations {
        inspect_operation(project, id, directory, op, true)?;
    }
    Ok(())
}
pub(super) fn remove_new(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    op: &TransactionOperation,
) -> io::Result<()> {
    let guard = GuardedTarget::open(project, op)?;
    let file = native::open_owned(&guard.path.join(&guard.name))?;
    guard.validate(&file, &guard.name)?;
    let r = receipt(directory, &intent(id, op, Phase::Apply))?
        .ok_or_else(|| invalid("owned temporary: new target receipt missing"))?;
    if native::identity(&file)? != r.object {
        return Err(invalid("owned temporary: new target identity mismatch"));
    }
    native::delete_owned(&file)
}

// 환경 기반 fault hook은 test build에만 존재하고 일반 실행의 우회 분기가 되지 않는다.
pub(super) fn checkpoint(_point: &str, _index: u32) -> io::Result<()> {
    #[cfg(test)]
    {
        super::test_support::boundary::check(
            super::test_support::boundary::Point::Owned(_point),
            Some(_index),
        )?;
        // 같은 child가 실제 durable Preparing을 기록한 직후 원문만 보관한다.
        if _point == "protocol-preparing" {
            if let Some(base) = std::env::var_os("B003_M1_BASE") {
                let base = Path::new(&base);
                let mut entries = fs::read_dir(base.join("project/.worldbuild/transactions"))?;
                let journal = entries
                    .next()
                    .ok_or_else(|| invalid("test journal missing"))??
                    .path();
                fs::copy(
                    journal.join("state.json"),
                    base.join("actual-preparing.json"),
                )?;
            }
        }
        if std::env::var("B003_M1_SECOND_POINT").ok().as_deref() == Some(_point) {
            return Err(io::Error::other(
                "owned temporary: injected secondary cleanup failure",
            ));
        }
        #[cfg(windows)]
        if let Some(base) = std::env::var_os("B003_M1_BASE") {
            use std::os::windows::fs::OpenOptionsExt;
            for name in ["documents", "templates"] {
                let path = Path::new(&base).join("project").join(name);
                let error = OpenOptions::new()
                    .write(true)
                    .share_mode(7)
                    .custom_flags(0x02200000)
                    .open(path)
                    .err()
                    .and_then(|e| e.raw_os_error());
                if error != Some(32) {
                    return Err(invalid(
                        "owned temporary: test observed weakened namespace guard",
                    ));
                }
            }
        }
        if std::env::var("B003_M1_POINT").ok().as_deref() == Some(_point)
            && std::env::var("B003_M1_INDEX")
                .ok()
                .and_then(|s| s.parse().ok())
                == Some(_index)
        {
            if std::env::var("B003_M1_ACTION").ok().as_deref() == Some("fail") {
                return Err(io::Error::other(
                    "owned temporary: injected primary failure",
                ));
            }
            if let Some(ready) = std::env::var_os("B003_M1_READY") {
                let mut f = File::create(ready)?;
                f.write_all(_point.as_bytes())?;
                f.sync_all()?;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    #[repr(C)]
    struct FileId {
        volume: u64,
        index: [u8; 16],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(h: *mut c_void, c: i32, b: *mut FileId, n: u32) -> i32;
    }
    pub(super) fn identity(f: &File) -> io::Result<ObjectId> {
        let mut b = FileId {
            volume: 0,
            index: [0; 16],
        };
        if unsafe { GetFileInformationByHandleEx(f.as_raw_handle(), 18, &mut b, 24) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(ObjectId {
            volume: b.volume,
            index: b.index,
        })
    }
    pub(super) fn create(p: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .access_mode(0xc0010000)
            .share_mode(0)
            .custom_flags(0x04200000)
            .create_new(true)
            .open(p)
    }
    pub(super) fn open_owned(p: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .access_mode(0xc0010000)
            .share_mode(0)
            .custom_flags(0x00200000)
            .open(p)
    }
    pub(super) use crate::data::atomic_file::owned::{delete_owned, disposition, rename};
}
#[cfg(not(windows))]
mod native {
    use super::*;
    fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "owned temporary protocol requires a supported Windows filesystem",
        ))
    }
    pub(super) fn identity(_: &File) -> io::Result<ObjectId> {
        unsupported()
    }
    pub(super) fn create(_: &Path) -> io::Result<File> {
        unsupported()
    }
    pub(super) fn open_owned(_: &Path) -> io::Result<File> {
        unsupported()
    }
    pub(super) fn disposition(_: &File, _: u32) -> io::Result<()> {
        unsupported()
    }
    pub(super) fn delete_owned(_: &File) -> io::Result<()> {
        unsupported()
    }
    pub(super) fn rename(_: &File, _: &str, _: bool) -> io::Result<()> {
        unsupported()
    }
}

#[path = "owned_evidence.rs"]
mod evidence;
pub(super) use evidence::{verify_completed_targets, verify_original_targets, TargetEvidence};
