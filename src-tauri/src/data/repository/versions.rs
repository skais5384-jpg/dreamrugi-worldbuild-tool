//! Bounded confirmed content versions. Attachment bytes are never copied here.
use super::*;
use crate::data::edit_recovery::native;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) const DIRECTORY: &str = "content-versions";
const LIMIT: usize = 10;

#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    schema_version: u32,
    version: u64,
    artifact: String,
    recorded_at_utc: String,
    content_digest: String,
    semantic_digest: String,
    content: String,
    template: Option<String>,
    template_digest: Option<String>,
    #[serde(default)]
    asset_names: BTreeMap<String, String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct Version {
    pub(crate) version: String,
    pub(crate) recorded_at_utc: String,
    pub(crate) available: bool,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn semantic(bytes: &[u8]) -> io::Result<String> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| invalid("version content is not an artifact"))?;
    for key in [
        "createdAtUtc",
        "updatedAtUtc",
        "revision",
        "templateRevision",
        "schemaVersion",
    ] {
        object.remove(key);
    }
    Ok(digest(
        &serde_json::to_vec(&value).map_err(io::Error::other)?,
    ))
}
fn target(id: ArtifactSourceId) -> io::Result<String> {
    match id {
        ArtifactSourceId::Template(id) => Ok(format!("template-{id}")),
        ArtifactSourceId::Document(id) => Ok(format!("document-{id}")),
        _ => Err(invalid("layout is not a content version target")),
    }
}
fn validate_content(id: ArtifactSourceId, bytes: &[u8]) -> io::Result<()> {
    let actual = match id {
        ArtifactSourceId::Template(_) => ArtifactSourceId::Template(
            artifact::decode_template(bytes)
                .map_err(io::Error::other)?
                .template_id(),
        ),
        ArtifactSourceId::Document(_) => ArtifactSourceId::Document(
            artifact::decode_document(bytes)
                .map_err(io::Error::other)?
                .document_id(),
        ),
        _ => return Err(invalid("invalid content version target")),
    };
    if actual != id {
        return Err(invalid("content version identity mismatch"));
    }
    Ok(())
}
fn directory(
    root: &Path,
    id: ArtifactSourceId,
    create: bool,
) -> io::Result<Option<(PathBuf, Vec<ProjectDirectory>)>> {
    let mut path = root.to_owned();
    let mut guards = vec![ProjectDirectory::open_root(root)?];
    let target = target(id)?;
    for name in [".worldbuild", DIRECTORY, target.as_str()] {
        path.push(name);
        if create {
            match fs::create_dir(&path) {
                Ok(()) => (),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e),
            }
        } else if matches!(fs::symlink_metadata(&path), Err(e) if e.kind() == io::ErrorKind::NotFound)
        {
            return Ok(None);
        }
        guards.push(ProjectDirectory::open_root(&path)?);
    }
    Ok(Some((path, guards)))
}
fn name(version: u64) -> String {
    format!("v{version:020}.json")
}
fn number(value: &str) -> Option<u64> {
    let version = value
        .strip_prefix('v')?
        .strip_suffix(".json")?
        .parse::<u64>()
        .ok()?;
    (version != 0 && name(version) == value).then_some(version)
}
fn entries(guard: &ProjectDirectory) -> io::Result<Vec<(u64, String)>> {
    let mut result = Vec::new();
    for entry in guard.read_dir()? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("noncanonical version entry"))?;
        if let Some(version) = number(&name) {
            result.push((version, name));
        } else if !name.starts_with("pending-") {
            return Err(invalid("unexpected version entry; preserve files"));
        }
    }
    result.sort_by_key(|(version, _)| *version);
    Ok(result)
}
fn read(path: &Path, id: ArtifactSourceId, version: u64) -> io::Result<Record> {
    let mut file = project_file::open_existing_private_file(path, &path.join(name(version)))?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(invalid("version record too large"));
    }
    let record: Record = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if record.schema_version != 1
        || record.version != version
        || record.artifact != target(id)?
        || record.content_digest != digest(record.content.as_bytes())
        || record.semantic_digest != semantic(record.content.as_bytes())?
    {
        return Err(invalid("version record verification failed"));
    }
    validate_content(id, record.content.as_bytes())?;
    let raw = crate::data::json::parse_strict_json_object(record.content.as_bytes())
        .map_err(io::Error::other)?;
    let references = crate::data::assets::references(&raw).map_err(io::Error::other)?;
    if record.asset_names.len() > 100_000
        || record.asset_names.iter().any(|(asset, name)| {
            !references.contains(asset)
                || !crate::data::media::valid_id(asset)
                || !crate::data::assets::safe_name(name)
        })
    {
        return Err(invalid(
            "version display metadata is not bound to its content",
        ));
    }
    match (id, &record.template, &record.template_digest) {
        (ArtifactSourceId::Document(_), Some(template), Some(expected)) => {
            if digest(template.as_bytes()) != *expected {
                return Err(invalid("version template digest mismatch"));
            }
            let document =
                artifact::decode_document(record.content.as_bytes()).map_err(io::Error::other)?;
            let template =
                artifact::decode_template(template.as_bytes()).map_err(io::Error::other)?;
            if document.template_id() != template.template_id() {
                return Err(invalid("version template identity mismatch"));
            }
        }
        (ArtifactSourceId::Template(_) | ArtifactSourceId::Document(_), None, None) => (),
        _ => {
            return Err(invalid(
                "version interpretation metadata missing or invalid",
            ))
        }
    }
    Ok(record)
}

