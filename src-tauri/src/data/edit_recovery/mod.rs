//! 앱 로컬 복구 저장소. 목록/본문 읽기는 프로젝트 쓰기 권한이나 인수 증거가 아니다.
mod assets;
pub(crate) mod center;
pub(crate) mod error;
mod legacy;
pub(crate) use legacy::HandoffStatus;
pub(crate) mod model;
pub(crate) mod native;
#[cfg(test)]
mod tests;
use crate::data::project_file::directory::ProjectDirectory;
use error::{Category, ErrorDto, RecoveryError, Stage};
use model::{Deposit, Key, MAX_FILE_BYTES, MAX_LIST_ENTRIES, MAX_LIST_READ_BYTES};
use serde::Serialize;
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

/// 생성자는 최종 파일 재검증/동기화가 성공한 저장소 내부에만 둔다.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Proof {
    key: Key,
    deposit_id: String,
    digest: String,
}
impl Proof {
    #[allow(dead_code, reason = "2B 명시적 owner/generation 대조 API")]
    pub(crate) fn matches(&self, key: &Key, deposit_id: &str, digest: &str) -> bool {
        self.key == *key && self.deposit_id == deposit_id && self.digest == digest
    }
}

pub(crate) struct Store {
    cursors: std::collections::BTreeMap<String, center::Cursor>,
    root: PathBuf,
    // 검증한 기존 부모와 생성한 앱 디렉터리를 붙잡아 junction/rename 경쟁을 차단한다.
    guards: Vec<ProjectDirectory>,
    _lock: File,
    #[cfg(test)]
    pub(crate) fault: Option<Stage>,
    #[cfg(test)]
    pub(crate) cleanup_fault: bool,
    #[cfg(test)]
    pub(crate) hook: Option<Box<dyn Fn(Stage) + Send>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub(crate) locator_fingerprint: String,
    pub(crate) key: Option<Key>,
    pub(crate) deposit_id: Option<String>,
    pub(crate) payload_kind: Option<String>,
    pub(crate) payload_digest: Option<String>,
    pub(crate) error: Option<ErrorDto>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Listing {
    pub(crate) entries: Vec<Entry>,
    pub(crate) complete: bool,
    pub(crate) bytes_read: usize,
    pub(crate) visited_nodes: usize,
}

impl Store {
    pub(crate) fn open(root: &Path) -> Result<Self, RecoveryError> {
        if !root.is_absolute() {
            return Err(RecoveryError::new(Category::UnsafePath, Stage::Initialize));
        }
        if root
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(RecoveryError::new(Category::UnsafePath, Stage::Initialize));
        }
        // 기존 부모를 먼저 실제 handle로 검증한다. 사용자 홈 전체의 열거 권한을 요구하지 않는다.
        // 실제 final path 대조가 중간 junction도 거부하며, 이후 생성은 붙잡은 부모 아래에서만 한다.
        let mut path = root.to_owned();
        let mut missing = Vec::new();
        let anchor = loop {
            match ProjectDirectory::open_root(&path) {
                Ok(guard) => break guard,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let name = path
                        .file_name()
                        .ok_or_else(|| RecoveryError::new(Category::UnsafePath, Stage::Initialize))?
                        .to_owned();
                    missing.push(name);
                    if !path.pop() {
                        return Err(RecoveryError::new(Category::UnsafePath, Stage::Initialize));
                    }
                }
                Err(e) => return Err(RecoveryError::io(Stage::Initialize, e)),
            }
        };
        let mut guards = vec![anchor];
        for name in missing.iter().rev() {
            path.push(name);
            guards.push(guard_directory(&path, true, Stage::Initialize)?);
        }
        let guard = guards
            .last()
            .ok_or_else(|| RecoveryError::new(Category::UnsafePath, Stage::Initialize))?;
        let lock_path = path.join(".store-lock");
        let lock = match native::open(&lock_path, true, true) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                native::open(&lock_path, false, true)
                    .map_err(|e| RecoveryError::io(Stage::Lock, e))?
            }
            Err(e) => return Err(RecoveryError::io(Stage::Lock, e)),
        };
        guard
            .validate_file(&lock, ".store-lock")
            .map_err(|e| RecoveryError::io(Stage::Lock, e))?;
        Ok(Self {
            cursors: std::collections::BTreeMap::new(),
            root: path,
            guards,
            _lock: lock,
            #[cfg(test)]
            fault: None,
            #[cfg(test)]
            cleanup_fault: false,
            #[cfg(test)]
            hook: None,
        })
    }
    fn checkpoint(&self, stage: Stage) -> Result<(), RecoveryError> {
        #[cfg(test)]
        {
            if let Some(hook) = &self.hook {
                hook(stage);
            }
            if self.fault == Some(stage) {
                return Err(RecoveryError::io(
                    stage,
                    std::io::Error::other("injected recovery failure"),
                ));
            }
        }
        let _ = stage;
        Ok(())
    }
    fn validate_root(&self) -> Result<(), RecoveryError> {
        for g in &self.guards {
            g.validate()
                .map_err(|e| RecoveryError::io(Stage::Validate, e))?;
        }
        Ok(())
    }
    fn directory(
        &self,
        key: &Key,
        create: bool,
    ) -> Result<(PathBuf, Vec<ProjectDirectory>), RecoveryError> {
        key.validate()?;
        self.validate_root()?;
        let mut path = self.root.clone();
        let mut guards = Vec::new();
        for name in [&key.project_fingerprint, &key.draft_id] {
            path.push(name);
            guards.push(guard_directory(&path, create, Stage::Validate)?);
        }
        Ok((path, guards))
    }
    /// UI의 부분 목록과 달리 한 초안의 파일명을 끝까지 확인한다. 읽기 한도에 걸리면
    /// 부분 최대값을 반환하지 않는다. 손상된 본문도 이미 점유한 세대는 재사용하지 않는다.
    pub(crate) fn latest_generation(&self, key: &Key) -> Result<u64, RecoveryError> {
        self.checkpoint(Stage::List)?;
        let (_, guards) = self.directory(key, false)?;
        let mut latest = key.generation;
        for (count, entry) in guards[1]
            .read_dir()
            .map_err(|e| RecoveryError::io(Stage::List, e))?
            .enumerate()
        {
            if count >= MAX_LIST_ENTRIES {
                return Err(RecoveryError::new(Category::TooLarge, Stage::List));
            }
            let entry = entry.map_err(|e| RecoveryError::io(Stage::List, e))?;
            if let Some(g) = generation(&entry.file_name().to_string_lossy()) {
                latest = latest.max(g);
            }
        }
        Ok(latest)
    }
    /// sessionless 초안도 호출 가능하다. 실패 시 caller의 Deposit/원 P를 소비하지 않는다.
    pub(crate) fn accept(&mut self, deposit: &Deposit) -> Result<Proof, RecoveryError> {
        self.verify_assets(deposit)?;
        self.checkpoint(Stage::Create)?;
        let (directory, guards) = self.directory(deposit.key(), true)?;
        let guard = &guards[1];
        let name = format!("{}.json", deposit.key().generation);
        let target = directory.join(&name);
        match native::open(&target, false, true) {
            Ok(file) => return self.verify(file, guard, &name, deposit),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(RecoveryError::io(Stage::Reopen, e)),
        }
        for (count, entry) in guard
            .read_dir()
            .map_err(|e| RecoveryError::io(Stage::List, e))?
            .enumerate()
        {
            if count >= MAX_LIST_ENTRIES {
                return Err(RecoveryError::new(Category::TooLarge, Stage::List));
            }
            let entry = entry.map_err(|e| RecoveryError::io(Stage::List, e))?;
            if generation(&entry.file_name().to_string_lossy())
                .is_some_and(|g| g >= deposit.key().generation)
            {
                return Err(RecoveryError::new(
                    Category::StaleGeneration,
                    Stage::Validate,
                ));
            }
        }
        let temp_name = format!(".{}.tmp", uuid::Uuid::new_v4());
        let mut file = native::open(&directory.join(&temp_name), true, true)
            .map_err(|e| RecoveryError::io(Stage::Create, e))?;
        let mut published = false;
        let result: Result<(), RecoveryError> = (|| {
            guard
                .validate_file(&file, &temp_name)
                .map_err(|e| RecoveryError::io(Stage::Validate, e))?;
            self.checkpoint(Stage::Write)?;
            file.write_all(deposit.bytes())
                .map_err(|e| RecoveryError::io(Stage::Write, e))?;
            self.checkpoint(Stage::Flush)?;
            file.flush()
                .map_err(|e| RecoveryError::io(Stage::Flush, e))?;
            self.checkpoint(Stage::Sync)?;
            file.sync_all()
                .map_err(|e| RecoveryError::io(Stage::Sync, e))?;
            self.checkpoint(Stage::Publish)?;
            // 일반 replace writer와 달리 같은 key의 기존 파일은 native no-replace가 보호한다.
            native::publish(&file, &name).map_err(|e| RecoveryError::io(Stage::Publish, e))?;
            published = true;
            guard
                .validate_file(&file, &name)
                .map_err(|e| RecoveryError::io(Stage::Reopen, e))?;
            Ok(())
        })();
        if let Err(mut error) = result {
            if published {
                error.category = Category::DurabilityUncertain;
                error.published = true;
            } else {
                #[cfg(test)]
                let cleanup = if self.cleanup_fault {
                    Err(std::io::Error::other("injected cleanup failure"))
                } else {
                    native::cleanup(&file)
                };
                #[cfg(not(test))]
                let cleanup = native::cleanup(&file);
                error.cleanup = cleanup.err();
            }
            return Err(error);
        }
        drop(file);
        let final_result = (|| {
            self.checkpoint(Stage::Reopen)?;
            let file = native::open(&target, false, true)
                .map_err(|e| RecoveryError::io(Stage::Reopen, e))?;
            self.verify(file, guard, &name, deposit)
        })();
        final_result.map_err(|mut e| {
            e.category = Category::DurabilityUncertain;
            e.published = true;
            e
        })
    }
    fn verify(
        &self,
        file: File,
        guard: &ProjectDirectory,
        name: &str,
        expected: &Deposit,
    ) -> Result<Proof, RecoveryError> {
        guard
            .validate_file(&file, name)
            .map_err(|e| RecoveryError::io(Stage::Reopen, e))?;
        let actual = read_file(&file)?;
        if actual.key() != expected.key()
            || actual.payload_digest() != expected.payload_digest()
            || actual.envelope().deposit_id != expected.envelope().deposit_id
        {
            return Err(RecoveryError::new(Category::Conflict, Stage::Revalidate));
        }
        // 존재/읽기 성공만으로 receipt를 발급하지 않는다. 재시도도 반드시 sync한다.
        self.checkpoint(Stage::Revalidate)
            .and_then(|()| {
                file.sync_all()
                    .map_err(|e| RecoveryError::io(Stage::Revalidate, e))
            })
            .map_err(|mut e| {
                e.category = Category::DurabilityUncertain;
                e.published = true;
                e
            })?;
        self.validate_root()?;
        self.verify_assets(expected)?;
        Ok(Proof {
            key: expected.key().clone(),
            deposit_id: expected.envelope().deposit_id.clone(),
            digest: expected.payload_digest().to_owned(),
        })
    }
    #[allow(dead_code, reason = "2B 본문 조회: 읽기는 receipt를 발급하지 않는다")]
    pub(crate) fn read(&self, key: &Key, deposit_id: &str) -> Result<Deposit, RecoveryError> {
        if !model::valid_id(deposit_id) {
            return Err(RecoveryError::new(Category::InvalidId, Stage::Read));
        }
        let (path, guards) = self.directory(key, false)?;
        let name = format!("{}.json", key.generation);
        let file = native::open(&path.join(&name), false, false)
            .map_err(|e| RecoveryError::io(Stage::Read, e))?;
        guards[1]
            .validate_file(&file, &name)
            .map_err(|e| RecoveryError::io(Stage::Read, e))?;
        let deposit = read_file(&file)?;
        if deposit.key() != key || deposit.envelope().deposit_id != deposit_id {
            return Err(RecoveryError::new(Category::Conflict, Stage::Read));
        }
        Ok(deposit)
    }
    /// 설치 인계의 읽기 증거다. receipt 발급/채택/삭제 없이 참조 자산까지 다시 검사한다.
    pub(crate) fn verify_handoff(&self, key: &Key, deposit_id: &str) -> Result<(), RecoveryError> {
        let deposit = self.read(key, deposit_id)?;
        self.verify_assets(&deposit)
    }
    #[allow(dead_code, reason = "2B 응답 유실 뒤 key/deposit/digest 재검증 API")]
    pub(crate) fn revalidate(
        &mut self,
        key: &Key,
        deposit_id: &str,
        digest: &str,
    ) -> Result<Proof, RecoveryError> {
        let deposit = self.read(key, deposit_id)?;
        if deposit.payload_digest() != digest {
            return Err(RecoveryError::new(Category::Conflict, Stage::Revalidate));
        }
        self.accept(&deposit)
    }
    #[allow(dead_code, reason = "2B Recovery Center 목록 API")]
    pub(crate) fn list(&self) -> Result<Listing, RecoveryError> {
        self.validate_root()?;
        let mut listing = Listing {
            entries: Vec::new(),
            complete: true,
            bytes_read: 0,
            visited_nodes: 0,
        };
        self.walk(&self.root, &[], &mut listing)?;
        Ok(listing)
    }
    fn walk(
        &self,
        path: &Path,
        parts: &[String],
        listing: &mut Listing,
    ) -> Result<(), RecoveryError> {
        let guard =
            ProjectDirectory::open_root(path).map_err(|e| RecoveryError::io(Stage::List, e))?;
        for entry in guard
            .read_dir()
            .map_err(|e| RecoveryError::io(Stage::List, e))?
        {
            if listing.visited_nodes >= MAX_LIST_ENTRIES
                || listing.bytes_read >= MAX_LIST_READ_BYTES
            {
                listing.complete = false;
                break;
            }
            let entry = entry.map_err(|e| RecoveryError::io(Stage::List, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if parts.is_empty() && name == ".store-lock" {
                continue;
            }
            listing.visited_nodes += 1;
            let mut next = parts.to_vec();
            next.push(name.clone());
            let mut row = Entry {
                locator_fingerprint: model::digest(next.join("/").as_bytes()),
                key: None,
                deposit_id: None,
                payload_kind: None,
                payload_digest: None,
                error: None,
            };
            let result = (|| {
                if parts.len() == 2 && name == "assets" {
                    // Assets belong to this draft, not to the generation-file list.
                    // Validate the namespace here; handoff validates every referenced byte.
                    ProjectDirectory::open_root(&entry.path())
                        .map_err(|e| RecoveryError::io(Stage::List, e))?
                        .validate()
                        .map_err(|e| RecoveryError::io(Stage::List, e))?;
                    return Ok(false);
                }
                if parts.len() < 2 {
                    let valid = if parts.is_empty() {
                        model::valid_digest(&name)
                    } else {
                        model::valid_id(&name)
                    };
                    if !valid {
                        return Err(RecoveryError::new(Category::InvalidId, Stage::List));
                    }
                    // 빈 디렉터리도 방문 수에 포함해 목록 검사를 유한하게 제한한다.
                    self.walk(&entry.path(), &next, listing)?;
                    return Ok(false);
                }
                let generation = generation(&name)
                    .ok_or_else(|| RecoveryError::new(Category::InvalidId, Stage::List))?;
                let key = Key {
                    project_fingerprint: parts[0].clone(),
                    draft_id: parts[1].clone(),
                    generation,
                };
                row.key = Some(key.clone());
                let file = native::open(&entry.path(), false, false)
                    .map_err(|e| RecoveryError::io(Stage::Read, e))?;
                guard
                    .validate_file(&file, &name)
                    .map_err(|e| RecoveryError::io(Stage::Read, e))?;
                let size = file
                    .metadata()
                    .map_err(|e| RecoveryError::io(Stage::Read, e))?
                    .len();
                if size > MAX_FILE_BYTES as u64 {
                    return Err(RecoveryError::new(Category::TooLarge, Stage::Read));
                }
                if size as usize > MAX_LIST_READ_BYTES - listing.bytes_read {
                    listing.complete = false;
                    return Err(RecoveryError::new(Category::TooLarge, Stage::List));
                }
                listing.bytes_read += size as usize;
                let deposit = read_file(&file)?;
                if deposit.key() != &key {
                    return Err(RecoveryError::new(Category::InvalidEnvelope, Stage::Read));
                }
                row.deposit_id = Some(deposit.envelope().deposit_id.clone());
                row.payload_digest = Some(deposit.payload_digest().to_owned());
                row.payload_kind = Some(deposit.envelope().draft.kind().to_owned());
                Ok(true)
            })();
            match result {
                Ok(false) => (),
                Ok(true) => listing.entries.push(row),
                Err(e) => {
                    row.error = Some(e.dto());
                    listing.entries.push(row);
                }
            }
        }
        Ok(())
    }
}
fn generation(name: &str) -> Option<u64> {
    let text = name.strip_suffix(".json")?;
    let n = text.parse::<u64>().ok()?;
    (n > 0 && n.to_string() == text).then_some(n)
}
fn guard_directory(
    path: &Path,
    create: bool,
    stage: Stage,
) -> Result<ProjectDirectory, RecoveryError> {
    let open = ProjectDirectory::open_root;
    match open(path) {
        Ok(guard) => return Ok(guard),
        Err(e) if create && e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(RecoveryError::io(stage, e)),
    }
    match fs::create_dir(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(RecoveryError::io(stage, e)),
    }
    open(path).map_err(|e| RecoveryError::io(stage, e))
}

fn read_file(file: &File) -> Result<Deposit, RecoveryError> {
    let size = file
        .metadata()
        .map_err(|e| RecoveryError::io(Stage::Read, e))?
        .len();
    if size > MAX_FILE_BYTES as u64 {
        return Err(RecoveryError::new(Category::TooLarge, Stage::Read));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| RecoveryError::io(Stage::Read, e))?;
    Deposit::decode(&bytes)
}

/// 앱 수명 owner는 초기화 실패를 보관한다. 다음 명시적 연결 시도에서만 다시 초기화한다.
pub(crate) struct Owner {
    root: PathBuf,
    legacy_root: Option<PathBuf>,
    handoff_gate: Mutex<()>,
    state: Mutex<OwnerState>,
}
struct OwnerState {
    store: Option<Arc<Mutex<Store>>>,
    error: Option<Arc<RecoveryError>>,
    handoff: HandoffStatus,
}
impl Owner {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self::with_legacy(root, None)
    }
    pub(crate) fn with_legacy(root: PathBuf, legacy_root: Option<PathBuf>) -> Self {
        Self {
            root,
            legacy_root,
            handoff_gate: Mutex::new(()),
            state: Mutex::new(OwnerState {
                store: None,
                error: None,
                handoff: HandoffStatus::pending(),
            }),
        }
    }
    pub(crate) fn handoff_status(&self) -> HandoffStatus {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .handoff
            .clone()
    }
    /// Called synchronously from Tauri setup before the window can close or the
    /// recovery center can be opened. A later explicit retry uses the same gate.
    pub(crate) fn handoff_legacy(
        &self,
    ) -> Result<Option<legacy::ImportSummary>, Arc<RecoveryError>> {
        let _gate = self
            .handoff_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = (|| {
            let Some(root) = self.legacy_root.as_deref() else {
                return Ok(None);
            };
            match fs::symlink_metadata(root) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(Arc::new(RecoveryError::io(Stage::Initialize, error))),
                Ok(_) => (),
            }
            let store = self.connect()?;
            let mut store = store
                .lock()
                .map_err(|_| Arc::new(RecoveryError::new(Category::Unavailable, Stage::Lock)))?;
            store.import_legacy(root).map_err(Arc::new)
        })();
        let status = match &result {
            Ok(Some(summary)) => HandoffStatus {
                state: if summary.needs_attention == 0 {
                    "preserved"
                } else {
                    "needs_attention"
                },
                summary: Some(summary.clone()),
                error: None,
            },
            Ok(None) => HandoffStatus {
                state: if self.legacy_root.is_some() {
                    "absent"
                } else {
                    "not_applicable"
                },
                summary: None,
                error: None,
            },
            Err(error) => HandoffStatus {
                state: "failed",
                summary: None,
                error: Some(error.dto()),
            },
        };
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .handoff = status;
        result
    }
    #[cfg(test)]
    pub(crate) fn release_fixture(&self) {
        // fixture 정리 전에 app owner만 해제한다. worker가 가진 owner는 해제하지 않는다.
        self.state.lock().unwrap().store = None;
    }
    pub(crate) fn connect(&self) -> Result<Arc<Mutex<Store>>, Arc<RecoveryError>> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Arc::new(RecoveryError::new(Category::Unavailable, Stage::Initialize)))?;
        if let Some(store) = &state.store {
            return Ok(store.clone());
        }
        match Store::open(&self.root) {
            Ok(store) => {
                let store = Arc::new(Mutex::new(store));
                state.store = Some(store.clone());
                state.error = None;
                Ok(store)
            }
            Err(e) => {
                let e = Arc::new(e);
                state.error = Some(e.clone());
                Err(e)
            }
        }
    }
}
