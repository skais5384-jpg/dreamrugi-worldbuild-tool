//! 복구 완료 접근권 안에서만 실제 artifact를 읽는 동기식 repository.
mod diagnostics;
pub(crate) mod drafts;
pub(crate) mod format;
pub(crate) mod progress;
mod scan;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(all(test, windows))]
mod tests;
pub(crate) mod versions;
mod write;
mod write_diagnostics;

use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    io::{self, Read},
};

use super::{
    artifact::{
        self,
        template_mutation::{self, TemplateReferenceAssessment},
        ArtifactType, DocumentArtifact, DocumentId, TemplateArtifact, TemplateId,
        TemplateLifecycle, TemplateRevision,
    },
    project_file::{
        self,
        directory::{DirectoryIdentity, ProjectDirectory},
    },
    project_relative_path::ProjectRelativePath,
    project_runtime::RecoveryReadyProject,
    schema::SchemaVersion,
};
pub(crate) use diagnostics::{
    RepositoryCategory, RepositoryDiagnostic, RepositoryError, RepositoryOperation, RepositoryStage,
};
pub(crate) use scan::DocumentScanVisitError;
pub(crate) use write::{CanonicalArtifactWrite, CanonicalWritePlan};
pub(crate) use write_diagnostics::{
    ArtifactCommitDiagnostic, ArtifactCommitError, ArtifactCommitOutcome, ArtifactWriteCategory,
    ArtifactWriteDiagnostic, ArtifactWriteError, ArtifactWriteStage,
};

#[derive(Clone, PartialEq, Eq)]
struct ProjectIdentity {
    fingerprint: String,
    directory: DirectoryIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ArtifactSourceId {
    Template(TemplateId),
    Document(DocumentId),
    DocumentLayout,
}
impl ArtifactSourceId {
    /// generic/migration 경로는 로그에 복사하지 않고 정규 artifact 경로만 typed ID로 투영한다.
    pub(crate) fn from_target(path: &ProjectRelativePath) -> Option<Self> {
        if path.as_str() == "workspace/document-layout.json" {
            return Some(Self::DocumentLayout);
        }
        let (namespace, filename) = path.as_str().split_once('/')?;
        let id = filename.strip_suffix(".json")?;
        let result = match namespace {
            "templates" => Self::Template(id.parse().ok()?),
            "documents" => Self::Document(id.parse().ok()?),
            _ => return None,
        };
        (result.path().ok().as_ref() == Some(path)).then_some(result)
    }
    fn kind(self) -> ArtifactType {
        match self {
            Self::Template(_) => ArtifactType::Template,
            Self::Document(_) => ArtifactType::Document,
            Self::DocumentLayout => ArtifactType::DocumentLayout,
        }
    }
    fn filename(self) -> String {
        match self {
            Self::Template(id) => format!("{id}.json"),
            Self::Document(id) => format!("{id}.json"),
            Self::DocumentLayout => "document-layout.json".into(),
        }
    }
    pub(crate) fn path(
        self,
    ) -> Result<ProjectRelativePath, super::project_relative_path::ProjectRelativePathError> {
        ProjectRelativePath::parse(&format!("{}/{}", namespace(self.kind()), self.filename()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceRevision {
    Template(TemplateRevision),
    /// Document 내용 버전이 아니라 읽은 Document의 역사적 Template binding이다.
    DocumentTemplate(TemplateRevision),
    Layout(u32),
}

/// 실제 decode 입력 bytes의 과거 관찰이다. Serialize/생성자/가변 accessor와 쓰기 권한은 없다.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SourceToken {
    project: ProjectIdentity,
    path: ProjectRelativePath,
    id: ArtifactSourceId,
    byte_length: usize,
    sha256: [u8; 32],
    schema: SchemaVersion,
    revision: SourceRevision,
}
impl SourceToken {
    pub(crate) fn id(&self) -> ArtifactSourceId {
        self.id
    }
    pub(crate) fn path(&self) -> &ProjectRelativePath {
        &self.path
    }
    pub(crate) fn byte_length(&self) -> usize {
        self.byte_length
    }
    pub(crate) fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }
    pub(crate) fn schema(&self) -> SchemaVersion {
        self.schema
    }
    pub(crate) fn revision(&self) -> SourceRevision {
        self.revision
    }
}
impl fmt::Debug for SourceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SourceToken([redacted historical observation])")
    }
}

pub(crate) struct LoadedArtifact<T> {
    artifact: T,
    source: SourceToken,
}
impl<T> LoadedArtifact<T> {
    pub(crate) fn artifact(&self) -> &T {
        &self.artifact
    }
    pub(crate) fn source(&self) -> &SourceToken {
        &self.source
    }
    pub(crate) fn into_artifact(self) -> T {
        self.artifact
    }
}
impl<T> fmt::Debug for LoadedArtifact<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LoadedArtifact([redacted])")
    }
}

