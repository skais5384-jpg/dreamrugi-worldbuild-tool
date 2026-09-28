use std::{error::Error, fmt};

use sha2::{Digest, Sha256};

use super::migration::{
    inspect_migration, validate_deterministic_migration_output, MigrationDecision, MigrationError,
    MigrationErrorCategory, MigrationRegistry,
};
use super::project_relative_path::ProjectRelativePath;
use super::schema::SchemaVersion;
use super::storage_estimate::TRANSACTION_METADATA_SAFETY_BYTES;

/// 검증된 논리 대상과 읽기 시점의 원본을 소유하는 순수 preflight 입력이다.
pub(crate) struct MigrationBatchInput {
    target: ProjectRelativePath,
    original_bytes: Vec<u8>,
}

impl MigrationBatchInput {
    pub(crate) fn new(target: ProjectRelativePath, original_bytes: Vec<u8>) -> Self {
        Self {
            target,
            original_bytes,
        }
    }
}

/// M1-8B가 적용 직전에 원본이 바뀌지 않았는지 확인할 불변 snapshot이다.
pub(crate) struct ExpectedOriginal {
    byte_length: u64,
    sha256: [u8; 32],
    schema_version: SchemaVersion,
    existed: bool,
}

impl ExpectedOriginal {
    fn from_bytes(
        bytes: &[u8],
        schema_version: SchemaVersion,
    ) -> Result<Self, MigrationBatchError> {
        let byte_length = u64::try_from(bytes.len()).map_err(|source| {
            MigrationBatchError::with_source(
                MigrationBatchErrorCategory::ExpectedOriginalCreationFailure,
                MigrationBatchStage::ExpectedOriginalCreation,
                "original byte length cannot be represented",
                None,
                Some(schema_version),
                None,
                source,
            )
        })?;
        Ok(Self {
            byte_length,
            sha256: Sha256::digest(bytes).into(),
            schema_version,
            // M1-8A 입력은 상위 계층이 기존 파일에서 읽은 원본이라는 계약이다.
            // 실제 존재 여부 재확인은 filesystem을 담당하는 M1-8B에서 수행한다.
            existed: true,
        })
    }

    pub(crate) fn byte_length(&self) -> u64 {
        self.byte_length
    }

    pub(crate) fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    pub(crate) fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    pub(crate) fn existed(&self) -> bool {
        self.existed
    }

    pub(crate) fn matches(&self, bytes: &[u8], schema_version: SchemaVersion) -> bool {
        self.existed
            && u64::try_from(bytes.len()) == Ok(self.byte_length)
            && self.schema_version == schema_version
            && self.sha256.as_slice() == Sha256::digest(bytes).as_slice()
    }
}

/// 한 대상의 검증된 migration 결과다. 임의 생성은 이 모듈 밖에 노출하지 않는다.
pub(crate) struct MigratedEntry {
    target: ProjectRelativePath,
    expected_original: ExpectedOriginal,
    source_version: SchemaVersion,
    target_version: SchemaVersion,
    migrated_bytes: Vec<u8>,
}

impl MigratedEntry {
    pub(crate) fn target(&self) -> &ProjectRelativePath {
        &self.target
    }

    pub(crate) fn expected_original(&self) -> &ExpectedOriginal {
        &self.expected_original
    }

    pub(crate) fn source_version(&self) -> SchemaVersion {
        self.source_version
    }

    pub(crate) fn target_version(&self) -> SchemaVersion {
        self.target_version
    }

    pub(crate) fn migrated_bytes(&self) -> &[u8] {
        &self.migrated_bytes
    }

