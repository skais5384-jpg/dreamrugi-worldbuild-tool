use serde::Serialize;
use std::{fmt, io};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    Initialization,
    Permission,
    Capacity,
    Busy,
    Unavailable,
    UnsafePath,
    InvalidId,
    InvalidEnvelope,
    UnsupportedVersion,
    UnsupportedKind,
    Corrupt,
    TooLarge,
    TooDeep,
    DigestMismatch,
    Conflict,
    StaleGeneration,
    Io,
    DurabilityUncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    Initialize,
    Lock,
    Validate,
    Create,
    Write,
    Flush,
    Sync,
    Publish,
    Reopen,
    Read,
    List,
    Revalidate,
    Discard,
}

/// 원인과 정리 실패는 보존하지만 임의 OS 문자열·본문·절대 경로는 formatting하지 않는다.
pub(crate) struct RecoveryError {
    pub(crate) category: Category,
    pub(crate) stage: Stage,
    pub(crate) source: Option<io::Error>,
    pub(crate) cleanup: Option<io::Error>,
    pub(crate) published: bool,
    // serde/codec 원인은 private owner로 남기고 자동 Error::source에 노출하지 않는다.
    cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}
impl RecoveryError {
    pub(crate) fn new(category: Category, stage: Stage) -> Self {
        Self {
            category,
            stage,
            source: None,
            cleanup: None,
            published: false,
            cause: None,
        }
    }
    pub(crate) fn caused(
        category: Category,
        stage: Stage,
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            cause: Some(Box::new(cause)),
            ..Self::new(category, stage)
        }
    }
    pub(crate) fn io(stage: Stage, source: io::Error) -> Self {
        // Only an actual sharing/lock violation means another app owns the
        // Store. Other lock-path failures need the generic recovery action.
        let held_elsewhere = stage == Stage::Lock
            && (matches!(source.raw_os_error(), Some(32 | 33))
                || source.kind() == io::ErrorKind::WouldBlock);
        let category = match source.kind() {
            _ if held_elsewhere => Category::Busy,
            io::ErrorKind::PermissionDenied => Category::Permission,
            io::ErrorKind::StorageFull => Category::Capacity,
            io::ErrorKind::Unsupported => Category::Unavailable,
            io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput => Category::UnsafePath,
            _ if stage == Stage::Initialize => Category::Initialization,
            _ => Category::Io,
        };
        Self {
            source: Some(source),
            ..Self::new(category, stage)
        }
    }
    pub(crate) fn dto(&self) -> ErrorDto {
        ErrorDto { category: self.category, stage: self.stage, published: self.published,
            io_kind: self.source.as_ref().map(|e| format!("{:?}", e.kind())),
            os_code: self.source.as_ref().and_then(io::Error::raw_os_error),
            boundary_reason: self.source.as_ref().and_then(crate::data::project_file::directory::location_mismatch_reason),
            cleanup_failed: self.cleanup.is_some(), cause_retained: self.cause.is_some(),
            next_action: "입력과 복구 파일을 보존하세요. 접근 권한·여유 공간과 다른 앱을 확인한 뒤 같은 보관 ID로 재검증하세요" }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ErrorDto {
    pub(crate) category: Category,
    pub(crate) stage: Stage,
    pub(crate) published: bool,
    pub(crate) io_kind: Option<String>,
    pub(crate) os_code: Option<i32>,
    pub(crate) boundary_reason: Option<&'static str>,
    pub(crate) cleanup_failed: bool,
    pub(crate) cause_retained: bool,
    pub(crate) next_action: &'static str,
}
impl fmt::Debug for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.dto().fmt(f)
    }
}
impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "복구 보관 {:?}/{:?}: {}",
            self.stage,
            self.category,
            self.dto().next_action
        )
    }
}
impl std::error::Error for RecoveryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_path_failure_without_sharing_violation_is_not_other_window_busy() {
        let error = RecoveryError::io(Stage::Lock, io::Error::other("lock file I/O"));
        assert_eq!(error.category, Category::Io);
    }

    #[cfg(windows)]
    #[test]
    fn windows_store_sharing_violation_is_other_window_busy() {
        let error = RecoveryError::io(Stage::Lock, io::Error::from_raw_os_error(32));
        assert_eq!(error.category, Category::Busy);
    }
}