pub(crate) struct TemplateRecord {
    source: SourceToken,
    lifecycle: TemplateLifecycle,
    name: String,
    glossary_excluded: bool,
}
impl TemplateRecord {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn source(&self) -> &SourceToken {
        &self.source
    }
    pub(crate) fn lifecycle(&self) -> TemplateLifecycle {
        self.lifecycle
    }
    pub(crate) fn glossary_excluded(&self) -> bool {
        self.glossary_excluded
    }
}
impl fmt::Debug for TemplateRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TemplateRecord([redacted])")
    }
}

pub(crate) struct CompleteTemplateScan {
    project: ProjectIdentity,
    records: Vec<TemplateRecord>,
}
impl CompleteTemplateScan {
    pub(crate) fn records(&self) -> &[TemplateRecord] {
        &self.records
    }
}

#[derive(Clone)]
struct DocumentRecord {
    source: SourceToken,
    template_id: TemplateId,
    name: String,
    english_name: String,
    glossary_summary: String,
    glossary_excluded: bool,
}
/// 본문/unknown metadata를 모으지 않는다. 이 private record들은 전체 codec admission 뒤 생성된다.
#[derive(Clone)]
pub(crate) struct CompleteDocumentScan {
    project: ProjectIdentity,
    records: Vec<DocumentRecord>,
}
/// 앱이 방금 커밋하고 다시 읽어 확정한 한 문서만 기존 strict scan에 연결한다.
/// 이전 source까지 함께 보관하여 외부 변경이나 다른 세대의 cache에는 적용되지 않는다.
#[derive(Clone)]
pub(crate) struct DocumentScanChange {
    project: ProjectIdentity,
    previous: SourceToken,
    current: DocumentRecord,
}
impl DocumentScanChange {
    pub(crate) fn document_id(&self) -> DocumentId {
        match self.current.source.id {
            ArtifactSourceId::Document(id) => id,
            _ => unreachable!("document change always owns a document source"),
        }
    }

    pub(crate) fn current_source(&self) -> &SourceToken {
        &self.current.source
    }

    pub(crate) fn committed(
        previous: SourceToken,
        current: &LoadedArtifact<DocumentArtifact>,
    ) -> Option<Self> {
        let ArtifactSourceId::Document(previous_id) = previous.id else {
            return None;
        };
        let ArtifactSourceId::Document(current_id) = current.source.id else {
            return None;
        };
        if previous_id != current_id || previous.project != current.source.project {
            return None;
        }
        Some(Self {
            project: current.source.project.clone(),
            previous,
            current: DocumentRecord {
                source: current.source.clone(),
                template_id: current.artifact.template_id(),
                name: current.artifact.name().into(),
                english_name: current.artifact.english_name().into(),
                glossary_summary: current.artifact.glossary_summary().into(),
                glossary_excluded: current.artifact.glossary_excluded(),
            },
        })
    }