    /// target·원본 snapshot·검증된 staged bytes를 서로 분리하지 않고 이동한다.
    pub(super) fn into_transaction_parts(
        self,
    ) -> (
        ProjectRelativePath,
        ExpectedOriginal,
        SchemaVersion,
        SchemaVersion,
        Vec<u8>,
    ) {
        (
            self.target,
            self.expected_original,
            self.source_version,
            self.target_version,
            self.migrated_bytes,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MigrationByteEstimate {
    staged_bytes: u64,
    original_backup_bytes: u64,
    metadata_safety_bytes: u64,
    total_bytes: u64,
}

impl MigrationByteEstimate {
    fn try_from_totals(
        staged_bytes: u64,
        original_backup_bytes: u64,
    ) -> Result<Self, MigrationBatchError> {
        let content_bytes = staged_bytes
            .checked_add(original_backup_bytes)
            .ok_or_else(MigrationBatchError::byte_estimate_overflow)?;
        let total_bytes = content_bytes
            .checked_add(TRANSACTION_METADATA_SAFETY_BYTES)
            .ok_or_else(MigrationBatchError::byte_estimate_overflow)?;
        Ok(Self {
            staged_bytes,
            original_backup_bytes,
            metadata_safety_bytes: TRANSACTION_METADATA_SAFETY_BYTES,
            total_bytes,
        })
    }

    pub(crate) fn staged_bytes(self) -> u64 {
        self.staged_bytes
    }

    pub(crate) fn original_backup_bytes(self) -> u64 {
        self.original_backup_bytes
    }

    pub(crate) fn metadata_safety_bytes(self) -> u64 {
        self.metadata_safety_bytes
    }

    pub(crate) fn total_bytes(self) -> u64 {
        self.total_bytes
    }
}

pub(crate) struct MigrationBatch {
    entries: Vec<MigratedEntry>,
    byte_estimate: MigrationByteEstimate,
}

impl MigrationBatch {
    pub(crate) fn entries(&self) -> &[MigratedEntry] {
        &self.entries
    }

    pub(crate) fn byte_estimate(&self) -> MigrationByteEstimate {
        self.byte_estimate
    }

    pub(super) fn into_entries(self) -> Vec<MigratedEntry> {
        self.entries
    }
}

pub(crate) enum MigrationBatchDecision {
    NoMigrationRequired,
    Prepared(MigrationBatch),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationBatchErrorCategory {
    EmptyBatch,
    DuplicateTarget,
    CompatibilityFailure,
    UnsupportedFuture,
    UnsupportedPast,
    InvalidInput,
    MigrationPlanFailure,
    MigrationExecutionFailure,
    MigratedOutputValidationFailure,
    ExpectedOriginalCreationFailure,
    ByteEstimateOverflow,
    BatchInvariantViolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationBatchStage {
    InputValidation,
    CompatibilityInspection,
    MigrationPlanning,
    MigrationExecution,
    OutputValidation,
    ExpectedOriginalCreation,
    ByteEstimation,
    BatchValidation,
}

pub(crate) struct MigrationBatchError {
    category: MigrationBatchErrorCategory,
    stage: MigrationBatchStage,
    detail: &'static str,
    target: Option<ProjectRelativePath>,
    source_version: Option<SchemaVersion>,
    target_version: Option<SchemaVersion>,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl MigrationBatchError {
    pub(crate) fn category(&self) -> MigrationBatchErrorCategory {
        self.category
    }

    fn context(
        category: MigrationBatchErrorCategory,
        stage: MigrationBatchStage,
        detail: &'static str,
        target: Option<ProjectRelativePath>,
        source_version: Option<SchemaVersion>,
        target_version: Option<SchemaVersion>,
    ) -> Self {
        Self {
            category,
            stage,
            detail,
            target,
            source_version,
            target_version,
            source: None,
        }
    }

    fn with_source(
        category: MigrationBatchErrorCategory,
        stage: MigrationBatchStage,
        detail: &'static str,
        target: Option<ProjectRelativePath>,
        source_version: Option<SchemaVersion>,
        target_version: Option<SchemaVersion>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        let mut error = Self::context(
            category,
            stage,
            detail,
            target,
            source_version,
            target_version,
        );
        error.source = Some(Box::new(source));
        error
    }

    fn byte_estimate_overflow() -> Self {
        Self::context(
            MigrationBatchErrorCategory::ByteEstimateOverflow,
            MigrationBatchStage::ByteEstimation,
            "migration byte estimate overflowed",
            None,
            None,
            None,
        )
    }
}

impl fmt::Debug for MigrationBatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 원인 오류에는 parser가 본 사용자 JSON이 있을 수 있으므로 출력하지 않는다.
        formatter
            .debug_struct("MigrationBatchError")
            .field("category", &self.category)
            .field("stage", &self.stage)
            .field("detail", &self.detail)
            .field("target", &self.target)
            .field("source_version", &self.source_version)
            .field("target_version", &self.target_version)
            .field("has_source", &self.source.is_some())
            .finish()
    }
}

impl fmt::Display for MigrationBatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "migration batch preflight failed ({:?}) at {:?}",
            self.category, self.stage
        )?;
        if let (Some(source), Some(target)) = (self.source_version, self.target_version) {
            write!(formatter, " for {} -> {}", source.get(), target.get())?;
        }
        write!(formatter, ": {}", self.detail)
    }
}

impl Error for MigrationBatchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

/// 모든 입력을 먼저 소유하고 검증한 뒤, 성공한 경우에만 완성된 batch를 반환한다.
pub(crate) fn preflight_migration_batch<'steps>(
    mut inputs: Vec<MigrationBatchInput>,
    registry: MigrationRegistry<'steps>,
) -> Result<MigrationBatchDecision, MigrationBatchError> {
    if inputs.is_empty() {
        return Err(MigrationBatchError::context(
            MigrationBatchErrorCategory::EmptyBatch,
            MigrationBatchStage::InputValidation,
            "migration batch input is empty",
            None,
            None,
            None,
        ));
    }

    inputs.sort_by(|left, right| left.target.cmp(&right.target));
    if let Some(duplicate) = inputs
        .windows(2)
        .find(|pair| pair[0].target == pair[1].target)
    {
        return Err(MigrationBatchError::context(
            MigrationBatchErrorCategory::DuplicateTarget,
            MigrationBatchStage::InputValidation,
            "migration batch contains duplicate logical targets",
            Some(duplicate[1].target.clone()),
            None,
            None,
        ));
    }

    let mut entries = Vec::new();
    let mut staged_bytes = 0_u64;
    let mut backup_bytes = 0_u64;
    for input in inputs {
        let decision = inspect_migration(&input.original_bytes, registry)
            .map_err(|source| map_inspection_error(input.target.clone(), source))?;
        match decision {
            MigrationDecision::Current { .. } => {}
            MigrationDecision::Required(plan) => {
                let result = plan.execute(&input.original_bytes).map_err(|source| {
                    map_execution_error(input.target.clone(), registry.current(), source)
                })?;
                if result.source_version >= result.final_version
                    || result.final_version != registry.current()
                {
                    return Err(MigrationBatchError::context(
                        MigrationBatchErrorCategory::BatchInvariantViolation,
                        MigrationBatchStage::BatchValidation,
                        "migration result versions violate batch invariants",
                        Some(input.target),
                        Some(result.source_version),
                        Some(result.final_version),
                    ));
                }
                validate_deterministic_migration_output(&result.bytes, result.final_version)
                    .map_err(|source| {
                        MigrationBatchError::with_source(
                            MigrationBatchErrorCategory::MigratedOutputValidationFailure,
                            MigrationBatchStage::OutputValidation,
                            "migrated output failed strict deterministic validation",
                            Some(input.target.clone()),
                            Some(result.source_version),
                            Some(result.final_version),
                            source,
                        )
                    })?;
                let expected_original =
                    ExpectedOriginal::from_bytes(&input.original_bytes, result.source_version)
                        .map_err(|mut error| {
                            error.target = Some(input.target.clone());
                            error
                        })?;
                let migrated_length = u64::try_from(result.bytes.len())
                    .map_err(|_| MigrationBatchError::byte_estimate_overflow())?;
                staged_bytes = staged_bytes
                    .checked_add(migrated_length)
                    .ok_or_else(MigrationBatchError::byte_estimate_overflow)?;
                backup_bytes = backup_bytes
                    .checked_add(expected_original.byte_length)
                    .ok_or_else(MigrationBatchError::byte_estimate_overflow)?;
                entries.push(MigratedEntry {
                    target: input.target,
                    expected_original,
                    source_version: result.source_version,
                    target_version: result.final_version,
                    migrated_bytes: result.bytes,
                });
            }
        }
    }

    if entries.is_empty() {
        return Ok(MigrationBatchDecision::NoMigrationRequired);
    }
    let byte_estimate = MigrationByteEstimate::try_from_totals(staged_bytes, backup_bytes)?;
    Ok(MigrationBatchDecision::Prepared(MigrationBatch {
        entries,
        byte_estimate,
    }))
}

