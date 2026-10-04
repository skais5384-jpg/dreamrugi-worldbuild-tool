use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::super::schema::SchemaVersion;
use super::super::utc_time::is_utc_milliseconds;
use super::{
    backup_artifact_path, is_lowercase_sha256, staged_artifact_path, ProjectRelativePath,
    TransactionId, TransactionModelError, TransactionModelResult,
};

/// transaction journal은 프로젝트 데이터와 독립적으로 호환성을 판정한다.
pub(crate) const TRANSACTION_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new_unchecked(1);
/// canonical owned journal만 v2를 사용한다. artifact schema와는 독립적이다.
pub(super) const OWNED_TRANSACTION_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new_unchecked(2);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransactionManifest {
    pub(crate) schema_version: SchemaVersion,
    pub(crate) transaction_id: TransactionId,
    pub(crate) created_at_utc: String,
    pub(crate) project_fingerprint: String,
    pub(crate) operations: Vec<TransactionOperation>,
}

impl TransactionManifest {
    /// Deserialize 성공만으로 journal을 신뢰하지 않고 모든 교차 필드 불변 조건을 검사한다.
    pub(crate) fn validate(&self) -> TransactionModelResult<()> {
        validate_schema(self.schema_version)?;
        validate_common(
            &self.transaction_id,
            &self.created_at_utc,
            &self.project_fingerprint,
        )?;
        if self.operations.is_empty() {
            return Err(TransactionModelError::invalid_journal(
                Some(&self.transaction_id),
                "operations",
                "at least one operation is required",
            ));
        }

        let mut targets = BTreeSet::new();
        let mut previous: Option<&ProjectRelativePath> = None;
        for (position, operation) in self.operations.iter().enumerate() {
            operation.validate(Some(&self.transaction_id))?;
            if usize::try_from(operation.index).ok() != Some(position) {
                return Err(TransactionModelError::invalid_journal(
                    Some(&self.transaction_id),
                    "operations.index",
                    "operation indexes must be continuous from zero",
                ));
            }
            if let Some(previous_target) = previous {
                if previous_target > &operation.target_path {
                    return Err(TransactionModelError::invalid_journal(
                        Some(&self.transaction_id),
                        "operations.targetPath",
                        "operations must be sorted by validated target path",
                    ));
                }
            }
            if !targets.insert(&operation.target_path) {
                return Err(TransactionModelError::DuplicateTarget {
                    target: operation.target_path.clone(),
                });
            }
            previous = Some(&operation.target_path);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransactionOperation {
    pub(crate) index: u32,
    pub(crate) target_path: ProjectRelativePath,
    pub(crate) staged_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) backup_path: Option<String>,
    pub(crate) original_existed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) original_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) original_sha256: Option<String>,
    pub(crate) staged_size: u64,
    pub(crate) staged_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) staged_schema_version: Option<SchemaVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) original_schema_version: Option<SchemaVersion>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) original_raw: bool,
}