    /// 보관본의 검증된 원 source 메타데이터로 committed candidate와의 전이를 복원한다.
    /// 현재 경로/project/id는 실제 재독한 candidate에서만 가져오며, 호출자는 보관본의
    /// snapshot·digest와 committed attempt가 이 candidate를 가리킴을 먼저 증명해야 한다.
    pub(crate) fn recovered_commit(
        previous_id: DocumentId,
        previous_byte_length: usize,
        previous_sha256: [u8; 32],
        previous_schema: SchemaVersion,
        previous_revision: TemplateRevision,
        current: &LoadedArtifact<DocumentArtifact>,
    ) -> Option<Self> {
        let ArtifactSourceId::Document(current_id) = current.source.id else {
            return None;
        };
        if previous_id != current_id {
            return None;
        }
        Self::committed(
            SourceToken {
                project: current.source.project.clone(),
                path: current.source.path.clone(),
                id: ArtifactSourceId::Document(previous_id),
                byte_length: previous_byte_length,
                sha256: previous_sha256,
                schema: previous_schema,
                revision: SourceRevision::DocumentTemplate(previous_revision),
            },
            current,
        )
    }
}
impl CompleteDocumentScan {
    pub(crate) fn summaries(
        &self,
    ) -> impl Iterator<Item = (DocumentId, TemplateId, &str, &str, &str, bool)> {
        self.records.iter().filter_map(|r| match r.source.id {
            ArtifactSourceId::Document(id) => Some((
                id,
                r.template_id,
                r.name.as_str(),
                r.english_name.as_str(),
                r.glossary_summary.as_str(),
                r.glossary_excluded,
            )),
            _ => None,
        })
    }
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }
    pub(crate) fn sources(&self) -> impl Iterator<Item = &SourceToken> {
        self.records.iter().map(|r| &r.source)
    }

    pub(crate) fn source(&self, id: DocumentId) -> Option<&SourceToken> {
        self.records.iter().find_map(|record| {
            (record.source.id == ArtifactSourceId::Document(id)).then_some(&record.source)
        })
    }

    /// 같은 프로젝트에서 같은 이전 source를 본 scan만 새 불변 scan으로 바꾼다.
    /// 이미 적용된 응답 재전달은 허용하지만, 다른 source를 본 cache는 그대로 남겨
    /// 다음 prepare의 전체 membership/실제 bytes 대조가 외부 변경을 거절하게 한다.
    pub(crate) fn apply_committed_change(&self, change: &DocumentScanChange) -> Option<Self> {
        if self.project != change.project {
            return None;
        }
        let target = change.current.source.id;
        let index = self
            .records
            .iter()
            .position(|record| record.source.id == target)?;
        let observed = &self.records[index].source;
        if observed == &change.current.source {
            return Some(self.clone());
        }
        if observed != &change.previous {
            return None;
        }
        let mut next = self.clone();
        next.records[index] = change.current.clone();
        Some(next)
    }
}

/// target 원본과 같은 프로젝트의 실제 전체 scan을 함께 소유한다. 독립 count/slice는 받지 않는다.
pub(crate) struct CompleteTemplateReferenceScan {
    source: LoadedArtifact<TemplateArtifact>,
    documents: CompleteDocumentScan,
}
impl CompleteTemplateReferenceScan {
    pub(crate) fn source(&self) -> &LoadedArtifact<TemplateArtifact> {
        &self.source
    }
    pub(crate) fn template_ids(&self) -> impl Iterator<Item = TemplateId> + '_ {
        self.documents.records.iter().map(|r| r.template_id)
    }
}
macro_rules! redacted_scan_debug {
    ($($name:ty),+ $(,)?) => { $(impl fmt::Debug for $name {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($name), "([redacted complete observation])")) }
    })+ };
}
redacted_scan_debug!(
    CompleteTemplateScan,
    CompleteDocumentScan,
    CompleteTemplateReferenceScan
);

