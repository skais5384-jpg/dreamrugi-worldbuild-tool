use std::{fmt, io};

use crate::data::artifact::{
    template_mutation::TemplateMutationError, ArtifactCodecError, ArtifactType,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepositoryCategory {
    Cancelled,
    RootUnavailable,
    NamespaceUnavailable,
    InvalidEntry,
    NotFound,
    IterationFailed,
    MetadataFailed,
    OpenFailed,
    ReadFailed,
    CodecRejected,
    IdMismatch,
    DuplicateId,
    DuplicateSource,
    SourceMismatch,
    ReferenceBlocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepositoryOperation {
    Open,
    LoadTemplate,
    LoadDocument,
    ScanTemplates,
    ScanDocuments,
    AssessReferences,
    PrepareWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepositoryStage {
    Root,
    Namespace,
    ReadDirectory,
    Iterate,
    EntryName,
    EntryMetadata,
    OpenFile,
    ReadFile,
    Decode,
    BindSource,
    Complete,
    References,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RepositoryDiagnostic {
    pub(crate) category: RepositoryCategory,
    pub(crate) operation: RepositoryOperation,
    pub(crate) stage: RepositoryStage,
    pub(crate) artifact_type: Option<ArtifactType>,
    pub(crate) source_id: Option<super::ArtifactSourceId>,
    pub(crate) io_kind: Option<io::ErrorKind>,
    pub(crate) os_code: Option<i32>,
    pub(crate) codec: Option<ArtifactCodecError>,
    pub(crate) reference: Option<TemplateMutationError>,
}

impl RepositoryDiagnostic {
    pub(crate) fn next_action(&self) -> &'static str {
        if self.io_kind == Some(io::ErrorKind::Unsupported) {
            return "현재 플랫폼에서는 안전한 자료 읽기를 지원하지 않습니다. 지원 환경에서 열거나 프로젝트를 닫으세요";
        }
        match self.category {
            RepositoryCategory::NotFound => "목록을 새로 읽어 대상이 있는지 확인하거나 작업을 취소하세요",
            RepositoryCategory::ReferenceBlocked => "참조하는 문서가 있으므로 템플릿을 유지하고 문서 목록을 확인하세요",
            RepositoryCategory::CodecRejected => "원본을 보존하고 파일 형식과 앱 버전을 확인한 뒤 다시 읽거나 프로젝트를 닫으세요",
            _ => "원본을 보존하고 프로젝트 위치와 접근 권한을 확인한 뒤 전체 작업을 다시 실행하거나 프로젝트를 닫으세요",
        }
    }
}

/// 원래 I/O 원인은 진단용으로 보관하지만 임의 path/credential을 가진 source chain은 공개하지 않는다.
pub(crate) struct RepositoryError {
    diagnostic: Box<RepositoryDiagnostic>,
    original_io: Option<io::Error>,
}

impl RepositoryError {
    pub(super) fn new(
        category: RepositoryCategory,
        operation: RepositoryOperation,
        stage: RepositoryStage,
        artifact_type: Option<ArtifactType>,
    ) -> Self {
        Self {
            diagnostic: Box::new(RepositoryDiagnostic {
                category,
                operation,
                stage,
                artifact_type,
                source_id: None,
                io_kind: None,
                os_code: None,
                codec: None,
                reference: None,
            }),
            original_io: None,
        }
    }
    pub(super) fn io(
        category: RepositoryCategory,
        operation: RepositoryOperation,
        stage: RepositoryStage,
        artifact_type: Option<ArtifactType>,
        error: io::Error,
    ) -> Self {
        let mut result = Self::new(category, operation, stage, artifact_type);
        result.diagnostic.io_kind = Some(error.kind());
        result.diagnostic.os_code = error.raw_os_error();
        result.original_io = Some(error);
        result
    }
    pub(super) fn codec(
        operation: RepositoryOperation,
        kind: ArtifactType,
        error: ArtifactCodecError,
    ) -> Self {
        let mut result = Self::new(
            RepositoryCategory::CodecRejected,
            operation,
            RepositoryStage::Decode,
            Some(kind),
        );
        result.diagnostic.codec = Some(error);
        result
    }
    pub(super) fn reference(error: TemplateMutationError) -> Self {
        let mut result = Self::new(
            RepositoryCategory::ReferenceBlocked,
            RepositoryOperation::AssessReferences,
            RepositoryStage::References,
            Some(ArtifactType::Template),
        );
        result.diagnostic.reference = Some(error);
        result
    }
    pub(crate) fn diagnostic(&self) -> RepositoryDiagnostic {
        *self.diagnostic
    }
    pub(super) fn at_source(mut self, id: super::ArtifactSourceId) -> Self {
        self.diagnostic.source_id = Some(id);
        self
    }
}
impl fmt::Debug for RepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RepositoryError")
            .field(&self.diagnostic)
            .finish()
    }
}
impl fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "자료 읽기 실패 ({:?}/{:?}/{:?}): {}",
            self.diagnostic.operation,
            self.diagnostic.stage,
            self.diagnostic.category,
            self.diagnostic.next_action()
        )
    }
}
impl std::error::Error for RepositoryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn original_io_is_preserved_privately_without_joining_the_error_chain() {
        let secret = "C:/Users/private-user/secret.json?credential=repository-canary";
        let error = RepositoryError::io(
            RepositoryCategory::ReadFailed,
            RepositoryOperation::LoadDocument,
            RepositoryStage::ReadFile,
            Some(ArtifactType::Document),
            io::Error::other(secret),
        );
        assert_eq!(error.original_io.as_ref().unwrap().to_string(), secret);
        assert!(!format!("{error:?} {error} {:?}", error.diagnostic()).contains(secret));
        assert!(error.source().is_none());
        assert_eq!(error.diagnostic().io_kind, Some(io::ErrorKind::Other));
    }
}
