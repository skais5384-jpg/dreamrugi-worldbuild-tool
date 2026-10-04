//! One-time legacy format-history transition. No canonical files are rewritten.
use super::*;
use crate::data::{
    artifact, project_backup,
    repository::{versions, ArtifactRepository, ArtifactSourceId},
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Seek, SeekFrom};
use transition::{owned_directory, read_exact, stage_exact};
pub(crate) const DIRECTORY: &str = "format-transition";
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Leaf {
    path: String,
    template: Option<String>,
    timestamp: String,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    schema: u32,
    fingerprint: String,
    backup: serde_json::Value,
    manifest: String,
    leaves: Vec<Leaf>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Completion {
    schema: u32,
    digest: String,
    receipt: Receipt,
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn target(relative: &str) -> io::Result<ArtifactSourceId> {
    let parts = relative.split('/').collect::<Vec<_>>();
    if parts.len() != 4 || parts[0] != ".worldbuild" || parts[1] != "format-history" {
        return Err(invalid("invalid format history path"));
    }
    let id = parts[2]
        .strip_prefix("template-")
        .map(|id| id.parse().map(ArtifactSourceId::Template))
        .or_else(|| {
            parts[2]
                .strip_prefix("document-")
                .map(|id| id.parse().map(ArtifactSourceId::Document))
        })
        .ok_or_else(|| invalid("invalid format history target"))?
        .map_err(io::Error::other)?;
    let expected = match id {
        ArtifactSourceId::Template(id) => format!("template-{id}"),
        ArtifactSourceId::Document(id) => format!("document-{id}"),
        _ => return Err(invalid("invalid format history target")),
    };
    if expected != parts[2]
        || !parts[3]
            .strip_suffix(".json")
            .is_some_and(model::valid_digest)
    {
        return Err(invalid("noncanonical format history path"));
    }
    Ok(id)
}
fn decode(id: ArtifactSourceId, bytes: &[u8]) -> io::Result<String> {
    match id {
        ArtifactSourceId::Template(id) => {
            let value = artifact::decode_template(bytes).map_err(io::Error::other)?;
            if value.template_id() != id {
                return Err(invalid("format template identity mismatch"));
            }
            Ok(value.updated_at_utc().into())
        }
        ArtifactSourceId::Document(id) => {
            let value = artifact::decode_document(bytes).map_err(io::Error::other)?;
            if value.document_id() != id {
                return Err(invalid("format document identity mismatch"));
            }
            Ok(value.updated_at_utc().into())
        }
        _ => Err(invalid("invalid format artifact")),
    }
}
fn held_bytes(files: &mut BTreeMap<String, File>, path: &str) -> io::Result<Vec<u8>> {
    let file = files
        .get_mut(path)
        .ok_or_else(|| invalid("format backup leaf missing"))?;
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    Read::by_ref(file)
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(invalid("format history exceeds bound"));
    }
    Ok(bytes)
}
fn hold(
    backup: &project_backup::BackupRow,
    inventory: &project_backup::VerifiedInventory,
) -> io::Result<(Vec<ProjectDirectory>, BTreeMap<String, File>)> {
    let payload = Path::new(&backup.locator).join("payload");
    let mut parents = BTreeSet::new();
    parents.insert(payload.clone());
    let names = inventory
        .files
        .keys()
        .filter(|path| {
            path.starts_with(".worldbuild/format-history/") || path.starts_with("templates/")
        })
        .cloned()
        .collect::<Vec<_>>();
    for name in &names {
        let mut parent = payload.join(name).parent().map(Path::to_path_buf);
        while let Some(path) = parent {
            if path == payload {
                break;
            }
            if !path.starts_with(&payload) {
                return Err(invalid("format backup boundary"));
            }
            parents.insert(path.clone());
            parent = path.parent().map(Path::to_path_buf);
        }
    }
    let mut guards = Vec::new();
    for path in parents {
        guards.push(ProjectDirectory::open_root(&path)?);
    }
    let mut files = BTreeMap::new();
    for name in names {
        let path = payload.join(&name);
        let file = native::open(&path, false, false)?;
        let guard = ProjectDirectory::open_root(
            path.parent().ok_or_else(|| invalid("format leaf parent"))?,
        )?;
        guard.validate_file(
            &file,
            path.file_name()
                .and_then(|v| v.to_str())
                .ok_or_else(|| invalid("format leaf name"))?,
        )?;
        files.insert(name.clone(), file);
        let bytes = held_bytes(&mut files, &name)?;
        if inventory.files.get(&name) != Some(&(bytes.len() as u64, model::digest(&bytes))) {
            return Err(invalid("format backup leaf differs"));
        }
    }
    for guard in &guards {
        guard.validate()?;
    }
    Ok((guards, files))
}
fn legacy_leaves(root: &Path) -> io::Result<Vec<String>> {
    let base = root.join(".worldbuild/format-history");
    let guard = match ProjectDirectory::open_root(&base) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut leaves = Vec::new();
    for entry in guard.read_dir()? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| invalid("format namespace name"))?;
        let child = ProjectDirectory::open_root(&base.join(&name))?;
        for entry in child.read_dir()? {
            let file = entry?
                .file_name()
                .into_string()
                .map_err(|_| invalid("format leaf name"))?;
            let relative = format!(".worldbuild/format-history/{name}/{file}");
            target(&relative)?;
            leaves.push(relative);
            if leaves.len() > 100_000 {
                return Err(invalid("format history count exceeds bound"));
            }
        }
    }
    leaves.sort();
    guard.validate()?;
    Ok(leaves)
}
type DefinitionIndex =
    BTreeMap<(artifact::TemplateId, artifact::TemplateRevision), Option<(String, String)>>;