/// root handle도 접근권 borrow 안에서만 유지한다. owned token/decoded model로 재개방할 수 없다.
pub(crate) struct ArtifactRepository<'access, 'runtime> {
    access: &'access RecoveryReadyProject<'runtime>,
    root: ProjectDirectory,
    identity: ProjectIdentity,
}
impl fmt::Debug for ArtifactRepository<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ArtifactRepository([redacted])")
    }
}

impl<'access, 'runtime> ArtifactRepository<'access, 'runtime> {
    pub(crate) fn new(
        access: &'access RecoveryReadyProject<'runtime>,
    ) -> Result<Self, RepositoryError> {
        let project = access.locked_project();
        let root = ProjectDirectory::open_root(project.canonical_root()).map_err(|e| {
            RepositoryError::io(
                RepositoryCategory::RootUnavailable,
                RepositoryOperation::Open,
                RepositoryStage::Root,
                None,
                e,
            )
        })?;
        let identity = ProjectIdentity {
            fingerprint: project.fingerprint().to_owned(),
            directory: root.identity(),
        };
        Ok(Self {
            access,
            root,
            identity,
        })
    }

    pub(crate) fn load_template(
        &self,
        id: TemplateId,
    ) -> Result<LoadedArtifact<TemplateArtifact>, RepositoryError> {
        let operation = RepositoryOperation::LoadTemplate;
        let directory = self
            .namespace(ArtifactType::Template, operation)?
            .ok_or_else(|| {
                not_found(
                    operation,
                    ArtifactType::Template,
                    RepositoryStage::Namespace,
                )
            })?;
        self.read_template(id, &directory, operation, false)
    }
    pub(crate) fn load_document(
        &self,
        id: DocumentId,
    ) -> Result<LoadedArtifact<DocumentArtifact>, RepositoryError> {
        let operation = RepositoryOperation::LoadDocument;
        let directory = self
            .namespace(ArtifactType::Document, operation)?
            .ok_or_else(|| {
                not_found(
                    operation,
                    ArtifactType::Document,
                    RepositoryStage::Namespace,
                )
            })?;
        self.read_document(id, &directory, operation, false)
    }

    /// 다른 프로젝트의 token은 contents가 같아도 비교를 시작하지 않는다. 저장 admission은 G4 책임이다.
    pub(crate) fn reread_matches(&self, token: &SourceToken) -> Result<bool, RepositoryError> {
        if token.project != self.identity {
            return Err(RepositoryError::new(
                RepositoryCategory::SourceMismatch,
                RepositoryOperation::Open,
                RepositoryStage::BindSource,
                Some(token.id.kind()),
            ));
        }
        let current = match token.id {
            ArtifactSourceId::Template(id) => self.load_template(id)?.source,
            ArtifactSourceId::Document(id) => self.load_document(id)?.source,
            ArtifactSourceId::DocumentLayout => match self.load_layout()? {
                Some(v) => v.source,
                None => return Ok(false),
            },
        };
        Ok(current == *token)
    }

    /// 이미 strict decode가 끝난 source를 커밋 직전 다시 대조한다. 동일 SHA-256/길이는
    /// 같은 원문을 뜻하므로 두 번째 JSON decode 없이도 외부 변경 감지를 그대로 유지한다.
    pub(crate) fn reread_bytes_match(&self, token: &SourceToken) -> Result<bool, RepositoryError> {
        if token.project != self.identity {
            return Err(RepositoryError::new(
                RepositoryCategory::SourceMismatch,
                RepositoryOperation::Open,
                RepositoryStage::BindSource,
                Some(token.id.kind()),
            ));
        }
        let operation = match token.id {
            ArtifactSourceId::Template(_) => RepositoryOperation::LoadTemplate,
            ArtifactSourceId::Document(_) | ArtifactSourceId::DocumentLayout => {
                RepositoryOperation::LoadDocument
            }
        };
        let kind = token.id.kind();
        let Some(directory) = self.namespace(kind, operation)? else {
            return Ok(false);
        };
        let (bytes, path) = match self.read_bytes(token.id, &directory, operation, false) {
            Ok(value) => value,
            Err(error) if error.diagnostic().category == RepositoryCategory::NotFound => {
                return Ok(false)
            }
            Err(error) => return Err(error),
        };
        #[cfg(test)]
        test_support::hashed();
        Ok(path == token.path
            && bytes.len() == token.byte_length
            && <[u8; 32]>::from(Sha256::digest(&bytes)) == token.sha256)
    }

