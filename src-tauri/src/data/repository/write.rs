//! candidate 계산과 I/O 권한을 분리한다. 이 모듈만 canonical bytes와 원본 조건을 결합한다.
#[cfg(all(test, windows))]
#[path = "write_tests.rs"]
mod tests;
use super::*;
use crate::data::{
    collaboration_lock::WritePermit,
    transaction::{self, LockedProject, PreparedArtifactTransaction},
};

enum ArtifactIntent {
    Create,
    Replace(SourceToken),
    RestoreObserved {
        project: ProjectIdentity,
        expected: Option<(usize, [u8; 32])>,
    },
}

/// 필드/생성자/가변 accessor가 없다. M1도 이 결합 전체를 소유하고 분해하지 않는다.
pub(crate) struct CanonicalArtifactWrite {
    id: ArtifactSourceId,
    target: ProjectRelativePath,
    intent: ArtifactIntent,
    bytes: Vec<u8>,
    guide_schema_upgrade: bool,
    relation_name_schema_upgrade: bool,
    format_transition: Option<(SchemaVersion, SchemaVersion)>,
    verified_missing_assets: crate::data::assets::ReferenceKinds,
}

/// 소비형 builder이므로 두 번째 candidate가 실패하면 부분 plan을 다시 prepare할 수 없다.
#[derive(Default)]
pub(crate) struct CanonicalWritePlan {
    writes: Vec<CanonicalArtifactWrite>,
    dependencies: Vec<SourceToken>,
    layout_dependency: Option<Option<SourceToken>>,
    document_membership: Option<std::collections::BTreeSet<ArtifactSourceId>>,
}