fn map_inspection_error(
    target: ProjectRelativePath,
    source: MigrationError,
) -> MigrationBatchError {
    let (source_version, target_version) = source.context_versions();
    let (category, stage) = match source.category() {
        MigrationErrorCategory::UnsupportedFuture => (
            MigrationBatchErrorCategory::UnsupportedFuture,
            MigrationBatchStage::CompatibilityInspection,
        ),
        MigrationErrorCategory::UnsupportedPast => (
            MigrationBatchErrorCategory::UnsupportedPast,
            MigrationBatchStage::CompatibilityInspection,
        ),
        MigrationErrorCategory::InvalidInputJson | MigrationErrorCategory::DuplicateJsonKey => (
            MigrationBatchErrorCategory::InvalidInput,
            MigrationBatchStage::CompatibilityInspection,
        ),
        MigrationErrorCategory::InvalidRegistry | MigrationErrorCategory::PlanUnavailable => (
            MigrationBatchErrorCategory::MigrationPlanFailure,
            MigrationBatchStage::MigrationPlanning,
        ),
        _ => (
            MigrationBatchErrorCategory::CompatibilityFailure,
            MigrationBatchStage::CompatibilityInspection,
        ),
    };
    MigrationBatchError::with_source(
        category,
        stage,
        "project could not enter migration preflight",
        Some(target),
        source_version,
        target_version,
        source,
    )
}