    /// 커밋된 신규 문서 하나만 현재 repository에서 다시 읽어, 이미 재대조한 scan을
    /// 다음 UI 작업용 snapshot으로 확장한다. 기존 항목의 쓰기 권한은 발급하지 않는다.
    pub(crate) fn extend_document_scan(
        &self,
        scan: &CompleteDocumentScan,
        id: DocumentId,
    ) -> Result<CompleteDocumentScan, RepositoryError> {
        if scan.project != self.identity {
            return Err(RepositoryError::new(
                RepositoryCategory::SourceMismatch,
                RepositoryOperation::Open,
                RepositoryStage::BindSource,
                Some(ArtifactType::Document),
            ));
        }
        let loaded = self.load_document(id)?;
        if scan
            .sources()
            .any(|source| source.id() == loaded.source.id())
        {
            return Err(RepositoryError::new(
                RepositoryCategory::SourceMismatch,
                RepositoryOperation::Open,
                RepositoryStage::Complete,
                Some(ArtifactType::Document),
            ));
        }
        let mut next = scan.clone();
        next.records.push(DocumentRecord {
            template_id: loaded.artifact.template_id(),
            name: loaded.artifact.name().into(),
            english_name: loaded.artifact.english_name().into(),
            glossary_summary: loaded.artifact.glossary_summary().into(),
            glossary_excluded: loaded.artifact.glossary_excluded(),
            source: loaded.source,
        });
        Ok(next)
    }

    pub(crate) fn scan_template_references(
        &self,
        target: TemplateId,
    ) -> Result<CompleteTemplateReferenceScan, RepositoryError> {
        #[cfg(test)]
        test_support::references(false);
        let source = self.load_template(target)?;
        let documents = self.scan_documents()?;
        // 두 입력은 caller가 건넨 값이 아니라 이 repository의 이번 실제 I/O 결과다.
        Ok(CompleteTemplateReferenceScan { source, documents })
    }
    pub(crate) fn assess_template_references(
        &self,
        target: TemplateId,
    ) -> Result<TemplateReferenceAssessment, RepositoryError> {
        #[cfg(test)]
        test_support::references(true);
        template_mutation::assess_repository_template_references(
            self.scan_template_references(target)?,
        )
        .map_err(RepositoryError::reference)
    }

    fn namespace(
        &self,
        kind: ArtifactType,
        operation: RepositoryOperation,
    ) -> Result<Option<ProjectDirectory>, RepositoryError> {
        self.root.validate().map_err(|e| {
            RepositoryError::io(
                RepositoryCategory::RootUnavailable,
                operation,
                RepositoryStage::Root,
                Some(kind),
                e,
            )
        })?;
        self.root.open_namespace(namespace(kind)).map_err(|e| {
            RepositoryError::io(
                RepositoryCategory::NamespaceUnavailable,
                operation,
                RepositoryStage::Namespace,
                Some(kind),
                e,
            )
        })
    }

