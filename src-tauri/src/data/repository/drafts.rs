//! One durable unsaved input per real artifact. No attachment-byte history.
use super::*;
use crate::data::edit_recovery::{
    model::{Deposit, Draft, Envelope},
    native,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(crate) const DIRECTORY: &str = "latest-drafts";
#[derive(PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    schema_version: u32,
    number: u64,
    payload_digest: String,
    envelope: Envelope,
}
#[derive(Clone, PartialEq)]
pub(crate) struct Checkpoint {
    id: ArtifactSourceId,
    number: u64,
    digest: String,
}
pub(crate) struct VerifiedCheckpoint {
    _file: fs::File,
    _directories: Vec<ProjectDirectory>,
}
impl Checkpoint {
    /// Cancellation may release an unresolved comparison only while the exact
    /// original input is still durable. This never acknowledges a new body.
    pub(crate) fn hold_current(
        &self,
        repository: &ArtifactRepository<'_, '_>,
    ) -> io::Result<VerifiedCheckpoint> {
        let (path, guards) =
            directory(repository.write_project().canonical_root(), self.id, false)?
                .ok_or_else(|| invalid("comparison input missing"))?;
        let guard = guards
            .last()
            .ok_or_else(|| invalid("comparison input directory missing"))?;
        if entries(guard)?.last().copied() != Some(self.number) {
            return Err(invalid("comparison input replaced"));
        }
        let name = filename(self.number);
        let mut file = project_file::open_existing_private_file(&path, &path.join(&name))?;
        let record = read_record(
            &mut file,
            self.id,
            self.number,
            repository.write_project().fingerprint(),
        )?;
        if record.payload_digest != self.digest {
            return Err(invalid("comparison input changed"));
        }
        guard.validate_file(&file, &name)?;
        for guard in &guards {
            guard.validate()?;
        }
        Ok(VerifiedCheckpoint {
            _file: file,
            _directories: guards,
        })
    }
    pub(crate) fn matches_deposit(&self, deposit: &Deposit) -> bool {
        target(deposit.envelope()).is_ok_and(|id| id == self.id)
            && self.digest == deposit.payload_digest()
    }
}
pub(crate) fn single_target(envelope: &Envelope) -> bool {
    target(envelope).is_ok()
}

