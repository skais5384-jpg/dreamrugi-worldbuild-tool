//! 미확정 SVN 결과와 보관 식별자를 프로젝트 밖에 영속 인계한다. 성공/해결로 바꾸지 않는다.
use crate::data::{atomic_file, edit_recovery, project_file::directory::ProjectDirectory};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PendingSvn {
    pub(crate) operation_id: String,
    pub(crate) project_fingerprint: String,
    pub(crate) requested_paths: Vec<String>,
    pub(crate) outcome: String,
    pub(crate) observed: serde_json::Value,
}
pub(crate) struct Store {
    root: PathBuf,
    records: Mutex<BTreeMap<String, PendingSvn>>,
    load_failed: bool,
    preserve_failed: AtomicBool,
}
fn directory(root: &Path) -> Result<ProjectDirectory, &'static str> {
    if !root.exists() {
        let parent = root.parent().ok_or("handoff")?;
        let parent_guard = directory(parent)?;
        match fs::create_dir(root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("handoff"),
        }
        parent_guard.validate().map_err(|_| "handoff")?;
    }
    ProjectDirectory::open_mutable_root(root).map_err(|error| {
        #[cfg(test)]
        eprintln!("handoff directory: {error:?}");
        #[cfg(not(test))]
        let _ = error;
        "handoff"
    })
}
fn read(root: &Path, name: &str) -> Result<Vec<u8>, &'static str> {
    let guard = directory(root)?;
    let mut file =
        edit_recovery::native::open(&root.join(name), false, false).map_err(|_| "handoff")?;
    guard.validate_file(&file, name).map_err(|_| "handoff")?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "handoff")?;
    if bytes.len() > 256 * 1024 {
        return Err("handoff");
    }
    Ok(bytes)
}
fn write(root: &Path, name: &str, value: &impl Serialize) -> Result<(), &'static str> {
    let guard = directory(root).inspect_err(|_| {
        #[cfg(test)]
        eprintln!("handoff write directory rejected");
    })?;
    let expected = crate::data::json::to_deterministic_json_bytes(value).map_err(|_| "handoff")?;
    if expected.len() > 256 * 1024 {
        return Err("handoff");
    }
    if root.join(name).exists() {
        read(root, name)?;
    }
    atomic_file::save_deterministic_json(&root.join(name), value).map_err(|error| {
        #[cfg(test)]
        eprintln!("handoff atomic write: {error:?}");
        #[cfg(not(test))]
        let _ = error;
        "handoff"
    })?;
    guard.validate().map_err(|_| "handoff")?;
    if read(root, name)? != expected {
        return Err("handoff");
    }
    Ok(())
}
fn valid(record: &PendingSvn) -> bool {
    uuid::Uuid::parse_str(&record.operation_id).is_ok()
        && record.project_fingerprint.len() == 64
        && record
            .project_fingerprint
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        && record.requested_paths.len() <= 4096
        && record.requested_paths.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 32768
                && !p.contains('\0')
                && !Path::new(p).is_absolute()
                && !p.split(['/', '\\']).any(|part| part == "..")
        })
        && matches!(record.outcome.as_str(), "unknown" | "partial")
}
impl Store {
    pub(crate) fn new(root: PathBuf) -> Self {
        let mut records = BTreeMap::new();
        let loaded = (|| {
            let guard = directory(&root)?;
            for (index, item) in guard.read_dir().map_err(|_| "handoff")?.enumerate() {
                if index >= 128 {
                    return Err("handoff");
                }
                let name = item
                    .map_err(|_| "handoff")?
                    .file_name()
                    .to_string_lossy()
                    .into_owned();
                let id = name.strip_suffix(".json").ok_or("handoff")?;
                uuid::Uuid::parse_str(id).map_err(|_| "handoff")?;
                let record: PendingSvn =
                    serde_json::from_slice(&read(&root, &name)?).map_err(|_| "handoff")?;
                if !valid(&record) || record.operation_id != id {
                    return Err("handoff");
                }
                records.insert(id.to_owned(), record);
            }
            Ok::<_, &'static str>(())
        })();
        Self {
            root,
            records: Mutex::new(records),
            load_failed: loaded.is_err(),
            preserve_failed: AtomicBool::new(false),
        }
    }
    pub(crate) fn preserve(&self, record: PendingSvn) -> Result<(), &'static str> {
        if !valid(&record) {
            self.preserve_failed.store(true, Ordering::Release);
            self.records
                .lock()
                .map_err(|_| "handoff")?
                .entry(record.operation_id.clone())
                .or_insert(record);
            return Err("handoff");
        }
        let mut records = self.records.lock().map_err(|_| "handoff")?;
        if records.len() >= 128 && !records.contains_key(&record.operation_id) {
            self.preserve_failed.store(true, Ordering::Release);
            records.insert(record.operation_id.clone(), record);
            return Err("handoff");
        }
        if records
            .get(&record.operation_id)
            .is_some_and(|old| old != &record)
        {
            self.preserve_failed.store(true, Ordering::Release);
            return Err("handoff");
        }
        // 쓰기 실패에도 원 관측 결과를 메모리에서 유지한다. 설치 전 flush가 반드시 재검증한다.
        records.insert(record.operation_id.clone(), record.clone());
        self.flush(&record)
    }
    pub(crate) fn admit(&self, in_flight: usize) -> Result<(), &'static str> {
        if self.load_failed
            || self.preserve_failed.load(Ordering::Acquire)
            || self
                .records
                .lock()
                .map_err(|_| "handoff")?
                .len()
                .saturating_add(in_flight)
                >= 128
        {
            return Err("handoff");
        }
        Ok(())
    }
    fn flush(&self, record: &PendingSvn) -> Result<(), &'static str> {
        let name = format!("{}.json", record.operation_id);
        if self.root.join(&name).exists() {
            let disk: PendingSvn =
                serde_json::from_slice(&read(&self.root, &name)?).map_err(|_| "handoff")?;
            if disk != *record {
                return Err("handoff");
            }
            Ok(())
        } else {
            write(&self.root, &name, record)
        }
    }
    pub(crate) fn verify(&self) -> Result<Vec<PendingSvn>, &'static str> {
        if self.load_failed || self.preserve_failed.load(Ordering::Acquire) {
            return Err("handoff");
        }
        let records = self.records.lock().map_err(|_| "handoff")?;
        for record in records.values() {
            self.flush(record)?;
        }
        let fresh = Store::new(self.root.clone());
        if fresh.load_failed || *fresh.records.lock().map_err(|_| "handoff")? != *records {
            return Err("handoff");
        }
        Ok(records.values().cloned().collect())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub(crate) schema_version: u32,
    pub(crate) candidate: String,
    pub(crate) recovery: Vec<serde_json::Value>,
    pub(crate) pending_svn: Vec<PendingSvn>,
}
pub(crate) fn preserve_snapshot(
    root: &Path,
    candidate: &str,
    recovery: Vec<edit_recovery::Entry>,
    pending_svn: Vec<PendingSvn>,
) -> Result<Snapshot, &'static str> {
    let snapshot = Snapshot {
        schema_version: 1,
        candidate: candidate.into(),
        recovery: recovery
            .iter()
            .map(|entry| serde_json::to_value(entry).map_err(|_| "handoff"))
            .collect::<Result<_, _>>()?,
        pending_svn,
    };
    write(root, "update-handoff.json", &snapshot)?;
    Ok(snapshot)
}
pub(crate) fn previous(root: &Path) -> Result<Option<Snapshot>, &'static str> {
    if !root.join("update-handoff.json").exists() {
        return Ok(None);
    }
    let snapshot: Snapshot =
        serde_json::from_slice(&read(root, "update-handoff.json")?).map_err(|_| "handoff")?;
    if snapshot.schema_version != 1 || snapshot.pending_svn.iter().any(|r| !valid(r)) {
        return Err("handoff");
    }
    Ok(Some(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(all(windows, feature = "compatibility-test"))]
    #[test]
    fn m8_owned_existing_install_handoff_components() {
        let base = PathBuf::from(std::env::var("M8_B_INSTALL_FIXTURE").unwrap());
        assert!(base.is_absolute() && base.file_name().unwrap() == "M8-B-INSTALL-TEST");
        let recovery = edit_recovery::Store::open(&base.join("data/edit-recovery")).unwrap();
        let listing = recovery.list().unwrap();
        assert!(listing.complete, "incomplete recovery listing");
        for entry in &listing.entries {
            assert!(entry.error.is_none(), "recovery entry: {:?}", entry.error);
            recovery
                .verify_handoff(
                    entry.key.as_ref().unwrap(),
                    entry.deposit_id.as_deref().unwrap(),
                )
                .unwrap();
        }
        eprintln!("owned recovery entries verified: {}", listing.entries.len());
        let pending = Store::new(base.join("data/svn/update-pending"))
            .verify()
            .unwrap();
        eprintln!("owned pending SVN records verified: {}", pending.len());
        assert!(previous(&base.join("data/settings")).is_ok());
        let probe = base.join("handoff-component-probe");
        preserve_snapshot(&probe, "component-probe-only", listing.entries, pending).unwrap();
        assert!(previous(&probe).unwrap().is_some());
    }
    #[test]
    fn m8_failed_write_keeps_observation_and_external_change_blocks_install() {
        let root = std::env::temp_dir().join(format!("m8-memory-handoff-{}", uuid::Uuid::new_v4()));
        let store = Store::new(root.clone());
        let record = PendingSvn {
            operation_id: uuid::Uuid::new_v4().to_string(),
            project_fingerprint: "b".repeat(64),
            requested_paths: vec!["documents/a.json".into()],
            outcome: "unknown".into(),
            observed: serde_json::json!({"error":"svn_commit_unverified"}),
        };
        let name = format!("{}.json", record.operation_id);
        fs::create_dir(root.join(&name)).unwrap();
        assert!(store.preserve(record.clone()).is_err());
        assert_eq!(store.records.lock().unwrap().len(), 1);
        assert!(store.verify().is_err());
        fs::remove_dir(root.join(&name)).unwrap();
        assert_eq!(store.verify().unwrap().len(), 1);
        let changed = br#"{"external":"changed"}"#;
        fs::write(root.join(&name), changed).unwrap();
        assert!(store.verify().is_err());
        assert_eq!(fs::read(root.join(&name)).unwrap(), changed);
        fs::remove_file(root.join(&name)).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn pending_result_survives_new_owner_without_becoming_success() {
        let root = std::env::temp_dir().join(format!("m8-fix-handoff-{}", uuid::Uuid::new_v4()));
        let id = uuid::Uuid::new_v4().to_string();
        let record = PendingSvn {
            operation_id: id.clone(),
            project_fingerprint: "a".repeat(64),
            requested_paths: vec!["documents/한글.json".into()],
            outcome: "unknown".into(),
            observed: serde_json::json!({"error":"svn_commit_unverified","revision":null}),
        };
        Store::new(root.clone()).preserve(record).unwrap();
        let reopened = Store::new(root.clone());
        let rows = reopened.verify().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].operation_id, id);
        assert_eq!(rows[0].outcome, "unknown");
        assert!(root.join(format!("{id}.json")).exists());
        fs::remove_file(root.join(format!("{id}.json"))).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn corrupt_or_unwritable_handoff_cannot_authorize_install() {
        let root = std::env::temp_dir().join(format!("m8-fix-bad-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let name = format!("{}.json", uuid::Uuid::new_v4());
        fs::write(root.join(&name), b"broken").unwrap();
        assert!(Store::new(root.clone()).verify().is_err());
        fs::remove_file(root.join(name)).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