    fn read_bytes(
        &self,
        id: ArtifactSourceId,
        directory: &ProjectDirectory,
        operation: RepositoryOperation,
        observed: bool,
    ) -> Result<(Vec<u8>, ProjectRelativePath), RepositoryError> {
        let kind = id.kind();
        let path = id.path().map_err(|_| {
            RepositoryError::new(
                RepositoryCategory::SourceMismatch,
                operation,
                RepositoryStage::BindSource,
                Some(kind),
            )
        })?;
        let requested = self
            .access
            .locked_project()
            .canonical_root()
            .join(path.as_str());
        #[cfg(test)]
        test_support::point(RepositoryStage::EntryMetadata, &requested).map_err(|e| {
            RepositoryError::io(
                RepositoryCategory::MetadataFailed,
                operation,
                RepositoryStage::EntryMetadata,
                Some(kind),
                e,
            )
        })?;
        let metadata = match fs::symlink_metadata(&requested) {
            Ok(metadata) => metadata,
            Err(error) if !observed && error.kind() == io::ErrorKind::NotFound => {
                self.finish_namespace(directory, operation, kind)?;
                return Err(RepositoryError::io(
                    RepositoryCategory::NotFound,
                    operation,
                    RepositoryStage::EntryMetadata,
                    Some(kind),
                    error,
                ));
            }
            Err(error) => {
                return Err(RepositoryError::io(
                    RepositoryCategory::MetadataFailed,
                    operation,
                    RepositoryStage::EntryMetadata,
                    Some(kind),
                    error,
                ))
            }
        };
        if !metadata.is_file() || project_file::directory::is_reparse(&metadata) {
            return Err(RepositoryError::new(
                RepositoryCategory::InvalidEntry,
                operation,
                RepositoryStage::EntryMetadata,
                Some(kind),
            ));
        }
        #[cfg(test)]
        test_support::point(RepositoryStage::OpenFile, &requested).map_err(|e| {
            RepositoryError::io(
                RepositoryCategory::OpenFailed,
                operation,
                RepositoryStage::OpenFile,
                Some(kind),
                e,
            )
        })?;
        let mut file = directory
            .open_child_candidate(&id.filename())
            .map_err(|e| {
                RepositoryError::io(
                    RepositoryCategory::OpenFailed,
                    operation,
                    RepositoryStage::OpenFile,
                    Some(kind),
                    e,
                )
            })?;
        directory
            .validate_file(&file, &id.filename())
            .map_err(|e| {
                RepositoryError::io(
                    RepositoryCategory::SourceMismatch,
                    operation,
                    RepositoryStage::BindSource,
                    Some(kind),
                    e,
                )
            })?;
        #[cfg(test)]
        test_support::point(RepositoryStage::ReadFile, &requested).map_err(|e| {
            RepositoryError::io(
                RepositoryCategory::ReadFailed,
                operation,
                RepositoryStage::ReadFile,
                Some(kind),
                e,
            )
        })?;
        let mut bytes = Vec::new();
        // canonical encode나 재개방 없이 검증된 바로 그 handle의 원본 bytes를 decode에 전달한다.
        let read = file.read_to_end(&mut bytes);
        #[cfg(test)]
        test_support::read_bytes(bytes.len());
        read.map_err(|e| {
            RepositoryError::io(
                RepositoryCategory::ReadFailed,
                operation,
                RepositoryStage::ReadFile,
                Some(kind),
                e,
            )
        })?;
        // The complete scanners own this guarded directory through their final
        // namespace validation. Individual loads still validate after the read.
        if !observed {
            self.finish_namespace(directory, operation, kind)?;
        }
        Ok((bytes, path))
    }