impl TransactionOperation {
    fn validate(&self, transaction_id: Option<&TransactionId>) -> TransactionModelResult<()> {
        if self.original_raw
            && (!self.original_existed
                || self.original_schema_version.is_some()
                || self.staged_schema_version.is_none()
                || !matches!(
                    crate::data::repository::ArtifactSourceId::from_target(&self.target_path),
                    Some(
                        crate::data::repository::ArtifactSourceId::Template(_)
                            | crate::data::repository::ArtifactSourceId::Document(_)
                    )
                ))
        {
            return Err(TransactionModelError::invalid_journal(
                transaction_id,
                "operations.originalRaw",
                "raw originals require an observed managed artifact replacement",
            ));
        }
        if self.staged_path != staged_artifact_path(self.index) {
            return Err(TransactionModelError::invalid_journal(
                transaction_id,
                "operations.stagedPath",
                "staged artifact path must use its operation index",
            ));
        }
        if !is_lowercase_sha256(&self.staged_sha256) {
            return Err(TransactionModelError::invalid_journal(
                transaction_id,
                "operations.stagedSha256",
                "SHA-256 must be 64 lowercase hexadecimal digits",
            ));
        }

        if self.original_existed {
            if self.backup_path.as_deref() != Some(backup_artifact_path(self.index).as_str())
                || self.original_size.is_none()
                || self
                    .original_sha256
                    .as_deref()
                    .is_none_or(|hash| !is_lowercase_sha256(hash))
            {
                return Err(TransactionModelError::invalid_journal(
                    transaction_id,
                    "operations.original",
                    "existing originals require fixed backup path, size, and SHA-256",
                ));
            }
        } else if self.backup_path.is_some()
            || self.original_size.is_some()
            || self.original_schema_version.is_some()
            || self.original_sha256.is_some()
        {
            return Err(TransactionModelError::invalid_journal(
                transaction_id,
                "operations.original",
                "new targets must not contain backup, original size, or original SHA-256",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TransactionState {
    Preparing,
    Prepared,
    Applying,
    RollingBack,
    Committed,
    RolledBack,
    CleaningCommitted,
    CleaningRolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransactionStateRecord {
    pub(crate) schema_version: SchemaVersion,
    pub(crate) transaction_id: TransactionId,
    pub(crate) updated_at_utc: String,
    pub(crate) project_fingerprint: String,
    pub(crate) state: TransactionState,
    /// 진행 정보는 진단과 재개 판단을 돕지만 복구 정확성의 유일한 근거가 아니다.
    pub(crate) applied_operations: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) original_targets: Option<Vec<OriginalTarget>>,
}

impl TransactionStateRecord {
    pub(crate) fn validate(&self) -> TransactionModelResult<()> {
        validate_schema(self.schema_version)?;
        validate_common(
            &self.transaction_id,
            &self.updated_at_utc,
            &self.project_fingerprint,
        )?;
        match &self.original_targets {
            Some(targets)
                if self.schema_version == OWNED_TRANSACTION_SCHEMA_VERSION
                    && !targets.is_empty() =>
            {
                let mut previous = None;
                for target in targets {
                    let valid = match (
                        &target.original_size,
                        &target.original_sha256,
                        &target.original_schema_version,
                    ) {
                        (None, None, None) if !target.original_raw => true,
                        (Some(_), Some(hash), Some(_)) if !target.original_raw => {
                            is_lowercase_sha256(hash)
                        }
                        (Some(_), Some(hash), None) if target.original_raw => {
                            is_lowercase_sha256(hash)
                        }
                        _ => false,
                    };
                    if !valid || previous.is_some_and(|p| p >= &target.target_path) {
                        return Err(TransactionModelError::invalid_journal(
                            Some(&self.transaction_id),
                            "originalTargets",
                            "invalid original target evidence",
                        ));
                    }
                    previous = Some(&target.target_path);
                }
            }
            None if self.schema_version == TRANSACTION_SCHEMA_VERSION => {}
            _ => {
                return Err(TransactionModelError::invalid_journal(
                    Some(&self.transaction_id),
                    "originalTargets",
                    "owned state requires original target evidence",
                ))
            }
        }
        if matches!(
            self.state,
            TransactionState::CleaningCommitted | TransactionState::CleaningRolledBack
        ) && (self.schema_version != OWNED_TRANSACTION_SCHEMA_VERSION
            || !self.applied_operations.is_empty())
        {
            return Err(TransactionModelError::invalid_journal(
                Some(&self.transaction_id),
                "state",
                "cleanup requires owned protocol and empty progress",
            ));
        }
        if self
            .applied_operations
            .iter()
            .enumerate()
            .any(|(position, index)| usize::try_from(*index).ok() != Some(position))
        {
            return Err(TransactionModelError::invalid_journal(
                Some(&self.transaction_id),
                "appliedOperations",
                "progress indexes must be a continuous prefix from zero",
            ));
        }
        Ok(())
    }
}

/// manifest가 사라져도 Preparing 재생과 현재 namespace의 모순을 확인할 범위다.
/// payload/임의 경로를 기록하지 않고 기존 M1 원본 지문만 중복 보유한다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OriginalTarget {
    pub(super) target_path: ProjectRelativePath,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) original_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) original_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) original_schema_version: Option<SchemaVersion>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) original_raw: bool,
}
impl TransactionManifest {
    pub(super) fn original_targets(&self) -> Option<Vec<OriginalTarget>> {
        (self.schema_version == OWNED_TRANSACTION_SCHEMA_VERSION).then(|| {
            self.operations
                .iter()
                .map(|op| OriginalTarget {
                    target_path: op.target_path.clone(),
                    original_size: op.original_size,
                    original_sha256: op.original_sha256.clone(),
                    original_schema_version: op.original_schema_version,
                    original_raw: op.original_raw,
                })
                .collect()
        })
    }
}

/// 이 marker가 유효하고 durable할 때만 전체 새 상태가 commit됐다고 판정한다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommittedMarker {
    pub(crate) schema_version: SchemaVersion,
    pub(crate) transaction_id: TransactionId,
    pub(crate) completed_at_utc: String,
    pub(crate) project_fingerprint: String,
}

impl CommittedMarker {
    pub(crate) fn validate(&self) -> TransactionModelResult<()> {
        validate_marker(
            self.schema_version,
            &self.transaction_id,
            &self.completed_at_utc,
            &self.project_fingerprint,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RolledBackMarker {
    pub(crate) schema_version: SchemaVersion,
    pub(crate) transaction_id: TransactionId,
    pub(crate) completed_at_utc: String,
    pub(crate) project_fingerprint: String,
}

impl RolledBackMarker {
    pub(crate) fn validate(&self) -> TransactionModelResult<()> {
        validate_marker(
            self.schema_version,
            &self.transaction_id,
            &self.completed_at_utc,
            &self.project_fingerprint,
        )
    }
}

fn validate_marker(
    schema_version: SchemaVersion,
    transaction_id: &TransactionId,
    completed_at_utc: &str,
    project_fingerprint: &str,
) -> TransactionModelResult<()> {
    validate_schema(schema_version)?;
    validate_common(transaction_id, completed_at_utc, project_fingerprint)
}

fn validate_schema(schema_version: SchemaVersion) -> TransactionModelResult<()> {
    if schema_version != TRANSACTION_SCHEMA_VERSION
        && schema_version != OWNED_TRANSACTION_SCHEMA_VERSION
    {
        return Err(TransactionModelError::UnsupportedTransactionSchema {
            found: schema_version.get(),
            supported: OWNED_TRANSACTION_SCHEMA_VERSION.get(),
        });
    }
    Ok(())
}

fn validate_common(
    transaction_id: &TransactionId,
    timestamp: &str,
    project_fingerprint: &str,
) -> TransactionModelResult<()> {
    if !is_utc_milliseconds(timestamp) {
        return Err(TransactionModelError::invalid_journal(
            Some(transaction_id),
            "timestamp",
            "time must be RFC 3339 UTC with exactly three milliseconds and Z",
        ));
    }
    if !is_lowercase_sha256(project_fingerprint) {
        return Err(TransactionModelError::invalid_journal(
            Some(transaction_id),
            "projectFingerprint",
            "fingerprint must be 64 lowercase hexadecimal digits",
        ));
    }
    Ok(())
}