fn definition_index(files: &mut BTreeMap<String, File>) -> io::Result<DefinitionIndex> {
    let mut definitions = BTreeMap::new();
    for path in files.keys().cloned().collect::<Vec<_>>() {
        if path.starts_with("templates/")
            || path.starts_with(".worldbuild/format-history/template-")
        {
            let bytes = held_bytes(files, &path)?;
            if let Ok(value) = artifact::decode_template(&bytes) {
                let candidate = (path, model::digest(&bytes));
                definitions
                    .entry((value.template_id(), value.revision()))
                    .and_modify(|existing: &mut Option<(String, String)>| {
                        if existing.as_ref().map(|(_, digest)| digest) != Some(&candidate.1) {
                            *existing = None;
                        }
                    })
                    .or_insert(Some(candidate));
            }
        }
    }
    Ok(definitions)
}
fn preflight_interpretations(
    receipt: &Receipt,
    files: &mut BTreeMap<String, File>,
) -> io::Result<()> {
    let definitions = definition_index(files)?;
    for leaf in &receipt.leaves {
        if let Some(path) = &leaf.template {
            if !matches!(target(&leaf.path)?, ArtifactSourceId::Document(_)) {
                return Err(invalid("template interpretation on non-document leaf"));
            }
            let bytes = held_bytes(files, &leaf.path)?;
            let doc = artifact::decode_document(&bytes).map_err(io::Error::other)?;
            let Some(Some((_, digest))) =
                definitions.get(&(doc.template_id(), doc.template_revision()))
            else {
                return Err(invalid(
                    "stored format interpretation unavailable or ambiguous",
                ));
            };
            let definition = held_bytes(files, path)?;
            let def = artifact::decode_template(&definition).map_err(io::Error::other)?;
            if model::digest(&definition) != *digest
                || def.template_id() != doc.template_id()
                || def.revision() != doc.template_revision()
            {
                return Err(invalid("stored format interpretation differs"));
            }
        }
    }
    Ok(())
}
fn initial_receipt(
    repository: &ArtifactRepository<'_, '_>,
    storage: &Path,
    names: &[String],
) -> io::Result<Receipt> {
    let project = repository.write_project();
    let private = project.canonical_root().join(".worldbuild");
    let (stage, guard) = owned_directory(&private, DIRECTORY)?;
    let id = transition::backup_intent(&stage, &guard, project.fingerprint())?;
    let backup = project_backup::create_transition_backup(
        project.canonical_root(),
        project.fingerprint(),
        storage,
        "이전 버전 자료 전환 전",
        &id,
    )
    .map_err(io::Error::other)?;
    let inventory = project_backup::verified_inventory(&backup).map_err(io::Error::other)?;
    let (_guards, mut files) = hold(&backup, &inventory)?;
    let definitions = definition_index(&mut files)?;
    let mut leaves = Vec::new();
    for path in names {
        let id = target(path)?;
        let bytes = held_bytes(&mut files, path)?;
        let hash = path
            .rsplit('/')
            .next()
            .and_then(|v| v.strip_suffix(".json"))
            .ok_or_else(|| invalid("format digest missing"))?;
        if model::digest(&bytes) != hash {
            return Err(invalid("legacy format digest mismatch"));
        }
        let timestamp = decode(id, &bytes)?;
        let template = if matches!(id, ArtifactSourceId::Document(_)) {
            let doc = artifact::decode_document(&bytes).map_err(io::Error::other)?;
            definitions
                .get(&(doc.template_id(), doc.template_revision()))
                .and_then(|candidate| candidate.as_ref().map(|(path, _)| path.clone()))
        } else {
            None
        };
        leaves.push(Leaf {
            path: path.clone(),
            template,
            timestamp,
        });
    }
    leaves.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.path.cmp(&b.path)));
    Ok(Receipt {
        schema: 1,
        fingerprint: project.fingerprint().into(),
        backup: serde_json::to_value(backup).map_err(io::Error::other)?,
        manifest: inventory.manifest_digest.clone(),
        leaves,
    })
}
impl Store {
    pub(crate) fn transition_format_history(
        &self,
        repository: &ArtifactRepository<'_, '_>,
    ) -> io::Result<()> {
        let project = repository.write_project();
        let root = project.canonical_root();
        let profile = self
            .root
            .parent()
            .ok_or_else(|| invalid("format backup profile missing"))?;
        let (storage, _storage) = owned_directory(profile, "transition-backup-storage")?;
        let private = root.join(".worldbuild");
        let (_private, _private_guard) = owned_directory(root, ".worldbuild")?;
        let stage = private.join(DIRECTORY);
        let existing = match ProjectDirectory::open_root(&stage) {
            Ok(guard) => Some(guard),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if let Some(guard) = &existing {
            if guard.read_dir()?.next().is_none() && legacy_leaves(root)?.is_empty() {
                return Ok(());
            }
        }
        let (receipt, completed) = if let Some(guard) = existing {
            let completion = match read_exact(&stage, &guard, "completed.json") {
                Ok(bytes) => {
                    let value: Completion =
                        serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                    if value.schema != 1
                        || value.digest
                            != model::digest(
                                &serde_json::to_vec(&value.receipt).map_err(io::Error::other)?,
                            )
                    {
                        return Err(invalid("format completion differs"));
                    }
                    Some(value)
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            let bytes = match read_exact(&stage, &guard, "transition.json") {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == io::ErrorKind::NotFound => match &completion {
                    Some(v) => serde_json::to_vec(&v.receipt).map_err(io::Error::other)?,
                    None => {
                        let names = legacy_leaves(root)?;
                        if names.is_empty() {
                            return Err(invalid("incomplete empty format transition"));
                        }
                        let value = initial_receipt(repository, &storage, &names)?;
                        let bytes = serde_json::to_vec(&value).map_err(io::Error::other)?;
                        stage_exact(&stage, &guard, "transition.json", &bytes)?;
                        bytes
                    }
                },
                Err(e) => return Err(e),
            };
            let receipt: Receipt = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if completion
                .as_ref()
                .is_some_and(|v| serde_json::to_vec(&v.receipt).ok() != Some(bytes.clone()))
            {
                return Err(invalid("format receipt completion mismatch"));
            }
            (receipt, completion.is_some())
        } else {
            let names = legacy_leaves(root)?;
            if names.is_empty() {
                return Ok(());
            }
            let value = initial_receipt(repository, &storage, &names)?;
            let (_, guard) = owned_directory(&private, DIRECTORY)?;
            stage_exact(
                &stage,
                &guard,
                "transition.json",
                &serde_json::to_vec(&value).map_err(io::Error::other)?,
            )?;
            (value, false)
        };
        if receipt.schema != 1
            || receipt.fingerprint != project.fingerprint()
            || receipt.leaves.is_empty()
            || receipt.leaves.len() > 100_000
            || !model::valid_digest(&receipt.manifest)
        {
            return Err(invalid("format receipt invalid"));
        }
        let locator = receipt
            .backup
            .get("locator")
            .and_then(|v| v.as_str())
            .ok_or_else(|| invalid("format backup locator missing"))?;
        let actual = fs::canonicalize(locator)?;
        let parent = fs::canonicalize(
            storage
                .join("worldbuild-backups")
                .join(project.fingerprint()),
        )?;
        if actual.parent() != Some(parent.as_path()) {
            return Err(invalid("format backup outside owned storage"));
        }
        let backup = project_backup::inspect_backup(&actual).map_err(io::Error::other)?;
        if backup.status != "verified"
            || serde_json::to_value(&backup).map_err(io::Error::other)? != receipt.backup
        {
            return Err(invalid("format backup receipt mismatch"));
        }
        let inventory = project_backup::verified_inventory(&backup).map_err(io::Error::other)?;
        if inventory.manifest_digest != receipt.manifest {
            return Err(invalid("format backup manifest changed"));
        }
        let (_held_guards, mut held) = hold(&backup, &inventory)?;
        // Validate every stored interpretation before importing or retiring any leaf.
        preflight_interpretations(&receipt, &mut held)?;
        let mut seen = BTreeSet::new();
        let stage_guard = ProjectDirectory::open_root(&stage)?;
        let mut allowed = BTreeSet::from([
            "transition.json".to_string(),
            "completed.json".to_string(),
            "backup-intent.json".to_string(),
        ]);
        for (index, leaf) in receipt.leaves.iter().enumerate() {
            let id = target(&leaf.path)?;
            if !seen.insert(leaf.path.clone()) {
                return Err(invalid("duplicate format receipt leaf"));
            }
            let bytes = held_bytes(&mut held, &leaf.path)?;
            if decode(id, &bytes)? != leaf.timestamp
                || model::digest(&bytes)
                    != leaf
                        .path
                        .rsplit('/')
                        .next()
                        .and_then(|v| v.strip_suffix(".json"))
                        .ok_or_else(|| invalid("format digest missing"))?
            {
                return Err(invalid("format preserved raw mismatch"));
            }
            let template = leaf
                .template
                .as_ref()
                .map(|path| {
                    held_bytes(&mut held, path)
                        .and_then(|bytes| String::from_utf8(bytes).map_err(io::Error::other))
                })
                .transpose()?;
            if let Some(template) = &template {
                let doc = artifact::decode_document(&bytes).map_err(io::Error::other)?;
                let def =
                    artifact::decode_template(template.as_bytes()).map_err(io::Error::other)?;
                if doc.template_id() != def.template_id()
                    || doc.template_revision() != def.revision()
                {
                    return Err(invalid("legacy interpretation revision mismatch"));
                }
            }
            let intent = format!("intent-{index}.json");
            let done = format!("done-{index}.json");
            allowed.insert(intent.clone());
            allowed.insert(done.clone());
            if completed {
                continue;
            }
            let planned = match read_exact(&stage, &stage_guard, &intent) {
                Ok(bytes) => serde_json::from_slice::<u64>(&bytes).map_err(io::Error::other)?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    let number = versions::number_for_preserved(repository, id, &bytes, &template)?;
                    stage_exact(
                        &stage,
                        &stage_guard,
                        &intent,
                        &serde_json::to_vec(&number).map_err(io::Error::other)?,
                    )?;
                    number
                }
                Err(e) => return Err(e),
            };
            match read_exact(&stage, &stage_guard, &done) {
                Ok(bytes) => {
                    if serde_json::from_slice::<u64>(&bytes).map_err(io::Error::other)? != planned {
                        return Err(invalid("format import receipt differs"));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    let actual = versions::import_legacy_at(
                        repository,
                        id,
                        &bytes,
                        template,
                        &leaf.timestamp,
                        planned,
                    )?;
                    checkpoint("after-import")?;
                    if actual != planned {
                        return Err(invalid("format import number differs"));
                    }
                    stage_exact(
                        &stage,
                        &stage_guard,
                        &done,
                        &serde_json::to_vec(&actual).map_err(io::Error::other)?,
                    )?;
                }
                Err(e) => return Err(e),
            };
        }
        let expected = inventory
            .files
            .keys()
            .filter(|v| v.starts_with(".worldbuild/format-history/"))
            .cloned()
            .collect::<BTreeSet<_>>();
        if expected != seen {
            return Err(invalid("format inventory coverage differs"));
        }
        if !completed {
            let ids = receipt
                .leaves
                .iter()
                .map(|leaf| target(&leaf.path))
                .collect::<io::Result<BTreeSet<_>>>()?;
            for id in ids {
                let current = match id {
                    ArtifactSourceId::Template(id) => repository
                        .load_template(id)
                        .map(|v| v.artifact().updated_at_utc().to_string()),
                    ArtifactSourceId::Document(id) => repository
                        .load_document(id)
                        .map(|v| v.artifact().updated_at_utc().to_string()),
                    _ => return Err(invalid("format baseline target")),
                };
                match current {
                    Ok(time) => {
                        versions::confirm(repository, id, &time)?;
                    }
                    Err(_) => { /* Missing/corrupt canonical remains discoverable from imported history. */
                    }
                }
            }
            let marker = Completion {
                schema: 1,
                digest: model::digest(&serde_json::to_vec(&receipt).map_err(io::Error::other)?),
                receipt: receipt.clone(),
            };
            stage_exact(
                &stage,
                &stage_guard,
                "completed.json",
                &serde_json::to_vec(&marker).map_err(io::Error::other)?,
            )?;
        }
        // Admit every remaining local leaf before deleting any. The entire exact
        // old history stays held in the independent whole backup until retirement.
        let actual_leaves = legacy_leaves(root)?;
        let mut retire = Vec::new();
        for relative in actual_leaves {
            if !seen.contains(&relative) {
                return Err(invalid("new format history appeared during transition"));
            }
            let path = root.join(&relative);
            let guard = ProjectDirectory::open_root(
                path.parent()
                    .ok_or_else(|| invalid("format retirement parent"))?,
            )?;
            let mut file = native::open_for_discard(&path)?;
            guard.validate_file(
                &file,
                path.file_name()
                    .and_then(|v| v.to_str())
                    .ok_or_else(|| invalid("format retirement name"))?,
            )?;
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take(64 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            if inventory.files.get(&relative) != Some(&(bytes.len() as u64, model::digest(&bytes)))
            {
                return Err(invalid("format local leaf changed before retirement"));
            }
            retire.push((guard, file));
        }
        for (index, (guard, file)) in retire.iter().enumerate() {
            guard.validate()?;
            native::cleanup(file)?;
            if index == 0 {
                checkpoint("after-first-retire")?;
            }
        }
        drop(retire);
        // Empty namespace directories are harmless; no raw history or new writes
        // remain in this former format-specific history.
        let mut remove = Vec::new();
        for entry in stage_guard.read_dir()? {
            let name = entry?
                .file_name()
                .into_string()
                .map_err(|_| invalid("format transition name"))?;
            if !allowed.contains(&name) {
                return Err(invalid("unknown format transition file"));
            }
            let file = native::open_for_discard(&stage.join(&name))?;
            stage_guard.validate_file(&file, &name)?;
            remove.push((name, file));
        }
        remove.sort_by_key(|(name, _)| {
            if name == "completed.json" {
                2
            } else if name == "transition.json" {
                1
            } else {
                0
            }
        });
        for (_, file) in remove {
            native::cleanup(&file)?;
        }
        drop(stage_guard);
        ProjectDirectory::open_owned_root(&stage)?.delete_owned()?;
        // A completed transition leaves no workflow namespace behind.
        Ok(())
    }
}

#[cfg(test)]
thread_local! {static FAULT: std::cell::Cell<Option<&'static str>>=const{std::cell::Cell::new(None)};}
fn checkpoint(_point: &'static str) -> io::Result<()> {
    #[cfg(test)]
    if FAULT.with(|f| {
        if f.get() == Some(_point) {
            f.set(None);
            true
        } else {
            false
        }
    }) {
        return Err(io::Error::other(
            "controlled format transition interruption",
        ));
    }
    Ok(())
}
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::data::project_runtime::ProjectRuntime;
    use serde_json::json;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "worldbuild-format-transition-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn template(id: &str, revision: u64, name: &str) -> Vec<u8> {
        let raw=serde_json::to_vec(&json!({"artifactType":"template","schemaVersion":1,"templateId":id,"revision":revision,"name":name,"lifecycle":"active","presentation":{},"fieldOrder":[],"fields":{},"createdAtUtc":"2026-10-04T00:00:00.000Z","updatedAtUtc":format!("2026-10-04T00:00:{:02}.000Z",revision),"futureSynthetic":"exact historical metadata"})).unwrap();
        artifact::decode_template(&raw).unwrap();
        raw
    }
    #[test]
    fn legacy_format_transition_is_bounded_and_resumes_import_and_cleanup_without_duplicate_backup()
    {
        for fault in ["after-import", "after-first-retire"] {
            let fixture = Fixture::new();
            let root = fixture.0.join("project");
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("templates")).unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let history = root.join(format!(".worldbuild/format-history/template-{id}"));
            fs::create_dir_all(&history).unwrap();
            let mut originals = Vec::new();
            for revision in 1..=14 {
                let bytes = template(&id, revision, &format!("과거 내용 {revision}"));
                let name = format!("{}.json", model::digest(&bytes));
                fs::write(history.join(&name), &bytes).unwrap();
                originals.push((name, bytes));
            }
            fs::write(
                root.join(format!("templates/{id}.json")),
                template(&id, 15, "현재 내용"),
            )
            .unwrap();
            let mut runtime = ProjectRuntime::acquire(&root, &fixture.0.join("locks")).unwrap();
            runtime.recover().unwrap();
            let ready = runtime.ready().unwrap();
            let repository = ArtifactRepository::new(&ready).unwrap();
            let store = Store::open(&fixture.0.join("edit-recovery")).unwrap();
            FAULT.with(|f| f.set(Some(fault)));
            assert!(store.transition_format_history(&repository).is_err());
            let locator = project_backup::list_backups(
                repository.write_project().fingerprint(),
                &fixture.0.join("transition-backup-storage"),
                None,
            )
            .unwrap()
            .backups[0]
                .locator
                .clone();
            for (name, bytes) in &originals {
                assert_eq!(
                    &fs::read(Path::new(&locator).join(format!(
                        "payload/.worldbuild/format-history/template-{id}/{name}"
                    )))
                    .unwrap(),
                    bytes
                );
            }
            store.transition_format_history(&repository).unwrap();
            let target = ArtifactSourceId::Template(id.parse().unwrap());
            let versions = versions::list(&repository, target).unwrap();
            assert_eq!(versions.len(), 10);
            assert_eq!(versions[0].version, "15");
            let before = versions
                .iter()
                .map(|v| v.version.clone())
                .collect::<Vec<_>>();
            for _ in 0..3 {
                store.transition_format_history(&repository).unwrap();
                assert_eq!(
                    versions::list(&repository, target)
                        .unwrap()
                        .iter()
                        .map(|v| v.version.clone())
                        .collect::<Vec<_>>(),
                    before
                );
            }
            assert_eq!(fs::read_dir(&history).unwrap().count(), 0);
            assert!(!root.join(".worldbuild/format-transition").exists());
            assert_eq!(
                project_backup::list_backups(
                    repository.write_project().fingerprint(),
                    &fixture.0.join("transition-backup-storage"),
                    None
                )
                .unwrap()
                .backups
                .len(),
                1
            );
            for number in [6, 15] {
                let (bytes, _) = versions::snapshot(&repository, target, number).unwrap();
                let value = artifact::decode_template(&bytes).unwrap();
                assert_eq!(
                    value.name(),
                    if number == 15 {
                        "현재 내용".to_string()
                    } else {
                        format!("과거 내용 {number}")
                    }
                );
            }
            drop(store);
            drop(repository);
            drop(ready);
            drop(runtime);
        }
    }
    #[test]
    fn repeated_invalid_format_record_reuses_one_preselected_backup_and_never_deletes_source() {
        let fixture = Fixture::new();
        let root = fixture.0.join("project");
        fs::create_dir(&root).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let bytes = b"{controlled corrupt artifact";
        let path = root.join(format!(
            ".worldbuild/format-history/template-{id}/{}.json",
            model::digest(bytes)
        ));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let mut runtime = ProjectRuntime::acquire(&root, &fixture.0.join("locks")).unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let store = Store::open(&fixture.0.join("edit-recovery")).unwrap();
        for _ in 0..3 {
            assert!(store.transition_format_history(&repository).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(
                project_backup::list_backups(
                    repository.write_project().fingerprint(),
                    &fixture.0.join("transition-backup-storage"),
                    None
                )
                .unwrap()
                .backups
                .len(),
                1
            );
        }
        drop(store);
        drop(repository);
        drop(ready);
        drop(runtime);
    }
    #[test]
    fn format_import_reuses_exact_existing_number_and_current_baseline_replaces_unavailable_latest()
    {
        let fixture = Fixture::new();
        let root = fixture.0.join("project");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("templates")).unwrap();
        fs::create_dir(root.join("documents")).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let bytes = template(&id, 2, "현재 템플릿");
        fs::write(root.join(format!("templates/{id}.json")), &bytes).unwrap();
        let current=serde_json::to_vec(&json!({"artifactType":"document","schemaVersion":1,"documentId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","templateId":id,"templateRevision":1,"name":"현재 문서","fieldValues":{},"orphanedFieldDefinitions":{},"createdAtUtc":"2026-10-04T00:00:00.000Z","updatedAtUtc":"2026-10-04T00:00:01.000Z"})).unwrap();
        fs::write(
            root.join("documents/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json"),
            &current,
        )
        .unwrap();
        for (kind, id, bytes) in [
            ("template", id.as_str(), bytes.as_slice()),
            (
                "document",
                "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                current.as_slice(),
            ),
        ] {
            let path = root.join(format!(
                ".worldbuild/format-history/{kind}-{id}/{}.json",
                model::digest(bytes)
            ));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let mut runtime = ProjectRuntime::acquire(&root, &fixture.0.join("locks")).unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let store = Store::open(&fixture.0.join("edit-recovery")).unwrap();
        let target = ArtifactSourceId::Template(id.parse().unwrap());
        versions::confirm(&repository, target, "2026-10-04T00:00:02.000Z").unwrap();
        store.transition_format_history(&repository).unwrap();
        assert_eq!(versions::list(&repository, target).unwrap().len(), 1);
        let document =
            ArtifactSourceId::Document("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse().unwrap());
        let rows = versions::list(&repository, document).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].available);
        assert!(!rows[1].available);
        assert!(versions::snapshot(&repository, document, 2).is_ok());
        assert!(versions::snapshot(&repository, document, 1).is_err());
        drop(store);
        drop(repository);
        drop(ready);
        drop(runtime);
    }
    #[test]
    fn legacy_document_without_matching_interpretation_is_unavailable_and_exact_raw_stays_in_backup(
    ) {
        for ambiguous in [false, true] {
            let fixture = Fixture::new();
            let root = fixture.0.join("project");
            fs::create_dir(&root).unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let template = uuid::Uuid::new_v4().to_string();
            let bytes=serde_json::to_vec(&json!({"artifactType":"document","schemaVersion":1,"documentId":id,"templateId":template,"templateRevision":1,"name":"이전 문서","fieldValues":{},"orphanedFieldDefinitions":{},"createdAtUtc":"2026-10-04T00:00:00.000Z","updatedAtUtc":"2026-10-04T00:00:01.000Z"})).unwrap();
            artifact::decode_document(&bytes).unwrap();
            let mut ambiguous_definitions = Vec::new();
            if ambiguous {
                for name in ["동일 버전의 첫 정의", "동일 버전의 다른 정의"] {
                    let definition = self::template(&template, 1, name);
                    let relative = format!(
                        ".worldbuild/format-history/template-{template}/{}.json",
                        model::digest(&definition)
                    );
                    let path = root.join(&relative);
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(&path, &definition).unwrap();
                    ambiguous_definitions.push((relative, definition));
                }
            }
            let relative = format!(
                ".worldbuild/format-history/document-{id}/{}.json",
                model::digest(&bytes)
            );
            let path = root.join(&relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &bytes).unwrap();
            let mut runtime = ProjectRuntime::acquire(&root, &fixture.0.join("locks")).unwrap();
            runtime.recover().unwrap();
            let ready = runtime.ready().unwrap();
            let repository = ArtifactRepository::new(&ready).unwrap();
            let store = Store::open(&fixture.0.join("edit-recovery")).unwrap();
            store.transition_format_history(&repository).unwrap();
            let target = ArtifactSourceId::Document(id.parse().unwrap());
            let rows = versions::list(&repository, target).unwrap();
            assert_eq!(rows.len(), 1);
            assert!(!rows[0].available);
            assert!(versions::snapshot(&repository, target, 1).is_err());
            assert!(!path.exists());
            let backup = project_backup::list_backups(
                repository.write_project().fingerprint(),
                &fixture.0.join("transition-backup-storage"),
                None,
            )
            .unwrap()
            .backups[0]
                .locator
                .clone();
            assert_eq!(
                fs::read(Path::new(&backup).join("payload").join(relative)).unwrap(),
                bytes
            );
            for (relative, definition) in ambiguous_definitions {
                assert_eq!(
                    fs::read(Path::new(&backup).join("payload").join(relative)).unwrap(),
                    definition
                );
            }
            drop(store);
            drop(repository);
            drop(ready);
            drop(runtime);
        }
    }
    #[test]
    fn stored_ambiguous_interpretation_stops_before_import_or_completed_cleanup() {
        for completed in [false, true] {
            let fixture = Fixture::new();
            let root = fixture.0.join("project");
            fs::create_dir(&root).unwrap();
            let template_id = uuid::Uuid::new_v4().to_string();
            let document_id = uuid::Uuid::new_v4().to_string();
            let mut originals = BTreeMap::new();
            for name in ["동일 버전 A", "동일 버전 B"] {
                let bytes = template(&template_id, 1, name);
                let relative = format!(
                    ".worldbuild/format-history/template-{template_id}/{}.json",
                    model::digest(&bytes)
                );
                fs::create_dir_all(root.join(&relative).parent().unwrap()).unwrap();
                fs::write(root.join(&relative), &bytes).unwrap();
                originals.insert(relative, bytes);
            }
            let selected = originals.keys().next().unwrap().clone();
            let doc = serde_json::to_vec(&json!({"artifactType":"document","schemaVersion":1,"documentId":document_id,"templateId":template_id,"templateRevision":1,"name":"보존할 문서","fieldValues":{},"orphanedFieldDefinitions":{},"createdAtUtc":"2026-10-04T00:00:00.000Z","updatedAtUtc":"2026-10-04T00:00:01.000Z"})).unwrap();
            let relative = format!(
                ".worldbuild/format-history/document-{document_id}/{}.json",
                model::digest(&doc)
            );
            fs::create_dir_all(root.join(&relative).parent().unwrap()).unwrap();
            fs::write(root.join(&relative), &doc).unwrap();
            originals.insert(relative, doc);
            let mut runtime = ProjectRuntime::acquire(&root, &fixture.0.join("locks")).unwrap();
            runtime.recover().unwrap();
            let ready = runtime.ready().unwrap();
            let repository = ArtifactRepository::new(&ready).unwrap();
            let store = Store::open(&fixture.0.join("edit-recovery")).unwrap();
            let (storage, _storage) =
                owned_directory(&fixture.0, "transition-backup-storage").unwrap();
            let mut receipt =
                initial_receipt(&repository, &storage, &legacy_leaves(&root).unwrap()).unwrap();
            receipt
                .leaves
                .iter_mut()
                .find(|v| matches!(target(&v.path).unwrap(), ArtifactSourceId::Document(_)))
                .unwrap()
                .template = Some(selected);
            let (stage, guard) = owned_directory(&root.join(".worldbuild"), DIRECTORY).unwrap();
            let mut markers = BTreeMap::from([
                ("transition.json", serde_json::to_vec(&receipt).unwrap()),
                ("intent-0.json", b"1".to_vec()),
                ("done-0.json", b"1".to_vec()),
            ]);
            if completed {
                markers.insert(
                    "completed.json",
                    serde_json::to_vec(&Completion {
                        schema: 1,
                        digest: model::digest(&serde_json::to_vec(&receipt).unwrap()),
                        receipt: receipt.clone(),
                    })
                    .unwrap(),
                );
            }
            for (name, bytes) in &markers {
                stage_exact(&stage, &guard, name, bytes).unwrap();
            }
            for _ in 0..3 {
                assert!(store.transition_format_history(&repository).is_err());
                for (name, bytes) in &markers {
                    assert_eq!(read_exact(&stage, &guard, name).unwrap(), *bytes);
                }
                for (relative, bytes) in &originals {
                    assert_eq!(fs::read(root.join(relative)).unwrap(), *bytes);
                }
                assert!(!root.join(".worldbuild/content-versions").exists());
            }
        }
    }
}