    pub(crate) fn load_layout(
        &self,
    ) -> Result<Option<LoadedArtifact<artifact::layout::DocumentLayout>>, RepositoryError> {
        let operation = RepositoryOperation::LoadDocument;
        let kind = ArtifactType::DocumentLayout;
        let Some(directory) = self.namespace(kind, operation)? else {
            return Ok(None);
        };
        let id = ArtifactSourceId::DocumentLayout;
        let (bytes, path) = match self.read_bytes(id, &directory, operation, false) {
            Ok(v) => v,
            Err(e) if e.diagnostic().category == RepositoryCategory::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let artifact = artifact::decode_layout(&bytes)
            .map_err(|e| RepositoryError::codec(operation, kind, e))?;
        let source = self.token(
            id,
            path,
            &bytes,
            crate::data::schema::SchemaVersion::new_unchecked(1),
            SourceRevision::Layout(artifact.revision),
        );
        Ok(Some(LoadedArtifact { artifact, source }))
    }
    fn read_template(
        &self,
        id: TemplateId,
        directory: &ProjectDirectory,
        operation: RepositoryOperation,
        observed: bool,
    ) -> Result<LoadedArtifact<TemplateArtifact>, RepositoryError> {
        let result = (|| {
            let (bytes, path) = self.read_bytes(
                ArtifactSourceId::Template(id),
                directory,
                operation,
                observed,
            )?;
            let artifact = artifact::decode_template(&bytes)
                .map_err(|e| RepositoryError::codec(operation, ArtifactType::Template, e))?;
            if artifact.template_id() != id {
                return Err(id_mismatch(operation, ArtifactType::Template));
            }
            let source = self.token(
                ArtifactSourceId::Template(id),
                path,
                &bytes,
                artifact.schema_version(),
                SourceRevision::Template(artifact.revision()),
            );
            #[cfg(test)]
            test_support::decoded();
            Ok(LoadedArtifact { artifact, source })
        })();
        result.map_err(|error: RepositoryError| error.at_source(ArtifactSourceId::Template(id)))
    }
    fn read_document(
        &self,
        id: DocumentId,
        directory: &ProjectDirectory,
        operation: RepositoryOperation,
        observed: bool,
    ) -> Result<LoadedArtifact<DocumentArtifact>, RepositoryError> {
        let result = (|| {
            let (bytes, path) = self.read_bytes(
                ArtifactSourceId::Document(id),
                directory,
                operation,
                observed,
            )?;
            let artifact = artifact::decode_document(&bytes)
                .map_err(|e| RepositoryError::codec(operation, ArtifactType::Document, e))?;
            if artifact.document_id() != id {
                return Err(id_mismatch(operation, ArtifactType::Document));
            }
            let source = self.token(
                ArtifactSourceId::Document(id),
                path,
                &bytes,
                artifact.schema_version(),
                SourceRevision::DocumentTemplate(artifact.template_revision()),
            );
            #[cfg(test)]
            test_support::decoded();
            Ok(LoadedArtifact { artifact, source })
        })();
        result.map_err(|error: RepositoryError| error.at_source(ArtifactSourceId::Document(id)))
    }
    fn token(
        &self,
        id: ArtifactSourceId,
        path: ProjectRelativePath,
        bytes: &[u8],
        schema: SchemaVersion,
        revision: SourceRevision,
    ) -> SourceToken {
        #[cfg(test)]
        test_support::hashed();
        SourceToken {
            project: self.identity.clone(),
            path,
            id,
            byte_length: bytes.len(),
            sha256: Sha256::digest(bytes).into(),
            schema,
            revision,
        }
    }
    fn finish_namespace(
        &self,
        directory: &ProjectDirectory,
        operation: RepositoryOperation,
        kind: ArtifactType,
    ) -> Result<(), RepositoryError> {
        self.root
            .validate()
            .and_then(|()| directory.validate())
            .map_err(|e| {
                RepositoryError::io(
                    RepositoryCategory::NamespaceUnavailable,
                    operation,
                    RepositoryStage::Complete,
                    Some(kind),
                    e,
                )
            })
    }
}

fn namespace(kind: ArtifactType) -> &'static str {
    match kind {
        ArtifactType::Template => "templates",
        ArtifactType::Document => "documents",
        ArtifactType::DocumentLayout => "workspace",
    }
}
fn not_found(
    operation: RepositoryOperation,
    kind: ArtifactType,
    stage: RepositoryStage,
) -> RepositoryError {
    RepositoryError::new(RepositoryCategory::NotFound, operation, stage, Some(kind))
}
fn id_mismatch(operation: RepositoryOperation, kind: ArtifactType) -> RepositoryError {
    RepositoryError::new(
        RepositoryCategory::IdMismatch,
        operation,
        RepositoryStage::BindSource,
        Some(kind),
    )
}