/// Does not read the canonical file: a missing/corrupt current source cannot hide versions.
pub(crate) fn list(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
) -> io::Result<Vec<Version>> {
    let Some((path, guards)) = directory(repository.write_project().canonical_root(), id, false)?
    else {
        return Ok(vec![]);
    };
    let guard = guards
        .last()
        .ok_or_else(|| invalid("version directory unavailable"))?;
    let mut result = entries(guard)?
        .into_iter()
        .rev()
        .map(|(version, _)| match read(&path, id, version) {
            Ok(record) => Version {
                version: version.to_string(),
                recorded_at_utc: record.recorded_at_utc,
                available: !matches!(id, ArtifactSourceId::Document(_))
                    || record.template.is_some(),
            },
            Err(_) => Version {
                version: version.to_string(),
                recorded_at_utc: String::new(),
                available: false,
            },
        })
        .collect::<Vec<_>>();
    result.truncate(LIMIT);
    Ok(result)
}

pub(crate) fn snapshot(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    version: u64,
) -> io::Result<(Vec<u8>, Option<Vec<u8>>)> {
    let (path, _guards) = directory(repository.write_project().canonical_root(), id, false)?
        .ok_or_else(|| invalid("version unavailable"))?;
    let record = read(&path, id, version)?;
    if matches!(id, ArtifactSourceId::Document(_)) && record.template.is_none() {
        return Err(invalid(
            "legacy interpretation metadata unavailable; use preserved whole backup",
        ));
    }
    Ok((
        record.content.into_bytes(),
        record.template.map(String::into_bytes),
    ))
}

/// Read-only discovery for ordinary lists. These identities grant no write authority.
pub(crate) fn targets(
    repository: &ArtifactRepository<'_, '_>,
) -> io::Result<Vec<ArtifactSourceId>> {
    let root = repository.write_project().canonical_root();
    let base = root.join(".worldbuild");
    let path = base.join(DIRECTORY);
    if matches!(fs::symlink_metadata(&path), Err(error) if error.kind() == io::ErrorKind::NotFound)
    {
        return Ok(vec![]);
    }
    let _root = ProjectDirectory::open_root(root)?;
    let _base = ProjectDirectory::open_root(&base)?;
    let guard = ProjectDirectory::open_root(&path)?;
    let mut ids = Vec::new();
    for entry in guard.read_dir()? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| invalid("invalid version target directory"))?;
        let id = if let Some(id) = name.strip_prefix("template-") {
            ArtifactSourceId::Template(id.parse().map_err(io::Error::other)?)
        } else if let Some(id) = name.strip_prefix("document-") {
            ArtifactSourceId::Document(id.parse().map_err(io::Error::other)?)
        } else {
            return Err(invalid("unknown version target directory"));
        };
        if target(id)? != name {
            return Err(invalid("noncanonical version target directory"));
        }
        ids.push(id);
        if ids.len() > 100_000 {
            return Err(invalid("version targets exceed bound"));
        }
    }
    Ok(ids)
}
pub(crate) fn newest_snapshot(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
) -> io::Result<Option<Vec<u8>>> {
    for version in list(repository, id)?
        .into_iter()
        .filter(|version| version.available)
    {
        return snapshot(
            repository,
            id,
            version.version.parse().map_err(io::Error::other)?,
        )
        .map(|value| Some(value.0));
    }
    Ok(None)
}

