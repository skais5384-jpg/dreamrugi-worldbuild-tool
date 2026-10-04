//! Safe first phase of the legacy transition. Staging and verified whole-project
//! backup grant no canonical-write or legacy-delete authority by themselves.
use super::*;
use crate::data::{assets, project_backup, repository::ArtifactRepository};
use std::io;
use std::io::{Seek, SeekFrom};
pub(crate) const DIRECTORY: &str = "transition-inputs";

pub(crate) struct StagedInput {
    pub(crate) key: Key,
    pub(crate) deposit_id: String,
    pub(crate) payload_digest: String,
    pub(crate) file_digest: String,
    pub(crate) path: PathBuf,
}
impl StagedInput {
    /// Valid owned packages referenced by the latest input become ordinary
    /// project assets. Missing/corrupt packages remain represented by their raw
    /// references and the verified pre-transition backup, never invented bytes.
    pub(crate) fn restore_live_assets(
        &self,
        verified: &VerifiedBackup,
        project: &Path,
        draft: &model::Draft,
    ) -> io::Result<()> {
        self.read_verified(verified)?;
        let backup = &verified.row;
        let source = match assets::Store::open(
            self.path
                .parent()
                .ok_or_else(|| invalid("invalid stage path"))?,
            false,
        ) {
            Ok(store) => store,
            Err(error)
                if error
                    .source
                    .as_ref()
                    .is_some_and(|source| source.kind() == io::ErrorKind::NotFound) =>
            {
                return Ok(())
            }
            Err(error) => return Err(io::Error::other(error)),
        };
        let raw = serde_json::to_value(draft).map_err(io::Error::other)?;
        let referenced = assets::references(&raw).map_err(io::Error::other)?;
        for id in referenced {
            let (metadata, bytes) = match source.read(&id) {
                Ok(package) => package,
                Err(error)
                    if error
                        .source
                        .as_ref()
                        .is_some_and(|source| source.kind() == io::ErrorKind::NotFound)
                        || matches!(
                            error.category,
                            error::Category::Corrupt | error::Category::DigestMismatch
                        ) =>
                {
                    continue
                }
                Err(error) => return Err(io::Error::other(error)),
            };
            let package = self
                .path
                .parent()
                .ok_or_else(|| invalid("invalid stage path"))?
                .join("assets")
                .join(&id);
            let package_guard = ProjectDirectory::open_root(&package)?;
            let copy = Path::new(&backup.locator)
                .join("payload/.worldbuild/transition-inputs")
                .join(&self.key.draft_id)
                .join("assets")
                .join(&id);
            let copy_guard = ProjectDirectory::open_root(&copy)?;
            let mut held = Vec::new();
            for name in ["metadata.json".to_owned(), assets::filename(&metadata)] {
                let mut stage_file = native::open(&package.join(&name), false, false)?;
                package_guard.validate_file(&stage_file, &name)?;
                let mut copy_file = native::open(&copy.join(&name), false, false)?;
                copy_guard.validate_file(&copy_file, &name)?;
                let mut staged = Vec::new();
                Read::by_ref(&mut stage_file)
                    .take(assets::MAX_BYTES as u64 + 1)
                    .read_to_end(&mut staged)?;
                let mut backed = Vec::new();
                Read::by_ref(&mut copy_file)
                    .take(assets::MAX_BYTES as u64 + 1)
                    .read_to_end(&mut backed)?;
                let relative = format!(
                    ".worldbuild/transition-inputs/{}/assets/{}/{}",
                    self.key.draft_id, id, name
                );
                verified.matches_leaf(&relative, &backed)?;
                if staged != backed || staged.len() > assets::MAX_BYTES {
                    return Err(invalid("transition asset differs from independent backup"));
                }
                if name == "metadata.json" {
                    if serde_json::from_slice::<assets::Metadata>(&staged)
                        .map_err(io::Error::other)?
                        != metadata
                    {
                        return Err(invalid("transition asset metadata changed"));
                    }
                } else if staged != bytes {
                    return Err(invalid("transition asset payload changed"));
                }
                held.push((stage_file, copy_file));
            }
            assets::Store::open(project, true)
                .map_err(io::Error::other)?
                .put(&metadata, &bytes)
                .map_err(io::Error::other)?;
            drop(held);
        }
        Ok(())
    }
    /// Workflow receipts are temporary transition metadata, not another history
    /// store. They bind each exact original to its preassigned version number.
    pub(crate) fn import_original(
        &self,
        repository: &ArtifactRepository<'_, '_>,
        target: crate::data::repository::ArtifactSourceId,
        bytes: &[u8],
        template: Option<String>,
        timestamp: &str,
    ) -> io::Result<()> {
        use crate::data::repository::versions;
        let identity = serde_json::to_vec(&(
            format!("{target:?}"),
            model::digest(bytes),
            template
                .as_ref()
                .map(|value| model::digest(value.as_bytes())),
        ))
        .map_err(io::Error::other)?;
        let token = model::digest(&identity);
        let parent = self
            .path
            .parent()
            .ok_or_else(|| invalid("invalid transition directory"))?;
        let guard = ProjectDirectory::open_root(parent)?;
        let intent_name = format!("import-{token}.json");
        let done_name = format!("imported-{token}.json");
        let planned = match read_exact(parent, &guard, &intent_name) {
            Ok(bytes) => {
                let intent: ImportIntent =
                    serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                if intent.schema_version != 1 || intent.identity != token || intent.version == 0 {
                    return Err(invalid("transition version intent mismatch"));
                }
                intent.version
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let planned = versions::number_for_preserved(repository, target, bytes, &template)?;
                let intent = ImportIntent {
                    schema_version: 1,
                    identity: token.clone(),
                    version: planned,
                };
                stage_exact(
                    parent,
                    &guard,
                    &intent_name,
                    &serde_json::to_vec(&intent).map_err(io::Error::other)?,
                )?;
                planned
            }
            Err(error) => return Err(error),
        };
        match read_exact(parent, &guard, &done_name) {
            Ok(bytes) => {
                let receipt: ImportIntent =
                    serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                if receipt.schema_version != 1
                    || receipt.identity != token
                    || receipt.version == 0
                    || receipt.version > planned
                {
                    return Err(invalid("transition version receipt mismatch"));
                }
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        let version =
            versions::import_preserved_at(repository, target, bytes, template, timestamp, planned)?;
        let receipt = ImportIntent {
            schema_version: 1,
            identity: token,
            version,
        };
        stage_exact(
            parent,
            &guard,
            &done_name,
            &serde_json::to_vec(&receipt).map_err(io::Error::other)?,
        )
    }
    /// Persist the unique target/candidate intent before the first create. A retry
    /// may adopt only this same candidate after transaction recovery has completed.
    pub(crate) fn creation_intent(
        &self,
        kind: &str,
        id: &str,
        candidate: &[u8],
        absent: bool,
    ) -> io::Result<()> {
        if !matches!(kind, "template" | "document") || !model::valid_id(id) {
            return Err(invalid("invalid creation intent"));
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| invalid("invalid stage path"))?;
        let guard = ProjectDirectory::open_root(parent)?;
        let name = format!("created-{kind}.json");
        let bytes = serde_json::to_vec(
            &serde_json::json!({"schemaVersion":1,"draft":self.key.draft_id,
            "target":id,"kind":kind,"candidateDigest":model::digest(candidate)}),
        )
        .map_err(io::Error::other)?;
        match read_exact(parent, &guard, &name) {
            Ok(previous) if previous == bytes => return Ok(()),
            Ok(_) => return Err(invalid("creation intent changed; retain legacy input")),
            Err(error) if error.kind() == io::ErrorKind::NotFound && absent => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(invalid("transition target already occupied"))
            }
            Err(error) => return Err(error),
        }
        stage_exact(parent, &guard, &name, &bytes)
    }
    pub(crate) fn read_backed(
        &self,
        backup: &project_backup::BackupRow,
    ) -> io::Result<model::Envelope> {
        self.read_verified(&VerifiedBackup::new(backup)?)
    }
    pub(crate) fn read_verified(&self, verified: &VerifiedBackup) -> io::Result<model::Envelope> {
        verified.guard.validate()?;
        let backup = &verified.row;
        let name = self
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid("invalid stage name"))?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| invalid("invalid stage path"))?;
        let guard = ProjectDirectory::open_root(parent)?;
        let bytes = read_exact(parent, &guard, name)?;
        if model::digest(&bytes) != self.file_digest {
            return Err(invalid("transition source changed"));
        }
        let copy = Path::new(&backup.locator)
            .join("payload/.worldbuild/transition-inputs")
            .join(&self.key.draft_id);
        let copy_guard = ProjectDirectory::open_root(&copy)?;
        let backed = read_exact(&copy, &copy_guard, name)?;
        verified.matches_leaf(
            &format!(
                ".worldbuild/transition-inputs/{}/{}",
                self.key.draft_id, name
            ),
            &backed,
        )?;
        if backed != bytes {
            return Err(invalid("transition backup differs from staged input"));
        }
        let deposit = Deposit::decode(&bytes).map_err(io::Error::other)?;
        if deposit.key() != &self.key
            || deposit.envelope().deposit_id != self.deposit_id
            || deposit.payload_digest() != self.payload_digest
        {
            return Err(invalid("transition receipt mismatch"));
        }
        Ok(deposit.envelope().clone())
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportIntent {
    schema_version: u32,
    identity: String,
    version: u64,
}
pub(crate) struct Prepared {
    pub(crate) inputs: Vec<StagedInput>,
    pub(crate) backup: project_backup::BackupRow,
    manifest_digest: String,
    stage: PathBuf,
    completed: bool,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransitionReceipt {
    schema_version: u32,
    project_fingerprint: String,
    backup: serde_json::Value,
    manifest_digest: String,
    inputs: Vec<InputReceipt>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InputReceipt {
    key: Key,
    deposit_id: String,
    payload_digest: String,
    file_digest: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompletionReceipt {
    schema_version: u32,
    transition_digest: String,
    transition: TransitionReceipt,
}

pub(crate) struct VerifiedBackup {
    row: project_backup::BackupRow,
    guard: ProjectDirectory,
    inventory: project_backup::VerifiedInventory,
}
impl VerifiedBackup {
    /// Keep every old raw input and attachment in the independent backup fixed
    /// until both local copies have been retired. An inventory hash alone is not
    /// a preservation proof when the backup could change during cleanup.
    fn hold_preserved_inputs(&self) -> io::Result<(Vec<ProjectDirectory>, Vec<std::fs::File>)> {
        self.guard.validate()?;
        let payload = Path::new(&self.row.locator).join("payload");
        let mut paths = std::collections::BTreeSet::new();
        paths.insert(payload.clone());
        for relative in self
            .inventory
            .files
            .keys()
            .filter(|relative| relative.starts_with(".worldbuild/transition-inputs/"))
        {
            let path = payload.join(relative);
            let mut parent = path.parent();
            while let Some(directory) = parent {
                if directory == payload {
                    break;
                }
                if !directory.starts_with(&payload) {
                    return Err(invalid("backup input outside payload"));
                }
                paths.insert(directory.to_path_buf());
                parent = directory.parent();
            }
        }
        let mut guards = Vec::new();
        for path in paths {
            guards.push(ProjectDirectory::open_root(&path)?);
        }
        let mut held = Vec::new();
        for relative in self
            .inventory
            .files
            .keys()
            .filter(|relative| relative.starts_with(".worldbuild/transition-inputs/"))
        {
            let mut file = native::open(&payload.join(relative), false, false)?;
            let parent = payload
                .join(relative)
                .parent()
                .ok_or_else(|| invalid("invalid backup input parent"))?
                .to_path_buf();
            let child = ProjectDirectory::open_root(&parent)?;
            child.validate_file(
                &file,
                Path::new(relative)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| invalid("invalid backup input name"))?,
            )?;
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take(assets::MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            self.matches_leaf(relative, &bytes)?;
            held.push(file);
        }
        for guard in &guards {
            guard.validate()?;
        }
        Ok((guards, held))
    }
    fn matches_leaf(&self, relative: &str, bytes: &[u8]) -> io::Result<()> {
        self.guard.validate()?;
        if self.inventory.files.get(relative) != Some(&(bytes.len() as u64, model::digest(bytes))) {
            return Err(invalid("transition leaf differs from verified manifest"));
        }
        Ok(())
    }
    fn new(backup: &project_backup::BackupRow) -> io::Result<Self> {
        let guard = ProjectDirectory::open_root(Path::new(&backup.locator))?;
        let inventory = project_backup::verified_inventory(backup).map_err(io::Error::other)?;
        if backup.status != "verified" {
            return Err(invalid("transition backup not verified"));
        }
        guard.validate()?;
        Ok(Self {
            row: backup.clone(),
            guard,
            inventory,
        })
    }
}
impl Prepared {
    pub(crate) fn verify(&self) -> io::Result<VerifiedBackup> {
        let verified = VerifiedBackup::new(&self.backup)?;
        if verified.inventory.manifest_digest != self.manifest_digest {
            return Err(invalid("transition backup manifest changed"));
        }
        Ok(verified)
    }
    pub(crate) fn completed(&self) -> bool {
        self.completed
    }
    fn receipt(&self, fingerprint: &str) -> io::Result<Vec<u8>> {
        serde_json::to_vec(&TransitionReceipt {
            schema_version: 1,
            project_fingerprint: fingerprint.into(),
            backup: serde_json::to_value(&self.backup).map_err(io::Error::other)?,
            manifest_digest: self.manifest_digest.clone(),
            inputs: self
                .inputs
                .iter()
                .map(|input| InputReceipt {
                    key: input.key.clone(),
                    deposit_id: input.deposit_id.clone(),
                    payload_digest: input.payload_digest.clone(),
                    file_digest: input.file_digest.clone(),
                })
                .collect(),
        })
        .map_err(io::Error::other)
    }
    /// Once every current target and version is durable, only cleanup may resume.
    /// The independent whole backup remains outside this temporary namespace.
    pub(crate) fn finish(&self, store: &mut Store) -> io::Result<()> {
        let verified = self.verify()?;
        let _preserved_backup = verified.hold_preserved_inputs()?;
        let guard = ProjectDirectory::open_root(&self.stage)?;
        let fingerprint = self
            .inputs
            .first()
            .ok_or_else(|| invalid("transition input missing"))?
            .key
            .project_fingerprint
            .as_str();
        let receipt = self.receipt(fingerprint)?;
        stage_exact(&self.stage, &guard, "transition.json", &receipt)?;
        let completion = serde_json::to_vec(&CompletionReceipt {
            schema_version: 1,
            transition_digest: model::digest(&receipt),
            transition: serde_json::from_slice(&receipt).map_err(io::Error::other)?,
        })
        .map_err(io::Error::other)?;
        stage_exact(&self.stage, &guard, "completed.json", &completion)?;
        drop(guard);
        cleanup_transition(store, self, &verified)
    }
}
#[cfg(test)]
thread_local! { static CLEANUP_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }
fn cleanup_checkpoint(_point: &'static str) -> io::Result<()> {
    #[cfg(test)]
    if CLEANUP_FAULT.with(|fault| {
        if fault.get() == Some(_point) {
            fault.set(None);
            true
        } else {
            false
        }
    }) {
        return Err(io::Error::other("injected transition cleanup failure"));
    }
    Ok(())
}
struct CleanupFile {
    path: PathBuf,
    length: usize,
    digest: String,
}
fn collect_transition_cleanup(
    path: &Path,
    relative: &str,
    prepared: &Prepared,
    verified: &VerifiedBackup,
    workflow: bool,
    files: &mut Vec<CleanupFile>,
    directories: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let guard = match ProjectDirectory::open_root(path) {
        Ok(guard) => guard,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in guard.read_dir()? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| invalid("unknown transition cleanup entry"))?;
        let child = path.join(&name);
        let suffix = relative
            .strip_prefix(".worldbuild/transition-inputs")
            .ok_or_else(|| invalid("invalid cleanup namespace"))?
            .trim_start_matches('/');
        let parts = if suffix.is_empty() {
            vec![]
        } else {
            suffix.split('/').collect::<Vec<_>>()
        };
        let folder = match parts.as_slice() {
            [] => {
                model::valid_id(&name)
                    && prepared
                        .inputs
                        .iter()
                        .any(|input| input.key.draft_id == name)
            }
            [draft] => model::valid_id(draft) && name == "assets",
            [draft, "assets"] => model::valid_id(draft) && model::valid_id(&name),
            _ => false,
        };
        if folder {
            collect_transition_cleanup(
                &child,
                &format!("{relative}/{name}"),
                prepared,
                verified,
                workflow,
                files,
                directories,
            )?;
            continue;
        }
        if files.len() >= 100_000 {
            return Err(invalid("transition cleanup exceeds bound"));
        }
        let leaf = format!("{relative}/{name}");
        let bytes = read_exact(path, &guard, &name)?;
        let raw_input = match parts.as_slice() {
            [draft] => generation(&name).is_some_and(|generation| {
                name == format!("{generation}.json")
                    && prepared.inputs.iter().any(|input| {
                        input.key.draft_id == *draft && input.key.generation == generation
                    })
            }),
            [draft, "assets", asset] => {
                model::valid_id(draft)
                    && model::valid_id(asset)
                    && (name == "metadata.json"
                        || name.strip_prefix("content.").is_some_and(|extension| {
                            !extension.is_empty()
                                && extension.len() <= 12
                                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
                        }))
            }
            _ => false,
        };
        if raw_input {
            verified.matches_leaf(&leaf, &bytes)?;
        } else if workflow {
            match parts.as_slice() {
                [] if name == "backup-intent.json" => {
                    let id: String = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                    if id != verified.row.id {
                        return Err(invalid("transition backup intent differs"));
                    }
                }
                [] if name == "transition.json" => {
                    let fingerprint = &prepared
                        .inputs
                        .first()
                        .ok_or_else(|| invalid("transition inputs missing"))?
                        .key
                        .project_fingerprint;
                    if bytes != prepared.receipt(fingerprint)? {
                        return Err(invalid("transition receipt changed before cleanup"));
                    }
                }
                [] if name == "completed.json" => {
                    let marker: CompletionReceipt =
                        serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                    let expected = prepared.receipt(
                        &prepared
                            .inputs
                            .first()
                            .ok_or_else(|| invalid("transition inputs missing"))?
                            .key
                            .project_fingerprint,
                    )?;
                    if marker.schema_version != 1
                        || marker.transition_digest != model::digest(&expected)
                        || serde_json::to_vec(&marker.transition).map_err(io::Error::other)?
                            != expected
                    {
                        return Err(invalid("transition completion changed"));
                    }
                }
                [_draft]
                    if name
                        .strip_prefix("import-")
                        .or_else(|| name.strip_prefix("imported-"))
                        .and_then(|value| value.strip_suffix(".json"))
                        .is_some_and(model::valid_digest) =>
                {
                    let intent: ImportIntent =
                        serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                    let token = name
                        .strip_prefix("import-")
                        .or_else(|| name.strip_prefix("imported-"))
                        .and_then(|value| value.strip_suffix(".json"))
                        .ok_or_else(|| invalid("invalid import metadata"))?;
                    if intent.schema_version != 1 || intent.identity != token || intent.version == 0
                    {
                        return Err(invalid("invalid import metadata"));
                    }
                }
                [draft] if name == "created-template.json" || name == "created-document.json" => {
                    let value = crate::data::json::parse_strict_json_object(&bytes)
                        .map_err(io::Error::other)?;
                    let object = value
                        .as_object()
                        .ok_or_else(|| invalid("invalid creation metadata"))?;
                    let kind = if name == "created-template.json" {
                        "template"
                    } else {
                        "document"
                    };
                    if object.len() != 5
                        || value["schemaVersion"] != 1
                        || value["draft"].as_str() != Some(*draft)
                        || value["kind"].as_str() != Some(kind)
                        || !value["target"].as_str().is_some_and(model::valid_id)
                        || !value["candidateDigest"]
                            .as_str()
                            .is_some_and(model::valid_digest)
                    {
                        return Err(invalid("invalid creation metadata"));
                    }
                }
                _ => return Err(invalid("unknown transition workflow file; retain it")),
            }
        } else {
            return Err(invalid("unknown legacy entry; retain it"));
        }
        files.push(CleanupFile {
            path: child,
            length: bytes.len(),
            digest: model::digest(&bytes),
        });
    }
    guard.validate()?;
    directories.push(path.to_path_buf());
    Ok(())
}
fn remove_cleanup_file(expected: CleanupFile) -> io::Result<()> {
    let parent = expected
        .path
        .parent()
        .ok_or_else(|| invalid("invalid cleanup parent"))?;
    let guard = ProjectDirectory::open_root(parent)?;
    let name = expected
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("invalid cleanup filename"))?;
    let mut file = native::open_for_discard(&expected.path)?;
    guard.validate_file(&file, name)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(assets::MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() != expected.length || model::digest(&bytes) != expected.digest {
        return Err(invalid("transition entry changed before cleanup"));
    }
    native::cleanup(&file)
}
fn cleanup_transition(
    store: &mut Store,
    prepared: &Prepared,
    verified: &VerifiedBackup,
) -> io::Result<()> {
    store.validate_root().map_err(io::Error::other)?;
    let fingerprint = &prepared
        .inputs
        .first()
        .ok_or_else(|| invalid("transition inputs missing"))?
        .key
        .project_fingerprint;
    let global = store.root.join(fingerprint);
    let mut legacy_files = Vec::new();
    let mut legacy_directories = Vec::new();
    collect_transition_cleanup(
        &global,
        ".worldbuild/transition-inputs",
        prepared,
        verified,
        false,
        &mut legacy_files,
        &mut legacy_directories,
    )?;
    let mut stage_files = Vec::new();
    let mut stage_directories = Vec::new();
    collect_transition_cleanup(
        &prepared.stage,
        ".worldbuild/transition-inputs",
        prepared,
        verified,
        true,
        &mut stage_files,
        &mut stage_directories,
    )?;
    // Complete admission of both namespaces precedes the first destructive step.
    for (index, file) in legacy_files.into_iter().enumerate() {
        remove_cleanup_file(file)?;
        if index == 0 {
            cleanup_checkpoint("after-first-legacy-file")?;
        }
    }
    for directory in legacy_directories {
        ProjectDirectory::open_owned_root(&directory)?.delete_owned()?;
    }
    stage_files.sort_by_key(|file| {
        if file.path == prepared.stage.join("completed.json") {
            2
        } else if file.path == prepared.stage.join("transition.json") {
            1
        } else {
            0
        }
    });
    // Keep the self-contained completion marker until every raw file and workflow
    // receipt is gone. A killed cleanup resumes without replaying the migration.
    for file in stage_files {
        remove_cleanup_file(file)?;
    }
    for directory in stage_directories {
        ProjectDirectory::open_owned_root(&directory)?.delete_owned()?;
    }
    Ok(())
}
fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}
pub(super) fn owned_directory(
    parent: &Path,
    name: &str,
) -> io::Result<(PathBuf, ProjectDirectory)> {
    let guard = ProjectDirectory::open_root(parent)?;
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error),
    }
    guard.validate()?;
    let child = ProjectDirectory::open_root(&path)?;
    Ok((path, child))
}
pub(super) fn read_exact(path: &Path, guard: &ProjectDirectory, name: &str) -> io::Result<Vec<u8>> {
    let mut file = native::open(&path.join(name), false, false)?;
    guard.validate_file(&file, name)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(assets::MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > assets::MAX_BYTES {
        return Err(invalid("legacy input exceeds limit"));
    }
    Ok(bytes)
}
pub(super) fn backup_intent(
    path: &Path,
    guard: &ProjectDirectory,
    fingerprint: &str,
) -> io::Result<String> {
    let id = match read_exact(path, guard, "backup-intent.json") {
        Ok(bytes) => serde_json::from_slice::<String>(&bytes).map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // The directory belongs to this one transition. Its identity and
            // creation time survive a killed first record publication.
            let created = fs::metadata(path)?
                .created()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let token =
                model::digest(format!("{fingerprint}:{:?}:{created}", guard.identity()).as_bytes());
            let id = uuid::Uuid::from_u128(
                u128::from_str_radix(&token[..32], 16).map_err(io::Error::other)?,
            )
            .to_string();
            stage_exact(
                path,
                guard,
                "backup-intent.json",
                &serde_json::to_vec(&id).map_err(io::Error::other)?,
            )?;
            id
        }
        Err(error) => return Err(error),
    };
    if uuid::Uuid::parse_str(&id)
        .ok()
        .map(|value| value.to_string())
        .as_deref()
        != Some(id.as_str())
    {
        return Err(invalid("invalid transition backup identity"));
    }
    Ok(id)
}
fn pending_name(name: &str, bytes: &[u8]) -> String {
    let mut identity = name.as_bytes().to_vec();
    identity.push(0);
    identity.extend_from_slice(bytes);
    format!("pending-{}", model::digest(&identity))
}
pub(crate) fn stage_exact(
    path: &Path,
    guard: &ProjectDirectory,
    name: &str,
    bytes: &[u8],
) -> io::Result<()> {
    let pending = pending_name(name, bytes);
    match read_exact(path, guard, name) {
        Ok(existing) if existing == bytes => {
            match native::open_for_discard(&path.join(&pending)) {
                Ok(mut file) => {
                    guard.validate_file(&file, &pending)?;
                    let mut prefix = Vec::new();
                    Read::by_ref(&mut file)
                        .take(bytes.len() as u64 + 1)
                        .read_to_end(&mut prefix)?;
                    if !bytes.starts_with(&prefix) {
                        return Err(invalid("owned pending differs from verified record"));
                    }
                    native::cleanup(&file)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
            return Ok(());
        }
        Ok(_) => return Err(invalid("transition input changed; preserve both sources")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let mut file = match native::open(&path.join(&pending), true, true) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            native::open_for_resume(&path.join(&pending))?
        }
        Err(error) => return Err(error),
    };
    guard.validate_file(&file, &pending)?;
    let mut prefix = Vec::new();
    Read::by_ref(&mut file)
        .take(bytes.len() as u64 + 1)
        .read_to_end(&mut prefix)?;
    if !bytes.starts_with(&prefix) {
        return Err(invalid("owned pending input is not the expected prefix"));
    }
    file.seek(SeekFrom::End(0))?;
    file.write_all(&bytes[prefix.len()..])?;
    file.flush()?;
    file.sync_all()?;
    native::publish(&file, name)?;
    guard.validate_file(&file, name)?;
    drop(file);
    if read_exact(path, guard, name)? != bytes {
        return Err(invalid("transition input verification failed"));
    }
    Ok(())
}

fn stage_assets(source: &Path, target: &Path) -> io::Result<()> {
    let source_path = source.join("assets");
    let source_guard = match ProjectDirectory::open_root(&source_path) {
        Ok(guard) => guard,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let (target_path, _target_guard) = owned_directory(target, "assets")?;
    let mut count = 0_usize;
    for entry in source_guard.read_dir()? {
        let id = entry?
            .file_name()
            .into_string()
            .map_err(|_| invalid("invalid legacy asset name"))?;
        if !model::valid_id(&id) {
            return Err(invalid("unknown legacy asset; retain input"));
        }
        let package = source_path.join(&id);
        let guard = ProjectDirectory::open_root(&package)?;
        let (copy, copy_guard) = owned_directory(&target_path, &id)?;
        for entry in guard.read_dir()? {
            count += 1;
            if count > 1_000_000 {
                return Err(invalid("legacy asset listing exceeds bound"));
            }
            let name = entry?
                .file_name()
                .into_string()
                .map_err(|_| invalid("invalid asset file"))?;
            let content = name.strip_prefix("content.").is_some_and(|ext| {
                !ext.is_empty()
                    && ext.len() <= 12
                    && ext.bytes().all(|byte| byte.is_ascii_alphanumeric())
            });
            if name != "metadata.json" && !content {
                return Err(invalid("unfinished or unknown asset file; retain input"));
            }
            stage_exact(
                &copy,
                &copy_guard,
                &name,
                &read_exact(&package, &guard, &name)?,
            )?;
        }
    }
    Ok(())
}
impl Store {
    /// Walk only the admitted project's namespace. No global first-page limit and
    /// no reading unrelated projects' contents. Unknown entries stop cleanup.
    pub(crate) fn stage_project(
        &self,
        root: &Path,
        fingerprint: &str,
    ) -> io::Result<Vec<StagedInput>> {
        if !model::valid_digest(fingerprint) {
            return Err(invalid("invalid transition project"));
        }
        self.validate_root().map_err(io::Error::other)?;
        let source = self.root.join(fingerprint);
        let source_guard = match ProjectDirectory::open_root(&source) {
            Ok(guard) => guard,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(error),
        };
        let (system, _system_guard) = owned_directory(root, ".worldbuild")?;
        let (stage, _stage_guard) = owned_directory(&system, DIRECTORY)?;
        let mut inputs = Vec::new();
        let mut visited = 0_usize;
        for draft in source_guard.read_dir()? {
            let draft = draft?;
            let draft_id = draft
                .file_name()
                .into_string()
                .map_err(|_| invalid("invalid legacy draft namespace"))?;
            if !model::valid_id(&draft_id) {
                return Err(invalid("unknown legacy draft namespace; retain source"));
            }
            let draft_path = source.join(&draft_id);
            let draft_guard = ProjectDirectory::open_root(&draft_path)?;
            let (stage_path, stage_guard) = owned_directory(&stage, &draft_id)?;
            for entry in draft_guard.read_dir()? {
                let entry = entry?;
                visited += 1;
                if visited > 1_000_000 {
                    return Err(invalid(
                        "transition listing exceeds safe bound; retain source",
                    ));
                }
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| invalid("invalid legacy input name"))?;
                if name == "assets" {
                    continue;
                }
                let number = generation(&name)
                    .filter(|number| format!("{number}.json") == name)
                    .ok_or_else(|| invalid("unfinished or unknown legacy input; retain source"))?;
                let bytes = read_exact(&draft_path, &draft_guard, &name)?;
                let deposit = Deposit::decode(&bytes).map_err(io::Error::other)?;
                if deposit.key().project_fingerprint != fingerprint
                    || deposit.key().draft_id != draft_id
                    || deposit.key().generation != number
                {
                    return Err(invalid("legacy input project/target binding changed"));
                }
                // Copy the owned packages byte-for-byte, including readable
                // metadata when content is missing/corrupt. A backup must not
                // silently omit an old input just because its attachment is gone.
                stage_assets(&draft_path, &stage_path)?;
                stage_exact(&stage_path, &stage_guard, &name, &bytes)?;
                inputs.push(StagedInput {
                    key: deposit.key().clone(),
                    deposit_id: deposit.envelope().deposit_id.clone(),
                    payload_digest: deposit.payload_digest().into(),
                    file_digest: model::digest(&bytes),
                    path: stage_path.join(name),
                });
            }
        }
        inputs.sort_by(|a, b| {
            a.key
                .draft_id
                .cmp(&b.key.draft_id)
                .then(a.key.generation.cmp(&b.key.generation))
        });
        Ok(inputs)
    }
    /// The application's owned profile is outside the project. A transition
    /// never starts cleanup if an independent, verified whole backup cannot be made.
    pub(crate) fn prepare_if_present(
        &self,
        repository: &ArtifactRepository<'_, '_>,
    ) -> io::Result<Option<Prepared>> {
        let project = repository.write_project();
        let stage = project.canonical_root().join(".worldbuild").join(DIRECTORY);
        let profile = self
            .root
            .parent()
            .ok_or_else(|| invalid("transition storage unavailable"))?;
        let (storage, _storage_guard) = owned_directory(profile, "transition-backup-storage")?;
        let stage_guard = match ProjectDirectory::open_root(&stage) {
            Ok(guard) => Some(guard),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if let Some(guard) = stage_guard {
            let receipt_bytes = match read_exact(&stage, &guard, "transition.json") {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    match read_exact(&stage, &guard, "completed.json") {
                        Ok(bytes) => {
                            let marker: CompletionReceipt =
                                serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                            let bytes =
                                serde_json::to_vec(&marker.transition).map_err(io::Error::other)?;
                            if marker.schema_version != 1
                                || marker.transition_digest != model::digest(&bytes)
                            {
                                return Err(invalid("invalid completion receipt"));
                            }
                            Ok(bytes)
                        }
                        Err(error) => Err(error),
                    }
                }
                result => result,
            };
            match receipt_bytes {
                Ok(bytes) => {
                    let receipt: TransitionReceipt =
                        serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                    if receipt.schema_version != 1
                        || receipt.project_fingerprint != project.fingerprint()
                        || !model::valid_digest(&receipt.manifest_digest)
                        || receipt.inputs.is_empty()
                        || receipt.inputs.len() > 100_000
                    {
                        return Err(invalid("invalid transition receipt"));
                    }
                    let locator = receipt
                        .backup
                        .get("locator")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| invalid("transition backup locator missing"))?;
                    let expected_parent = fs::canonicalize(
                        storage
                            .join("worldbuild-backups")
                            .join(project.fingerprint()),
                    )?;
                    let actual = fs::canonicalize(locator)?;
                    if actual.parent() != Some(expected_parent.as_path()) {
                        return Err(invalid("transition backup outside owned profile"));
                    }
                    let backup =
                        project_backup::inspect_backup(&actual).map_err(io::Error::other)?;
                    if serde_json::to_value(&backup).map_err(io::Error::other)? != receipt.backup {
                        return Err(invalid("transition backup receipt differs"));
                    }
                    let mut inputs = Vec::new();
                    let mut seen = std::collections::BTreeSet::new();
                    for entry in receipt.inputs {
                        entry.key.validate().map_err(io::Error::other)?;
                        if entry.key.project_fingerprint != project.fingerprint()
                            || !model::valid_id(&entry.deposit_id)
                            || !model::valid_digest(&entry.payload_digest)
                            || !model::valid_digest(&entry.file_digest)
                            || !seen.insert((entry.key.draft_id.clone(), entry.key.generation))
                        {
                            return Err(invalid("invalid transition input receipt"));
                        }
                        inputs.push(StagedInput {
                            path: stage
                                .join(&entry.key.draft_id)
                                .join(format!("{}.json", entry.key.generation)),
                            key: entry.key,
                            deposit_id: entry.deposit_id,
                            payload_digest: entry.payload_digest,
                            file_digest: entry.file_digest,
                        });
                    }
                    let completed = match read_exact(&stage, &guard, "completed.json") {
                        Ok(marker) => {
                            let marker: CompletionReceipt =
                                serde_json::from_slice(&marker).map_err(io::Error::other)?;
                            if marker.schema_version != 1
                                || marker.transition_digest != model::digest(&bytes)
                                || serde_json::to_vec(&marker.transition)
                                    .map_err(io::Error::other)?
                                    != bytes
                            {
                                return Err(invalid("invalid transition completion"));
                            }
                            true
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                        Err(error) => return Err(error),
                    };
                    let prepared = Prepared {
                        inputs,
                        backup,
                        manifest_digest: receipt.manifest_digest,
                        stage,
                        completed,
                    };
                    prepared.verify()?;
                    return Ok(Some(prepared));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        let inputs = self.stage_project(project.canonical_root(), project.fingerprint())?;
        if inputs.is_empty() {
            return Ok(None);
        }
        self.prepare_staged(repository, &storage, inputs).map(Some)
    }
    pub(crate) fn prepare_transition(
        &self,
        repository: &ArtifactRepository<'_, '_>,
        storage: &Path,
    ) -> io::Result<Prepared> {
        let project = repository.write_project();
        let inputs = self.stage_project(project.canonical_root(), project.fingerprint())?;
        self.prepare_staged(repository, storage, inputs)
    }
    fn prepare_staged(
        &self,
        repository: &ArtifactRepository<'_, '_>,
        storage: &Path,
        inputs: Vec<StagedInput>,
    ) -> io::Result<Prepared> {
        let project = repository.write_project();
        let stage = project.canonical_root().join(".worldbuild").join(DIRECTORY);
        let guard = ProjectDirectory::open_root(&stage)?;
        let id = backup_intent(&stage, &guard, project.fingerprint())?;
        let backup = project_backup::create_transition_backup(
            project.canonical_root(),
            project.fingerprint(),
            storage,
            "자료 보존 체계 전환 전",
            &id,
        )
        .map_err(io::Error::other)?;
        let checked =
            project_backup::inspect_backup(Path::new(&backup.locator)).map_err(io::Error::other)?;
        if checked != backup || checked.status != "verified" {
            return Err(invalid("transition backup verification failed"));
        }
        let inventory = project_backup::verified_inventory(&backup).map_err(io::Error::other)?;
        let stage = project.canonical_root().join(".worldbuild").join(DIRECTORY);
        let guard = ProjectDirectory::open_root(&stage)?;
        let prepared = Prepared {
            inputs,
            backup,
            manifest_digest: inventory.manifest_digest.clone(),
            stage,
            completed: false,
        };
        stage_exact(
            &prepared.stage,
            &guard,
            "transition.json",
            &prepared.receipt(project.fingerprint())?,
        )?;
        Ok(prepared)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::data::project_runtime::ProjectRuntime;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("worldbuild-transition-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn root(&self) -> PathBuf {
            self.0.join("edit-recovery")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn sample() -> model::Envelope {
        model::Envelope {
            residual: None,
            residual_ack: None,
            key: Key {
                project_fingerprint: "a".repeat(64),
                draft_id: uuid::Uuid::new_v4().to_string(),
                generation: 1,
            },
            deposit_id: uuid::Uuid::new_v4().to_string(),
            app_version: "1.0.0".into(),
            created_at_utc: "2026-10-04T01:02:03.004Z".into(),
            originals: vec![],
            attempt: None,
            draft: model::Draft::Template {
                sections: vec![],
                template: None,
                name: "미생성 입력".into(),
                glossary_excluded: false,
                presentation: model::Intent::Keep,
                composing: false,
                fields: vec![],
            },
        }
    }

    #[test]
    fn transition_backup_contains_exact_legacy_input_and_never_deletes_before_acceptance() {
        let fixture = Fixture::new();
        let root = fixture.0.join("project");
        let storage = fixture.0.join("backups");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&storage).unwrap();
        let mut runtime = ProjectRuntime::acquire(&root, &fixture.0.join("locks")).unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let mut store = Store::open(&fixture.root()).unwrap();
        let mut envelope = sample();
        envelope.key.project_fingerprint = repository.write_project().fingerprint().into();
        let deposit = Deposit::freeze(envelope).unwrap();
        store.accept(&deposit).unwrap();
        let (original, _) = store.directory(deposit.key(), false).unwrap();
        let original = original.join("1.json");
        let before = fs::read(&original).unwrap();
        let prepared = store.prepare_transition(&repository, &storage).unwrap();
        assert_eq!(prepared.inputs.len(), 1);
        assert_eq!(prepared.backup.status, "verified");
        let input = &prepared.inputs[0];
        assert_eq!(input.key, *deposit.key());
        assert_eq!(input.deposit_id, deposit.envelope().deposit_id);
        assert_eq!(input.payload_digest, deposit.payload_digest());
        assert_eq!(input.file_digest, model::digest(&before));
        assert_eq!(fs::read(&input.path).unwrap(), before);
        let backup_copy = Path::new(&prepared.backup.locator)
            .join("payload/.worldbuild/transition-inputs")
            .join(&deposit.key().draft_id)
            .join("1.json");
        assert_eq!(fs::read(&backup_copy).unwrap(), before);
        assert!(input.read_backed(&prepared.backup).unwrap() == *deposit.envelope());
        assert_eq!(fs::read(&original).unwrap(), before);
        // Staging retry is exact and bounded; failed backup admission has no delete side effects.
        let repeated = store
            .stage_project(&root, repository.write_project().fingerprint())
            .unwrap();
        assert_eq!(repeated.len(), 1);
        assert_eq!(fs::read(&repeated[0].path).unwrap(), before);
        assert!(store.prepare_transition(&repository, &root).is_err());
        assert_eq!(fs::read(&original).unwrap(), before);
        assert_eq!(fs::read(&input.path).unwrap(), before);
        fs::write(&backup_copy, b"changed backup copy").unwrap();
        assert!(input.read_backed(&prepared.backup).is_err());
        assert_eq!(fs::read(&original).unwrap(), before);
    }
    #[test]
    fn transition_does_not_read_or_copy_foreign_project_inputs() {
        let fixture = Fixture::new();
        let root = fixture.0.join("project");
        fs::create_dir(&root).unwrap();
        let mut store = Store::open(&fixture.root()).unwrap();
        let deposit = Deposit::freeze(sample()).unwrap();
        store.accept(&deposit).unwrap();
        let foreign = store.root.join("b".repeat(64));
        fs::create_dir(&foreign).unwrap();
        fs::write(
            foreign.join("corrupt-unrelated-input.json"),
            b"do not read me",
        )
        .unwrap();
        let staged = store.stage_project(&root, &"a".repeat(64)).unwrap();
        assert_eq!(staged.len(), 1);
        assert_eq!(
            fs::read(foreign.join("corrupt-unrelated-input.json")).unwrap(),
            b"do not read me"
        );
    }
    #[test]
    fn transition_version_receipts_keep_retry_idempotent_after_pruning_and_publication() {
        use crate::data::repository::{versions, ArtifactSourceId};
        let fixture = Fixture::new();
        let root = fixture.0.join("project");
        let storage = fixture.0.join("backups");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&storage).unwrap();
        let mut runtime = ProjectRuntime::acquire(&root, &fixture.0.join("locks")).unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let mut store = Store::open(&fixture.root()).unwrap();
        let mut envelope = sample();
        envelope.key.project_fingerprint = repository.write_project().fingerprint().into();
        store.accept(&Deposit::freeze(envelope).unwrap()).unwrap();
        let prepared = store.prepare_transition(&repository, &storage).unwrap();
        let input = &prepared.inputs[0];
        let id = uuid::Uuid::new_v4();
        let target = ArtifactSourceId::Template(id.to_string().parse().unwrap());
        let mut originals = Vec::new();
        for number in 0..14 {
            let value = serde_json::json!({"artifactType":"template","schemaVersion":1,"templateId":id,"revision":1,
                "name":format!("기존 확정 {number}"),"lifecycle":"active","presentation":{},"fieldOrder":[],"fields":{},
                "createdAtUtc":"2026-10-04T01:02:03.004Z","updatedAtUtc":"2026-10-04T01:02:03.004Z"});
            let source =
                crate::data::artifact::decode_template(&serde_json::to_vec(&value).unwrap())
                    .unwrap();
            let bytes = crate::data::artifact::encode_template(&source).unwrap();
            input
                .import_original(
                    &repository,
                    target,
                    &bytes,
                    None,
                    "2026-10-04T01:02:03.004Z",
                )
                .unwrap();
            originals.push(bytes);
        }
        let mut current: serde_json::Value =
            serde_json::from_slice(originals.last().unwrap()).unwrap();
        current["name"] = "현재 확정 내용".into();
        fs::create_dir(root.join("templates")).unwrap();
        fs::write(
            root.join(target.path().unwrap().as_str()),
            serde_json::to_vec(&current).unwrap(),
        )
        .unwrap();
        assert_eq!(
            versions::confirm(&repository, target, "2026-10-04T02:02:03.004Z").unwrap(),
            15
        );
        for _ in 0..3 {
            for bytes in &originals {
                input
                    .import_original(&repository, target, bytes, None, "2026-10-04T01:02:03.004Z")
                    .unwrap();
            }
        }
        let history = versions::list(&repository, target).unwrap();
        assert_eq!(history.len(), 10);
        assert_eq!(history[0].version, "15");
        // Crash after an actual publication but before its completion receipt.
        let mut following = current.clone();
        following["name"] = "후속 전환 원본".into();
        let following = crate::data::artifact::encode_template(
            &crate::data::artifact::decode_template(&serde_json::to_vec(&following).unwrap())
                .unwrap(),
        )
        .unwrap();
        input
            .import_original(
                &repository,
                target,
                &following,
                None,
                "2026-10-04T03:02:03.004Z",
            )
            .unwrap();
        let identity = serde_json::to_vec(&(
            format!("{target:?}"),
            model::digest(&following),
            None::<String>,
        ))
        .unwrap();
        fs::remove_file(
            input
                .path
                .parent()
                .unwrap()
                .join(format!("imported-{}.json", model::digest(&identity))),
        )
        .unwrap();
        input
            .import_original(
                &repository,
                target,
                &following,
                None,
                "2026-10-04T03:02:03.004Z",
            )
            .unwrap();
        assert_eq!(
            versions::list(&repository, target).unwrap()[0].version,
            "16"
        );
    }
    #[test]
    fn transition_asset_replacement_of_both_copies_cannot_change_project_bytes() {
        let fixture = Fixture::new();
        let root = fixture.0.join("project");
        let storage = fixture.0.join("backups");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&storage).unwrap();
        let mut runtime = ProjectRuntime::acquire(&root, &fixture.0.join("locks")).unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let mut store = Store::open(&fixture.root()).unwrap();
        let asset = uuid::Uuid::new_v4().to_string();
        let mut envelope = sample();
        envelope.key.project_fingerprint = repository.write_project().fingerprint().into();
        if let model::Draft::Template { fields, .. } = &mut envelope.draft {
            fields.push(serde_json::from_value(serde_json::json!({
            "id":format!("new:{}", uuid::Uuid::new_v4()),"label":"첨부", "configuration":{"kind":"file"},"required":false,
            "presentation":{"intent":"keep"},"default":{"intent":"set","value":{"kind":"file","value":[asset]}},"archived":false
        })).unwrap());
        }
        let deposit = Deposit::freeze(envelope.clone()).unwrap();
        let (source, _) = store.directory(deposit.key(), true).unwrap();
        let original = b"verified attachment";
        let metadata = assets::Metadata {
            schema_version: 1,
            id: asset.clone(),
            name: "notes.txt".into(),
            size: original.len() as u64,
            sha256: model::digest(original),
            image: false,
            width: None,
            height: None,
        };
        assets::Store::open(&source, true)
            .unwrap()
            .put(&metadata, original)
            .unwrap();
        store.accept(&deposit).unwrap();
        let prepared = store.prepare_transition(&repository, &storage).unwrap();
        let input = &prepared.inputs[0];
        let verified = prepared.verify().unwrap();
        let replacement = b"different valid attachment";
        let changed = assets::Metadata {
            size: replacement.len() as u64,
            sha256: model::digest(replacement),
            ..metadata.clone()
        };
        let stage = input.path.parent().unwrap().join("assets").join(&asset);
        let backup = Path::new(&prepared.backup.locator)
            .join("payload/.worldbuild/transition-inputs")
            .join(&deposit.key().draft_id)
            .join("assets")
            .join(&asset);
        for package in [stage, backup] {
            fs::write(
                package.join("metadata.json"),
                serde_json::to_vec(&changed).unwrap(),
            )
            .unwrap();
            fs::write(package.join("content.txt"), replacement).unwrap();
        }
        assert!(input
            .restore_live_assets(&verified, &root, &envelope.draft)
            .is_err());
        assert!(!root.join("assets").exists());
        assert_eq!(
            assets::Store::open(&source, false)
                .unwrap()
                .read(&asset)
                .unwrap(),
            (metadata, original.to_vec())
        );
    }

    #[test]
    fn completed_transition_cleanup_restarts_without_duplicate_backup_or_input_loss() {
        let fixture = Fixture::new();
        let root = fixture.0.join("project");
        fs::create_dir(&root).unwrap();
        let mut runtime = ProjectRuntime::acquire(&root, &fixture.0.join("locks")).unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let mut store = Store::open(&fixture.root()).unwrap();
        let mut envelope = sample();
        envelope.key.project_fingerprint = repository.write_project().fingerprint().into();
        for generation in 1..=2 {
            envelope.key.generation = generation;
            store
                .accept(&Deposit::freeze(envelope.clone()).unwrap())
                .unwrap();
        }
        let prepared = store.prepare_if_present(&repository).unwrap().unwrap();
        let locator = prepared.backup.locator.clone();
        let backup_inputs = prepared
            .inputs
            .iter()
            .map(|input| {
                let path = Path::new(&locator)
                    .join("payload/.worldbuild/transition-inputs")
                    .join(&input.key.draft_id)
                    .join(format!("{}.json", input.key.generation));
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect::<Vec<_>>();
        // A corrupt independent leaf stops cleanup before any source deletion.
        fs::write(&backup_inputs[0].0, b"changed independent copy").unwrap();
        assert!(prepared.finish(&mut store).is_err());
        for input in &prepared.inputs {
            let (path, _) = store.directory(&input.key, false).unwrap();
            assert!(path.join(format!("{}.json", input.key.generation)).exists());
        }
        fs::write(&backup_inputs[0].0, &backup_inputs[0].1).unwrap();
        CLEANUP_FAULT.with(|fault| fault.set(Some("after-first-legacy-file")));
        assert!(prepared.finish(&mut store).is_err());
        assert!(prepared.stage.join("completed.json").exists());
        let resumed = store.prepare_if_present(&repository).unwrap().unwrap();
        assert!(resumed.completed());
        assert_eq!(resumed.backup.locator, locator);
        resumed.finish(&mut store).unwrap();
        assert!(!prepared.stage.exists());
        assert!(!store
            .root
            .join(repository.write_project().fingerprint())
            .exists());
        assert!(store.prepare_if_present(&repository).unwrap().is_none());
        for (path, expected) in backup_inputs {
            assert_eq!(fs::read(path).unwrap(), expected);
        }
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

    #[test]
    fn first_backup_intent_publication_resumes_its_prefix_without_changing_identity() {
        let fixture = Fixture::new();
        let path = fixture.0.join("intent-only");
        fs::create_dir(&path).unwrap();
        let guard = ProjectDirectory::open_root(&path).unwrap();
        let id = backup_intent(&path, &guard, &"a".repeat(64)).unwrap();
        let bytes = serde_json::to_vec(&id).unwrap();
        let final_file = native::open_for_discard(&path.join("backup-intent.json")).unwrap();
        native::cleanup(&final_file).unwrap();
        drop(final_file);
        fs::write(
            path.join(pending_name("backup-intent.json", &bytes)),
            &bytes[..7],
        )
        .unwrap();
        assert_eq!(backup_intent(&path, &guard, &"a".repeat(64)).unwrap(), id);
        assert_eq!(
            read_exact(&path, &guard, "backup-intent.json").unwrap(),
            bytes
        );
        assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
        drop(guard);
    }
    #[test]
    fn deterministic_pending_transition_record_resumes_exact_prefix_and_preserves_foreign_bytes() {
        let fixture = Fixture::new();
        let root = fixture.0.join("stage");
        fs::create_dir(&root).unwrap();
        let guard = ProjectDirectory::open_root(&root).unwrap();
        let bytes = b"{\"logical intent\":\"exact preserved input\"}";
        let pending = pending_name("test.json", bytes);
        fs::write(root.join(&pending), &bytes[..9]).unwrap();
        stage_exact(&root, &guard, "test.json", bytes).unwrap();
        assert_eq!(fs::read(root.join("test.json")).unwrap(), bytes);
        assert!(!root.join(&pending).exists());
        fs::write(root.join(&pending), &bytes[..11]).unwrap();
        stage_exact(&root, &guard, "test.json", bytes).unwrap();
        assert!(!root.join(&pending).exists());
        let bad = pending_name("different.json", bytes);
        fs::write(root.join(&bad), b"foreign raw input").unwrap();
        assert!(stage_exact(&root, &guard, "different.json", bytes).is_err());
        assert_eq!(fs::read(root.join(&bad)).unwrap(), b"foreign raw input");
        assert!(!root.join("different.json").exists());
    }

    #[test]
    fn transition_raw_backup_handles_prevent_mutation_after_verification_until_retirement() {
        let fixture = Fixture::new();
        let root = fixture.0.join("project");
        fs::create_dir(&root).unwrap();
        let mut runtime = ProjectRuntime::acquire(&root, &fixture.0.join("locks")).unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let mut store = Store::open(&fixture.root()).unwrap();
        let mut envelope = sample();
        envelope.key.project_fingerprint = repository.write_project().fingerprint().into();
        store
            .accept(&Deposit::freeze(envelope.clone()).unwrap())
            .unwrap();
        let prepared = store.prepare_if_present(&repository).unwrap().unwrap();
        let verified = prepared.verify().unwrap();
        let leaf = Path::new(&prepared.backup.locator)
            .join("payload/.worldbuild/transition-inputs")
            .join(&envelope.key.draft_id)
            .join(format!("{}.json", envelope.key.generation));
        let before = fs::read(&leaf).unwrap();
        let held = verified.hold_preserved_inputs().unwrap();
        assert!(fs::write(&leaf, b"post-verification mutation").is_err());
        assert!(fs::remove_file(&leaf).is_err());
        drop(held);
        assert_eq!(fs::read(&leaf).unwrap(), before);
        drop(verified);
        prepared.finish(&mut store).unwrap();
        assert_eq!(fs::read(&leaf).unwrap(), before);
    }
}