impl CanonicalWritePlan {
    pub(super) fn format_change(
        mut self,
        source: &SourceToken,
        bytes: Vec<u8>,
    ) -> Result<Self, ArtifactWriteError> {
        let id = source.id();
        let header = artifact::inspect_artifact_header(&bytes)
            .map_err(|e| ArtifactWriteError::codec(id, ArtifactWriteStage::Encode, e))?;
        let target = source.path().clone();
        if self.writes.iter().any(|w| w.target == target) {
            return Err(ArtifactWriteError::closed(
                ArtifactWriteCategory::DuplicateTarget,
                ArtifactWriteStage::BuildPlan,
                Some(id),
            ));
        }
        self.writes.push(CanonicalArtifactWrite {
            id,
            target,
            intent: ArtifactIntent::Replace(source.clone()),
            bytes,
            guide_schema_upgrade: false,
            relation_name_schema_upgrade: false,
            format_transition: Some((source.schema(), header.schema_version())),
            verified_missing_assets: Default::default(),
        });
        Ok(self)
    }
    /// Only verified same-target content versions may replace an observed raw
    /// source. The ordinary transaction keeps its exact bytes for rollback.
    pub(super) fn restore_observed(
        mut self,
        repository: &ArtifactRepository<'_, '_>,
        id: ArtifactSourceId,
        bytes: Vec<u8>,
        observed: Option<&[u8]>,
        verified_assets: crate::data::assets::ReferenceKinds,
    ) -> Result<Self, ArtifactWriteError> {
        let target = id.path().map_err(|_| {
            ArtifactWriteError::closed(
                ArtifactWriteCategory::SourceMismatch,
                ArtifactWriteStage::BindSource,
                Some(id),
            )
        })?;
        self.writes.push(CanonicalArtifactWrite {
            id,
            target,
            intent: ArtifactIntent::RestoreObserved {
                project: repository.identity.clone(),
                expected: observed.map(|bytes| (bytes.len(), Sha256::digest(bytes).into())),
            },
            bytes,
            guide_schema_upgrade: false,
            relation_name_schema_upgrade: false,
            format_transition: None,
            verified_missing_assets: verified_assets,
        });
        Ok(self)
    }
    pub(super) fn version_assets(mut self, assets: crate::data::assets::ReferenceKinds) -> Self {
        if let Some(write) = self.writes.last_mut() {
            write.verified_missing_assets = assets;
        }
        self
    }
    pub(crate) fn read_dependency(mut self, source: &SourceToken) -> Self {
        self.dependencies.push(source.clone());
        self
    }
    /// 기존 단일 문서 저장에는 전체 scan 없이 배치 상태만 읽기 의존성으로 묶는다.
    pub(crate) fn document_state(
        mut self,
        repository: &ArtifactRepository<'_, '_>,
        id: artifact::DocumentId,
    ) -> Result<Self, ArtifactWriteError> {
        let layout = repository
            .load_layout()
            .map_err(ArtifactWriteError::repository)?;
        if layout.as_ref().is_some_and(|l| {
            l.artifact()
                .nodes
                .get(&id)
                .is_some_and(|n| n.state == artifact::layout::LayoutState::Trashed)
        }) {
            return Err(ArtifactWriteError::closed(
                ArtifactWriteCategory::SourceMismatch,
                ArtifactWriteStage::ReadSource,
                Some(ArtifactSourceId::DocumentLayout),
            ));
        }
        self.layout_dependency = Some(layout.as_ref().map(|l| l.source().clone()));
        Ok(self)
    }
    pub(crate) fn document_scan(mut self, scan: &CompleteDocumentScan) -> Self {
        self.document_membership = Some(scan.sources().map(|s| s.id()).collect());
        self.dependencies.extend(scan.sources().cloned());
        self
    }
    pub(crate) fn new() -> Self {
        Self::default()
    }
    pub(crate) fn create_template(
        self,
        candidate: &TemplateArtifact,
    ) -> Result<Self, ArtifactWriteError> {
        self.template(candidate, ArtifactIntent::Create)
    }
    pub(crate) fn replace_template(
        self,
        candidate: &TemplateArtifact,
        source: &SourceToken,
    ) -> Result<Self, ArtifactWriteError> {
        self.template(candidate, ArtifactIntent::Replace(source.clone()))
    }
    pub(crate) fn create_document(
        self,
        candidate: &DocumentArtifact,
    ) -> Result<Self, ArtifactWriteError> {
        self.document(candidate, ArtifactIntent::Create)
    }
    pub(crate) fn replace_document(
        self,
        candidate: &DocumentArtifact,
        source: &SourceToken,
    ) -> Result<Self, ArtifactWriteError> {
        self.document(candidate, ArtifactIntent::Replace(source.clone()))
    }
    pub(crate) fn layout(
        self,
        candidate: &artifact::layout::DocumentLayout,
        source: Option<&SourceToken>,
    ) -> Result<Self, ArtifactWriteError> {
        self.add(
            ArtifactSourceId::DocumentLayout,
            source.map_or(ArtifactIntent::Create, |s| {
                ArtifactIntent::Replace(s.clone())
            }),
            || artifact::encode_layout(candidate),
        )
    }
    fn template(
        self,
        candidate: &TemplateArtifact,
        intent: ArtifactIntent,
    ) -> Result<Self, ArtifactWriteError> {
        let id = ArtifactSourceId::Template(candidate.template_id());
        let upgrade = matches!(&intent, ArtifactIntent::Replace(source) if source.schema.get() == 1 && candidate.schema_version().get() == 2)
            && candidate
                .fields()
                .values()
                .any(|field| field.writing_guide().is_some());
        let mut plan = self.add(id, intent, || artifact::encode_template(candidate))?;
        if let Some(write) = plan.writes.last_mut() {
            write.guide_schema_upgrade = upgrade;
        }
        Ok(plan)
    }
    fn document(
        self,
        candidate: &DocumentArtifact,
        intent: ArtifactIntent,
    ) -> Result<Self, ArtifactWriteError> {
        let id = ArtifactSourceId::Document(candidate.document_id());
        let upgrade = matches!(&intent, ArtifactIntent::Replace(source) if source.schema.get() == 5 && candidate.schema_version().get() == 6)
            && candidate
                .field_values()
                .values()
                .any(artifact::FieldValue::has_named_relation);
        let mut plan = self.add(id, intent, || artifact::encode_document(candidate))?;
        if let Some(write) = plan.writes.last_mut() {
            write.relation_name_schema_upgrade = upgrade;
        }
        Ok(plan)
    }
    fn add(
        mut self,
        id: ArtifactSourceId,
        intent: ArtifactIntent,
        encode: impl FnOnce() -> Result<Vec<u8>, artifact::ArtifactCodecError>,
    ) -> Result<Self, ArtifactWriteError> {
        let target = id.path().map_err(|_| {
            ArtifactWriteError::closed(
                ArtifactWriteCategory::SourceMismatch,
                ArtifactWriteStage::BindSource,
                Some(id),
            )
        })?;
        if let ArtifactIntent::Replace(source) = &intent {
            if source.id != id || source.path != target {
                return Err(ArtifactWriteError::closed(
                    ArtifactWriteCategory::SourceMismatch,
                    ArtifactWriteStage::BindSource,
                    Some(id),
                ));
            }
        }
        if self.writes.iter().any(|write| write.target == target) {
            return Err(ArtifactWriteError::closed(
                ArtifactWriteCategory::DuplicateTarget,
                ArtifactWriteStage::BuildPlan,
                Some(id),
            ));
        }
        let bytes =
            encode().map_err(|e| ArtifactWriteError::codec(id, ArtifactWriteStage::Encode, e))?;
        self.writes.push(CanonicalArtifactWrite {
            id,
            target,
            intent,
            bytes,
            guide_schema_upgrade: false,
            relation_name_schema_upgrade: false,
            format_transition: None,
            verified_missing_assets: Default::default(),
        });
        Ok(self)
    }
    pub(crate) fn targets(&self) -> impl Iterator<Item = &ProjectRelativePath> {
        self.writes.iter().map(|write| &write.target)
    }
    /// G5의 domain/plan 및 exact 대조 뒤에만 호출한다. 경로나 caller의 bool은 권한이 아니다.
    /// 반환 guard는 호출자가 prepare와 commit이 끝날 때까지 보유한다.
    pub(crate) fn prepare_create_namespaces(
        &self,
        repository: &ArtifactRepository<'_, '_>,
        permit: &mut WritePermit<'_>,
    ) -> Result<Vec<ProjectDirectory>, ArtifactWriteError> {
        let creates: Vec<_> = self
            .writes
            .iter()
            .filter(|write| {
                matches!(
                    write.intent,
                    ArtifactIntent::Create | ArtifactIntent::RestoreObserved { expected: None, .. }
                )
            })
            .collect();
        if creates.is_empty() {
            return Ok(Vec::new());
        }
        let mut targets: Vec<_> = self.targets().cloned().collect();
        targets.sort();
        permit
            .validate_for(repository.write_project().fingerprint(), &targets)
            .map_err(|error| {
                ArtifactWriteError::prepare(transaction::TransactionPrepareError::Permit(error))
            })?;
        let mut prepared_kinds = Vec::new();
        let mut guards = Vec::new();
        for write in creates {
            let kind = write.id.kind();
            if prepared_kinds.contains(&kind) {
                continue;
            }
            if let Some(directory) = repository
                .namespace(kind, RepositoryOperation::PrepareWrite)
                .map_err(ArtifactWriteError::repository)?
            {
                prepared_kinds.push(kind);
                guards.push(directory);
                continue;
            }
            let fail =
                |error| ArtifactWriteError::io(write.id, ArtifactWriteStage::Namespace, error);
            repository.root.validate().map_err(fail)?;
            let name = match kind {
                ArtifactType::Template => "templates",
                ArtifactType::Document => "documents",
                ArtifactType::DocumentLayout => "workspace",
            };
            #[cfg(test)]
            super::test_support::point(
                RepositoryStage::Namespace,
                &repository.write_project().canonical_root().join(name),
            )
            .map_err(fail)?;
            // root는 이미 G2/G3가 연 handle로 고정돼 있다. 고정된 자식 한 단계만 만든다.
            match fs::create_dir(repository.write_project().canonical_root().join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(fail(error)),
            }
            let directory = repository
                .namespace(kind, RepositoryOperation::PrepareWrite)
                .map_err(ArtifactWriteError::repository)?
                .ok_or_else(|| {
                    ArtifactWriteError::closed(
                        ArtifactWriteCategory::NamespaceUnavailable,
                        ArtifactWriteStage::Namespace,
                        Some(write.id),
                    )
                })?;
            prepared_kinds.push(kind);
            guards.push(directory);
        }
        Ok(guards)
    }
    pub(crate) fn prepare<'repo, 'access, 'runtime, 'guard>(
        self,
        repository: &'repo ArtifactRepository<'access, 'runtime>,
        permit: WritePermit<'guard>,
    ) -> Result<PreparedArtifactTransaction<'repo, 'access, 'runtime, 'guard>, ArtifactWriteError>
    {
        if let Some(expected) = &self.layout_dependency {
            let current = repository
                .load_layout()
                .map_err(ArtifactWriteError::repository)?;
            if current.as_ref().map(|v| v.source()) != expected.as_ref() {
                return Err(ArtifactWriteError::closed(
                    ArtifactWriteCategory::SourceMismatch,
                    ArtifactWriteStage::ReadSource,
                    Some(ArtifactSourceId::DocumentLayout),
                ));
            }
        }
        for source in &self.dependencies {
            progress::checkpoint(false).map_err(ArtifactWriteError::repository)?;
            if !repository
                .reread_bytes_match(source)
                .map_err(ArtifactWriteError::repository)?
            {
                return Err(ArtifactWriteError::closed(
                    ArtifactWriteCategory::SourceMismatch,
                    ArtifactWriteStage::ReadSource,
                    Some(source.id()),
                ));
            }
        }
        if let Some(expected) = &self.document_membership {
            repository
                .confirm_document_membership(expected)
                .map_err(ArtifactWriteError::repository)?;
        }
        // 형식 전환/명시 복원 직전 원본을 영구 보존한다. 모든 permit/source 검사가 먼저다.
        let mut permit = permit;
        if self.writes.iter().any(|w| w.format_transition.is_some()) {
            let mut targets: Vec<_> = self.targets().cloned().collect();
            targets.sort();
            permit
                .validate_for(repository.write_project().fingerprint(), &targets)
                .map_err(|error| {
                    ArtifactWriteError::prepare(transaction::TransactionPrepareError::Permit(error))
                })?;
            for write in &self.writes {
                if write.format_transition.is_some() {
                    let directory = write.guard(repository)?;
                    let bytes = write
                        .read_original(repository.write_project(), &directory)?
                        .ok_or_else(|| {
                            ArtifactWriteError::closed(
                                ArtifactWriteCategory::SourceMismatch,
                                ArtifactWriteStage::ReadSource,
                                Some(write.id),
                            )
                        })?;
                    super::format::preserve(repository, write.id, &bytes)?;
                }
            }
        }
        // 모든 정식 쓰기 경로에서 첨부 참조의 실제 bytes를 확인한다. 낮은 수준의
        // 명령이나 형식 복원도 UI 검증을 우회하여 누락 참조를 확정하지 못한다.
        for write in &self.writes {
            let mut allowed = write.verified_missing_assets.clone();
            if let ArtifactIntent::Replace(_) = &write.intent {
                let directory = write.guard(repository)?;
                if let Some(original) =
                    write.read_original(repository.write_project(), &directory)?
                {
                    let raw = crate::data::json::parse_strict_json_object(&original).map_err(
                        |error| {
                            ArtifactWriteError::io(
                                write.id,
                                ArtifactWriteStage::ReadSource,
                                io::Error::other(error),
                            )
                        },
                    )?;
                    allowed.extend(crate::data::assets::reference_kinds(&raw).map_err(
                        |error| {
                            ArtifactWriteError::io(
                                write.id,
                                ArtifactWriteStage::ReadSource,
                                io::Error::other(error),
                            )
                        },
                    )?);
                }
            }
            crate::data::assets::validate_document_with_missing(
                repository.write_project().canonical_root(),
                &write.bytes,
                &allowed,
            )
            .map_err(|error| {
                ArtifactWriteError::io(
                    write.id,
                    ArtifactWriteStage::ReadSource,
                    io::Error::other(error),
                )
            })?;
        }
        progress::preparing().map_err(ArtifactWriteError::repository)?;
        transaction::prepare_canonical(self, repository, permit)
    }
    // 소비하는 M1에서도 닫힌 write 그대로 보유한다. bytes/source를 따로 넘기지 않는다.
    pub(crate) fn into_writes(self) -> Vec<CanonicalArtifactWrite> {
        self.writes
    }
}