/// Constructed only from the actual project's lock. This capability can write
/// private input records, never canonical files or transaction plans.
pub(crate) struct InputSink {
    root: PathBuf,
    fingerprint: String,
    guard: ProjectDirectory,
    epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    admitted_epoch: u64,
}
impl InputSink {
    pub(crate) fn bind(
        project: &crate::data::transaction::LockedProject<'_>,
        epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) -> io::Result<Self> {
        let guard = ProjectDirectory::open_root(project.canonical_root())?;
        let admitted_epoch = epoch.load(std::sync::atomic::Ordering::Acquire);
        Ok(Self {
            root: project.canonical_root().to_path_buf(),
            fingerprint: project.fingerprint().into(),
            guard,
            epoch,
            admitted_epoch,
        })
    }
    /// Read-only admission of the actual owned targets. It does not prove that
    /// future writes will succeed; durable acceptance remains the write gate.
    pub(crate) fn validate_targets(&self, targets: &[ProjectRelativePath]) -> io::Result<()> {
        if self.epoch.load(std::sync::atomic::Ordering::Acquire) != self.admitted_epoch {
            return Err(invalid("latest input capability invalidated"));
        }
        self.guard.validate()?;
        for relative in targets {
            let text = relative.as_str();
            let id = if let Some(name) = text
                .strip_prefix("documents/")
                .and_then(|v| v.strip_suffix(".json"))
            {
                ArtifactSourceId::Document(name.parse().map_err(io::Error::other)?)
            } else if let Some(name) = text
                .strip_prefix("templates/")
                .and_then(|v| v.strip_suffix(".json"))
            {
                ArtifactSourceId::Template(name.parse().map_err(io::Error::other)?)
            } else {
                // Other canonical session targets have no single-artifact input store.
                continue;
            };
            if id.path().map_err(io::Error::other)?.as_str() != text {
                return Err(invalid("noncanonical latest input target"));
            }
            if let Some((path, guards)) = directory(&self.root, id, false)? {
                let guard = guards
                    .last()
                    .ok_or_else(|| invalid("missing target guard"))?;
                for entry in guard.read_dir()? {
                    let entry = entry?;
                    if entry.file_name().to_string_lossy().starts_with("pending-") {
                        return Err(invalid("unfinished target input requires recovery"));
                    }
                }
                for number in entries(guard)? {
                    read(&path, id, number, &self.fingerprint)?;
                }
                for guard in guards {
                    guard.validate()?;
                }
            }
        }
        self.guard.validate()?;
        if self.epoch.load(std::sync::atomic::Ordering::Acquire) != self.admitted_epoch {
            return Err(invalid("latest input capability invalidated"));
        }
        Ok(())
    }
    pub(crate) fn accept(
        &self,
        deposit: &Deposit,
        targets: &[ProjectRelativePath],
    ) -> Result<crate::data::edit_recovery::Proof, crate::data::edit_recovery::error::RecoveryError>
    {
        use crate::data::edit_recovery::error::{RecoveryError, Stage};
        let result = (|| {
            if self.epoch.load(std::sync::atomic::Ordering::Acquire) != self.admitted_epoch {
                return Err(invalid("latest input capability invalidated"));
            }
            self.guard.validate()?;
            let id = target(deposit.envelope())?;
            if !targets.contains(&id.path().map_err(io::Error::other)?) {
                return Err(invalid("latest input target is outside owned session"));
            }
            preserve_owned(&self.root, &self.fingerprint, deposit.envelope().clone())
        })()
        .map_err(|error| RecoveryError::io(Stage::Write, error))?;
        crate::data::edit_recovery::Proof::from_latest(result, deposit)
    }
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn target(envelope: &Envelope) -> io::Result<ArtifactSourceId> {
    match &envelope.draft {
        Draft::Template {
            template: Some(id), ..
        } => id
            .parse()
            .map(ArtifactSourceId::Template)
            .map_err(io::Error::other),
        Draft::Document {
            document: Some(id), ..
        }
        | Draft::AdmittedDocument { document: id, .. } => id
            .parse()
            .map(ArtifactSourceId::Document)
            .map_err(io::Error::other),
        _ => Err(invalid("latest input requires a single real artifact")),
    }
}
fn directory(
    root: &Path,
    id: ArtifactSourceId,
    create: bool,
) -> io::Result<Option<(PathBuf, Vec<ProjectDirectory>)>> {
    let target = match id {
        ArtifactSourceId::Template(id) => format!("template-{id}"),
        ArtifactSourceId::Document(id) => format!("document-{id}"),
        _ => return Err(invalid("invalid latest input target")),
    };
    let mut path = root.to_path_buf();
    let mut guards = vec![ProjectDirectory::open_root(root)?];
    for name in [".worldbuild", DIRECTORY, &target] {
        path.push(name);
        if create {
            match fs::create_dir(&path) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error),
            }
        } else if matches!(fs::symlink_metadata(&path), Err(error) if error.kind() == io::ErrorKind::NotFound)
        {
            return Ok(None);
        }
        guards.push(ProjectDirectory::open_root(&path)?);
    }
    Ok(Some((path, guards)))
}
fn filename(number: u64) -> String {
    format!("d{number:020}.json")
}
fn entries(guard: &ProjectDirectory) -> io::Result<Vec<u64>> {
    let mut numbers = Vec::new();
    for entry in guard.read_dir()? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| invalid("invalid latest input filename"))?;
        if name.starts_with("pending-") {
            continue;
        }
        let number = name
            .strip_prefix('d')
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|name| name.parse::<u64>().ok())
            .filter(|number| *number > 0 && filename(*number) == name)
            .ok_or_else(|| invalid("unknown latest input file; preserve it"))?;
        numbers.push(number);
        if numbers.len() > 4096 {
            return Err(invalid("latest input listing exceeds bound"));
        }
    }
    numbers.sort_unstable();
    Ok(numbers)
}
fn read(path: &Path, id: ArtifactSourceId, number: u64, fingerprint: &str) -> io::Result<Record> {
    let mut file = project_file::open_existing_private_file(path, &path.join(filename(number)))?;
    read_record(&mut file, id, number, fingerprint)
}
fn read_record(
    file: &mut fs::File,
    id: ArtifactSourceId,
    number: u64,
    fingerprint: &str,
) -> io::Result<Record> {
    let mut bytes = Vec::new();
    Read::by_ref(file)
        .take(34 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 34 * 1024 * 1024 {
        return Err(invalid("latest input exceeds bound"));
    }
    let record: Record = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let deposit = Deposit::freeze(record.envelope.clone()).map_err(io::Error::other)?;
    if record.schema_version != 1
        || record.number != number
        || target(&record.envelope)? != id
        || record.envelope.key.project_fingerprint != fingerprint
        || record.payload_digest != deposit.payload_digest()
    {
        return Err(invalid("latest input verification failed"));
    }
    Ok(record)
}
pub(crate) fn latest(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
) -> io::Result<Option<Envelope>> {
    Ok(latest_checkpoint(repository, id)?.map(|(envelope, _)| envelope))
}
pub(crate) fn latest_checkpoint(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
) -> io::Result<Option<(Envelope, Checkpoint)>> {
    let Some((path, guards)) = directory(repository.write_project().canonical_root(), id, false)?
    else {
        return Ok(None);
    };
    let numbers = entries(
        guards
            .last()
            .ok_or_else(|| invalid("latest input directory missing"))?,
    )?;
    numbers
        .last()
        .map(|number| {
            read(&path, id, *number, repository.write_project().fingerprint()).map(|record| {
                let proof = Checkpoint {
                    id,
                    number: *number,
                    digest: record.payload_digest,
                };
                (record.envelope, proof)
            })
        })
        .transpose()
}
pub(crate) fn preserve(
    repository: &ArtifactRepository<'_, '_>,
    envelope: Envelope,
) -> io::Result<Checkpoint> {
    preserve_locked(repository.write_project(), envelope)
}
/// The owned project lock permits only the private latest-input sink here.
/// It grants no canonical write or transaction access while recovery is pending.
pub(crate) fn preserve_locked(
    project: &crate::data::transaction::LockedProject<'_>,
    envelope: Envelope,
) -> io::Result<Checkpoint> {
    preserve_owned(project.canonical_root(), project.fingerprint(), envelope)
}
fn preserve_owned(
    root: &Path,
    fingerprint: &str,
    mut envelope: Envelope,
) -> io::Result<Checkpoint> {
    let id = target(&envelope)?;
    if envelope.key.project_fingerprint != fingerprint {
        return Err(invalid("latest input project mismatch"));
    }
    let (path, guards) =
        directory(root, id, true)?.ok_or_else(|| invalid("latest input directory missing"))?;
    let guard = guards
        .last()
        .ok_or_else(|| invalid("latest input directory missing"))?;
    let previous = entries(guard)?;
    let number = previous
        .last()
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| invalid("latest input number overflow"))?;
    // Unknown/corrupt earlier inputs require migration/repair, never blind pruning.
    let old = previous
        .iter()
        .map(|number| read(&path, id, *number, fingerprint))
        .collect::<io::Result<Vec<_>>>()?;
    if let Some(latest) = old.last() {
        if let Some(residual) = &latest.envelope.residual {
            if envelope.residual.as_ref() != Some(residual)
                && envelope.residual_ack.as_deref()
                    != Some(residual.digest().map_err(io::Error::other)?.as_str())
            {
                return Err(invalid(
                    "latest input cannot erase unapplied content without exact acknowledgement",
                ));
            }
        }
        // A newer raw input must retain unresolved writes until their actual outcome is known.
        // Copying the metadata keeps the newest body without discarding the older transaction proof.
        if let Some(previous) = latest
            .envelope
            .attempt
            .as_ref()
            .filter(|attempt| attempt.unresolved())
        {
            match &envelope.attempt {
                None => {
                    if previous.submitted_generation > envelope.key.generation {
                        return Err(invalid("unresolved input belongs to a newer generation"));
                    }
                    envelope.attempt = Some(previous.clone());
                }
                Some(next) if previous.operation_id != next.operation_id => {
                    return Err(invalid("latest input has an unresolved previous write"))
                }
                Some(_) => (),
            }
        }
        if latest.envelope.key.draft_id == envelope.key.draft_id {
            if envelope.key.generation < latest.envelope.key.generation {
                return Err(invalid("stale latest input generation"));
            }
            if envelope.key.generation == latest.envelope.key.generation {
                if envelope.draft != latest.envelope.draft
                    || envelope.originals != latest.envelope.originals
                {
                    return Err(invalid("same generation has different raw input"));
                }
                if latest.envelope == envelope {
                    for previous in old.iter().take(old.len().saturating_sub(1)) {
                        remove_verified(&path, guard, previous)?;
                    }
                    return Ok(Checkpoint {
                        id,
                        number: latest.number,
                        digest: latest.payload_digest.clone(),
                    });
                }
                if latest.envelope.attempt.is_some() && envelope.attempt.is_none() {
                    return Err(invalid(
                        "latest input acknowledgement cannot erase save outcome",
                    ));
                }
                if let (Some(previous), Some(next)) = (&latest.envelope.attempt, &envelope.attempt)
                {
                    if previous.operation_id != next.operation_id && previous.unresolved() {
                        return Err(invalid("latest input has an unresolved previous write"));
                    }
                }
            }
        }
    }
    let deposit = Deposit::freeze(envelope.clone()).map_err(io::Error::other)?;
    let record = Record {
        schema_version: 1,
        number,
        payload_digest: deposit.payload_digest().into(),
        envelope,
    };
    let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    if bytes.len() > 34 * 1024 * 1024 {
        return Err(invalid("latest input exceeds bound"));
    }
    let pending = format!("pending-{}", uuid::Uuid::new_v4());
    let mut file = native::open(&path.join(&pending), true, true)?;
    let result = (|| {
        guard.validate_file(&file, &pending)?;
        checkpoint("before-write")?;
        file.write_all(&bytes)?;
        file.flush()?;
        file.sync_all()?;
        checkpoint("before-publish")?;
        native::publish(&file, &filename(number))?;
        guard.validate_file(&file, &filename(number))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = native::cleanup(&file);
        return Err(error);
    }
    drop(file);
    checkpoint("after-publish")?;
    if read(&path, id, number, fingerprint)? != record {
        return Err(invalid("latest input differs after publish"));
    }
    for record in old {
        remove_verified(&path, guard, &record)?;
    }
    Ok(Checkpoint {
        id,
        number,
        digest: record.payload_digest,
    })
}
fn remove_verified(path: &Path, guard: &ProjectDirectory, expected: &Record) -> io::Result<()> {
    let name = filename(expected.number);
    let mut file = native::open_for_discard(&path.join(&name))?;
    guard.validate_file(&file, &name)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(34 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    let actual: Record = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if actual != *expected {
        return Err(invalid("latest input changed before cleanup"));
    }
    checkpoint("before-cleanup")?;
    native::cleanup(&file)
}
/// Only a newly materialized, byte-verified private snapshot may call this.
/// Ordinary project opening never accepts a foreign fingerprint. The canonical
/// destination is not published until every old-bound record is validated and
/// re-encoded for that new project, leaving the source/backup unchanged.
pub(crate) fn rebind_verified_snapshot(
    root: &Path,
    old_fingerprint: &str,
    new_fingerprint: &str,
) -> io::Result<()> {
    if !crate::data::edit_recovery::model::valid_digest(old_fingerprint)
        || !crate::data::edit_recovery::model::valid_digest(new_fingerprint)
    {
        return Err(invalid("snapshot project identity invalid"));
    }
    let namespace = root.join(".worldbuild").join(DIRECTORY);
    let guard = match ProjectDirectory::open_root(&namespace) {
        Ok(guard) => guard,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut plan = Vec::new();
    for entry in guard.read_dir()? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| invalid("invalid copied input namespace"))?;
        let id = name
            .strip_prefix("template-")
            .map(|id| id.parse().map(ArtifactSourceId::Template))
            .or_else(|| {
                name.strip_prefix("document-")
                    .map(|id| id.parse().map(ArtifactSourceId::Document))
            })
            .ok_or_else(|| invalid("unknown copied input namespace"))?
            .map_err(io::Error::other)?;
        let path = namespace.join(&name);
        let child = ProjectDirectory::open_root(&path)?;
        for entry in child.read_dir()? {
            let name = entry?
                .file_name()
                .into_string()
                .map_err(|_| invalid("invalid copied input filename"))?;
            if name.starts_with("pending-") {
                return Err(invalid("snapshot contains unfinished input"));
            }
        }
        for number in entries(&child)? {
            let mut record = read(&path, id, number, old_fingerprint)?;
            let previous = serde_json::to_vec(&record).map_err(io::Error::other)?;
            record.envelope.key.project_fingerprint = new_fingerprint.into();
            record.payload_digest = Deposit::freeze(record.envelope.clone())
                .map_err(io::Error::other)?
                .payload_digest()
                .into();
            let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
            plan.push((path.clone(), id, number, previous, bytes));
        }
    }
    guard.validate()?;
    for (path, id, number, previous, bytes) in plan {
        let child = ProjectDirectory::open_root(&path)?;
        if serde_json::to_vec(&read(&path, id, number, old_fingerprint)?)
            .map_err(io::Error::other)?
            != previous
        {
            return Err(invalid("copied input changed before rebinding"));
        }
        let pending = format!("pending-{}", uuid::Uuid::new_v4());
        let mut file = native::open(&path.join(&pending), true, true)?;
        let result = (|| {
            child.validate_file(&file, &pending)?;
            file.write_all(&bytes)?;
            file.flush()?;
            file.sync_all()?;
            child.validate()?;
            crate::data::atomic_file::owned::rename(&file, &filename(number), true)?;
            child.validate_file(&file, &filename(number))
        })();
        if let Err(error) = result {
            let _ = native::cleanup(&file);
            return Err(error);
        }
        drop(file);
        if serde_json::to_vec(&read(&path, id, number, new_fingerprint)?)
            .map_err(io::Error::other)?
            != bytes
        {
            return Err(invalid("copied input verification failed"));
        }
    }
    guard.validate()
}

/// A no-change acknowledgement must stay tied to the source used for comparison.
/// These handles deny write/delete sharing until the exact checkpoint is cleared.
pub(crate) fn clear_if_sources_match(
    repository: &ArtifactRepository<'_, '_>,
    expected: &Checkpoint,
    sources: &[&SourceToken],
) -> io::Result<bool> {
    let mut held = Vec::new();
    for source in sources {
        let mut file = project_file::open_existing_project_file(
            repository.write_project().canonical_root(),
            source.path(),
        )?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() != source.byte_length()
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != *source.sha256()
        {
            return Err(invalid(
                "latest input comparison source changed; retain input",
            ));
        }
        held.push(file);
    }
    let result = clear(repository, expected);
    drop(held);
    result
}
pub(crate) fn clear(
    repository: &ArtifactRepository<'_, '_>,
    expected: &Checkpoint,
) -> io::Result<bool> {
    let Some((path, guards)) = directory(
        repository.write_project().canonical_root(),
        expected.id,
        false,
    )?
    else {
        return Ok(false);
    };
    let guard = guards
        .last()
        .ok_or_else(|| invalid("latest input directory missing"))?;
    if entries(guard)?.last().copied() != Some(expected.number) {
        return Ok(false);
    }
    let record = read(
        &path,
        expected.id,
        expected.number,
        repository.write_project().fingerprint(),
    )?;
    if record.payload_digest != expected.digest {
        return Err(invalid("latest input changed before acknowledgement"));
    }
    if record.envelope.residual.is_some() {
        return Ok(false);
    }
    remove_verified(&path, guard, &record)?;
    Ok(true)
}
#[cfg(test)]
thread_local! { static FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }
fn checkpoint(_point: &'static str) -> io::Result<()> {
    #[cfg(test)]
    if FAULT.with(|fault| {
        if fault.get() == Some(_point) {
            fault.set(None);
            true
        } else {
            false
        }
    }) {
        return Err(io::Error::other("injected latest input failure"));
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::super::tests::{template_bytes, Fixture, TEMPLATE};
    use super::*;
    use crate::data::{
        edit_recovery::model::{Intent, Key, Original, OriginalKind},
        project_runtime::ProjectRuntime,
    };
    #[test]
    #[ignore = "explicit owned GUI input preparation; not GUI acceptance"]
    fn v1_data_safety_003_prepare_owned_gui_input() {
        let root = PathBuf::from(std::env::var_os("WB_UX003_GUI_PROJECT").expect("owned project"));
        let envelope_path = PathBuf::from(
            std::env::var_os("WB_UX003_GUI_ENVELOPE").expect("owned input preparation"),
        );
        let owned = PathBuf::from(std::env::var_os("WB_UX003_GUI_BASE").expect("owned base"));
        let owned = fs::canonicalize(owned).unwrap();
        assert!(fs::canonicalize(&root).unwrap().starts_with(&owned));
        assert!(fs::canonicalize(&envelope_path)
            .unwrap()
            .starts_with(&owned));
        let mut envelope: Envelope =
            serde_json::from_slice(&fs::read(envelope_path).unwrap()).unwrap();
        let mut runtime = ProjectRuntime::acquire(&root, &owned.join("preparation-locks")).unwrap();
        runtime.recover().unwrap();
        envelope.key.project_fingerprint = runtime
            .ready()
            .unwrap()
            .locked_project()
            .fingerprint()
            .into();
        let frozen = Deposit::freeze(envelope.clone()).unwrap();
        assert!(Deposit::decode(frozen.bytes()).unwrap().envelope() == &envelope);
        if let Some(store_root) = std::env::var_os("WB_UX003_GUI_LEGACY_STORE") {
            let store_root = fs::canonicalize(PathBuf::from(store_root)).unwrap();
            assert!(store_root.starts_with(&owned));
            runtime.close().unwrap();
            crate::data::edit_recovery::Store::open(&store_root)
                .unwrap()
                .accept(&frozen)
                .unwrap();
            println!(
                "Owned legacy input prepared with production validation; GUI not yet performed"
            );
        } else {
            runtime.preserve_latest_input(envelope).unwrap();
            runtime.close().unwrap();
            println!(
                "Owned latest input prepared with production validation; GUI not yet performed"
            );
        }
    }
    fn envelope(repository: &ArtifactRepository<'_, '_>, name: &str) -> Envelope {
        let bytes = template_bytes(TEMPLATE, "active");
        let artifact = artifact::decode_template(&bytes).unwrap();
        let snapshot = String::from_utf8(bytes.clone()).unwrap();
        Envelope {
            residual: None,
            residual_ack: None,
            key: Key {
                project_fingerprint: repository.write_project().fingerprint().into(),
                draft_id: uuid::Uuid::new_v4().to_string(),
                generation: 2,
            },
            deposit_id: uuid::Uuid::new_v4().to_string(),
            app_version: "1.0.1".into(),
            created_at_utc: "2026-10-04T01:02:03.004Z".into(),
            attempt: None,
            originals: vec![Original {
                kind: OriginalKind::Template,
                artifact_id: TEMPLATE.into(),
                schema: 1,
                template_revision: artifact.revision().get(),
                source_byte_length: bytes.len() as u64,
                source_digest: crate::data::edit_recovery::model::digest(&bytes),
                snapshot_digest: crate::data::edit_recovery::model::digest(&bytes),
                snapshot,
            }],
            draft: Draft::Template {
                sections: vec![],
                template: Some(TEMPLATE.into()),
                name: name.into(),
                glossary_excluded: false,
                presentation: Intent::Keep,
                fields: vec![],
                composing: false,
            },
        }
    }
    #[test]
    fn target_validation_is_read_only_strict_repeatable_and_epoch_bound() {
        let fixture = Fixture::new().unwrap();
        let mut runtime = ProjectRuntime::acquire(
            &fixture.root,
            &fixture.root.parent().unwrap().join("draft-locks"),
        )
        .unwrap();
        runtime.recover().unwrap();
        let epoch = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let sink = runtime.latest_input_sink(epoch.clone()).unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        let targets = [id.path().unwrap()];
        assert!(sink.validate_targets(&targets).is_ok());
        assert!(!fixture.root.join(".worldbuild").join(DIRECTORY).exists());
        let deposit =
            Deposit::freeze(envelope(&repository, "verified input survives validation")).unwrap();
        sink.accept(&deposit, &targets).unwrap();
        let (path, guards) = directory(&fixture.root, id, false).unwrap().unwrap();
        let number = entries(guards.last().unwrap()).unwrap()[0];
        let original = fs::read(path.join(filename(number))).unwrap();
        for marker in ["foreign-input.json", "pending-incomplete.json"] {
            let marker = path.join(marker);
            fs::write(&marker, b"do not prune this owned obstacle").unwrap();
            assert!(sink.validate_targets(&targets).is_err());
            assert_eq!(
                fs::read(&marker).unwrap(),
                b"do not prune this owned obstacle"
            );
            assert_eq!(fs::read(path.join(filename(number))).unwrap(), original);
            fs::remove_file(marker).unwrap();
            assert!(sink.validate_targets(&targets).is_ok());
        }
        let mut corrupt: serde_json::Value = serde_json::from_slice(&original).unwrap();
        corrupt["payloadDigest"] = serde_json::json!("0".repeat(64));
        fs::write(
            path.join(filename(number)),
            serde_json::to_vec(&corrupt).unwrap(),
        )
        .unwrap();
        assert!(sink.validate_targets(&targets).is_err());
        fs::write(path.join(filename(number)), &original).unwrap();
        assert!(sink.validate_targets(&targets).is_ok());
        drop(guards);
        let alias = path.join("d00000000000000009999.json");
        fs::hard_link(path.join(filename(number)), &alias).unwrap();
        assert!(sink.validate_targets(&targets).is_err());
        fs::remove_file(alias).unwrap();
        assert!(sink.validate_targets(&targets).is_ok());
        epoch.store(2, std::sync::atomic::Ordering::Release);
        assert!(sink.validate_targets(&targets).is_err());
        assert_eq!(fs::read(path.join(filename(number))).unwrap(), original);
    }
    #[test]
    fn owned_input_sink_returns_exact_receipts_and_external_replacement_revokes_access() {
        let fixture = Fixture::new().unwrap();
        let mut runtime = ProjectRuntime::acquire(
            &fixture.root,
            &fixture.root.parent().unwrap().join("draft-locks"),
        )
        .unwrap();
        runtime.recover().unwrap();
        let epoch = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let sink = runtime.latest_input_sink(epoch.clone()).unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        let targets = [id.path().unwrap()];
        let mut input = envelope(&repository, "같은 입력");
        let first = Deposit::freeze(input.clone()).unwrap();
        let first_proof = sink.accept(&first, &targets).unwrap();
        assert!(first_proof.matches(
            first.key(),
            &first.envelope().deposit_id,
            first.payload_digest()
        ));
        input.deposit_id = uuid::Uuid::new_v4().to_string();
        input.created_at_utc = "2026-10-04T01:02:04.004Z".into();
        let second = Deposit::freeze(input).unwrap();
        let second_proof = sink.accept(&second, &targets).unwrap();
        assert!(second_proof.matches(
            second.key(),
            &second.envelope().deposit_id,
            second.payload_digest()
        ));
        assert!(!first_proof.matches(
            second.key(),
            &second.envelope().deposit_id,
            second.payload_digest()
        ));
        let (path, guards) = directory(&fixture.root, id, false).unwrap().unwrap();
        assert_eq!(entries(guards.last().unwrap()).unwrap().len(), 1);
        assert!(latest(&repository, id).unwrap().unwrap() == *second.envelope());
        assert!(sink.accept(&second, &[]).is_err());
        epoch.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        assert!(sink.accept(&second, &targets).is_err());
        assert_eq!(
            read(
                &path,
                id,
                entries(guards.last().unwrap()).unwrap()[0],
                repository.write_project().fingerprint()
            )
            .unwrap()
            .payload_digest,
            second.payload_digest()
        );
    }
    #[test]
    #[cfg(windows)]
    fn comparison_checkpoint_holds_input_and_ancestors_until_owner_release() {
        let fixture = Fixture::new().unwrap();
        let mut runtime = ProjectRuntime::acquire(
            &fixture.root,
            &fixture.root.parent().unwrap().join("comparison-locks"),
        )
        .unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let input = envelope(&repository, "원래 미저장 입력");
        let checkpoint = preserve(&repository, input).unwrap();
        let held = checkpoint.hold_current(&repository).unwrap();
        let (folder, _) = directory(&fixture.root, checkpoint.id, false)
            .unwrap()
            .unwrap();
        let file = folder.join(filename(checkpoint.number));
        let before = fs::read(&file).unwrap();
        assert!(fs::write(&file, b"changed after verification").is_err());
        assert!(fs::remove_file(&file).is_err());
        assert!(fs::rename(&file, folder.join("replaced.json")).is_err());
        assert!(fs::rename(&folder, folder.with_extension("replaced")).is_err());
        assert_eq!(fs::read(&file).unwrap(), before);
        drop(held);
        fs::write(&file, b"invalid input after release").unwrap();
        assert!(checkpoint.hold_current(&repository).is_err());
        fs::write(&file, before).unwrap();
        let _held = checkpoint.hold_current(&repository).unwrap();
    }
    #[test]
    fn latest_input_is_one_per_target_and_late_ack_cannot_clear_newer_input() {
        let fixture = Fixture::new().unwrap();
        let mut runtime = ProjectRuntime::acquire(
            &fixture.root,
            &fixture.root.parent().unwrap().join("draft-locks"),
        )
        .unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        let mut input = envelope(&repository, "초안 A");
        input.key.generation = 1;
        let first = preserve(&repository, input.clone()).unwrap();
        input.key.generation = 2;
        if let Draft::Template { name, .. } = &mut input.draft {
            *name = "초안 B".into();
        }
        let second = preserve(&repository, input.clone()).unwrap();
        input.key.generation = 1;
        if let Draft::Template { name, .. } = &mut input.draft {
            *name = "초안 A".into();
        }
        assert!(preserve(&repository, input).is_err());
        assert!(!clear(&repository, &first).unwrap());
        let (path, guards) = directory(&fixture.root, id, false).unwrap().unwrap();
        assert_eq!(entries(guards.last().unwrap()).unwrap().len(), 1);
        assert_eq!(second.number, 2);
        let loaded = latest(&repository, id).unwrap().unwrap();
        assert!(matches!(loaded.draft, Draft::Template { name, .. } if name == "초안 B"));
        assert_eq!(
            read(
                &path,
                id,
                second.number,
                repository.write_project().fingerprint()
            )
            .unwrap()
            .payload_digest,
            second.digest
        );
        assert!(clear(&repository, &second).unwrap());
        assert!(latest(&repository, id).unwrap().is_none());
    }
    #[test]
    fn latest_no_change_acknowledgement_retains_input_when_comparison_source_changes() {
        let fixture = Fixture::new().unwrap();
        let mut runtime = ProjectRuntime::acquire(
            &fixture.root,
            &fixture.root.parent().unwrap().join("draft-locks"),
        )
        .unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        fs::create_dir(fixture.root.join("templates")).unwrap();
        fs::write(
            fixture.root.join(id.path().unwrap().as_str()),
            template_bytes(TEMPLATE, "active"),
        )
        .unwrap();
        let input = envelope(&repository, "입력 유지");
        let proof = preserve(&repository, input.clone()).unwrap();
        let loaded = repository.load_template(TEMPLATE.parse().unwrap()).unwrap();
        let path = fixture.root.join(id.path().unwrap().as_str());
        let mut changed: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        changed["name"] = "다른 현재 이름".into();
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(clear_if_sources_match(&repository, &proof, &[loaded.source()]).is_err());
        assert!(latest(&repository, id).unwrap().unwrap() == input);
        let current = repository.load_template(TEMPLATE.parse().unwrap()).unwrap();
        assert!(clear_if_sources_match(&repository, &proof, &[current.source()]).unwrap());
        assert!(latest(&repository, id).unwrap().is_none());
    }
    #[test]
    fn newer_raw_input_keeps_unresolved_write_and_allows_verified_outcome_update() {
        use crate::data::edit_recovery::model::{Attempt, SaveState};
        let fixture = Fixture::new().unwrap();
        let mut runtime = ProjectRuntime::acquire(
            &fixture.root,
            &fixture.root.parent().unwrap().join("draft-locks"),
        )
        .unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        let mut input = envelope(&repository, "초안 A");
        let attempt = Attempt {
            submitted_generation: 2,
            recovery_checked: false,
            operation_id: uuid::Uuid::new_v4().to_string(),
            result: SaveState::Uncertain,
            candidate_digest: None,
            transaction_id: None,
        };
        input.attempt = Some(attempt.clone());
        let first = preserve(&repository, input.clone()).unwrap();
        input.key.generation = 3;
        input.attempt = None;
        if let Draft::Template { name, .. } = &mut input.draft {
            *name = "초안 B".into();
        }
        preserve(&repository, input.clone()).unwrap();
        let restored = latest(&repository, id).unwrap().unwrap();
        assert!(matches!(&restored.draft, Draft::Template { name, .. } if name == "초안 B"));
        assert!(restored.attempt == Some(attempt.clone()));
        assert!(!clear(&repository, &first).unwrap());
        input.attempt = Some(Attempt {
            result: SaveState::Committed,
            ..attempt
        });
        preserve(&repository, input.clone()).unwrap();
        assert!(latest(&repository, id).unwrap().unwrap().attempt == input.attempt);
        let (_, guards) = directory(&fixture.root, id, false).unwrap().unwrap();
        assert_eq!(entries(guards.last().unwrap()).unwrap().len(), 1);
    }
    #[test]
    fn failed_latest_publication_preserves_previous_and_cleanup_failure_is_retryable() {
        let fixture = Fixture::new().unwrap();
        let mut runtime = ProjectRuntime::acquire(
            &fixture.root,
            &fixture.root.parent().unwrap().join("draft-locks"),
        )
        .unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        preserve(&repository, envelope(&repository, "초안 A")).unwrap();
        FAULT.with(|fault| fault.set(Some("before-publish")));
        assert!(preserve(&repository, envelope(&repository, "초안 B")).is_err());
        assert!(
            matches!(latest(&repository, id).unwrap().unwrap().draft, Draft::Template { name, .. } if name == "초안 A")
        );
        FAULT.with(|fault| fault.set(Some("before-cleanup")));
        assert!(preserve(&repository, envelope(&repository, "초안 B")).is_err());
        assert!(
            matches!(latest(&repository, id).unwrap().unwrap().draft, Draft::Template { name, .. } if name == "초안 B")
        );
        preserve(&repository, envelope(&repository, "초안 B")).unwrap();
        let (_, guards) = directory(&fixture.root, id, false).unwrap().unwrap();
        assert_eq!(entries(guards.last().unwrap()).unwrap().len(), 1);
    }
    #[test]
    fn verified_copy_and_backup_new_project_keep_unsaved_input_and_source_binding() {
        use crate::data::project_backup::{self, BackupKind};
        let fixture = Fixture::new().unwrap();
        let parent = fixture.root.parent().unwrap();
        let mut runtime =
            ProjectRuntime::acquire(&fixture.root, &parent.join("draft-locks")).unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        let input = envelope(&repository, "복사할 미저장 내용");
        preserve(&repository, input.clone()).unwrap();
        fs::create_dir(fixture.root.join("templates")).unwrap();
        fs::write(
            fixture.root.join(id.path().unwrap().as_str()),
            template_bytes(TEMPLATE, "active"),
        )
        .unwrap();
        let (old_path, guards) = directory(&fixture.root, id, false).unwrap().unwrap();
        let old_number = entries(guards.last().unwrap()).unwrap()[0];
        let before = fs::read(old_path.join(filename(old_number))).unwrap();
        fs::create_dir(parent.join("backup-storage")).unwrap();
        fs::create_dir(parent.join("copies")).unwrap();
        let backup = project_backup::create_backup(
            &fixture.root,
            repository.write_project().fingerprint(),
            &parent.join("backup-storage"),
            None,
            BackupKind::Manual,
        )
        .unwrap();
        let copied =
            project_backup::export_project(&fixture.root, &parent.join("copies"), "사본").unwrap();
        assert_eq!(copied.outcome, "published_verified");
        let restored = project_backup::restore_new(
            Path::new(&backup.locator),
            &parent.join("copies"),
            "백업 복원",
        )
        .unwrap();
        assert_eq!(restored.outcome, "published_verified");
        for snapshot in [copied, restored] {
            let mut other =
                ProjectRuntime::acquire(&snapshot.root, &parent.join("draft-locks")).unwrap();
            other.recover().unwrap();
            let other_ready = other.ready().unwrap();
            let other_repo = ArtifactRepository::new(&other_ready).unwrap();
            let actual = latest(&other_repo, id).unwrap().unwrap();
            assert!(
                actual.draft == input.draft
                    && actual.originals == input.originals
                    && actual.attempt == input.attempt
            );
            assert_ne!(
                actual.key.project_fingerprint,
                input.key.project_fingerprint
            );
            assert_eq!(
                actual.key.project_fingerprint,
                other_repo.write_project().fingerprint()
            );
            assert!(Deposit::freeze(actual).is_ok());
        }
        assert_eq!(
            fs::read(old_path.join(filename(old_number))).unwrap(),
            before
        );
        assert!(latest(&repository, id).unwrap().unwrap() == input);
        // Opening a manually copied foreign-bound record is not an adoption path.
        let foreign = parent.join("foreign");
        fs::create_dir(&foreign).unwrap();
        let (foreign_path, _) = directory(&foreign, id, true).unwrap().unwrap();
        fs::write(foreign_path.join(filename(old_number)), &before).unwrap();
        let mut other = ProjectRuntime::acquire(&foreign, &parent.join("draft-locks")).unwrap();
        other.recover().unwrap();
        let other_ready = other.ready().unwrap();
        assert!(latest(&ArtifactRepository::new(&other_ready).unwrap(), id).is_err());
        assert_eq!(
            fs::read(foreign_path.join(filename(old_number))).unwrap(),
            before
        );
    }

    #[test]
    fn copied_latest_input_reserves_pending_space_before_publication() {
        use crate::data::storage_estimate::test_support::{
            with_storage_response, TestStorageResponse,
        };
        let fixture = Fixture::new().unwrap();
        let parent = fixture.root.parent().unwrap();
        let mut runtime =
            ProjectRuntime::acquire(&fixture.root, &parent.join("draft-locks")).unwrap();
        runtime.recover().unwrap();
        let ready = runtime.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        preserve(&repository, envelope(&repository, "보존된 입력")).unwrap();
        let before = latest(&repository, id).unwrap().unwrap();
        let (result, queries) = with_storage_response(
            TestStorageResponse::Available {
                available_bytes: 32 * 1024 * 1024,
                allocation_unit_bytes: 4096,
            },
            || crate::data::project_backup::export_project(&fixture.root, parent, "공간 부족 사본"),
        );
        assert!(result.is_err());
        assert_eq!(queries.len(), 1);
        assert!(!parent.join("공간 부족 사본").exists());
        assert!(!fs::read_dir(parent).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".worldbuild-copy-")));
        assert!(latest(&repository, id).unwrap().unwrap() == before);
    }
}
