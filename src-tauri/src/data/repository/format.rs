//! 사용자 명시 형식 전환/버전 복원. 영구 원문은 임시 transaction backup과 별개다.
use super::*;
use std::path::PathBuf;

fn invalid(id: ArtifactSourceId, reason: &'static str) -> ArtifactWriteError {
    ArtifactWriteError::io(
        id,
        ArtifactWriteStage::ReadSource,
        io::Error::new(io::ErrorKind::InvalidData, reason),
    )
}
fn cause(
    id: ArtifactSourceId,
    error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> ArtifactWriteError {
    ArtifactWriteError::io(id, ArtifactWriteStage::ReadSource, io::Error::other(error))
}
fn source(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
) -> Result<SourceToken, ArtifactWriteError> {
    match id {
        ArtifactSourceId::Template(id) => repository.load_template(id).map(|v| v.source().clone()),
        ArtifactSourceId::Document(id) => repository.load_document(id).map(|v| v.source().clone()),
        ArtifactSourceId::DocumentLayout => {
            return Err(invalid(id, "layout has no format transition"))
        }
    }
    .map_err(ArtifactWriteError::repository)
}
fn validate_identity(id: ArtifactSourceId, bytes: &[u8]) -> Result<(), ArtifactWriteError> {
    let found = match id {
        ArtifactSourceId::Template(_) => ArtifactSourceId::Template(
            artifact::decode_template(bytes)
                .map_err(|e| cause(id, e))?
                .template_id(),
        ),
        ArtifactSourceId::Document(_) => ArtifactSourceId::Document(
            artifact::decode_document(bytes)
                .map_err(|e| cause(id, e))?
                .document_id(),
        ),
        ArtifactSourceId::DocumentLayout => {
            return Err(invalid(id, "layout has no format history"))
        }
    };
    if found != id {
        return Err(invalid(id, "format history artifact identity differs"));
    }
    Ok(())
}
fn read_source(
    repository: &ArtifactRepository<'_, '_>,
    token: &SourceToken,
) -> Result<Vec<u8>, ArtifactWriteError> {
    let mut file = project_file::open_existing_project_file(
        repository.write_project().canonical_root(),
        token.path(),
    )
    .map_err(|e| cause(token.id(), e))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| cause(token.id(), e))?;
    if bytes.len() != token.byte_length()
        || <[u8; 32]>::from(Sha256::digest(&bytes)) != *token.sha256()
    {
        return Err(invalid(token.id(), "format source changed"));
    }
    Ok(bytes)
}
fn directory(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    create: bool,
) -> Result<Option<(PathBuf, Vec<ProjectDirectory>)>, ArtifactWriteError> {
    let root = repository.write_project().canonical_root();
    let mut path = root.to_path_buf();
    let mut guards = vec![ProjectDirectory::open_root(root).map_err(|e| cause(id, e))?];
    let folder = format!(
        "{}-{}",
        match id {
            ArtifactSourceId::Template(_) => "template",
            ArtifactSourceId::Document(_) => "document",
            _ => return Err(invalid(id, "invalid history target")),
        },
        id.filename().trim_end_matches(".json")
    );
    for name in [".worldbuild", "format-history", folder.as_str()] {
        path.push(name);
        if create {
            match fs::create_dir(&path) {
                Ok(()) => (),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(cause(id, e)),
            }
        } else if matches!(fs::symlink_metadata(&path), Err(e) if e.kind()==io::ErrorKind::NotFound)
        {
            return Ok(None);
        }
        guards.push(ProjectDirectory::open_root(&path).map_err(|e| cause(id, e))?);
    }
    Ok(Some((path, guards)))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_snapshot(
    path: &std::path::Path,
    id: ArtifactSourceId,
    hash: &str,
) -> Result<Vec<u8>, ArtifactWriteError> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid(id, "invalid history digest"));
    }
    let mut file =
        project_file::open_existing_private_file(path, &path.join(format!("{hash}.json")))
            .map_err(|e| cause(id, e))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| cause(id, e))?;
    if digest(&bytes) != hash {
        return Err(invalid(id, "history digest mismatch; preserve original"));
    }
    validate_identity(id, &bytes)?;
    Ok(bytes)
}
pub(super) fn preserve(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    bytes: &[u8],
) -> Result<(), ArtifactWriteError> {
    validate_identity(id, bytes)?;
    let timestamp = match id {
        ArtifactSourceId::Template(_) => artifact::decode_template(bytes)
            .map_err(|e| cause(id, e))?
            .updated_at_utc()
            .to_owned(),
        ArtifactSourceId::Document(_) => artifact::decode_document(bytes)
            .map_err(|e| cause(id, e))?
            .updated_at_utc()
            .to_owned(),
        _ => return Err(invalid(id, "invalid version target")),
    };
    let expected = digest(bytes);
    super::versions::confirm_source(repository, id, &timestamp, Some(&expected))
        .map_err(|e| cause(id, e))?;
    Ok(())
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct Snapshot {
    pub(crate) digest: String,
    pub(crate) schema: u32,
    pub(crate) content_updated_at: String,
}
pub(crate) fn history(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
) -> Result<Vec<Snapshot>, ArtifactWriteError> {
    let Some((path, guards)) = directory(repository, id, false)? else {
        return Ok(vec![]);
    };
    let mut result = vec![];
    for entry in guards
        .last()
        .ok_or_else(|| invalid(id, "history directory unavailable"))?
        .read_dir()
        .map_err(|e| cause(id, e))?
    {
        let entry = entry.map_err(|e| cause(id, e))?;
        let name = entry.file_name();
        let hash = name
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .ok_or_else(|| invalid(id, "unexpected history entry; preserve files"))?;
        let bytes = read_snapshot(&path, id, hash)?;
        let header = artifact::inspect_artifact_header(&bytes).map_err(|e| cause(id, e))?;
        let content_updated_at = match id {
            ArtifactSourceId::Template(_) => artifact::decode_template(&bytes)
                .map_err(|e| cause(id, e))?
                .updated_at_utc()
                .to_owned(),
            ArtifactSourceId::Document(_) => artifact::decode_document(&bytes)
                .map_err(|e| cause(id, e))?
                .updated_at_utc()
                .to_owned(),
            ArtifactSourceId::DocumentLayout => return Err(invalid(id, "layout has no history")),
        };
        result.push(Snapshot {
            content_updated_at,
            digest: hash.into(),
            schema: header.schema_version().get(),
        });
    }
    result.sort_by(|a, b| a.digest.cmp(&b.digest));
    Ok(result)
}
/// digest는 화면이 읽은 현재 source 증거다. 변환/복원 모두 source 재확인을 강제한다.
pub(crate) fn build(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
    expected: &str,
    restore: Option<&str>,
) -> Result<CanonicalWritePlan, ArtifactWriteError> {
    append(repository, CanonicalWritePlan::new(), id, expected, restore)
}

fn append(
    repository: &ArtifactRepository<'_, '_>,
    plan: CanonicalWritePlan,
    id: ArtifactSourceId,
    expected: &str,
    restore: Option<&str>,
) -> Result<CanonicalWritePlan, ArtifactWriteError> {
    let token = source(repository, id)?;
    let bytes = read_source(repository, &token)?;
    if digest(&bytes) != expected {
        return Err(invalid(id, "format source is stale; reread before retry"));
    }
    let next = if let Some(hash) = restore {
        let (path, _guards) = directory(repository, id, false)?
            .ok_or_else(|| invalid(id, "format history unavailable"))?;
        let historical = read_snapshot(&path, id, hash)?;
        let header =
            artifact::inspect_artifact_header(&historical).map_err(|error| cause(id, error))?;
        let current = match id {
            ArtifactSourceId::Template(_) => artifact::TEMPLATE_SCHEMA_VERSION,
            ArtifactSourceId::Document(_) => artifact::DOCUMENT_SCHEMA_VERSION,
            _ => return Err(invalid(id, "invalid format restore target")),
        };
        if header.schema_version() == current {
            historical
        } else {
            artifact::transition_format(&historical, token.path())
                .map_err(|error| cause(id, error))?
        }
    } else {
        artifact::transition_format(&bytes, token.path()).map_err(|e| cause(id, e))?
    };
    validate_identity(id, &next)?;
    plan.format_change(&token, next)
}

/// All members cross the policy boundary in the same canonical transaction.
pub(crate) fn build_policy_batch(
    repository: &ArtifactRepository<'_, '_>,
    sources: &[(ArtifactSourceId, String)],
) -> Result<CanonicalWritePlan, ArtifactWriteError> {
    let mut plan = CanonicalWritePlan::new();
    for (id, expected) in sources {
        plan = append(repository, plan, *id, expected, None)?;
    }
    Ok(plan)
}
pub(crate) fn validate_policy_backup(
    prepared: &crate::data::edit_recovery::policy_transition::Prepared,
) -> Result<(), ArtifactWriteError> {
    prepared
        .validate()
        .map_err(|error| cause(prepared.sources[0].0, error))
}
pub(crate) fn inspect(
    repository: &ArtifactRepository<'_, '_>,
    id: ArtifactSourceId,
) -> Result<(u32, String, Vec<Snapshot>), ArtifactWriteError> {
    let token = source(repository, id)?;
    Ok((
        token.schema().get(),
        digest(&read_source(repository, &token)?),
        history(repository, id)?,
    ))
}