impl CanonicalArtifactWrite {
    pub(crate) fn target(&self) -> &ProjectRelativePath {
        &self.target
    }
    /// 가이드 전환 또는 닫힌 format builder의 전환/복원만 exact source 검사 안에서 허용한다.
    pub(crate) fn allows_schema_transition(
        &self,
        original: crate::data::schema::SchemaVersion,
        staged: crate::data::schema::SchemaVersion,
    ) -> bool {
        (self.guide_schema_upgrade && original.get() == 1 && staged.get() == 2)
            || (self.relation_name_schema_upgrade && original.get() == 5 && staged.get() == 6)
            || self.format_transition == Some((original, staged))
    }
    pub(crate) fn restores_observed_source(&self) -> bool {
        matches!(self.intent, ArtifactIntent::RestoreObserved { .. })
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn guard(
        &self,
        repository: &ArtifactRepository<'_, '_>,
    ) -> Result<ProjectDirectory, ArtifactWriteError> {
        let bound_project = match &self.intent {
            ArtifactIntent::Replace(source) => Some(&source.project),
            ArtifactIntent::RestoreObserved { project, .. } => Some(project),
            ArtifactIntent::Create => None,
        };
        if let Some(project) = bound_project {
            if *project != repository.identity {
                return Err(ArtifactWriteError::closed(
                    ArtifactWriteCategory::SourceMismatch,
                    ArtifactWriteStage::BindSource,
                    Some(self.id),
                ));
            }
        }
        repository
            .namespace(self.id.kind(), RepositoryOperation::PrepareWrite)
            .map_err(ArtifactWriteError::repository)?
            .ok_or_else(|| {
                ArtifactWriteError::closed(
                    ArtifactWriteCategory::NamespaceUnavailable,
                    ArtifactWriteStage::Namespace,
                    Some(self.id),
                )
            })
    }

    /// 할당 전과 backup 직전이 같은 검사를 쓴다. 비교한 handle의 bytes를 그대로 backup에 넘긴다.
    pub(crate) fn read_original(
        &self,
        project: &LockedProject<'_>,
        directory: &ProjectDirectory,
    ) -> Result<Option<Vec<u8>>, ArtifactWriteError> {
        let fail_io = |stage, error| ArtifactWriteError::io(self.id, stage, error);
        directory
            .validate()
            .map_err(|e| fail_io(ArtifactWriteStage::Namespace, e))?;
        let path = project.canonical_root().join(self.target.as_str());
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file() || project_file::directory::is_reparse(&metadata) {
                    return Err(ArtifactWriteError::closed(
                        ArtifactWriteCategory::InvalidEntry,
                        ArtifactWriteStage::ReadSource,
                        Some(self.id),
                    ));
                }
                if matches!(
                    self.intent,
                    ArtifactIntent::Create | ArtifactIntent::RestoreObserved { expected: None, .. }
                ) {
                    return Err(ArtifactWriteError::closed(
                        ArtifactWriteCategory::TargetExists,
                        ArtifactWriteStage::ReadSource,
                        Some(self.id),
                    ));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                directory
                    .validate()
                    .map_err(|e| fail_io(ArtifactWriteStage::Namespace, e))?;
                return match self.intent {
                    ArtifactIntent::Create
                    | ArtifactIntent::RestoreObserved { expected: None, .. } => Ok(None),
                    ArtifactIntent::Replace(_)
                    | ArtifactIntent::RestoreObserved {
                        expected: Some(_), ..
                    } => Err(ArtifactWriteError::closed(
                        ArtifactWriteCategory::SourceMissing,
                        ArtifactWriteStage::ReadSource,
                        Some(self.id),
                    )),
                };
            }
            Err(e) => return Err(fail_io(ArtifactWriteStage::ReadSource, e)),
        }
        let mut file =
            project_file::open_existing_project_file(project.canonical_root(), &self.target)
                .map_err(|e| fail_io(ArtifactWriteStage::ReadSource, e))?;
        directory
            .validate_file(&file, &self.id.filename())
            .map_err(|e| fail_io(ArtifactWriteStage::ReadSource, e))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| fail_io(ArtifactWriteStage::ReadSource, e))?;
        // 개별 file handle은 여기서 닫는다. namespace guard만 commit까지 유지한다.
        drop(file);
        directory
            .validate()
            .map_err(|e| fail_io(ArtifactWriteStage::Namespace, e))?;
        self.validate_source(&bytes)?;
        Ok(Some(bytes))
    }

    fn validate_source(&self, bytes: &[u8]) -> Result<(), ArtifactWriteError> {
        if let ArtifactIntent::RestoreObserved {
            expected: Some((length, digest)),
            ..
        } = &self.intent
        {
            return if bytes.len() == *length && <[u8; 32]>::from(Sha256::digest(bytes)) == *digest {
                Ok(())
            } else {
                Err(ArtifactWriteError::closed(
                    ArtifactWriteCategory::SourceMismatch,
                    ArtifactWriteStage::ReadSource,
                    Some(self.id),
                ))
            };
        }
        let (id, schema) = match self.id {
            ArtifactSourceId::DocumentLayout => {
                artifact::decode_layout(bytes).map_err(|e| {
                    ArtifactWriteError::codec(self.id, ArtifactWriteStage::ReadSource, e)
                })?;
                (
                    ArtifactSourceId::DocumentLayout,
                    crate::data::schema::SchemaVersion::new_unchecked(1),
                )
            }
            ArtifactSourceId::Template(_) => {
                let value = artifact::decode_template(bytes).map_err(|e| {
                    ArtifactWriteError::codec(self.id, ArtifactWriteStage::ReadSource, e)
                })?;
                (
                    ArtifactSourceId::Template(value.template_id()),
                    value.schema_version(),
                )
            }
            ArtifactSourceId::Document(_) => {
                let value = artifact::decode_document(bytes).map_err(|e| {
                    ArtifactWriteError::codec(self.id, ArtifactWriteStage::ReadSource, e)
                })?;
                (
                    ArtifactSourceId::Document(value.document_id()),
                    value.schema_version(),
                )
            }
        };
        let ArtifactIntent::Replace(source) = &self.intent else {
            return Err(ArtifactWriteError::closed(
                ArtifactWriteCategory::TargetExists,
                ArtifactWriteStage::ReadSource,
                Some(self.id),
            ));
        };
        if id != self.id
            || schema != source.schema
            || bytes.len() != source.byte_length
            || <[u8; 32]>::from(Sha256::digest(bytes)) != source.sha256
        {
            return Err(ArtifactWriteError::closed(
                ArtifactWriteCategory::SourceMismatch,
                ArtifactWriteStage::ReadSource,
                Some(self.id),
            ));
        }
        Ok(())
    }
}

impl ArtifactRepository<'_, '_> {
    pub(crate) fn write_project(&self) -> &LockedProject<'_> {
        self.access.locked_project()
    }
}

impl fmt::Debug for CanonicalArtifactWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CanonicalArtifactWrite([redacted])")
    }
}
impl fmt::Debug for CanonicalWritePlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CanonicalWritePlan")
            .field("write_count", &self.writes.len())
            .finish()
    }
}