fn map_execution_error(
    target: ProjectRelativePath,
    target_version: SchemaVersion,
    source: MigrationError,
) -> MigrationBatchError {
    let (source_version, error_target_version) = source.context_versions();
    let category = match source.category() {
        MigrationErrorCategory::OutputValidationFailure
        | MigrationErrorCategory::OutputVersionMismatch
        | MigrationErrorCategory::FinalVersionMismatch
        | MigrationErrorCategory::SerializationFailure
        | MigrationErrorCategory::DeterministicOutputRevalidationFailure => {
            MigrationBatchErrorCategory::MigratedOutputValidationFailure
        }
        _ => MigrationBatchErrorCategory::MigrationExecutionFailure,
    };
    MigrationBatchError::with_source(
        category,
        MigrationBatchStage::MigrationExecution,
        "a migration step failed",
        Some(target),
        source_version,
        error_target_version.or(Some(target_version)),
        source,
    )
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::data::migration::{production_registry, MigrationStep, MigrationStepError};

    fn path(value: &str) -> ProjectRelativePath {
        ProjectRelativePath::parse(value).expect("test path should be valid")
    }

    fn input(target: &str, json: &str) -> MigrationBatchInput {
        MigrationBatchInput::new(path(target), json.as_bytes().to_vec())
    }

    fn validate_version(value: &Value, expected: u64) -> Result<(), MigrationStepError> {
        if value.get("schemaVersion").and_then(Value::as_u64) == Some(expected) {
            Ok(())
        } else {
            Err(MigrationStepError::new("unexpected test schema version"))
        }
    }

    fn validate_v1(value: &Value) -> Result<(), MigrationStepError> {
        validate_version(value, 1)
    }

    fn validate_v2(value: &Value) -> Result<(), MigrationStepError> {
        validate_version(value, 2)
    }

    fn validate_v3(value: &Value) -> Result<(), MigrationStepError> {
        validate_version(value, 3)
    }

    fn v1_to_v2(mut value: Value) -> Result<Value, MigrationStepError> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| MigrationStepError::new("test root must be object"))?;
        object.insert("schemaVersion".into(), json!(2));
        object.insert("migratedToV2".into(), json!(true));
        Ok(value)
    }

    fn v2_to_v3(mut value: Value) -> Result<Value, MigrationStepError> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| MigrationStepError::new("test root must be object"))?;
        if object.get("failMigration") == Some(&Value::Bool(true)) {
            return Err(MigrationStepError::new("synthetic intermediate failure"));
        }
        object.insert("schemaVersion".into(), json!(3));
        object.insert("migratedToV3".into(), json!(true));
        Ok(value)
    }

    static STEPS: [MigrationStep; 2] = [
        MigrationStep::new(1, 2, v1_to_v2, validate_v1, validate_v2),
        MigrationStep::new(2, 3, v2_to_v3, validate_v2, validate_v3),
    ];

    fn registry() -> MigrationRegistry<'static> {
        MigrationRegistry::try_new(3, &STEPS).expect("test registry should be valid")
    }

    fn prepared(decision: MigrationBatchDecision) -> MigrationBatch {
        match decision {
            MigrationBatchDecision::Prepared(batch) => batch,
            MigrationBatchDecision::NoMigrationRequired => panic!("migration should be required"),
        }
    }

    #[test]
    fn rejects_empty_and_duplicate_logical_targets() {
        let empty = preflight_migration_batch(Vec::new(), registry())
            .err()
            .expect("empty batch must fail");
        assert_eq!(empty.category(), MigrationBatchErrorCategory::EmptyBatch);

        let duplicate = preflight_migration_batch(
            vec![
                input("data/a.json", r#"{"schemaVersion":1}"#),
                input(r"data\a.json", r#"{"schemaVersion":2}"#),
            ],
            registry(),
        )
        .err()
        .expect("normalized aliases must fail");
        assert_eq!(
            duplicate.category(),
            MigrationBatchErrorCategory::DuplicateTarget
        );

        let exact_duplicate = preflight_migration_batch(
            vec![
                input("same.json", r#"{"schemaVersion":1}"#),
                input("same.json", r#"{"schemaVersion":1}"#),
            ],
            registry(),
        )
        .err()
        .expect("exact duplicate targets must fail");
        assert_eq!(
            exact_duplicate.category(),
            MigrationBatchErrorCategory::DuplicateTarget
        );
    }

    #[cfg(windows)]
    #[test]
    fn rejects_ascii_case_aliases_on_windows() {
        let error = preflight_migration_batch(
            vec![
                input("DATA/A.json", r#"{"schemaVersion":1}"#),
                input("data/a.json", r#"{"schemaVersion":2}"#),
            ],
            registry(),
        )
        .err()
        .expect("Windows case aliases must fail");
        assert_eq!(
            error.category(),
            MigrationBatchErrorCategory::DuplicateTarget
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn preserves_case_sensitive_targets_on_non_windows() {
        let batch = prepared(
            preflight_migration_batch(
                vec![
                    input("DATA/A.json", r#"{"schemaVersion":1}"#),
                    input("data/a.json", r#"{"schemaVersion":2}"#),
                ],
                registry(),
            )
            .expect("case-distinct targets should be accepted"),
        );
        assert_eq!(batch.entries().len(), 2);
    }

    #[test]
    fn sorts_entries_and_excludes_current_inputs_without_reserializing_them() {
        let current = b"{ \"schemaVersion\" : 3, \"untouched\" : true }".to_vec();
        let batch = prepared(
            preflight_migration_batch(
                vec![
                    MigrationBatchInput::new(path("현재.json"), current.clone()),
                    input("z.json", r#"{"schemaVersion":2,"name":"끝"}"#),
                    input("a.json", r#"{"schemaVersion":1,"name":"가"}"#),
                ],
                registry(),
            )
            .expect("mixed batch should succeed"),
        );
        assert_eq!(current, b"{ \"schemaVersion\" : 3, \"untouched\" : true }");
        assert_eq!(batch.entries().len(), 2);
        assert_eq!(batch.entries()[0].target().as_str(), "a.json");
        assert_eq!(batch.entries()[1].target().as_str(), "z.json");
        assert!(batch
            .entries()
            .iter()
            .all(|entry| entry.target().as_str() != "현재.json"));
    }

    #[test]
    fn all_current_returns_no_migration_and_production_cannot_prepare_v1() {
        let decision =
            preflight_migration_batch(vec![input("a.json", r#"{"schemaVersion":3}"#)], registry())
                .expect("current test batch should succeed");
        assert!(matches!(
            decision,
            MigrationBatchDecision::NoMigrationRequired
        ));

        let production = preflight_migration_batch(
            vec![input("project.json", r#"{"schemaVersion":1}"#)],
            production_registry().expect("production registry should be valid"),
        )
        .expect("current production input should succeed");
        assert!(matches!(
            production,
            MigrationBatchDecision::NoMigrationRequired
        ));
    }

    #[test]
    fn preserves_snapshot_hash_versions_unknown_fields_korean_and_array_order() {
        let original =
            r#"{"schemaVersion":1,"unknown":{"k":"값"},"items":["둘","하나"]}"#.as_bytes();
        let batch = prepared(
            preflight_migration_batch(
                vec![MigrationBatchInput::new(
                    path("data.json"),
                    original.to_vec(),
                )],
                registry(),
            )
            .expect("migration should succeed"),
        );
        let entry = &batch.entries()[0];
        let expected = entry.expected_original();
        assert_eq!(expected.byte_length(), original.len() as u64);
        assert_eq!(
            expected.sha256(),
            &<[u8; 32]>::from(Sha256::digest(original))
        );
        assert!(expected.existed());
        assert!(expected.matches(original, SchemaVersion::try_from(1).unwrap()));
        assert!(!expected.matches(b"changed", SchemaVersion::try_from(1).unwrap()));
        assert_eq!(expected.schema_version(), entry.source_version());
        assert_eq!(entry.source_version().get(), 1);
        assert_eq!(entry.target_version().get(), 3);

        let output = std::str::from_utf8(entry.migrated_bytes()).unwrap();
        let output_value: Value = serde_json::from_str(output).unwrap();
        assert_eq!(output_value["unknown"]["k"], "값");
        assert_eq!(output_value["items"], json!(["둘", "하나"]));
        assert!(output.ends_with('\n'));
        assert!(!output.ends_with("\n\n"));
        assert!(!entry.migrated_bytes().starts_with(&[0xef, 0xbb, 0xbf]));
    }

    #[test]
    fn repeated_preflight_is_byte_identical_and_estimate_counts_only_migrations() {
        let make_inputs = || {
            vec![
                input("old.json", r#"{"schemaVersion":2,"한글":true}"#),
                input("current.json", r#"{"schemaVersion":3,"large":"ignored"}"#),
            ]
        };
        let first = prepared(preflight_migration_batch(make_inputs(), registry()).unwrap());
        let second = prepared(preflight_migration_batch(make_inputs(), registry()).unwrap());
        assert_eq!(
            first.entries()[0].migrated_bytes(),
            second.entries()[0].migrated_bytes()
        );
        let estimate = first.byte_estimate();
        assert_eq!(
            estimate.staged_bytes(),
            first.entries()[0].migrated_bytes().len() as u64
        );
        assert_eq!(
            estimate.original_backup_bytes(),
            first.entries()[0].expected_original().byte_length()
        );
        assert_eq!(
            estimate.metadata_safety_bytes(),
            TRANSACTION_METADATA_SAFETY_BYTES
        );
        assert_eq!(
            estimate.total_bytes(),
            estimate.staged_bytes()
                + estimate.original_backup_bytes()
                + estimate.metadata_safety_bytes()
        );
    }

    #[test]
    fn caller_order_does_not_change_entries_bytes_or_estimate() {
        let first = prepared(
            preflight_migration_batch(
                vec![
                    input("z.json", r#"{"schemaVersion":2,"value":"z"}"#),
                    input("a.json", r#"{"schemaVersion":1,"value":"a"}"#),
                ],
                registry(),
            )
            .unwrap(),
        );
        let second = prepared(
            preflight_migration_batch(
                vec![
                    input("a.json", r#"{"schemaVersion":1,"value":"a"}"#),
                    input("z.json", r#"{"schemaVersion":2,"value":"z"}"#),
                ],
                registry(),
            )
            .unwrap(),
        );

        assert_eq!(first.byte_estimate(), second.byte_estimate());
        for (left, right) in first.entries().iter().zip(second.entries()) {
            assert_eq!(left.target(), right.target());
            assert_eq!(left.source_version(), right.source_version());
            assert_eq!(left.migrated_bytes(), right.migrated_bytes());
        }
    }

    #[test]
    fn expected_original_rejects_each_mutation_and_repeats_hash_for_korean_bytes() {
        let original = r#"{"schemaVersion":1,"name":"한글"}"#.as_bytes();
        let first = ExpectedOriginal::from_bytes(original, SchemaVersion::try_from(1).unwrap())
            .expect("snapshot should be created");
        let second = ExpectedOriginal::from_bytes(original, SchemaVersion::try_from(1).unwrap())
            .expect("same snapshot should be created");
        assert_eq!(first.sha256(), second.sha256());

        let same_length = r#"{"schemaVersion":1,"name":"한굴"}"#.as_bytes();
        assert_eq!(same_length.len(), original.len());
        assert!(!first.matches(same_length, SchemaVersion::try_from(1).unwrap()));
        assert!(!first.matches(b"short", SchemaVersion::try_from(1).unwrap()));

        let version_changed = r#"{"schemaVersion":2,"name":"한글"}"#.as_bytes();
        assert!(!first.matches(version_changed, SchemaVersion::try_from(2).unwrap()));
        assert!(!first.matches(original, SchemaVersion::try_from(2).unwrap()));
    }

    #[test]
    fn rejects_future_past_malformed_and_intermediate_failure_without_partial_batch() {
        let future = preflight_migration_batch(
            vec![input("future.json", r#"{"schemaVersion":4}"#)],
            registry(),
        )
        .err()
        .expect("future version must fail");
        assert_eq!(
            future.category(),
            MigrationBatchErrorCategory::UnsupportedFuture
        );

        let only_v2_to_v3 = [STEPS[1]];
        let past_registry = MigrationRegistry::try_new(3, &only_v2_to_v3).unwrap();
        let past = preflight_migration_batch(
            vec![input("past.json", r#"{"schemaVersion":1}"#)],
            past_registry,
        )
        .err()
        .expect("unsupported past must fail");
        assert_eq!(
            past.category(),
            MigrationBatchErrorCategory::UnsupportedPast
        );

        let malformed = preflight_migration_batch(
            vec![input(
                "invalid.json",
                r#"{"schemaVersion":1,"secret":"본문""#,
            )],
            registry(),
        )
        .err()
        .expect("malformed JSON must fail");
        assert_eq!(
            malformed.category(),
            MigrationBatchErrorCategory::InvalidInput
        );
        assert!(!malformed.to_string().contains("본문"));
        assert!(!format!("{malformed:?}").contains("본문"));

        let first_original = br#"{"schemaVersion":1}"#.to_vec();
        let failing_original = br#"{"schemaVersion":2,"failMigration":true}"#.to_vec();
        let failure = preflight_migration_batch(
            vec![
                MigrationBatchInput::new(path("a.json"), first_original.clone()),
                MigrationBatchInput::new(path("b.json"), failing_original.clone()),
            ],
            registry(),
        )
        .err()
        .expect("one failure must reject the whole batch");
        assert_eq!(
            failure.category(),
            MigrationBatchErrorCategory::MigrationExecutionFailure
        );
        assert!(failure.to_string().contains("2 -> 3"));
        assert_eq!(first_original, br#"{"schemaVersion":1}"#);
        assert_eq!(
            failing_original,
            br#"{"schemaVersion":2,"failMigration":true}"#
        );
    }

    #[test]
    fn checked_estimate_rejects_each_overflow_boundary() {
        assert_eq!(
            MigrationByteEstimate::try_from_totals(u64::MAX, 1)
                .expect_err("content sum must overflow")
                .category(),
            MigrationBatchErrorCategory::ByteEstimateOverflow
        );
        assert_eq!(
            MigrationByteEstimate::try_from_totals(
                u64::MAX - TRANSACTION_METADATA_SAFETY_BYTES + 1,
                0,
            )
            .expect_err("metadata addition must overflow")
            .category(),
            MigrationBatchErrorCategory::ByteEstimateOverflow
        );
    }
}