pub(crate) fn asset_names(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    version: u64,
) -> io::Result<BTreeMap<String, String>> {
    let (path, _guards) = directory(repository.write_project().canonical_root(), id, false)?
        .ok_or_else(|| invalid("version unavailable"))?;
    let record = read(&path, id, version)?;
    if record.asset_names.len() > 100_000
        || record.asset_names.iter().any(|(id, name)| {
            !crate::data::media::valid_id(id) || !crate::data::assets::safe_name(name)
        })
    {
        return Err(invalid("invalid version display metadata"));
    }
    Ok(record.asset_names)
}

/// Read labels only from the same artifact's verified retained versions. This
/// grants neither resource bytes nor write authority and never visits another project.
pub(crate) fn known_asset_name(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    asset: &str,
) -> io::Result<Option<String>> {
    let Some((path, guards)) = directory(repository.write_project().canonical_root(), id, false)?
    else {
        return Ok(None);
    };
    let guard = guards
        .last()
        .ok_or_else(|| invalid("version directory unavailable"))?;
    for (version, _) in entries(guard)?.into_iter().rev().take(LIMIT) {
        if let Ok(record) = read(&path, id, version) {
            if record.asset_names.len() > 100_000
                || record.asset_names.iter().any(|(id, name)| {
                    !crate::data::media::valid_id(id) || !crate::data::assets::safe_name(name)
                })
            {
                continue;
            }
            if let Some(name) = record.asset_names.get(asset) {
                return Ok(Some(name.clone()));
            }
        }
    }
    Ok(None)
}

/// Import only a decoded, identity-bound preserved source after a verified whole
/// transition backup. This publishes read-only history, never repairs a canonical item.
pub(crate) fn import_preserved(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    bytes: &[u8],
    template: Option<String>,
    timestamp: &str,
) -> io::Result<u64> {
    validate_content(id, bytes)?;
    match (id, &template) {
        (ArtifactSourceId::Document(_), Some(template)) => {
            let doc = artifact::decode_document(bytes).map_err(io::Error::other)?;
            let definition =
                artifact::decode_template(template.as_bytes()).map_err(io::Error::other)?;
            if doc.template_id() != definition.template_id() {
                return Err(invalid("imported version template mismatch"));
            }
        }
        (ArtifactSourceId::Template(_), None) => (),
        _ => return Err(invalid("imported version interpretation metadata missing")),
    }
    publish(
        repository.write_project().canonical_root(),
        id,
        bytes,
        template,
        timestamp,
    )
}

pub(crate) fn next_number(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
) -> io::Result<u64> {
    let latest = list(repository, id)?
        .first()
        .map(|row| row.version.parse::<u64>())
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or(0);
    latest
        .checked_add(1)
        .ok_or_else(|| invalid("version sequence exhausted"))
}
/// The transition intent chooses the number before publication. Replaying the
/// same intent never appends a duplicate, including a crash before its receipt.
pub(crate) fn import_preserved_at(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    bytes: &[u8],
    template: Option<String>,
    timestamp: &str,
    planned: u64,
) -> io::Result<u64> {
    validate_content(id, bytes)?;
    match (id, &template) {
        (ArtifactSourceId::Document(_), Some(definition)) => {
            let doc = artifact::decode_document(bytes).map_err(io::Error::other)?;
            let definition =
                artifact::decode_template(definition.as_bytes()).map_err(io::Error::other)?;
            if doc.template_id() != definition.template_id() {
                return Err(invalid("imported version template mismatch"));
            }
        }
        (ArtifactSourceId::Template(_), None) => (),
        _ => return Err(invalid("imported version interpretation metadata missing")),
    }
    publish_planned(
        repository.write_project().canonical_root(),
        id,
        bytes,
        template,
        timestamp,
        Some(planned),
    )
}

/// Legacy format records with missing interpretation remain explicitly unavailable.
/// Raw bytes are preserved in the verified whole transition backup.
pub(crate) fn number_for_preserved(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    bytes: &[u8],
    template: &Option<String>,
) -> io::Result<u64> {
    if let Some((path, guards)) = directory(repository.write_project().canonical_root(), id, false)?
    {
        let guard = guards
            .last()
            .ok_or_else(|| invalid("version guard missing"))?;
        if let Some((number, _)) = entries(guard)?.last() {
            if read(&path, id, *number).is_ok_and(|record| {
                record.content_digest == digest(bytes) && &record.template == template
            }) {
                return Ok(*number);
            }
        }
    }
    next_number(repository, id)
}

pub(crate) fn import_legacy_at(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    bytes: &[u8],
    template: Option<String>,
    timestamp: &str,
    planned: u64,
) -> io::Result<u64> {
    if template.is_some() || matches!(id, ArtifactSourceId::Template(_)) {
        return import_preserved_at(repository, id, bytes, template, timestamp, planned);
    }
    validate_content(id, bytes)?;
    publish_planned(
        repository.write_project().canonical_root(),
        id,
        bytes,
        None,
        timestamp,
        Some(planned),
    )
}

pub(crate) fn ensure_baseline(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    timestamp: &str,
) -> io::Result<()> {
    if list(repository, id)?.is_empty() {
        confirm(repository, id, timestamp)?;
    }
    Ok(())
}

/// Call only for a creation baseline or after a successful normal editing end.
pub(crate) fn confirm(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    timestamp: &str,
) -> io::Result<u64> {
    confirm_source(repository, id, timestamp, None)
}

pub(crate) fn confirm_source(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    timestamp: &str,
    expected: Option<&str>,
) -> io::Result<u64> {
    let source = match id {
        ArtifactSourceId::Template(id) => repository.load_template(id).map(|v| v.source().clone()),
        ArtifactSourceId::Document(id) => repository.load_document(id).map(|v| v.source().clone()),
        _ => return Err(invalid("invalid version target")),
    }
    .map_err(io::Error::other)?;
    let mut file = project_file::open_existing_project_file(
        repository.write_project().canonical_root(),
        source.path(),
    )?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes.len() != source.byte_length()
        || <[u8; 32]>::from(Sha256::digest(&bytes)) != *source.sha256()
    {
        return Err(invalid("version source changed"));
    }
    if expected.is_some_and(|expected| expected != digest(&bytes)) {
        return Err(invalid(
            "confirmed version source differs from the completed edit",
        ));
    }
    let template = if let ArtifactSourceId::Document(id) = id {
        let document = repository.load_document(id).map_err(io::Error::other)?;
        let template = repository
            .load_template(document.artifact().template_id())
            .map_err(io::Error::other)?;
        let mut file = project_file::open_existing_project_file(
            repository.write_project().canonical_root(),
            template.source().path(),
        )?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        if bytes.len() != template.source().byte_length()
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != *template.source().sha256()
        {
            return Err(invalid("version template changed"));
        }
        Some(String::from_utf8(bytes).map_err(io::Error::other)?)
    } else {
        None
    };
    publish(
        repository.write_project().canonical_root(),
        id,
        &bytes,
        template,
        timestamp,
    )
}

fn publish(
    root: &Path,
    id: ArtifactSourceId,
    bytes: &[u8],
    template: Option<String>,
    timestamp: &str,
) -> io::Result<u64> {
    publish_planned(root, id, bytes, template, timestamp, None)
}
fn publish_planned(
    root: &Path,
    id: ArtifactSourceId,
    bytes: &[u8],
    template: Option<String>,
    timestamp: &str,
    planned: Option<u64>,
) -> io::Result<u64> {
    validate_content(id, bytes)?;
    let semantic_digest = semantic(bytes)?;
    let (path, guards) =
        directory(root, id, true)?.ok_or_else(|| invalid("version directory unavailable"))?;
    let guard = guards
        .last()
        .ok_or_else(|| invalid("version directory unavailable"))?;
    let existing = entries(guard)?;
    let latest = existing.last().map(|(n, _)| *n).unwrap_or(0);
    if let Some(planned) = planned {
        if existing.iter().any(|(number, _)| *number == planned) {
            let previous = read(&path, id, planned)?;
            if previous.content_digest != digest(bytes) || previous.template != template {
                return Err(invalid("transition version number occupied"));
            }
            prune(&path, guard, id, &existing, LIMIT)?;
            return Ok(planned);
        }
    }
    if latest > 0
        && read(&path, id, latest).is_ok_and(|record| {
            record.semantic_digest == semantic_digest
                && planned.is_none()
                && record.template.is_some() == template.is_some()
        })
    {
        prune(&path, guard, id, &existing, LIMIT)?;
        return Ok(latest);
    }
    let version = latest
        .checked_add(1)
        .ok_or_else(|| invalid("version sequence exhausted"))?;
    if planned.is_some_and(|planned| planned != version) {
        return Err(invalid("transition version sequence changed"));
    }
    let record = Record {
        schema_version: 1,
        version,
        artifact: target(id)?,
        recorded_at_utc: timestamp.into(),
        content_digest: digest(bytes),
        semantic_digest,
        content: String::from_utf8(bytes.to_vec()).map_err(io::Error::other)?,
        template_digest: template.as_ref().map(|value| digest(value.as_bytes())),
        template,
        asset_names: {
            let raw =
                crate::data::json::parse_strict_json_object(bytes).map_err(io::Error::other)?;
            let ids = crate::data::assets::references(&raw).map_err(io::Error::other)?;
            let store = crate::data::assets::Store::open(root, false).ok();
            let mut names = BTreeMap::new();
            for asset in ids {
                if let Some(metadata) = store
                    .as_ref()
                    .and_then(|store| store.display_metadata(&asset).ok())
                {
                    names.insert(asset, metadata.name);
                }
            }
            names
        },
    };
    let encoded = serde_json::to_vec(&record).map_err(io::Error::other)?;
    if encoded.len() > 64 * 1024 * 1024 {
        return Err(invalid("version record too large"));
    }
    let pending = path.join(format!("pending-{}.json", uuid::Uuid::new_v4()));
    let mut file = native::open(&pending, true, true)?;
    guard.validate_file(
        &file,
        pending
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| invalid("invalid staging name"))?,
    )?;
    if let Err(error) = file
        .write_all(&encoded)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .and_then(|_| checkpoint("before_publish"))
    {
        let _ = native::cleanup(&file);
        return Err(error);
    }
    if let Err(error) = native::publish(&file, &name(version)) {
        let _ = native::cleanup(&file);
        return Err(error);
    }
    drop(file);
    checkpoint("after_publish")?;
    if read(&path, id, version)?.content_digest != record.content_digest {
        return Err(invalid("published version differs"));
    }
    checkpoint("before_prune")?;
    let mut committed = existing;
    committed.push((version, name(version)));
    prune(&path, guard, id, &committed, LIMIT)?;
    Ok(version)
}

fn prune(
    path: &Path,
    guard: &ProjectDirectory,
    _id: ArtifactSourceId,
    records: &[(u64, String)],
    keep: usize,
) -> io::Result<()> {
    // Namespace/name admission happened before publication. Retention includes
    // corrupt old versions: only the newest ten files remain. Read the exact
    // owned handle twice, and never delete an entry that changes during pruning.
    for (_, filename) in records.iter().take(records.len().saturating_sub(keep)) {
        let mut file = native::open_for_discard(&path.join(filename))?;
        guard.validate_file(&file, filename)?;
        let mut expected = Vec::new();
        Read::by_ref(&mut file)
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut expected)?;
        if expected.len() > 64 * 1024 * 1024 {
            return Err(invalid("version exceeds retention bound"));
        }
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0))?;
        let mut actual = Vec::new();
        Read::by_ref(&mut file)
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut actual)?;
        guard.validate_file(&file, filename)?;
        if actual != expected {
            return Err(invalid("version changed during pruning"));
        }
        native::cleanup(&file)?;
    }
    Ok(())
}

/// The preview carries this opaque observation; it is never shown to users.
pub(crate) fn observation(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
) -> io::Result<String> {
    let path = id.path().map_err(io::Error::other)?;
    let root = repository.write_project().canonical_root();
    let mut file = match project_file::open_existing_project_file(root, &path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok("missing".into()),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() > 64 * 1024 * 1024 {
        return Err(invalid("source too large"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(digest(&bytes))
}

pub(crate) fn restore_plan(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    version: u64,
    expected: &str,
    timestamp: &str,
) -> Result<Option<CanonicalWritePlan>, ArtifactWriteError> {
    let fail = |error| ArtifactWriteError::io(id, ArtifactWriteStage::ReadSource, error);
    if observation(repository, id).map_err(fail)? != expected {
        return Err(fail(invalid(
            "version restore source changed after preview",
        )));
    }
    let (bytes, metadata) = snapshot(repository, id, version).map_err(fail)?;
    let current_format = |bytes: &[u8]| -> Result<Vec<u8>, ArtifactWriteError> {
        let header = artifact::inspect_artifact_header(bytes)
            .map_err(|error| fail(io::Error::other(error)))?;
        let current = match id {
            ArtifactSourceId::Template(_) => artifact::TEMPLATE_SCHEMA_VERSION,
            ArtifactSourceId::Document(_) => artifact::DOCUMENT_SCHEMA_VERSION,
            _ => return Err(fail(invalid("invalid version format target"))),
        };
        if header.schema_version() == current {
            return Ok(bytes.to_vec());
        }
        artifact::transition_format(
            bytes,
            &id.path().map_err(|error| fail(io::Error::other(error)))?,
        )
        .map_err(|error| fail(io::Error::other(error)))
    };
    let historical_bytes = current_format(&bytes)?;
    let verified_assets = crate::data::assets::reference_kinds(
        &crate::data::json::parse_strict_json_object(&bytes)
            .map_err(|error| fail(io::Error::other(error)))?,
    )
    .map_err(|error| fail(io::Error::other(error)))?;
    let mut original = Vec::new();
    let observed = match project_file::open_existing_project_file(
        repository.write_project().canonical_root(),
        &id.path().map_err(|error| fail(io::Error::other(error)))?,
    ) {
        Ok(mut file) => {
            if file.metadata().map_err(fail)?.len() > 64 * 1024 * 1024 {
                return Err(fail(invalid("source too large")));
            }
            file.read_to_end(&mut original).map_err(fail)?;
            if digest(&original) != expected {
                return Err(fail(invalid("source changed before restoration")));
            }
            Some(original.as_slice())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && expected == "missing" => None,
        Err(error) => return Err(fail(error)),
    };
    // If the canonical item is damaged/missing, the newest valid confirmed
    // version supplies stable definition and revision metadata, never the path.
    let fallback = || -> Result<Vec<u8>, ArtifactWriteError> {
        let rows = list(repository, id).map_err(fail)?;
        let row = rows
            .iter()
            .find(|row| row.available)
            .ok_or_else(|| fail(invalid("no valid confirmed content")))?;
        snapshot(
            repository,
            id,
            row.version
                .parse()
                .map_err(|error| fail(io::Error::other(error)))?,
        )
        .map(|(bytes, _)| bytes)
        .map_err(fail)
    };
    #[cfg(test)]
    RESTORE_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    let matches_observed = |source: &super::SourceToken| -> Result<(), ArtifactWriteError> {
        if observed.is_none_or(|bytes| {
            bytes.len() != source.byte_length()
                || <[u8; 32]>::from(Sha256::digest(bytes)) != *source.sha256()
        }) {
            return Err(fail(invalid("source changed during version restoration")));
        }
        Ok(())
    };
    match id {
        ArtifactSourceId::Template(template_id) => {
            let historical = artifact::decode_template(&historical_bytes)
                .map_err(|error| fail(io::Error::other(error)))?;
            let loaded = repository.load_template(template_id);
            if let Ok(loaded) = &loaded {
                matches_observed(loaded.source())?;
            }
            let fallback_bytes;
            let current = match &loaded {
                Ok(current) => current.artifact(),
                Err(_) => {
                    if observed.is_some_and(|bytes| artifact::decode_template(bytes).is_ok()) {
                        return Err(fail(invalid("source is readable but binding is invalid")));
                    }
                    fallback_bytes = current_format(&fallback()?)?;
                    // Decode below is owned separately so the fallback lifetime is explicit.
                    return restore_template_observed(
                        repository,
                        id,
                        &fallback_bytes,
                        &historical,
                        timestamp,
                        observed,
                        verified_assets,
                    );
                }
            };
            let outcome = artifact::template_mutation::whole::prepare_version_restore(
                current,
                &historical,
                timestamp,
            )
            .map_err(|error| fail(io::Error::other(error)))?;
            match outcome {
                artifact::template_mutation::TemplateMutationOutcome::Unchanged => Ok(None),
                artifact::template_mutation::TemplateMutationOutcome::Changed(candidate) => {
                    Ok(Some(
                        CanonicalWritePlan::new()
                            .replace_template(&candidate, loaded.as_ref().unwrap().source())?
                            .version_assets(verified_assets),
                    ))
                }
            }
        }
        ArtifactSourceId::Document(document_id) => {
            let historical = artifact::decode_document(&historical_bytes)
                .map_err(|error| fail(io::Error::other(error)))?;
            let loaded = repository.load_document(document_id);
            if let Ok(loaded) = &loaded {
                matches_observed(loaded.source())?;
            }
            let fallback_document;
            let current = match &loaded {
                Ok(current) => current.artifact(),
                Err(_) => {
                    if observed.is_some_and(|bytes| artifact::decode_document(bytes).is_ok()) {
                        return Err(fail(invalid("source is readable but binding is invalid")));
                    }
                    fallback_document = artifact::decode_document(&fallback()?)
                        .map_err(|error| fail(io::Error::other(error)))?;
                    &fallback_document
                }
            };
            if loaded.is_ok()
                && semantic(
                    &artifact::encode_document(current)
                        .map_err(|error| fail(io::Error::other(error)))?,
                )
                .map_err(fail)?
                    == semantic(&bytes).map_err(fail)?
            {
                return Ok(None);
            }
            let historical_template = artifact::decode_template(
                &metadata.ok_or_else(|| fail(invalid("historical template missing")))?,
            )
            .map_err(|error| fail(io::Error::other(error)))?;
            let template = repository
                .load_template(current.template_id())
                .map_err(ArtifactWriteError::repository)?;
            let candidate = artifact::prepare_document_version_restore(
                current,
                &historical,
                &historical_template,
                template.artifact(),
                timestamp,
            )
            .map_err(|error| fail(io::Error::other(error)))?;
            let plan = if let Ok(loaded) = loaded {
                CanonicalWritePlan::new()
                    .replace_document(&candidate, loaded.source())?
                    .version_assets(verified_assets)
            } else {
                CanonicalWritePlan::new().restore_observed(
                    repository,
                    id,
                    artifact::encode_document(&candidate)
                        .map_err(|error| fail(io::Error::other(error)))?,
                    observed,
                    verified_assets,
                )?
            };
            Ok(Some(
                plan.document_state(repository, document_id)?
                    .read_dependency(template.source()),
            ))
        }
        _ => Err(fail(invalid("invalid content version target"))),
    }
}

fn restore_template_observed(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    current_bytes: &[u8],
    historical: &artifact::TemplateArtifact,
    timestamp: &str,
    observed: Option<&[u8]>,
    assets: crate::data::assets::ReferenceKinds,
) -> Result<Option<CanonicalWritePlan>, ArtifactWriteError> {
    let fail = |error| ArtifactWriteError::io(id, ArtifactWriteStage::ReadSource, error);
    let current =
        artifact::decode_template(current_bytes).map_err(|error| fail(io::Error::other(error)))?;
    let outcome = artifact::template_mutation::whole::prepare_version_restore(
        &current, historical, timestamp,
    )
    .map_err(|error| fail(io::Error::other(error)))?;
    let candidate = outcome.changed().unwrap_or(&current);
    let bytes =
        artifact::encode_template(candidate).map_err(|error| fail(io::Error::other(error)))?;
    Ok(Some(CanonicalWritePlan::new().restore_observed(
        repository, id, bytes, observed, assets,
    )?))
}

#[cfg(test)]
thread_local! { static RESTORE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
pub(super) fn install_restore_hook(hook: Box<dyn FnOnce()>) {
    RESTORE_HOOK.with(|slot| *slot.borrow_mut() = Some(hook));
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
        return Err(io::Error::other("injected version storage failure"));
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::data::repository::tests::{template_bytes, Fixture, TEMPLATE};
    fn changed(n: u64) -> Vec<u8> {
        let mut value: serde_json::Value =
            serde_json::from_slice(&template_bytes(TEMPLATE, "active")).unwrap();
        value["name"] = serde_json::json!(format!("내용 {n}"));
        serde_json::to_vec(&value).unwrap()
    }
    #[test]
    fn content_versions_are_bounded_and_numbers_never_restart_after_pruning() {
        let fixture = Fixture::new().unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        for n in 1..=14 {
            assert_eq!(
                publish(&fixture.root, id, &changed(n), None, "2026-10-04T00:00:00Z").unwrap(),
                n
            );
        }
        let (path, guards) = directory(&fixture.root, id, false).unwrap().unwrap();
        assert_eq!(entries(guards.last().unwrap()).unwrap().len(), 10);
        assert!(read(&path, id, 4).is_err());
        assert_eq!(read(&path, id, 14).unwrap().content.as_bytes(), changed(14));
        assert_eq!(
            publish(
                &fixture.root,
                id,
                &changed(14),
                None,
                "2026-10-05T00:00:00Z"
            )
            .unwrap(),
            14
        );
        assert_eq!(
            publish(
                &fixture.root,
                id,
                &changed(15),
                None,
                "2026-10-05T00:00:00Z"
            )
            .unwrap(),
            15
        );
    }
    #[test]
    fn corrupt_old_versions_do_not_accumulate_outside_the_latest_ten() {
        let fixture = Fixture::new().unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        for n in 1..=10 {
            publish(&fixture.root, id, &changed(n), None, "2026-10-04T00:00:00Z").unwrap();
        }
        let (path, guards) = directory(&fixture.root, id, false).unwrap().unwrap();
        fs::write(path.join(name(1)), b"corrupt older version").unwrap();
        fs::write(path.join(name(9)), b"corrupt recent version").unwrap();
        publish(
            &fixture.root,
            id,
            &changed(11),
            None,
            "2026-10-04T00:00:00Z",
        )
        .unwrap();
        assert_eq!(entries(guards.last().unwrap()).unwrap().len(), 10);
        assert!(!path.join(name(1)).exists());
        assert_eq!(
            fs::read(path.join(name(9))).unwrap(),
            b"corrupt recent version"
        );
        assert_eq!(read(&path, id, 11).unwrap().content.as_bytes(), changed(11));
    }
    #[test]
    fn unrelated_asset_label_cannot_be_displayed_from_a_verified_content_version() {
        let fixture = Fixture::new().unwrap();
        let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
        publish(&fixture.root, id, &changed(1), None, "2026-10-04T00:00:00Z").unwrap();
        let (path, _guards) = directory(&fixture.root, id, false).unwrap().unwrap();
        let record_path = path.join(name(1));
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        value["assetNames"] =
            serde_json::json!({uuid::Uuid::new_v4().to_string(): "다른 자료.png"});
        fs::write(record_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(read(&path, id, 1).is_err());
    }
    #[test]
    fn timestamps_and_internal_revision_do_not_make_another_content_version() {
        let bytes = changed(1);
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["revision"] = serde_json::json!(99);
        value["updatedAtUtc"] = serde_json::json!("2026-10-05T00:00:00Z");
        assert_eq!(
            semantic(&bytes).unwrap(),
            semantic(&serde_json::to_vec(&value).unwrap()).unwrap()
        );
        value["name"] = serde_json::json!("다른 내용");
        assert_ne!(
            semantic(&bytes).unwrap(),
            semantic(&serde_json::to_vec(&value).unwrap()).unwrap()
        );
    }
    #[test]
    fn publication_failure_never_prunes_previous_versions_and_retry_is_idempotent() {
        for point in ["before_publish", "after_publish", "before_prune"] {
            let fixture = Fixture::new().unwrap();
            let id = ArtifactSourceId::Template(TEMPLATE.parse().unwrap());
            for n in 1..=10 {
                publish(&fixture.root, id, &changed(n), None, "2026-10-04T00:00:00Z").unwrap();
            }
            FAULT.with(|fault| fault.set(Some(point)));
            assert!(publish(
                &fixture.root,
                id,
                &changed(11),
                None,
                "2026-10-04T00:00:00Z"
            )
            .is_err());
            let (path, guards) = directory(&fixture.root, id, false).unwrap().unwrap();
            assert!(read(&path, id, 1).is_ok());
            assert!(read(&path, id, 10).is_ok());
            assert_eq!(
                publish(
                    &fixture.root,
                    id,
                    &changed(11),
                    None,
                    "2026-10-04T00:00:00Z"
                )
                .unwrap(),
                11
            );
            assert_eq!(entries(guards.last().unwrap()).unwrap().len(), 10);
            assert!(read(&path, id, 1).is_err());
            assert!(read(&path, id, 11).is_ok());
        }
    }
    #[test]
    fn transition_same_document_bytes_with_distinct_interpretation_are_not_deduplicated() {
        let fixture = Fixture::new().unwrap();
        let bytes = super::super::tests::document_bytes(1, TEMPLATE);
        let doc = artifact::decode_document(&bytes).unwrap();
        let id = ArtifactSourceId::Document(doc.document_id());
        let t1 = String::from_utf8(template_bytes(TEMPLATE, "active")).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&t1).unwrap();
        value["name"] = "다른 해석 이름".into();
        let t2 = String::from_utf8(
            artifact::encode_template(
                &artifact::decode_template(&serde_json::to_vec(&value).unwrap()).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            publish_planned(
                &fixture.root,
                id,
                &bytes,
                Some(t1.clone()),
                "2026-10-04T00:00:00Z",
                Some(1)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            publish_planned(
                &fixture.root,
                id,
                &bytes,
                Some(t2.clone()),
                "2026-10-04T00:00:00Z",
                Some(2)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            publish_planned(
                &fixture.root,
                id,
                &bytes,
                Some(t2.clone()),
                "2026-10-04T00:00:00Z",
                Some(2)
            )
            .unwrap(),
            2
        );
        assert!(publish_planned(
            &fixture.root,
            id,
            &bytes,
            Some(t1.clone()),
            "2026-10-04T00:00:00Z",
            Some(2)
        )
        .is_err());
        let (path, _) = directory(&fixture.root, id, false).unwrap().unwrap();
        assert_eq!(read(&path, id, 1).unwrap().template, Some(t1));
        assert_eq!(read(&path, id, 2).unwrap().template, Some(t2));
    }
}
